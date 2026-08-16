//! `submit_feedback` — an Interactive tool. The agent loop intercepts it by
//! kind (alongside `ask_user` and `apply_mode_mapping`), surfaces the proposal
//! to the user via the `Feedback` overlay, and submits on confirm. `run()` is
//! never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct SubmitFeedback;

impl Tool for SubmitFeedback {
    fn name(&self) -> &str {
        "submit_feedback"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "submit_feedback".into(),
            description: "Offer to submit user feedback or a bug report to the zoid \
                maintainers (GitHub issues on strvmarv/zoid). The user MUST \
                confirm/edit before it is submitted — never file silently. Use when \
                the user asks to report a bug or give feedback, or when a reproducible \
                error occurs and the user agrees to report it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["bug","feature","general"] },
                    "title": { "type": "string", "description": "Short summary of the issue or feedback" },
                    "body":  { "type": "string", "description": "Detailed description: steps to reproduce, expected vs actual, or the suggestion" }
                },
                "required": ["kind", "title", "body"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Interactive
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("submit_feedback must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_submit_feedback_schema() {
        let s = SubmitFeedback.spec();
        assert_eq!(s.name, "submit_feedback");
        assert_eq!(SubmitFeedback.kind(), ToolKind::Interactive);
        assert!(s.parameters["properties"]["kind"].is_object());
        assert!(s.parameters["properties"]["title"].is_object());
        assert!(s.parameters["properties"]["body"].is_object());
        assert_eq!(s.parameters["required"][0], "kind");
    }

    #[test]
    fn run_is_error_not_panic() {
        let out = SubmitFeedback.run(&json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("must be handled by the agent loop"));
    }
}
