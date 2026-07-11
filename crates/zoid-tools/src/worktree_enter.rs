use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `enter_worktree { name }` — an Emitting tool that requests the main loop to
/// create and enter a persistent git worktree. The loop performs the actual
/// relocation between turns (spec: chat-worktree-design).
pub struct EnterWorktree;

impl Tool for EnterWorktree {
    fn name(&self) -> &str {
        "enter_worktree"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "enter_worktree".into(),
            description: "Create and enter an isolated git worktree. All subsequent \
                          tool calls and file operations will run inside the worktree \
                          directory. Use exit_worktree to return to the main checkout."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The branch name for the worktree (also used as the directory name under .zoid/worktrees/)"
                    }
                },
                "required": ["name"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("enter_worktree is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(EnterWorktree.name(), "enter_worktree");
        assert_eq!(EnterWorktree.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_name_param_required() {
        let spec = EnterWorktree.spec();
        assert_eq!(spec.name, "enter_worktree");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("name")));
    }
}