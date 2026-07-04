//! The `recall` tool: search the cold tier and re-admit matching turns. Like
//! `update_tasks`, it is `Emitting` — the agent loop executes it (it needs the
//! session actor + the event log), so `run()` is never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct Recall;

impl Tool for Recall {
    fn name(&self) -> &str {
        "recall"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recall".into(),
            description: "Search earlier, paged-out conversation history by keyword and bring \
                          matching turns back into context. Use when the breadcrumb says context \
                          was paged out and you need it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "keywords to search paged-out history" },
                    "limit": { "type": "integer", "description": "max turns to retrieve (default 5)" }
                },
                "required": ["query"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the loop branches on Emitting before calling run().
        ToolOutput::err("recall is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolKind};
    #[test]
    fn recall_spec_and_kind() {
        assert_eq!(Recall.name(), "recall");
        assert_eq!(Recall.spec().name, "recall");
        assert_eq!(Recall.kind(), ToolKind::Emitting); // executed in-loop, never via run()
    }
}
