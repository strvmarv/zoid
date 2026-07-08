# Reasoning / Thinking Modes — Phase 2: Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture reasoning text from providers, carry it as ephemeral in-memory events, render it as a collapsed "▶ Thinking…" marker at Normal zoom that expands to full text at Detail zoom, and add TPS + thinking mode indicator to the session drawer.

**Architecture:** Provider parse layers emit `ThinkingDelta`/`ThinkingSignature` instead of discarding. The agent loop accumulates reasoning per sub-turn, discards intermediate (tool-call) reasoning, and flushes final-answer reasoning as an ephemeral `ModelThinking` event. The projection attaches reasoning to the next `ChatMsg::Assistant`. The TUI renders a collapsed marker at Normal zoom and full dimmed text at Detail zoom. Session drawer gains TPS and a thinking mode label.

**Tech Stack:** Rust, tokio, ratatui, serde_json, SQLite (ephemeral events skip the store).

## Global Constraints

- All changes must compile with `cargo build --workspace` and pass `cargo test --workspace`.
- Run `cargo clippy --workspace` — no new warnings from this plan's changes.
- Every `CompletionRequest` construction site must include `thinking: ThinkingMode::Off` (or the appropriate variant) — this field already exists from Phase 1.
- Every `Usage` construction site must include `thinking_tokens: 0` (the new field).
- Every `TokenStat` construction site must include `thinking: 0` (the new field).
- Every `ChatMsg::Assistant` construction site must include `thinking: None` (the new field).
- Ephemeral events (`ModelThinking`) are pushed to the in-memory `EventLog` and sent via `AgentUpdate::Appended`, but `session.append()` is skipped — they never touch SQLite.
- Reasoning text is NOT sent to the provider. `map_msg` ignores the `thinking` field.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/zoid-provider/src/lib.rs` | `ProviderEvent::ThinkingDelta` + `ThinkingSignature` variants; `Usage.thinking_tokens` field |
| `crates/zoid-provider/src/anthropic/parse.rs` | Emit `ThinkingDelta` / `ThinkingSignature` from Anthropic stream events |
| `crates/zoid-provider/src/openai_compat.rs` | Emit `ThinkingDelta` from `reasoning_content` in OpenAI-compat chunks |
| `crates/zoid-provider/src/ollama.rs` | Emit `ThinkingDelta` from `message.thinking` in Ollama lines |
| `crates/zoid-core/src/event.rs` | `EventKind::ModelThinking` variant; `TokenStat.thinking` field |
| `crates/zoid-core/src/economy.rs` | `TokenLedger.thinking` field |
| `crates/zoid-core/src/projection.rs` | `ChatMsg::Assistant.thinking` field; fold `ModelThinking` into next assistant message |
| `crates/zoid/src/agent.rs` | Accumulate reasoning, discard intermediate, flush final; `emit_ephemeral` helper; `map_msg` ignores thinking |
| `crates/zoid/src/main.rs` | Per-frame TPS + thinking label computation for ShellState |
| `crates/zoid-tui/src/state.rs` | `ShellState.tps` + `ShellState.thinking_label` fields |
| `crates/zoid-tui/src/render.rs` | Session drawer: thinking label on dur line, tok/cac/tps on new line |
| `crates/zoid-tui/src/chat.rs` | "▶ Thinking…" marker at Normal zoom; full dimmed text at Detail zoom |

---

### Task 1: ProviderEvent + Usage extensions

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs`

**Interfaces:**
- Produces: `ProviderEvent::ThinkingDelta(String)`, `ProviderEvent::ThinkingSignature(String)`, `Usage.thinking_tokens: u64`

- [ ] **Step 1: Add ThinkingDelta + ThinkingSignature to ProviderEvent**

In `crates/zoid-provider/src/lib.rs`, find the `ProviderEvent` enum (around line 159) and add the two new variants after `TextDelta`:

```rust
pub enum ProviderEvent {
    TextDelta(String),
    /// Reasoning/thinking text from the model (Anthropic thinking blocks,
    /// DeepSeek `reasoning_content`, Ollama `message.thinking`). Accumulated
    /// by the agent loop and rendered as a collapsible "▶ Thinking…" marker.
    ThinkingDelta(String),
    /// Anthropic thinking-block signature (for future replay). Emitted at the
    /// end of each thinking block. Other providers never emit this.
    ThinkingSignature(String),
    ToolCall(ToolCall),
    // ... rest unchanged
```

- [ ] **Step 2: Add thinking_tokens to Usage**

In the same file, find the `Usage` struct (around line 127) and add the field:

```rust
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached: u64,
    /// Reasoning/thinking token count (Anthropic only, if reported separately).
    /// 0 for providers that don't break out thinking tokens (DeepSeek bundles
    /// them into `output_tokens`; Ollama has no token-level breakdown).
    pub thinking_tokens: u64,
}
```

- [ ] **Step 3: Fix all Usage construction sites**

Search for every `Usage {` construction in the workspace and add `thinking_tokens: 0`. The main sites are:

- `crates/zoid-provider/src/anthropic/parse.rs` (two sites: `MessageStart` and `MessageDelta`)
- `crates/zoid-provider/src/openai_compat.rs` (usage extraction in `parse_chunk`)
- Any test helpers that construct `Usage`

Run: `grep -rn "Usage {" crates/ --include="*.rs"`
Add `thinking_tokens: 0,` to each.

- [ ] **Step 4: Build and test**

Run: `cargo test --workspace`
Expected: PASS (all existing tests; the new fields default to 0 and don't change behavior)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add ThinkingDelta/ThinkingSignature to ProviderEvent + thinking_tokens to Usage"
```

---

### Task 2: EventKind::ModelThinking + TokenStat.thinking

**Files:**
- Modify: `crates/zoid-core/src/event.rs`
- Modify: `crates/zoid-core/src/economy.rs`

**Interfaces:**
- Consumes: `Usage.thinking_tokens` from Task 1
- Produces: `EventKind::ModelThinking { text: String }`, `TokenStat.thinking: u64`, `TokenLedger.thinking: u64`

- [ ] **Step 1: Add ModelThinking to EventKind**

In `crates/zoid-core/src/event.rs`, find the `EventKind` enum and add the new variant after `ModelDelta`:

```rust
pub enum EventKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ModelDelta { text: String },
    /// Reasoning/thinking text from the model. Ephemeral — pushed to the
    /// in-memory `EventLog` but NOT persisted to SQLite (skipped by
    /// `emit_ephemeral`). Only the final sub-turn's reasoning is kept;
    /// intermediate reasoning from tool-selection sub-turns is discarded.
    /// The projection attaches this to the next `ChatMsg::Assistant`.
    ModelThinking { text: String },
    ToolCall { id: String, name: String, args: String },
    // ... rest unchanged
```

- [ ] **Step 2: Add thinking to TokenStat**

In the same file, find `TokenStat` (around line 14) and add the field:

```rust
pub struct TokenStat {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    /// Reasoning/thinking token count (from `Usage.thinking_tokens`).
    pub thinking: u64,
}
```

- [ ] **Step 3: Fix all TokenStat construction sites**

Run: `grep -rn "TokenStat {" crates/ --include="*.rs"`
Add `thinking: 0,` to each construction site. The main sites are:
- `crates/zoid/src/agent.rs` (the `turn_usage` init and test helpers)
- Test files that construct `TokenStat`

- [ ] **Step 4: Add thinking to TokenLedger and token_ledger**

In `crates/zoid-core/src/economy.rs`, add the field to `TokenLedger`:

```rust
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub total: u64,
    /// Cumulative thinking/reasoning tokens across all turns.
    pub thinking: u64,
}
```

Update `token_ledger` to sum it:

```rust
pub fn token_ledger<'a>(events: impl IntoIterator<Item = &'a Event>) -> TokenLedger {
    let mut l = TokenLedger::default();
    for e in events {
        if let Some(t) = e.tokens {
            l.input += t.input;
            l.output += t.output;
            l.cached += t.cached;
            l.thinking += t.thinking;
        }
    }
    l.total = l.input + l.output;
    l
}
```

- [ ] **Step 5: Build and test**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add ModelThinking event + thinking to TokenStat/TokenLedger"
```

---

### Task 3: Anthropic parse — emit ThinkingDelta + ThinkingSignature

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/parse.rs`

**Interfaces:**
- Consumes: `ProviderEvent::ThinkingDelta` + `ThinkingSignature` from Task 1
- Produces: `event()` now emits `ThinkingDelta`/`ThinkingSignature` instead of `vec![]`

- [ ] **Step 1: Write failing test for ThinkingDelta**

In `crates/zoid-provider/src/anthropic/parse.rs`, find the test `thinking_delta_emits_nothing` (around line 253) and replace it:

```rust
#[test]
fn thinking_delta_emits_thinking_delta() {
    let frame = StreamEvent::ContentBlockDelta {
        index: 0,
        delta: Delta::ThinkingDelta {
            thinking: "reasoning".into(),
        },
    };
    let mut acc = ToolUseAccumulator::default();
    let events = event(frame, &mut acc);
    assert_eq!(
        events,
        vec![ProviderEvent::ThinkingDelta("reasoning".into())]
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider thinking_delta_emits`
Expected: FAIL — `event()` returns `vec![]`, not `vec![ThinkingDelta]`

- [ ] **Step 3: Implement — emit ThinkingDelta + ThinkingSignature**

In `crates/zoid-provider/src/anthropic/parse.rs`, find the line that discards both (around line 84):

```rust
            Delta::ThinkingDelta { .. } | Delta::SignatureDelta { .. } => vec![],
```

Replace with:

```rust
            Delta::ThinkingDelta { thinking } => {
                vec![ProviderEvent::ThinkingDelta(thinking)]
            }
            Delta::SignatureDelta { signature } => {
                vec![ProviderEvent::ThinkingSignature(signature)]
            }
```

- [ ] **Step 4: Write failing test for SignatureDelta**

Find the test `signature_delta_emits_nothing` (around line 493) and replace it:

```rust
#[test]
fn signature_delta_emits_thinking_signature() {
    let frame = StreamEvent::ContentBlockDelta {
        index: 0,
        delta: Delta::SignatureDelta {
            signature: "sig".into(),
        },
    };
    let mut acc = ToolUseAccumulator::default();
    let events = event(frame, &mut acc);
    assert_eq!(
        events,
        vec![ProviderEvent::ThinkingSignature("sig".into())]
    );
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-provider anthropic::parse`
Expected: PASS — both new tests pass

- [ ] **Step 6: Add thinking_tokens to Anthropic usage events**

In `crates/zoid-provider/src/anthropic/parse.rs`, the `MessageStart` and `MessageDelta` events construct `Usage`. Find each `Usage {` and add `thinking_tokens: 0,`. (If Anthropic reports thinking tokens separately in the API response, extract them here — verify during implementation. For now, default to 0.)

- [ ] **Step 7: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(anthropic): emit ThinkingDelta + ThinkingSignature from stream events"
```

---

### Task 4: OpenAI-compat parse — emit ThinkingDelta from reasoning_content

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs`

**Interfaces:**
- Consumes: `ProviderEvent::ThinkingDelta` from Task 1
- Produces: `parse_chunk()` now emits `ThinkingDelta` for `reasoning_content` instead of discarding

- [ ] **Step 1: Write failing test for reasoning_content → ThinkingDelta**

In `crates/zoid-provider/src/openai_compat.rs`, find the test `parse_chunk_reasoning_content_is_discarded` (around line 856) and replace it:

```rust
#[test]
fn parse_chunk_reasoning_content_emits_thinking_delta() {
    let data = r#"{"choices":[{"delta":{"content":"answer","reasoning_content":"thinking..."}}]}"#;
    let events = parse_chunk(data, &mut ToolCallAccumulator::new());
    assert!(events.contains(&ProviderEvent::ThinkingDelta("thinking...".into())),
        "reasoning_content must emit ThinkingDelta, got: {:?}", events);
    assert!(events.contains(&ProviderEvent::TextDelta("answer".into())),
        "content must still emit TextDelta, got: {:?}", events);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider parse_chunk_reasoning_content_emits`
Expected: FAIL — `reasoning_content` is currently discarded

- [ ] **Step 3: Implement — extract reasoning_content and emit ThinkingDelta**

In `crates/zoid-provider/src/openai_compat.rs`, find `parse_chunk` (around line 190). After the `content` extraction block (which pushes `TextDelta`), add a `reasoning_content` extraction block:

```rust
    if let Some(reasoning) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("reasoning_content"))
        .and_then(|c| c.as_str())
    {
        if !reasoning.is_empty() {
            out.push(ProviderEvent::ThinkingDelta(reasoning.to_string()));
        }
    }
```

Insert this immediately after the `content` block (after the closing `}` of the `if let Some(content)` block, before the `tool_calls` block).

- [ ] **Step 4: Update the reasoning_content-alone test**

Find `parse_chunk_reasoning_content_alone_yields_nothing` (around line 867) and replace it:

```rust
#[test]
fn parse_chunk_reasoning_content_alone_emits_thinking_delta_only() {
    let data = r#"{"choices":[{"delta":{"reasoning_content":"deep thoughts"}}]}"#;
    let events = parse_chunk(data, &mut ToolCallAccumulator::new());
    assert_eq!(
        events,
        vec![ProviderEvent::ThinkingDelta("deep thoughts".into())],
        "reasoning-only delta must produce only ThinkingDelta"
    );
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-provider openai_compat`
Expected: PASS

- [ ] **Step 6: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(openai-compat): emit ThinkingDelta from reasoning_content"
```

---

### Task 5: Ollama parse — emit ThinkingDelta from message.thinking

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`

**Interfaces:**
- Consumes: `ProviderEvent::ThinkingDelta` from Task 1
- Produces: `parse_line()` now emits `ThinkingDelta` for `message.thinking` instead of discarding

- [ ] **Step 1: Write failing test for thinking → ThinkingDelta**

In `crates/zoid-provider/src/ollama.rs`, find the test that references `"thinking":"reasoning"` (search for `thinking.*reasoning`). Add a new test:

```rust
#[test]
fn parse_line_thinking_field_emits_thinking_delta() {
    let line = r#"{"message":{"role":"assistant","content":"answer","thinking":"reasoning"},"done":false}"#;
    let atomic = std::sync::atomic::AtomicU64::new(0);
    let events = parse_line(line, &atomic);
    assert!(events.contains(&ProviderEvent::ThinkingDelta("reasoning".into())),
        "thinking field must emit ThinkingDelta, got: {:?}", events);
    assert!(events.contains(&ProviderEvent::TextDelta("answer".into())),
        "content must still emit TextDelta, got: {:?}", events);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider parse_line_thinking_field`
Expected: FAIL — `thinking` field is currently ignored

- [ ] **Step 3: Implement — extract thinking and emit ThinkingDelta**

In `crates/zoid-provider/src/ollama.rs`, find `parse_line` (around line 83). After the `content` extraction block (which pushes `TextDelta`), add a `thinking` extraction block:

```rust
    if let Some(thinking) = v
        .get("message")
        .and_then(|m| m.get("thinking"))
        .and_then(|t| t.as_str())
    {
        if !thinking.is_empty() {
            out.push(ProviderEvent::ThinkingDelta(thinking.to_string()));
        }
    }
```

Insert this immediately after the `content` block, before the `tool_calls` block.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider ollama`
Expected: PASS

- [ ] **Step 5: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ollama): emit ThinkingDelta from message.thinking"
```

---

### Task 6: Projection — ChatMsg::Assistant.thinking + fold ModelThinking

**Files:**
- Modify: `crates/zoid-core/src/projection.rs`

**Interfaces:**
- Consumes: `EventKind::ModelThinking { text }` from Task 2
- Produces: `ChatMsg::Assistant { thinking: Option<String>, .. }`

- [ ] **Step 1: Add thinking field to ChatMsg::Assistant**

In `crates/zoid-core/src/projection.rs`, find the `ChatMsg` enum (around line 20) and add the field:

```rust
pub enum ChatMsg {
    User { text: String, ts: i64 },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRef>,
        ts: i64,
        /// Reasoning/thinking text from the final sub-turn (from a preceding
        /// `ModelThinking` event). `None` when the model didn't reason.
        thinking: Option<String>,
    },
    // ... rest unchanged
```

- [ ] **Step 2: Fix all ChatMsg::Assistant construction sites**

Run: `grep -rn "ChatMsg::Assistant {" crates/ --include="*.rs"`
Add `thinking: None,` to every construction site. Sites include:
- `crates/zoid-core/src/projection.rs` (the `flush` helper and the standalone assistant push)
- `crates/zoid-tui/src/chat.rs` (test fixtures)
- `crates/zoid/src/agent.rs` (test helpers, `apply_streaming`)

- [ ] **Step 3: Write failing test for ModelThinking folding**

Add a test in `crates/zoid-core/src/projection.rs` (in the test module):

```rust
#[test]
fn model_thinking_attaches_to_next_assistant_message() {
    use crate::event::{Event, EventKind};
    use ulid::Ulid;

    let events = vec![
        Event::new(Ulid::new(), None, 1, EventKind::UserMessage { text: "hi".into() }),
        Event::new(Ulid::new(), None, 2, EventKind::ModelThinking { text: "let me think...".into() }),
        Event::new(Ulid::new(), None, 3, EventKind::ModelDelta { text: "Hello".into() }),
        Event::new(Ulid::new(), None, 4, EventKind::ModelDelta { text: " world".into() }),
    ];

    let msgs = conversation(&events);
    assert_eq!(msgs.len(), 2); // user + assistant
    match &msgs[1] {
        ChatMsg::Assistant { text, thinking, .. } => {
            assert_eq!(text, "Hello world");
            assert_eq!(thinking.as_deref(), Some("let me think..."));
        }
        _ => panic!("expected Assistant"),
    }
}

#[test]
fn model_thinking_with_no_following_message_is_standalone() {
    use crate::event::{Event, EventKind};
    use ulid::Ulid;

    let events = vec![
        Event::new(Ulid::new(), None, 1, EventKind::UserMessage { text: "hi".into() }),
        Event::new(Ulid::new(), None, 2, EventKind::ModelThinking { text: "just thinking".into() }),
    ];

    let msgs = conversation(&events);
    assert_eq!(msgs.len(), 2); // user + assistant (empty text)
    match &msgs[1] {
        ChatMsg::Assistant { text, thinking, .. } => {
            assert!(text.is_empty());
            assert_eq!(thinking.as_deref(), Some("just thinking"));
        }
        _ => panic!("expected Assistant"),
    }
}

#[test]
fn assistant_without_thinking_has_none() {
    use crate::event::{Event, EventKind};
    use ulid::Ulid;

    let events = vec![
        Event::new(Ulid::new(), None, 1, EventKind::UserMessage { text: "hi".into() }),
        Event::new(Ulid::new(), None, 2, EventKind::ModelDelta { text: "Hello".into() }),
    ];

    let msgs = conversation(&events);
    match &msgs[1] {
        ChatMsg::Assistant { thinking, .. } => {
            assert!(thinking.is_none());
        }
        _ => panic!("expected Assistant"),
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p zoid-core model_thinking`
Expected: FAIL — the projection doesn't handle `ModelThinking` yet

- [ ] **Step 5: Implement — fold ModelThinking into the next assistant message**

In `crates/zoid-core/src/projection.rs`, find the `conversation` function. It uses a `flush` helper to accumulate `ModelDelta` text into a `ChatMsg::Assistant`. The approach:

1. Add a `pending_thinking: Option<String>` accumulator before the event loop.
2. Handle `EventKind::ModelThinking { text }` by setting `pending_thinking = Some(text.clone())` (or appending if already set).
3. When `flush` produces a `ChatMsg::Assistant`, pass `thinking: pending_thinking.take()` into it.
4. If `ModelThinking` is the last event (no following `ModelDelta`/`AssistantMessage`), flush a standalone `ChatMsg::Assistant` with empty text and the thinking.

The `flush` function signature needs to accept the thinking text. Find the `flush` closure/helper and modify it to take `thinking: Option<String>`:

```rust
// In the flush helper:
let thinking = pending_thinking.take();
if !text.is_empty() || calls.is_empty() {
    blank_between_turns(&mut out);
    out.push(ChatMsg::Assistant {
        text: text.clone(),
        tool_calls: calls.clone(),
        ts: turn_ts,
        thinking,
    });
}
```

Add the `ModelThinking` handler in the match:

```rust
EventKind::ModelThinking { text } => {
    flush(&mut text_buf, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
    *pending_thinking = Some(text.clone());
}
```

At the end of the loop (after iterating all events), flush any remaining `pending_thinking` as a standalone assistant message:

```rust
if let Some(thinking) = pending_thinking.take() {
    if text_buf.is_empty() && calls.is_empty() {
        blank_between_turns(&mut out);
        out.push(ChatMsg::Assistant {
            text: String::new(),
            tool_calls: Vec::new(),
            ts: turn_ts,
            thinking: Some(thinking),
        });
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core projection`
Expected: PASS

- [ ] **Step 7: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: projection folds ModelThinking into ChatMsg::Assistant.thinking"
```

---

### Task 7: Agent loop — accumulate reasoning, discard intermediate, flush final

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Interfaces:**
- Consumes: `ProviderEvent::ThinkingDelta` from Task 1, `EventKind::ModelThinking` from Task 2
- Produces: ephemeral `ModelThinking` events in the `EventLog` (not persisted)

- [ ] **Step 1: Add emit_ephemeral helper**

In `crates/zoid/src/agent.rs`, add a helper after the existing `emit` function (around line 1542). This pushes to the in-memory log and notifies the UI but skips `session.append()`:

```rust
/// Push an ephemeral event to the in-memory log + UI, skipping SQLite.
/// Used for `ModelThinking` — reasoning text that survives only for the
/// current process lifetime, not persisted across restarts.
async fn emit_ephemeral(
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind).with_session(session_id);
    ev.branch = branch.clone();
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    Ok(())
}
```

- [ ] **Step 2: Add reasoning accumulator in the stream loop**

In `crates/zoid/src/agent.rs`, find the stream loop (around line 478). Add a `thinking_buf` alongside `turn_usage` and `pending`:

```rust
let mut turn_usage = zoid_core::event::TokenStat::default();
let mut pending: Vec<ToolCall> = Vec::new();
let mut thinking_buf: String = String::new();
let mut aborted = false;
```

- [ ] **Step 3: Handle ThinkingDelta in the stream match**

In the `match pe` block (around line 490), add a handler for `ThinkingDelta` alongside `TextDelta`:

```rust
ProviderEvent::ThinkingDelta(s) => {
    thinking_buf.push_str(&s);
}
```

Do NOT emit any event here — just accumulate. The reasoning is flushed only after the sub-turn completes.

Handle `ThinkingSignature` by ignoring it for now (Phase 3 will use it):

```rust
ProviderEvent::ThinkingSignature(_) => {
    // Accumulated for future Phase 3 replay; not rendered or persisted.
}
```

- [ ] **Step 4: Add thinking_tokens to turn_usage accumulation**

In the `ProviderEvent::Usage(u)` handler (around line 524), add:

```rust
ProviderEvent::Usage(u) => {
    turn_usage.input += u.input_tokens;
    turn_usage.output += u.output_tokens;
    turn_usage.cached += u.cached;
    turn_usage.thinking += u.thinking_tokens;
}
```

- [ ] **Step 5: Flush reasoning after the sub-turn completes**

After the stream loop ends and `pending.is_empty()` is checked (around line 635, the `break 'turn` for "model answered without tools"), flush the reasoning as an ephemeral event BEFORE the break:

```rust
if pending.is_empty() {
    // Final answer: flush reasoning as ephemeral ModelThinking.
    if !thinking_buf.is_empty() {
        emit_ephemeral(
            &mut events,
            ui,
            &config.branch,
            EventKind::ModelThinking { text: std::mem::take(&mut thinking_buf) },
            session_id,
            now,
        )
        .await?;
    }
    break 'turn; // model answered without tools — turn complete
}
```

If `pending` is NOT empty (tool calls → intermediate sub-turn), the reasoning is discarded by simply clearing `thinking_buf` before the next iteration. Add this right after the `iterations` check (around line 642):

```rust
// Intermediate sub-turn (tool calls): discard reasoning — it helped
// the model select tools but isn't useful to the user and would
// clutter the history + context.
thinking_buf.clear();
```

- [ ] **Step 6: Handle the aborted path — discard reasoning**

In the `aborted` block (around line 590), clear `thinking_buf`:

```rust
if aborted {
    stream_task.abort();
    let _ = stream_task.await;
    thinking_buf.clear();
    // ... existing drain logic unchanged
```

- [ ] **Step 7: Write integration test**

Add a test in the `tests` module in `crates/zoid/src/agent.rs`:

```rust
#[tokio::test]
async fn final_sub_turn_thinking_is_persisted_as_model_thinking() {
    use zoid_core::event::EventKind;
    use zoid_provider::{ProviderEvent, ToolCall};

    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::from(1u128), None, 1,
        EventKind::UserMessage { text: "hi".into() },
    )];
    for e in &seed { session.append(e.clone()).await.unwrap(); }

    let provider = std::sync::Arc::new(SequencedProvider::new(vec![
        vec![
            ProviderEvent::ThinkingDelta("let me think...".into()),
            ProviderEvent::TextDelta("answer".into()),
            ProviderEvent::Done,
        ],
    ]));
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let out = run_agent_turn(
        chat_turn_config(),
        provider,
        std::sync::Arc::new(zoid_tools::registry()),
        std::sync::Arc::new(zoid_tools::AllowAll),
        session,
        crate::eventlog::EventLog::from_vec(seed),
        "m".into(),
        tx,
        Ulid::from(0u128),
        zoid_companion::CompanionHub::new(),
        || 0,
    ).await.unwrap();

    assert!(
        out.iter().any(|e| matches!(&e.kind, EventKind::ModelThinking { text } if text == "let me think...")),
        "final sub-turn reasoning must be persisted as ModelThinking"
    );
}

#[tokio::test]
async fn intermediate_sub_turn_thinking_is_discarded() {
    use serde_json::json;
    use zoid_core::event::EventKind;
    use zoid_provider::{ProviderEvent, ToolCall};

    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::from(1u128), None, 1,
        EventKind::UserMessage { text: "read a file".into() },
    )];
    for e in &seed { session.append(e.clone()).await.unwrap(); }

    // Sub-turn 1: thinking + tool call. Sub-turn 2: no thinking, just answer.
    let provider = std::sync::Arc::new(SequencedProvider::new(vec![
        vec![
            ProviderEvent::ThinkingDelta("intermediate reasoning".into()),
            ProviderEvent::ToolCall(ToolCall {
                id: "c1".into(), name: "read_file".into(),
                args: json!({"path": "x"}),
            }),
            ProviderEvent::Done,
        ],
        vec![
            ProviderEvent::TextDelta("final answer".into()),
            ProviderEvent::Done,
        ],
    ]));
    let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
        std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let out = run_agent_turn(
        chat_turn_config(),
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session,
        crate::eventlog::EventLog::from_vec(seed),
        "m".into(),
        tx,
        Ulid::from(0u128),
        zoid_companion::CompanionHub::new(),
        || 0,
    ).await.unwrap();

    assert!(
        !out.iter().any(|e| matches!(&e.kind, EventKind::ModelThinking { text } if text == "intermediate reasoning")),
        "intermediate sub-turn reasoning must be discarded"
    );
    assert!(
        !out.iter().any(|e| matches!(&e.kind, EventKind::ModelThinking { .. })),
        "no ModelThinking event should exist when the final sub-turn had no reasoning"
    );
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p zoid agent::tests`
Expected: PASS

- [ ] **Step 9: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: agent loop accumulates reasoning, discards intermediate, flushes final"
```

---

### Task 8: map_msg ignores thinking field

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Interfaces:**
- Consumes: `ChatMsg::Assistant { thinking: Option<String>, .. }` from Task 6
- Produces: no change to provider `Message` (thinking is NOT sent to the provider)

- [ ] **Step 1: Update map_msg to destructure but ignore thinking**

In `crates/zoid/src/agent.rs`, find `map_msg` (around line 174). Update the `ChatMsg::Assistant` arm to destructure `thinking` but not use it:

```rust
ChatMsg::Assistant {
    text, tool_calls, thinking: _, ..
} => {
    // `thinking` is deliberately ignored — reasoning is NOT replayed to the
    // provider in Phase 2. Phase 3 will add replay of thinking blocks/signatures.
    Message {
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
    }
}
```

- [ ] **Step 2: Build and test**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: map_msg ignores thinking field (no replay in Phase 2)"
```

---

### Task 9: TUI — "▶ Thinking…" marker at Normal zoom

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Consumes: `ChatMsg::Assistant { thinking: Option<String>, .. }` from Task 6
- Produces: collapsed "▶ Thinking…" marker line at Normal zoom

- [ ] **Step 1: Write failing test for thinking marker at Normal zoom**

In `crates/zoid-tui/src/chat.rs`, add a test:

```rust
#[test]
fn normal_zoom_thinking_marker_when_thinking_present() {
    let msgs = vec![ChatMsg::Assistant {
        text: "answer".into(),
        tool_calls: vec![],
        ts: 0,
        thinking: Some("deep reasoning".into()),
    }];
    let view = ChatView {
        zoom: crate::state::Zoom::Normal,
        caret_on: false,
        reveal: None,
        tz_offset_secs: 0,
    };
    let lines = conversation_view(&msgs, &view, false, 80, None);
    let joined: String = lines.iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(joined.contains("Thinking"), "Normal zoom must show Thinking marker");
    assert!(!joined.contains("deep reasoning"), "Normal zoom must NOT show reasoning text");
}

#[test]
fn normal_zoom_no_marker_when_thinking_absent() {
    let msgs = vec![ChatMsg::Assistant {
        text: "answer".into(),
        tool_calls: vec![],
        ts: 0,
        thinking: None,
    }];
    let view = ChatView {
        zoom: crate::state::Zoom::Normal,
        caret_on: false,
        reveal: None,
        tz_offset_secs: 0,
    };
    let lines = conversation_view(&msgs, &view, false, 80, None);
    let joined: String = lines.iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(!joined.contains("Thinking"), "no marker when thinking is None");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui normal_zoom_thinking`
Expected: FAIL — no thinking marker is rendered yet

- [ ] **Step 3: Implement — render thinking marker in build_conversation**

In `crates/zoid-tui/src/chat.rs`, find the `ChatMsg::Assistant` arm in `build_conversation` (around line 169). The struct destructuring currently doesn't include `thinking`. Update it:

```rust
ChatMsg::Assistant {
    text,
    tool_calls,
    ts,
    thinking,
} => {
    // Thinking marker (collapsed at Normal zoom).
    if let Some(thinking_text) = thinking {
        if !thinking_text.is_empty() {
            blank_between_turns(&mut lines);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", glyph::EXPANDED),
                    Style::new().fg(color::DIM),
                ),
                Span::styled("Thinking…", Style::new().fg(color::DIM)),
            ]));
        }
    }
    let mut shown = text.clone();
    // ... rest of existing rendering unchanged
```

Note: use `glyph::EXPANDED` (▶) or the appropriate expand glyph from the tokens module. Check `crates/zoid-tui/src/tokens/glyph.rs` for the right constant.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui normal_zoom_thinking`
Expected: PASS

- [ ] **Step 5: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): render collapsed '▶ Thinking…' marker at Normal zoom"
```

---

### Task 10: TUI — full reasoning text at Detail zoom

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Consumes: `ChatMsg::Assistant { thinking: Option<String>, .. }` from Task 6
- Produces: full dimmed reasoning text with separator at Detail zoom

- [ ] **Step 1: Write failing test for thinking text at Detail zoom**

In `crates/zoid-tui/src/chat.rs`, add a test:

```rust
#[test]
fn detail_zoom_thinking_text_visible() {
    let msgs = vec![ChatMsg::Assistant {
        text: "answer".into(),
        tool_calls: vec![],
        ts: 0,
        thinking: Some("deep reasoning here".into()),
    }];
    let view = ChatView {
        zoom: crate::state::Zoom::Detail,
        caret_on: false,
        reveal: None,
        tz_offset_secs: 0,
    };
    let lines = conversation_view(&msgs, &view, false, 80, None);
    let joined: String = lines.iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(joined.contains("deep reasoning here"), "Detail zoom must show reasoning text");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui detail_zoom_thinking`
Expected: FAIL — detail zoom doesn't render thinking text yet

- [ ] **Step 3: Implement — render thinking text in detail_lines**

In `crates/zoid-tui/src/chat.rs`, find `detail_lines` (around line 770). The `ChatMsg::Assistant` case falls through to the `other` arm at the bottom which calls `conversation_lines`. Before that catch-all, add an explicit `ChatMsg::Assistant` arm that renders the thinking section:

```rust
ChatMsg::Assistant {
    text,
    tool_calls,
    ts,
    thinking,
} => {
    // Thinking section (full text at Detail zoom).
    if let Some(thinking_text) = thinking {
        if !thinking_text.is_empty() {
            blank_between_turns(&mut out);
            // Separator line
            out.push(Line::from(vec![
                Span::styled(
                    "─ Thinking ─────────────────────",
                    Style::new().fg(color::DIM),
                ),
            ]));
            // Reasoning text, dimmed
            for line in crate::markdown::render_markdown(thinking_text) {
                let mut spans = vec![Span::styled("    ", Style::new())];
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
            out.push(Line::from("")); // blank after thinking
        }
    }
    // Answer text (same as the existing conversation_lines path)
    let mut shown = text.clone();
    if !shown.is_empty() || tool_calls.is_empty() {
        blank_between_turns(&mut out);
        let prefix = vec![
            stamp(*ts),
            Span::styled("zoid ".to_string(), Style::new().fg(color::DIM)),
        ];
        // Reuse the existing push_message helper for the answer
        let mut temp_lines = Vec::new();
        let mut temp_code = Vec::new();
        push_message(
            &mut temp_lines,
            &mut temp_code,
            prefix,
            render_body(&shown),
            width,
        );
        out.extend(temp_lines);
    }
    for tc in tool_calls {
        out.push(Line::from(vec![
            Span::styled(
                format!("  {} ", glyph::EDIT),
                Style::new().fg(color::CHAT_ACCENT),
            ),
            Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
            Span::styled(
                format!("({})", arg_summary(&tc.args)),
                Style::new().fg(color::DIM),
            ),
        ]));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui detail_zoom_thinking`
Expected: PASS

- [ ] **Step 5: Build and test full workspace**

Run: `cargo test --workspace`
Expected: PASS — note: some snapshot tests may need updating if the seeded test fixtures include assistant messages. Run `cargo insta accept` if needed.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): render full reasoning text at Detail zoom with separator"
```

---

### Task 11: ShellState — tps + thinking_label fields

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`

**Interfaces:**
- Produces: `ShellState.tps: u64`, `ShellState.thinking_label: Option<String>`

- [ ] **Step 1: Add fields to ShellState**

In `crates/zoid-tui/src/state.rs`, find the `ShellState` struct and add the fields near the existing `duration`/`session_tokens` fields (around line 233):

```rust
    /// Compact elapsed-time-in-session label (e.g. "12m", "1h3m").
    pub duration: String,
    /// Thinking mode label for the session drawer (e.g. "thinking high").
    /// `None` when thinking is off.
    pub thinking_label: Option<String>,
    /// Tokens per second (output_tokens / stream_seconds) from the last turn.
    /// 0 when no turn has completed or stream duration is 0.
    pub tps: u64,
    /// Total tokens spent in the active session (session drawer "tok" line).
    pub session_tokens: u64,
```

- [ ] **Step 2: Fix ShellState::new construction**

In the same file, find `ShellState::new` (or the `Default` impl) and add:

```rust
    duration: "0m".into(),
    thinking_label: None,
    tps: 0,
    session_tokens: 0,
```

- [ ] **Step 3: Fix all other ShellState construction sites**

Run: `grep -rn "ShellState {" crates/ --include="*.rs"`
Add `thinking_label: None,` and `tps: 0,` to every construction site (test helpers, etc.).

- [ ] **Step 4: Build and test**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add tps + thinking_label to ShellState"
```

---

### Task 12: Session drawer — TPS + thinking mode indicator

**Files:**
- Modify: `crates/zoid/src/main.rs` (per-frame computation)
- Modify: `crates/zoid-tui/src/render.rs` (rendering)

**Interfaces:**
- Consumes: `ShellState.tps` + `ShellState.thinking_label` from Task 11
- Produces: new session drawer layout

- [ ] **Step 1: Compute tps + thinking_label per-frame in main.rs**

In `crates/zoid/src/main.rs`, find the per-frame sync block (around line 2008). After the existing `duration` assignment, add:

```rust
    app.shell.duration = fmt_duration(app.session_started_ms, now_ms());
    // Thinking mode label for the session drawer.
    app.shell.thinking_label = if app.config.thinking.enabled {
        match &app.config.thinking.effort {
            None => Some("thinking".to_string()),
            Some(e) => Some(format!("thinking {e}")),
        }
    } else {
        None
    };
    // TPS: output tokens from the last Usage event / stream duration.
    // Both are already tracked; compute from the latest projection data.
    let last_output = app.proj.last_input_tokens; // placeholder — see below
    // Actually we need the last output_tokens, not input. Check ProjectionCache.
```

Wait — `ProjectionCache` tracks `last_input_tokens` but not `last_output_tokens`. We need the output token count from the most recent `Usage` event. Add `last_output_tokens: Option<u64>` to `ProjectionCache` (in `crates/zoid/src/main.rs`), populated in `refresh()` by scanning for the last `Usage` event's `output` field (mirror the existing `last_input_tokens` logic):

In `ProjectionCache`:
```rust
    last_output_tokens: Option<u64>,
```

In `refresh()`:
```rust
    self.last_output_tokens = events
        .iter()
        .rev()
        .find_map(|e| e.tokens.map(|t| t.output))
        .filter(|&t| t > 0);
```

Then compute TPS per-frame:

```rust
    // TPS from the last turn's output tokens and the obs stream duration.
    // provider_total is a rolling average of stream_ms; we use it as the
    // denominator. 0 when no data is available.
    let stream_ms = obs_state
        .lock()
        .ok()
        .map(|s| s.provider_total.avg())
        .unwrap_or(0);
    app.shell.tps = if stream_ms > 0 {
        app.proj.last_output_tokens.unwrap_or(0) * 1000 / stream_ms
    } else {
        0
    };
```

Note: `obs_state` is available in the `run()` function scope. The exact placement may need adjustment — find where `obs_state` is accessible and compute TPS there. The `build_overview_data` function already reads from `obs_state`, so the pattern exists.

- [ ] **Step 2: Update session drawer rendering in render.rs**

In `crates/zoid-tui/src/render.rs`, find the session drawer's dur/tok/cac line (around line 688). The current single line renders `dur tok cac`. Split it into two lines:

**Line 1** (dur + thinking label):
```rust
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::SESS_DURATION),
                Style::new().fg(color::DIM),
            ),
            Span::styled("dur ", Style::new().fg(color::DIM)),
            Span::styled(
                format!("{}  ", state.duration),
                Style::new().fg(color::TXT),
            ),
            if let Some(label) = &state.thinking_label {
                Span::styled(
                    format!("◆ {}", label),
                    Style::new().fg(color::CHAT_ACCENT),
                )
            } else {
                Span::styled("", Style::new())
            },
        ]),
```

**Line 2** (tok + cac + tps):
```rust
        Line::from(vec![
            Span::styled(
                if state.cache_supported {
                    "tok "
                } else {
                    "tok/cac "
                },
                Style::new().fg(color::DIM),
            ),
            Span::styled(
                format!(
                    "{}   ",
                    if state.cache_supported {
                        human_tokens(state.session_tokens)
                    } else {
                        human_tokens(state.session_tokens + state.cached_tokens)
                    }
                ),
                Style::new().fg(color::TXT),
            ),
            if state.cache_supported {
                Span::styled("cac ", Style::new().fg(color::DIM))
            } else {
                Span::styled("", Style::new())
            },
            if state.cache_supported {
                Span::styled(
                    format!("{}   ", human_tokens(state.cached_tokens)),
                    Style::new().fg(color::TXT),
                )
            } else {
                Span::styled("", Style::new())
            },
            Span::styled("tps ", Style::new().fg(color::DIM)),
            Span::styled(
                format!("{}", state.tps),
                Style::new().fg(color::TXT),
            ),
        ]),
```

- [ ] **Step 3: Fix all ShellState construction sites with new fields**

Run: `grep -rn "ShellState {" crates/ --include="*.rs"`
Ensure every construction site includes `thinking_label: None,` and `tps: 0,`.

- [ ] **Step 4: Build and test**

Run: `cargo test --workspace`
Expected: Some snapshot tests may fail due to the new session drawer layout. Run `cargo insta accept` to update them.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: session drawer shows TPS + thinking mode indicator"
```

---

### Task 13: Final workspace test + clippy

**Files:**
- None (verification only)

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No new warnings from this plan's changes (pre-existing warnings are acceptable)

- [ ] **Step 3: Run insta review if snapshots changed**

Run: `cargo insta review`
Accept any snapshot changes that reflect the new thinking markers or session drawer layout.

- [ ] **Step 4: Commit any remaining changes**

```bash
git add -A
git commit -m "chore: final clippy + snapshot updates for Phase 2"
```

---

## Summary

| Task | What | Key files |
|---|---|---|
| 1 | ProviderEvent + Usage extensions | `zoid-provider/src/lib.rs` |
| 2 | ModelThinking event + TokenStat.thinking | `zoid-core/src/event.rs`, `economy.rs` |
| 3 | Anthropic parse emits ThinkingDelta/Signature | `zoid-provider/src/anthropic/parse.rs` |
| 4 | OpenAI-compat parse emits ThinkingDelta | `zoid-provider/src/openai_compat.rs` |
| 5 | Ollama parse emits ThinkingDelta | `zoid-provider/src/ollama.rs` |
| 6 | Projection: ChatMsg::Assistant.thinking + fold | `zoid-core/src/projection.rs` |
| 7 | Agent loop: accumulate, discard, flush | `zoid/src/agent.rs` |
| 8 | map_msg ignores thinking | `zoid/src/agent.rs` |
| 9 | TUI: collapsed marker at Normal zoom | `zoid-tui/src/chat.rs` |
| 10 | TUI: full text at Detail zoom | `zoid-tui/src/chat.rs` |
| 11 | ShellState: tps + thinking_label | `zoid-tui/src/state.rs` |
| 12 | Session drawer: TPS + thinking indicator | `zoid/src/main.rs`, `zoid-tui/src/render.rs` |
| 13 | Final test + clippy | — |