# OpenCode Go Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `opencode-go` as a selectable provider in zoid, routing its 13 models across two wire shapes (OpenAI Chat Completions + Anthropic Messages) via a dedicated `OpenCodeGoProvider` that delegates per-model.

**Architecture:** A new generic `OpenAICompatProvider` client (OpenAI `/v1/chat/completions` SSE + tool-calling with fragment accumulation) plus a dedicated `OpenCodeGoProvider` holding a static per-model wire-shape map and delegating `stream()`/`list_models()` to either the new client or the existing `AnthropicProvider` (with a `base_url` override). One new registry entry in `zoid-model`, 12 new `MODEL_CAPS` entries + 1 in-place correction (`deepseek-v4-pro`), a `Message.tool_call_id` field, and three new arms in the bin's provider-selection functions.

**Tech Stack:** Rust 2021, `reqwest` 0.12, `eventsource-stream` 0.2, `futures-util` 0.3, `tokio` 1, `serde_json` 1, `async-trait` 0.1. Tests: `#[test]` / `#[tokio::test]` with throwaway `tokio::net::TcpListener` stubs (no live-endpoint CI).

**Spec:** `docs/superpowers/specs/2026-07-05-opencode-go-provider-design.md`

---

## File Structure

**Created:**
- `crates/zoid-provider/src/openai_compat.rs` — generic OpenAI Chat Completions client (`OpenAICompatProvider` + pure `request_body`/`parse_chunk` + tool-call fragment accumulator + `list_models`).
- `crates/zoid-provider/src/opencode_go.rs` — dedicated `OpenCodeGoProvider` + static `GO_MODELS` wire-shape map; delegates to `OpenAICompatProvider` or `AnthropicProvider`.

**Modified:**
- `crates/zoid-provider/src/lib.rs` — add `pub mod openai_compat; pub mod opencode_go;`; add `tool_call_id: Option<String>` field to `Message`; add `Message::tool_with_call_id` constructor; extract `parse_data_id_models` helper (shared by `anthropic.rs` and `openai_compat.rs`).
- `crates/zoid-provider/src/anthropic.rs` — call the shared `parse_data_id_models` instead of the local `parse_anthropic_models` (the local fn stays as a thin re-export for existing call sites, or is replaced — keep the diff minimal).
- `crates/zoid-model/src/lib.rs` — add `opencode-go` `ProviderEntry`; add 12 `MODEL_CAPS` entries; correct the existing `deepseek-v4-pro` entry (128k→1M, max_output 384K, prompt_cache true); update two existing tests that assert the old wrong values.
- `crates/zoid/src/agent.rs` — `map_msg`'s `ChatMsg::ToolResult` arm populates `tool_call_id` from the event's `id` (currently discarded).
- `crates/zoid/src/main.rs` — `key_env_for` + `select_provider` + `provider_for_id` gain `opencode-go` arms; `key_status` array gains `OPENCODE_GO_API_KEY`.

---

## Task 1: Add `Message.tool_call_id` field + `tool_with_call_id` constructor

**Why first:** the `OpenAICompatProvider` request body (Task 4) reads `tool_call_id` from tool-result messages. Land the field before the client that uses it.

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs:63-98` (the `Message` struct + constructors)
- Test: `crates/zoid-provider/src/lib.rs` (inline `#[cfg(test)] mod tool_call_id_tests`)

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `crates/zoid-provider/src/lib.rs` (after the existing `tool_types_tests` module):

```rust
#[cfg(test)]
mod tool_call_id_tests {
    use super::*;

    #[test]
    fn existing_constructors_default_tool_call_id_to_none() {
        assert_eq!(Message::user("hi").tool_call_id, None);
        assert_eq!(Message::assistant("hi").tool_call_id, None);
        assert_eq!(Message::tool("read_file", "body").tool_call_id, None);
    }

    #[test]
    fn tool_with_call_id_sets_the_field() {
        let m = Message::tool_with_call_id("read_file", "call-42", "body");
        assert_eq!(m.role, MsgRole::Tool);
        assert_eq!(m.content, "body");
        assert_eq!(m.tool_name.as_deref(), Some("read_file"));
        assert_eq!(m.tool_call_id.as_deref(), Some("call-42"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider tool_call_id_tests`
Expected: FAIL — `tool_with_call_id` not found / no `tool_call_id` field on `Message`.

- [ ] **Step 3: Add the field and constructor**

In `crates/zoid-provider/src/lib.rs`, add `tool_call_id: Option<String>` to the `Message` struct (after `tool_name`):

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: MsgRole,
    pub content: String,
    /// Populated only on assistant messages that requested tools.
    pub tool_calls: Vec<ToolCall>,
    /// Populated only on `MsgRole::Tool` messages: the tool whose result this is.
    pub tool_name: Option<String>,
    /// Populated only on `MsgRole::Tool` messages: the originating tool-call id.
    /// OpenAI Chat Completions identifies tool results by `tool_call_id`;
    /// Ollama's native API uses `tool_name` instead (its request-body writer
    /// ignores this field). Anthropic (text-only P1b) also ignores it.
    pub tool_call_id: Option<String>,
}
```

Update the three existing constructors to set the field to `None`:

```rust
impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
            tool_call_id: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
            tool_call_id: None,
        }
    }
    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: Some(name.into()),
            tool_call_id: None,
        }
    }
    /// Like `Message::tool` but with the originating tool-call id. The agent
    /// loop uses this when dispatching a tool result so the OpenAI-compat
    /// request body can emit `tool_call_id`. Existing providers ignore the id.
    pub fn tool_with_call_id(
        name: impl Into<String>,
        call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MsgRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: Some(name.into()),
            tool_call_id: Some(call_id.into()),
        }
    }
}
```

- [ ] **Step 4: Fix every existing `Message { ... }` literal that lacks the new field**

Search for `Message {` struct literals in the workspace and add `tool_call_id: None` to each. The known site is `crates/zoid/src/agent.rs:148-159` (the `ChatMsg::Assistant` arm) and `:162-167` (`ChatMsg::Delegated`). Run:

```bash
rg -n "role: zoid_provider::MsgRole|role: MsgRole" crates/ --type rust
```

For each match that is a `Message { ... }` literal, add `tool_call_id: None,` before the closing brace. In `agent.rs:148-159`:

```rust
ChatMsg::Assistant {
    text, tool_calls, ..
} => Message {
    role: zoid_provider::MsgRole::Assistant,
    content: text,
    tool_calls: tool_calls
        .into_iter()
        .map(|c| ToolCall {
            id: c.id,
            name: c.name,
            args: serde_json::from_str(&c.args).unwrap_or(serde_json::Value::Null),
        })
        .collect(),
    tool_name: None,
    tool_call_id: None,
},
```

And `agent.rs:162-167`:

```rust
ChatMsg::Delegated { summary, .. } => Message {
    role: zoid_provider::MsgRole::Assistant,
    content: format!("[delegated subagent] {summary}"),
    tool_calls: vec![],
    tool_name: None,
    tool_call_id: None,
},
```

- [ ] **Step 5: Run all tests to verify nothing broke**

Run: `cargo test --workspace`
Expected: PASS (all existing tests + the two new `tool_call_id_tests` pass). If any test fails to compile, search for `Message {` literals missing `tool_call_id: None` and add the field.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src/lib.rs crates/zoid/src/agent.rs
git commit -m "feat(provider): add Message.tool_call_id field + tool_with_call_id constructor

Additive: a new Option<String> field for the originating tool-call id, used by
the upcoming OpenAI Chat Completions client to emit \`tool_call_id\` on tool
results. Existing constructors default it to None; Ollama/Anthropic writers
ignore it. Wire the agent loop's ChatMsg::Assistant/Delegated literals to None."
```

---

## Task 2: Populate `tool_call_id` from `ChatMsg::ToolResult.id` in the agent loop

**Why:** `ChatMsg::ToolResult` already carries the originating tool-call `id` (from `EventKind::ToolResult.id`), but `map_msg` currently discards it (`agent.rs:161`). The OpenAI-compat client needs it.

**Files:**
- Modify: `crates/zoid/src/agent.rs:161` (the `ChatMsg::ToolResult` arm of `map_msg`)
- Test: `crates/zoid/src/agent.rs` (inline test module, or extend an existing one)

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/zoid/src/agent.rs` (or a new `tool_call_id_threading_tests` module at the bottom of the file):

```rust
#[cfg(test)]
mod tool_call_id_threading_tests {
    use super::*;
    use zoid_core::eventlog::EventLog;

    /// A minimal build_request smoke: feed one UserMessage + one ToolCall +
    /// one ToolResult into the event log and assert the resulting
    /// CompletionRequest carries tool_call_id on the tool-result Message.
    #[test]
    fn tool_result_message_carries_tool_call_id() {
        // This test depends on how EventLog is constructed in existing tests;
        // mirror the pattern used in agent.rs's existing tests (search for
        // `EventLog::` or `ev(` helpers). The assertion is:
        //   let req = build_request(&events, "m", &[], "sys");
        //   let tool_msg = req.messages.iter().rev().find(|m| m.role == MsgRole::Tool).unwrap();
        //   assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call-7"));
        // Build the EventLog with:
        //   ev(EventKind::UserMessage { text: "go".into() });
        //   ev(EventKind::ToolCall { id: "call-7".into(), name: "read_file".into(), args: "{}".into() });
        //   ev(EventKind::ToolResult { id: "call-7".into(), name: "read_file".into(), output: "ok".into(), is_error: false });
        // (Use whatever EventLog test helper the existing tests use — grep for `fn ev(` in this file.)
        unimplemented!("see comment for the test body — adapt to the existing EventLog test helper")
    }
}
```

Note: the existing test helpers in `agent.rs` use a local `ev()` closure pattern. Grep `crates/zoid/src/agent.rs` for `let mut events = ` or `fn ev(` to find the pattern, then write the test body using that helper. The assertion is the contract: `tool_msg.tool_call_id == Some("call-7")`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid tool_call_id_threading_tests`
Expected: FAIL — `tool_msg.tool_call_id` is `None` (the `id` is discarded today).

- [ ] **Step 3: Wire the id through `map_msg`**

In `crates/zoid/src/agent.rs:161`, change:

```rust
ChatMsg::ToolResult { name, output, .. } => Message::tool(name, output),
```

to:

```rust
ChatMsg::ToolResult { id, name, output, .. } => {
    Message::tool_with_call_id(name, id, output)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid tool_call_id_threading_tests`
Expected: PASS.

- [ ] **Step 5: Run all tests to verify no regressions**

Run: `cargo test --workspace`
Expected: PASS (existing tool-dispatch tests should be unaffected — they don't assert on `tool_call_id`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): thread ChatMsg::ToolResult.id into Message.tool_call_id

The agent loop discarded the tool-call id when mapping ToolResult events to
provider Message\`s; the OpenAI Chat Completions client needs it as
\`tool_call_id\` on tool-result messages. Unconditional — Ollama/Anthropic
request-body writers ignore the field."
```

---

## Task 3: Extract `parse_data_id_models` helper (shared by Anthropic + OpenAI-compat)

**Why:** both `anthropic.rs` (`parse_anthropic_models`) and the upcoming `openai_compat.rs` parse the same `{"data":[{"id":...}]}` shape. Extract once, call from both.

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs` (add the helper)
- Modify: `crates/zoid-provider/src/anthropic.rs` (call the helper)
- Test: `crates/zoid-provider/src/lib.rs` (inline test)

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-provider/src/lib.rs` (a new `parse_data_id_models_tests` module at the bottom):

```rust
#[cfg(test)]
mod parse_data_id_models_tests {
    use super::parse_data_id_models;

    #[test]
    fn parses_data_id_array() {
        let body = r#"{"data":[{"id":"glm-5.2"},{"id":"kimi-k2.6"}]}"#;
        assert_eq!(parse_data_id_models(body), vec!["glm-5.2", "kimi-k2.6"]);
    }

    #[test]
    fn empty_or_bad_body_is_empty() {
        assert!(parse_data_id_models("{}").is_empty());
        assert!(parse_data_id_models("not json").is_empty());
        assert!(parse_data_id_models(r#"{"data":[]}"#).is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider parse_data_id_models_tests`
Expected: FAIL — `parse_data_id_models` not found.

- [ ] **Step 3: Add the helper + rewire `anthropic.rs`**

In `crates/zoid-provider/src/lib.rs`, add near the other free functions (e.g. after `is_context_length_error`):

```rust
/// Parse a `{"data":[{"id":...}]}` model-list response body (the shape used by
/// both the Anthropic `/v1/models` and OpenAI-compat `/v1/models` endpoints).
/// Lenient: unknown/!json → empty.
pub fn parse_data_id_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
```

In `crates/zoid-provider/src/anthropic.rs`, replace the local `parse_anthropic_models` body (at `anthropic.rs:137-147`) with a delegation, keeping the function name as a thin re-export so existing call sites keep resolving:

```rust
/// Extract model ids from an Anthropic `/v1/models` response body. Lenient.
pub fn parse_anthropic_models(body: &str) -> Vec<String> {
    crate::parse_data_id_models(body)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (new `parse_data_id_models_tests` + existing `anthropic.rs` tests still pass — same behavior, shared implementation).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/lib.rs crates/zoid-provider/src/anthropic.rs
git commit -m "refactor(provider): extract parse_data_id_models shared by anthropic + openai-compat

The {\"data\":[{\"id\":...}]} shape is used by both /v1/models endpoints.
anthropic.rs's parse_anthropic_models becomes a thin re-export."
```

---

## Task 4: `openai_compat.rs` — request body + pure parse functions

**Why:** the pure logic first (testable without a server), then the provider struct + streaming in Task 5.

**Files:**
- Create: `crates/zoid-provider/src/openai_compat.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod openai_compat;`)
- Test: `crates/zoid-provider/src/openai_compat.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Register the module**

In `crates/zoid-provider/src/lib.rs`, add alongside the existing `pub mod anthropic; pub mod ollama;`:

```rust
pub mod openai_compat;
pub mod opencode_go;  // (added in Task 6; add the line now if you prefer, but the file must exist before \`cargo build\` — add it in Task 6 instead to keep this task self-contained)
```

Actually, to keep this task self-contained, add **only** `pub mod openai_compat;` here:

```rust
pub mod anthropic;
pub mod ollama;
pub mod openai_compat;
```

- [ ] **Step 2: Write the failing tests for `request_body`**

Create `crates/zoid-provider/src/openai_compat.rs` with just the test module first (the imports will fail to resolve until Step 3, which is expected):

```rust
//! The generic OpenAI Chat Completions client (POST {base}/v1/chat/completions,
//! SSE streaming, tool-calling with fragment accumulation). Self-contained
//! like the `anthropic`/`ollama` modules; uses the crate's `Provider` seam.
//! No opencode-go-specifics — a generic leaf reusable by any OpenAI-compat
//! provider (Go, Zen, OpenRouter, etc.).

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
    fn body_has_stream_options_and_system_leading_message() {
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(body["model"], "glm-5.2");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
            ])
        );
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        assert_eq!(
            request_body(&req)["messages"],
            json!([{ "role": "user", "content": "x" }])
        );
    }

    #[test]
    fn assistant_with_tool_calls_emits_arguments_as_json_string() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message {
                role: MsgRole::Assistant,
                content: "".into(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    args: json!({"path": "a.txt"}),
                }],
                tool_name: None,
                tool_call_id: None,
            }],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        // arguments MUST be a JSON-encoded string, not an object
        let tc = &body["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "call-1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        assert_eq!(tc["function"]["arguments"], json!(r#"{"path":"a.txt"}"#));
        assert!(tc["function"]["arguments"].is_string(), "arguments must be a string");
    }

    #[test]
    fn tool_message_emits_tool_call_id() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::tool_with_call_id("read_file", "call-1", "body")],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "tool", "tool_call_id": "call-1", "content": "body" })
        );
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
        };
        let body = request_body(&req);
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": { "name": "read_file", "description": "read a file", "parameters": {"type": "object"} }
            }])
        );
    }

    #[test]
    fn body_without_tools_omits_tools_key() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        assert!(request_body(&req).get("tools").is_none());
    }
}
```

- [ ] **Step 3: Implement `request_body` to make the tests pass**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/openai_compat.rs`:

```rust
/// Build the OpenAI Chat Completions `/v1/chat/completions` request body.
/// System prompt is a leading `{"role":"system"}` message. Tool-call
/// `arguments` are serialized as a JSON-encoded **string** (OpenAI's shape,
/// the inverse of Ollama's object shape). Tool results carry `tool_call_id`.
pub fn request_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    for m in &req.messages {
        match m.role {
            MsgRole::User => messages.push(json!({ "role": "user", "content": m.content })),
            MsgRole::Assistant => {
                let mut obj = json!({ "role": "assistant", "content": m.content });
                if !m.tool_calls.is_empty() {
                    obj["tool_calls"] = Value::Array(
                        m.tool_calls.iter().map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".into()),
                                }
                            })
                        }).collect(),
                    );
                }
                messages.push(obj);
            }
            MsgRole::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": m.content,
            })),
        }
    }
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools.iter().map(|t| json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
            })).collect(),
        );
    }
    body
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_compat::tests`
Expected: PASS (all 6 `request_body` tests).

- [ ] **Step 5: Write the failing tests for `parse_chunk` + the tool-call accumulator**

Append to the `tests` module in `openai_compat.rs`:

```rust
    #[test]
    fn parse_chunk_content_delta_yields_textdelta() {
        let data = r#"{"choices":[{"delta":{"content":"Hel"}}]}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn parse_chunk_empty_content_yields_nothing() {
        let data = r#"{"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_chunk(data, &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn parse_chunk_finish_reason_length_yields_truncated() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Truncated]
        );
    }

    #[test]
    fn parse_chunk_finish_reason_stop_yields_nothing() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        assert!(parse_chunk(data, &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn parse_chunk_usage_yields_usage_with_cached_tokens() {
        let data = r#"{"usage":{"prompt_tokens":120,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":30}}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Usage(Usage { input_tokens: 120, output_tokens: 40, cached: 30 })]
        );
    }

    #[test]
    fn parse_chunk_usage_without_cached_tokens_defaults_to_zero() {
        let data = r#"{"usage":{"prompt_tokens":120,"completion_tokens":40}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Usage(Usage { input_tokens: 120, output_tokens: 40, cached: 0 })]
        );
    }

    #[test]
    fn parse_chunk_error_yields_error() {
        let data = r#"{"error":{"message":"Unauthorized"}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Error("Unauthorized".into())]
        );
    }

    #[test]
    fn parse_chunk_malformed_yields_nothing() {
        assert!(parse_chunk("not json", &mut ToolCallAccumulator::new()).is_empty());
        assert!(parse_chunk("", &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn tool_call_accumulator_single_chunk_flushes_at_take() {
        let mut acc = ToolCallAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}}]}"#;
        let _ = parse_chunk(data, &mut acc);
        assert_eq!(
            acc.take(),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn tool_call_accumulator_two_chunks_concatenates_arguments() {
        let mut acc = ToolCallAccumulator::new();
        // chunk 1: id + name + first half of arguments
        let c1 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#;
        // chunk 2: second half of arguments (a JSON fragment)
        let c2 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]}}]}"#;
        let _ = parse_chunk(c1, &mut acc);
        let _ = parse_chunk(c2, &mut acc);
        assert_eq!(
            acc.take(),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn tool_call_accumulator_two_distinct_calls_in_index_order() {
        let mut acc = ToolCallAccumulator::new();
        let c1 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","function":{"name":"read_file","arguments":"{}"}}]}}]}"#;
        let c2 = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call-b","function":{"name":"list_dir","arguments":"{}"}}]}}]}"#;
        let _ = parse_chunk(c1, &mut acc);
        let _ = parse_chunk(c2, &mut acc);
        let out = acc.take();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ProviderEvent::ToolCall(ToolCall { id: "call-a".into(), name: "read_file".into(), args: json!({}) }));
        assert_eq!(out[1], ProviderEvent::ToolCall(ToolCall { id: "call-b".into(), name: "list_dir".into(), args: json!({}) }));
    }
```

- [ ] **Step 6: Implement `ToolCallAccumulator` + `parse_chunk`**

Add above the `#[cfg(test)]` block:

```rust
/// Accumulates OpenAI tool-call fragments (which arrive piecewise across SSE
/// chunks) by `index`, flushing complete `ToolCall`s when `take()` is called
/// (at `data: [DONE]` or stream end). id + name lock on first sighting;
/// arguments strings concatenate across chunks and are re-parsed to an object.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    by_index: std::collections::BTreeMap<u32, ToolCallAccum>,
}

#[derive(Debug, Clone)]
struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn new() -> Self { Self::default() }

    /// Feed one chunk's `delta.tool_calls[]` entry.
    fn feed(&mut self, call: &Value) {
        let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
        let entry = self.by_index.entry(index).or_insert_with(|| ToolCallAccum {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        });
        if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
            entry.id = id.to_string();
        }
        if let Some(func) = call.get("function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                entry.name = name.to_string();
            }
            if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                entry.arguments.push_str(args);
            }
        }
    }

    /// Drain the accumulated tool calls as `ToolCall` events, in index order.
    /// Each `arguments` JSON string is re-parsed to a `Value::Object` (matching
    /// `coerce_tool_args`'s contract in `ollama.rs`); invalid JSON → `{}`
    pub fn take(&mut self) -> Vec<ProviderEvent> {
        self.by_index
            .iter()
            .map(|(_, a)| ProviderEvent::ToolCall(ToolCall {
                id: a.id.clone(),
                name: a.name.clone(),
                args: serde_json::from_str(&a.arguments)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({})),
            }))
            .collect()
    }
}

/// Parse one OpenAI Chat Completions SSE `data:` payload (the JSON object after
/// `data: `) into zero-or-more `ProviderEvent`s. `acc` accumulates tool-call
/// fragments across calls. Never panics. The caller handles `data: [DONE]`
/// separately (flushing `acc.take()` then emitting `Done`).
pub fn parse_chunk(data: &str, acc: &mut ToolCallAccumulator) -> Vec<ProviderEvent> {
    let data = data.trim();
    if data.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(err) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return vec![ProviderEvent::Error(err.to_string())];
    }
    let mut out = Vec::new();
    if let Some(content) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")).and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
        if !content.is_empty() {
            out.push(ProviderEvent::TextDelta(content.to_string()));
        }
    }
    if let Some(calls) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")).and_then(|d| d.get("tool_calls")).and_then(|t| t.as_array()) {
        for call in calls {
            acc.feed(call);
        }
    }
    if let Some(reason) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("finish_reason")).and_then(|f| f.as_str()) {
        if reason == "length" {
            out.push(ProviderEvent::Truncated);
        }
    }
    if let Some(usage) = v.get("usage") {
        let input = usage.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        let output = usage.get("completion_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        let cached = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        out.push(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: output, cached }));
    }
    out
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_compat`
Expected: PASS (all `request_body` + `parse_chunk` + accumulator tests).

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): add openai_compat request_body + parse_chunk + tool-call accumulator

Pure logic for the OpenAI Chat Completions wire shape. request_body emits
arguments as a JSON-encoded string and tool results with tool_call_id.
parse_chunk handles content deltas, finish_reason, usage (with cached_tokens),
and error objects. ToolCallAccumulator concatenates piecewise tool-call
fragments by index and re-parses arguments to an object at flush."
```

---

## Task 5: `openai_compat.rs` — `OpenAICompatProvider` struct + streaming + `list_models`

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs` (add the provider struct + `Provider` impl + stalling-server tests)

- [ ] **Step 1: Add the provider struct + builders**

Append to `crates/zoid-provider/src/openai_compat.rs` (above the `#[cfg(test)]` block):

```rust
/// Default base URL when none is configured. Callers override via
/// `with_base_url`; the OpenAI-compat leaf has no single canonical host
/// (OpenAI, OpenRouter, OpenCode Go/Zen all differ), so this is a placeholder
/// that real callers always override.
const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Streaming OpenAI Chat Completions provider.
pub struct OpenAICompatProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenAICompatProvider {
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
```

- [ ] **Step 2: Implement `Provider::stream` + `list_models`**

Append the `Provider` impl:

```rust
#[async_trait]
impl Provider for OpenAICompatProvider {
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;
        let mut acc = ToolCallAccumulator::new();

        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/v1/chat/completions", self.base_url))
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&request_body(req))
                .send(),
        ).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                return Ok(());
            }
            Err(_) => {
                let _ = sink.send(ProviderEvent::Error(format!(
                    "provider request timed out after {}s (no response)",
                    self.idle_timeout.as_secs()
                ))).await;
                return Ok(());
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = match tokio::time::timeout(self.idle_timeout, resp.text()).await {
                Ok(Ok(t)) => t,
                _ => String::new(),
            };
            let _ = sink.send(ProviderEvent::Error(format!("HTTP {status}: {text}"))).await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        let mut ended_early = false;
        loop {
            let item = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    let _ = sink.send(ProviderEvent::Error(format!(
                        "provider idle timeout: no data for {}s",
                        self.idle_timeout.as_secs()
                    ))).await;
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
            // OpenAI uses `data: [DONE]` as the terminator; eventsource may
            // surface it as an event with data "[DONE]" (no event type).
            if item.data == "[DONE]" {
                // flush accumulated tool calls, then Done
                for tc in acc.take() {
                    if ttft.is_none() { ttft = Some(start.elapsed().as_millis() as u64); }
                    if sink.send(tc).await.is_err() { ended_early = true; break; }
                }
                if !ended_early {
                    let _ = sink.send(ProviderEvent::Done).await;
                }
                break;
            }
            for pe in parse_chunk(&item.data, &mut acc) {
                if ttft.is_none() { ttft = Some(start.elapsed().as_millis() as u64); }
                let is_done = matches!(pe, ProviderEvent::Done);
                if sink.send(pe).await.is_err() { ended_early = true; break; }
                if is_done { break; }
            }
            if ended_early { break; }
        }
        // If the transport closed without an explicit [DONE], flush + Done
        // (matches the ollama.rs trailing-line flush philosophy).
        if !ended_early {
            for tc in acc.take() {
                if ttft.is_none() { ttft = Some(start.elapsed().as_millis() as u64); }
                if sink.send(tc).await.is_err() { break; }
            }
            let _ = sink.send(ProviderEvent::Done).await;
        }
        tracing::info!(
            kind = "provider",
            provider = "openai-compat",
            model = %req.model,
            ttft_ms = ttft.unwrap_or(0),
            total_ms = start.elapsed().as_millis() as u64,
            "provider stream complete"
        );
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

- [ ] **Step 3: Write the stalling-server tests**

Append to the `tests` module in `openai_compat.rs`:

```rust
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Throwaway server that accepts one connection, optionally writes
    /// `headers`, then stalls. Mirrors ollama.rs:773-789.
    async fn spawn_stalling_server(headers: Option<&'static [u8]>) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                if let Some(hdr) = headers {
                    let _ = sock.write_all(hdr).await;
                    let _ = sock.flush().await;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        addr
    }

    const OK_SSE_HEADERS: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
        }
    }

    #[tokio::test]
    async fn idle_timeout_emits_error_when_stream_stalls() {
        let addr = spawn_stalling_server(Some(OK_SSE_HEADERS)).await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done = tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung — idle timeout not enforced");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await { got.push(ev); }
        assert!(matches!(got.last(), Some(ProviderEvent::Error(_))), "expected trailing idle-timeout Error, got {got:?}");
    }

    #[tokio::test]
    async fn request_timeout_emits_error_when_no_response() {
        let addr = spawn_stalling_server(None).await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done = tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung waiting for response headers");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await { got.push(ev); }
        assert!(matches!(got.last(), Some(ProviderEvent::Error(_))), "expected a request-timeout Error, got {got:?}");
    }

    #[tokio::test]
    async fn error_body_timeout_emits_error_with_status() {
        let addr = spawn_stalling_server(Some(
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 100\r\n\r\n",
        )).await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done = tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung reading a stalled error body");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await { got.push(ev); }
        assert!(matches!(got.last(), Some(ProviderEvent::Error(e)) if e.contains("429")), "expected an HTTP 429 Error, got {got:?}");
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_compat`
Expected: PASS (the three stalling-server tests confirm timeout enforcement; the happy-path SSE test is a manual smoke per the spec's "no live-endpoint CI" stance — but if you want a happy-path unit test, add a `TcpListener` that writes a couple of `data: {...}\n\ndata: [DONE]\n\n` chunks and assert ordered events. Add it here if desired; not strictly required by the spec.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs
git commit -m "feat(provider): add OpenAICompatProvider struct + streaming + list_models

Streaming OpenAI Chat Completions provider with the three-tier timeout
pattern (initial send, between SSE events, error-body read) matching
anthropic.rs/ollama.rs. Parses data: [DONE] as the terminator, flushing
accumulated tool calls before Done. list_models hits /v1/models via the
shared parse_data_id_models helper."
```

---

## Task 6: `opencode_go.rs` — wire-shape map + `OpenCodeGoProvider` delegation

**Files:**
- Create: `crates/zoid-provider/src/opencode_go.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod opencode_go;`)
- Test: `crates/zoid-provider/src/opencode_go.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Register the module**

In `crates/zoid-provider/src/lib.rs`, add:

```rust
pub mod anthropic;
pub mod ollama;
pub mod openai_compat;
pub mod opencode_go;
```

- [ ] **Step 2: Write the failing tests for the wire-shape map + routing**

Create `crates/zoid-provider/src/opencode_go.rs` with the test module:

```rust
//! The dedicated OpenCode Go provider: holds a static per-model wire-shape map
//! and delegates `stream()`/`list_models()` to either `OpenAICompatProvider`
//! (POST {base}/v1/chat/completions, 8 models) or `AnthropicProvider`
//! (POST {base}/v1/messages, 5 models) based on the active model id.

use crate::{CompletionRequest, Provider, ProviderEvent};
use crate::openai_compat::OpenAICompatProvider;
use crate::anthropic::AnthropicProvider;
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireShape { OpenAICompat, Anthropic }

const GO_MODELS: &[(&str, WireShape)] = &[
    ("glm-5.2",           WireShape::OpenAICompat),
    ("glm-5.1",           WireShape::OpenAICompat),
    ("kimi-k2.7-code",    WireShape::OpenAICompat),
    ("kimi-k2.6",         WireShape::OpenAICompat),
    ("deepseek-v4-pro",   WireShape::OpenAICompat),
    ("deepseek-v4-flash", WireShape::OpenAICompat),
    ("mimo-v2.5",         WireShape::OpenAICompat),
    ("mimo-v2.5-pro",     WireShape::OpenAICompat),
    ("minimax-m3",        WireShape::Anthropic),
    ("minimax-m2.7",      WireShape::Anthropic),
    ("minimax-m2.5",      WireShape::Anthropic),
    ("qwen3.7-max",       WireShape::Anthropic),
    ("qwen3.7-plus",      WireShape::Anthropic),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_for_known_models_matches_table() {
        for (id, shape) in GO_MODELS {
            let p = OpenCodeGoProvider::new("k".into());
            assert_eq!(p.wire_shape_for(id), *shape, "mismatch for {id}");
        }
    }

    #[test]
    fn wire_shape_for_unknown_defaults_to_openai_compat() {
        let p = OpenCodeGoProvider::new("k".into());
        assert_eq!(p.wire_shape_for("unknown-model"), WireShape::OpenAICompat);
    }
}
```

- [ ] **Step 3: Implement the provider struct + `wire_shape_for`**

Add above the `#[cfg(test)]` block:

```rust
pub struct OpenCodeGoProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeGoProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("opencode-go")
                .unwrap_or("https://opencode.ai/zen/go")
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
    fn wire_shape_for(&self, model: &str) -> WireShape {
        match GO_MODELS.iter().find(|(id, _)| *id == model) {
            Some((_, shape)) => *shape,
            None => {
                tracing::warn!(model = %model, "opencode-go: model not in wire-shape map; defaulting to OpenAICompat");
                WireShape::OpenAICompat
            }
        }
    }
}
```

- [ ] **Step 4: Run the map tests to verify they pass**

Run: `cargo test -p zoid-provider opencode_go::tests`
Expected: PASS (13 map entries + the unknown default).

- [ ] **Step 5: Implement `Provider::stream` + `list_models` delegation**

Add the `Provider` impl above the `#[cfg(test)]` block:

```rust
#[async_trait]
impl Provider for OpenCodeGoProvider {
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        match self.wire_shape_for(&req.model) {
            WireShape::OpenAICompat => {
                OpenAICompatProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink).await
            }
            WireShape::Anthropic => {
                AnthropicProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink).await
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Both sub-clients' /v1/models share the {data:[{id}]} shape; reuse
        // the OpenAI-compat client's list_models (it hits {base}/v1/models).
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_idle_timeout(self.idle_timeout)
            .list_models().await
    }
}
```

- [ ] **Step 6: Write the routing integration test (TcpListener records the path)**

Append to the `tests` module:

```rust
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Server that records the request line of the first request, then writes
    /// a minimal SSE `data: [DONE]` so the stream terminates cleanly.
    async fn spawn_recording_server() -> (std::net::SocketAddr, std::sync::Arc<tokio::sync::Mutex<Option<String>>>) {
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
                // respond with a minimal SSE stream ending in [DONE]
                let body = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n19\r\ndata: [DONE]\r\n\r\n0\r\n\r\n";
                let _ = sock.write_all(body).await;
            }
        });
        (addr, recorded)
    }

    #[tokio::test]
    async fn openai_compat_model_routes_to_chat_completions() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeGoProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![crate::Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/chat/completions"), "expected /v1/chat/completions, got: {first}");
    }

    #[tokio::test]
    async fn anthropic_model_routes_to_messages() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeGoProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let req = CompletionRequest {
            model: "minimax-m3".into(),
            system: None,
            messages: vec![crate::Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(first.contains("/v1/messages"), "expected /v1/messages, got: {first}");
    }

    #[test]
    fn with_base_url_propagates_to_subclient() {
        let p = OpenCodeGoProvider::new("k".into()).with_base_url("https://example.test/go/");
        assert_eq!(p.base_url, "https://example.test/go");
    }
```

- [ ] **Step 7: Run all opencode_go tests to verify they pass**

Run: `cargo test -p zoid-provider opencode_go`
Expected: PASS (map tests + routing tests + base_url test). If the routing test flakes on the SSE response format, adjust the raw response bytes — the contract is that the recorded request line contains the right path; the stream just needs to terminate cleanly enough for `stream()` to return.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-provider/src/opencode_go.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): add OpenCodeGoProvider with per-model wire routing

Dedicated provider for OpenCode Go: a static GO_MODELS map routes each of
the 13 model ids to OpenAICompatProvider (/v1/chat/completions, 8 models)
or AnthropicProvider (/v1/messages, 5 models). Unknown models default to
OpenAICompat with a tracing::warn. Fresh sub-client per stream() call;
with_base_url propagates to the sub-client."
```

---

## Task 7: `zoid-model` — registry entry + `MODEL_CAPS` + `deepseek-v4-pro` correction + test updates

**Files:**
- Modify: `crates/zoid-model/src/lib.rs:54-103` (PROVIDERS), `:107-154` (MODEL_CAPS), `:224-225` and `:240` (existing tests)
- Test: inline `#[cfg(test)] mod tests` updates + new table-driven test

- [ ] **Step 1: Write the failing tests first (registry + caps assertions)**

Add a new test module at the bottom of `crates/zoid-model/src/lib.rs` (before the closing of the file, after the existing `tests` module):

```rust
#[cfg(test)]
mod opencode_go_tests {
    use super::*;

    #[test]
    fn opencode_go_registry_entry_exists_and_is_selectable() {
        let e = entry("opencode-go").expect("opencode-go entry must exist");
        assert_eq!(e.id, "opencode-go");
        assert_eq!(e.family, "opencode-go");
        assert_eq!(e.status, Status::Available);
        assert_eq!(
            e.transport,
            Transport::Http { default_base_url: "https://opencode.ai/zen/go" }
        );
        assert_eq!(e.models.len(), 13);
        assert_eq!(e.models[0], "glm-5.2"); // default model
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"opencode-go"));
    }

    #[test]
    fn canonical_id_opencode_go_is_passthrough() {
        assert_eq!(canonical_id("opencode-go"), "opencode-go");
    }

    /// Table-driven caps assertion: every Go model id has the reconciled caps.
    #[test]
    fn opencode_go_model_caps_match_reconciled_table() {
        let cases: &[(&str, u64, u64, bool, bool)] = &[
            // (id, context_window, max_output, tools, prompt_cache)
            ("glm-5.2",           1_000_000, 0,       true,  true),
            ("glm-5.1",           1_000_000, 0,       true,  true),
            ("kimi-k2.7-code",    262_144,   0,       true,  true),
            ("kimi-k2.6",         262_144,   0,       true,  true),
            ("deepseek-v4-pro",   1_000_000, 384_000, true,  true),
            ("deepseek-v4-flash", 1_000_000, 384_000, true,  true),
            ("mimo-v2.5",         128_000,   0,       true,  true),
            ("mimo-v2.5-pro",     128_000,   0,       true,  true),
            ("minimax-m3",        200_000,   0,       false, true),
            ("minimax-m2.7",      200_000,   0,       false, true),
            ("minimax-m2.5",      200_000,   0,       false, true),
            ("qwen3.7-max",       256_000,   0,       false, true),
            ("qwen3.7-plus",      256_000,   0,       false, true),
        ];
        for (id, ctx, max_out, tools, pc) in cases {
            let info = model_info(id);
            assert_eq!(info.context_window, *ctx, "ctx mismatch for {id}");
            assert_eq!(info.max_output, *max_out, "max_output mismatch for {id}");
            assert_eq!(info.tools, *tools, "tools mismatch for {id}");
            assert_eq!(info.prompt_cache, *pc, "prompt_cache mismatch for {id}");
        }
    }

    /// Regression lock for the deepseek-v4-pro correction.
    #[test]
    fn deepseek_v4_pro_correction_locked() {
        let info = model_info("deepseek-v4-pro");
        assert_eq!(info.context_window, 1_000_000, "was 128_000; do not revert");
        assert_eq!(info.max_output, 384_000);
        assert!(info.prompt_cache, "was false; do not revert");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-model opencode_go_tests`
Expected: FAIL — `entry("opencode-go")` returns `None`; `model_info` for the 13 ids falls back to the conservative default (32k).

- [ ] **Step 3: Add the registry entry**

In `crates/zoid-model/src/lib.rs`, insert the new `ProviderEntry` into `PROVIDERS` after `ollama-cloud` (so it appears in the picker between ollama and anthropic) and before `anthropic-api`:

```rust
    ProviderEntry {
        id: "opencode-go",
        display: "opencode · go",
        family: "opencode-go",
        transport: Transport::Http {
            default_base_url: "https://opencode.ai/zen/go",
        },
        models: &[
            "glm-5.2", "glm-5.1",
            "kimi-k2.7-code", "kimi-k2.6",
            "deepseek-v4-pro", "deepseek-v4-flash",
            "mimo-v2.5", "mimo-v2.5-pro",
            "minimax-m3", "minimax-m2.7", "minimax-m2.5",
            "qwen3.7-max", "qwen3.7-plus",
        ],
        status: Status::Available,
    },
```

- [ ] **Step 4: Correct the existing `deepseek-v4-pro` MODEL_CAPS entry**

Find the existing `deepseek-v4-pro` entry in `MODEL_CAPS` (around `lib.rs:146-153`) and change it:

```rust
    // deepseek-v4-pro: corrected via api-docs.deepseek.com (was 128_000/0/false).
    // DeepSeek's own docs confirm a 1M context window and 384K max output, plus
    // a Context Caching feature (cached read at $0.0028 vs $0.14 uncached).
    (
        "deepseek-v4-pro",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 384_000,
            tools: true,
            prompt_cache: true,
        },
    ),
```

- [ ] **Step 5: Add the 12 new MODEL_CAPS entries**

Append to `MODEL_CAPS` (before the closing `];`):

```rust
    // --- OpenCode Go models (12 new entries; deepseek-v4-pro corrected above) ---
    // glm-5.2: reconciled with existing glm-5.2:cloud (same model, 1M window).
    ("glm-5.2", ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: true }),
    // glm-5.1: inferred from glm-5.2:cloud sibling (same GLM-5.x family, 1M window).
    ("glm-5.1", ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: true }),
    // Kimi: confirmed via platform.kimi.ai (262,144-token window).
    ("kimi-k2.7-code", ModelInfo { context_window: 262_144, max_output: 0, tools: true, prompt_cache: true }),
    ("kimi-k2.6", ModelInfo { context_window: 262_144, max_output: 0, tools: true, prompt_cache: true }),
    // deepseek-v4-flash: confirmed via api-docs.deepseek.com (1M window, 384K max output).
    ("deepseek-v4-flash", ModelInfo { context_window: 1_000_000, max_output: 384_000, tools: true, prompt_cache: true }),
    // MiMo: unconfirmed — approx from public claims; override via ZOID_CONTEXT_CEILING.
    ("mimo-v2.5", ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: true }),
    ("mimo-v2.5-pro", ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: true }),
    // Anthropic-shape Go models: tools=false on day one (existing AnthropicProvider
    // is text-only P1b; a zoid-implementation limitation, not a model limitation —
    // flips to true when P1b.1 Anthropic tool_use/tool_result mapping lands).
    // prompt_cache=true per Go's advertised cached-read pricing for all 13 models.
    // Windows unconfirmed — approx from public claims; override via ZOID_CONTEXT_CEILING.
    ("minimax-m3", ModelInfo { context_window: 200_000, max_output: 0, tools: false, prompt_cache: true }),
    ("minimax-m2.7", ModelInfo { context_window: 200_000, max_output: 0, tools: false, prompt_cache: true }),
    ("minimax-m2.5", ModelInfo { context_window: 200_000, max_output: 0, tools: false, prompt_cache: true }),
    ("qwen3.7-max", ModelInfo { context_window: 256_000, max_output: 0, tools: false, prompt_cache: true }),
    ("qwen3.7-plus", ModelInfo { context_window: 256_000, max_output: 0, tools: false, prompt_cache: true }),
```

- [ ] **Step 6: Update the two existing tests that assert the old wrong `deepseek-v4-pro` values**

At `lib.rs:224-225` (in `model_info_exact_lookup`):

```rust
        assert_eq!(model_info("deepseek-v4-pro").context_window, 1_000_000);
        assert_eq!(model_info("deepseek-v4-pro").max_output, 384_000);
        assert!(model_info("deepseek-v4-pro").prompt_cache);
```

At `lib.rs:240` (in `model_info_case_insensitive`):

```rust
        assert_eq!(model_info("DEEPSEEK-V4-PRO").context_window, 1_000_000);
```

- [ ] **Step 7: Run all zoid-model tests to verify they pass**

Run: `cargo test -p zoid-model`
Expected: PASS (new `opencode_go_tests` + the two updated existing tests + all other existing tests).

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): add opencode-go registry entry + 12 MODEL_CAPS + deepseek-v4-pro correction

- opencode-go ProviderEntry (family opencode-go, 13 models, glm-5.2 default)
- 12 new MODEL_CAPS entries for the Go models (tools=true for the 8
  OpenAI-compat models, false for the 5 Anthropic-shape models per the
  text-only P1b AnthropicProvider; prompt_cache=true for all 13 per Go's
  advertised cached-read pricing)
- correct the existing deepseek-v4-pro entry: 128k -> 1M, max_output
  384K, prompt_cache true (confirmed via api-docs.deepseek.com)
- update the two existing tests that asserted the old wrong values"
```

---

## Task 8: Bin wiring — `key_env_for` + `select_provider` + `provider_for_id` + `key_status`

**Files:**
- Modify: `crates/zoid/src/main.rs:571-579` (`key_env_for`), `:587-638` (`select_provider`), `:685-719` (`provider_for_id`), `:2013-2016` (`key_status`)

- [ ] **Step 1: Write the failing tests first**

Find the existing `key_env_for` tests around `main.rs:4156-4165` and add to that test module (or create a new one):

```rust
    #[test]
    fn key_env_for_opencode_go_is_opencode_go_api_key() {
        assert_eq!(key_env_for("opencode-go"), Some("OPENCODE_GO_API_KEY"));
    }

    #[test]
    fn entry_requires_key_opencode_go_is_true() {
        assert!(entry_requires_key("opencode-go"));
    }

    #[test]
    fn key_env_for_regressions_still_hold() {
        assert_eq!(key_env_for("ollama-local"), None);
        assert_eq!(key_env_for("anthropic-api"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(key_env_for("ollama-cloud"), Some("OLLAMA_API_KEY"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid key_env_for_opencode_go`
Expected: FAIL — `key_env_for("opencode-go")` falls through to the `_ => Some("OLLAMA_API_KEY")` arm.

- [ ] **Step 3: Add the `opencode-go` arm to `key_env_for`**

In `crates/zoid/src/main.rs:571-579`, change:

```rust
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}
```

to:

```rust
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("opencode-go") => Some("OPENCODE_GO_API_KEY"),
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid key_env_for`
Expected: PASS (the three new + existing tests).

- [ ] **Step 5: Add the `opencode-go` arm to `select_provider`**

In `crates/zoid/src/main.rs:618-637`, the `match family { ... }` block gains a new arm before `"anthropic"`:

```rust
    match family {
        "opencode-go" => match key_for("OPENCODE_GO_API_KEY") {
            Some(k) => (
                Arc::new(zoid_provider::opencode_go::OpenCodeGoProvider::new(k)
                    .with_base_url(base_url)),
                "opencode-go",
                true,
            ),
            None => (default_provider(), "opencode-go", false),
        },
        "anthropic" => match key_for("ANTHROPIC_API_KEY") {
            /* unchanged */
        },
        _ => match key_for("OLLAMA_API_KEY") {
            /* unchanged */
        },
    }
```

- [ ] **Step 6: Add the `opencode-go` arm to `provider_for_id`**

In `crates/zoid/src/main.rs:712-719`, the `match family { ... }` block gains a new arm before `"anthropic"`:

```rust
    match family {
        "opencode-go" => key_for("OPENCODE_GO_API_KEY").map(|k| {
            Arc::new(zoid_provider::opencode_go::OpenCodeGoProvider::new(k)
                .with_base_url(base_url))
                as Arc<dyn Provider>
        }),
        "anthropic" => key_for("ANTHROPIC_API_KEY").map(|k| {
            Arc::new(zoid_provider::anthropic::AnthropicProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
        _ => key_for("OLLAMA_API_KEY").map(|k| {
            Arc::new(zoid_provider::ollama::OllamaProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
    }
```

- [ ] **Step 7: Add `OPENCODE_GO_API_KEY` to the `key_status` array**

In `crates/zoid/src/main.rs:2013-2016`, change:

```rust
    let key_status = [
        ("OLLAMA_API_KEY", status("OLLAMA_API_KEY")),
        ("ANTHROPIC_API_KEY", status("ANTHROPIC_API_KEY")),
    ];
```

to:

```rust
    let key_status = [
        ("OLLAMA_API_KEY", status("OLLAMA_API_KEY")),
        ("ANTHROPIC_API_KEY", status("ANTHROPIC_API_KEY")),
        ("OPENCODE_GO_API_KEY", status("OPENCODE_GO_API_KEY")),
    ];
```

- [ ] **Step 8: Run the whole workspace to verify everything compiles + tests pass**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 9: Run lint + fmt**

Run: `cargo fmt --check && cargo clippy --workspace -- -D warnings`
Expected: PASS. Fix any warnings the new code introduces (unused imports, etc.).

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): wire opencode-go into key_env_for/select_provider/provider_for_id

- key_env_for: opencode-go family -> OPENCODE_GO_API_KEY
- select_provider: opencode-go arm constructs OpenCodeGoProvider with the
  configured base_url; falls back to default_provider() if no key
- provider_for_id: same arm for quick-switch live model fetch
- key_status array: add OPENCODE_GO_API_KEY so the settings UI shows its
  secret status alongside OLLAMA/ANTHROPIC"
```

---

## Task 9: Final verification + smoke checklist

**Files:** none (verification only)

- [ ] **Step 1: Full workspace build + test + lint**

Run:
```bash
cargo test --workspace
cargo fmt --check
cargo clippy --workspace -- -D warnings
```
Expected: all PASS.

- [ ] **Step 2: Confirm the picker shows the 13 Go models**

Run the binary, open settings, focus `provider`, select `opencode-go`, then focus `model` — confirm the 13 ids appear (the live `/v1/models` fetch may add more if Go ships new models; the static registry 13 is the fallback).

- [ ] **Step 3: Manual smoke against the live endpoint (documented, not CI)**

Set up:
```bash
export OPENCODE_GO_API_KEY="<your key>"
```
Add to your zoid config (or use the settings UI):
```toml
provider = "opencode-go"
model = "glm-5.2"
```
Run a turn. Assert:
- Streaming text arrives.
- Tool-calling works (e.g. ask the agent to read a file; `invoke_skill` or `read_file` should fire).

Then switch `model = "qwen3.7-max"` (an Anthropic-shape model) and run a turn. Assert:
- Streaming text arrives.
- Tools are **not** advertised (the agent's system prompt won't list tools the provider can't call — honest `tools: false`).

- [ ] **Step 4: Manual smoke — cache sparkline**

While running a Go model with cached-read pricing (e.g. `glm-5.2`), open the context drawer and check the cache sparkline. If it renders non-"n/a" and shows cache hits, the optimistic `prompt_cache: true` stance is validated. If it shows a misleading 0 (the field is absent/always-zero on the wire), flip `prompt_cache` to `false` for the affected models in `MODEL_CAPS` per the spec §4.4 caveat.

- [ ] **Step 5: Final commit (if any smoke-driven cap adjustments were made)**

If Step 4 found a `prompt_cache` issue, commit the fix:
```bash
git add crates/zoid-model/src/lib.rs
git commit -m "fix(model): flip prompt_cache to false for <affected> (live endpoint does not return cache-read tokens)"
```

Otherwise, no commit — the slice is complete.

---

## Self-Review Checklist (run after the final task)

**Spec coverage:**
- [x] §4.1 architecture & module layout — Tasks 4, 5, 6 (openai_compat.rs, opencode_go.rs) + Task 8 (bin wiring)
- [x] §4.2 OpenAICompatProvider wire — Task 4 (request_body, parse_chunk, accumulator), Task 5 (struct + streaming + list_models)
- [x] §4.2.1 Message.tool_call_id — Task 1 (field + constructor), Task 2 (agent loop threading)
- [x] §4.3 OpenCodeGoProvider + wire-shape map — Task 6
- [x] §4.4 registry + MODEL_CAPS + bin — Task 7 (model), Task 8 (bin)
- [x] §5 testing & verification — Tasks 1-8 include the tests; Task 9 is the DoD + smoke
- [x] §5 known day-one gap (5 Anthropic-shape models tools=false) — Task 7's `opencode_go_model_caps_match_reconciled_table` locks it
- [x] §5 deepseek-v4-pro correction regression — Task 7's `deepseek_v4_pro_correction_locked` + the two updated existing tests
- [x] §5 not-tested-in-CI items (live endpoint, prompt_cache empirical, Anthropic-shape tools, rate limits) — Task 9 manual smoke

**Placeholder scan:** no TODOs/TBDs; the one `unimplemented!` in Task 2 Step 1 is a placeholder for the test body the implementer must adapt to the existing `EventLog` test helper pattern (the comment spells out the exact assertion contract). That's intentional guidance, not a plan placeholder.

**Type consistency:** `Message::tool_with_call_id` (Task 1) → used in `agent.rs:161` (Task 2) → read by `openai_compat::request_body` (Task 4). `OpenAICompatProvider::new().with_base_url().with_idle_timeout()` (Task 5) → called in `opencode_go.rs` (Task 6) and `main.rs` (Task 8). `OpenCodeGoProvider::new().with_base_url()` (Task 6) → called in `main.rs` (Task 8). `parse_data_id_models` (Task 3) → called in `openai_compat.rs` (Task 5). Names match across tasks.