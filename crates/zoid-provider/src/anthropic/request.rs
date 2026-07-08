use super::cache::place_breakpoints;
use super::types::*;
use crate::{CompletionRequest, Message, MsgRole, ToolSpec};

/// Translate zoid's provider-agnostic `CompletionRequest` into a typed
/// `AnthropicRequest` ready for serde serialization. Tool-use replay, tool
/// results, and cache breakpoints are all handled here.
pub fn build(req: &CompletionRequest) -> AnthropicRequest {
    let messages: Vec<AnthropicMessage> = req.messages.iter().map(map_message).collect();
    let thinking = build_thinking(req);
    let mut out = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        stream: true,
        messages,
        system: req.system.as_ref().map(|s| {
            vec![SystemBlock {
                kind: SystemBlockKind::Text,
                text: s.clone(),
                cache_control: None,
            }]
        }),
        tools: req.tools.iter().map(tool_def).collect(),
        thinking,
    };
    place_breakpoints(&mut out);
    out
}

/// Map `ThinkingMode` + model capability → `ThinkingConfig` (or `None`).
fn build_thinking(req: &CompletionRequest) -> Option<ThinkingConfig> {
    let info = crate::model::model_info(&req.model);
    match req.thinking {
        crate::ThinkingMode::Off => None,
        crate::ThinkingMode::Auto => match info.thinking {
            crate::model::ThinkingSupport::Budget => {
                let budget = (req.max_tokens as f64 * 0.6) as u32;
                let budget = budget.min(req.max_tokens.saturating_sub(2048));
                Some(ThinkingConfig {
                    r#type: ThinkingType::Enabled,
                    budget_tokens: Some(budget),
                    effort: None,
                })
            }
            crate::model::ThinkingSupport::Adaptive => Some(ThinkingConfig {
                r#type: ThinkingType::Adaptive,
                budget_tokens: None,
                effort: None,
            }),
            _ => None,
        },
        crate::ThinkingMode::Effort(level) => match info.thinking {
            crate::model::ThinkingSupport::Budget => {
                let pct = match level {
                    crate::EffortLevel::Low => 0.20,
                    crate::EffortLevel::Medium => 0.40,
                    crate::EffortLevel::High => 0.60,
                    crate::EffortLevel::Max => 0.80,
                };
                let budget = (req.max_tokens as f64 * pct) as u32;
                let budget = budget.min(req.max_tokens.saturating_sub(2048));
                Some(ThinkingConfig {
                    r#type: ThinkingType::Enabled,
                    budget_tokens: Some(budget),
                    effort: None,
                })
            }
            crate::model::ThinkingSupport::Adaptive => {
                let effort = match level {
                    crate::EffortLevel::Low => "low",
                    crate::EffortLevel::Medium => "medium",
                    crate::EffortLevel::High => "high",
                    crate::EffortLevel::Max => "max",
                };
                Some(ThinkingConfig {
                    r#type: ThinkingType::Adaptive,
                    budget_tokens: None,
                    effort: Some(effort.into()),
                })
            }
            _ => None,
        },
    }
}

/// The beta flags needed for thinking on this model, if any.
pub fn thinking_betas(req: &CompletionRequest) -> Vec<String> {
    let info = crate::model::model_info(&req.model);
    match req.thinking {
        crate::ThinkingMode::Off => Vec::new(),
        crate::ThinkingMode::Auto | crate::ThinkingMode::Effort(_) => {
            match info.thinking {
                crate::model::ThinkingSupport::Budget
                | crate::model::ThinkingSupport::Adaptive => {
                    vec!["extended-thinking-2025-05-14".into()]
                }
                _ => Vec::new(),
            }
        }
    }
}

fn map_message(m: &Message) -> AnthropicMessage {
    match m.role {
        MsgRole::User => AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text(m.content.clone()),
        },
        MsgRole::Assistant => {
            if m.tool_calls.is_empty() {
                // Plain assistant text turn: emit as a plain string (legacy
                // request_body parity). place_breakpoints converts the last
                // message to a cacheable block array if needed.
                AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: MessageContent::Text(m.content.clone()),
                }
            } else {
                // Replay assistant turns that requested tools: emit a block
                // array with a Text block (if non-empty) + one ToolUse block
                // per tool_call.
                let mut blocks: Vec<ContentBlock> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: m.content.clone(),
                        cache_control: None,
                    });
                }
                for tc in &m.tool_calls {
                    blocks.push(ContentBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.args.clone(),
                    });
                }
                AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: MessageContent::Blocks(blocks),
                }
            }
        }
        MsgRole::Tool => AnthropicMessage {
            // tool_result rides in a user message (Anthropic has no "tool" role).
            // Fallback chain per spec §5: tool_call_id → tool_name → empty.
            // Ollama sets tool_name but not tool_call_id; using tool_name as the
            // tool_use_id gives Anthropic a chance to correlate if the prior
            // assistant turn synthesized the same id. Anthropic sets
            // tool_call_id (real toolu_* id) which wins when both are present.
            role: AnthropicRole::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: m
                    .tool_call_id
                    .clone()
                    .or_else(|| m.tool_name.clone())
                    .unwrap_or_default(),
                content: m.content.clone(),
                is_error: None,
            }]),
        },
    }
}

fn tool_def(t: &ToolSpec) -> ToolDef {
    ToolDef {
        name: t.name.clone(),
        description: t.description.clone(),
        input_schema: t.parameters.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolCall};
    use serde_json::json;

    fn req(
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        system: Option<&str>,
    ) -> CompletionRequest {
        CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: system.map(String::from),
            messages,
            max_tokens: 1024,
            tools,
            thinking: crate::ThinkingMode::Off,
        }
    }

    #[test]
    fn plain_user_assistant_body() {
        let r = req(
            vec![Message::user("hi"), Message::assistant("hello")],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 1024,
                "stream": true,
                "messages": [
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": [
                        { "type": "text", "text": "hello", "cache_control": {"type": "ephemeral"} }
                    ]}
                ]
            })
        );
    }

    #[test]
    fn system_emits_as_cacheable_block() {
        let r = req(vec![Message::user("x")], vec![], Some("be terse"));
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "be terse", "cache_control": {"type": "ephemeral"} }])
        );
    }

    #[test]
    fn assistant_with_tool_calls_replays_tool_use_blocks() {
        let r = req(
            vec![
                Message::user("read foo"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "toolu_1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "foo"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
            ],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        let asst = &body["messages"][1];
        assert_eq!(asst["role"], "assistant");
        // empty text block is omitted (only tool_use blocks emitted)
        let blocks = asst["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "tool_use");
        assert_eq!(blocks[0]["id"], "toolu_1");
        assert_eq!(blocks[0]["name"], "read_file");
        assert_eq!(blocks[0]["input"], json!({"path": "foo"}));
    }

    #[test]
    fn tool_message_uses_tool_call_id_as_tool_use_id() {
        let r = req(
            vec![
                Message::user("read foo"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "toolu_1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "foo"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                // tool result with the originating call id
                {
                    let mut m = Message::tool("read_file", "bar");
                    m.tool_call_id = Some("toolu_1".into());
                    m
                },
            ],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        let tool_msg = &body["messages"][2];
        assert_eq!(tool_msg["role"], "user");
        let blocks = tool_msg["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "toolu_1");
        assert_eq!(blocks[0]["content"], "bar");
    }

    #[test]
    fn tool_message_without_call_id_falls_back_to_tool_name() {
        // Ollama case: tool_call_id is None, tool_name is "read_file". Per
        // spec §5 the fallback chain is tool_call_id → tool_name → empty, so
        // tool_use_id serializes as "read_file" (gives Anthropic a chance to
        // correlate if the prior assistant turn synthesized the same id).
        let r = req(vec![Message::tool("read_file", "bar")], vec![], None);
        let body = serde_json::to_value(build(&r)).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["tool_use_id"], "read_file");
    }

    #[test]
    fn tool_call_id_wins_over_tool_name() {
        // When both tool_call_id and tool_name are set (the Anthropic case,
        // where map_msg populates tool_call_id with the real toolu_* id), the
        // tool_call_id wins and tool_name is ignored for tool_use_id.
        let r = req(
            vec![{
                let mut m = Message::tool("read_file", "bar");
                m.tool_call_id = Some("toolu_1".into());
                m
            }],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["tool_use_id"], "toolu_1");
        // tool_name is NOT emitted as a field (Anthropic's ToolResult has no such field)
        assert!(blocks[0].get("tool_name").is_none());
    }

    #[test]
    fn tools_array_emitted_with_input_schema_field() {
        let r = req(
            vec![Message::user("x")],
            vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(
            body["tools"],
            json!([{
                "name": "read_file",
                "description": "read a file",
                "input_schema": {"type": "object"}
            }])
        );
    }

    #[test]
    fn no_tools_omits_tools_key() {
        let r = req(vec![Message::user("x")], vec![], None);
        let body = serde_json::to_value(build(&r)).unwrap();
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn interior_messages_stay_plain_text() {
        let r = req(
            vec![
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
            ],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // interior messages stay plain strings
        assert_eq!(msgs[0]["content"], "a");
        assert_eq!(msgs[1]["content"], "b");
        // last message gets a cache breakpoint block array
        assert!(msgs[2]["content"].is_array());
    }

    #[test]
    fn assistant_with_text_and_tool_calls_emits_both_blocks() {
        // Assistant turn with BOTH narrative text AND a tool call →
        // Blocks([Text{...}, ToolUse{...}]) in that order. Anthropic's API
        // requires the text block before the tool_use block in a tool-call turn.
        let r = req(
            vec![
                Message::user("read foo"),
                Message {
                    role: MsgRole::Assistant,
                    content: "Let me check.".into(),
                    tool_calls: vec![ToolCall {
                        id: "toolu_1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "foo"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
            ],
            vec![],
            None,
        );
        let body = serde_json::to_value(build(&r)).unwrap();
        let asst = &body["messages"][1];
        assert_eq!(asst["role"], "assistant");
        let blocks = asst["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        // Text block first, with the narrative
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Let me check.");
        // ToolUse block second
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "toolu_1");
        assert_eq!(blocks[1]["name"], "read_file");
        assert_eq!(blocks[1]["input"], json!({"path": "foo"}));
    }

    #[test]
    fn thinking_off_emits_no_thinking_key() {
        let r = req(vec![Message::user("x")], vec![], None);
        let body = serde_json::to_value(build(&r)).unwrap();
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn thinking_auto_budget_model_emits_enabled_with_budget() {
        let r = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert!(budget > 0, "budget must be positive");
        assert!(budget < 16000, "budget must be < max_tokens");
    }

    #[test]
    fn thinking_auto_adaptive_model_emits_adaptive() {
        let r = CompletionRequest {
            model: "claude-opus-4-8".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn thinking_effort_high_budget_model_maps_to_60pct() {
        let r = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 10000,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::High),
        };
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert_eq!(budget, 6000, "High effort = 60% of max_tokens");
    }

    #[test]
    fn thinking_effort_max_adaptive_model_emits_effort() {
        let r = CompletionRequest {
            model: "claude-opus-4-8".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = serde_json::to_value(build(&r)).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["effort"], "max");
    }

    #[test]
    fn thinking_betas_returns_extended_thinking_for_budget_models() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let betas = thinking_betas(&req);
        assert_eq!(betas, vec!["extended-thinking-2025-05-14".to_string()]);
    }

    #[test]
    fn thinking_betas_empty_when_off() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        assert!(thinking_betas(&req).is_empty());
    }

}