# Anthropic Provider Flesh-Out Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote zoid's text-only `AnthropicProvider` to a typed internal submodule that wires Anthropic Messages-API tool-use (parity with the Ollama provider), plus connect-phase 429 retry, `anthropic-beta` header plumbing, and correct extended-thinking parsing — and remove the two dead registry rows the spike falsified.

**Architecture:** Replace `crates/zoid-provider/src/anthropic.rs` (single file, `json!` blobs) with `crates/zoid-provider/src/anthropic/` (typed submodule: `types.rs` serde structs, `request.rs` builder, `parse.rs` SSE mapping, `cache.rs` breakpoint placement, `mod.rs` provider/stream loop/retry). The `Provider` trait contract is untouched — `AnthropicProvider` still emits `ProviderEvent::{TextDelta, ToolCall, Usage, Truncated, Done, Error}` into the same `mpsc::Sender`. The agent loop needs no changes.

**Tech Stack:** Rust 2021, tokio, reqwest (rustls), serde/serde_json, eventsource-stream, futures-util, tracing, anyhow. Tests: `#[test]` + `#[tokio::test]`, stalling-server pattern from the existing `anthropic.rs`/`ollama.rs` tests.

**Prerequisite:** This plan rebases onto `feature/opencode-go-provider` (or its merge to `main`), which already added `Message.tool_call_id: Option<String>` + `Message::tool_with_call_id(name, call_id, content)` + agent-loop `map_msg` threading (commits `20c4048`, `5d18e30`). Task 1 consumes `m.tool_call_id`; it does NOT re-widen `Message`.

## Global Constraints

- **Provider seam (`zoid-provider`) stays free of any `zoid-core` dependency.** The typed submodule is self-contained, mirroring the existing `anthropic`/`ollama` modules.
- **`serde_json::Value` does not implement `Eq`.** Wire types carrying `Value` (`ToolDef.input_schema`, `ContentBlock::ToolUse.input`, `ToolCall.args`) derive `PartialEq` only — drop `Eq`. The existing `ToolCall`/`ToolSpec` already follow this.
- **Warning-free + clippy-clean:** `cargo build` and `cargo clippy --all-targets` emit **0 warnings**. When a task removes the last use of an import, remove the import.
- **TDD discipline — honest split:** Tasks with external dependencies (I/O, network, the stream loop in Task 7) follow strict red → green (write the test referencing a not-yet-existing function, run it, watch it fail to compile, then write the impl). **Pure functions** (`place_breakpoints`, `build`, `event`, `coerce_args` in Tasks 3–6) are written impl + tests together in one step, with the test run as the green check — the "red" state is the empty file failing to compile against the test module that references it. This is a deliberate, honest adaptation: pure functions are the *easiest* to TDD and writing them twice is busywork, but the plan does not pretend they're red-green when they're green-green. Either approach is acceptable; the rule is "never lie about which one you're doing."
- **Commits:** Conventional Commits style; **never** add a `Co-Authored-By` or any co-author trailer (user's `~/CLAUDE.md`).
- **Never panic on malformed input.** Every parse function returns `Vec<ProviderEvent>` (possibly empty) — matching the existing `parse_event`/`parse_one` behavior at `anthropic.rs:66-80`.
- **No blanket `_` arms on `match` over provider enums** (`StreamEvent`, `ContentBlock`, `Delta`, `MsgRole`) — handle each variant explicitly so future additions are compile-breaking, not silent drops. (Exception: unknown SSE `type` strings fall through to `vec![]` because the API ships new event types without notice.)

**Non-goals (do NOT build):**
- Thinking-block replay across compaction (parsed + discarded this slice; §7.2 of spec).
- 5m cache TTL config knob (`CacheKind::Ephemeral5m` typed but unused).
- Mid-stream resume on 429.
- `fetch_model_info` for Anthropic (`/v1/models` doesn't expose caps; static `MODEL_CAPS` is the fallback).
- Re-widening `Message` (already done on `feature/opencode-go-provider`).

---

## File Structure

- **`crates/zoid-provider/src/anthropic.rs`** *(delete)* — single-file module replaced by the submodule directory.
- **`crates/zoid-provider/src/anthropic/mod.rs`** *(create)* — `AnthropicProvider`, `Provider` impl, stream loop, 429 retry, `list_models`, `fetch_model_info`, `ToolUseAccumulator` ownership, `DEFAULT_MODEL`.
- **`crates/zoid-provider/src/anthropic/types.rs`** *(create)* — typed request/response: `AnthropicRequest`, `ContentBlock`, `AnthropicMessage`, `AnthropicRole`, `MessageContent`, `ToolDef`, `ToolUse`, `ToolResult`, `ThinkingBlock`, `CacheControl`, `CacheKind`, `SystemBlock`, `ThinkingConfig`, `StreamEvent`, `Delta`, `ContentBlockStart`, `MessageStart`, `MessageDeltaBody`, `Usage`, `ApiError`.
- **`crates/zoid-provider/src/anthropic/request.rs`** *(create)* — `build(req: &CompletionRequest) -> AnthropicRequest` translation.
- **`crates/zoid-provider/src/anthropic/parse.rs`** *(create)* — `event(frame: StreamEvent, acc: &mut ToolUseAccumulator) -> Vec<ProviderEvent>`, `ToolUseAccumulator`, `coerce_args(partial_json: &str) -> Value`.
- **`crates/zoid-provider/src/anthropic/cache.rs`** *(create)* — `place_breakpoints(&mut AnthropicRequest)`.
- **`crates/zoid-provider/src/lib.rs`** *(modify)* — `pub mod anthropic;` stays (now resolves to the directory); `default_provider`/`default_model` unchanged.
- **`crates/zoid-model/src/lib.rs`** *(modify)* — remove `anthropic-cli`/`anthropic-sdk` registry rows; flip `MODEL_CAPS` `tools: false → true` for both Claude models; delete the tests referencing the removed rows.
- **`crates/zoid-provider/Cargo.toml`** *(no change)* — no new deps (serde/serde_json/reqwest/eventsource-stream/futures-util already present).

---

## Task 0: Prerequisite guard

**Files:** none (verification-only).

This plan consumes `Message.tool_call_id` (added on `feature/opencode-go-provider`, commits `20c4048`/`5d18e30`). If that branch isn't merged to `main` (or this branch isn't rebased onto it), Task 4 won't compile and a subagent will get stuck with no pointer to why. This task fails fast.

- [ ] **Step 1: Verify the prerequisite is in HEAD**

Run:
```bash
git merge-base --is-ancestor 5d18e30 HEAD && echo "prerequisite present" || \
  echo "FATAL: prerequisite commit 5d18e30 (Message.tool_call_id) is not in HEAD. Rebase onto feature/opencode-go-provider (or merge it to main) before starting this plan."
```

Expected: `prerequisite present`. If you see the FATAL message, stop and rebase/merge before Task 1.

- [ ] **Step 2: Sanity-check the field exists**

Run:
```bash
rg -n 'tool_call_id' crates/zoid-provider/src/lib.rs | head -3
```

Expected: at least one match (the `Message.tool_call_id` field declaration + the `tool_with_call_id` constructor). If no matches, the prerequisite isn't actually merged despite the ancestor check — stop and reconcile.

---

## Task 1: `types.rs` — typed wire structs (request side)

**Files:**
- Create: `crates/zoid-provider/src/anthropic/types.rs`
- Modify: `crates/zoid-provider/src/anthropic.rs` → split: rename to `mod.rs`, keep existing code, add `pub mod types;` (compiles but unused until Task 5)
- Test: `crates/zoid-provider/src/anthropic/types.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `AnthropicRequest`, `AnthropicMessage`, `AnthropicRole`, `MessageContent`, `ContentBlock`, `CacheControl`, `CacheKind`, `SystemBlock`, `ToolDef`, `ThinkingConfig` — all `Serialize` (and `Deserialize` where tests round-trip).
- Consumes: `serde_json::Value` (for `ToolDef.input_schema`, `ContentBlock::ToolUse.input`).

> **Note on the file split:** Tasks 1–4 create the new submodule files alongside the existing `anthropic.rs`. The `anthropic.rs` file is renamed to `anthropic/mod.rs` at Step 1 below (git mv preserves history). The existing code stays compilable throughout — `pub mod types;` in `mod.rs` is the only wiring change in this task. Tasks 5–7 replace the old code in `mod.rs` piece by piece.

- [ ] **Step 1: Split the file (git mv preserves history)**

Run:
```bash
git mv crates/zoid-provider/src/anthropic.rs crates/zoid-provider/src/anthropic/mod.rs
```

Then add `pub mod types;` near the top of `crates/zoid-provider/src/anthropic/mod.rs` (after the existing `use` lines, before `pub const DEFAULT_MODEL`):

```rust
pub mod types;
```

Create `crates/zoid-provider/src/anthropic/types.rs` as an empty file for now (the test in Step 2 will fail to compile until Step 3).

Run: `cargo build -p zoid-provider`
Expected: PASS (empty `types.rs` is a valid module; `pub mod types;` resolves).

- [ ] **Step 2: Write the failing test**

Append to `crates/zoid-provider/src/anthropic/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_request_serializes_minimal_body() {
        let req = AnthropicRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 1024,
            stream: true,
            messages: vec![AnthropicMessage {
                role: AnthropicRole::User,
                content: MessageContent::Text("hi".into()),
            }],
            system: None,
            tools: vec![],
            thinking: None,
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 1024,
                "stream": true,
                "messages": [{ "role": "user", "content": "hi" }]
            })
        );
        // empty tools/system/thinking must NOT appear on the wire
        assert!(body.get("tools").is_none());
        assert!(body.get("system").is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn content_block_tool_use_serializes() {
        let block = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: json!({"path": "a.txt"}),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "tool_use",
                "id": "toolu_1",
                "name": "read_file",
                "input": {"path": "a.txt"}
            })
        );
    }

    #[test]
    fn content_block_tool_result_serializes() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "file body".into(),
            is_error: None,
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "file body"
            })
        );
        assert!(v.get("is_error").is_none()); // skip_serializing_if
    }

    #[test]
    fn content_block_text_with_cache_control_serializes() {
        let block = ContentBlock::Text {
            text: "hi".into(),
            cache_control: Some(CacheControl { kind: CacheKind::Ephemeral1h }),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "text",
                "text": "hi",
                "cache_control": {"type": "ephemeral"}
            })
        );
    }

    #[test]
    fn message_content_text_emits_plain_string() {
        let msg = AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text("hi".into()),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v, json!({"role": "user", "content": "hi"}));
    }

    #[test]
    fn message_content_blocks_emits_array() {
        let msg = AnthropicMessage {
            role: AnthropicRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "hello".into(),
                cache_control: None,
            }]),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            v,
            json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "hello"}]
            })
        );
        assert!(v["content"][0].get("cache_control").is_none()); // skip_serializing_if
    }

    #[test]
    fn content_block_thinking_serializes() {
        // Thinking is typed + round-trips, even though it's discarded from
        // the event stream this slice (spec §7.2). The variant exists so the
        // parse side doesn't crash on it.
        let block = ContentBlock::Thinking {
            thinking: "reasoning".into(),
            signature: "sig".into(),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "thinking",
                "thinking": "reasoning",
                "signature": "sig"
            })
        );
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::types`
Expected: FAIL to compile (`AnthropicRequest` undefined, etc.).

- [ ] **Step 4: Write minimal implementation**

Write the request-side types in `crates/zoid-provider/src/anthropic/types.rs` (above the test module):

```rust
//! Typed Anthropic Messages-API wire structs. Replaces the hand-built `json!`
//! blobs in the legacy `anthropic.rs::request_body`. Serialize-only on the
//! request side; the response side (`StreamEvent`) is added in Task 2.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: CacheKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    /// 1-hour ephemeral cache (the current default).
    Ephemeral1h,
    /// 5-minute ephemeral cache (typed seam; no config knob exposes it yet).
    Ephemeral5m,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain string shorthand: `"content": "hi"`. Used for interior messages
    /// without cache_control or tool blocks.
    Text(String),
    /// Block array: `"content": [{"type":"text",...}]`. Used when a message
    /// carries cache_control, tool_use, or tool_result blocks.
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// Anthropic's field name is `input_schema` (not OpenAI's `parameters`).
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct SystemBlock {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

// Note: SystemBlock wraps a single text; Anthropic's system field is an array
// of these blocks. `AnthropicRequest.system: Option<Vec<SystemBlock>>` models
// that. We can't `#[serde(skip_serializing_if)]` on the Vec directly inside the
// Option, so the `Option` itself encodes presence.

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct ThinkingConfig {
    pub r#type: ThinkingType,
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingType {
    Enabled,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic::types`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src/anthropic/
git commit -m "feat(provider): typed Anthropic request wire structs (types.rs)"
```

---

## Task 2: `types.rs` — typed SSE response structs

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/types.rs`
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `StreamEvent` (tagged enum), `Delta`, `ContentBlockStart`, `MessageStart`, `MessageDeltaBody`, `Usage`, `ApiError`.

- [ ] **Step 1: Write the failing test**

Append to the test module in `crates/zoid-provider/src/anthropic/types.rs`:

```rust
    #[test]
    fn stream_event_message_start_with_cache_tokens() {
        let frame = r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"cache_read_input_tokens":40,"cache_creation_input_tokens":3}}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::MessageStart { message } => {
                assert_eq!(message.usage.input_tokens, 7);
                assert_eq!(message.usage.cache_read_input_tokens, 40);
                assert_eq!(message.usage.cache_creation_input_tokens, 3);
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_start_tool_use() {
        let frame = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockStart { index, content_block } => {
                assert_eq!(index, 0);
                match content_block {
                    ContentBlockStart::ToolUse { id, name } => {
                        assert_eq!(id, "toolu_1");
                        assert_eq!(name, "read_file");
                    }
                    other => panic!("expected ToolUse, got {other:?}"),
                }
            }
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_delta_text() {
        let frame = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                match delta {
                    Delta::TextDelta { text } => assert_eq!(text, "Hello"),
                    other => panic!("expected TextDelta, got {other:?}"),
                }
            }
            other => panic!("expected ContentBlockDelta, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_delta_input_json() {
        let frame = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockDelta { index, delta: Delta::InputJsonDelta { partial_json } } => {
                assert_eq!(index, 0);
                assert_eq!(partial_json, r#"{"path":"#);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_delta_with_stop_reason() {
        let frame = r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":4096}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("max_tokens"));
                assert_eq!(usage.output_tokens, 4096);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_stop() {
        let frame = r#"{"type":"message_stop"}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        assert!(matches!(ev, StreamEvent::MessageStop));
    }

    #[test]
    fn stream_event_error() {
        let frame = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::Error { error } => assert_eq!(error.message, "Overloaded"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_ping() {
        let frame = r#"{"type":"ping"}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        assert!(matches!(ev, StreamEvent::Ping));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::types`
Expected: FAIL to compile (`StreamEvent` undefined).

- [ ] **Step 3: Write minimal implementation**

Append to `crates/zoid-provider/src/anthropic/types.rs` (above the test module):

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart { message: MessageStart },
    ContentBlockStart { index: u32, content_block: ContentBlockStart },
    ContentBlockDelta { index: u32, delta: Delta },
    ContentBlockStop { index: u32 },
    MessageDelta { delta: MessageDeltaBody, usage: Usage },
    MessageStop,
    Error { error: ApiError },
    Ping,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockStart {
    Text,
    ToolUse { id: String, name: String },
    Thinking,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MessageStart {
    pub usage: Usage,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MessageDeltaBody {
    /// `stop_reason` is present on the terminal `message_delta` only; absent on
    /// intermediate ones. `None` means "still streaming".
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiError {
    pub message: String,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic::types`
Expected: PASS (all tests including Task 1's 6).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/types.rs
git commit -m "feat(provider): typed Anthropic SSE response structs (StreamEvent)"
```

---

## Task 3: `cache.rs` — breakpoint placement

**Files:**
- Create: `crates/zoid-provider/src/anthropic/cache.rs`
- Modify: `crates/zoid-provider/src/anthropic/mod.rs` — add `pub mod cache;`
- Test: `crates/zoid-provider/src/anthropic/cache.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `place_breakpoints(&mut AnthropicRequest)`.

- [ ] **Step 1: Wire the module**

Add to `crates/zoid-provider/src/anthropic/mod.rs` (near `pub mod types;`):

```rust
pub mod cache;
```

Create `crates/zoid-provider/src/anthropic/cache.rs` empty.

Run: `cargo build -p zoid-provider` — Expected: PASS.

- [ ] **Step 2: Write the failing test**

Append to `crates/zoid-provider/src/anthropic/cache.rs`:

```rust
use super::types::{
    AnthropicMessage, AnthropicRequest, AnthropicRole, CacheControl, CacheKind, ContentBlock,
    MessageContent, SystemBlock,
};

/// Place ephemeral (1h) cache breakpoints on the system block and on the last
/// message's last block. Interior messages stay plain. Mirrors the rolling-
/// breakpoint behavior of the legacy `request_body` (anthropic.rs:42-58): the
/// previous turn's breakpoint becomes an interior read on the next turn, and
/// the new breakpoint extends the cached prefix.
pub fn place_breakpoints(req: &mut AnthropicRequest) {
    if let Some(sys) = req.system.as_mut() {
        for block in sys.iter_mut() {
            block.cache_control = Some(CacheControl {
                kind: CacheKind::Ephemeral1h,
            });
        }
    }
    if let Some(last_msg) = req.messages.last_mut() {
        if let MessageContent::Blocks(blocks) = &mut last_msg.content {
            if let Some(last_block) = blocks.last_mut() {
                if let ContentBlock::Text { cache_control, .. } = last_block {
                    *cache_control = Some(CacheControl {
                        kind: CacheKind::Ephemeral1h,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_text(s: &str) -> AnthropicMessage {
        AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text(s.into()),
        }
    }

    fn user_blocks(blocks: Vec<ContentBlock>) -> AnthropicMessage {
        AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Blocks(blocks),
        }
    }

    #[test]
    fn places_breakpoint_on_system_when_present() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_text("x")],
            system: Some(vec![SystemBlock {
                text: "be terse".into(),
                cache_control: None,
            }]),
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        let sys = req.system.as_ref().unwrap();
        assert_eq!(
            sys[0].cache_control,
            Some(CacheControl {
                kind: CacheKind::Ephemeral1h
            })
        );
    }

    #[test]
    fn no_system_no_system_breakpoint() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_text("x")],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        assert!(req.system.is_none());
    }

    #[test]
    fn places_breakpoint_on_last_message_last_block_only() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![
                user_text("a"),
                AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: MessageContent::Text("b".into()),
                },
                user_blocks(vec![ContentBlock::Text {
                    text: "c".into(),
                    cache_control: None,
                }]),
            ],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        // interior messages unchanged
        assert!(matches!(req.messages[0].content, MessageContent::Text(_)));
        assert!(matches!(req.messages[1].content, MessageContent::Text(_)));
        // last message's last block gets the breakpoint
        match &req.messages[2].content {
            MessageContent::Blocks(blocks) => match &blocks[0] {
                ContentBlock::Text { cache_control, .. } => assert_eq!(
                    *cache_control,
                    Some(CacheControl {
                        kind: CacheKind::Ephemeral1h
                    })
                ),
                other => panic!("got {other:?}"),
            },
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn interior_blocks_stay_plain() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_blocks(vec![
                ContentBlock::Text {
                    text: "first".into(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "second".into(),
                    cache_control: None,
                },
            ])],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        match &req.messages[0].content {
            MessageContent::Blocks(blocks) => {
                // first block stays plain
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Text { cache_control: None, .. }
                ));
                // last block gets the breakpoint
                assert!(matches!(
                    &blocks[1],
                    ContentBlock::Text {
                        cache_control: Some(_),
                        ..
                    }
                ));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn empty_messages_no_panic() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req); // must not panic
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::cache`
Expected: FAIL (functions/types not yet in scope for tests; the `use super::types::...` won't resolve if `cache.rs` is empty — but we wrote the code + tests together here. Run to confirm the test compiles and passes.)

> **TDD note:** This task writes impl + tests together because `place_breakpoints` is a pure function with no external dependencies. The "red" step is the compile failure of an empty `cache.rs` against the tests; the "green" step is the impl above making it pass. If you prefer a stricter red/green split, write only the test module first (with `place_breakpoints` referenced but undefined), watch it fail to compile, then add the impl.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic::cache`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/cache.rs crates/zoid-provider/src/anthropic/mod.rs
git commit -m "feat(provider): typed Anthropic cache-control breakpoint placement"
```

---

## Task 4: `request.rs` — `CompletionRequest` → `AnthropicRequest`

**Files:**
- Create: `crates/zoid-provider/src/anthropic/request.rs`
- Modify: `crates/zoid-provider/src/anthropic/mod.rs` — add `pub mod request;`
- Test: `crates/zoid-provider/src/anthropic/request.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `build(req: &crate::CompletionRequest) -> AnthropicRequest`.
- Consumes: `crate::{CompletionRequest, Message, MsgRole, ToolSpec}`, `super::types::*`, `super::cache::place_breakpoints`.

- [ ] **Step 1: Wire the module**

Add to `crates/zoid-provider/src/anthropic/mod.rs`:

```rust
pub mod request;
```

Create `crates/zoid-provider/src/anthropic/request.rs` empty.

Run: `cargo build -p zoid-provider` — Expected: PASS.

- [ ] **Step 2: Write the failing test**

Append to `crates/zoid-provider/src/anthropic/request.rs`:

```rust
use super::cache::place_breakpoints;
use super::types::*;
use crate::{CompletionRequest, Message, MsgRole, ToolCall, ToolSpec};
use serde_json::{json, Value};

/// Translate zoid's provider-agnostic `CompletionRequest` into a typed
/// `AnthropicRequest` ready for serde serialization. Tool-use replay, tool
/// results, and cache breakpoints are all handled here.
pub fn build(req: &CompletionRequest) -> AnthropicRequest {
    let messages: Vec<AnthropicMessage> = req.messages.iter().map(map_message).collect();
    let mut out = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        stream: true,
        messages,
        system: req.system.as_ref().map(|s| vec![SystemBlock {
            text: s.clone(),
            cache_control: None,
        }]),
        tools: req.tools.iter().map(tool_def).collect(),
        thinking: None,
    };
    place_breakpoints(&mut out);
    out
}

fn map_message(m: &Message) -> AnthropicMessage {
    match m.role {
        MsgRole::User => AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text(m.content.clone()),
        },
        MsgRole::Assistant => {
            // Replay assistant turns that requested tools: emit a block array
            // with a Text block (if non-empty) + one ToolUse block per tool_call.
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
    use crate::Message;

    fn req(messages: Vec<Message>, tools: Vec<ToolSpec>, system: Option<&str>) -> CompletionRequest {
        CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: system.map(String::from),
            messages,
            max_tokens: 1024,
            tools,
        }
    }

    #[test]
    fn plain_user_assistant_body() {
        let r = req(
            vec![Message::user("hi"), Message::assistant("hello")],
            vec![],
            None,
        );
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let r = req(
            vec![Message::tool("read_file", "bar")],
            vec![],
            None,
        );
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
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
        let body = serde_json::to_value(&build(&r)).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // interior messages stay plain strings
        assert_eq!(msgs[0]["content"], "a");
        assert_eq!(msgs[1]["content"], "b");
        // last message gets a cache breakpoint block array
        assert!(msgs[2]["content"].is_array());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::request`
Expected: FAIL (impl + tests written together; first run should pass, but if you wrote only the test module first, it fails to compile. If you wrote both together, skip to Step 4.)

> **TDD note:** Like Task 3, this task writes impl + tests together because `build` is pure. The compile error of an empty `request.rs` is the red state.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic::request`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/request.rs crates/zoid-provider/src/anthropic/mod.rs
git commit -m "feat(provider): typed Anthropic request builder (tool-use replay, tool_result)"
```

---

## Task 5: `parse.rs` — SSE event → `ProviderEvent` (text + usage + done)

**Files:**
- Create: `crates/zoid-provider/src/anthropic/parse.rs`
- Modify: `crates/zoid-provider/src/anthropic/mod.rs` — add `pub mod parse;`
- Test: `crates/zoid-provider/src/anthropic/parse.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `event(frame: StreamEvent, acc: &mut ToolUseAccumulator) -> Vec<ProviderEvent>`, `ToolUseAccumulator`, `coerce_args(partial_json: &str) -> Value`.
- Consumes: `super::types::*`, `crate::{ProviderEvent, ToolCall, Usage}`.

> **Scope:** This task handles the non-tool-use arms (text, usage, message_stop, error, ping). Task 6 adds the tool-use accumulation arms. Splitting them keeps each task's diff reviewable.

- [ ] **Step 1: Wire the module**

Add to `crates/zoid-provider/src/anthropic/mod.rs`:

```rust
pub mod parse;
```

Create `crates/zoid-provider/src/anthropic/parse.rs` empty.

Run: `cargo build -p zoid-provider` — Expected: PASS.

- [ ] **Step 2: Write the failing test**

Append to `crates/zoid-provider/src/anthropic/parse.rs`:

```rust
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
        StreamEvent::ContentBlockStart { index, content_block } => match content_block {
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
    use super::*;

    #[test]
    fn message_start_folds_cache_tokens_into_input() {
        let frame = StreamEvent::MessageStart {
            message: MessageStart {
                usage: Usage {
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
                usage: Usage {
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
            usage: Usage {
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
            usage: Usage {
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
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::parse`
Expected: FAIL to compile (impl + tests written together; first run should pass. If you wrote only the test module first, it fails.)

> **TDD note:** Like Tasks 3–4, impl + tests are together because `event` is pure. Run to confirm green.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic::parse`
Expected: PASS (11 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/parse.rs crates/zoid-provider/src/anthropic/mod.rs
git commit -m "feat(provider): typed Anthropic SSE parser (text/usage/done/error)"
```

---

## Task 6: `parse.rs` — tool-use accumulation tests

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/parse.rs` (add tests to the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to the test module in `crates/zoid-provider/src/anthropic/parse.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p zoid-provider anthropic::parse`
Expected: PASS (all parse tests including these 5 new ones; the impl in Task 5 already handles tool-use).

> **Note:** These tests pass on first run because the Task 5 impl already covers tool-use accumulation. They exist to lock in the behavior and catch regressions. This is "characterization tests" — the impl was written in Task 5; these confirm the tool-use path specifically.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-provider/src/anthropic/parse.rs
git commit -m "test(provider): tool-use accumulation across SSE frames (parse.rs)"
```

---

## Task 7: `mod.rs` — provider, stream loop, retry, headers

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/mod.rs` (replace the legacy `request_body`/`parse_event`/`parse_one`/`AnthropicProvider` with the new submodule-backed version)

**Interfaces:**
- `AnthropicProvider::new`, `::with_base_url`, `::with_idle_timeout`, `::with_betas`, `Provider` impl (`stream`, `list_models`) — same surface as today plus `with_betas`.
- `fetch_model_info` is **NOT** overridden — inherits the `Provider` trait default `Ok(None)` (spec §7.4). Do not add it.
- `DEFAULT_MODEL` unchanged.
- `parse_anthropic_models` stays inline in `mod.rs` (it's about the `/v1/models` HTTP endpoint, not SSE parsing — it does NOT move to `parse.rs`).

> **This is the cutover task:** the typed submodules (Tasks 1–6) were built alongside the legacy code; this task deletes the legacy `request_body`/`parse_event`/`parse_one` and rewrites `AnthropicProvider::stream` to use `request::build` + `parse::event` + a `ToolUseAccumulator`. The three stalling-server timeout tests from the legacy `anthropic.rs:538-646` are ported here. The two `parse_anthropic_models` tests (legacy `anthropic.rs:524-536`) are kept verbatim — do NOT delete them (they test a function that still lives here). Two new 429 retry tests + two `with_betas` tests are added.

- [ ] **Step 1: Write the failing tests (retry + betas)**

Add to the `#[cfg(test)] mod tests` in `crates/zoid-provider/src/anthropic/mod.rs` (after the existing tests, which we'll port in Step 3):

```rust
    #[test]
    fn with_betas_sets_header_value() {
        let p = AnthropicProvider::new("k".into())
            .with_betas(vec!["extended-thinking-2025-05-14".into()]);
        assert_eq!(
            p.beta_header_value().as_deref(),
            Some("extended-thinking-2025-05-14")
        );
    }

    #[test]
    fn empty_betas_omits_header() {
        let p = AnthropicProvider::new("k".into());
        assert!(p.beta_header_value().is_none());
    }

    #[test]
    fn multiple_betas_are_comma_joined() {
        let p = AnthropicProvider::new("k".into()).with_betas(vec![
            "extended-thinking-2025-05-14".into(),
            "fine-grained-tool-streaming-2025-05-14".into(),
        ]);
        assert_eq!(
            p.beta_header_value().as_deref(),
            Some("extended-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14")
        );
    }
```

Then the two retry tests:

```rust
    /// Spawn a server that responds 429 once (with retry-after: 0) then 200
    /// with a minimal SSE stream. Returns the bound address. Two accepts on
    /// one listener: first gets the 429, second gets the 200 + SSE.
    async fn spawn_429_then_ok_server() -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // first connection: 429
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 0\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.flush().await;
            // second connection: 200 + minimal SSE (a single message_stop)
            let (mut sock2, _) = listener.accept().await.unwrap();
            let mut buf2 = [0u8; 4096];
            let _ = sock2.read(&mut buf2).await;
            let _ = sock2
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
                .await;
            let _ = sock2.flush().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        addr
    }

    #[tokio::test]
    async fn retry_on_429_then_succeeds() {
        let addr = spawn_429_then_ok_server().await;
        let provider = AnthropicProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(5));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        // after retry, the stream emits Done (from the message_stop frame)
        assert!(
            got.iter().any(|e| matches!(e, ProviderEvent::Done)),
            "expected a Done after retry, got {got:?}"
        );
    }

    /// A server that always returns 429 for up to MAX_RETRIES+2 connections.
    /// After the retry loop exhausts (MAX_RETRIES retries), the provider must
    /// surface an Error mentioning 429.
    async fn spawn_always_429_server() -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // MAX_RETRIES + 2 = 5 connections; each returns 429.
            for _ in 0..5 {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    let _ = sock.flush().await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn retry_exhausted_surfaces_error() {
        let addr = spawn_always_429_server().await;
        let provider = AnthropicProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(5));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(
            matches!(got.last(), Some(ProviderEvent::Error(e)) if e.contains("429")),
            "expected a trailing 429 Error, got {got:?}"
        );
    }
}
```

> **Retry test backoff note:** the `retry-after: 0` header and `BASE_BACKOFF=500ms` mean each retry sleeps ~500ms + jitter; `retry_on_429_then_succeeds` sleeps ~500ms once, `retry_exhausted_surfaces_error` sleeps ~500ms three times (~1.5s total). Both stay well under the 5s `idle_timeout`. If you change `BASE_BACKOFF` or `MAX_RETRIES`, re-check these test timings.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider anthropic::tests::retry_on_429_then_succeeds anthropic::tests::retry_exhausted_surfaces_error`
Expected: FAIL (the legacy `stream()` doesn't retry; `retry_on_429_then_succeeds` will hang or time out without a Done; `retry_exhausted_surfaces_error` may pass on the legacy impl since it already surfaces 429 — that's fine, the new impl must keep it passing).

- [ ] **Step 3: Replace the legacy `mod.rs` body with the typed cutover**

In `crates/zoid-provider/src/anthropic/mod.rs`, delete the legacy `request_body`, `parse_event`, `parse_one`, and the old `AnthropicProvider::stream` impl. Replace with the new submodule-backed versions. The full new `mod.rs` body (after the `pub mod` declarations and `use` imports) is:

```rust
pub mod cache;
pub mod parse;
pub mod request;
pub mod types;

use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use parse::ToolUseAccumulator;
use std::time::Duration;
use tokio::sync::mpsc;

/// Default model when `$ZOID_MODEL` is unset (latest Claude Sonnet).
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);

/// Extract model ids from an Anthropic `/v1/models` response body. Lenient.
/// Lives here in `mod.rs` (NOT `parse.rs`) because it's about the HTTP models
/// endpoint, not SSE streaming.
pub fn parse_anthropic_models(body: &str) -> Vec<String> {
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

/// Streaming Anthropic Messages API provider.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
    /// Beta feature flags sent as the `anthropic-beta` header (comma-joined).
    /// Populated from config or `ZOID_ANTHROPIC_BETAS`. Empty = no header.
    betas: Vec<String>,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("anthropic-api")
                .unwrap_or("https://api.anthropic.com")
                .to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
            betas: Vec::new(),
        }
    }

    /// Override the default base URL. Empty/whitespace ignored; trailing slash trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }

    /// Override the stream idle/response timeout. Primarily for tests.
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }

    /// Set the `anthropic-beta` header flags. Empty clears them.
    pub fn with_betas(mut self, betas: Vec<String>) -> Self {
        self.betas = betas;
        self
    }

    fn beta_header_value(&self) -> Option<String> {
        if self.betas.is_empty() {
            None
        } else {
            Some(self.betas.join(","))
        }
    }

    /// Build the request headers (x-api-key, anthropic-version, optional beta).
    /// All inserts are fallible `if let Ok` — never panics on a malformed api
    /// key or beta value (the header is simply skipped, matching the "never
    /// panic on malformed input" constraint).
    fn request_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = self.api_key.as_str().parse() {
            headers.insert("x-api-key", v);
        }
        if let Ok(v) = "2023-06-01".parse() {
            headers.insert("anthropic-version", v);
        }
        if let Ok(v) = "application/json".parse() {
            headers.insert("content-type", v);
        }
        if let Some(beta) = self.beta_header_value() {
            if let Ok(v) = beta.parse() {
                headers.insert("anthropic-beta", v);
            }
        }
        headers
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        // `fetch_model_info` is NOT overridden — inherits the trait default
        // `Ok(None)` (spec §7.4). Only `stream` and `list_models` are impl'd.
        self.stream_with_retries(req, &sink, 0).await
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;
        Ok(parse_anthropic_models(&resp.text().await?))
    }
}

impl AnthropicProvider {
    /// Connect-phase send with bounded 429 retry. `attempt` is the zero-based
    /// retry index (0 = first try). On 429 with `attempt < MAX_RETRIES`, sleep
    /// `retry-after` (or exponential backoff) + jitter, then recurse with
    /// `attempt + 1`. The recursion is bounded by the `attempt < MAX_RETRIES`
    /// check *before* recursing, so `attempt` strictly grows — unlike a naive
    /// `stream()` re-entry that would reset the counter. Tail-recursion at
    /// depth ≤ 3 does not need `Box::pin` on modern rustc.
    async fn stream_with_retries(
        &self,
        req: &CompletionRequest,
        sink: &mpsc::Sender<ProviderEvent>,
        attempt: u32,
    ) -> Result<()> {
        let body = request::build(req);
        let url = format!("{}/v1/messages", self.base_url);

        let send = self
            .client
            .post(&url)
            .headers(self.request_headers())
            .json(&body)
            .send();
        let resp = match tokio::time::timeout(self.idle_timeout, send).await {
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

        // 429 retry (connect-phase only). Mid-stream overload is terminal.
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            if attempt < MAX_RETRIES {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| BASE_BACKOFF.saturating_mul(2u32.pow(attempt)));
                let jitter = Duration::from_millis(rand_jitter_ms());
                tracing::warn!(attempt, "anthropic 429; retrying after backoff");
                tokio::time::sleep(retry_after + jitter).await;
                return self.stream_with_retries(req, sink, attempt + 1).await;
            }
            // exhausted: surface the 429 as an Error
            let _ = sink
                .send(ProviderEvent::Error(format!(
                    "HTTP 429: retried {MAX_RETRIES} times"
                )))
                .await;
            return Ok(());
        }

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

        self.stream_sse(resp, sink).await
    }

    /// Drive the SSE stream after a successful 200 response. Owns the
    /// `ToolUseAccumulator` and maps each frame via `parse::event`.
    async fn stream_sse(
        &self,
        resp: reqwest::Response,
        sink: &mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;
        let mut acc = ToolUseAccumulator::default();
        let mut stream = resp.bytes_stream().eventsource();
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
                    break;
                }
            };
            match item {
                Ok(event) => {
                    let mut stop = false;
                    // Deserialize the SSE data as a typed StreamEvent; unknown
                    // types fall through to None (no panic).
                    let frame: Option<types::StreamEvent> =
                        serde_json::from_str(&event.data).ok();
                    if let Some(frame) = frame {
                        for pe in parse::event(frame, &mut acc) {
                            if ttft.is_none() {
                                ttft = Some(start.elapsed().as_millis() as u64);
                            }
                            let is_done = matches!(pe, ProviderEvent::Done);
                            if sink.send(pe).await.is_err() {
                                stop = true;
                                break;
                            }
                            if is_done {
                                stop = true;
                                break;
                            }
                        }
                    }
                    if stop {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }
        tracing::info!(
            kind = "provider",
            provider = "anthropic",
            ttft_ms = ttft.unwrap_or(0),
            total_ms = start.elapsed().as_millis() as u64,
            "provider stream complete"
        );
        Ok(())
    }
}

/// Non-cryptographic jitter from wall-clock nanos; sufficient for retry
/// spacing (avoids pulling the `rand` workspace dep into this crate).
fn rand_jitter_ms() -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % 250
}
```

> **Retry design note:** `stream_with_retries` is tail-recursive and bounded — `attempt` strictly increments (0 → 1 → 2 → 3) and the `attempt < MAX_RETRIES` check fires *before* recursing, so at most 3 retries (4 total attempts). `Box::pin` is not needed at depth ≤ 3 on modern rustc; if clippy warns, wrap only the recursive call site: `return Box::pin(self.stream_with_retries(req, sink, attempt + 1)).await;`.

- [ ] **Step 4: Port the legacy timeout tests**

In the `#[cfg(test)] mod tests` of `crates/zoid-provider/src/anthropic/mod.rs`:

- **Keep verbatim** from the legacy `anthropic.rs`: `spawn_stalling_server`, `OK_SSE_HEADERS`, `probe_req`, `idle_timeout_emits_error_when_stream_stalls`, `request_timeout_emits_error_when_no_response`, `error_body_timeout_emits_error_when_body_stalls` (lines 538-646); `new_uses_default_base_url`, `with_base_url_overrides_and_trims_trailing_slash`, `with_base_url_ignores_empty_or_blank` (lines 326-355); **and** `parses_anthropic_model_ids`, `anthropic_models_bad_is_empty` (lines 524-536 — these test `parse_anthropic_models`, which still lives in `mod.rs`; do NOT delete them with the other legacy tests).
- **Add** the two retry tests + three `with_betas` tests from Step 1.
- **Delete** only the legacy `request_body`/`parse_event`/`parse_one` tests (lines 357-522, 558-616 — they're replaced by the typed tests in Tasks 1–6). Specifically: `builds_messages_body_with_stream_flag`, `includes_system_as_cacheable_block_when_present`, `caches_only_the_last_message`, `parses_text_delta`, `parses_message_stop_as_done`, `parses_message_delta_usage`, `parses_message_start_input_usage`, `message_start_folds_cache_tokens_into_input_and_reports_cached`, `parses_error`, `ignores_unhandled_frames`, `malformed_data_yields_empty_not_panic`, `message_delta_with_max_tokens_stop_yields_usage_then_truncated`. Keep `parses_anthropic_model_ids` and `anthropic_models_bad_is_empty`.

The final test module needs these imports at the top of the `#[cfg(test)] mod tests`:

```rust
use super::*;
use crate::{Message, ProviderEvent};
use std::time::Duration;
use tokio::sync::mpsc;
```

- [ ] **Step 5: Run all tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (all anthropic submodule tests + selection_tests + tests in lib.rs).

Run: `cargo clippy --all-targets -p zoid-provider`
Expected: 0 warnings.

Run: `cargo fmt --all --check -p zoid-provider` (or `cargo fmt --all --check` workspace-wide)
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src/anthropic/mod.rs
git commit -m "feat(provider): typed AnthropicProvider stream loop + 429 retry + beta headers"
```

---

## Task 8: Remove dead registry rows + flip `MODEL_CAPS`

**Files:**
- Modify: `crates/zoid-model/src/lib.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Replace the `entry_resolves_through_alias_and_transport`, `default_base_url_only_for_http`, and `selectable_excludes_planned` tests in `crates/zoid-model/src/lib.rs` with versions that no longer reference `anthropic-cli`/`anthropic-sdk`:

```rust
    #[test]
    fn entry_anthropic_api_is_http() {
        let e = entry("anthropic-api").unwrap();
        assert_eq!(e.id, "anthropic-api");
        assert_eq!(e.family, "anthropic");
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://api.anthropic.com"
            }
        );
        assert_eq!(e.status, Status::Available);
    }

    #[test]
    fn default_base_url_anthropic_api_only() {
        assert_eq!(
            default_base_url("anthropic-api"),
            Some("https://api.anthropic.com")
        );
        // removed rows resolve to None (entry() returns None)
        assert!(entry("anthropic-cli").is_none());
        assert!(entry("anthropic-sdk").is_none());
        assert!(default_base_url("anthropic-cli").is_none());
        assert!(default_base_url("anthropic-sdk").is_none());
    }

    #[test]
    fn selectable_has_three_providers() {
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"ollama-local"));
        assert!(ids.contains(&"ollama-cloud"));
        assert!(ids.contains(&"anthropic-api"));
    }
```

Also add a test asserting the capability flip:

```rust
    #[test]
    fn claude_models_now_support_tools() {
        assert!(model_info("claude-sonnet-4-6").tools);
        assert!(model_info("claude-opus-4-8").tools);
        assert!(model_info("claude-sonnet-4-6").prompt_cache);
        assert!(model_info("claude-opus-4-8").prompt_cache);
    }
```

Remove the old `tools_capability_matches_what_providers_actually_support` test (it asserted `tools: false` for Claude, which is now wrong).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-model`
Expected: FAIL (`entry("anthropic-cli").is_none()` fails because the row still exists; `claude_models_now_support_tools` fails because `tools` is still false).

- [ ] **Step 3: Remove the registry rows + flip the caps**

In `crates/zoid-model/src/lib.rs`:

Delete the two `ProviderEntry` blocks for `anthropic-cli` (lines 85-94) and `anthropic-sdk` (lines 95-102). The `PROVIDERS` array now has 3 entries.

In `MODEL_CAPS`, for both `claude-sonnet-4-6` and `claude-opus-4-8` entries: change `tools: false` to `tools: true`. Replace the multi-line "Anthropic tool-use is not wired yet" comment with a single line: `// Tool-use wired via the typed anthropic submodule (P1b.1).`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-model`
Expected: PASS (all tests including the 4 new ones).

- [ ] **Step 5: Verify workspace builds + clippy clean**

Run: `cargo build --workspace`
Expected: PASS (the model picker reads `PROVIDERS` via `selectable()`; the removed rows no longer appear).

Run: `cargo clippy --all-targets -p zoid-model`
Expected: 0 warnings.

Run: `cargo fmt --all --check`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): remove falsified anthropic-cli/sdk rows; flip Claude tools:true"
```

---

## Task 9: Workspace verification + integration sanity

**Files:**
- Verify-only (no code changes unless a test fails).

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS (all 37+ test binaries). Pay attention to:
- `crates/zoid/tests/agent_loop.rs` — exercises `ProviderEvent::ToolCall` via `FakeProvider`; must still pass (the agent loop is provider-agnostic).
- `crates/zoid/tests/subagent_integration.rs` — same.
- `crates/zoid-tui` snapshot tests — must not regress (no TUI changes).

> **Deferred (spec §9 Layer 2):** the spec calls for "one new integration test: a scripted `FakeProvider` that replays a real Anthropic SSE byte stream" as the tool-use parity acceptance test. This is **deferred** — the unit-level `parse::event` tests in Tasks 5–6 demonstrate the same `StreamEvent` → `ProviderEvent` mapping, including tool-use accumulation. A byte-fixture integration test (captured real SSE bytes through `eventsource` → `parse::event`) is a follow-up, not part of this slice. A subagent executing this plan does NOT need to add it; noting the deferral here so it's not silently dropped.

- [ ] **Step 2: Clippy + fmt workspace-wide**

Run: `cargo clippy --all-targets`
Expected: 0 warnings.

Run: `cargo fmt --all --check`
Expected: clean.

- [ ] **Step 3: Manual smoke (if API key available)**

If `ANTHROPIC_API_KEY` is set in the env:
```bash
cargo run -p zoid -- -p "Use a tool: list files in the current directory."
```
Expected: the agent emits a `ToolCall` (read_file or similar), executes it, and continues to a final answer. The economy drawer shows non-zero usage with `cached` > 0 if the prompt exceeds the cache minimum.

If no key: skip this step (CI is keyless by design).

- [ ] **Step 4: Document the spike + spec + plan in CHANGELOG**

Append to `CHANGELOG.md` under `## Unreleased`:

```markdown
- Anthropic provider flesh-out: typed internal submodule (`anthropic/{types,request,parse,cache,mod}.rs`) replacing the hand-rolled `json!` wire; wires tool-use (parity with Ollama — `tools` array, `tool_use`/`tool_result` blocks, `tool_call_id` threading), connect-phase 429 retry with `retry-after` + exponential backoff, `anthropic-beta` header plumbing, and correct (if discarded) extended-thinking parsing. Removes the `anthropic-cli`/`anthropic-sdk` registry rows falsified by the `spikes/cc-infer` spike. Flips `MODEL_CAPS` `tools: true` for both Claude models.
```

- [ ] **Step 5: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for anthropic provider flesh-out"
```

---

## Self-Review (run after all tasks — gilfoyle-revised)

**1. Spec coverage:**
- §3 module layout → Tasks 1–7 (each file created).
- §4 typed wire format → Tasks 1 (request), 2 (response).
- §5 request building + `tool_call_id` threading + `tool_name` fallback → Task 4 (build) + prerequisite Task 0 (opencode-go merge).
- §6 SSE parsing + accumulator → Tasks 5 (non-tool arms), 6 (tool-use accumulation tests).
- §7.1 429 retry → Task 7 (`stream_with_retries` loop, bounded by `attempt < MAX_RETRIES`).
- §7.2 extended thinking → Task 5 (thinking deltas return `vec![]`). The **config gate is implemented** as `AnthropicRequest.thinking: None` in `request::build` (Task 4 line 826) — when `None`, the API never emits thinking blocks, so the parse-side drop is belt-and-suspenders. The gate is NOT a parse-side `if`; it's the request-side `None`. Replay across compaction is the only deferred part (§10).
- §7.3 `anthropic-beta` headers → Task 7 (`with_betas`, `beta_header_value`, `request_headers` + 3 unit tests).
- §7.4 stays-as-is → `fetch_model_info` inherits the trait default `Ok(None)` (NOT overridden — documented in Task 7 Interfaces); idle timeout, `http_client`, `list_models`, `parse_anthropic_models` unchanged.
- §8 registry cleanup + caps → Task 8.
- §9 testing → Tasks 1–8 each include their tests; Task 9 is workspace verification. **Deferred:** spec §9 Layer 2 "one new byte-fixture integration test" — noted in Task 9 Step 1 as a follow-up.

**2. Placeholder scan:** None. Every step has complete code. The previous draft's `/* ... send + timeout ... */` and `/* surface Error */` placeholders in the self-review snippet were replaced by a complete `stream_with_retries` impl in the Task 7 code block (no placeholders, no "see corrected snippet" indirection).

**3. Type consistency:**
- `tool_use_id` field name in `ContentBlock::ToolResult` (Task 1) matches `request.rs` (Task 4) and the spec §5. Anthropic's wire field is `tool_use_id`; zoid's `Message` field is `tool_call_id` (opencode-go prerequisite). Task 4 maps `m.tool_call_id → ContentBlock::ToolResult.tool_use_id` with the spec's `tool_call_id → tool_name → empty` fallback chain.
- `ToolUseAccumulator` defined in Task 5, used in Tasks 5, 6, 7.
- `StreamEvent`/`ContentBlockStart`/`Delta` from Task 2 used in Tasks 5, 6.
- `AnthropicRequest`/`AnthropicMessage`/`MessageContent` from Task 1 used in Tasks 3, 4.
- `place_breakpoints` from Task 3 used in Task 4.
- `parse_anthropic_models` lives inline in `mod.rs` (Task 7) — NOT re-exported from `parse.rs`. The earlier draft's `pub use parse::parse_anthropic_models;` line was a bug (re-exporting a function that doesn't exist in `parse.rs`); it's deleted in the corrected Task 7 code block.

**4. Retry design (gilfoyle C2 fix):** Task 7 uses `stream_with_retries(req, sink, attempt)` where `attempt` strictly increments (0 → 1 → 2 → 3) and the `attempt < MAX_RETRIES` check fires *before* recursing. The earlier draft's `Box::pin(self.stream(req, sink)).await` recursion reset `attempt` to 0 each call — an infinite 429 loop. The corrected version threads `attempt` as a parameter; tail-recursion at depth ≤ 3 does not need `Box::pin` (the Task 7 code block notes this). The two retry tests (`retry_on_429_then_succeeds`, `retry_exhausted_surfaces_error`) exercise both the success-after-retry and exhaustion paths.

**5. `tool_name` fallback (gilfoyle C4 fix):** Task 4's `MsgRole::Tool` arm uses `m.tool_call_id.clone().or_else(|| m.tool_name.clone()).unwrap_or_default()` per spec §5. The earlier draft's `unwrap_or_default()` dropped the `tool_name` fallback and asserted empty-string for Ollama — that contradicted the spec and gave Anthropic nothing to correlate. The corrected version uses `tool_name` as the fallback, and the test `tool_message_without_call_id_falls_back_to_tool_name` asserts `"read_file"` (not `""`). A new test `tool_call_id_wins_over_tool_name` verifies `tool_call_id` takes precedence when both are set.

**6. No-panic headers (gilfoyle I1 fix):** Task 7's `request_headers` uses `if let Ok(v) = ...parse()` for all four headers — never `.unwrap()`. An API key or beta value with a stray byte surfaces as a skipped header, not a panic, matching the "never panic on malformed input" constraint.

**7. Prerequisite enforcement (gilfoyle I4 fix):** Task 0 (`git merge-base --is-ancestor 5d18e30 HEAD` + `rg tool_call_id lib.rs`) fails fast if the opencode-go prerequisite isn't merged. A subagent starting on `main` without the merge hits Task 0 before Task 4's `m.tool_call_id` compile failure.

**8. TDD honesty (gilfoyle I3 fix):** The Global Constraint now documents the two-mode discipline honestly: strict red-green for I/O-bound code (Task 7's stream loop), impl+tests-together for pure functions (Tasks 3–6), with the test run as the green check. The earlier draft claimed "every task is red → green" while writing impl+tests together in 4 of 9 tasks — that dishonesty is removed.