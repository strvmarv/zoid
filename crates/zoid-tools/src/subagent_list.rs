use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `list_subagents {}` — an Emitting tool the main Chat agent uses to see which
/// subagents are currently running. The agent loop reads the `in_flight`
/// registry and returns each subagent's id + task. No parameters.
pub struct ListSubagents;

impl Tool for ListSubagents {
    fn name(&self) -> &str {
        "list_subagents"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_subagents".into(),
            description: "List subagents that are currently running. Returns each \
                          subagent's id and task description. Call this to check \
                          in-flight work before dispatching or canceling."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("list_subagents is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(ListSubagents.name(), "list_subagents");
        assert_eq!(ListSubagents.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_no_required_params() {
        let spec = ListSubagents.spec();
        assert_eq!(spec.name, "list_subagents");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.is_empty(), "list_subagents takes no params");
    }

    #[test]
    fn not_in_base_registry() {
        // Subagents must NOT be able to list subagents (they can't dispatch).
        assert!(
            !crate::registry().iter().any(|t| t.name() == "list_subagents"),
            "list_subagents must be chat-only, never in the subagent registry"
        );
    }
}
