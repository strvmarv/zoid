# Peek Removal & Incremental Projection — Design

**Date:** 2026-07-26
**Status:** Design (approved shape; pending spec review)

---

## 1. Problem

The `acm` session (225,569 events, ~7,121 folded `ChatMsg`s, 428 MB DB) makes
the TUI sluggish during streaming. A fresh session is fast. The degradation is
linear in session size and has two independent causes:

### Cause A — per-frame O(n) peek-hit computation

`peek_hits()` runs a full `build_conversation()` — markdown parsing, line
wrapping, code-block detection for every message — **every frame at 30 FPS
during streaming** (`main.rs:2754`). This is the same expensive work the
`BodyCache` was designed to avoid, but `peek_hits` bypasses the cache entirely.
It exists only to support click hit-testing on `⏎ peek` hint text — a popup
that duplicates what Detail zoom already shows.

### Cause B — full O(225K) projection rebuilds on bookkeeping events

Every non-`ModelDelta`/non-`ToolCall` event invalidates `ProjectionCache` and
`BodyCache`, forcing a full rebuild of all 5 projections
(`conversation`, `context_window`, `churn_timeline`, `tasks`, `token_ledger`)
plus a full body rebuild. Per turn, 24–705 structural events do this. Most are
bookkeeping events (`Usage`, `Tasks`, `Wake*`, etc.) that don't change `msgs`
at all — yet they trigger a full `conversation()` fold over 225K events.

---

## 2. Part 1 — Remove peek entirely

### Rationale

Peek duplicates Detail zoom. The `⏎ peek` hint on tool-call lines and
delegated chips is the only visual artifact; the popup itself is a bordered,
scrollable overlay showing tool-call args + result output (or a delegated
summary). Detail zoom already renders tool results with syntax highlighting
and delegated summaries as markdown. The per-frame `peek_hits()` call is the
primary performance bottleneck. Removing the feature eliminates the
bottleneck with zero functional loss.

### Removal surface

Every file and symbol that references peek:

**`crates/zoid-tui/src/chat.rs`**
- Delete: `PeekHit`, `PeekKind`, `peek_hits()` function.
- Remove the `peek_hits: &mut Vec<PeekHit>` parameter from
  `build_conversation()`.
- Remove the two `peek_hits.push(...)` call sites (tool-call line ~334,
  delegated chip ~450).
- Remove the `⏎ peek` hint strings from the tool-call line and the delegated
  chip line.
- Delete 4 tests: `peek_hits_finds_tool_call_line`,
  `peek_hits_finds_delegated_chip`, `peek_hits_empty_for_prose_only`,
  `peek_hits_multiple_tool_calls_each_get_own_hit`.

**`crates/zoid-tui/src/state.rs`**
- Delete: `PeekState`, `PeekContent` enum.
- Remove the `peek: Option<PeekState>` field from `ShellState`.
- Remove its default init (`peek: None` in `Default` impl).
- Delete 3 tests: `peek_is_none_by_default`, `peek_set_and_clear`,
  `peek_included_in_equality`.

**`crates/zoid-tui/src/render.rs`**
- Delete: `render_peek_overlay()` function.
- Remove the peek-overlay draw block (`if let Some(p) = layout.peek { ... }`).
- Remove the `layout.peek` read.

**`crates/zoid-tui/src/layout.rs`**
- Remove the `peek: Option<Rect>` field from `ShellLayout`.
- Remove its computation in `compute()`.
- Delete 2 tests: `peek_rect_none_when_peek_closed`,
  `peek_rect_some_when_peek_open`.

**`crates/zoid-tui/src/route.rs`**
- Delete `Action::DismissPeek`, `Action::ScrollPeek(i32)`,
  `Action::PeekClick(u16, u16)`.
- Remove the key-routing block for an open peek (Esc/dismiss, arrows/scroll).
- Remove the mouse-routing block for an open peek (scroll/click).
- Delete 5 tests: `peek_open_esc_dismisses`, `peek_open_arrows_scroll`,
  `peek_open_other_keys_are_noop`, `peek_open_mouse_scroll_scrolls_peek`,
  `peek_open_mouse_click_returns_peek_click`, `peek_closed_mouse_behaves_normally`.

**`crates/zoid/src/main.rs`**
- Delete `PeekCache` struct and `peek_cache` field from `App`.
- Delete the per-frame `peek_hits()` call (the bottleneck, ~line 2754) and
  the `body_rebuilt || app.streaming` guard around it.
- Delete the `Action::DismissPeek`, `Action::ScrollPeek`,
  `Action::PeekClick` handler arms in `handle_action`.
- Delete the peek-open logic in `handle_conversation_click` (~lines 1019–1056).
- Remove `peek_cache` init at all `App` construction sites.
- Delete 3 tests: `conversation_click_on_tool_call_opens_peek`,
  `dismiss_peek_clears_state`, `scroll_peek_adjusts_scroll`.

### Post-removal behavior

Clicking a tool-call line or delegated chip does nothing (same as clicking a
prose line). The `⏎ peek` hint text is gone from the transcript. Detail zoom
remains the way to see full tool output and delegated summaries. No
replacement feature is needed.

**Note (from review):** Peek's `PeekContent::ToolCall` showed the full raw
`args` JSON alongside the result output. The tool-call *line* at Normal zoom
shows only `arg_summary` (truncated). Before shipping, verify that Detail zoom
surfaces the full args (not just the result output) — if not, the "zero
functional loss" claim is slightly overstated for the args specifically. The
result output (the primary peek use case) is fully covered by Detail zoom.

---

## 3. Part 2 — Stop full projection rebuilds on bookkeeping events

### Current flow

```
AgentUpdate::Appended(ev)
  → apply_streaming(ev)     // O(1) for ModelDelta/ToolCall; returns false for everything else
  → if false:
      proj.events_len = None    // invalidates ALL projections
      body_cache.key = None     // invalidates the body
  → next frame:
      proj.refresh()            // full O(n) rebuild of all 5 projections
      body_cache.refresh()      // full O(n) body rebuild with markdown parsing
```

### New flow — three tiers

Structural events are split into three tiers based on what they actually
change. `apply_streaming` is replaced by `apply_event`, which returns a
`ProjectionImpact` describing what changed:

```rust
enum ProjectionImpact {
    /// No `msgs` change, no body change. Economy projections may need refresh.
    /// The caller does NOT invalidate `body_cache`.
    Economy,
    /// `msgs` content changed but `msg_count` did not (content mutation on an
    /// existing message, or streaming append to the last message). Carries the
    /// index of the mutated message. The caller does NOT set
    /// `body_cache.key = None` when the mutation is at the last index — the
    /// `BodyCache` incremental path re-renders just that message (O(1)). When
    /// the mutation is NOT at the last index, the caller DOES set
    /// `body_cache.key = None` — the incremental path only re-renders the
    /// last message, so a non-last mutation would leave stale lines.
    MsgsMutated { mutated_index: Option<usize> },
    /// A new ChatMsg was appended (msg_count changed). The caller invalidates
    /// `body_cache` (full body rebuild unavoidable — line positions shift).
    MsgsAppended,
    /// Could not apply incrementally — caller must do a full refresh.
    FullRefresh,
}
```

`MsgsMutated` carries `mutated_index` so the caller can distinguish
last-message mutations (O(1) incremental re-render via `BodyCache`) from
non-last mutations (full body rebuild needed — the `BodyCache` incremental
path only re-renders the last message). `None` means "the mutation is at the
end of `msgs`" (e.g. streaming `ModelDelta` appending to the last assistant
message — no specific index needed, the `BodyCache` already handles this via
`msg_count` match + `structural_match`). `Some(i)` means "message at index
`i` was mutated" — the caller checks `i == msgs.len() - 1` to decide whether
to invalidate `body_cache`.

#### Tier 1 — pure bookkeeping (no `msgs` change, no body change)

Events: `Usage`, `Tasks`, `WakeScheduled`, `WakeFired`, `WakeCancelled`,
`TurnsDropped`, `ContextMutation`, `DirectiveReasserted`, `TurnsReadmitted`.

These don't appear in `conversation()` at all. `apply_event` applies them
incrementally to the economy projections and returns `ProjectionImpact::Economy`.
`msgs` and `body_cache` are untouched.

| Event | Incremental action |
|-------|-------------------|
| `Usage` | Add to `ledger_total`/`cached_total`; update `last_input_tokens`/`last_output_tokens` from the event's `TokenStat`. |
| `Tasks` | Replace `tasks` vec (last-write-wins, O(1)). |
| `Wake*` | No projection effect. No-op. |
| `TurnsDropped` | No `msgs` effect. Flag `window_dirty` (economy may change). |
| `ContextMutation` | No `msgs` effect. Flag `window_dirty` (pin/evict/restore changes items). |
| `DirectiveReasserted` | No `msgs` effect. Flag `window_dirty`. |
| `TurnsReadmitted` | No `msgs` effect. Flag `window_dirty` (evicted set changes). |

`window_dirty` and `churn_dirty` are new bool fields on `ProjectionCache`.
When dirty, the next `refresh()` call rebuilds only the dirty projections
(not `msgs`), then clears the flags. `body_cache` is never invalidated by
tier-1 events.

#### Tier 2 — append-only `msgs` change

Events: `UserMessage`, `AssistantMessage`, `ModelThinking`, `ToolResult`,
`QuestionAsked`, `DelegationResult`, `TurnsEvicted`.

These push a new `ChatMsg` onto `msgs` (after flushing any pending assistant
turn — the same `flush()` logic from `conversation_for_branch`). The flush +
push is O(1). `apply_event` returns `ProjectionImpact::MsgsAppended`.

The caller invalidates `body_cache` (msg count changed → full body rebuild
unavoidable — a new message shifts line positions throughout the wrapped
transcript). But the O(225K) `conversation()` fold is avoided: `msgs` is
already updated.

Economy projections (`window`, `churn`) are flagged dirty (the new event may
add a context item or churn point). `token_ledger` is updated incrementally
for `Usage`-bearing events (most tier-2 events carry no tokens, so it's a
no-op). `tasks` is unaffected.

`QuestionAnswered` is a special case: it changes an existing `Question` card's
state from `Open` to `Answered` (same msg count, changed content). `apply_event`
finds the matching `ChatMsg::Question` by id and updates its `state` field
in-place. Returns `MsgsMutated { mutated_index: Some(idx) }` — the caller
checks if `idx` is the last message: if so, the `BodyCache` incremental path
re-renders it O(1); if not, the caller invalidates `body_cache` for a full
rebuild. If the question is not found in `msgs` (partial resume), returns
`Economy` (no-op — the next full refresh handles it).

#### Tier 3 — content mutation (existing msg changes, count unchanged)

Events: `ToolResultCompacted`.

Replaces an existing `ToolResult`'s `output` with a summary and sets
`compacted = true`. `apply_event` finds the matching `ChatMsg::ToolResult` by
id and updates it in-place. Returns `MsgsMutated { mutated_index: Some(idx) }`
— the caller checks if `idx` is the last message: if so, O(1) incremental
re-render; if not, full body rebuild (the compacted result may be many
messages back — ACM compaction is a background sweep, not inline). If the
result is not found in `msgs` (partial resume), returns `Economy` (no-op —
the next full refresh handles it via the `compacted` map in the fold).

`window_dirty` is flagged (the context-window item's token count changes —
the compacted summary replaces the full output's token estimate).

### `ProjectionCache` changes

New fields:

```rust
struct ProjectionCache {
    events_len: Option<usize>,
    msgs: Vec<ChatMsg>,
    window: ContextWindow,
    churn: ChurnTimeline,
    tasks: Vec<TaskItem>,
    ledger_total: u64,
    cached_total: u64,
    /// Cumulative thinking tokens across all Usage events. Maintained
    /// incrementally by apply_event (Usage) so the session drawer's thinking
    /// display is current without a full refresh.
    thinking_total: u64,
    last_input_tokens: Option<u64>,
    last_output_tokens: Option<u64>,
    // NEW — dirty flags for deferred economy rebuilds.
    window_dirty: bool,
    churn_dirty: bool,
    // NEW — ids of non-Approval QuestionAsked events, so ToolResults with
    // the same id are suppressed (mirrors conversation_for_branch's pre-pass).
    question_ids: std::collections::HashSet<String>,
    // NEW — accumulator state for incremental economy updates.
    // Used by apply_event for Usage (ledger) and Tasks (last-write-wins).
    // Not used for window/churn — those are rebuilt from the full log when dirty.
}
```

`refresh()` changes: when `events_len` matches (no full invalidation), check
`window_dirty`/`churn_dirty` and rebuild only the dirty projections. When
`events_len` is `None` (full invalidation — e.g. session resume, subagent
branch handling), rebuild everything as today.

New method `apply_event(&mut self, ev: &Event) -> ProjectionImpact`:

```rust
fn apply_event(&mut self, ev: &Event) -> ProjectionImpact {
    use crate::event::EventKind;
    match &ev.kind {
        // Streaming hot path — same logic as today's apply_streaming, but
        // returns MsgsMutated { mutated_index: None } (not MsgsAppended) so
        // the caller does NOT invalidate body_cache. None means "mutation at
        // the end" — the BodyCache incremental path re-renders just the last
        // message. The BodyCache detects this via its existing
        // structural_match && msg_count == msgs.len() check.
        EventKind::ModelDelta { text } => {
            if let Some(ChatMsg::Assistant { text: t, .. }) = self.msgs.last_mut() {
                t.push_str(text);
            } else {
                self.pending_text.get_or_insert_with(String::new).push_str(text);
                self.pending_turn_ts.get_or_insert(ev.ts);
            }
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsMutated { mutated_index: None }
        }
        EventKind::ToolCall { id, name, args } => {
            if let Some(ChatMsg::Assistant { tool_calls, .. }) = self.msgs.last_mut() {
                tool_calls.push(ToolCallRef { id: id.clone(), name: name.clone(), args: args.clone() });
            } else {
                self.pending_turn_ts.get_or_insert(ev.ts);
                self.pending_calls.push(ToolCallRef { id: id.clone(), name: name.clone(), args: args.clone() });
            }
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsMutated { mutated_index: None }
        }

        // Tier 1 — bookkeeping.
        EventKind::Usage => {
            if let Some(t) = ev.tokens {
                self.ledger_total += t.input + t.output;
                self.cached_total += t.cached;
                self.thinking_total += t.thinking;
                if t.input > 0 { self.last_input_tokens = Some(t.input); }
                if t.output > 0 { self.last_output_tokens = Some(t.output); }
            }
            self.churn_dirty = true;  // churn accumulates per-turn token deltas
            ProjectionImpact::Economy
        }
        EventKind::Tasks { items } => {
            self.tasks = items.clone();
            ProjectionImpact::Economy
        }
        EventKind::WakeScheduled | EventKind::WakeFired | EventKind::WakeCancelled => {
            ProjectionImpact::Economy  // no-op
        }
        EventKind::TurnsDropped | EventKind::ContextMutation { .. }
        | EventKind::DirectiveReasserted | EventKind::TurnsReadmitted { .. } => {
            self.window_dirty = true;
            self.churn_dirty = true;
            ProjectionImpact::Economy
        }

        // Tier 2 — append-only msgs change (new ChatMsg pushed).
        EventKind::UserMessage { text } => {
            self.flush_pending_assistant();
            self.msgs.push(ChatMsg::User { text: text.clone(), ts: ev.ts });
            self.window_dirty = true;
            self.churn_dirty = true;
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }
        EventKind::AssistantMessage { text } => {
            self.flush_pending_assistant();
            self.msgs.push(ChatMsg::Assistant { thinking: None, text: text.clone(), tool_calls: Vec::new(), ts: ev.ts });
            self.window_dirty = true;
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }
        EventKind::ModelThinking { text } => {
            // flush_pending_assistant may push a ChatMsg::Assistant if there's
            // accumulated delta text/calls. If it did, msgs grew — return
            // MsgsMutated so the caller knows the body may need a re-render.
            // If the flush was a no-op (no pending turn), only stash the
            // thinking — no msgs change, return Economy.
            let flushed = self.flush_pending_assistant();
            self.pending_thinking = Some(text.clone());
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            if flushed { ProjectionImpact::MsgsMutated { mutated_index: None } } else { ProjectionImpact::Economy }
        }
        EventKind::ToolResult { id, name, output, is_error } => {
            self.flush_pending_assistant();
            // Suppress the tool-result line when a non-Approval QuestionAsked
            // owns this id — the card is the human-facing record (mirrors
            // conversation_for_branch:242–248). Approval-gate questions do
            // NOT suppress — the model needs the real tool output.
            if self.question_ids.contains(id.as_str()) {
                // The assistant turn that made the call(s) still ends here —
                // flush already handled it. No ChatMsg pushed.
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                return ProjectionImpact::MsgsMutated { mutated_index: None };
            }
            self.msgs.push(ChatMsg::ToolResult {
                id: id.clone(), name: name.clone(), output: output.clone(),
                is_error: *is_error, compacted: false, ts: ev.ts,
            });
            self.window_dirty = true;
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }
        EventKind::QuestionAsked { id, kind, question, choices } => {
            self.flush_pending_assistant();
            // Track non-Approval question ids so later ToolResults with the
            // same id are suppressed (mirrors conversation_for_branch:156–162).
            if !matches!(kind, crate::event::QuestionKind::Approval) {
                self.question_ids.insert(id.clone());
            }
            self.msgs.push(ChatMsg::Question {
                id: id.clone(), kind: kind.clone(), question: question.clone(),
                choices: choices.clone(),
                state: QuestionCardState::Open { selected: 0, free_text: String::new() },
                ts: ev.ts,
            });
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }
        EventKind::QuestionAnswered { id, answer } => {
            // Find the matching Question card and update its state in-place.
            // Same msg count, changed content → MsgsMutated with the index.
            // The caller checks if the index is the last message: if so, the
            // BodyCache incremental path re-renders it O(1); if not, the caller
            // invalidates body_cache for a full rebuild.
            if let Some((idx, ChatMsg::Question { state, .. })) =
                self.msgs.iter_mut().enumerate().rev().find(|(_, m)| matches!(m, ChatMsg::Question { id: qid, .. } if qid == id))
            {
                *state = QuestionCardState::Answered { answer: answer.clone() };
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsMutated { mutated_index: Some(idx) }
            } else {
                // Question not in msgs (partial resume / race) — no-op.
                // Return Economy so the caller doesn't trigger a spurious
                // body re-render. The next full refresh will handle it.
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
        }
        EventKind::DelegationResult { summary, ok, .. } => {
            self.flush_pending_assistant();
            self.msgs.push(ChatMsg::Delegated { summary: summary.clone(), ok: *ok });
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }
        EventKind::TurnsEvicted { reclaimed_tokens, marker, rescue } => {
            self.flush_pending_assistant();
            let evicted_topics = marker.spans.iter().map(|s| s.topic_hint.clone()).collect();
            let rescue = rescue.as_ref().map(|r| RescueSummary { /* ... */ });
            self.msgs.push(ChatMsg::Evicted { reclaimed_tokens: *reclaimed_tokens, evicted_topics, rescue, ts: ev.ts });
            self.window_dirty = true;
            self.events_len = Some(self.events_len.unwrap_or(0) + 1);
            ProjectionImpact::MsgsAppended
        }

        // Tier 3 — content mutation (existing msg changes, count unchanged).
        EventKind::ToolResultCompacted { id, summary, .. } => {
            if let Some((idx, ChatMsg::ToolResult { output, compacted, .. })) =
                self.msgs.iter_mut().enumerate().rev().find(|(_, m)| matches!(m, ChatMsg::ToolResult { id: rid, .. } if rid == id))
            {
                *output = summary.clone();
                *compacted = true;
                self.window_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsMutated { mutated_index: Some(idx) }
            } else {
                // Result not in msgs (partial resume) — no-op. The next full
                // refresh will handle it (the compacted map in the fold
                // catches it). Acknowledge: the incremental path does NOT
                // auto-trigger a full refresh for this edge case.
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
        }
    }
}
```

### Pending-assistant-turn state

`conversation_for_branch` maintains `text: Option<String>`,
`calls: Vec<ToolCallRef>`, `turn_ts: Option<i64>`, and
`pending_thinking: Option<String>` as it folds. The incremental `apply_event`
must maintain the same state so a `ToolResult` arriving after a `ModelDelta`
run correctly flushes the pending assistant turn.

New fields on `ProjectionCache`:

```rust
// Pending assistant-turn accumulator (mirrors conversation_for_branch locals).
pending_text: Option<String>,
pending_calls: Vec<ToolCallRef>,
pending_turn_ts: Option<i64>,
pending_thinking: Option<String>,
```

`flush_pending_assistant()` returns `true` if it pushed a `ChatMsg::Assistant`
onto `msgs` (i.e. `pending_text` or `pending_calls` was non-empty), `false`
otherwise. The pushed message carries `pending_thinking.take()` as its
`thinking` field — matching `conversation_for_branch`'s `flush()` closure
(projection.rs:178–194), which passes `pending_thinking.take()` into the
flushed message. After pushing, all four pending fields are cleared. This
ensures thinking is attached to the correct assistant turn, not lost on the
`ModelThinking` → `AssistantMessage` boundary.

`ModelDelta` appends to `pending_text` (and sets `pending_turn_ts`).
`ToolCall` appends to `pending_calls` (and sets `pending_turn_ts`).
All tier-2 events call `flush_pending_assistant()` before pushing their own
`ChatMsg`, using the return value to decide their `ProjectionImpact`.

### Trailing standalone-thinking flush

`conversation_for_branch` has a trailing flush after the main loop
(projection.rs:344–356): if `pending_thinking` is set and no text/calls
followed, it emits a standalone `ChatMsg::Assistant { text: "",
thinking: Some(...) }`. The incremental path has no event that triggers
this final emission — `pending_thinking` would stay stashed indefinitely.

**Fix:** the caller (main.rs event handler) calls a new
`ProjectionCache::finalize_pending()` after the agent turn ends (on
`AgentUpdate::TurnComplete` or equivalent — the `app.streaming = false`
transition). `finalize_pending()` checks if `pending_thinking` is set with
no pending text/calls; if so, it pushes the standalone thinking message and
returns `MsgsAppended`. If `pending_text`/`pending_calls` is non-empty, it
flushes normally (same as `flush_pending_assistant`). This mirrors the fold's
trailing flush and preserves parity.

### `refresh()` changes

```rust
fn refresh(&mut self, events: &EventLog) -> bool {
    if self.events_len == Some(events.len()) {
        // No full invalidation. Rebuild only dirty economy projections.
        let mut rebuilt = false;
        if self.window_dirty {
            self.window = context_window(events.iter());
            self.window_dirty = false;
            rebuilt = true;
        }
        if self.churn_dirty {
            self.churn = churn_timeline(events.iter());
            self.churn_dirty = false;
            rebuilt = true;
        }
        return rebuilt;
    }
    // Full invalidation (session resume, first frame, subagent branch).
    // Rebuild everything from scratch.
    self.msgs = conversation(events.iter());
    self.window = context_window(events.iter());
    self.churn = churn_timeline(events.iter());
    self.tasks = tasks(events.iter());
    let ledger = token_ledger(events.iter());
    self.ledger_total = ledger.total;
    self.cached_total = ledger.cached;
    self.thinking_total = ledger.thinking;
    self.last_input_tokens = /* ... */;
    self.last_output_tokens = /* ... */;
    self.window_dirty = false;
    self.churn_dirty = false;
    // Rebuild question_ids from the full log (non-Approval QuestionAsked ids).
    self.question_ids = events.iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAsked { id, kind, .. }
                if !matches!(kind, crate::event::QuestionKind::Approval) => Some(id.clone()),
            _ => None,
        })
        .collect();
    self.events_len = Some(events.len());
    // Reset pending-turn state — the full fold produced final msgs.
    self.pending_text = None;
    self.pending_calls = Vec::new();
    self.pending_turn_ts = None;
    self.pending_thinking = None;
    true
}
```

### Caller changes (`main.rs` event handling)

The `AgentUpdate::Appended` handler replaces the `apply_streaming` call:

```rust
let is_subagent_branch = ev.branch != BranchId::default();
if !is_subagent_branch {
    let impact = app.proj.apply_event(&ev);
    match impact {
        ProjectionImpact::Economy => {
            // msgs unchanged — body_cache NOT invalidated.
            // Economy projections will refresh on the next frame (dirty flags).
        }
        ProjectionImpact::MsgsMutated { mutated_index } => {
            // msgs content changed but count didn't. If the mutation is at
            // the last message, the BodyCache incremental path handles it
            // (O(1) re-render). If not, invalidate for a full rebuild —
            // the incremental path only re-renders the last message.
            match mutated_index {
                None | Some(i) if i == app.proj.msgs.len() - 1 => {
                    // Last-message mutation — BodyCache incremental path.
                    // Do NOT invalidate.
                }
                Some(_) => {
                    // Non-last mutation — full body rebuild needed.
                    app.body_cache.key = None;
                }
            }
        }
        ProjectionImpact::MsgsAppended => {
            // A new message was appended — body_cache must do a full rebuild.
            app.body_cache.key = None;
        }
        ProjectionImpact::FullRefresh => {
            // Should not happen (all kinds are handled), but defensive:
            app.proj.events_len = None;
            app.body_cache.key = None;
        }
    }
}
```

When `ProjectionImpact::Economy` is returned, `body_cache.key` is NOT set to
`None`. When `MsgsMutated` is returned with `mutated_index` at the last
message (or `None` for streaming appends), `body_cache.key` is also NOT set
to `None` — the `BodyCache` incremental path detects unchanged `msg_count` +
`structural_match` and re-renders just the last message. Only `MsgsAppended`
or a non-last `MsgsMutated` forces a full body rebuild.

### What `events_len` means now

`events_len` is still updated on every applied event (incremented by 1). It
still serves as the full-invalidation guard: if someone sets it to `None`
(only happens on session resume / subagent branch edge cases), the next
`refresh()` does a full rebuild. The dirty flags handle the common case
(economy-only refresh without touching `msgs`).

### Edge case — `ToolResultCompacted` arriving before `ToolResult`

The `ToolResultCompacted` event references a tool-result id. During normal
streaming, the `ToolResult` always arrives first (the tool runs, produces
output, then ACM compacts it). If a compacted event arrives and no matching
`ChatMsg::ToolResult` exists in `msgs` (e.g. on a partial resume where the
log starts mid-stream), `apply_event` finds no match, returns `Economy`, and
does nothing. The incremental path does NOT auto-trigger a full refresh for
this case — the stale state persists until some other event sets
`events_len = None`. This is an accepted limitation: partial resume is rare,
and the next `UserMessage` or session restart will trigger a full refresh
that corrects it via the `compacted` map in `conversation_for_branch`.

### Edge case — `QuestionAnswered` for a question not in `msgs`

`apply_event` scans `msgs` for the matching id; if not found, it returns
`Economy` (no-op — no spurious body re-render). The next full `refresh()`
will handle it (the `answered` map in `conversation_for_branch` catches it).

---

## 4. Performance impact

### Part 1 (peek removal)

Eliminates the per-frame `peek_hits()` call — the primary bottleneck. During
streaming at 30 FPS, this was a full `build_conversation()` over ~7,121
messages with markdown parsing and line wrapping. Cost goes from O(n) per
frame to zero.

### Part 2 (incremental projection)

| Scenario | Before | After |
|----------|--------|-------|
| `Usage` event during streaming | Full O(225K) rebuild of 5 projections + O(7K) body rebuild | O(1) ledger update; body untouched; economy deferred to next frame |
| `Tasks` event | Full O(225K) rebuild (including `tasks()` which collects 225K events into a Vec and reverses) | O(1) replace |
| `ToolResult` event | Full O(225K) rebuild of 5 projections + O(7K) body rebuild | O(1) append to `msgs`; body rebuilds (unavoidable — new message); economy deferred |
| `ToolResultCompacted` event | Full O(225K) rebuild + O(7K) body rebuild | O(1) in-place mutation; body incremental (O(1) if last msg); economy deferred |
| `ModelDelta` event | O(1) (already incremental) | O(1) (unchanged) — returns `MsgsMutated`, body NOT invalidated |
| `Wake*` event | Full O(225K) rebuild | No-op |
| `TurnsEvicted` event | Full O(225K) rebuild + O(7K) body rebuild | O(1) append to `msgs`; body rebuilds; economy deferred |

The body rebuild on `MsgsChanged` is unavoidable when a new message is
appended (line positions shift). But the O(225K) `conversation()` fold is
eliminated for every structural event — that's the dominant cost per event.
The economy projections (`window`, `churn`) still do a full O(n) rebuild when
dirty, but at most once per frame instead of once per event. With 100–700
structural events per turn, that's a 100–700× reduction in economy rebuilds.

---

## 5. Testing

### Part 1 — peek removal

- All existing peek tests are deleted (they test removed code).
- Existing `build_conversation` tests that don't reference peek continue to
  pass (the `peek_hits` parameter is removed, but tests that don't use it are
  unaffected — they pass `&mut Vec::new()` which is now gone, so call sites
  are updated).
- Snapshot tests: any snapshot that includes `⏎ peek` hint text is updated
  via `cargo insta test --accept`. The change is a visual diff (hint text
  removed from tool-call and delegated lines), verified to be
  hint-removal-only.

### Part 2 — incremental projection

- **Parity test**: for every event kind, assert that `apply_event` followed
  by a dirty-flag refresh produces the same `msgs`, `window`, `churn`,
  `tasks`, `ledger_total`, `cached_total`, `thinking_total`,
  `last_input_tokens`, and `last_output_tokens` as a full `refresh()` from
  scratch. This is the critical correctness invariant: incremental == full.
- **Tier classification test**: for each event kind, assert that `apply_event`
  returns the expected `ProjectionImpact` variant (`Economy`, `MsgsMutated`,
  `MsgsAppended`, or `FullRefresh`).
- **Bookkeeping no-touch test**: after applying a `Usage` event via
  `apply_event`, assert `msgs` is unchanged and the caller does NOT
  invalidate `body_cache` (returns `Economy`).
- **Streaming no-touch test**: after applying a `ModelDelta` event via
  `apply_event`, assert it returns `MsgsMutated { mutated_index: None }` (NOT
  `MsgsAppended`), so the caller does NOT set `body_cache.key = None`.
- **ModelThinking flush test**: apply `ModelDelta` then `ModelThinking` via
  `apply_event`. Assert `ModelThinking` returns `MsgsMutated` (not `Economy`)
  because `flush_pending_assistant` pushed a `ChatMsg::Assistant` with the
  delta text. Also assert the pushed message carries the thinking text from
  a *prior* `ModelThinking` (if any) in its `thinking` field.
- **ModelThinking no-op test**: apply `ModelThinking` with no prior
  `ModelDelta`/`ToolCall`. Assert it returns `Economy` (flush was a no-op,
  no msgs change).
- **Trailing-thinking finalize test**: apply `ModelThinking` as the last
  event, then call `finalize_pending()`. Assert a standalone
  `ChatMsg::Assistant { text: "", thinking: Some(...) }` was pushed onto
  `msgs` and `finalize_pending` returned `MsgsAppended`.
- **ToolResult suppression test**: apply `QuestionAsked` (non-`Approval`
  kind) then `ToolResult` with the same id via `apply_event`. Assert no
  `ChatMsg::ToolResult` was pushed (suppressed). Then apply `QuestionAsked`
  with `Approval` kind and a `ToolResult` with the same id — assert the
  `ToolResult` IS pushed (Approval does not suppress).
- **Pending-turn flush test**: apply `ModelDelta` then `ToolResult` via
  `apply_event` and assert the assistant turn was flushed (a
  `ChatMsg::Assistant` appears before the `ChatMsg::ToolResult` in `msgs`).
- **Compaction in-place test**: apply `ToolResult` then
  `ToolResultCompacted` via `apply_event` and assert the existing
  `ChatMsg::ToolResult` has `compacted == true` and `output == summary`
  (no new msg pushed; returns `MsgsMutated { mutated_index: Some(idx) }`).
- **Compaction non-last mutation test**: apply `ToolResult`, then
  `AssistantMessage`, then `ToolResultCompacted` for the first result. Assert
  `mutated_index` is `Some(0)` (not the last message), so the caller
  invalidates `body_cache` for a full rebuild.
- **QuestionAnswered in-place test**: apply `QuestionAsked` then
  `QuestionAnswered` via `apply_event` and assert the existing
  `ChatMsg::Question` has `state == Answered` (no new msg pushed; returns
  `MsgsMutated { mutated_index: Some(idx) }`).
- **QuestionAnswered miss test**: apply `QuestionAnswered` with no prior
  `QuestionAsked`. Assert it returns `Economy` (no spurious body re-render).
- **Thinking-token accumulation test**: apply a `Usage` event with
  `tokens.thinking > 0` via `apply_event`. Assert `thinking_total` was
  incremented. Assert the full `refresh()` produces the same `thinking_total`.
- **Parity test thinking field**: the parity test (spec line 583) must also
  assert `thinking_total` matches the full `token_ledger` output, not just
  `ledger_total`/`cached_total`.
- **Dirty-flag refresh test**: apply a `Usage` event (sets `churn_dirty`),
  call `refresh()`, assert `churn` was rebuilt and `churn_dirty` is cleared,
  but `msgs` and `window` were NOT rebuilt.
- **Full invalidation test**: set `events_len = None`, call `refresh()`,
  assert all projections rebuilt and dirty flags cleared.

---

## 6. Out of scope

- **Incrementalizing `context_window` and `churn_timeline`**: these maintain
  cross-event state that's hard to incrementalize correctly. The dirty-flag
  deferral (rebuild at most once per frame) captures most of the win. Full
  incrementalization is a future optimization if the once-per-frame rebuild
  is still too slow.
- **The `tasks()` function's O(n) Vec + reverse**: replaced by O(1)
  last-write-wins in `apply_event`, but the full `refresh()` path still
  calls `tasks()` (O(n)). This is only hit on full invalidation (session
  resume), not per-event.
- **Parallelizing projections**: the existing
  `2026-07-26-parallelize-projections-design.md` is complementary — it
  speeds up the full `refresh()` path, while this spec avoids calling it
  unnecessarily.