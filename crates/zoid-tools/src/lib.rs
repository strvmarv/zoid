//! zoid-tools — the curated, cwd-scoped tool set the agent loop can call.
//! Tools run in the process working directory (Chat is safe by human presence,
//! spec §9); no path-jailing here.

pub mod ask;
pub mod approval;
pub mod edit;
pub mod kill;
pub mod read;
pub mod recall;
pub mod search;
pub mod subagent_dispatch;
pub mod shell;
pub mod show;
pub mod tasks;
pub mod write;
pub mod subagent_diff;

use serde_json::Value;
use std::path::{Path, PathBuf};
use zoid_provider::{ToolCall, ToolSpec};

/// The outcome of running a tool. `text` is fed back to the model as the tool
/// result; `is_error` marks failures (still returned to the model, not panicked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
    pub fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

pub use kill::KillSlot;

/// How the agent loop must execute a tool. `Local` tools run synchronously in
/// the working directory (the v1 default). `Emitting` tools append a domain
/// event instead of doing I/O. `Interactive` tools suspend the loop to collect
/// input from the UI. The loop branches on this BEFORE calling `run()`, so only
/// `Local` tools ever have `run()` invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Local,
    Emitting,
    Interactive,
    /// Routed to an MCP server over async I/O; intercepted by the agent loop
    /// before the synchronous path, so `run()` is never called (like Emitting).
    Mcp,
}

/// A callable tool. `spec()` is sent to the provider; `run()` executes it.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput;
    /// The execution kind (see [`ToolKind`]). Defaults to `Local`;
    /// `update_tasks` overrides to `Emitting` and `ask_user` to `Interactive`.
    fn kind(&self) -> ToolKind {
        ToolKind::Local
    }
}

/// The compiled-in tool set (spec §9: fixed curated set in v1).
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::ReadFile),
        Box::new(write::WriteFile),
        Box::new(edit::EditFile),
        Box::new(search::Search),
        Box::new(shell::Shell::default()),
        Box::new(tasks::UpdateTasks),
        Box::new(ask::AskUser),
    ]
}

/// Like [`registry`] but the `shell` tool carries a shared [`KillSlot`] so a
/// hard-stop can kill its process group. Used by the chat turn; subagents and
/// tests use the zero-arg `registry()` (their shell is not hard-killable).
pub fn registry_with_kill(kill: KillSlot) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::ReadFile),
        Box::new(write::WriteFile),
        Box::new(edit::EditFile),
        Box::new(search::Search),
        Box::new(shell::Shell::new(kill)),
        Box::new(tasks::UpdateTasks),
        Box::new(ask::AskUser),
    ]
}

/// The decision a [`ToolGate`] returns for a pending tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    /// Block the call; the string is fed back to the model as the tool result.
    Deny(String),
    /// Request an interactive approval from the user. The agent loop reuses
    /// the existing `ask_user` oneshot + `AgentUpdate::AskUser` park-and-await
    /// path to suspend and resume on the user's answer. `question` is shown in
    /// the question overlay; `choices` are the selectable options.
    Prompt {
        question: String,
        choices: Vec<String>,
    },
}

/// Consulted once per pending tool call, immediately before dispatch. v1 ships
/// only [`AllowAll`]; this is the insertion point where interactive tool
/// approval will later live (an `ask_user` prompt gating `Deny`).
pub trait ToolGate: Send + Sync {
    fn check(&self, call: &ToolCall) -> Gate;
}

/// The v1 gate: every tool call is allowed.
pub struct AllowAll;
impl ToolGate for AllowAll {
    fn check(&self, _call: &ToolCall) -> Gate {
        Gate::Allow
    }
}

pub use approval::BlacklistGate;

/// Dispatch a tool call by name. Unknown tools return an error `ToolOutput`
/// (the model sees it and can recover) rather than panicking.
pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value, cwd: &Path) -> ToolOutput {
    match tools.iter().find(|t| t.name() == name) {
        Some(t) => t.run(args, cwd),
        None => ToolOutput::err(format!("unknown tool: {name}")),
    }
}

/// Helper for tools: pull a required string argument.
pub(crate) fn str_arg(args: &Value, key: &str) -> Result<String, ToolOutput> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolOutput::err(format!("missing or non-string argument: {key}")))
}

/// Resolve a tool's path argument against the run's working directory.
/// Relative paths join `cwd`; absolute paths pass through. For subagent
/// relocation, NOT a security jail (spec §9: no path-jailing).
pub(crate) fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_has_unique_named_tools() {
        let reg = registry();
        let mut names: Vec<&str> = reg.iter().map(|t| t.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "tool names must be unique");
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"update_tasks"));
        assert!(names.contains(&"ask_user"));
    }

    #[test]
    fn unknown_tool_is_error_not_panic() {
        let reg = registry();
        let out = run_tool(&reg, "nope", &json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("unknown tool"));
    }

    #[test]
    fn resolve_joins_relative_and_passes_absolute() {
        use std::path::Path;
        assert_eq!(
            resolve(Path::new("/work"), "src/a.rs"),
            Path::new("/work/src/a.rs")
        );
        assert_eq!(
            resolve(Path::new("/work"), "/etc/hosts"),
            Path::new("/etc/hosts")
        );
    }

    #[test]
    fn read_tool_resolves_relative_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "in cwd").unwrap();
        let out = crate::read::ReadFile.run(&serde_json::json!({ "path": "note.txt" }), dir.path());
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "in cwd");
    }

    #[test]
    fn allow_all_allows_every_call() {
        let g = AllowAll;
        let call = zoid_provider::ToolCall {
            id: String::new(),
            name: "shell".into(),
            args: json!({}),
        };
        assert_eq!(g.check(&call), Gate::Allow);
    }

    #[test]
    fn registry_tools_are_all_local_by_default() {
        // `update_tasks` (Emitting) and `ask_user` (Interactive) are the
        // intentional exceptions; everything else still defaults to Local.
        for t in registry()
            .into_iter()
            .filter(|t| t.name() != "update_tasks" && t.name() != "ask_user")
        {
            assert_eq!(
                t.kind(),
                ToolKind::Local,
                "{} should default to Local",
                t.name()
            );
        }
    }

    #[test]
    fn registry_excludes_chat_only_tools() {
        let reg = registry();
        assert!(
            !reg.iter().any(|t| t.name() == "dispatch_subagent"),
            "dispatch_subagent must not be in base registry (subagents can't dispatch)"
        );
        assert!(
            !reg.iter().any(|t| t.name() == "subagent_diff"),
            "subagent_diff must not be in base registry"
        );
    }
}
