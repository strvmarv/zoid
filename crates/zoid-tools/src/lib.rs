//! zoid-tools — the curated, cwd-scoped tool set the agent loop can call.
//! Tools run in the process working directory (Chat is safe by human presence,
//! spec §9); no path-jailing here.

pub mod edit;
pub mod read;
pub mod search;
pub mod shell;
pub mod tasks;
pub mod write;

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
}

/// A callable tool. `spec()` is sent to the provider; `run()` executes it.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput;
    /// The execution kind (see [`ToolKind`]). Defaults to `Local`; the five
    /// built-in tools do not override it.
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
        Box::new(shell::Shell),
        Box::new(tasks::UpdateTasks),
    ]
}

/// The decision a [`ToolGate`] returns for a pending tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    /// Block the call; the string is fed back to the model as the tool result.
    Deny(String),
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
        // `update_tasks` is the sole intentional exception (ToolKind::Emitting);
        // everything else still defaults to Local.
        for t in registry()
            .into_iter()
            .filter(|t| t.name() != "update_tasks")
        {
            assert_eq!(
                t.kind(),
                ToolKind::Local,
                "{} should default to Local",
                t.name()
            );
        }
    }
}
