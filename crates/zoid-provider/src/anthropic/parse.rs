use super::types::*;
use crate::{ProviderEvent, ToolCall, Usage};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Per-stream accumulator for in-flight tool_use blocks. A tool call spans
/// multiple SSE frames: `content_block_start` (id + name) → N×
/// `input_json_delta` (partial args) → `content_block_stop` (finalize). The
/// stream loop in `mod.rs` owns one of these and passes it to `event()` per
/// frame.
#[derive(Default)]
pub struct ToolUseAccumulator {
    /// index → (id, name, accumulated partial_json)
    slots: HashMap<u32, (String, String, String)>,
}

impl ToolUseAccumulator {
    fn start(&mut self, index: u32, id: String, name: String) {
        self.slots.insert(index, (id, name, String::new()));
    }
    fn append(&mut self, index: u32, partial: &str) {
        if let Some(slot) = self.slots.get_mut(&index) {
            slot.2.push_str(partial);
        }
    }
    fn finalize(&mut self, index: u32) -> Option<ToolCall> {
        self.slots.remove(&index).map(|(id, name, raw)| ToolCall {
            id,
            name,
            args: coerce_args(&raw),
        })
    }
}

#[cfg(test)]
impl ToolUseAccumulator {
    /// Test-only: finalize a slot by index without going through `event`.
    /// Used to verify orphaned slots (start without stop) are still present.
    pub fn finalize_for_test(&mut self, index: u32) -> Option<ToolCall> {
        self.finalize(index)
    }
}

/// Coerce an accumulated tool-args JSON string into a usable arguments Value.
/// Mirrors `ollama.rs::coerce_tool_args`: a valid object is kept; anything
/// else (garbage, empty, non-object) falls back to `{}`.
pub fn coerce_args(partial_json: &str) -> Value {
    serde_json::from_str::<Value>(partial_json)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Map one typed SSE frame to zero-or-more `ProviderEvent`s. Never panics;
/// unhandled variants return `vec![]`.
pub fn event(frame: StreamEvent, acc: &mut ToolUseAccumulator) -> Vec<ProviderEvent> {
    match frame {
        StreamEvent::MessageStart { message } => {
            let u = &message.usage;
            let input = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
            vec![ProviderEvent::Usage(Usage {
                input_tokens: input,
                output_tokens: 0,
                cached: u.cache_read_input_tokens,
            })]
        }
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => match content_block {
            ContentBlockStart::Text => vec![],
            ContentBlockStart::ToolUse { id, name } => {
                acc.start(index, id, name);
                vec![]
            }
            ContentBlockStart::Thinking => vec![],
        },
        StreamEvent::ContentBlockDelta { index, delta } => match delta {
            Delta::TextDelta { text } => vec![ProviderEvent::TextDelta(text)],
            Delta::InputJsonDelta { partial_json } => {
                acc.append(index, &partial_json);
                vec![]
            }
            Delta::ThinkingDelta { .. } | Delta::SignatureDelta { .. } => vec![],
        },
        StreamEvent::ContentBlockStop { index } => match acc.finalize(index) {
            Some(tc) => vec![ProviderEvent::ToolCall(tc)],
            None => vec![],
        },
        StreamEvent::MessageDelta { delta, usage } => {
            let mut out = vec![ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: usage.output_tokens,
                cached: 0,
            })];
            if delta.stop_reason.as_deref() == Some("max_tokens") {
                out.push(ProviderEvent::Truncated);
            }
            out
        }
        StreamEvent::MessageStop => vec![ProviderEvent::Done],
        StreamEvent::Error { error } => vec![ProviderEvent::Error(error.message)],
        StreamEvent::Ping => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::super::types;
    use super::*;

    #[test]
    fn message_start_folds_cache_tokens_into_input() {
        let frame = StreamEvent::MessageStart {
            message: MessageStart {
                usage: types::Usage {
                    input_tokens: 7,
                    output_tokens: 0,
                    cache_read_input_tokens: 40,
                    cache_creation_input_tokens: 3,
                },
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 50,
                output_tokens: 0,
                cached: 40,
            })]
        );
    }

    #[test]
    fn message_start_without_cache_tokens() {
        let frame = StreamEvent::MessageStart {
            message: MessageStart {
                usage: types::Usage {
                    input_tokens: 7,
                    output_tokens: 0,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                },
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 7,
                output_tokens: 0,
                cached: 0,
            })]
        );
    }

    #[test]
    fn text_delta_emits_textdelta() {
        let frame = StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::TextDelta {
                text: "Hello".into(),
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(out, vec![ProviderEvent::TextDelta("Hello".into())]);
    }

    #[test]
    fn message_stop_emits_done() {
        let frame = StreamEvent::MessageStop;
        let mut acc = ToolUseAccumulator::default();
        assert_eq!(event(frame, &mut acc), vec![ProviderEvent::Done]);
    }

    #[test]
    fn message_delta_emits_usage() {
        let frame = StreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some("end_turn".into()),
            },
            usage: types::Usage {
                input_tokens: 0,
                output_tokens: 12,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: 12,
                cached: 0,
            })]
        );
    }

    #[test]
    fn message_delta_max_tokens_emits_usage_then_truncated() {
        let frame = StreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some("max_tokens".into()),
            },
            usage: types::Usage {
                input_tokens: 0,
                output_tokens: 4096,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(
            out,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 0,
                    output_tokens: 4096,
                    cached: 0,
                }),
                ProviderEvent::Truncated
            ]
        );
    }

    #[test]
    fn error_emits_error() {
        let frame = StreamEvent::Error {
            error: ApiError {
                message: "Overloaded".into(),
            },
        };
        let mut acc = ToolUseAccumulator::default();
        let out = event(frame, &mut acc);
        assert_eq!(out, vec![ProviderEvent::Error("Overloaded".into())]);
    }

    #[test]
    fn ping_emits_nothing() {
        let frame = StreamEvent::Ping;
        let mut acc = ToolUseAccumulator::default();
        assert!(event(frame, &mut acc).is_empty());
    }

    #[test]
    fn thinking_delta_emits_nothing() {
        let frame = StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::ThinkingDelta {
                thinking: "reasoning".into(),
            },
        };
        let mut acc = ToolUseAccumulator::default();
        assert!(event(frame, &mut acc).is_empty());
    }

    #[test]
    fn coerce_args_valid_object() {
        assert_eq!(coerce_args(r#"{"path":"a.txt"}"#), json!({"path": "a.txt"}));
    }

    #[test]
    fn coerce_args_garbage_returns_empty_object() {
        assert_eq!(coerce_args("not json"), json!({}));
        assert_eq!(coerce_args(""), json!({}));
    }

    #[test]
    fn coerce_args_non_object_returns_empty_object() {
        assert_eq!(coerce_args("[1,2]"), json!({}));
        assert_eq!(coerce_args("42"), json!({}));
        assert_eq!(coerce_args("null"), json!({}));
    }

    #[test]
    fn tool_use_accumulates_across_start_deltas_stop() {
        let mut acc = ToolUseAccumulator::default();
        // start
        let start = StreamEvent::ContentBlockStart {
            index: 1,
            content_block: ContentBlockStart::ToolUse {
                id: "toolu_1".into(),
                name: "read_file".into(),
            },
        };
        assert!(event(start, &mut acc).is_empty());
        // two json deltas
        let d1 = StreamEvent::ContentBlockDelta {
            index: 1,
            delta: Delta::InputJsonDelta {
                partial_json: r#"{"path":"#.into(),
            },
        };
        assert!(event(d1, &mut acc).is_empty());
        let d2 = StreamEvent::ContentBlockDelta {
            index: 1,
            delta: Delta::InputJsonDelta {
                partial_json: r#""a.txt"}"#.into(),
            },
        };
        assert!(event(d2, &mut acc).is_empty());
        // stop finalizes
        let stop = StreamEvent::ContentBlockStop { index: 1 };
        let out = event(stop, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "toolu_1".into(),
                name: "read_file".into(),
                args: json!({"path": "a.txt"}),
            })]
        );
    }

    #[test]
    fn tool_use_with_garbage_args_falls_back_to_empty_object() {
        let mut acc = ToolUseAccumulator::default();
        let start = StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStart::ToolUse {
                id: "toolu_2".into(),
                name: "list_dir".into(),
            },
        };
        event(start, &mut acc);
        let d = StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::InputJsonDelta {
                partial_json: "not json".into(),
            },
        };
        event(d, &mut acc);
        let out = event(StreamEvent::ContentBlockStop { index: 0 }, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "toolu_2".into(),
                name: "list_dir".into(),
                args: json!({}),
            })]
        );
    }

    #[test]
    fn tool_use_with_no_json_deltas_emits_empty_object_args() {
        let mut acc = ToolUseAccumulator::default();
        let start = StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStart::ToolUse {
                id: "toolu_3".into(),
                name: "ping".into(),
            },
        };
        event(start, &mut acc);
        let out = event(StreamEvent::ContentBlockStop { index: 0 }, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "toolu_3".into(),
                name: "ping".into(),
                args: json!({}),
            })]
        );
    }

    #[test]
    fn text_block_stop_emits_nothing() {
        // A text content block's stop has no accumulator entry → no ToolCall.
        let mut acc = ToolUseAccumulator::default();
        let out = event(StreamEvent::ContentBlockStop { index: 0 }, &mut acc);
        assert!(out.is_empty());
    }

    #[test]
    fn multiple_tool_uses_in_one_stream_finalize_independently() {
        let mut acc = ToolUseAccumulator::default();
        // start both
        event(
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStart::ToolUse {
                    id: "toolu_a".into(),
                    name: "read".into(),
                },
            },
            &mut acc,
        );
        event(
            StreamEvent::ContentBlockStart {
                index: 1,
                content_block: ContentBlockStart::ToolUse {
                    id: "toolu_b".into(),
                    name: "write".into(),
                },
            },
            &mut acc,
        );
        // deltas for both, interleaved
        event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: Delta::InputJsonDelta {
                    partial_json: r#"{"x":1}"#.into(),
                },
            },
            &mut acc,
        );
        event(
            StreamEvent::ContentBlockDelta {
                index: 1,
                delta: Delta::InputJsonDelta {
                    partial_json: r#"{"y":2}"#.into(),
                },
            },
            &mut acc,
        );
        // stop in reverse order
        let out0 = event(StreamEvent::ContentBlockStop { index: 0 }, &mut acc);
        let out1 = event(StreamEvent::ContentBlockStop { index: 1 }, &mut acc);
        assert_eq!(
            out0,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "toolu_a".into(),
                name: "read".into(),
                args: json!({"x": 1}),
            })]
        );
        assert_eq!(
            out1,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "toolu_b".into(),
                name: "write".into(),
                args: json!({"y": 2}),
            })]
        );
    }

    #[test]
    fn interrupted_tool_use_start_without_stop_emits_no_toolcall() {
        // Stream interrupted mid-tool: start + deltas but no content_block_stop
        // before message_stop. The slot is orphaned; no ToolCall is emitted.
        // This is parity with legacy (which didn't accumulate at all), but the
        // test makes the silent-drop behavior explicit.
        let mut acc = ToolUseAccumulator::default();
        event(
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStart::ToolUse {
                    id: "toolu_x".into(),
                    name: "read".into(),
                },
            },
            &mut acc,
        );
        event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: Delta::InputJsonDelta {
                    partial_json: r#"{"path":"a"}"#.into(),
                },
            },
            &mut acc,
        );
        // message_stop arrives without a content_block_stop for index 0
        let out = event(StreamEvent::MessageStop, &mut acc);
        assert_eq!(out, vec![ProviderEvent::Done]);
        // No ToolCall was ever emitted — the slot is orphaned in `acc`.
        // (Verify the orphan by finalizing the slot now: it would still emit.)
        let orphan = acc.finalize_for_test(0);
        assert!(
            orphan.is_some(),
            "orphaned slot should still be finalizable"
        );
    }

    #[test]
    fn orphan_content_block_stop_emits_nothing() {
        // content_block_stop for an index that was never started → no ToolCall,
        // no panic. (Defensive against malformed streams.)
        let mut acc = ToolUseAccumulator::default();
        let out = event(StreamEvent::ContentBlockStop { index: 99 }, &mut acc);
        assert!(out.is_empty());
    }

    #[test]
    fn signature_delta_emits_nothing() {
        // Spec §7.2: thinking blocks are parsed but discarded. The signature
        // delta is part of a thinking block's lifecycle; dropped from the
        // event stream. Pin this so a future split of the `|` arm doesn't
        // silently regress signature handling.
        let frame = StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::SignatureDelta {
                signature: "sig".into(),
            },
        };
        let mut acc = ToolUseAccumulator::default();
        assert!(event(frame, &mut acc).is_empty());
    }

    #[test]
    fn content_block_start_text_emits_nothing() {
        // Text content block start → no event (deltas carry the text).
        let frame = StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStart::Text,
        };
        let mut acc = ToolUseAccumulator::default();
        assert!(event(frame, &mut acc).is_empty());
    }

    #[test]
    fn content_block_start_thinking_emits_nothing() {
        // Thinking content block start → no event (discarded per §7.2).
        let frame = StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStart::Thinking,
        };
        let mut acc = ToolUseAccumulator::default();
        assert!(event(frame, &mut acc).is_empty());
    }
}
