# zoid — Anthropic Provider Flesh-Out (Tool-Use + Hardening) · Design

**Date:** 2026-07-05
**Status:** Approved design (all 7 sections user-approved), ready for implementation plan
**Slice:** Anthropic provider parity + hardening — promotes `crates/zoid-provider/src/anthropic.rs` to a typed internal submodule, wires tool-use to match Ollama's capability, and removes the two dead registry rows the spike falsified.
**Author:** strvmarv (with Claude)

> **Spike context.** The `anthropic-cli` (Architecture A — Claude Code as agent) and `anthropic-sdk` (Architecture B' — Claude Code as streaming inference endpoint) registry rows were falsified by `spikes/cc-infer/RESULTS.md`. `claude -p` is an agent, not an inference endpoint: it never returns `tool_use` blocks to the caller for execution. This spec removes those rows and instead fleshes out the one Anthropic transport that actually works — the existing `anthropic-api` HTTP provider — which today has a known capability gap (the "capability lie" at `zoid-model/src/lib.rs:113`).

---

## 1. Overview

The existing `AnthropicProvider` (`crates/zoid-provider/src/anthropic.rs`, ~650 lines) is **text-only**. It cannot send a `tools` array, cannot parse `tool_use` content blocks, and maps `MsgRole::Tool` to a plain `user` message with string content (`anthropic.rs:17-23`). The model registry advertises this honestly — `MODEL_CAPS` has `tools: false` for both Claude models with a comment instructing the flip when tool-use lands.

### Prerequisite (already on `feature/opencode-go-provider`)

This slice assumes `Message.tool_call_id: Option<String>` + the `Message::tool_with_call_id(name, call_id, content)` constructor + the agent-loop `map_msg` threading are merged to `main` first. They land on the `feature/opencode-go-provider` branch (commits `20c4048`, `5d18e30`) for OpenAI-compat providers but are provider-agnostic. Without that prerequisite, Task 1 of the implementation plan would re-widen `Message` here; with it merged, Task 1 just *consumes* `m.tool_call_id` in the Anthropic request body. The plan rebases onto `feature/opencode-go-provider` (or its merge to main) before starting.

The Ollama provider (`ollama.rs`) is the reference: it sends tools, parses `tool_calls`, maps tool results, and reports `tools: true`. zoid's agent loop (`crates/zoid/src/agent.rs:401`) is built and waiting — it pushes `ProviderEvent::ToolCall` to `pending` and executes after the stream ends. Ollama feeds it; Anthropic doesn't.

This spec closes that gap by promoting the Anthropic wire format to a **typed internal submodule** (serde structs, no external dependency) and wiring tool-use parity, plus three hardening items the typed form makes tractable: connect-phase 429 retry, `anthropic-beta` header plumbing, and correct (if discarded) extended-thinking parsing.

The `Provider` trait contract (`zoid-provider/src/lib.rs:162`) is untouched. `AnthropicProvider` still emits the same `ProviderEvent` variants into the same `mpsc::Sender`. The agent loop, Ollama provider, and seam decoupling from `zoid-core` all stay as-is.

---

## 2. North star

**The wire format should be code, not stringly-typed JSON.** Today every Anthropic feature (tool-use, thinking, beta headers, cache TTL variants) is a manual `json!` blob edit and a `match` arm on `serde_json::Value`. The spike's lesson was that Anthropic moves fast and zoid shouldn't hand-maintain the wire. Typed serde structs make the wire format greppable, compile-checked, and maintainable — without taking on an external community crate (supply-chain risk the spike called out) or redefining what zoid is (B' would have).

Design rule for this slice: **parity with Ollama's tool-calling capability, typed wire, no new surface area beyond the Provider seam.** Where a capability isn't needed yet (thinking replay across compaction, 5m cache TTL), we build the typed seam and defer the behavior.

---

## 3. Architecture & module layout

`crates/zoid-provider/src/anthropic.rs` (single file) becomes `crates/zoid-provider/src/anthropic/` (submodule directory):

```
crates/zoid-provider/src/anthropic/
  mod.rs          — AnthropicProvider, Provider impl, stream loop, 429 retry,
                    list_models, fetch_model_info, ToolUseAccumulator ownership
  types.rs        — typed request/response: AnthropicRequest, ContentBlock
                    (Text/ToolUse/ToolResult/Thinking), ToolDef, ToolUse, ToolResult,
                    ThinkingBlock, CacheControl, StreamEvent (tagged enum), Delta,
                    Usage breakdown
  request.rs     — CompletionRequest -> AnthropicRequest translation (system block,
                    cache_control, messages, tools array, tool_calls replay,
                    tool_result mapping)
  parse.rs       — SSE event -> ProviderEvent: content_block_start (text/tool_use),
                    content_block_delta (text_delta/input_json_delta/thinking_delta),
                    content_block_stop (finalize tool_use -> ToolCall), message_start,
                    message_delta, message_stop, error
  cache.rs       — cache_control breakpoint placement (extracted from current
                   request_body lines 42-58), operates on typed blocks
```

**What stays:** the `Provider` trait contract (`lib.rs:162`) is untouched — `AnthropicProvider` emits `ProviderEvent::{TextDelta, ToolCall, Usage, Truncated, Done, Error}` into the same `mpsc::Sender`. The agent loop (`crates/zoid/src/agent.rs:401`) needs no changes. Ollama is untouched. The seam stays decoupled from `zoid-core`.

**What dies:** the `anthropic-cli` and `anthropic-sdk` rows in `crates/zoid-model/src/lib.rs:85-102` are removed from `PROVIDERS`. The `Transport::Cli` and `Transport::Sdk` enum variants stay (harmless, reserved for future use) but the registry entries go. `canonical_id`, `selectable()`, `models_for()` shrink automatically. The `[planned]` entries disappear from the model picker.

**Why submodule, not one file:** the typed structs alone are ~200 lines; keeping them in `mod.rs` with the provider impl would push past 1000 lines in one file. The spike's lesson was maintainability — split files make the wire format greppable and testable in isolation.

---

## 4. Typed wire format (`types.rs`)

The Messages API becomes typed serde structs that round-trip to the wire, replacing the hand-built `json!` blobs.

### Request types

```rust
#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    kind: CacheKind,  // "ephemeral" (1h) | "ephemeral_5m"
}

#[derive(Serialize)]
pub struct AnthropicMessage {
    role: AnthropicRole,
    content: MessageContent,
}

pub enum AnthropicRole { User, Assistant }

pub enum MessageContent {
    Text(String),            // plain string shorthand: "content": "hi"
    Blocks(Vec<ContentBlock>), // block array: "content": [{"type":"text",...}]
}

#[derive(Serialize)]
pub struct ToolDef {
    name: String,
    description: String,
    input_schema: Value,  // Anthropic's field name (not "parameters")
}

#[derive(Serialize)]
pub struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    stream: bool,  // always true
    #[serde(skip_serializing_if = "Vec::is_empty")]
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
    // anthropic-beta handled as headers, not body
}
```

### Response types (SSE parsing)

```rust
#[derive(Deserialize)]
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

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}
```

### Why this shape

- **`ContentBlock` is the union** Anthropic's API uses for everything: text, tool_use, tool_result, and thinking are all variants. This single enum is the heart of the hardening — today the code treats these as ad-hoc `json!` shapes, and each one would be a separate manual edit. With the enum, they're arms.
- **`MessageContent` dual form** — Anthropic accepts a plain string `"content": "hi"` OR a block array. The enum models both; the request builder decides which to emit (plain for interior messages, block array when cache_control or tool_use is involved). This preserves the current optimization (`anthropic.rs:42-47`) where only the last message gets the cache breakpoint.
- **`skip_serializing_if` everywhere** — the wire stays minimal. Empty tools array isn't sent. No thinking config isn't sent. This is what the `json!` macro did implicitly; now it's explicit and compile-checked.
- **`CacheKind` enum** — today the code hardcodes `"ephemeral"` (1h). Anthropic also offers `"ephemeral_5m"`. The typed form makes the 5m vs 1h choice a configuration knob later, not a code change.
- **`StreamEvent` tagged enum** — replaces the stringly-typed `event_type` match in `parse_one` (`anthropic.rs:85`). Every SSE frame deserializes to a typed variant or is ignored. `Ping` and unknown variants fall through to `None`.

### What this replaces in the current code

| Current (`anthropic.rs`) | New |
|---|---|
| `request_body()` building `json!` blob (lines 12-60) | `request::build(req) -> AnthropicRequest` using typed structs |
| `parse_one()` stringly-typed match (lines 84-134) | `parse::event(StreamEvent, &mut ToolUseAccumulator) -> Vec<ProviderEvent>` |
| Inline `cache_control` json (lines 43-47, 56-58) | `cache::place_breakpoints(&mut AnthropicRequest)` operating on typed blocks |
| `MsgRole::Tool` mapped to `"user"` with plain text (line 25) | `ContentBlock::ToolResult { tool_use_id, content }` |

---

## 5. Request building (`request.rs` + `cache.rs`)

Translates zoid's `CompletionRequest` → `AnthropicRequest`. The mapping in `request_body()` (`anthropic.rs:12-60`) splits into typed construction.

### `request::build(req: &CompletionRequest) -> AnthropicRequest`

```rust
pub fn build(req: &CompletionRequest) -> AnthropicRequest {
    let messages: Vec<AnthropicMessage> = req.messages.iter().map(|m| match m.role {
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
        },
        MsgRole::Tool => AnthropicMessage {
            role: AnthropicRole::User,  // tool_result rides in a user message
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                    content: m.content.clone(),
                    is_error: None,
                },
            ]),
        },
    }).collect();

    let mut request = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        stream: true,
        messages,
        system: req.system.as_ref().map(|s| vec![SystemBlock {
            text: s.clone(),
            cache_control: None,
        }]),
        tools: req.tools.iter().map(|t| ToolDef {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.parameters.clone(),
        }).collect(),
        thinking: None,  // opt-in via config/flag, not this slice
    };

    cache::place_breakpoints(&mut request);
    request
}
```

### The `tool_use_id` threading (shared `Message` widening)

Anthropic's `tool_result` block requires a `tool_use_id` that matches a `tool_use` block's `id` from a prior assistant turn. But zoid's `Message::tool(name, content)` constructor (`lib.rs:90`) carries only `tool_name`, not an id. And `ToolCall.id` (`lib.rs:128`) is documented as "empty for providers that don't issue call ids" — Ollama doesn't issue them, Anthropic does.

**Decision: consume the existing `Message.tool_call_id` field.** zoid's `Message` already carries `tool_call_id: Option<String>` (alongside `tool_name`), added on the `feature/opencode-go-provider` branch (commits `20c4048`, `5d18e30`) for OpenAI-compat providers. The agent loop's `map_msg` already populates it from `ChatMsg::ToolResult.id` via `Message::tool_with_call_id(name, call_id, content)`. This slice assumes that branch's `Message` widening is merged first (or rebased onto it).

Anthropic's `tool_result` block requires a `tool_use_id` that matches a `tool_use` block's `id` from a prior assistant turn. The agent loop (`agent.rs:416`) already has the `ToolCall` with its `id` from the provider stream; `map_msg` already threads it into `Message.tool_call_id`. So `request::build` reads `m.tool_call_id` for the `ContentBlock::ToolResult { tool_use_id, content }` field, falling back to `m.tool_name` (then empty string) when `tool_call_id` is `None` — so Ollama tool results (which set `tool_name` but not `tool_call_id`) still serialize correctly, just without a matching id.

**Why not synthesize ids:** Anthropic requires the `tool_use_id` to *match* a prior `tool_use` block's `id`, not to be globally unique. We could synthesize a deterministic id from tool_name + position when replaying. But this only works if the *same* synthesized id appears in both the assistant `tool_use` block AND the `tool_result` block — meaning both must derive from the same source. This is fragile across compaction (where messages get reconstructed from summaries, losing position). The real id threads cleanly and survives compaction because the id travels with the `Message` (already proven on the opencode-go branch).

**Ollama compatibility:** Ollama doesn't issue call ids — `ToolCall.id` is an empty string, so `ChatMsg::ToolResult.id` is empty, so `tool_call_id` is empty/`None`. Ollama's request builder (`ollama.rs:38-43`) ignores `tool_call_id` and emits `tool_name`. Both providers benefit: Anthropic gets correct replay, Ollama is unaffected.

### `cache.rs::place_breakpoints(&mut AnthropicRequest)`

Extracts the current logic (`anthropic.rs:42-58`) into a function operating on typed blocks:
- System block gets `cache_control: Some(Ephemeral1h)` (if present).
- Last message's last block gets `cache_control: Some(Ephemeral1h)`.
- Interior messages stay plain.
- Preserves the rolling-breakpoint behavior: previous turn's breakpoint becomes an interior read, new breakpoint extends the cached prefix.

The typed form makes a future knob (1h vs 5m TTL, max 4 breakpoints) a config field, not a code change.

---

## 6. SSE parsing (`parse.rs`)

Replaces `parse_one()` (`anthropic.rs:84-134`) with typed event handling.

### `parse::event(frame: StreamEvent, acc: &mut ToolUseAccumulator) -> Vec<ProviderEvent>`

Each SSE frame deserializes into `StreamEvent` (the tagged enum from §4) and maps to zero-or-more `ProviderEvent`s. Unhandled variants and malformed JSON fall through to `[]` — never panic, matching current behavior.

### The arms

```rust
match frame {
    StreamEvent::MessageStart { message } => {
        // Fold cache_read + cache_creation into input (matches current anthropic.rs:107)
        let input = usage.input_tokens + cache_read + cache_creation;
        vec![ProviderEvent::Usage(Usage {
            input_tokens: input,
            output_tokens: 0,
            cached: cache_read,
        })]
    }
    StreamEvent::ContentBlockStart { index, content_block } => match content_block {
        ContentBlockStart::Text => vec![],  // nothing until deltas arrive
        ContentBlockStart::ToolUse { id, name } => {
            // Stash (index, id, name, empty partial_json buffer) in acc.
            // No ProviderEvent yet — args arrive as deltas.
            vec![]
        }
        ContentBlockStart::Thinking => vec![],
    }
    StreamEvent::ContentBlockDelta { index, delta } => match delta {
        Delta::TextDelta { text } => vec![ProviderEvent::TextDelta(text)],
        Delta::InputJsonDelta { partial_json } => {
            // Append partial_json to acc slot for `index`.
            // No ProviderEvent — the ToolCall fires on content_block_stop.
            vec![]
        }
        Delta::ThinkingDelta { .. } | Delta::SignatureDelta { .. } => vec![],
    }
    StreamEvent::ContentBlockStop { index } => {
        // If `index` corresponds to an accumulating ToolUse, finalize:
        // parse the accumulated partial_json into a Value (fall back to {} on
        // garbage, mirroring ollama.rs:157 coerce_tool_args), emit
        // ToolCall(ToolCall { id, name, args }), clear the acc slot.
        // Text/Thinking blocks emit nothing.
        vec![/* ToolCall if this was a tool_use block */]
    }
    StreamEvent::MessageDelta { delta, usage } => {
        let mut out = vec![ProviderEvent::Usage(Usage {
            input_tokens: 0,
            output_tokens: usage.output_tokens,
            cached: 0,
        })];
        if delta.stop_reason == "max_tokens" {
            out.push(ProviderEvent::Truncated);
        }
        out
    }
    StreamEvent::MessageStop => vec![ProviderEvent::Done],
    StreamEvent::Error { error } => vec![ProviderEvent::Error(error.message)],
    StreamEvent::Ping => vec![],
}
```

### The tool-use accumulator

`parse::event` is stateless per frame, but tool-use spans multiple frames: `content_block_start` (id + name) → N×`input_json_delta` (partial args) → `content_block_stop` (finalize). So the parse module needs a small accumulator, owned by the `Provider::stream` loop in `mod.rs`:

```rust
#[derive(Default)]
pub struct ToolUseAccumulator {
    slots: HashMap<u32, (String /*id*/, String /*name*/, String /*partial_json*/)>,
}
```

The `stream` loop in `mod.rs` owns the accumulator, passes each frame to `parse::event(frame, &mut acc)`, and emits the returned `ProviderEvent`s into the sink. This keeps `parse.rs` testable in isolation (pass an accumulator, check the returned events) while the stream loop owns the lifecycle.

### What this gives over the current code

| Current (`anthropic.rs:84-134`) | New |
|---|---|
| `parse_one` matches on `event_type: &str` | `StreamEvent` tagged enum — compile-checked, no typos |
| `content_block_start` returns `None` (`anthropic.rs:497`) | Tool-use arms accumulate id/name |
| `content_block_delta` handles `text_delta` only | Also handles `input_json_delta` for tool args |
| `content_block_stop` returns `None` | Finalizes tool-use → `ToolCall` |
| `message_delta` checks `stop_reason:"max_tokens"` by string | `delta.stop_reason` typed comparison |

The accumulator is the one piece of stateful logic; everything else is pure mapping. The `parse` functions stay unit-testable with canned SSE frames — same pattern as the current `parses_text_delta`/`parses_message_stop_as_done` tests, just more arms.

---

## 7. Hardening (`mod.rs`)

### 7.1 Connect-phase 429 / overload retry

Today the stream loop surfaces HTTP 429 as a single `ProviderEvent::Error` (`anthropic.rs:248-251`) and the turn dies. Anthropic's overload responses carries `retry-after` (seconds); the recommended backoff is exponential with jitter.

Add a bounded retry loop around the *initial* `send()` (not mid-stream — a partial stream that 429s is rare and resuming is complex; YAGNI):

```rust
const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);
```

- On 429, read `retry-after` header (fall back to `BASE_BACKOFF * 2^attempt`), sleep with jitter, retry.
- On other non-2xx, surface `Error` immediately (no retry).
- Mid-stream errors (overloaded_error in SSE) surface as `Error` and end the turn — the next turn's request retries naturally.

Bounded, logged, only on connect-phase 429. Mid-stream overload stays a terminal `Error` (the next user turn retries). This matches Anthropic's documented guidance without inventing resume semantics.

### 7.2 Extended thinking blocks

Claude's extended thinking arrives as `content_block_start` (type `thinking`) + `thinking_delta` deltas + `signature_delta` + `content_block_stop`. The typed `Delta` enum has these arms (§4). The parse mapping (§6) currently drops them (`vec![]`).

**This slice: discard thinking deltas** (drop on the floor, don't emit). zoid's `EventKind` (`zoid-core/src/event.rs`) has no `Thinking` variant; adding one is a `zoid-core` change beyond this slice. The signature is parsed (typed) but not retained for replay.

**Replay limitation (deferred):** Anthropic requires the `signature` field to be replayed for multi-turn thinking. When an assistant turn that produced thinking is replayed (next turn's request), the `ThinkingBlock` must include the original signature. The typed `ContentBlock::Thinking` carries it (§4), but zoid's `Message` doesn't carry thinking content today, so replay is impossible until `Message` widens. **This slice: don't replay thinking.** Config-gate thinking off by default; when enabled, each turn's thinking is ephemeral and not replayed. The limitation is documented in code.

Net for this slice: thinking is *parsed* correctly (typed, doesn't crash on unknown variants) but *discarded* from the event stream. Config-gated off by default. Replay is a deferred follow-up that widens `Message` — explicitly out of scope here.

### 7.3 `anthropic-beta` header plumbing

Anthropic ships features behind beta headers (`extended-thinking-2025-05-14`, `context-management-2025-10-02`, `fine-grained-tool-streaming-2025-05-14`, etc.). Today the stream loop sends a single hardcoded `anthropic-version: 2023-06-01` (`anthropic.rs:213`).

Add:
- A `Vec<String>` of beta flags on `AnthropicProvider` (default empty).
- Each flag sent as an `anthropic-beta: flag1,flag2` header (comma-joined).
- Populated from config (`[provider.anthropic] betas = [...]`) or env (`ZOID_ANTHROPIC_BETAS=comma,separated`).

This makes beta features opt-in without code changes. The typed request/response structs already handle the body side (e.g. `thinking: Some(ThinkingConfig)` in `AnthropicRequest`); the header is the complementary plumbing.

### 7.4 What stays as-is

- The `idle_timeout` / `stream_idle_timeout()` machinery (`anthropic.rs:163-175`, `lib.rs:38-45`) — already hardened, no change.
- The `http_client()` connect timeout (`lib.rs:49-54`) — already correct.
- `list_models` / `parse_anthropic_models` (`anthropic.rs:137-147`, 308-317) — already work, re-homed into the submodule.
- `fetch_model_info` — Anthropic's `/v1/models` doesn't expose capabilities, so this stays `None` (the static `MODEL_CAPS` registry is the fallback). No change.

---

## 8. Registry cleanup & model caps (`zoid-model/src/lib.rs`)

### Registry row removal

**Remove** the two `anthropic-cli` and `anthropic-sdk` entries (lines 85-102). The `PROVIDERS` array shrinks from 5 entries to 3: `ollama-local`, `ollama-cloud`, `anthropic-api`. The `[planned]` rows disappear from the model picker; `selectable()` and `models_for()` shrink automatically.

**Keep** the `Transport::Cli` and `Transport::Sdk` enum variants (lines 30-31). They're harmless, reserved for future use, and removing them would churn the enum for no gain. The variants just become uninhabited by any entry — a future `anthropic-cli` (Architecture A, if ever pursued) reuses `Cli`.

**Update** the tests that reference the removed rows: tests asserting `anthropic-cli`/`anthropic-sdk` entries exist, `default_base_url("anthropic-cli") == None`, `default_base_url("anthropic-sdk") == None`, and `!ids.contains(&"anthropic-cli")` / `!ids.contains(&"anthropic-sdk")` (lines 280-316) get deleted with the rows. The `anthropic-api` tests stay and become the sole Anthropic-row assertions.

### Capability flip (`MODEL_CAPS`)

Flip `tools: false → true` for both Claude models (lines 113-134). The comments "Anthropic tool-use is not wired yet... Flip to true when the tool_use/tool_result wire mapping lands" get replaced with a one-liner noting tool-use is wired. The `prompt_cache: true` stays. This unblocks the agent's tool-calling path for Claude — the picker no longer hides tools, and the agent's `tools: Vec<ToolSpec>` reaches the wire.

### What this section doesn't touch

- `canonical_id()` (line 168-173) — `anthropic → anthropic-api` alias stays (legacy ids still resolve).
- `default_base_url("anthropic-api")` — unchanged, still `https://api.anthropic.com`.
- `model_info()` lookup — unchanged shape, just the `tools` field value flips.
- `selectable()` / `entry()` / `models_for()` — work automatically on the smaller array.

---

## 9. Testing

The typed submodule makes testing cleaner than the current stringly-typed shape. Three layers, mirroring zoid's existing conventions.

### Layer 1 — Unit tests (per submodule, `#[cfg(test)] mod tests`)

Same pattern as the current `anthropic.rs:320-646` and `ollama.rs:423-877` — pure functions, no I/O.

**`types.rs` tests:**
- `AnthropicRequest` serializes to the expected Messages-API JSON (round-trip via `serde_json::to_value` + assert against `json!`). Confirms `skip_serializing_if` drops empty tools/system/thinking.
- `ContentBlock` round-trips: `ToolUse { id, name, input }` ↔ JSON, `ToolResult { tool_use_id, content }` ↔ JSON, `Thinking` ↔ JSON.
- `StreamEvent` deserializes each SSE frame type: `content_block_start` (text + tool_use variants), `content_block_delta` (text/input_json/thinking deltas), `message_start` (with + without cache tokens), `message_delta`, `message_stop`, `error`, `ping`, and an unknown type → falls through (no panic).

**`request.rs` tests:**
- `build()` on a plain user/assistant conversation → matches the current `builds_messages_body_with_stream_flag` assertion (interior messages stay plain strings, system becomes a block).
- `build()` with a `Message::assistant` carrying `tool_calls` → emits `ContentBlock::ToolUse` blocks in the replayed assistant message.
- **`build()` with a `Message::tool(...)` (no call id) or `Message::tool_with_call_id(...)` → emits `ContentBlock::ToolResult { tool_use_id, content }` in a `user` message; `tool_use_id` comes from `m.tool_call_id` (falling back to `m.tool_name`).
- `build()` with `tools` → emits the `tools` array with `input_schema` (not `parameters` — Anthropic's field name).
- `build()` with empty tools → no `tools` key in the serialized request.

**`cache.rs` tests:**
- `place_breakpoints` on a system + multi-message request → system block gets `Ephemeral1h`, last message's last block gets `Ephemeral1h`, interior blocks get `None`.
- The rolling-breakpoint behavior preserved from current `caches_only_the_last_message` test.

**`parse.rs` tests:**
- Each `StreamEvent` arm maps to the expected `Vec<ProviderEvent>` — port the existing `parses_text_delta`, `parses_message_stop_as_done`, `parses_message_delta_usage`, `parses_message_start_input_usage`, `message_start_folds_cache_tokens_into_input_and_reports_cached`, `parses_error`, `ignores_unhandled_frames`, `malformed_data_yields_empty_not_panic`, `message_delta_with_max_tokens_stop_yields_usage_then_truncated` tests to the typed form.
- **New tool-use accumulation tests:** feed a canned `content_block_start` (tool_use) → 2× `input_json_delta` → `content_block_stop` sequence through `parse::event` with a `ToolUseAccumulator`, assert the final `ToolCall { id, name, args }` with parsed args.
- **New tool-args coercion tests:** mirror `ollama.rs` `coerce_tool_args` behavior — garbage partial_json → `{}`, valid object → parsed, empty deltas → `{}`.

### Layer 2 — Integration tests (`crates/zoid/tests/`)

The existing `agent_loop.rs:168` and `subagent_integration.rs:69` tests script a `FakeProvider` that emits `ProviderEvent::ToolCall`. They already pass today (Ollama feeds them). No new integration tests needed for the agent loop — it's provider-agnostic and doesn't care whether Anthropic or Ollama emitted the ToolCall. The typed Anthropic module just needs to *also* emit ToolCalls, exercised at layer 1.

**One new integration test:** a scripted `FakeProvider` that replays a real Anthropic SSE byte stream (captured fixture) → assert the agent loop receives the same `ProviderEvent` sequence it would from Ollama. Confirms the two providers are interchangeable on the seam. This is the "tool-use parity" acceptance test.

### Layer 3 — Timeout / retry tests (in `mod.rs`, `#[tokio::test]`)

Port the existing three stalling-server tests verbatim (`idle_timeout_emits_error_when_stream_stalls`, `request_timeout_emits_error_when_no_response`, `error_body_timeout_emits_error_when_body_stalls`). They test the stream loop, which moves to `mod.rs` unchanged in shape. Plus two new:

- `retry_on_429_then_succeeds`: a mock server that returns 429 + `retry-after: 0` on the first POST, then 200 + SSE on the second. Assert the stream retries and emits the expected events. Bounded by `MAX_RETRIES`.
- `retry_exhausted_surfaces_error`: mock returns 429 on every attempt. Assert the final `ProviderEvent::Error` mentions 429.

### What's NOT tested here

- Real Anthropic API calls (no key in CI, network-dependent). The typed wire format + canned SSE fixtures cover the contract.
- Thinking-block replay across compaction (deferred per §7.2).
- The `anthropic-beta` header plumbing beyond asserting the header is set when `betas` is non-empty (the actual beta features need real API calls to validate).

---

## 10. Out of scope (deferred)

- **Thinking replay across compaction** — requires widening `Message` to carry thinking content + signature, plus a `zoid-core` `EventKind::Thinking` variant. This slice parses thinking correctly but discards it; replay is a follow-up.
- **5m cache TTL** — `CacheKind::Ephemeral5m` is typed and ready, but no config knob exposes it yet. Future config field.
- **Mid-stream resume on 429** — only connect-phase 429 retries. Mid-stream overload is terminal.
- **`fetch_model_info` for Anthropic** — `/v1/models` doesn't expose capabilities; the static `MODEL_CAPS` registry remains the source of truth.
- **Architecture A (Claude Code as agent)** — the `anthropic-cli` row is removed but `Transport::Cli` stays. If "zoid as a TUI for Claude Code" becomes the thesis later, that's a fresh brainstorm and a different slice.
- **External Rust Anthropic crate** — the typed submodule gets the maintainability benefits without the supply-chain risk the spike called out.

---

## 11. Open questions

None. All seven design sections user-approved before writing.