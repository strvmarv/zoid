# Peek Removal & Incremental Projection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the per-frame O(n) `peek_hits()` call and the full O(225K) projection rebuilds on bookkeeping events that make large sessions sluggish.

**Architecture:** Part 1 deletes the entire peek feature (7 files, ~17 tests) — the popup, routing, hit-testing, cache, and `⏎ peek` hint text. Part 2 replaces `apply_streaming` with `apply_event` returning a 4-variant `ProjectionImpact` enum, splitting events into bookkeeping (no msgs change), append (new ChatMsg), and content-mutation (existing msg changed in-place) tiers. Economy projections use dirty flags (rebuild at most once per frame).

**Tech Stack:** Rust workspace (`zoid-core` pure projections; `zoid-tui` pure renderer; `zoid` bin with render loop). `ratatui` for TUI. `rusqlite` for persistence. `ulid` for ids.

**Spec:** `docs/superpowers/specs/2026-07-26-peek-removal-and-incremental-projection-design.md`

## Global Constraints

- **Every task ends with `cargo build --workspace` succeeding** — the changes are cross-crate. Tasks 1–4 are sequenced to minimize compile errors (each removes references exposed by the previous task's deletions), but only Task 5 achieves a clean workspace build. Intermediate commits (T1–T4) may not compile — this is expected and acceptable.
- **Every task runs `cargo test --workspace --no-fail-fast`** (the release gate from AGENTS.md §4, minus the `local-embed` feature which is not relevant here).
- **No co-author trailer** in commits (repo `CLAUDE.md`).
- **TDD:** write the failing test first, verify it fails, implement, verify it passes, commit.
- **Snapshot tests:** any snapshot containing `⏎ peek` hint text is updated via `cargo insta test --accept` — verify the diff is hint-removal-only.
- **Do not delete `PeekState`/`PeekContent`/`PeekHit`/`PeekKind` until all references are removed** — Rust's compiler will guide the removal order, but the plan sequences it to minimize compile errors between commits.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tui/src/chat.rs` | `build_conversation`, `peek_hits`, `PeekHit`, `PeekKind` | Remove peek types, `peek_hits()` fn, `peek_hits` param from `build_conversation`, `⏎ peek` hint strings, 4 tests |
| `crates/zoid-tui/src/state.rs` | `PeekState`, `PeekContent`, `ShellState.peek` | Remove peek types, `peek` field, 3 tests |
| `crates/zoid-tui/src/render.rs` | `render_peek_overlay`, peek draw block | Remove function + call site |
| `crates/zoid-tui/src/layout.rs` | `ShellLayout.peek`, peek rect computation | Remove field + computation, 2 tests |
| `crates/zoid-tui/src/route.rs` | `Action::DismissPeek`/`ScrollPeek`/`PeekClick`, routing | Remove variants + routing blocks, 5 tests |
| `crates/zoid/src/main.rs` | `PeekCache`, per-frame `peek_hits` call, peek handlers, `ProjectionCache`, `apply_streaming` | Remove peek cache + call + handlers + 3 tests; replace `apply_streaming` with `apply_event` + `ProjectionImpact` |
| `crates/zoid-core/src/projection.rs` | `conversation_for_branch` fold | No change (reference implementation — the incremental path must match it) |

**Task order & dependency:** Tasks 1–5 are the peek removal (Part 1), sequenced to keep the workspace compiling between commits: TUI types first (state, layout, route, chat), then the bin. Task 6 is the incremental projection (Part 2), independent of Part 1. Task 7 is the per-frame `peek_hits` call removal (depends on T1–T5 for the `peek_hits` function to be gone, and T6 for the new `apply_event` to be in place).

---

### Task 1: Remove peek from `state.rs` (types + field + tests)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (delete `PeekState` at :399–421, `peek` field at :533, `peek: None` init at :711, 3 tests at :1393–1428)

**Interfaces:**
- Produces: `ShellState` without the `peek` field. All downstream consumers (render, layout, route, main) will fail to compile until they stop referencing `state.peek` — those are fixed in Tasks 2–5.

- [ ] **Step 1: Delete the peek types, field, and tests**

In `crates/zoid-tui/src/state.rs`:

1. Delete the `PeekState` struct (lines 399–403) and `PeekContent` enum (lines 405–421) — the entire block from the `/// State for the peek popup` doc comment through the closing `}` of `PeekContent`.

2. Delete the `pub peek: Option<PeekState>` field from `ShellState` (line 533).

3. Delete the `peek: None,` line from the `Default` impl (line 711).

4. Delete the 3 tests: `peek_is_none_by_default` (line 1393), `peek_set_and_clear` (line 1399), `peek_included_in_equality` (line 1417).

- [ ] **Step 2: Verify the crate fails to compile (expected — downstream references)**

Run: `cargo build -p zoid-tui 2>&1 | head -30`
Expected: FAIL — multiple `peek` / `PeekState` / `PeekContent` references in `render.rs`, `layout.rs`, `route.rs`. These are fixed in Tasks 2–4.

- [ ] **Step 3: Commit (workspace won't fully build yet, but the TUI crate change is self-contained)**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "refactor(tui): remove PeekState/PeekContent types and peek field from ShellState"
```

---

### Task 2: Remove peek from `layout.rs` (field + computation + tests)

**Files:**
- Modify: `crates/zoid-tui/src/layout.rs` (delete `peek` field at :95–97, peek computation at :225–230, `peek` in struct init at :242, 2 tests at :453–469)

**Interfaces:**
- Consumes: `ShellState` without `peek` (from Task 1).
- Produces: `ShellLayout` without the `peek` field.

- [ ] **Step 1: Delete the peek field, computation, and tests**

In `crates/zoid-tui/src/layout.rs`:

1. Delete the `peek` field from `ShellLayout` (lines 95–97 — the doc comment + `pub peek: Option<Rect>`).

2. Delete the peek computation block (lines 225–230):
   ```rust
   let peek = if state.peek.is_some() {
       let max_h = (conversation.height as f32 * 0.65).floor() as u16;
       Some(centered(conversation, conversation.width, max_h))
   } else {
       None
   };
   ```

3. Delete the `peek,` line from the `ShellLayout { ... }` construction (line 242).

4. Delete the 2 tests: `peek_rect_none_when_peek_closed` (line 453), `peek_rect_some_when_peek_open` (line 460).

- [ ] **Step 2: Verify the TUI crate compiles**

Run: `cargo build -p zoid-tui 2>&1 | head -20`
Expected: Fewer errors than Task 1 (layout no longer references `state.peek` or `ShellLayout.peek`). Still fails on `render.rs` and `route.rs` references.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/layout.rs
git commit -m "refactor(tui): remove peek rect from ShellLayout"
```

---

### Task 3: Remove peek from `route.rs` (action variants + routing + tests)

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (delete `DismissPeek`/`ScrollPeek`/`PeekClick` at :44–50, key routing at :238–246, mouse routing at :619–626, 5 tests at :1854–1940)

**Interfaces:**
- Consumes: `ShellState` without `peek` (from Task 1).
- Produces: `Action` enum without `DismissPeek`/`ScrollPeek`/`PeekClick`.

- [ ] **Step 1: Delete the peek action variants, routing blocks, and tests**

In `crates/zoid-tui/src/route.rs`:

1. Delete the 3 `Action` variants (lines 44–50):
   ```rust
   /// Dismiss the peek popup (Esc or click-away).
   DismissPeek,
   /// Scroll the peek popup content by delta lines (positive = down).
   ScrollPeek(i32),
   /// A mouse click while the peek popup is open. The bin tests whether (row, col)
   /// falls inside the popup rect: if outside, dismiss; if inside, no-op.
   PeekClick(u16, u16),
   ```

2. Delete the key-routing block for an open peek (lines 238–246):
   ```rust
   // 0.5. An open peek popup captures Esc and scroll keys.
   if state.peek.is_some() {
       match key.code {
           KeyCode::Esc => return Action::DismissPeek,
           KeyCode::Down | KeyCode::PageDown => return Action::ScrollPeek(1),
           KeyCode::Up | KeyCode::PageUp => return Action::ScrollPeek(-1),
           _ => return Action::Noop,
       }
   }
   ```

3. Delete the mouse-routing block for an open peek (lines 619–626):
   ```rust
   // An open peek popup captures mouse input: scroll scrolls the popup
   // content; a click is resolved by the bin (inside = no-op, outside =
   // dismiss).
   if state.peek.is_some() {
       return match m.kind {
           MouseEventKind::ScrollDown => Action::ScrollPeek(1),
           MouseEventKind::ScrollUp => Action::ScrollPeek(-1),
           MouseEventKind::Down(MouseButton::Left) => Action::PeekClick(m.row, m.column),
           _ => Action::Noop,
       };
   }
   ```

4. Delete the 6 tests: `peek_open_esc_dismisses` (line 1854), `peek_open_arrows_scroll` (line 1865), `peek_open_other_keys_are_noop` (line 1877), `peek_open_mouse_scroll_scrolls_peek` (line 1891), `peek_open_mouse_click_returns_peek_click` (line 1909), `peek_closed_mouse_behaves_normally` (line 1928).

- [ ] **Step 2: Verify the TUI crate compiles**

Run: `cargo build -p zoid-tui 2>&1 | head -20`
Expected: Fewer errors. Still fails on `render.rs` references to `layout.peek` and `state.peek`.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/route.rs
git commit -m "refactor(tui): remove peek action variants and routing"
```

---

### Task 4: Remove peek from `chat.rs` (types + `peek_hits` fn + param + hints + tests)

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (delete `PeekHit` at :41–58, `peek_hits()` at :177–203, `peek_hits` param from `build_conversation` at :228, 2 `peek_hits.push` sites at :334 and :450, `⏎ peek` hint strings, 4 tests at :2067–2135)

**Interfaces:**
- Produces: `build_conversation` without the `peek_hits` parameter. All call sites (4 in chat.rs, 1 in main.rs via `peek_hits()`) must be updated.

- [ ] **Step 1: Delete the peek types, function, parameter, hint strings, and tests**

In `crates/zoid-tui/src/chat.rs`:

1. Delete `PeekHit` struct (lines 41–44) and `PeekKind` enum (lines 48–58) — the entire block from `/// A clickable tool-call line or delegated chip` through the closing `}` of `PeekKind`.

2. Delete the `peek_hits()` function (lines 177–203) — the entire `pub fn peek_hits(...)` block.

3. Remove the `peek_hits: &mut Vec<PeekHit>` parameter from `build_conversation` (line 228).

4. Remove the `peek_hits.push(PeekHit { ... })` block at the tool-call line (after line 333, the `⏎ peek` hint span and the push):
   ```rust
   Span::styled(
       format!(" {} peek", glyph::RETURN),
       Style::new().fg(color::DIM),
   ),
   ```
   Delete this span from the `lines.push(Line::from(vec![...]))` at the tool-call rendering. Also delete the `peek_hits.push(PeekHit { line: lines.len() - 1, kind: PeekKind::ToolCall { ... } })` block immediately after.

5. Remove the `⏎ peek` hint span and `peek_hits.push` from the delegated chip rendering (around line 446–456):
   ```rust
   Span::styled(
       format!("{} peek", glyph::RETURN),
       Style::new().fg(color::DIM),
   ),
   ```
   Delete this span from the delegated chip `Line::from(vec![...])`. Also delete the `peek_hits.push(PeekHit { line: lines.len() - 1, kind: PeekKind::Delegated { ... } })` block.

6. Update all 4 `build_conversation` call sites in chat.rs to remove the `peek_hits` argument:
   - Line 94 (`conversation_lines`): remove the last `&mut Vec::new()` argument.
   - Line 124 (`conversation_lines_with_diffs`): remove the last `&mut Vec::new()` argument.
   - Line 155 (`question_choices` function): remove the last `&mut Vec::new()` argument.
   - Line 186 (`peek_hits` function — being deleted, so this whole function goes away).
   - Line 845 (`conversation_view_indexed`): remove the `&mut Vec::new()` argument (the 5th `&mut` — currently `&mut Vec::new()` for peek_hits).

7. Delete the 4 tests: `peek_hits_finds_tool_call_line` (line 2067), `peek_hits_finds_delegated_chip` (line 2094), `peek_hits_empty_for_prose_only` (line 2108), `peek_hits_multiple_tool_calls_each_get_own_hit` (line 2123).

- [ ] **Step 2: Verify the TUI crate compiles**

Run: `cargo build -p zoid-tui 2>&1 | head -20`
Expected: Fewer errors. `render.rs` still references `layout.peek` (fixed in Task 5). The `peek_hits` function is gone, so `main.rs` will fail — fixed in Task 5/7.

- [ ] **Step 3: Accept updated snapshots**

Run: `cargo test -p zoid-tui -- --nocapture 2>&1 | grep -i snapshot`
Expected: Some snapshot tests fail because the `⏎ peek` hint text is gone from tool-call and delegated lines.

Run: `cargo insta test --accept -p zoid-tui`
Expected: Snapshots updated. Review the diff to confirm only `⏎ peek` hint text was removed — no other visual changes.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-tui/src/snapshots/
git commit -m "refactor(tui): remove PeekHit/PeekKind types, peek_hits fn, peek hints from transcript"
```

---

### Task 5: Remove peek from `render.rs` and `main.rs` (overlay + cache + handlers + tests)

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (delete `render_peek_overlay` at :1110–1204, peek draw block at :258–260)
- Modify: `crates/zoid/src/main.rs` (delete `PeekCache` at :1589–1591, `peek_cache` field at :1776, per-frame `peek_hits` call at :2747–2765, `PeekClick` mouse handler at :2985–2991, `DismissPeek`/`ScrollPeek`/`PeekClick` action arms at :5048–5062, `handle_conversation_click` peek-open logic at :1019–1056, `peek_cache` inits at :2334 and :7811, 3 tests at :10042–10120)

**Interfaces:**
- Consumes: `Action` without peek variants (Task 3), `ShellLayout` without `peek` (Task 2), `ShellState` without `peek` (Task 1), `chat` without `peek_hits` (Task 4).
- Produces: A workspace that compiles and passes all tests with peek fully removed.

- [ ] **Step 1: Delete `render_peek_overlay` and the peek draw block from `render.rs`**

In `crates/zoid-tui/src/render.rs`:

1. Delete the peek draw block (lines 258–260):
   ```rust
   // Peek popup — drawn last so it sits on top of everything.
   if let Some(p) = layout.peek {
       render_peek_overlay(frame, state, p);
   }
   ```

2. Delete the `render_peek_overlay` function (lines 1110–1214) — the entire `fn render_peek_overlay(...)` block.

- [ ] **Step 2: Delete peek from `main.rs` — cache, field, handlers, click logic, tests**

In `crates/zoid/src/main.rs`:

1. Delete the `PeekCache` struct (lines 1586–1591):
   ```rust
   /// Cached peek hits from the last painted frame. ...
   struct PeekCache {
       hits: Vec<zoid_tui::chat::PeekHit>,
   }
   ```

2. Delete the `peek_cache: PeekCache,` field from `App` (line 1776).

3. Delete the per-frame `peek_hits` call block (lines 2747–2765):
   ```rust
   // Cache peek hits for click hit-testing ...
   let body_rebuilt = !matches!(cache_hit, Some(true));
   if body_rebuilt || app.streaming {
       app.peek_cache = PeekCache {
           hits: zoid_tui::chat::peek_hits(
               &app.proj.msgs,
               app.streaming,
               true,
               app.tz_offset_secs,
               body_w,
               None,
           ),
       };
   }
   ```
   Also delete the `let body_rebuilt = ...` line (it was only used for the peek cache guard).

4. Delete the `PeekClick` mouse handler (lines 2985–2991):
   ```rust
   zoid_tui::route::Action::PeekClick(row, col) => {
       if let Some(p) = layout.peek {
           if !zoid_tui::layout::in_rect(p, col, row) {
               app.shell.peek = None;
           }
       }
   }
   ```
   This is a match arm inside the mouse event handler — remove the entire arm. The `action => { ... }` catch-all below it remains.

5. Delete the 3 action handler arms (lines 5048–5062):
   ```rust
   Action::DismissPeek => {
       app.shell.peek = None;
   }
   Action::ScrollPeek(delta) => {
       if let Some(ps) = &mut app.shell.peek {
           if delta > 0 {
               ps.scroll = ps.scroll.saturating_add(delta as usize);
           } else {
               ps.scroll = ps.scroll.saturating_sub((-delta) as usize);
           }
       }
   }
   Action::PeekClick(_, _) => {
       // Resolved in the mouse event handler (needs layout rect).
   }
   ```

6. Delete the peek-open logic from `handle_conversation_click` (lines 1019–1056) — the entire block from `// Peek hits — clicking a tool-call line or delegated chip opens a popup.` through the closing `}` of the `if let Some(hit) = peeks.iter().find(...)` block.

7. Delete `peek_cache: PeekCache { hits: Vec::new() },` from all `App` construction sites (lines 2334 and 7811).

8. Delete the 3 tests: `conversation_click_on_tool_call_opens_peek` (line 10042), `dismiss_peek_clears_state` (line 10095), `scroll_peek_adjusts_scroll` (line 10109).

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: SUCCESS — all peek references are gone.

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: PASS — all remaining tests pass (peek tests are deleted, no other tests reference peek).

- [ ] **Step 5: Accept any remaining snapshot updates**

Run: `cargo insta test --accept --workspace`
Expected: Any remaining snapshots with `⏎ peek` text are updated. Review diffs.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid/src/main.rs crates/zoid-tui/src/snapshots/ crates/zoid/src/snapshots/
git commit -m "refactor: remove peek overlay, cache, handlers, and click logic"
```

---

### Task 6: Replace `apply_streaming` with `apply_event` + `ProjectionImpact`

**Files:**
- Modify: `crates/zoid/src/main.rs` — `ProjectionCache` struct (add fields), `apply_streaming` method (replace with `apply_event`), `refresh` method (add dirty-flag path), `AgentUpdate::Appended` handler (use `apply_event` + `ProjectionImpact`)

**Interfaces:**
- Consumes: `EventKind` (all 20 variants), `ChatMsg`, `ToolCallRef`, `QuestionCardState`, `RescueSummary`, `RescuedTurnSummary`, `TokenStat`, `QuestionKind` — all from `zoid-core`.
- Produces: `ProjectionImpact` enum, `ProjectionCache::apply_event(&mut self, ev: &Event) -> ProjectionImpact`, `ProjectionCache::finalize_pending(&mut self) -> ProjectionImpact`, `ProjectionCache::refresh` with dirty-flag path.

- [ ] **Step 1: Write the failing parity test**

Add to the `#[cfg(test)]` module in `crates/zoid/src/main.rs`, near the existing `projection_cache_recomputes_only_on_len_change` test (line 7945):

```rust
#[test]
fn apply_event_parity_with_full_refresh() {
    use zoid_core::event::{Event, EventKind, TokenStat};
    use ulid::Ulid;

    // Build a realistic event sequence covering every tier:
    // UserMessage, ModelDelta, ModelThinking, ToolCall, ToolResult,
    // Usage, QuestionAsked, QuestionAnswered, DelegationResult,
    // TurnsEvicted, ToolResultCompacted, Tasks, WakeScheduled.
    let mut events: Vec<Event> = Vec::new();
    let mut ts = 1000i64;
    let mut mk = |kind: EventKind| {
        let e = Event::new(Ulid::new(), None, ts, kind);
        ts += 1;
        e
    };

    events.push(mk(EventKind::UserMessage { text: "hello".into() }));
    events.push(mk(EventKind::ModelDelta { text: "res".into() }));
    events.push(mk(EventKind::ModelThinking { text: "hmm".into() }));
    events.push(mk(EventKind::ToolCall { id: "t1".into(), name: "read".into(), args: r#"{"path":"f.rs"}"#.into() }));
    // ToolResult with tokens
    let mut tr = mk(EventKind::ToolResult { id: "t1".into(), name: "read".into(), output: "file contents".into(), is_error: false });
    tr.tokens = Some(TokenStat { input: 100, output: 50, cached: 20, thinking: 5 });
    events.push(tr);
    events.push(mk(EventKind::Usage));
    events.push(mk(EventKind::QuestionAsked { id: "q1".into(), kind: zoid_core::event::QuestionKind::Ask, question: "which?".into(), choices: vec!["a".into(), "b".into()] }));
    events.push(mk(EventKind::QuestionAnswered { id: "q1".into(), answer: "a".into() }));
    events.push(mk(EventKind::AssistantMessage { text: "final answer".into() }));
    events.push(mk(EventKind::DelegationResult { subagent_id: "s1".into(), branch: "subagent:s1".into(), summary: "done".into(), ok: true }));
    events.push(mk(EventKind::ToolResultCompacted { id: "t1".into(), summary: "compacted summary".into(), original_tokens: 500 }));
    events.push(mk(EventKind::Tasks { items: vec![] }));
    events.push(mk(EventKind::WakeScheduled { wake_id: "w1".into(), fire_at_ms: 99999, note: "reminder".into() }));
    events.push(mk(EventKind::WakeFired { wake_id: "w1".into() }));
    events.push(mk(EventKind::WakeCancelled { wake_id: "w2".into() }));
    events.push(mk(EventKind::TurnsDropped { turns_dropped: 1 }));
    events.push(mk(EventKind::ContextMutation { item: "msg:0".into(), op: zoid_core::event::MutationOp::Pin }));
    events.push(mk(EventKind::DirectiveReasserted { at_cumulative: 500 }));
    events.push(mk(EventKind::TurnsReadmitted { ids: vec![Ulid::from(42u128)] }));
    events.push(mk(EventKind::TurnsEvicted {
        ids: vec![Ulid::from(1u128)],
        reclaimed_tokens: 1000,
        marker: zoid_core::event::EvictionMarker { spans: vec![zoid_core::event::EvictedSpan { token_estimate: 500, topic_hint: "topic".into() }] },
        rescue: None,
    }));

    let log = zoid::eventlog::EventLog::from_vec(events.clone());

    // Full refresh from scratch (the reference).
    let mut full = ProjectionCache::default();
    full.refresh(&log);

    // Incremental: apply each event one at a time, then refresh dirty flags.
    let mut incr = ProjectionCache::default();
    for ev in &events {
        let _ = incr.apply_event(ev);
    }
    // Flush dirty economy projections.
    incr.refresh(&log);

    // Parity: msgs
    assert_eq!(incr.msgs, full.msgs, "msgs must match");
    // Parity: ledger
    assert_eq!(incr.ledger_total, full.ledger_total, "ledger_total");
    assert_eq!(incr.cached_total, full.cached_total, "cached_total");
    assert_eq!(incr.thinking_total, full.thinking_total, "thinking_total");
    // Parity: last tokens
    assert_eq!(incr.last_input_tokens, full.last_input_tokens, "last_input_tokens");
    assert_eq!(incr.last_output_tokens, full.last_output_tokens, "last_output_tokens");
    // Parity: tasks
    assert_eq!(incr.tasks, full.tasks, "tasks");
    // Parity: window + churn (rebuilt from dirty flags)
    assert_eq!(incr.window, full.window, "context window");
    assert_eq!(incr.churn, full.churn, "churn timeline");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid -- apply_event_parity 2>&1 | tail -10`
Expected: FAIL — `apply_event` method not found, `thinking_total` field not found, `ProjectionImpact` type not found.

- [ ] **Step 3: Add the `ProjectionImpact` enum and new `ProjectionCache` fields**

In `crates/zoid/src/main.rs`, above the `ProjectionCache` struct (line 1408):

```rust
/// What `apply_event` changed. Determines whether the caller invalidates
/// `body_cache` and whether the economy projections need a dirty-flag refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionImpact {
    /// No `msgs` change, no body change. Economy projections may need refresh.
    Economy,
    /// `msgs` content changed but `msg_count` did not. Carries the index of
    /// the mutated message. `None` means "mutation at the end" (streaming
    /// append to the last message — BodyCache incremental path handles it).
    /// `Some(i)` means message at index `i` was mutated — the caller checks
    /// `i == msgs.len() - 1` to decide whether to invalidate `body_cache`.
    MsgsMutated { mutated_index: Option<usize> },
    /// A new ChatMsg was appended (msg_count changed). Full body rebuild.
    MsgsAppended,
    /// Could not apply incrementally — caller must do a full refresh.
    FullRefresh,
}
```

Add the new fields to `ProjectionCache` (after the existing fields, before the `impl` block):

```rust
    // NEW — dirty flags for deferred economy rebuilds.
    window_dirty: bool,
    churn_dirty: bool,
    // NEW — ids of non-Approval QuestionAsked events, so ToolResults with
    // the same id are suppressed (mirrors conversation_for_branch's pre-pass).
    question_ids: std::collections::HashSet<String>,
    // NEW — cumulative thinking tokens (maintained incrementally by Usage).
    thinking_total: u64,
    // NEW — pending assistant-turn accumulator (mirrors conversation_for_branch
    // locals). ModelDelta/ToolCall accumulate here; tier-2 events flush.
    pending_text: Option<String>,
    pending_calls: Vec<zoid_core::projection::ToolCallRef>,
    pending_turn_ts: Option<i64>,
    pending_thinking: Option<String>,
```

- [ ] **Step 4: Implement `flush_pending_assistant` and `apply_event`**

Replace the `apply_streaming` method (lines 1462–1498) with:

```rust
    /// Flush the pending assistant turn into `msgs` as a `ChatMsg::Assistant`.
    /// Returns `true` if a message was pushed (pending text/calls were
    /// non-empty), `false` otherwise. Carries `pending_thinking` into the
    /// flushed message, matching `conversation_for_branch`'s `flush()`.
    ///
    /// When the flush is a no-op (no pending text/calls), `pending_thinking`
    /// is **dropped** (not put back) — matching the reference fold, where
    /// `flush()` takes `thinking` by value and it goes out of scope when
    /// the flush doesn't push. The `ModelThinking` handler re-stashes
    /// thinking immediately after calling flush, so there's no risk of
    /// losing it on the `ModelThinking` path.
    fn flush_pending_assistant(&mut self) -> bool {
        let text = self.pending_text.take();
        let calls = std::mem::take(&mut self.pending_calls);
        let ts = self.pending_turn_ts.take();
        let thinking = self.pending_thinking.take();
        if text.is_some() || !calls.is_empty() {
            self.msgs.push(zoid_core::projection::ChatMsg::Assistant {
                text: text.unwrap_or_default(),
                tool_calls: calls,
                ts: ts.unwrap_or(0),
                thinking,
            });
            true
        } else {
            // No pending turn — thinking is dropped (matches reference fold).
            false
        }
    }

    /// Finalize any pending state after a turn ends. Mirrors the trailing
    /// flush in `conversation_for_branch` (projection.rs:344–356): if
    /// `pending_thinking` is set with no pending text/calls, emit a
    /// standalone `ChatMsg::Assistant { text: "", thinking: Some(...) }`.
    fn finalize_pending(&mut self) -> ProjectionImpact {
        let flushed = self.flush_pending_assistant();
        if flushed {
            return ProjectionImpact::MsgsAppended;
        }
        // Trailing standalone-thinking: no text/calls, but thinking is set.
        if let Some(thinking) = self.pending_thinking.take() {
            self.msgs.push(zoid_core::projection::ChatMsg::Assistant {
                text: String::new(),
                tool_calls: Vec::new(),
                ts: self.pending_turn_ts.take().unwrap_or(0),
                thinking: Some(thinking),
            });
            return ProjectionImpact::MsgsAppended;
        }
        ProjectionImpact::Economy
    }

    /// Incrementally apply a single new event to the cached projections.
    /// Returns a `ProjectionImpact` describing what changed, so the caller
    /// knows whether to invalidate `body_cache`.
    fn apply_event(&mut self, ev: &Event) -> ProjectionImpact {
        use zoid_core::event::EventKind;
        use zoid_core::projection::{ChatMsg, ToolCallRef, QuestionCardState, RescueSummary, RescuedTurnSummary};
        let bump_len = || ProjectionImpact::MsgsMutated { mutated_index: None };
        match &ev.kind {
            // Streaming hot path — ALWAYS accumulate into pending_text/
            // pending_calls, never append to the last assistant message.
            // This matches the reference fold (projection.rs:224–235), which
            // always accumulates into locals and only emits a ChatMsg::Assistant
            // on flush. Appending to the last assistant would diverge after any
            // event that pushes a new Assistant (ModelThinking, AssistantMessage,
            // finalize_pending) — the fold starts a fresh pending turn, while
            // append-to-last would mutate the just-pushed message.
            EventKind::ModelDelta { text } => {
                self.pending_text.get_or_insert_with(String::new).push_str(text);
                self.pending_turn_ts.get_or_insert(ev.ts);
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                bump_len()
            }
            EventKind::ToolCall { id, name, args } => {
                self.pending_turn_ts.get_or_insert(ev.ts);
                self.pending_calls.push(ToolCallRef { id: id.clone(), name: name.clone(), args: args.clone() });
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                bump_len()
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
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::Tasks { items } => {
                self.tasks = items.clone();
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::WakeScheduled { .. } | EventKind::WakeFired { .. } | EventKind::WakeCancelled { .. } => {
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::TurnsDropped { .. } | EventKind::ContextMutation { .. }
            | EventKind::DirectiveReasserted { .. } | EventKind::TurnsReadmitted { .. } => {
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }

            // Tier 2 — append-only msgs change.
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
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::ModelThinking { text } => {
                let flushed = self.flush_pending_assistant();
                self.pending_thinking = Some(text.clone());
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                if flushed { bump_len() } else { ProjectionImpact::Economy }
            }
            EventKind::ToolResult { id, name, output, is_error } => {
                self.flush_pending_assistant();
                if self.question_ids.contains(id.as_str()) {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    return bump_len();
                }
                self.msgs.push(ChatMsg::ToolResult {
                    id: id.clone(), name: name.clone(), output: output.clone(),
                    is_error: *is_error, compacted: false, ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::QuestionAsked { id, kind, question, choices } => {
                self.flush_pending_assistant();
                if !matches!(kind, zoid_core::event::QuestionKind::Approval) {
                    self.question_ids.insert(id.clone());
                }
                self.msgs.push(ChatMsg::Question {
                    id: id.clone(), kind: kind.clone(), question: question.clone(),
                    choices: choices.clone(),
                    state: QuestionCardState::Open { selected: 0, free_text: String::new() },
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::QuestionAnswered { id, answer } => {
                if let Some((idx, ChatMsg::Question { state, .. })) =
                    self.msgs.iter_mut().enumerate().rev()
                        .find(|(_, m)| matches!(m, ChatMsg::Question { id: qid, .. } if qid == id))
                {
                    *state = QuestionCardState::Answered { answer: answer.clone() };
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::MsgsMutated { mutated_index: Some(idx) }
                } else {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::Economy
                }
            }
            EventKind::DelegationResult { summary, ok, .. } => {
                self.flush_pending_assistant();
                self.msgs.push(ChatMsg::Delegated { summary: summary.clone(), ok: *ok });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::TurnsEvicted { reclaimed_tokens, marker, rescue } => {
                self.flush_pending_assistant();
                let evicted_topics: Vec<String> = marker.spans.iter().map(|s| s.topic_hint.clone()).collect();
                let rescue = rescue.as_ref().map(|r| RescueSummary {
                    goal_text: r.goal_text.clone(),
                    weight: r.weight.round() as u32,
                    rescued: r.survivors.iter().map(|s| RescuedTurnSummary {
                        topic_hint: s.topic_hint.clone(),
                        bump_milli: (s.rescue_bump * 1000.0).round() as u32,
                    }).collect(),
                });
                self.msgs.push(ChatMsg::Evicted {
                    reclaimed_tokens: *reclaimed_tokens, evicted_topics, rescue, ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }

            // Tier 3 — content mutation.
            EventKind::ToolResultCompacted { id, summary, .. } => {
                if let Some((idx, ChatMsg::ToolResult { output, compacted, .. })) =
                    self.msgs.iter_mut().enumerate().rev()
                        .find(|(_, m)| matches!(m, ChatMsg::ToolResult { id: rid, .. } if rid == id))
                {
                    *output = summary.clone();
                    *compacted = true;
                    self.window_dirty = true;
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::MsgsMutated { mutated_index: Some(idx) }
                } else {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::Economy
                }
            }
        }
    }
```

- [ ] **Step 5: Update `refresh()` with the dirty-flag path**

Replace the `refresh` method (lines 1434–1459) with:

```rust
    /// Refresh projections. When `events_len` matches (no full invalidation),
    /// rebuild only dirty economy projections. When `events_len` is `None`
    /// (full invalidation — session resume, first frame), rebuild everything.
    fn refresh(&mut self, events: &zoid::eventlog::EventLog) -> bool {
        if self.events_len == Some(events.len()) {
            let mut rebuilt = false;
            if self.window_dirty {
                self.window = zoid_core::context::context_window(events.iter());
                self.window_dirty = false;
                rebuilt = true;
            }
            if self.churn_dirty {
                self.churn = zoid_core::economy::churn_timeline(events.iter());
                self.churn_dirty = false;
                rebuilt = true;
            }
            return rebuilt;
        }
        // Full invalidation — rebuild everything from scratch.
        self.msgs = zoid_core::projection::conversation(events.iter());
        self.window = zoid_core::context::context_window(events.iter());
        self.churn = zoid_core::economy::churn_timeline(events.iter());
        self.tasks = zoid_core::tasks::tasks(events.iter());
        let ledger = zoid_core::economy::token_ledger(events.iter());
        self.ledger_total = ledger.total;
        self.cached_total = ledger.cached;
        self.thinking_total = ledger.thinking;
        self.last_input_tokens = events.iter().rev().find_map(|e| e.tokens.map(|t| t.input)).filter(|&t| t > 0);
        self.last_output_tokens = events.iter().rev().find_map(|e| e.tokens.map(|t| t.output)).filter(|&t| t > 0);
        self.window_dirty = false;
        self.churn_dirty = false;
        self.question_ids = events.iter()
            .filter_map(|e| match &e.kind {
                EventKind::QuestionAsked { id, kind, .. }
                    if !matches!(kind, zoid_core::event::QuestionKind::Approval) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.events_len = Some(events.len());
        self.pending_text = None;
        self.pending_calls = Vec::new();
        self.pending_turn_ts = None;
        self.pending_thinking = None;
        true
    }
```

- [ ] **Step 6: Update the `AgentUpdate::Appended` handler to use `apply_event`**

In `crates/zoid/src/main.rs`, first update the stale comment block (lines 3088–3099) that describes the old `apply_streaming` behavior. Replace the entire comment block with:

```rust
                        // Incremental projection: apply_event handles every
                        // EventKind in O(1) (append to msgs, in-place mutation,
                        // or bookkeeping update). Returns a ProjectionImpact
                        // describing what changed — the caller uses it to
                        // decide whether body_cache needs invalidation.
                        // Subagent-branch events are persisted but skipped
                        // (the projection only tracks the main branch).
```

Then replace the `apply_streaming` call block (lines 3101–3110):

```rust
                        let is_subagent_branch =
                            ev.branch != zoid_core::event::BranchId::default();
                        if !is_subagent_branch {
                            let impact = app.proj.apply_event(&ev);
                            match impact {
                                ProjectionImpact::Economy => {
                                    // msgs unchanged — body_cache NOT invalidated.
                                }
                                ProjectionImpact::MsgsMutated { mutated_index } => {
                                    match mutated_index {
                                        None | Some(i) if i == app.proj.msgs.len() - 1 => {
                                            // Last-message mutation — BodyCache incremental path.
                                        }
                                        Some(_) => {
                                            // Non-last mutation — full body rebuild.
                                            app.body_cache.key = None;
                                        }
                                    }
                                }
                                ProjectionImpact::MsgsAppended => {
                                    app.body_cache.key = None;
                                }
                                ProjectionImpact::FullRefresh => {
                                    app.proj.events_len = None;
                                    app.body_cache.key = None;
                                }
                            }
                        }
```

- [ ] **Step 7: Add `finalize_pending` call on turn end**

Find the `AgentUpdate::TurnComplete` handler where `app.streaming = false` is set (line 3142). After setting `app.streaming = false`, add:

```rust
                        // Finalize any pending assistant-turn state (trailing
                        // standalone-thinking flush — mirrors the fold's
                        // trailing flush at projection.rs:344–356).
                        let impact = app.proj.finalize_pending();
                        if matches!(impact, ProjectionImpact::MsgsAppended) {
                            app.body_cache.key = None;
                        }
```

**Do NOT add this after the 11 test-setup `app.streaming = false` sites** (lines 7877, 7902, 8485, 8523, 8688, 8876, 8925, 8946, 8961, 8970, 8983) — those simulate idle state in test setup, not a real turn end. The `SessionTakenOver` handler (line 3233) also sets `app.streaming = false` but does not need `finalize_pending` — the session is being taken over, not completing a turn.

- [ ] **Step 8: Update existing tests that call `apply_streaming`**

Two existing tests call `app.proj.apply_streaming(...)` directly and must be updated to use `apply_event`:

1. `subagent_branch_event_skips_apply_streaming` (line 8996): Replace `!app.proj.apply_streaming(&sub_ev)` with `app.proj.apply_event(&sub_ev)` and adjust the guard logic. The test asserts that a subagent-branch event does NOT modify the projection — the branch guard (`is_subagent_branch`) already prevents `apply_event` from being called, so the test logic is unchanged, just the method name.

Replace (line 9032):
```rust
        if !is_subagent_branch && !app.proj.apply_streaming(&sub_ev) {
            app.proj.events_len = None;
        }
```
with:
```rust
        if !is_subagent_branch {
            let impact = app.proj.apply_event(&sub_ev);
            if matches!(impact, ProjectionImpact::FullRefresh) {
                app.proj.events_len = None;
            }
        }
```

2. `main_branch_event_applies_streaming` (line 9055): Replace `!app.proj.apply_streaming(&main_ev)` with `app.proj.apply_event(&main_ev)`. The test asserts that a main-branch `ModelDelta` adds to `msgs` — but with the new always-accumulate-in-pending logic, `ModelDelta` no longer immediately pushes a `ChatMsg::Assistant`. It accumulates in `pending_text`. The test must call `finalize_pending()` after `apply_event` to flush the pending turn, then assert `msgs.len()` grew.

Replace (lines 9078–9081):
```rust
        let is_subagent_branch = main_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch && !app.proj.apply_streaming(&main_ev) {
            app.proj.events_len = None;
        }
```
with:
```rust
        let is_subagent_branch = main_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch {
            let _ = app.proj.apply_event(&main_ev);
            let _ = app.proj.finalize_pending();
        }
```

The assertion at line 9086 (`msgs_before + 1`) still holds — `finalize_pending` flushes the accumulated `pending_text` as a `ChatMsg::Assistant`.

Also rename both tests to replace `apply_streaming` with `apply_event` in their names and doc comments, to avoid confusion. Update the `#[allow(dead_code)]` on `apply_streaming` if the method is fully removed — but since `apply_event` replaces it, just delete the old method.

- [ ] **Step 9: Build the workspace**

Run: `cargo build --workspace`
Expected: SUCCESS.

- [ ] **Step 10: Run the parity test**

Run: `cargo test -p zoid -- apply_event_parity 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 11: Run all tests**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: PASS — no regressions. The existing `projection_cache_recomputes_only_on_len_change` test still passes (the `events_len` guard is unchanged). The two renamed `apply_event` tests pass.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "perf: replace apply_streaming with tiered apply_event + dirty-flag economy refresh"
```

---

### Task 7: Write focused tests for each tier and edge case

**Files:**
- Modify: `crates/zoid/src/main.rs` — add tests to the `#[cfg(test)]` module

**Interfaces:**
- Consumes: `apply_event`, `ProjectionImpact`, `finalize_pending` (from Task 6).

- [ ] **Step 1: Write the tier classification and edge-case tests**

Add these tests to the `#[cfg(test)]` module in `crates/zoid/src/main.rs`:

```rust
#[test]
fn apply_event_usage_returns_economy_and_accumulates() {
    use zoid_core::event::{Event, EventKind, TokenStat};
    let mut cache = ProjectionCache::default();
    let mut ev = Event::new(Ulid::new(), None, 0, EventKind::Usage);
    ev.tokens = Some(TokenStat { input: 100, output: 50, cached: 20, thinking: 5 });
    let impact = cache.apply_event(&ev);
    assert_eq!(impact, ProjectionImpact::Economy);
    assert_eq!(cache.ledger_total, 150);
    assert_eq!(cache.cached_total, 20);
    assert_eq!(cache.thinking_total, 5);
    assert_eq!(cache.last_input_tokens, Some(100));
    assert_eq!(cache.last_output_tokens, Some(50));
    assert!(cache.churn_dirty);
    assert!(cache.msgs.is_empty());
}

#[test]
fn apply_event_model_delta_returns_msgs_mutated_none() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    // Seed an assistant message to append to.
    cache.msgs.push(zoid_core::projection::ChatMsg::Assistant {
        text: "hello".into(), tool_calls: vec![], ts: 0, thinking: None,
    });
    let ev = Event::new(Ulid::new(), None, 0, EventKind::ModelDelta { text: " world".into() });
    let impact = cache.apply_event(&ev);
    assert_eq!(impact, ProjectionImpact::MsgsMutated { mutated_index: None });
    // Not MsgsAppended — caller must NOT invalidate body_cache.
}

#[test]
fn apply_event_model_thinking_flushes_when_pending() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    // Accumulate a pending delta.
    let ev1 = Event::new(Ulid::new(), None, 0, EventKind::ModelDelta { text: "partial".into() });
    cache.apply_event(&ev1);
    // ModelThinking should flush the pending turn.
    let ev2 = Event::new(Ulid::new(), None, 1, EventKind::ModelThinking { text: "hmm".into() });
    let impact = cache.apply_event(&ev2);
    assert_eq!(impact, ProjectionImpact::MsgsMutated { mutated_index: None });
    assert_eq!(cache.msgs.len(), 1, "pending assistant turn was flushed");
    // The thinking should be stashed (not on the flushed message — it goes
    // to the NEXT assistant message).
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::Assistant { thinking, text, .. } => {
            assert_eq!(text, "partial");
            assert!(thinking.is_none(), "thinking goes to next message, not the flushed one");
        }
        _ => panic!("expected Assistant"),
    }
    assert!(cache.pending_thinking.is_some(), "thinking stashed for next message");
}

#[test]
fn apply_event_model_thinking_no_op_when_no_pending() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let ev = Event::new(Ulid::new(), None, 0, EventKind::ModelThinking { text: "hmm".into() });
    let impact = cache.apply_event(&ev);
    assert_eq!(impact, ProjectionImpact::Economy);
    assert!(cache.msgs.is_empty(), "no message pushed");
    assert!(cache.pending_thinking.is_some());
}

#[test]
fn finalize_pending_emits_standalone_thinking() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let ev = Event::new(Ulid::new(), None, 0, EventKind::ModelThinking { text: "deep thoughts".into() });
    cache.apply_event(&ev);
    assert!(cache.msgs.is_empty());
    let impact = cache.finalize_pending();
    assert_eq!(impact, ProjectionImpact::MsgsAppended);
    assert_eq!(cache.msgs.len(), 1);
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::Assistant { text, thinking, .. } => {
            assert!(text.is_empty());
            assert_eq!(thinking.as_deref(), Some("deep thoughts"));
        }
        _ => panic!("expected standalone thinking Assistant"),
    }
}

#[test]
fn apply_event_tool_result_suppressed_for_non_approval_question() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let q = Event::new(Ulid::new(), None, 0, EventKind::QuestionAsked {
        id: "q1".into(), kind: zoid_core::event::QuestionKind::Ask,
        question: "which?".into(), choices: vec!["a".into()],
    });
    cache.apply_event(&q);
    assert_eq!(cache.msgs.len(), 1, "question card pushed");
    let tr = Event::new(Ulid::new(), None, 1, EventKind::ToolResult {
        id: "q1".into(), name: "ask_user".into(), output: "answer".into(), is_error: false,
    });
    let impact = cache.apply_event(&tr);
    assert_eq!(cache.msgs.len(), 1, "ToolResult suppressed — no new msg");
    assert!(matches!(impact, ProjectionImpact::MsgsMutated { .. }));
}

#[test]
fn apply_event_tool_result_not_suppressed_for_approval_question() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let q = Event::new(Ulid::new(), None, 0, EventKind::QuestionAsked {
        id: "q2".into(), kind: zoid_core::event::QuestionKind::Approval,
        question: "approve?".into(), choices: vec!["yes".into(), "no".into()],
    });
    cache.apply_event(&q);
    let tr = Event::new(Ulid::new(), None, 1, EventKind::ToolResult {
        id: "q2".into(), name: "shell".into(), output: "done".into(), is_error: false,
    });
    let impact = cache.apply_event(&tr);
    assert_eq!(cache.msgs.len(), 2, "ToolResult NOT suppressed for Approval");
    assert_eq!(impact, ProjectionImpact::MsgsAppended);
}

#[test]
fn apply_event_tool_result_compacted_non_last_mutates_index() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    // Push a ToolResult, then an AssistantMessage, then compact the result.
    let tr = Event::new(Ulid::new(), None, 0, EventKind::ToolResult {
        id: "t1".into(), name: "read".into(), output: "full output".into(), is_error: false,
    });
    cache.apply_event(&tr);
    let am = Event::new(Ulid::new(), None, 1, EventKind::AssistantMessage { text: "ok".into() });
    cache.apply_event(&am);
    assert_eq!(cache.msgs.len(), 2);
    let comp = Event::new(Ulid::new(), None, 2, EventKind::ToolResultCompacted {
        id: "t1".into(), summary: "summary".into(), original_tokens: 100,
    });
    let impact = cache.apply_event(&comp);
    assert_eq!(impact, ProjectionImpact::MsgsMutated { mutated_index: Some(0) });
    // Caller sees index 0 != msgs.len()-1 (which is 1) → invalidates body_cache.
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::ToolResult { output, compacted, .. } => {
            assert_eq!(output, "summary");
            assert!(*compacted);
        }
        _ => panic!("expected ToolResult at index 0"),
    }
}

#[test]
fn apply_event_question_answered_miss_returns_economy() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let ev = Event::new(Ulid::new(), None, 0, EventKind::QuestionAnswered {
        id: "nonexistent".into(), answer: "x".into(),
    });
    let impact = cache.apply_event(&ev);
    assert_eq!(impact, ProjectionImpact::Economy);
    assert!(cache.msgs.is_empty());
}

#[test]
fn apply_event_tasks_replaces_vec() {
    use zoid_core::event::{Event, EventKind};
    use zoid_core::tasks::TaskItem;
    let mut cache = ProjectionCache::default();
    let t1 = TaskItem { text: "a".into(), status: zoid_core::tasks::TaskStatus::Done };
    let ev1 = Event::new(Ulid::new(), None, 0, EventKind::Tasks { items: vec![t1.clone()] });
    cache.apply_event(&ev1);
    assert_eq!(cache.tasks.len(), 1);
    let t2 = TaskItem { text: "b".into(), status: zoid_core::tasks::TaskStatus::Active };
    let ev2 = Event::new(Ulid::new(), None, 1, EventKind::Tasks { items: vec![t2.clone()] });
    let impact = cache.apply_event(&ev2);
    assert_eq!(impact, ProjectionImpact::Economy);
    assert_eq!(cache.tasks.len(), 1, "last-write-wins");
    assert_eq!(cache.tasks[0].text, "b");
}

#[test]
fn apply_event_wake_scheduled_is_noop() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let ev = Event::new(Ulid::new(), None, 0, EventKind::WakeScheduled {
        wake_id: "w1".into(), fire_at_ms: 99999, note: "reminder".into(),
    });
    let impact = cache.apply_event(&ev);
    assert_eq!(impact, ProjectionImpact::Economy);
    assert!(cache.msgs.is_empty());
    assert!(!cache.window_dirty);
    assert!(!cache.churn_dirty);
}

#[test]
fn apply_event_pending_turn_flush_on_tool_result() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    // ModelDelta accumulates in pending, ToolResult flushes it.
    let d = Event::new(Ulid::new(), None, 0, EventKind::ModelDelta { text: "partial".into() });
    cache.apply_event(&d);
    assert!(cache.msgs.is_empty(), "delta accumulates in pending, not msgs");
    let tc = Event::new(Ulid::new(), None, 1, EventKind::ToolCall { id: "t1".into(), name: "read".into(), args: "{}".into() });
    cache.apply_event(&tc);
    assert!(cache.msgs.is_empty(), "tool call accumulates in pending");
    let tr = Event::new(Ulid::new(), None, 2, EventKind::ToolResult { id: "t1".into(), name: "read".into(), output: "ok".into(), is_error: false });
    let impact = cache.apply_event(&tr);
    assert_eq!(impact, ProjectionImpact::MsgsAppended);
    assert_eq!(cache.msgs.len(), 2, "flushed Assistant + ToolResult");
    // First msg is the flushed assistant turn with delta text + tool call.
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::Assistant { text, tool_calls, .. } => {
            assert_eq!(text, "partial");
            assert_eq!(tool_calls.len(), 1);
        }
        _ => panic!("expected Assistant"),
    }
}

#[test]
fn apply_event_tool_result_compacted_in_place() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let tr = Event::new(Ulid::new(), None, 0, EventKind::ToolResult {
        id: "t1".into(), name: "read".into(), output: "full output".into(), is_error: false,
    });
    cache.apply_event(&tr);
    assert_eq!(cache.msgs.len(), 1);
    let comp = Event::new(Ulid::new(), None, 1, EventKind::ToolResultCompacted {
        id: "t1".into(), summary: "summary".into(), original_tokens: 100,
    });
    let impact = cache.apply_event(&comp);
    assert!(matches!(impact, ProjectionImpact::MsgsMutated { mutated_index: Some(0) }));
    assert_eq!(cache.msgs.len(), 1, "no new msg — in-place mutation");
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::ToolResult { output, compacted, .. } => {
            assert_eq!(output, "summary");
            assert!(*compacted);
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn apply_event_question_answered_in_place() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let q = Event::new(Ulid::new(), None, 0, EventKind::QuestionAsked {
        id: "q1".into(), kind: zoid_core::event::QuestionKind::Ask,
        question: "which?".into(), choices: vec!["a".into(), "b".into()],
    });
    cache.apply_event(&q);
    assert_eq!(cache.msgs.len(), 1);
    let a = Event::new(Ulid::new(), None, 1, EventKind::QuestionAnswered {
        id: "q1".into(), answer: "a".into(),
    });
    let impact = cache.apply_event(&a);
    assert!(matches!(impact, ProjectionImpact::MsgsMutated { mutated_index: Some(0) }));
    assert_eq!(cache.msgs.len(), 1, "no new msg — in-place mutation");
    match &cache.msgs[0] {
        zoid_core::projection::ChatMsg::Question { state, .. } => {
            assert!(matches!(state, zoid_core::projection::QuestionCardState::Answered { .. }));
        }
        _ => panic!("expected Question"),
    }
}

#[test]
fn full_invalidation_rebuilds_all_and_clears_dirty() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    // Seed with some events applied incrementally.
    let ev = Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() });
    cache.apply_event(&ev);
    let mut usage = Event::new(Ulid::new(), None, 1, EventKind::Usage);
    usage.tokens = Some(zoid_core::event::TokenStat { input: 10, output: 5, cached: 0, thinking: 0 });
    cache.apply_event(&usage);
    assert!(cache.churn_dirty);
    // Force full invalidation.
    cache.events_len = None;
    let log = zoid::eventlog::EventLog::from_vec(vec![ev, usage]);
    assert!(cache.refresh(&log));
    assert!(!cache.window_dirty, "window_dirty cleared");
    assert!(!cache.churn_dirty, "churn_dirty cleared");
    assert!(cache.events_len.is_some(), "events_len set");
    assert!(!cache.msgs.is_empty(), "msgs rebuilt");
}

#[test]
fn refresh_dirty_flags_rebuild_only_dirty() {
    use zoid_core::event::{Event, EventKind};
    let mut cache = ProjectionCache::default();
    let log = zoid::eventlog::EventLog::from_vec(vec![
        Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() }),
    ]);
    // Full refresh to seed.
    cache.refresh(&log);
    let window_before = cache.window.clone();
    // Set only churn_dirty (via a Usage event).
    let mut ev = Event::new(Ulid::new(), None, 1, EventKind::Usage);
    ev.tokens = Some(zoid_core::event::TokenStat { input: 10, output: 5, cached: 0, thinking: 0 });
    cache.apply_event(&ev);
    cache.events_len = Some(cache.events_len.unwrap_or(0)); // don't trigger full refresh
    // Push the event to the log so refresh sees the right length.
    let log2 = zoid::eventlog::EventLog::from_vec(vec![
        Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() }),
        ev,
    ]);
    cache.refresh(&log2);
    // window should NOT have been rebuilt (window_dirty was not set by Usage).
    assert_eq!(cache.window, window_before, "window not rebuilt — only churn was dirty");
    assert!(!cache.churn_dirty, "churn_dirty cleared");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p zoid -- apply_event_ 2>&1 | tail -20 && cargo test -p zoid -- finalize_pending 2>&1 | tail -20 && cargo test -p zoid -- refresh_dirty 2>&1 | tail -20`
Expected: PASS — all new tests pass.

- [ ] **Step 3: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: PASS — no regressions.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "test: tier classification, edge cases, and dirty-flag refresh for apply_event"
```

---

## Self-Review

### Spec coverage

| Spec section | Task(s) |
|---|---|
| Part 1 — Peek removal: chat.rs | T4 |
| Part 1 — Peek removal: state.rs | T1 |
| Part 1 — Peek removal: render.rs | T5 |
| Part 1 — Peek removal: layout.rs | T2 |
| Part 1 — Peek removal: route.rs | T3 |
| Part 1 — Peek removal: main.rs | T5 |
| Part 2 — `ProjectionImpact` enum | T6 |
| Part 2 — Tier 1 (bookkeeping) | T6, T7 |
| Part 2 — Tier 2 (append) | T6, T7 |
| Part 2 — Tier 3 (content mutation) | T6, T7 |
| Part 2 — `flush_pending_assistant` | T6 |
| Part 2 — `finalize_pending` (trailing thinking) | T6, T7 |
| Part 2 — `question_ids` suppression | T6, T7 |
| Part 2 — `thinking_total` | T6, T7 |
| Part 2 — dirty-flag `refresh()` | T6, T7 |
| Part 2 — caller `AgentUpdate::Appended` handler | T6 |
| Part 2 — `finalize_pending` on turn end | T6 |
| Part 2 — parity test | T6 |
| Part 2 — edge case tests | T7 |
| Part 1 — snapshot updates | T4, T5 |

All spec sections are covered. ✅

### Placeholder scan

No "TBD", "TODO", "implement later", "add error handling", or "similar to Task N" found. All code blocks contain complete implementations. ✅

### Type consistency

- `ProjectionImpact` variants: `Economy`, `MsgsMutated { mutated_index: Option<usize> }`, `MsgsAppended`, `FullRefresh` — consistent across T6 (definition) and T7 (tests). ✅
- `apply_event(&mut self, ev: &Event) -> ProjectionImpact` — consistent. ✅
- `finalize_pending(&mut self) -> ProjectionImpact` — consistent. ✅
- `flush_pending_assistant(&mut self) -> bool` — consistent. ✅
- `thinking_total: u64` — consistent across struct definition, `apply_event` Usage handler, `refresh` full path, and tests. ✅
- `question_ids: std::collections::HashSet<String>` — consistent across struct, `apply_event` QuestionAsked/ToolResult arms, `refresh` full path. ✅
- `pending_text`, `pending_calls`, `pending_turn_ts`, `pending_thinking` — consistent across struct, `flush_pending_assistant`, `apply_event`, `refresh` full path. ✅