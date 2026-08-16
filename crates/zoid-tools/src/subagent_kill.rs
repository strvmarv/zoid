use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `cancel_subagent { id? }` — an Emitting tool the main Chat agent uses to abort
/// a dispatched subagent. `Some(id)` kills one; omitted kills all in-flight.
/// The agent loop performs the cancel against its shared subagent registry; this
/// stub only advertises the tool (its `run` is never called).
pub struct CancelSubagent;

impl Tool for CancelSubagent {
    fn name(&self) -> &str {
        "cancel_subagent"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "cancel_subagent".into(),
            description: "Cancel a dispatched subagent. Pass `id` to cancel one \
                          specific subagent, or omit it to cancel all in-flight \
                          subagents. Aborted subagents report a failure result and \
                          their worktree is discarded."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The subagent id (e.g. sub-01H…) to cancel. Omit to cancel all."
                    }
                },
                "required": []
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("cancel_subagent is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(CancelSubagent.name(), "cancel_subagent");
        assert_eq!(CancelSubagent.kind(), ToolKind::Emitting);
    }

    #[test]
    fn id_is_optional() {
        let spec = CancelSubagent.spec();
        assert_eq!(spec.name, "cancel_subagent");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.is_empty(), "id must be optional (omit = kill all)");
    }

    #[test]
    fn not_in_base_registry() {
        // Subagents must NOT be able to cancel their siblings.
        assert!(
            !crate::registry()
                .iter()
                .any(|t| t.name() == "cancel_subagent"),
            "cancel_subagent must be chat-only, never in the subagent registry"
        );
    }
}
