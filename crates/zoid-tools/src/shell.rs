use crate::{str_arg, KillSlot, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use zoid_provider::ToolSpec;

/// Run a shell command in the working directory and capture its output.
/// (Chat is safe by human presence, spec §9 — no sandbox.)
///
/// On unix the child is spawned in its own process group and its pgid is
/// published to a shared [`KillSlot`], so a hard-stop can SIGKILL the whole
/// tree (the shell plus any grandchildren it spawned).
#[derive(Default)]
pub struct Shell {
    kill: KillSlot,
}

impl Shell {
    pub fn new(kill: KillSlot) -> Self {
        Self { kill }
    }
}

impl Tool for Shell {
    fn name(&self) -> &str {
        "shell"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Run a shell command in the working directory; returns stdout, stderr, and exit code.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "command": { "type": "string", "description": "Command line to execute." } },
                "required": ["command"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let command = match str_arg(args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };

        let output = self.spawn_and_wait(&command, cwd);
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                let code = o.status.code().unwrap_or(-1);
                let mut text = String::new();
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&format!("[exit {code}]"));
                ToolOutput {
                    text,
                    is_error: code != 0,
                    diff: None,
                }
            }
            Err(e) => ToolOutput::err(format!("shell({command}): {e}")),
        }
    }
}

impl Shell {
    /// Unix: spawn in a fresh process group, publish the pgid to the kill slot,
    /// wait for output, then clear the slot. Non-unix: the previous
    /// fire-and-collect behavior (no killability).
    #[cfg(unix)]
    fn spawn_and_wait(&self, command: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
        use std::os::unix::process::CommandExt;
        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0) // child's pid becomes its pgid
            .spawn()?;
        self.kill.register(child.id()); // id() read before wait_with_output moves child
        let out = child.wait_with_output(); // reads piped stdout/stderr, then waits
        self.kill.clear();
        out
    }

    #[cfg(not(unix))]
    fn spawn_and_wait(&self, command: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
        Command::new("cmd")
            .arg("/C")
            .arg(command)
            .current_dir(cwd)
            .output()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_command_captures_stdout_and_exit() {
        let out = Shell::default().run(
            &json!({ "command": "echo hello-zoid" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("hello-zoid"));
        assert!(out.text.contains("[exit 0]"));
    }

    #[test]
    fn captures_stderr() {
        let out = Shell::default().run(
            &json!({ "command": "echo oops 1>&2; exit 1" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(
            out.text.contains("oops"),
            "stderr should be captured: {}",
            out.text
        );
        assert!(out.text.contains("[exit 1]"));
    }

    #[test]
    fn nonzero_exit_is_error() {
        let out = Shell::default().run(&json!({ "command": "exit 3" }), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("[exit 3]"));
    }

    #[test]
    fn missing_command_is_error() {
        let out = Shell::default().run(&json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("command"));
    }

    #[cfg(unix)]
    #[test]
    fn hard_kill_terminates_process_group_including_grandchildren() {
        use crate::KillSlot;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("SENTINEL");
        let kill = KillSlot::new();
        let shell = Shell::new(kill.clone());
        // A backgrounded grandchild that would write the sentinel after 3s,
        // and a parent that also sleeps. Under non-interactive `sh` the
        // background job shares the shell's process group, so a group-kill
        // must stop the grandchild before it can touch the sentinel.
        let cmd = format!("(sleep 3; touch {}) & sleep 3", sentinel.display());
        let dir_path = dir.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            shell.run(&serde_json::json!({ "command": cmd }), &dir_path)
        });
        // Wait until the child has registered its pgid, then kill the group.
        let mut waited = 0;
        while kill.pgid().is_none() && waited < 2000 {
            std::thread::sleep(Duration::from_millis(10));
            waited += 10;
        }
        assert!(kill.pgid().is_some(), "shell must register its pgid");
        kill.kill();
        let _ = handle.join().unwrap(); // wait must return promptly post-kill
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !sentinel.exists(),
            "group kill must prevent the grandchild from writing the sentinel"
        );
        // Slot is cleared once run() returns.
        assert_eq!(kill.pgid(), None);
    }
}
