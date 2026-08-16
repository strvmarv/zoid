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
            description: "Fire-and-forget: dispatch a subagent to execute a task in \
                          isolation, then STOP. The result arrives later as a \
                          DelegationResult event that re-invokes you automatically — \
                          never poll for status, never call list_subagents to check \
                          progress, and do not edit files in the main worktree while a \
                          subagent runs (they share the working directory unless \
                          worktree: true). Returns the subagent ID immediately. Up to \
                          max_concurrent subagents (default 3) may run simultaneously — \
                          additional dispatches are queued and start when a slot frees. \
                          Use worktree: true for file isolation when subagents might \
                          edit the same files."
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
        let desc = DispatchSubagent.spec().description;
        assert!(
            desc.starts_with("Fire-and-forget"),
            "description must lead with 'Fire-and-forget' so the no-poll rule is \
             the first thing the model reads, not buried mid-paragraph: {desc}"
        );
        assert!(
            desc.contains("never call list_subagents"),
            "description must explicitly name list_subagents as a do-not-call: {desc}"
        );
    }
}
