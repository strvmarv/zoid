use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct DispatchSubagent;

impl Tool for DispatchSubagent {
    fn name(&self) -> &str {
        "dispatch_subagent"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "dispatch_subagent".into(),
            description: "Dispatch a subagent to execute a task in isolation. Returns the subagent's \
                          ID immediately; the result arrives later as a DelegationResult event. Use \
                          worktree: true for file isolation when subagents might edit the same files."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The task description for the subagent" },
                    "agent": { "type": "string", "description": "The agent profile name to use (default: 'delegate'). Call list_agents to see available agents.", "default": "delegate" },
                    "worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false }
                },
                "required": ["task"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("dispatch_subagent is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_subagent_spec_and_kind() {
        assert_eq!(DispatchSubagent.name(), "dispatch_subagent");
        assert_eq!(DispatchSubagent.spec().name, "dispatch_subagent");
        assert_eq!(DispatchSubagent.kind(), ToolKind::Emitting);
        let params = DispatchSubagent.spec().parameters;
        assert_eq!(params["required"][0], "task");
        assert_eq!(params["properties"]["agent"]["type"], "string");
        assert_eq!(params["properties"]["agent"]["default"], "delegate");
        assert!(
            params["properties"]["worktree"]["default"].is_boolean(),
            "worktree default must remain boolean"
        );
        assert!(
            params["properties"].get("model").is_none(),
            "model must not be in the dispatch_subagent spec — subagents inherit the session model"
        );
    }
}