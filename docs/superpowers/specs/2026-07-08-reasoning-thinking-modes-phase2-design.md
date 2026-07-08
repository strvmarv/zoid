# Reasoning / Thinking Modes — Phase 2: Visibility

**Date:** 2026-07-08
**Status:** Design
**Depends on:** Phase 1 (2026-07-07-reasoning-thinking-modes-design.md) — complete and merged.

## Overview

Phase 2 adds **visibility**: reasoning text is captured from the provider, carried
through the event log (in-memory only), projected into the conversation, and
rendered as a collapsible "▶ Thinking…" marker at Normal zoom that expands to
the full reasoning text at Detail zoom. Only the final sub-turn's reasoning is
kept — intermediate reasoning from tool-selection sub-turns is discarded to
bound context cost and avoid clutter.

Additionally, two session-widget improvements ride along:
- **TPS** (tokens per second) added to the right-rail session drawer
- **Thinking mode indicator** next to duration in the session drawer

### What Phase 2 does NOT include

**Replay** (persisting reasoning and sending it back to the provider on
subsequent turns) is explicitly out of scope for Phase 2. The Phase 1
limitation stands: no reasoning continuity across tool-use loops. Replay is a
future Phase 3.

## Design

### The Provider Seam — Capturing Reasoning

#### `ProviderEvent::ThinkingDelta(String)`

A new event variant alongside `TextDelta`:

```rust
pub enum ProviderEvent {
    TextDelta(String),
    ThinkingDelta(String),   // NEW — reasoning text from the model
    ToolCall(ToolCall),
    // ... rest unchanged
}
```

Each provider's parse layer emits `ThinkingDelta` instead of discarding:

- **Anthropic** (`parse.rs`): `Delta::ThinkingDelta { thinking }` →
  `vec![ProviderEvent::ThinkingDelta(thinking)]` (currently returns `vec![]`).
  `Delta::SignatureDelta` emits a separate `ThinkingSignature` event (below).
- **OpenAI-compat / DeepSeek** (`openai_compat.rs`): `parse_chunk` extracts
  `delta.reasoning_content` and emits `ProviderEvent::ThinkingDelta(reasoning_content)`
  (currently discarded). The existing discard tests are updated to assert
  `ThinkingDelta` is emitted instead of nothing.
- **Ollama** (`ollama.rs`): `parse_line` extracts `message.thinking` and emits
  `ProviderEvent::ThinkingDelta(thinking)` (currently discarded).
- **OpenAI (o-series)**: no reasoning returned — no change.

#### `ProviderEvent::ThinkingSignature(String)` — Anthropic only

A separate event for Anthropic's thinking block signature, needed for future
replay (Phase 3):

```rust
pub enum ProviderEvent {
    // ...
    ThinkingSignature(String),   // NEW — Anthropic signature for replay
    // ...
}
```

Anthropic's `Delta::SignatureDelta { signature }` →
`vec![ProviderEvent::ThinkingSignature(signature)]`. Emitted at the end of each
thinking block. Other providers never emit this. The agent loop accumulates
signatures in-memory alongside reasoning text; they're not rendered but are
available for Phase 3 replay.

#### `Usage` gains `thinking_tokens`

```rust
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached: u64,
    pub thinking_tokens: u64,   // NEW — reasoning token count
}
```

**Anthropic**: if the API reports thinking tokens separately in the usage
object, extract them. Otherwise `thinking_tokens` stays 0 (thinking tokens are
bundled in `output_tokens`). Verify during implementation.

**DeepSeek**: `reasoning_content` tokens are included in `output_tokens` with
no separate reporting. `thinking_tokens` stays 0 (accepted limitation).

**Ollama**: no token-level breakdown. `thinking_tokens` stays 0.

### Event Layer — In-Memory Reasoning

#### `EventKind::ModelThinking { text: String }`

A new event kind for reasoning text:

```rust
pub enum EventKind {
    // ...
    ModelThinking {
        text: String,
    },
    // ...
}
```

This event is **in-memory only** — it's added to the `EventLog` but NOT
persisted to SQLite. On session resume, reasoning events are gone (the "▶
Thinking…" markers disappear, but the answers remain).

**Implementation**: `EventKind::ModelThinking` events are pushed to the
in-memory `EventLog` but the `session.append()` call is skipped (or the event
is marked as ephemeral). The `EventLog` needs a way to hold ephemeral events
that aren't written to the store. The simplest approach: add an
`ephemeral: bool` flag to `Event` (defaults to `false`), and skip
`session.append()` when `ephemeral` is true. The projection and TUI see the
event normally; the store never receives it.

#### `TokenStat` gains `thinking`

```rust
pub struct TokenStat {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub thinking: u64,   // NEW — thinking token count
}
```

The `Usage` event carries `thinking_tokens` into `TokenStat.thinking`. The
economy ledger's `TokenLedger` gains a `thinking: u64` field so the session
drawer can display it.

#### Accumulation and discard strategy

During a tool-use loop, each sub-turn may produce reasoning. The agent loop
accumulates reasoning text from `ThinkingDelta` events into a buffer. When a
sub-turn ends:

- **If the sub-turn produced tool calls** (intermediate): the reasoning buffer
  is **discarded**. No `ModelThinking` event is emitted. The reasoning helped
  the model select its tool calls, but it's not interesting to the user and
  would clutter the history.
- **If the sub-turn produced a final answer** (no tool calls): the reasoning
  buffer is flushed as a single `ModelThinking { text }` event, inserted into
  the `EventLog` immediately before the `AssistantMessage` (or the first
  `ModelDelta` that begins the answer). This is the only reasoning the user
  sees.

Signatures (`ThinkingSignature`) are accumulated the same way and attached to
the `ModelThinking` event (or a parallel in-memory structure) for future Phase
3 replay. They're not rendered.

### Projection — `ChatMsg::Assistant` gains `thinking`

```rust
pub enum ChatMsg {
    User { text: String, ts: i64 },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRef>,
        ts: i64,
        thinking: Option<String>,   // NEW — reasoning text (final sub-turn only)
    },
    // ... rest unchanged
}
```

The projection folds `ModelThinking` events into the **next** `Assistant`
message's `thinking` field. A `ModelThinking` event is followed by
`ModelDelta` events that build the assistant's answer; the projection
attaches the thinking text to the assistant message that follows it.

If a `ModelThinking` event has no following assistant message (edge case:
model produced only reasoning then stopped), it becomes a standalone
`ChatMsg::Assistant` with empty `text` and `thinking` set.

#### `map_msg` — reasoning is NOT sent to the provider

The `map_msg` function in `agent.rs` maps `ChatMsg::Assistant` to a provider
`Message`. The `thinking` field is **ignored** — it's not included in the
`Message.content` or any other field. The provider only sees the answer text,
not the reasoning. (This is the Phase 2 replay boundary — Phase 3 will add
replay of thinking blocks/signatures to the provider request.)

### TUI Rendering — Collapsible Thinking Marker

#### Normal zoom: "▶ Thinking…" marker

When a `ChatMsg::Assistant` has `thinking: Some(text)`, the conversation view
renders a one-line marker above the answer:

```
▶ Thinking…
```

The marker uses `color::DIM` and a `▶` glyph. It's a single line — no
expansion at Normal zoom. Clicking on it does nothing (detail zoom is the
only expansion path, per the design decision).

The marker is **not interactive** at Normal zoom — it's purely informational.
The user zooms into the message (Detail zoom) to see the reasoning.

#### Detail zoom: full reasoning text

At Detail zoom, the same `ChatMsg::Assistant` renders the full reasoning text
above the answer, in a dimmed/italic style:

```
─ Thinking ─────────────────────
<reasoning text, wrapped, dimmed>

<answer text, normal style>
```

The reasoning text is wrapped at the same width as the conversation pane. A
thin separator line (`─ Thinking ─...`) delimits it from the answer. The
reasoning uses `color::DIM` with italic style (if supported by the terminal).

The reasoning text is **not live-streamed** — it only appears after the
sub-turn completes and the `ModelThinking` event is emitted. During active
reasoning, the user sees only the status bar spinner (existing behavior) and
the "▶ Thinking…" marker appears once reasoning is complete.

#### Body cache interaction

The `BodyCache` key needs to include whether a message has thinking text, so
that zooming into a message with reasoning triggers a body rebuild. The
existing `zoom` field in `BodyKey` already handles this — zooming from Normal
to Detail changes the key and triggers a rebuild. No new `BodyKey` field is
needed; the thinking text is part of the `ChatMsg` which is already an input
to `body_cache.refresh()`.

The `msg_starts` and `msg_count` fields in `BodyCache` need to account for the
extra lines consumed by the reasoning text at Detail zoom. The existing
`conversation_view_indexed` function handles this naturally — it renders all
lines including the thinking section, and `msg_starts` indexes the message
boundaries.

### Session Drawer — TPS + Thinking Mode Indicator

#### Current layout (session drawer right rail)

```
● session-name
model · provider
⏱ dur 12m   tok 1.2k   cac 400
ctx ████████░░ 45k/200k
cwd ~/source/zoid
```

#### New layout

```
● session-name
model · provider
⏱ dur 12m   ◆ thinking high
tok 1.2k   cac 400   tps 42
ctx ████████░░ 45k/200k
cwd ~/source/zoid
```

**Line 3** (was `dur tok cac`): now `dur` + thinking mode indicator.
- `⏱ dur 12m` — unchanged
- `◆ thinking` when enabled with Auto effort
- `◆ thinking high` when enabled with explicit effort
- `◆ thinking max` etc. for other effort levels
- Nothing when thinking is off (just `⏱ dur 12m`)

**Line 4** (new line, was part of line 3): `tok cac tps`.
- `tok 1.2k` — session tokens (unchanged)
- `cac 400` — cached tokens (unchanged, only when `cache_supported`)
- `tps 42` — tokens per second (new)

#### TPS calculation

`tps = output_tokens / (stream_ms / 1000)` where:
- `output_tokens` is from the most recent `Usage` event's `output` field
- `stream_ms` is the provider stream duration from the obs layer
  (`provider_total` rolling stat)

The `ShellState` gains:
- `tps: u64` — computed per-frame from the latest usage + stream duration
- `thinking_label: Option<String>` — `Some("thinking high")` or `None`

These are set in the per-frame sync in `run()` alongside the existing
`session_tokens`, `cached_tokens`, `duration`, etc.

### Testing Strategy

#### Provider parse tests (emit ThinkingDelta)

- **Anthropic**: `ThinkingDelta` frame → `vec![ProviderEvent::ThinkingDelta("...")]`.
  `SignatureDelta` frame → `vec![ProviderEvent::ThinkingSignature("...")]`.
  Update existing `thinking_delta_emits_nothing` and `signature_delta_emits_nothing`
  tests to assert the new events.
- **OpenAI-compat**: `delta.reasoning_content` → `vec![ProviderEvent::ThinkingDelta("...")]`.
  Update existing `parse_chunk_reasoning_content_is_discarded` tests to assert
  `ThinkingDelta` is emitted.
- **Ollama**: `message.thinking` → `vec![ProviderEvent::ThinkingDelta("...")]`.
  Update existing `thinking_only_line_yields_none` test to assert `ThinkingDelta`.

#### Event layer tests

- `ModelThinking` event is ephemeral: pushed to `EventLog` but not to
  `session.append()`. Verify with a test that checks `events.len()` increases
  but the session store doesn't grow.
- `TokenStat.thinking` is carried through from `Usage` event.

#### Projection tests

- `ModelThinking` followed by `ModelDelta` events → `ChatMsg::Assistant` with
  `thinking: Some("reasoning text")` and `text: "answer text"`.
- `ModelThinking` with no following assistant message → standalone
  `ChatMsg::Assistant` with empty text + thinking.
- `map_msg` ignores the `thinking` field (provider `Message` has no reasoning).

#### TUI rendering tests

- At Normal zoom: `ChatMsg::Assistant` with `thinking: Some(...)` renders a
  "▶ Thinking…" marker line above the answer. No reasoning text visible.
- At Detail zoom: same message renders the full reasoning text in dimmed style
  above the answer, with a separator line.
- At Normal zoom: `ChatMsg::Assistant` with `thinking: None` renders no marker.

#### Session drawer tests

- TPS is computed from `output_tokens / stream_ms`.
- Thinking label shows `◆ thinking high` when config has `enabled: true, effort:
  Some("high")`.
- Thinking label is absent when thinking is off.
- Tok/cac bumped to a new line below dur.

#### Agent loop integration test

A `SequencedProvider` test: provider emits `ThinkingDelta("reasoning")` then
`TextDelta("answer")` then `Done`. Verify:
- A `ModelThinking` event is in the event log with `text: "reasoning"`.
- The projection's `ChatMsg::Assistant` has `thinking: Some("reasoning")`.
- No `ModelThinking` event when the sub-turn has tool calls (intermediate
  reasoning is discarded).

### Scope Boundaries

#### In scope (Phase 2)

- `ProviderEvent::ThinkingDelta(String)` + `ThinkingSignature(String)`.
- `Usage.thinking_tokens` + `TokenStat.thinking`.
- `EventKind::ModelThinking { text }` (ephemeral, in-memory only).
- `ChatMsg::Assistant { thinking: Option<String> }`.
- TUI: "▶ Thinking…" marker at Normal zoom, full text at Detail zoom.
- Session drawer: TPS line + thinking mode indicator.
- Agent loop: accumulate reasoning, discard intermediate, persist final.

#### Out of scope (Phase 3 — future)

- Replay of thinking blocks / `reasoning_content` in subsequent requests.
- Anthropic signature persistence and replay.
- DeepSeek conditional replay (tool-call-aware `reasoning_content` inclusion).
- `Message.thinking` + `Message.thinking_signature` fields for provider replay.
- Per-mode thinking overrides.
- `display: "omitted"` optimization for Anthropic.
- Live streaming of reasoning text at Detail zoom (reasoning appears only after
  the sub-turn completes).

### Verify-during-implementation

- Whether Anthropic's API reports `thinking_tokens` separately in the usage
  object, or bundles them into `output_tokens`. If separate, extract into
  `Usage.thinking_tokens`; if bundled, leave as 0 and note the limitation.
- The exact Anthropic `SignatureDelta` payload shape — confirm it carries a
  `signature` string that can be accumulated for Phase 3 replay.
- Whether Ollama's `message.thinking` field is present on all frames or only
  some (e.g. only when `think: true` is set). The parse layer should handle
  its absence gracefully (already does — missing fields default to empty).