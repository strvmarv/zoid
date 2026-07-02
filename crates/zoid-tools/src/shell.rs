use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use zoid_provider::ToolSpec;

/// Run a shell command in the working directory and capture its output.
/// (Chat is safe by human presence, spec §9 — no sandbox.)
pub struct Shell;

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

        let output = if cfg!(windows) {
            Command::new("cmd")
                .arg("/C")
                .arg(&command)
                .current_dir(cwd)
                .output()
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(cwd)
                .output()
        };
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
                }
            }
            Err(e) => ToolOutput::err(format!("shell({command}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_command_captures_stdout_and_exit() {
        let out = Shell.run(
            &json!({ "command": "echo hello-zoid" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("hello-zoid"));
        assert!(out.text.contains("[exit 0]"));
    }

    #[test]
    fn captures_stderr() {
        let out = Shell.run(
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
        let out = Shell.run(&json!({ "command": "exit 3" }), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("[exit 3]"));
    }

    #[test]
    fn missing_command_is_error() {
        let out = Shell.run(&json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("command"));
    }
}
