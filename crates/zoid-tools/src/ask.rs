//! `ask_user` — an Interactive tool. The agent loop intercepts it (by kind),
//! prompts the UI, and awaits the user's answer; `run()` is never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct AskUser;

impl Tool for AskUser {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_user".into(),
            description: "Ask the user a question and wait for their answer. Omit `choices` for a \
                free-text answer, or provide `choices` to offer specific options. Use sparingly, \
                when you genuinely need the user to decide or clarify."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "choices": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["question"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Interactive
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("ask_user must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_ask_user_schema() {
        let s = AskUser.spec();
        assert_eq!(s.name, "ask_user");
        assert_eq!(AskUser.kind(), ToolKind::Interactive);
        assert!(s.parameters["properties"]["question"].is_object());
    }
}
