//! Test harness for zoid agents built on `zoid-core` + `zoid-provider`.
//!
//! Drive an agent loop with a scripted model instead of a live provider, then
//! assert on the resulting event log. Depends only on `zoid-core` and
//! `zoid-provider`, so it works for any agent built on that seam.
//!
//! ```
//! use zoid_testkit::{script, text, tool_call};
//! use serde_json::json;
//! let provider = script(vec![
//!     tool_call("write_file", json!({"path": "a.txt", "content": "hi"})),
//!     text("done"),
//! ]);
//! // hand `provider` to your run_agent_turn; then inspect the log.
//! # let _ = provider;
//! ```

use std::sync::Arc;
use zoid_core::event::{Event, EventKind};
use zoid_provider::{FakeProvider, Provider, ProviderEvent, ToolCall};

pub use zoid_provider::FakeProvider as ScriptedProvider;

/// A model text chunk.
pub fn text(s: &str) -> ProviderEvent {
    ProviderEvent::TextDelta(s.to_string())
}

/// A tool call with an empty id (Ollama-native shape) and parsed args.
pub fn tool_call(name: &str, args: serde_json::Value) -> ProviderEvent {
    ProviderEvent::ToolCall(ToolCall {
        id: String::new(),
        name: name.to_string(),
        args,
    })
}

/// Build a scripted provider from an ordered event list.
pub fn script(events: Vec<ProviderEvent>) -> Arc<dyn Provider> {
    Arc::new(FakeProvider::new(events))
}

/// Extract `(name, output, is_error)` for every `ToolResult` in the log.
///
/// Accepts anything iterable as `&Event` — a `&Vec<Event>`/`&[Event]`, or (in
/// the `zoid` crate, which depends on this testkit for integration tests) an
/// `EventLog::iter()` — so it works for both a raw `Vec<Event>` log and the
/// `EventLog`-typed value `run_agent_turn` returns.
pub fn tool_results<'a>(
    events: impl IntoIterator<Item = &'a Event>,
) -> Vec<(String, String, bool)> {
    events
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult {
                name,
                output,
                is_error,
                ..
            } => Some((name.clone(), output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// Panic if any tool result is an error.
pub fn assert_no_tool_errors<'a>(events: impl IntoIterator<Item = &'a Event>) {
    for (name, output, is_error) in tool_results(events) {
        assert!(!is_error, "tool `{name}` errored: {output}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn script_builds_a_provider_and_helpers_shape_events() {
        let _p = script(vec![tool_call("search", json!({"q": "x"})), text("ok")]);
        match tool_call("search", json!({"q": "x"})) {
            ProviderEvent::ToolCall(tc) => {
                assert_eq!(tc.name, "search");
                assert_eq!(tc.id, "");
            }
            _ => panic!("expected a ToolCall"),
        }
    }

    #[test]
    fn tool_results_filters_and_flags_errors() {
        // Hand-build a log with one ok result and one error result.
        let mk = |name: &str, err: bool| {
            let kind = EventKind::ToolResult {
                id: String::new(),
                name: name.into(),
                output: if err { "boom".into() } else { "fine".into() },
                is_error: err,
                error_kind: None,
            };
            Event::new(ulid::Ulid::nil(), None, 0, kind)
        };
        let log = vec![mk("read_file", false), mk("shell", true)];
        let got = tool_results(&log);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1], ("shell".to_string(), "boom".to_string(), true));
    }
}
