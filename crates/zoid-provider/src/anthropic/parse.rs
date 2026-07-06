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
}
