use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `exit_worktree {}` — an Emitting tool that requests the main loop to leave
/// the current worktree, restoring the prior working directory. The loop
/// prompts keep/remove if the worktree has uncommitted changes.
pub struct ExitWorktree;

impl Tool for ExitWorktree {
    fn name(&self) -> &str {
        "exit_worktree"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exit_worktree".into(),
            description: "Exit the current git worktree and return to the main \
                          checkout. If the worktree has uncommitted changes, it \
                          will be kept on disk for manual cleanup. If the branch \
                          has commits not yet merged to HEAD, the branch ref is \
                          retained (the worktree directory is still removed) and \
                          the tool result includes a warning with the branch name."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("exit_worktree is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(ExitWorktree.name(), "exit_worktree");
        assert_eq!(ExitWorktree.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_no_required_params() {
        let spec = ExitWorktree.spec();
        assert_eq!(spec.name, "exit_worktree");
        // No "required" key or empty array.
        if let Some(req) = spec.parameters.get("required").and_then(|r| r.as_array()) {
            assert!(req.is_empty(), "exit_worktree takes no required params");
        }
    }
}