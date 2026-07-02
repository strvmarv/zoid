//! `update_tasks` — an Emitting tool. The agent loop intercepts it (by kind)
//! and appends an `EventKind::Tasks` snapshot; `run()` is a defensive no-op that
//! is never called on the happy path.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct UpdateTasks;

impl Tool for UpdateTasks {
    fn name(&self) -> &str {
        "update_tasks"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "update_tasks".into(),
            description: "Publish your current task list to the user's rail. Send the FULL list \
                every time (it replaces the previous one). Keep at most one task 'active' at a \
                time. Statuses: pending, active, done."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "active", "done"] }
                            },
                            "required": ["text", "status"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // The loop handles Emitting tools; run() is never reached on the happy
        // path. Return an error if somehow dispatched directly.
        ToolOutput::err("update_tasks must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_update_tasks_schema() {
        let s = UpdateTasks.spec();
        assert_eq!(s.name, "update_tasks");
        assert_eq!(UpdateTasks.kind(), ToolKind::Emitting);
        assert!(s.parameters["properties"]["tasks"].is_object());
    }
}
