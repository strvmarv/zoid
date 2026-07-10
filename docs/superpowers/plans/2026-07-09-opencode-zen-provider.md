# OpenCode Zen Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `opencode-zen` provider that streams completions + tool-calls against OpenCode's Zen endpoint by routing each model to one of four wire shapes (OpenAI Chat Completions, Anthropic Messages, OpenAI Responses, Google Gemini), plus prettify the config Secrets section labels.

**Architecture:** A dedicated `OpenCodeZenProvider` (`opencode_zen.rs`) owns a static per-model `ZEN_MODELS` wire-shape table and delegates `stream()`/`list_models()` to one of four sub-clients — the two existing leaves (`OpenAICompatProvider`, `AnthropicProvider`) and two new generic leaves (`OpenAIResponsesProvider` in `openai_responses.rs`, `GoogleGeminiProvider` in `google_gemini.rs`). One new `ProviderEntry` in `zoid-model`; three bin-wiring arms in `main.rs`. The Secrets section decouples display label from secret-store key via a new `FieldRow::secret_key` field.

**Tech Stack:** Rust 2021, workspace crates (`zoid-model`, `zoid-provider`, `zoid-tui`, `zoid`), `reqwest`, `eventsource-stream`, `futures-util`, `serde_json`, `async-trait`, `tokio` (test dev-dep with `net`+`io-util` for `TcpListener` stubs).

## Global Constraints

- **Offline tests only.** No live-endpoint CI. All streaming tests use `tokio::net::TcpListener` stub servers (matches `openai_compat.rs`/`opencode_go.rs` stance).
- **Generic leaves.** `openai_responses.rs` and `google_gemini.rs` contain zero opencode-zen-specifics — they're reusable leaves like `openai_compat.rs`.
- **Shared key.** `opencode-zen` reads `OPENCODE_GO_API_KEY` (same env var as Go). No new env var, no new settings secret field.
- **Provider trait unchanged.** No new `ProviderEvent` variants. Non-essential Responses/Gemini event types are parsed-but-ignored (logged at trace).
- **Go entry unchanged.** The `opencode-go` `ProviderEntry` (id, family, display, base_url) is not modified. Only the Secrets *section row labels* change (via `FieldRow::secret_key`), not the picker.
- **No placeholders in shipped code.** The `ZEN_MODELS` table, registry `models`, and `MODEL_CAPS` entries use the concrete placeholder set defined in Task 1 (a minimal representative model per wire shape) so the build is green and tests pass end-to-end; real Zen catalog fill-in is a follow-up spec-review pass, not part of this plan.
- **PRODUCT DECISION — placeholder model ids are user-visible.** The four `zen-*-demo` ids are deliberately fake, but the `opencode-zen` entry is `Status::Available`, so a user who selects it sees `zen-chat-demo` / `zen-claude-demo` / `zen-gpt-demo` / `zen-gemini-demo` in the model picker. This is a deliberate trade-off: shipping the provider selectable (so the wiring is exercised end-to-end and the slice is mergeable) vs. gating it `Status::Planned` (visible-but-not-selectable) until the real Zen catalog lands. **If this plan merges before the real catalog is filled in, prefer `Status::Planned` for `opencode-zen`** so fake model names never reach a user; flip to `Available` in the catalog-fill follow-up. If the real catalog lands in the same PR, keep `Available`. The implementer should confirm which path with the reviewer before Task 1.
- **Existing patterns.** New clients mirror `openai_compat.rs` structurally: `request_body()`/`parse_event()` pure fns + provider struct + `new().with_base_url().with_idle_timeout()` + `TcpListener`-stubbed tests. New tests mirror `opencode_go.rs`'s `spawn_recording_server` and `openai_compat.rs`'s `spawn_stalling_server`.
- **Known follow-up (out of this plan's scope): key-prompt gate label.** When a key-requiring provider is selected without a key, the gate prompt (`render.rs:1206`, `Enter {env}`) shows the raw env-var name (e.g. `OPENCODE_GO_API_KEY`), while the Secrets *rows* now show the friendly name (`opencode`). This is a cosmetic inconsistency in a transient prompt, scoped out per the spec (which limits prettification to Secrets rows). Closing it requires extracting `friendly_secret_label` to a public helper and wiring `render.rs` to it — a small but separate change, deferred to a follow-up so this plan doesn't expand into the render layer.
- Commit frequently (every task or sub-step).

---

## File Structure

**Create:**
- `crates/zoid-provider/src/openai_responses.rs` — OpenAI Responses API client (generic leaf).
- `crates/zoid-provider/src/google_gemini.rs` — Google Gemini client (generic leaf).
- `crates/zoid-provider/src/opencode_zen.rs` — dedicated `OpenCodeZenProvider` (4-way routing).

**Modify:**
- `crates/zoid-provider/src/lib.rs` — `pub mod openai_responses; pub mod google_gemini; pub mod opencode_zen;`
- `crates/zoid-model/src/lib.rs` — new `PROVIDERS` entry, new `MODEL_CAPS` entries, `selectable` test update.
- `crates/zoid-tui/src/config_view.rs` — `FieldRow::secret_key` field, `build_sections` Secrets labels.
- `crates/zoid/src/main.rs` — `key_env_for`/`select_provider`/`provider_for_id` `opencode-zen` arms; secret commit/clear flows use `secret_key`; `current_config_field` returns the row's `secret_key`; tests.
- `crates/zoid-tui/src/route.rs` — `FieldRow` literals in tests gain `secret_key: None`.

---

## Task 1: Registry entry + placeholder model caps (zoid-model)

**Files:**
- Modify: `crates/zoid-model/src/lib.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Produces: `ProviderEntry { id: "opencode-zen", family: "opencode-zen", display: "opencode · zen", transport: Http { default_base_url: "https://opencode.ai/zen" }, models: &[ZEN_PLACEHOLDER_MODELS], status: Available }` where `ZEN_PLACEHOLDER_MODELS = &["zen-chat-demo", "zen-claude-demo", "zen-gpt-demo", "zen-gemini-demo"]` (first = default). Also `MODEL_CAPS` entries for those four ids.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-model/src/lib.rs` in the `opencode_go_tests` module (or a new `opencode_zen_tests` module right after it):

```rust
#[cfg(test)]
mod opencode_zen_tests {
    use super::*;

    #[test]
    fn opencode_zen_registry_entry_exists_and_is_selectable() {
        let e = entry("opencode-zen").expect("opencode-zen entry must exist");
        assert_eq!(e.id, "opencode-zen");
        assert_eq!(e.family, "opencode-zen");
        assert_eq!(e.display, "opencode · zen");
        assert_eq!(e.status, Status::Available);
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://opencode.ai/zen"
            }
        );
        assert!(!e.models.is_empty(), "must list at least one model");
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"opencode-zen"));
    }

    #[test]
    fn canonical_id_opencode_zen_is_passthrough() {
        assert_eq!(canonical_id("opencode-zen"), "opencode-zen");
    }

    #[test]
    fn default_base_url_opencode_zen() {
        assert_eq!(
            default_base_url("opencode-zen"),
            Some("https://opencode.ai/zen")
        );
    }

    #[test]
    fn opencode_zen_model_caps_present() {
        for id in entry("opencode-zen").unwrap().models {
            let info = model_info(id);
            // conservative but non-default: ensure each placeholder has an
            // explicit entry (not the 32k DEFAULT_MODEL_INFO floor).
            assert!(
                info.context_window >= 128_000,
                "{id} should have an explicit caps entry, got {info:?}"
            );
        }
    }
}
```

Also update the existing `selectable_has_four_providers` test to five:

```rust
    #[test]
    fn selectable_has_five_providers() {
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert_eq!(ids.len(), 5);
        assert!(ids.contains(&"ollama-local"));
        assert!(ids.contains(&"ollama-cloud"));
        assert!(ids.contains(&"opencode-go"));
        assert!(ids.contains(&"opencode-zen"));
        assert!(ids.contains(&"anthropic-api"));
    }
```

(Remove or rename the old `selectable_has_four_providers`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-model`
Expected: FAIL — `entry("opencode-zen")` returns `None`; `selectable_has_five_providers` length mismatch (4 ≠ 5).

- [ ] **Step 3: Add the registry entry and model caps**

In `crates/zoid-model/src/lib.rs`, append to `PROVIDERS` (after the `anthropic-api` entry):

```rust
    ProviderEntry {
        id: "opencode-zen",
        display: "opencode · zen",
        family: "opencode-zen",
        transport: Transport::Http {
            default_base_url: "https://opencode.ai/zen",
        },
        models: &[
            "zen-chat-demo",
            "zen-claude-demo",
            "zen-gpt-demo",
            "zen-gemini-demo",
        ],
        status: Status::Available,
    },
```

Append to `MODEL_CAPS` (before the `o3` entry or at the end):

```rust
    // --- OpenCode Zen placeholder models (one per wire shape; real catalog
    // filled in a later spec-review pass). Conservative caps. ---
    (
        "zen-chat-demo",
        ModelInfo {
            context_window: 128_000,
            max_output: 0,
            tools: true,
            prompt_cache: false,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "zen-claude-demo",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: false, // Anthropic text-only P1b limitation, same as Go's Anthropic-shape models
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "zen-gpt-demo",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: true,
            prompt_cache: false,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::OpenAI,
        },
    ),
    (
        "zen-gemini-demo",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 0,
            tools: true,
            prompt_cache: false,
            thinking: ThinkingSupport::Toggle,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-model`
Expected: PASS — all `opencode_zen_tests` + `selectable_has_five_providers` green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): add opencode-zen registry entry + placeholder model caps"
```

---

## Task 2: OpenAI Responses client — request body + pure parse (zoid-provider)

**Files:**
- Create: `crates/zoid-provider/src/openai_responses.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod openai_responses;`)
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::{CompletionRequest, Message, MsgRole, Provider, ProviderEvent, ToolCall, Usage, ThinkingMode, EffortLevel}`, `crate::model::model_info`.
- Produces: `pub fn request_body(req: &CompletionRequest) -> Value`, `pub fn parse_event(data: &str, acc: &mut ResponsesToolAccum) -> Vec<ProviderEvent>`, `pub struct ResponsesToolAccum`, `pub struct OpenAIResponsesProvider`.

- [ ] **Step 1: Write the failing test for request_body**

Create `crates/zoid-provider/src/openai_responses.rs` with the module doc comment and a `#[cfg(test)]` block. First the request-body test:

```rust
//! The generic OpenAI Responses client (POST {base}/v1/responses, SSE streaming,
//! response.* events, function-call via response.function_call_arguments.delta/.done,
//! reasoning summaries, usage on response.completed). Self-contained like the
//! `openai_compat`/`anthropic` modules; uses the crate's `Provider` seam. No
//! opencode-zen-specifics — a generic leaf reusable by direct-OpenAI, OpenRouter, etc.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolSpec};
    use serde_json::json;

    #[test]
    fn body_has_model_input_instructions_stream() {
        let req = CompletionRequest {
            model: "zen-gpt-demo".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["model"], "zen-gpt-demo");
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "be terse");
        // input is a string shorthand when there's a single user message and no tool messages
        assert_eq!(body["input"], "hi");
        assert_eq!(body["max_output_tokens"], 1024);
        assert!(body.get("reasoning").is_none(), "Off must omit reasoning");
    }
}
```

Add `pub mod openai_responses;` to `crates/zoid-provider/src/lib.rs` (after `pub mod opencode_go;`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider openai_responses::tests::body_has_model_input_instructions_stream`
Expected: FAIL — `request_body` not defined.

- [ ] **Step 3: Implement request_body**

Add to `crates/zoid-provider/src/openai_responses.rs` (above the test module):

```rust
/// Map `ThinkingMode` to the Responses `reasoning.effort` object, or `None` to
/// omit the field entirely (Off). zoid's EffortLevel::Max → OpenAI "xhigh".
fn reasoning_params(req: &CompletionRequest) -> Option<Value> {
    let effort = match req.thinking {
        crate::ThinkingMode::Off => return None,
        crate::ThinkingMode::Auto => "medium",
        crate::ThinkingMode::Effort(crate::EffortLevel::Low) => "low",
        crate::ThinkingMode::Effort(crate::EffortLevel::Medium) => "medium",
        crate::ThinkingMode::Effort(crate::EffortLevel::High) => "high",
        crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "xhigh",
    };
    Some(json!({ "effort": effort }))
}

/// Build the OpenAI Responses `/v1/responses` request body. System prompt maps
/// to the top-level `instructions` field. A single user text message with no
/// tool messages uses the `input: <string>` shorthand; otherwise `input` is an
/// array of items (message items + function_call_output items for tool results).
pub fn request_body(req: &CompletionRequest) -> Value {
    let has_tool_messages = req.messages.iter().any(|m| m.role == MsgRole::Tool);
    let single_user_string = req.messages.len() == 1
        && req.messages[0].role == MsgRole::User
        && req.messages[0].tool_calls.is_empty()
        && !has_tool_messages;

    let mut body = json!({
        "model": req.model,
        "stream": true,
        "max_output_tokens": req.max_tokens,
        "tool_choice": "auto",
    });

    if single_user_string {
        body["input"] = json!(req.messages[0].content.clone());
    } else {
        let mut input: Vec<Value> = Vec::new();
        for m in &req.messages {
            match m.role {
                MsgRole::User => input.push(json!({
                    "role": "user",
                    "content": [{ "type": "input_text", "text": m.content }],
                })),
                MsgRole::Assistant => {
                    let mut parts = vec![json!({ "type": "output_text", "text": m.content })];
                    for tc in &m.tool_calls {
                        // reasoning models emit prior function calls as input items on
                        // the next turn; an assistant message carrying tool_calls is
                        // represented as a function_call input item (not a message).
                        parts.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".into()),
                        }));
                    }
                    input.push(json!({
                        "role": "assistant",
                        "content": parts,
                    }));
                }
                MsgRole::Tool => input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content,
                })),
            }
        }
        body["input"] = Value::Array(input);
    }

    if let Some(sys) = &req.system {
        body["instructions"] = json!(sys);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools.iter().map(|t| json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
                "strict": false,
            })).collect(),
        );
    }
    if let Some(r) = reasoning_params(req) {
        body["reasoning"] = r;
    }
    body
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider openai_responses::tests::body_has_model_input_instructions_stream`
Expected: PASS.

- [ ] **Step 5: Add request_body edge-case tests**

Append to the `tests` module:

```rust
    #[test]
    fn body_with_tool_message_uses_input_array_with_function_call_output() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![
                Message::user("call the tool"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "a.txt"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                Message::tool_with_call_id("read_file", "call-1", "file body"),
            ],
            max_tokens: 64,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert!(body["input"].is_array(), "multi-message input must be an array");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        // third item is the tool result
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call-1");
        assert_eq!(input[2]["output"], "file body");
    }

    #[test]
    fn body_includes_tools_array_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["strict"], false);
    }

    #[test]
    fn body_emits_reasoning_effort_for_auto() {
        let req = CompletionRequest {
            model: "zen-gpt-demo".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn body_emits_xhigh_for_max_effort() {
        let req = CompletionRequest {
            model: "zen-gpt-demo".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning"]["effort"], "xhigh");
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_responses::tests`
Expected: PASS (all request-body tests green; parse tests not yet added).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-provider/src/openai_responses.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): OpenAI Responses request_body + tests"
```

---

## Task 3: OpenAI Responses client — SSE parse + accumulator

**Files:**
- Modify: `crates/zoid-provider/src/openai_responses.rs`
- Test: same file

**Interfaces:**
- Produces: `pub struct ResponsesToolAccum` (with `new()`, private feed, `take()`), `pub fn parse_event(data: &str, acc: &mut ResponsesToolAccum) -> Vec<ProviderEvent>`.

- [ ] **Step 1: Write the failing parse tests**

Append to the `tests` module in `openai_responses.rs`:

```rust
    #[test]
    fn parse_output_text_delta_yields_textdelta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hel"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn parse_reasoning_summary_delta_yields_thinking_delta() {
        let data = r#"{"type":"response.reasoning_summary_text.delta","delta":"pondering"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::ThinkingDelta("pondering".into())]
        );
    }

    #[test]
    fn parse_function_call_arguments_done_emits_toolcall() {
        let mut acc = ResponsesToolAccum::new();
        let d1 = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":"}"#;
        let d2 = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"\"a\"}"}"#;
        let done = r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","name":"read_file","output_index":0,"arguments":"{\"path\":\"a\"}"}"#;
        let _ = parse_event(d1, &mut acc);
        let _ = parse_event(d2, &mut acc);
        let out = parse_event(done, &mut acc);
        assert_eq!(
            out,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "fc_1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn parse_completed_emits_usage_then_done() {
        let data = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":20,"input_tokens_details":{"cached_tokens":3},"output_tokens_details":{"reasoning_tokens":5},"total_tokens":33}}}"#;
        let out = parse_event(data, &mut ResponsesToolAccum::new());
        assert_eq!(
            out,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cached: 3,
                    thinking_tokens: 5,
                }),
                ProviderEvent::Done,
            ]
        );
    }

    #[test]
    fn parse_incomplete_emits_truncated_then_done() {
        let data = r#"{"type":"response.incomplete"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::Truncated, ProviderEvent::Done]
        );
    }

    #[test]
    fn parse_failed_emits_error() {
        let data = r#"{"type":"response.failed","response":{"error":{"message":"rate limited"}}}"#;
        let out = parse_event(data, &mut ResponsesToolAccum::new());
        assert!(matches!(out.last(), Some(ProviderEvent::Error(e)) if e.contains("rate limited")));
    }

    #[test]
    fn parse_unknown_event_yields_nothing() {
        let data = r#"{"type":"response.created"}"#;
        assert!(parse_event(data, &mut ResponsesToolAccum::new()).is_empty());
    }

    #[test]
    fn parse_malformed_yields_nothing() {
        assert!(parse_event("not json", &mut ResponsesToolAccum::new()).is_empty());
        assert!(parse_event("", &mut ResponsesToolAccum::new()).is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider openai_responses::tests::parse`
Expected: FAIL — `parse_event` and `ResponsesToolAccum` not defined.

- [ ] **Step 3: Implement the accumulator and parse_event**

Add to `crates/zoid-provider/src/openai_responses.rs` (above the test module):

```rust
/// Accumulates OpenAI Responses function-call argument fragments by `item_id`,
/// flushing a complete `ToolCall` on `response.function_call_arguments.done`.
#[derive(Debug, Default)]
pub struct ResponsesToolAccum {
    by_item: std::collections::BTreeMap<String, String>,
}

impl ResponsesToolAccum {
    pub fn new() -> Self {
        Self::default()
    }

    fn feed(&mut self, item_id: &str, delta: &str) {
        self.by_item
            .entry(item_id.to_string())
            .or_default()
            .push_str(delta);
    }

    fn flush(&mut self, item_id: &str, name: &str, arguments: &str) -> Option<ProviderEvent> {
        self.by_item.remove(item_id);
        let args: Value = serde_json::from_str(arguments)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        // ASSUMPTION (spec §9 open-question #3): the `response.function_call_arguments.done`
        // event's `item_id` is usable as the tool-call `call_id` for the next turn's
        // `function_call_output` input item. OpenAI's Responses API distinguishes
        // `item_id` (the output item's id) from `call_id` (the function-call id the
        // model generated); the `function_call_output` input item on the next turn
        // is keyed by `call_id`, not `item_id`. If Zen's gateway surfaces them as
        // distinct values, this must source `call_id` from `response.output_item.added`
        // / `response.output_item.done` (which carry the full function_call output
        // item including `call_id`) rather than the `.done` event's `item_id`.
        // Until confirmed against a real Zen capture, we use `item_id` as a
        // best-effort id (matches Ollama's call-id-less fallback shape if empty).
        // TODO(spec §9 q3): confirm item_id == call_id against a real Zen capture.
        Some(ProviderEvent::ToolCall(ToolCall {
            id: item_id.to_string(),
            name: name.to_string(),
            args,
        }))
    }
}

/// Parse one OpenAI Responses SSE `data:` payload (the JSON object after
/// `data: `) into zero-or-more `ProviderEvent`s. `acc` accumulates
/// function-call argument fragments. Never panics. The caller handles the
/// stream end separately.
pub fn parse_event(data: &str, acc: &mut ResponsesToolAccum) -> Vec<ProviderEvent> {
    let data = data.trim();
    if data.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut out = Vec::new();
    match ty {
        "response.output_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    out.push(ProviderEvent::TextDelta(delta.to_string()));
                }
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    out.push(ProviderEvent::ThinkingDelta(delta.to_string()));
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let item_id = v.get("item_id").and_then(|i| i.as_str()).unwrap_or("");
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                acc.feed(item_id, delta);
            }
        }
        "response.function_call_arguments.done" => {
            let item_id = v.get("item_id").and_then(|i| i.as_str()).unwrap_or("");
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = v.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
            if let Some(ev) = acc.flush(item_id, name, arguments) {
                out.push(ev);
            }
        }
        "response.completed" => {
            if let Some(usage) = v
                .get("response")
                .and_then(|r| r.get("usage"))
            {
                let input = usage.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                let output = usage.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                let cached = usage
                    .get("input_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let thinking = usage
                    .get("output_tokens_details")
                    .and_then(|d| d.get("reasoning_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                out.push(ProviderEvent::Usage(Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cached,
                    thinking_tokens: thinking,
                }));
            }
            out.push(ProviderEvent::Done);
        }
        "response.incomplete" => {
            out.push(ProviderEvent::Truncated);
            out.push(ProviderEvent::Done);
        }
        "response.failed" => {
            let msg = v
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("response failed");
            out.push(ProviderEvent::Error(msg.to_string()));
        }
        _ => {
            // All other event types (created, in_progress, queued, output_item.*,
            // content_part.*, refusal.*, reasoning_summary_text.done, audio/web_search/
            // file_search/mcp/code_interpreter/image_gen, etc.) are parsed-but-ignored.
            tracing::trace!(event = %ty, "openai-responses: ignoring event");
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_responses::tests`
Expected: PASS — all request-body + parse tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_responses.rs
git commit -m "feat(provider): OpenAI Responses SSE parse + tool-call accumulator"
```

---

## Task 4: OpenAI Responses client — Provider impl + streaming tests

**Files:**
- Modify: `crates/zoid-provider/src/openai_responses.rs`
- Test: same file

**Interfaces:**
- Produces: `pub struct OpenAIResponsesProvider { … }` with `new(api_key: String)`, `with_base_url(impl Into<String>)`, `with_idle_timeout(Duration)`, implementing `Provider` (`stream`, `list_models`).

- [ ] **Step 1: Write the failing streaming test**

Append to the `tests` module in `openai_responses.rs`:

```rust
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "zen-gpt-demo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        }
    }

    #[tokio::test]
    async fn responses_routes_to_v1_responses() {
        // Server records the request line, then writes a minimal SSE stream
        // ending in response.completed so stream() terminates cleanly.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None::<String>));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\r\n\r\n\
                            data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = OpenAIResponsesProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/responses"), "expected /v1/responses, got: {first}");
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "hi")));
        // Exercise the usage path end-to-end (not just the pure-parse unit test):
        // response.completed carries usage, which must emit a Usage event.
        assert!(
            got.iter().any(|e| matches!(e, ProviderEvent::Usage(u) if u.input_tokens == 1 && u.output_tokens == 1)),
            "expected a Usage event from response.completed, got {got:?}"
        );
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::Done)));
    }

    #[tokio::test]
    async fn responses_idle_timeout_emits_error_when_stream_stalls() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await;
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        let provider = OpenAIResponsesProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done = tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung — idle timeout not enforced");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(matches!(got.last(), Some(ProviderEvent::Error(_))), "expected trailing idle-timeout Error, got {got:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider openai_responses::tests::responses`
Expected: FAIL — `OpenAIResponsesProvider` not defined.

- [ ] **Step 3: Implement the provider struct + Provider impl**

Add to `crates/zoid-provider/src/openai_responses.rs` (above the test module, after `parse_event`):

```rust
const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Streaming OpenAI Responses provider.
pub struct OpenAIResponsesProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenAIResponsesProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }
}

#[async_trait]
impl Provider for OpenAIResponsesProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let mut acc = ResponsesToolAccum::new();
        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/v1/responses", self.base_url))
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&request_body(req))
                .send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                return Ok(());
            }
            Err(_) => {
                let _ = sink
                    .send(ProviderEvent::Error(format!(
                        "provider request timed out after {}s (no response)",
                        self.idle_timeout.as_secs()
                    )))
                    .await;
                return Ok(());
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let text = match tokio::time::timeout(self.idle_timeout, resp.text()).await {
                Ok(Ok(t)) => t,
                _ => String::new(),
            };
            let _ = sink
                .send(ProviderEvent::Error(format!("HTTP {status}: {text}")))
                .await;
            return Ok(());
        }
        let mut stream = resp.bytes_stream().eventsource();
        let mut ended_early = false;
        loop {
            let item = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    let _ = sink
                        .send(ProviderEvent::Error(format!(
                            "provider idle timeout: no data for {}s",
                            self.idle_timeout.as_secs()
                        )))
                        .await;
                    ended_early = true;
                    break;
                }
            };
            let item = match item {
                Ok(ev) => ev,
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    ended_early = true;
                    break;
                }
            };
            // Responses has no [DONE] terminator; the stream ends when the server
            // closes it (response.completed carries Done). Skip empty keepalive data.
            if item.data.is_empty() {
                continue;
            }
            let mut got_done = false;
            for pe in parse_event(&item.data, &mut acc) {
                if matches!(pe, ProviderEvent::Done) {
                    got_done = true;
                }
                if sink.send(pe).await.is_err() {
                    ended_early = true;
                    break;
                }
            }
            if got_done || ended_early {
                break;
            }
        }
        if !ended_early {
            // Transport closed without an explicit response.completed — flush Done
            // (the trailing-flush philosophy, matching openai_compat.rs).
            let _ = sink.send(ProviderEvent::Done).await;
        }
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Ok(crate::parse_data_id_models(&resp.text().await?))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_responses`
Expected: PASS — all request-body, parse, and streaming tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_responses.rs
git commit -m "feat(provider): OpenAIResponsesProvider streaming impl + tests"
```

---

## Task 5: Google Gemini client — request body + pure parse (zoid-provider)

**Files:**
- Create: `crates/zoid-provider/src/google_gemini.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod google_gemini;`)
- Test: same file

**Interfaces:**
- Consumes: `crate::{CompletionRequest, Message, MsgRole, Provider, ProviderEvent, ToolCall, Usage, ThinkingMode}`.
- Produces: `pub fn request_body(req: &CompletionRequest, model: &str) -> (String, Value)` (returns `(path_suffix, body)`), `pub fn parse_chunk(obj: &Value) -> Vec<ProviderEvent>`, `pub struct GoogleGeminiProvider`.

- [ ] **Step 1: Write the failing request_body test**

Create `crates/zoid-provider/src/google_gemini.rs`:

```rust
//! The generic Google Gemini client (POST {base}/v1/models/<model>:streamGenerateContent
//! ?alt=sse, candidates[].content.parts[] with text/functionCall/thought, usageMetadata).
//! Self-contained like the other leaves; uses the crate's `Provider` seam. No
//! opencode-zen-specifics — a generic leaf reusable by direct-Gemini etc.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// Build the Gemini generate request: returns (path_suffix, body). The model
/// lives in the path, not the body. `contents` carries the conversation;
/// `systemInstruction` carries the system prompt; `tools` carries function
/// declarations; `generationConfig` carries maxOutputTokens + thinkingConfig.
pub fn request_body(req: &CompletionRequest, model: &str) -> (String, Value) {
    let path = format!("v1/models/{model}:streamGenerateContent");

    let mut contents: Vec<Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            MsgRole::User => contents.push(json!({
                "role": "user",
                "parts": [{ "text": m.content }],
            })),
            MsgRole::Assistant => {
                let mut parts: Vec<Value> = if m.content.is_empty() {
                    Vec::new()
                } else {
                    vec![json!({ "text": m.content })]
                };
                for tc in &m.tool_calls {
                    parts.push(json!({
                        "functionCall": {
                            "id": tc.id,
                            "name": tc.name,
                            "args": tc.args,
                        }
                    }));
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            MsgRole::Tool => contents.push(json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "name": m.tool_name.clone().unwrap_or_default(),
                        "response": { "content": m.content },
                    }
                }],
            })),
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": { "maxOutputTokens": req.max_tokens },
    });

    if let Some(sys) = &req.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
    }
    if !req.tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": req.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })).collect::<Vec<_>>()
        }]);
    }
    // thinking → thinkingConfig.includeThoughts (Gemini surfaces thought parts
    // separately). Off omits the config entirely.
    if !matches!(req.thinking, crate::ThinkingMode::Off) {
        body["generationConfig"]["thinkingConfig"] = json!({ "includeThoughts": true });
    }
    (path, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolSpec};
    use serde_json::json;

    #[test]
    fn body_path_includes_model_and_endpoint() {
        let req = CompletionRequest {
            model: "zen-gemini-demo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 128,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (path, body) = request_body(&req, "zen-gemini-demo");
        assert_eq!(path, "v1/models/zen-gemini-demo:streamGenerateContent");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 128);
        assert!(body.get("systemInstruction").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("thinkingConfig").is_none());
    }
}
```

Add `pub mod google_gemini;` to `crates/zoid-provider/src/lib.rs` (after `pub mod openai_responses;`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider google_gemini::tests::body_path_includes_model_and_endpoint`
Expected: FAIL — module not declared (until lib.rs edit) / function not found.

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p zoid-provider google_gemini::tests::body_path_includes_model_and_endpoint`
Expected: PASS.

- [ ] **Step 4: Add request_body edge-case tests + commit**

Append to the `tests` module:

```rust
    #[test]
    fn body_with_system_prompt_emits_system_instruction() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be terse");
    }

    #[test]
    fn body_with_tools_emits_function_declarations() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: crate::ThinkingMode::Off,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "read_file");
    }

    #[test]
    fn body_tool_message_emits_function_response() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![
                Message::user("call it"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "fc_1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "a"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                Message::tool("read_file", "file body"),
            ],
            max_tokens: 64,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (_, body) = request_body(&req, "m");
        let contents = body["contents"].as_array().unwrap();
        // assistant tool_calls → model role with functionCall part
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read_file");
        // tool result → user role with functionResponse part
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "read_file");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["response"]["content"], "file body");
    }

    #[test]
    fn body_thinking_on_emits_thinking_config() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["generationConfig"]["thinkingConfig"]["includeThoughts"], true);
    }
```

Run: `cargo test -p zoid-provider google_gemini::tests`
Expected: PASS.

```bash
git add crates/zoid-provider/src/google_gemini.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): Google Gemini request_body + tests"
```

---

## Task 6: Google Gemini client — chunk parse + Provider impl + streaming tests

**Files:**
- Modify: `crates/zoid-provider/src/google_gemini.rs`
- Test: same file

**Interfaces:**
- Produces: `pub fn parse_chunk(obj: &Value) -> Vec<ProviderEvent>`, `pub struct GoogleGeminiProvider` with `new`/`with_base_url`/`with_idle_timeout` implementing `Provider`.

- [ ] **Step 1: Write the failing parse tests**

Append to the `tests` module in `google_gemini.rs`:

```rust
    #[test]
    fn parse_text_part_yields_textdelta() {
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "Hel" }] }
            }]
        });
        assert_eq!(parse_chunk(&chunk), vec![ProviderEvent::TextDelta("Hel".into())]);
    }

    #[test]
    fn parse_function_call_part_yields_toolcall() {
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{
                    "functionCall": { "id": "fc_1", "name": "read_file", "args": { "path": "a" } }
                }] }
            }]
        });
        assert_eq!(
            parse_chunk(&chunk),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "fc_1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn parse_thought_part_yields_thinking_delta() {
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "thought": true, "text": "pondering" }] }
            }]
        });
        assert_eq!(
            parse_chunk(&chunk),
            vec![ProviderEvent::ThinkingDelta("pondering".into())]
        );
    }

    #[test]
    fn parse_finish_reason_max_tokens_yields_truncated() {
        let chunk = json!({
            "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": "MAX_TOKENS" }]
        });
        assert_eq!(parse_chunk(&chunk), vec![ProviderEvent::Truncated]);
    }

    #[test]
    fn parse_usage_metadata_yields_usage_then_done() {
        let chunk = json!({
            "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": "STOP" }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 5,
                "totalTokenCount": 35
            }
        });
        assert_eq!(
            parse_chunk(&chunk),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cached: 3,
                    thinking_tokens: 5,
                }),
                ProviderEvent::Done,
            ]
        );
    }

    #[test]
    fn parse_empty_candidates_yields_nothing() {
        let chunk = json!({ "candidates": [] });
        assert!(parse_chunk(&chunk).is_empty());
    }

    #[test]
    fn parse_malformed_yields_nothing() {
        assert!(parse_chunk(&json!(42)).is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider google_gemini::tests::parse`
Expected: FAIL — `parse_chunk` not defined.

- [ ] **Step 3: Implement parse_chunk**

Add to `crates/zoid-provider/src/google_gemini.rs` (above the test module, after `request_body`):

```rust
/// Parse one Gemini `GenerateContentResponse` chunk (a parsed JSON object) into
/// zero-or-more `ProviderEvent`s. `usageMetadata` (final chunk) emits `Usage`
/// then `Done`; `finishReason: MAX_TOKENS` emits `Truncated`. Never panics.
pub fn parse_chunk(obj: &Value) -> Vec<ProviderEvent> {
    let mut out = Vec::new();
    if let Some(cands) = obj.get("candidates").and_then(|c| c.as_array()) {
        for cand in cands {
            if let Some(parts) = cand
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    // thought part (only present when includeThoughts true)
                    let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            out.push(if is_thought {
                                ProviderEvent::ThinkingDelta(text.to_string())
                            } else {
                                ProviderEvent::TextDelta(text.to_string())
                            });
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let id = fc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args = fc.get("args").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
                        out.push(ProviderEvent::ToolCall(ToolCall { id, name, args }));
                    }
                }
            }
            if let Some(reason) = cand.get("finishReason").and_then(|f| f.as_str()) {
                if reason == "MAX_TOKENS" {
                    out.push(ProviderEvent::Truncated);
                }
            }
        }
    }
    if let Some(usage) = obj.get("usageMetadata") {
        let input = usage.get("promptTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
        let output = usage.get("candidatesTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
        let cached = usage.get("cachedContentTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
        let thinking = usage.get("thoughtsTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
        out.push(ProviderEvent::Usage(Usage {
            input_tokens: input,
            output_tokens: output,
            cached,
            thinking_tokens: thinking,
        }));
        out.push(ProviderEvent::Done);
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider google_gemini::tests::parse`
Expected: PASS.

- [ ] **Step 5: Write the failing streaming test**

Append to the `tests` module:

```rust
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "zen-gemini-demo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        }
    }

    #[tokio::test]
    async fn gemini_routes_to_stream_generate_content() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None::<String>));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\r\n\r\n\
                            data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":1}}\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = GoogleGeminiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("streamGenerateContent"),
            "expected streamGenerateContent, got: {first}"
        );
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "hi")));
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::Done)));
    }
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p zoid-provider google_gemini::tests::gemini_routes_to_stream_generate_content`
Expected: FAIL — `GoogleGeminiProvider` not defined.

- [ ] **Step 7: Implement the provider struct + Provider impl**

Add to `crates/zoid-provider/src/google_gemini.rs` (above the test module, after `parse_chunk`):

```rust
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Streaming Google Gemini provider.
pub struct GoogleGeminiProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl GoogleGeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }
}

#[async_trait]
impl Provider for GoogleGeminiProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let model = req.model.clone();
        let (path, body) = request_body(req, &model);
        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/{}?alt=sse", self.base_url, path))
                .header("x-goog-api-key", &self.api_key)
                .header("content-type", "application/json")
                .json(&body)
                .send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                return Ok(());
            }
            Err(_) => {
                let _ = sink
                    .send(ProviderEvent::Error(format!(
                        "provider request timed out after {}s (no response)",
                        self.idle_timeout.as_secs()
                    )))
                    .await;
                return Ok(());
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let text = match tokio::time::timeout(self.idle_timeout, resp.text()).await {
                Ok(Ok(t)) => t,
                _ => String::new(),
            };
            let _ = sink
                .send(ProviderEvent::Error(format!("HTTP {status}: {text}")))
                .await;
            return Ok(());
        }
        let mut stream = resp.bytes_stream().eventsource();
        let mut ended_early = false;
        loop {
            let item = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    let _ = sink
                        .send(ProviderEvent::Error(format!(
                            "provider idle timeout: no data for {}s",
                            self.idle_timeout.as_secs()
                        )))
                        .await;
                    ended_early = true;
                    break;
                }
            };
            let item = match item {
                Ok(ev) => ev,
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    ended_early = true;
                    break;
                }
            };
            if item.data.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&item.data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut got_done = false;
            for pe in parse_chunk(&v) {
                if matches!(pe, ProviderEvent::Done) {
                    got_done = true;
                }
                if sink.send(pe).await.is_err() {
                    ended_early = true;
                    break;
                }
            }
            if got_done || ended_early {
                break;
            }
        }
        if !ended_early {
            let _ = sink.send(ProviderEvent::Done).await;
        }
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Gemini's list endpoint is GET {base}/v1/models; returns {models:[{name:"models/<id>"}]}.
        // Normalize to bare ids for parity with the picker's free-text model field.
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await?;
        let body = resp.text().await?;
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        Ok(v
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("name")
                            .and_then(|n| n.as_str())
                            .and_then(|s| s.strip_prefix("models/"))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p zoid-provider google_gemini`
Expected: PASS — all request-body, parse, and streaming tests green.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-provider/src/google_gemini.rs
git commit -m "feat(provider): GoogleGeminiProvider parse + streaming impl + tests"
```

---

## Task 7: OpenCodeZenProvider — 4-way routing (zoid-provider)

**Files:**
- Create: `crates/zoid-provider/src/opencode_zen.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod opencode_zen;`)
- Test: same file

**Interfaces:**
- Consumes: `crate::openai_compat::OpenAICompatProvider`, `crate::anthropic::AnthropicProvider`, `crate::openai_responses::OpenAIResponsesProvider`, `crate::google_gemini::GoogleGeminiProvider`.
- Produces: `pub struct OpenCodeZenProvider` with `new(api_key: String)`, `with_base_url(impl Into<String>)`, `with_idle_timeout(Duration)`, implementing `Provider`.

- [ ] **Step 1: Write the failing routing tests**

Create `crates/zoid-provider/src/opencode_zen.rs`:

```rust
//! The dedicated OpenCode Zen provider: holds a static per-model wire-shape map
//! and delegates `stream()`/`list_models()` to one of four sub-clients
//! (OpenAICompatProvider, AnthropicProvider, OpenAIResponsesProvider,
//! GoogleGeminiProvider) based on the active model id.

use crate::anthropic::AnthropicProvider;
use crate::google_gemini::GoogleGeminiProvider;
use crate::openai_compat::OpenAICompatProvider;
use crate::openai_responses::OpenAIResponsesProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZenWireShape {
    OpenAIChat,
    AnthropicMessages,
    OpenAIResponses,
    GoogleGemini,
}

const ZEN_MODELS: &[(&str, ZenWireShape)] = &[
    ("zen-chat-demo", ZenWireShape::OpenAIChat),
    ("zen-claude-demo", ZenWireShape::AnthropicMessages),
    ("zen-gpt-demo", ZenWireShape::OpenAIResponses),
    ("zen-gemini-demo", ZenWireShape::GoogleGemini),
];

pub struct OpenCodeZenProvider {
    api_key: String,
    base_url: String,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeZenProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("opencode-zen")
                .unwrap_or("https://opencode.ai/zen")
                .to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }

    fn wire_shape_for(&self, model: &str) -> ZenWireShape {
        match ZEN_MODELS.iter().find(|(id, _)| *id == model) {
            Some((_, shape)) => *shape,
            None => {
                tracing::warn!(
                    model = %model,
                    "opencode-zen: model not in wire-shape map; defaulting to OpenAIChat"
                );
                ZenWireShape::OpenAIChat
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn wire_shape_for_known_models_matches_table() {
        for (id, shape) in ZEN_MODELS {
            let p = OpenCodeZenProvider::new("k".into());
            assert_eq!(p.wire_shape_for(id), *shape, "mismatch for {id}");
        }
    }

    #[test]
    fn wire_shape_for_unknown_defaults_to_openai_chat() {
        let p = OpenCodeZenProvider::new("k".into());
        assert_eq!(p.wire_shape_for("unknown-model"), ZenWireShape::OpenAIChat);
    }

    #[test]
    fn with_base_url_propagates_to_subclient() {
        let p = OpenCodeZenProvider::new("k".into()).with_base_url("https://example.test/zen/");
        assert_eq!(p.base_url, "https://example.test/zen");
    }
}
```

Add `pub mod opencode_zen;` to `crates/zoid-provider/src/lib.rs` (after `pub mod google_gemini;`).

- [ ] **Step 2: Run tests to verify they pass (routing logic is self-contained)**

Run: `cargo test -p zoid-provider opencode_zen::tests`
Expected: PASS — the routing-logic tests don't depend on the `Provider` impl.

- [ ] **Step 3: Write the failing path-routing streaming tests**

Append to the `tests` module in `opencode_zen.rs` (mirrors `opencode_go.rs`'s `spawn_recording_server`):

```rust
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn spawn_recording_server() -> (
        std::net::SocketAddr,
        std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: [DONE]\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (addr, recorded)
    }

    fn zen_req(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        }
    }

    #[tokio::test]
    async fn chat_model_routes_to_chat_completions() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("zen-chat-demo"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/chat/completions"), "expected /v1/chat/completions, got: {first}");
    }

    #[tokio::test]
    async fn anthropic_model_routes_to_messages() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("zen-claude-demo"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/messages"), "expected /v1/messages, got: {first}");
    }

    #[tokio::test]
    async fn responses_model_routes_to_responses() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("zen-gpt-demo"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/responses"), "expected /v1/responses, got: {first}");
    }

    #[tokio::test]
    async fn gemini_model_routes_to_stream_generate_content() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("zen-gemini-demo"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("streamGenerateContent"),
            "expected streamGenerateContent, got: {first}"
        );
    }
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p zoid-provider opencode_zen::tests::chat_model_routes_to_chat_completions`
Expected: FAIL — `Provider` impl for `OpenCodeZenProvider` not defined.

- [ ] **Step 5: Implement the Provider trait**

Add to `crates/zoid-provider/src/opencode_zen.rs` (after `wire_shape_for`, before the test module):

```rust
#[async_trait]
impl Provider for OpenCodeZenProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        match self.wire_shape_for(&req.model) {
            ZenWireShape::OpenAIChat => {
                OpenAICompatProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::AnthropicMessages => {
                AnthropicProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::OpenAIResponses => {
                OpenAIResponsesProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::GoogleGemini => {
                GoogleGeminiProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Both OpenAI-shape and Anthropic /v1/models share the {data:[{id}]} shape;
        // Gemini's /v1/models differs ({models:[{name:"models/<id>"}]}), but the Zen
        // gateway normalizes to the OpenAI shape at /v1/models. Reuse the OpenAI-compat
        // client's list_models (hits {base}/v1/models).
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_idle_timeout(self.idle_timeout)
            .list_models()
            .await
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-provider opencode_zen`
Expected: PASS — all routing-logic and path-routing tests green.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-provider/src/opencode_zen.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): OpenCodeZenProvider 4-way wire-shape routing + tests"
```

---

## Task 8: Secrets section label prettification (zoid-tui + zoid bin)

**Files:**
- Modify: `crates/zoid-tui/src/config_view.rs` (`FieldRow::secret_key`, `build_sections` Secrets section)
- Modify: `crates/zoid-tui/src/route.rs` (test `FieldRow` literals gain `secret_key: None`)
- Modify: `crates/zoid/src/main.rs` (`current_config_field` returns `(label, kind, secret_key)`; commit/clear flows use `secret_key`; tests)
- Test: `crates/zoid-tui/src/config_view.rs`, `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: existing `FieldRow`, `build_sections`, `current_config_field`, secret commit/clear flows. (`field_target` is unchanged — it keys off `FieldKind::Secret`, not the label.)
- Produces: `FieldRow.secret_key: Option<&'static str>`; Secrets section rows render `opencode`/`ollama`/`anthropic` with `secret_key` set to the real env-var names.

- [ ] **Step 1: Write the failing test for build_sections labels**

In `crates/zoid-tui/src/config_view.rs`, add to the `tests` module:

```rust
    #[test]
    fn secrets_section_renders_friendly_labels_with_secret_key() {
        let cfg = Config::default();
        let prov = Provenance {
            provider: Source::Default,
            base_url: Source::Default,
            model: Source::Default,
            context_target: Source::Default,
            auto_evict_cold: Source::Default,
            compact_threshold_pct: Source::Default,
            band_headroom_pct: Source::Default,
            recent_n: Source::Default,
            reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default,
            approval: Source::Default,
        };
        let ks = [
            ("OLLAMA_API_KEY", SecretStatus::NotSet),
            ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
            ("OPENCODE_GO_API_KEY", SecretStatus::NotSet),
        ];
        let sections = build_sections(&cfg, &prov, &ks);
        let sec = sections.iter().find(|s| s.title == "Secrets").unwrap();
        let labels: Vec<&str> = sec.rows.iter().map(|r| r.label).collect();
        assert_eq!(labels, vec!["ollama", "anthropic", "opencode"]);
        let keys: Vec<Option<&str>> = sec.rows.iter().map(|r| r.secret_key).collect();
        assert_eq!(
            keys,
            vec![
                Some("OLLAMA_API_KEY"),
                Some("ANTHROPIC_API_KEY"),
                Some("OPENCODE_GO_API_KEY"),
            ]
        );
    }
```

Also update the existing `builds_four_sections_with_env_shadow` test: its `ks` array currently has only 2 entries; the new Secrets section expects 3 friendly labels, so either extend `ks` to 3 entries or relax the assertions. Extend `ks` to include the opencode row and assert the row count.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui config_view::tests`
Expected: FAIL — `FieldRow` has no `secret_key` field (compile error).

- [ ] **Step 3: Add the `secret_key` field to FieldRow and update build_sections**

In `crates/zoid-tui/src/config_view.rs`:

Change the `FieldRow` struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRow {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub source: Source,
    pub env_shadowed: bool,
    /// The secret-store key for `FieldKind::Secret` rows. When set, the row
    /// renders `label` (a friendly name) but the edit/clear flows key the
    /// secret store by `secret_key`. `None` for non-secret rows and any future
    /// secret row whose label still equals its env-var name.
    pub secret_key: Option<&'static str>,
}
```

Add `secret_key: None` to every existing `FieldRow { … }` literal in this file (Provider & Model, Economy, Interface sections — all the non-secret rows). 

Then change the Secrets section builder to map `key_status` into rows with friendly labels + secret keys:

```rust
    let secrets = Section {
        title: "Secrets".into(),
        rows: key_status
            .iter()
            .map(|(name, st)| {
                let (value, shadowed) = match st {
                    SecretStatus::Set { from_env: true } => ("set".to_string(), true),
                    SecretStatus::Set { from_env: false } => ("set".to_string(), false),
                    SecretStatus::NotSet => ("not set".to_string(), false),
                };
                let (label, secret_key) = friendly_secret_label(*name);
                FieldRow {
                    label,
                    value,
                    kind: FieldKind::Secret,
                    source: if shadowed {
                        Source::Env
                    } else {
                        Source::Default
                    },
                    env_shadowed: shadowed,
                    secret_key,
                }
            })
            .collect(),
    };
```

Add the helper above `build_sections`:

```rust
/// Map a secret env-var name to a friendly display label + the (unchanged)
/// secret-store key. Unknown env vars fall back to the raw name as both.
/// `name` is `&'static str` because `key_status` carries `&'static str` entries.
fn friendly_secret_label(name: &'static str) -> (&'static str, Option<&'static str>) {
    match name {
        "OPENCODE_GO_API_KEY" => ("opencode", Some("OPENCODE_GO_API_KEY")),
        "OLLAMA_API_KEY" => ("ollama", Some("OLLAMA_API_KEY")),
        "ANTHROPIC_API_KEY" => ("anthropic", Some("ANTHROPIC_API_KEY")),
        other => (other, None), // fallback: label == key
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui config_view::tests`
Expected: PASS for `config_view` tests. There may be compile errors in `route.rs` test `FieldRow` literals (they don't set `secret_key`).

- [ ] **Step 5: Fix route.rs test FieldRow literals**

In `crates/zoid-tui/src/route.rs`, add `secret_key: None` to every `FieldRow { … }` literal in the test modules (search for `FieldRow {` in that file; each needs the new field). Run:

Run: `cargo test -p zoid-tui`
Expected: PASS — all zoid-tui tests green.

- [ ] **Step 6: Update main.rs commit/clear flows to use secret_key**

In `crates/zoid/src/main.rs`:

The commit flow (`Action::ConfigFieldCommit`, ~line 3714) currently does `s.set(label, &buffer)`. `current_config_field` returns `(label, kind)`. Change `current_config_field` to also return the row's `secret_key`:

```rust
/// The (label, kind, secret_key) of the row under the config cursor, if any.
fn current_config_field(app: &App) -> Option<(&'static str, zoid_tui::config_view::FieldKind, Option<&'static str>)> {
    app.shell
        .config_sections
        .get(app.shell.config_section)
        .and_then(|s| s.rows.get(app.shell.config_field))
        .map(|r| (r.label, r.kind.clone(), r.secret_key))
}
```

Update **every** caller of `current_config_field` (there are 7 — search `current_config_field(app)` in main.rs) to destructure the third element. Non-secret callers ignore it with `_`:

- ~3060 `apply_models_fetched`: `current_config_field(app).map(|(l, _, _)| l) != Some("model")`.
- ~3712 `Action::ConfigFieldCommit`: `if let (Some((label, kind, secret_key)), Some(buffer)) =` — change `s.set(label, &buffer)` → `let key = secret_key.unwrap_or(label); s.set(key, &buffer)` and update the eprintln `{label}` → `{key}`. The "secret store unavailable" eprintln likewise uses `key`.
- ~3748 `Action::ConfigToggle`: `if let Some((label, _kind, _)) = current_config_field(app)`.
- ~3765 (the other toggle/picker commit): `if let Some((label, _, _)) = current_config_field(app)`.
- ~3862 the provider-picker commit: `let label = current_config_field(app).map(|(l, _, _)| l).unwrap_or("")`.
- ~3932 `Action::ConfigSaveToRepo`: `if let Some((label, kind, _)) = current_config_field(app)`.
- ~3947 `Action::ConfigClearSecret`: `if let Some((label, kind, secret_key)) = current_config_field(app)` and `s.clear(secret_key.unwrap_or(label))`.

- [ ] **Step 7: Write the failing bin test for key resolution**

Add to `main.rs` tests:

```rust
    #[test]
    fn key_env_for_opencode_zen_is_opencode_go_api_key() {
        assert_eq!(key_env_for("opencode-zen"), Some("OPENCODE_GO_API_KEY"));
    }

    #[test]
    fn entry_requires_key_opencode_zen_is_true() {
        assert!(entry_requires_key("opencode-zen"));
    }
```

- [ ] **Step 8: Run tests to verify they pass (after adding the arm in Task 9)**

This test depends on the `opencode-zen` arm in `key_env_for`, which is added in Task 9. Run after Task 9 Step 3:

Run: `cargo test -p zoid key_env_for_opencode_zen`
Expected: PASS.

- [ ] **Step 9: Update the provider-options count test (Task 1 added a 5th provider)**

Adding the `opencode-zen` registry entry (Task 1) grows `PROVIDERS` to 5, so `provider_options()` now returns 5 entries. The existing test `provider_options_annotate_endpoints_and_mark_planned` in `crates/zoid-tui/src/config_view.rs` asserts `opts.len() == 4` — it will fail. Update it:

```rust
    #[test]
    fn provider_options_annotate_endpoints_and_mark_planned() {
        let opts = provider_options("ollama-cloud");
        let cloud = opts.iter().find(|o| o.id == "ollama-cloud").unwrap();
        assert!(cloud.is_current);
        assert!(cloud.selectable);
        assert!(cloud.detail.contains("https://ollama.com"));

        // All surviving providers are selectable (no [planned] rows remain).
        assert_eq!(opts.len(), 5);
        let zen = opts.iter().find(|o| o.id == "opencode-zen").unwrap();
        assert!(zen.selectable);
        assert!(zen.detail.contains("https://opencode.ai/zen"));
        let api = opts.iter().find(|o| o.id == "anthropic-api").unwrap();
        assert!(api.selectable);
        assert!(api.detail.contains("https://api.anthropic.com"));
    }
```

Run: `cargo test -p zoid-tui provider_options_annotate_endpoints`
Expected: PASS.

- [ ] **Step 10: Regenerate the provider-picker insta snapshots**

Three `insta` snapshot files embed the full provider-picker list and will break when `opencode-zen` is added (Task 1): `shell_snapshot__config_overlay_provider_picker.snap`, `shell_snapshot__config_overlay_narrow_degrades.snap`, `shell_snapshot__provider_switch_card.snap`. The snapshots assert the rendered picker, which now includes the `opencode · zen` row.

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Expected: the three snapshots update with the new 5th row; the rest are unchanged.

Then review the diffs to confirm only the expected new row was added (no incidental whitespace drift):

Run: `git diff crates/zoid-tui/tests/snapshots/`

If the diff shows only the new `opencode · zen` row in each affected snapshot, accept the updates:

Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS (snapshots now match).

- [ ] **Step 11: Commit**

```bash
git add crates/zoid-tui/src/config_view.rs crates/zoid-tui/src/route.rs crates/zoid-tui/tests/snapshots/ crates/zoid/src/main.rs
git commit -m "feat(ui): prettify Secrets labels via FieldRow::secret_key decoupling"
```

---

## Task 9: Bin wiring — opencode-zen arms (zoid bin)

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Test: same file

**Interfaces:**
- Consumes: `zoid_provider::opencode_zen::OpenCodeZenProvider`, `zoid_provider::model::entry`/`canonical_id` (from Task 1).
- Produces: `opencode-zen` arms in `key_env_for`, `select_provider`, `provider_for_id`.

- [ ] **Step 1: Write the failing test for select_provider routing**

Add to `main.rs` tests (this asserts the family arm exists; it constructs a provider for an id whose family is `opencode-zen`):

```rust
    #[test]
    fn key_env_for_opencode_zen_maps_to_shared_go_key() {
        assert_eq!(key_env_for("opencode-zen"), Some("OPENCODE_GO_API_KEY"));
        // sanity: Go unchanged
        assert_eq!(key_env_for("opencode-go"), Some("OPENCODE_GO_API_KEY"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid key_env_for_opencode_zen_maps_to_shared_go_key`
Expected: FAIL — `key_env_for("opencode-zen")` returns `Some("OLLAMA_API_KEY")` (the `_` arm).

- [ ] **Step 3: Add the three arms**

In `crates/zoid/src/main.rs`:

`key_env_for` (~864) — add the `opencode-zen` family arm alongside `opencode-go`:

```rust
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("opencode-go") | Some("opencode-zen") => Some("OPENCODE_GO_API_KEY"),
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}
```

`select_provider` (~912) — add the `opencode-zen` family arm (before the `_` arm):

```rust
        "opencode-zen" => match key_for("OPENCODE_GO_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::opencode_zen::OpenCodeZenProvider::new(k).with_base_url(base_url),
                ),
                "opencode-zen",
                true,
            ),
            None => (default_provider(), "opencode-zen", false),
        },
```

`provider_for_id` (~1016) — add the `opencode-zen` family arm:

```rust
        "opencode-zen" => key_for("OPENCODE_GO_API_KEY").map(|k| {
            Arc::new(zoid_provider::opencode_zen::OpenCodeZenProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid key_env_for_opencode_zen_maps_to_shared_go_key && cargo test -p zoid key_env_for_opencode_zen_is_opencode_go_api_key`
Expected: PASS.

- [ ] **Step 5: Full workspace build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: PASS — all workspace tests green (including the new Tasks 1-9 tests and the updated `selectable_has_five_providers`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): wire opencode-zen provider arms (shared OPENCODE_GO_API_KEY)"
```

---

## Task 10: Spec-driven regression lock + final verification

**Files:**
- Modify: none (verification only); optionally add a regression test to `crates/zoid-model/src/lib.rs` for the placeholder caps table.

- [ ] **Step 1: Add a table-driven caps regression lock**

In `crates/zoid-model/src/lib.rs`, add to `opencode_zen_tests`:

```rust
    #[test]
    fn opencode_zen_placeholder_caps_match_table() {
        let cases: &[(&str, u64, u64, bool, bool)] = &[
            // (id, context_window, max_output, tools, prompt_cache)
            ("zen-chat-demo", 128_000, 0, true, false),
            ("zen-claude-demo", 200_000, 0, false, true),
            ("zen-gpt-demo", 200_000, 0, true, false),
            ("zen-gemini-demo", 1_000_000, 0, true, false),
        ];
        for (id, ctx, max_out, tools, pc) in cases {
            let info = model_info(id);
            assert_eq!(info.context_window, *ctx, "ctx mismatch for {id}");
            assert_eq!(info.max_output, *max_out, "max_output mismatch for {id}");
            assert_eq!(info.tools, *tools, "tools mismatch for {id}");
            assert_eq!(info.prompt_cache, *pc, "prompt_cache mismatch for {id}");
        }
    }

    #[test]
    fn opencode_go_entry_unchanged() {
        // Regression: the Go entry must NOT have been modified by the Zen slice.
        let e = entry("opencode-go").unwrap();
        assert_eq!(e.display, "opencode · go");
        assert_eq!(e.family, "opencode-go");
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://opencode.ai/zen/go"
            }
        );
        assert_eq!(e.models.len(), 13);
    }
```

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`
Expected: PASS — all green.

- [ ] **Step 3: Clippy + fmt**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: no warnings; formatting clean.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "test(model): regression lock for opencode-zen placeholder caps + opencode-go unchanged"
```