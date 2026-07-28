use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `schedule_wake { delay_secs, note }` — Emitting: the main loop validates,
/// persists a WakeScheduled, and arms the watcher. Main Chat agent only.
pub struct ScheduleWake;
impl Tool for ScheduleWake {
    fn name(&self) -> &str {
        "schedule_wake"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_wake".into(),
            description: "Schedule a one-shot reminder to resume THIS conversation \
                          after delay_secs seconds. On fire you are re-invoked with \
                          `note` as a message. Minimum 30s. Use when waiting on \
                          something to check later. Schedule exactly ONE wake per \
                          event — do not schedule multiple wakes for the same thing. \
                          If a wake is already pending, cancel it before scheduling a \
                          new one. Duplicate wakes for the same note are rejected."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "delay_secs": { "type": "integer", "minimum": 30,
                        "description": "Seconds from now to wake (>= 30)." },
                    "note": { "type": "string",
                        "description": "What to remind yourself to do on wake." }
                },
                "required": ["delay_secs", "note"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("schedule_wake is executed by the agent loop")
    }
}

/// `cancel_wake { id? }` — Emitting: cancels one pending wake by id, or all when
/// `id` is omitted. Main Chat agent only.
pub struct CancelWake;
impl Tool for CancelWake {
    fn name(&self) -> &str {
        "cancel_wake"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "cancel_wake".into(),
            description: "Cancel a scheduled wake by its id (from schedule_wake), or all \
                          pending wakes when id is omitted."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string",
                        "description": "The wake id to cancel; omit to cancel all pending wakes." }
                }
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("cancel_wake is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_wake_name_and_kind() {
        assert_eq!(ScheduleWake.name(), "schedule_wake");
        assert_eq!(ScheduleWake.kind(), ToolKind::Emitting);
    }

    #[test]
    fn schedule_wake_spec_requires_delay_and_note() {
        let spec = ScheduleWake.spec();
        assert_eq!(spec.name, "schedule_wake");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("delay_secs")));
        assert!(required.iter().any(|r| r.as_str() == Some("note")));
        let desc = ScheduleWake.spec().description;
        assert!(
            desc.contains("exactly ONE wake per event"),
            "description must say 'exactly ONE wake per event': {desc}"
        );
        assert!(
            desc.contains("Duplicate wakes for the same note are rejected"),
            "description must mention that duplicates are rejected: {desc}"
        );
    }

    #[test]
    fn cancel_wake_name_and_kind() {
        assert_eq!(CancelWake.name(), "cancel_wake");
        assert_eq!(CancelWake.kind(), ToolKind::Emitting);
    }

    #[test]
    fn cancel_wake_spec_has_optional_id() {
        let spec = CancelWake.spec();
        assert_eq!(spec.name, "cancel_wake");
        // id is not required — omitting it means "cancel all".
        let required = spec.parameters.get("required");
        if let Some(required) = required {
            let required = required.as_array().unwrap();
            assert!(!required.iter().any(|r| r.as_str() == Some("id")));
        }
    }
}
