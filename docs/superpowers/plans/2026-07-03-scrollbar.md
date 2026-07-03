# Scrollbar + Cross-Zoom Position Anchoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an always-visible, draggable vertical scrollbar to the conversation pane, and preserve reading position across zoom (altitude) changes by anchoring to the message at the top of the viewport.

**Architecture:** A pure geometry function computes thumb position/size; the scrollbar paints into the existing right-pad gutter column (no text re-wrap). Mouse drag is classified in the pure `route_mouse` via a `scrollbar_drag` flag and mapped to an absolute offset in the bin. Cross-zoom anchoring records the top message index before a zoom and re-applies it (as a line offset in the new altitude's body) in the pre-draw block, once the new body is built — flicker-free. The top-down zoom reveal animation is retired (dormant) because it is incompatible with position anchoring.

**Tech Stack:** Rust, ratatui (TUI), crossterm (mouse events), insta (snapshot tests).

## Global Constraints

- Glyphs come from `crates/zoid-tui/src/tokens.rs::glyph`, never string literals (spec §16). New glyphs: `SCROLL_TRACK`, `SCROLL_THUMB`.
- Thumb geometry and line↔message mapping are **pure functions** with unit tests (mirroring `motion::spinner_frame`, `max_scroll`).
- The scrollbar occupies the **rightmost column of `layout.conversation`** (`CONV_PAD = 2` already reserves the gutter); text width is unchanged, the body does not re-wrap.
- No new crate dependencies (static-musl release).
- Snapshot-affecting state has fixed defaults (`scrollbar_drag: false`).
- `msg_starts` is a `Vec<usize>` of **length `msgs.len()`** at every altitude; entry `i` = the body line where message `i`'s block begins (collapsed messages in Summary share their turn's line). The sequence is non-decreasing.

---

## File Structure

- **Create** `crates/zoid-tui/src/scrollbar.rs` — pure thumb geometry (`scrollbar_thumb`) and line↔message mapping (`msg_at_line`, `line_of_msg`).
- **Modify** `crates/zoid-tui/src/lib.rs` — declare + re-export the `scrollbar` module.
- **Modify** `crates/zoid-tui/src/tokens.rs` — add `SCROLL_TRACK`, `SCROLL_THUMB`.
- **Modify** `crates/zoid-tui/src/chat.rs` — emit `msg_starts` from the three altitude builders; add `conversation_view_indexed`.
- **Modify** `crates/zoid-tui/src/state.rs` — add `scrollbar_drag` field + `scroll_to_offset` method.
- **Modify** `crates/zoid-tui/src/render.rs` — paint the scrollbar into the gutter.
- **Modify** `crates/zoid-tui/src/route.rs` — `Target::Scrollbar`, `hit_test`, `route_mouse` grab/drag/release, new `Action`s.
- **Modify** `crates/zoid/src/main.rs` — `BodyCache.msg_starts`; `pending_zoom_anchor`; scrollbar drag→offset mapping; anchor application in the pre-draw block; stop setting `zoom_changed_at`.

---

### Task 1: Scrollbar thumb geometry (pure)

**Files:**
- Create: `crates/zoid-tui/src/scrollbar.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub fn scrollbar_thumb(offset: u16, max_scroll: u16, track_h: u16, content_len: u16) -> (u16, u16)` returning `(thumb_start, thumb_len)` in track rows.

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-tui/src/scrollbar.rs`:

```rust
//! Pure geometry + line↔message mapping for the conversation scrollbar. Kept
//! out of the renderer so the math is unit-testable (spec §13 determinism),
//! like `motion::spinner_frame`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_thumb_when_content_fits() {
        // content_len <= track_h: nothing to scroll → full-height thumb at top.
        assert_eq!(scrollbar_thumb(0, 0, 10, 8), (0, 10));
        assert_eq!(scrollbar_thumb(0, 0, 10, 10), (0, 10));
    }

    #[test]
    fn thumb_is_proportional_and_clamped() {
        // track_h 10, content 40 → thumb_len = round(10*10/40) = 3 (>=1).
        let (_, len) = scrollbar_thumb(0, 30, 10, 40);
        assert_eq!(len, 3);
        // never zero even for huge content
        let (_, len) = scrollbar_thumb(0, 9990, 10, 10000);
        assert_eq!(len, 1);
        // thumb stays within the track
        let (start, len) = scrollbar_thumb(30, 30, 10, 40);
        assert!(start + len <= 10, "thumb overflows track: {start}+{len}");
    }

    #[test]
    fn thumb_at_top_and_bottom() {
        // offset 0 → thumb at row 0
        assert_eq!(scrollbar_thumb(0, 30, 10, 40).0, 0);
        // offset == max_scroll → thumb flush at the bottom (start = track_h - len)
        let (start, len) = scrollbar_thumb(30, 30, 10, 40);
        assert_eq!(start, 10 - len);
    }

    #[test]
    fn zero_track_or_content_is_safe() {
        assert_eq!(scrollbar_thumb(0, 0, 0, 0), (0, 0));
        assert_eq!(scrollbar_thumb(5, 5, 0, 100), (0, 0));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib scrollbar::tests 2>&1 | head`
Expected: FAIL — `cannot find function scrollbar_thumb`.

- [ ] **Step 3: Implement `scrollbar_thumb`**

Prepend to `crates/zoid-tui/src/scrollbar.rs` (above the test module):

```rust
/// Vertical scrollbar thumb geometry. `offset` ∈ [0, max_scroll], `track_h` =
/// track height in rows, `content_len` = total body lines. Returns
/// (thumb_start, thumb_len) in track rows, both within [0, track_h]. Always a
/// ≥1-row thumb when the track is non-empty; a full-height thumb when everything
/// fits (max_scroll == 0). The thumb sits flush at the bottom when
/// offset == max_scroll.
pub fn scrollbar_thumb(offset: u16, max_scroll: u16, track_h: u16, content_len: u16) -> (u16, u16) {
    if track_h == 0 {
        return (0, 0);
    }
    if max_scroll == 0 || content_len <= track_h {
        return (0, track_h); // everything fits → full-height thumb
    }
    // Thumb length ∝ viewport/content. viewport == track_h.
    let len = (((track_h as u32 * track_h as u32) + content_len as u32 / 2) / content_len as u32)
        .max(1)
        .min(track_h as u32) as u16;
    let travel = track_h - len; // rows the thumb can move
    let start = ((travel as u32 * offset as u32 + max_scroll as u32 / 2) / max_scroll as u32) as u16;
    (start.min(travel), len)
}
```

- [ ] **Step 4: Declare the module and re-export**

In `crates/zoid-tui/src/lib.rs`, add the module declaration next to the other `mod` lines (alphabetical-ish, near `mod route;`):

```rust
pub mod scrollbar;
```

And add a re-export next to the `pub use motion::{...}` line:

```rust
pub use scrollbar::{line_of_msg, msg_at_line, scrollbar_thumb};
```

> Note: `line_of_msg` / `msg_at_line` are added in Task 2; if this task is implemented first, temporarily export only `scrollbar_thumb` and extend the re-export in Task 2. Prefer implementing Task 2 before compiling the full re-export.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib scrollbar::tests 2>&1 | tail -5`
Expected: PASS — 4 tests ok.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/scrollbar.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): pure scrollbar thumb geometry"
```

---

### Task 2: Line↔message mapping helpers (pure)

**Files:**
- Modify: `crates/zoid-tui/src/scrollbar.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (finalize the re-export from Task 1)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn msg_at_line(starts: &[usize], line: usize) -> usize`
  - `pub fn line_of_msg(starts: &[usize], idx: usize) -> usize`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-tui/src/scrollbar.rs`:

```rust
    #[test]
    fn msg_at_line_finds_the_containing_message() {
        // messages start at lines [0, 4, 9]
        let starts = [0usize, 4, 9];
        assert_eq!(msg_at_line(&starts, 0), 0);
        assert_eq!(msg_at_line(&starts, 3), 0); // still inside msg 0
        assert_eq!(msg_at_line(&starts, 4), 1); // boundary → msg 1
        assert_eq!(msg_at_line(&starts, 100), 2); // past the end → last msg
    }

    #[test]
    fn msg_at_line_handles_collapsed_and_empty() {
        // Summary: msgs 0..2 collapse onto turn line 0, msg 3 onto line 1.
        let starts = [0usize, 0, 0, 1];
        assert_eq!(msg_at_line(&starts, 0), 2, "last msg sharing line 0");
        assert_eq!(msg_at_line(&starts, 1), 3);
        // empty → 0
        assert_eq!(msg_at_line(&[], 5), 0);
    }

    #[test]
    fn line_of_msg_maps_back_and_clamps() {
        let starts = [0usize, 4, 9];
        assert_eq!(line_of_msg(&starts, 0), 0);
        assert_eq!(line_of_msg(&starts, 2), 9);
        assert_eq!(line_of_msg(&starts, 99), 0, "out of range → 0");
        assert_eq!(line_of_msg(&[], 0), 0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib scrollbar::tests 2>&1 | head`
Expected: FAIL — `cannot find function msg_at_line`.

- [ ] **Step 3: Implement the helpers**

Add to `crates/zoid-tui/src/scrollbar.rs` (above the test module):

```rust
/// Index of the message occupying `line`: the last message whose start ≤ `line`.
/// Returns 0 when `starts` is empty or `line` precedes the first start. When
/// several messages share a start line (Summary collapses a turn onto one line),
/// returns the last of them.
pub fn msg_at_line(starts: &[usize], line: usize) -> usize {
    starts.partition_point(|&s| s <= line).saturating_sub(1)
}

/// First body line of message `idx`. Clamps to 0 when `idx` is out of range or
/// `starts` is empty.
pub fn line_of_msg(starts: &[usize], idx: usize) -> usize {
    starts.get(idx).copied().unwrap_or(0)
}
```

- [ ] **Step 4: Ensure the re-export is complete**

Confirm `crates/zoid-tui/src/lib.rs` has:

```rust
pub use scrollbar::{line_of_msg, msg_at_line, scrollbar_thumb};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib scrollbar::tests 2>&1 | tail -5`
Expected: PASS — 7 tests ok.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/scrollbar.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): line<->message mapping helpers for scroll anchoring"
```

---

### Task 3: Emit `msg_starts` from the altitude builders

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:59-66` (`build_conversation` signature + loop), `:490` (`detail_lines`), `:436-448` (`conversation_view`); add `conversation_view_indexed` + `summary_msg_starts`.
- Test: `crates/zoid-tui/src/chat.rs` (tests module).

**Interfaces:**
- Consumes: `ChatView` (existing), `Zoom` (existing).
- Produces:
  - `pub fn conversation_view_indexed(msgs: &[ChatMsg], view: &ChatView, streaming: bool, width: usize) -> (Vec<Line<'static>>, Vec<usize>)`
  - `conversation_view` retained as a wrapper returning only the `Vec<Line>` (existing callers unchanged).
  - `msg_starts` length == `msgs.len()`, non-decreasing, entry `i` = body line where message `i` begins.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-tui/src/chat.rs`:

```rust
    #[test]
    fn conversation_view_indexed_starts_len_matches_msgs_at_each_zoom() {
        let msgs = vec![
            ChatMsg::User { text: "first question".into(), ts: 0 },
            ChatMsg::Assistant { text: "an answer".into(), tool_calls: vec![], ts: 0 },
            ChatMsg::User { text: "second question".into(), ts: 0 },
            ChatMsg::Assistant { text: "another answer".into(), tool_calls: vec![], ts: 0 },
        ];
        for zoom in [Zoom::Summary, Zoom::Normal, Zoom::Detail] {
            let view = ChatView { zoom, caret_on: false, reveal: None, tz_offset_secs: 0 };
            let (lines, starts) = conversation_view_indexed(&msgs, &view, false, 80);
            assert_eq!(starts.len(), msgs.len(), "one start per message at {zoom:?}");
            // non-decreasing
            assert!(starts.windows(2).all(|w| w[0] <= w[1]), "starts not monotonic at {zoom:?}");
            // every start is within the rendered body
            assert!(starts.iter().all(|&s| s < lines.len().max(1)), "start past body at {zoom:?}");
            // Summary collapses two turns onto two lines: msg 0 & 1 → line 0, msg 2 & 3 → line 1
            if zoom == Zoom::Summary {
                assert_eq!(starts, vec![0, 0, 1, 1]);
            }
        }
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib conversation_view_indexed_starts 2>&1 | head`
Expected: FAIL — `cannot find function conversation_view_indexed`.

- [ ] **Step 3: Add `msg_starts` out-param to `build_conversation`**

Change the signature at `crates/zoid-tui/src/chat.rs:59`:

```rust
fn build_conversation(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    hits: &mut Vec<CodeHit>,
    msg_starts: &mut Vec<usize>,
) -> Vec<Line<'static>> {
```

Inside the `for (i, m) in msgs.iter().enumerate() {` loop (line 87), make the first statement record the start line:

```rust
    for (i, m) in msgs.iter().enumerate() {
        let _ = i;
        msg_starts.push(lines.len());
        match m {
```

Update the two existing internal callers to pass a throwaway `msg_starts`:
- `conversation_lines` (line ~41): `build_conversation(msgs, streaming, caret_on, tz_offset_secs, width, &mut hits, &mut Vec::new())`
- `code_hits` (line ~55): `build_conversation(msgs, streaming, caret_on, tz_offset_secs, width, &mut hits, &mut Vec::new())`

- [ ] **Step 4: Add `msg_starts` out-param to `detail_lines`**

Change the signature at `crates/zoid-tui/src/chat.rs:490`:

```rust
fn detail_lines(msgs: &[ChatMsg], tz_offset_secs: i32, width: usize, msg_starts: &mut Vec<usize>) -> Vec<Line<'static>> {
```

In the **second** loop (the one building `out`, at line ~505 `for m in msgs {`), make the first statement:

```rust
    for m in msgs {
        msg_starts.push(out.len());
        match m {
```

(The first loop at line ~494 only builds `id_path` — do not touch it.)

- [ ] **Step 5: Add `summary_msg_starts` (replicates `digests()` grouping)**

Add to `crates/zoid-tui/src/chat.rs` (near `digest_lines`):

```rust
/// Per-message digest-line index for the Summary altitude, mirroring the
/// turn grouping in `zoom::digests`: a new turn begins at each `User` message,
/// and any leading non-user message opens turn 0. Result length == msgs.len(),
/// entry i = the digest line index (0-based) message i is folded into.
fn summary_msg_starts(msgs: &[ChatMsg]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(msgs.len());
    let mut turn: i64 = -1;
    for m in msgs {
        match m {
            ChatMsg::User { .. } => turn += 1,
            _ if turn == -1 => turn = 0, // leading non-user opens turn 0
            _ => {}
        }
        starts.push(turn.max(0) as usize);
    }
    starts
}
```

- [ ] **Step 6: Add `conversation_view_indexed` and reduce `conversation_view` to a wrapper**

Replace the body of `conversation_view` (lines 436-452) so the tuple-returning version holds the logic:

```rust
pub fn conversation_view(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
) -> Vec<Line<'static>> {
    conversation_view_indexed(msgs, view, streaming, width).0
}

/// Like `conversation_view`, but also returns `msg_starts` (length msgs.len()):
/// the body line where each message's block begins at this altitude. Used for
/// cross-zoom position anchoring.
pub fn conversation_view_indexed(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut starts: Vec<usize> = Vec::new();
    let mut lines: Vec<Line<'static>> = match view.zoom {
        Zoom::Summary => {
            starts = summary_msg_starts(msgs);
            digest_lines(&digests(msgs))
        }
        Zoom::Normal => {
            let mut hits = Vec::new();
            build_conversation(msgs, streaming, view.caret_on, view.tz_offset_secs, width, &mut hits, &mut starts)
        }
        Zoom::Detail => detail_lines(msgs, view.tz_offset_secs, width, &mut starts),
    };
    if let Some(n) = view.reveal {
        lines.truncate(n);
    }
    (lines, starts)
}
```

> Preserve whatever the original lines 449-451 did for `view.reveal` — if the original used a different truncation call, keep it verbatim inside `conversation_view_indexed`.

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --lib conversation_view_indexed_starts 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 8: Run the whole tui lib + snapshot suite (no behavior change expected)**

Run: `cargo test -p zoid-tui --no-fail-fast 2>&1 | grep "test result"`
Expected: all ok (this task changes no rendered output).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat(tui): emit per-message start lines (msg_starts) at every altitude"
```

---

### Task 4: `scroll_to_offset` + `scrollbar_drag` state

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (field near `follow_tail`, Default init, method near `scroll_conversation`).
- Test: `crates/zoid-tui/src/state.rs` (tests module).

**Interfaces:**
- Consumes: `follow_tail`, `conversation_scroll` (existing).
- Produces:
  - field `pub scrollbar_drag: bool`
  - `pub fn scroll_to_offset(&mut self, offset: u16, max_scroll: u16)`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-tui/src/state.rs`:

```rust
    #[test]
    fn scroll_to_offset_clamps_and_toggles_follow() {
        let mut s = ShellState::new();
        // absolute jump above the bottom → detaches follow
        s.scroll_to_offset(20, 100);
        assert_eq!(s.conversation_scroll, 20);
        assert!(!s.follow_tail);
        // jump to (or past) the bottom → re-engages follow, clamps
        s.scroll_to_offset(999, 100);
        assert_eq!(s.conversation_scroll, 100);
        assert!(s.follow_tail);
    }

    #[test]
    fn scrollbar_drag_defaults_false() {
        assert!(!ShellState::new().scrollbar_drag);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib scroll_to_offset 2>&1 | head`
Expected: FAIL — no method `scroll_to_offset` / no field `scrollbar_drag`.

- [ ] **Step 3: Add the field**

In `crates/zoid-tui/src/state.rs`, after the `follow_tail` field (~line 99, right after the `pub follow_tail: bool,` and its doc comment — but before `busy`):

```rust
    /// True while the user is dragging the scrollbar thumb. Cross-event memory so
    /// the pure `route_mouse` can classify bare `Drag(Left)` events as scrollbar
    /// drags. Set on grab, cleared on release.
    pub scrollbar_drag: bool,
```

In the `Default`/`new` initializer, after `follow_tail: true,`:

```rust
            scrollbar_drag: false,
```

- [ ] **Step 4: Add the method**

In `crates/zoid-tui/src/state.rs`, right after `scroll_conversation` (the method added for tail-follow):

```rust
    /// Set the conversation scroll to an absolute `offset` (clamped to
    /// [0, max_scroll]) and re-derive tail-follow: landing at (or past) the
    /// bottom re-engages follow, any position above it detaches. Used by the
    /// scrollbar drag / track click.
    pub fn scroll_to_offset(&mut self, offset: u16, max_scroll: u16) {
        let next = offset.min(max_scroll);
        self.conversation_scroll = next;
        self.follow_tail = next >= max_scroll;
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib scroll_to_offset scrollbar_drag 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): scroll_to_offset + scrollbar_drag state for scrollbar"
```

---

### Task 5: Render the scrollbar in the gutter

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` (add glyphs).
- Modify: `crates/zoid-tui/src/render.rs:88-100` (paint after the paragraph).
- Test: snapshot suite (`crates/zoid-tui/tests/*`).

**Interfaces:**
- Consumes: `scrollbar::scrollbar_thumb`, `state.conversation_scroll`, the chat block's `max_scroll`, `body.len()`, the `text` rect.
- Produces: visible scrollbar (not yet draggable).

- [ ] **Step 1: Add the glyphs**

In `crates/zoid-tui/src/tokens.rs`, in the `glyph` module (near `SPINNER`):

```rust
    pub const SCROLL_TRACK: char = '│'; // conversation scrollbar track (§16)
    pub const SCROLL_THUMB: char = '█'; // conversation scrollbar thumb (§16)
```

- [ ] **Step 2: Paint the scrollbar after the conversation paragraph**

In `crates/zoid-tui/src/render.rs`, the Chat block currently ends (line ~99-100) with:

```rust
            let scroll = state.conversation_scroll.min(max_scroll);
            frame.render_widget(Paragraph::new(body).scroll((scroll, 0)), text);
```

Replace those two lines with (note: `body` is moved into the Paragraph, so capture `content_len` first):

```rust
            let scroll = state.conversation_scroll.min(max_scroll);
            let content_len = body.len().min(u16::MAX as usize) as u16;
            frame.render_widget(Paragraph::new(body).scroll((scroll, 0)), text);

            // Always-visible scrollbar in the rightmost gutter column of the
            // conversation rect (CONV_PAD reserves it, so text never overlaps).
            let track_h = layout.conversation.height;
            if track_h > 0 && layout.conversation.width > 0 {
                let bar_x = layout.conversation.right().saturating_sub(1);
                let (thumb_start, thumb_len) =
                    crate::scrollbar::scrollbar_thumb(scroll, max_scroll, track_h, content_len);
                let buf = frame.buffer_mut();
                for dy in 0..track_h {
                    let y = layout.conversation.y + dy;
                    let in_thumb = dy >= thumb_start && dy < thumb_start + thumb_len;
                    let (ch, fg) = if in_thumb {
                        (glyph::SCROLL_THUMB, color::CHAT_ACCENT)
                    } else {
                        (glyph::SCROLL_TRACK, color::DIM)
                    };
                    buf[(bar_x, y)].set_char(ch).set_style(Style::new().fg(fg));
                }
            }
```

> Confirmed for **ratatui 0.29** (this workspace): `frame.buffer_mut()` returns `&mut Buffer`, and `Buffer` implements `Index<(u16, u16)>`, so `buf[(x, y)].set_char(ch).set_style(style)` is correct. (This is the first direct cell-write in `render.rs`; everything else uses widgets.)

- [ ] **Step 3: Build**

Run: `cargo build -p zoid-tui 2>&1 | grep -E "error|warning" ; echo done`
Expected: `done` with no errors.

- [ ] **Step 4: Regenerate snapshots and verify the diff is localized to the bar column**

Run:
```bash
cargo test -p zoid-tui --no-fail-fast 2>&1 | grep FAILED$ | sort -u
```
Expected: the frame/shell/session/tasks snapshots that render the conversation now fail.

Inspect one diff to confirm the change is only in the rightmost conversation column (track/thumb glyphs), not the body text:
```bash
cd crates/zoid-tui/tests/snapshots
diff shell_snapshot__chat_with_rail_frame.snap shell_snapshot__chat_with_rail_frame.snap.new | head -30
cd -
```
Expected: only cells at the bar column change (track `│` / thumb `█`).

- [ ] **Step 5: Accept the regenerated snapshots**

Run:
```bash
cd crates/zoid-tui/tests/snapshots && for f in *.snap.new; do mv -f "$f" "${f%.new}"; done; cd -
cargo test -p zoid-tui --no-fail-fast 2>&1 | grep "test result"
```
Expected: all ok.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests/snapshots
git commit -m "feat(tui): always-visible conversation scrollbar in the gutter"
```

---

### Task 6: Mouse drag / click interaction

**Files:**
- Modify: `crates/zoid-tui/src/route.rs:73-78` (`Target`), `:297-313` (`hit_test`), `:316-353` (`route_mouse`), `Action` enum (~line 35).
- Modify: `crates/zoid/src/main.rs` (handle the three actions; map row→offset).
- Test: `crates/zoid-tui/src/route.rs` (tests module) or `crates/zoid-tui/tests/`.

**Interfaces:**
- Consumes: `Target::Scrollbar`, `state.scrollbar_drag`, `ShellState::scroll_to_offset` (Task 4).
- Produces: `Action::ScrollbarGrab(u16)`, `Action::ScrollbarDrag(u16)`, `Action::ScrollbarRelease`; `Target::Scrollbar`.

- [ ] **Step 1: Write the failing tests**

Add to the tests module in `crates/zoid-tui/src/route.rs` (adapt helpers to those already present — there is a `click` helper near line 518; add a `mouse(kind, col, row)` builder if none exists):

```rust
    #[test]
    fn hit_test_detects_scrollbar_column() {
        let layout = test_layout(100, 24); // use the crate's existing layout helper
        let conv = layout.conversation;
        let bar_x = conv.right() - 1;
        assert_eq!(hit_test(&layout, bar_x, conv.y + 1), Target::Scrollbar);
        // one column left of the bar is still the conversation
        assert_eq!(hit_test(&layout, bar_x - 1, conv.y + 1), Target::Conversation);
    }

    #[test]
    fn scrollbar_grab_then_drag_then_release() {
        let mut s = ShellState::new();
        let layout = test_layout(100, 24);
        let bar_x = layout.conversation.right() - 1;
        let row = layout.conversation.y + 5;
        // grab
        let a = route_mouse(&s, &layout, mouse(MouseEventKind::Down(MouseButton::Left), bar_x, row));
        assert!(matches!(a, Action::ScrollbarGrab(r) if r == row));
        // once dragging, a bare Drag(Left) anywhere is a scrollbar drag
        s.scrollbar_drag = true;
        let a = route_mouse(&s, &layout, mouse(MouseEventKind::Drag(MouseButton::Left), 3, row + 2));
        assert!(matches!(a, Action::ScrollbarDrag(r) if r == row + 2));
        // release
        let a = route_mouse(&s, &layout, mouse(MouseEventKind::Up(MouseButton::Left), 3, row));
        assert!(matches!(a, Action::ScrollbarRelease));
    }

    #[test]
    fn drag_without_grab_is_ignored() {
        let s = ShellState::new(); // scrollbar_drag == false
        let layout = test_layout(100, 24);
        let a = route_mouse(&s, &layout, mouse(MouseEventKind::Drag(MouseButton::Left), 3, 5));
        assert_eq!(a, Action::Noop);
    }
```

> If the tests module has no `test_layout`/`mouse` helpers, add minimal ones: `fn test_layout(w: u16, h: u16) -> ShellLayout { compute(Rect::new(0,0,w,h), &ShellState::new()) }` and `fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent { MouseEvent { kind, column, row, modifiers: KeyModifiers::empty() } }`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib scrollbar_grab 2>&1 | head`
Expected: FAIL — no `Target::Scrollbar` / no `Action::ScrollbarGrab`.

- [ ] **Step 3: Add the `Target` variant**

In `crates/zoid-tui/src/route.rs:73`:

```rust
pub enum Target {
    Conversation,
    Input,
    DrawerHeader(DrawerId),
    Scrollbar,
    None,
}
```

- [ ] **Step 4: Add the `Action` variants**

In the `Action` enum (near line 35, next to `ScrollConversation(i32)`):

```rust
    /// Left-button press landed on the scrollbar at this screen row (begin drag).
    ScrollbarGrab(u16),
    /// Scrollbar drag in progress; the thumb should track this screen row.
    ScrollbarDrag(u16),
    /// Scrollbar drag ended (button released).
    ScrollbarRelease,
```

- [ ] **Step 5: Detect the scrollbar column in `hit_test`**

In `hit_test` (`crates/zoid-tui/src/route.rs:297`), before the `if in_rect(layout.conversation, col, row)` check (line ~310), add:

```rust
    // The scrollbar occupies the rightmost column of the conversation rect.
    if in_rect(layout.conversation, col, row) && col == layout.conversation.right().saturating_sub(1) {
        return Target::Scrollbar;
    }
```

- [ ] **Step 6: Route grab/drag/release in `route_mouse`**

In `route_mouse` (`crates/zoid-tui/src/route.rs:338`), extend the `match m.kind` for the no-overlay case. Add a drag branch guarded by `state.scrollbar_drag` and an `Up` branch, and a `Scrollbar` case in the `Down(Left)` hit-test:

```rust
    match m.kind {
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::CONTROL) => Action::ZoomIn,
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::ZoomOut
        }
        MouseEventKind::ScrollDown => Action::ScrollConversation(1),
        MouseEventKind::ScrollUp => Action::ScrollConversation(-1),
        MouseEventKind::Drag(MouseButton::Left) if state.scrollbar_drag => {
            Action::ScrollbarDrag(m.row)
        }
        MouseEventKind::Up(MouseButton::Left) if state.scrollbar_drag => Action::ScrollbarRelease,
        MouseEventKind::Down(MouseButton::Left) => match hit_test(layout, m.column, m.row) {
            Target::DrawerHeader(id) => Action::ToggleDrawer(id),
            Target::Input => Action::FocusRegion(Focus::Input),
            Target::Scrollbar => Action::ScrollbarGrab(m.row),
            Target::Conversation => Action::ConversationClick(m.row),
            Target::None => Action::Noop,
        },
        _ => Action::Noop,
    }
```

- [ ] **Step 7: Handle the actions in the bin**

In `crates/zoid/src/main.rs` `handle_action`, near the `Action::ScrollConversation(d)` arm, add:

```rust
        Action::ScrollbarGrab(row) => {
            app.shell.scrollbar_drag = true;
            scrollbar_row_to_offset(app, row);
        }
        Action::ScrollbarDrag(row) => scrollbar_row_to_offset(app, row),
        Action::ScrollbarRelease => app.shell.scrollbar_drag = false,
```

`handle_action` does not have `terminal` in scope, so the helper must read a **stored** conversation rect, not recompute one. First add a field to `App` (next to `last_conv_max_scroll`):

```rust
    last_conv_rect: ratatui::layout::Rect,
```

Initialize `last_conv_rect: ratatui::layout::Rect::default(),` at both `App` construction sites (main ~673 and test ~2126, same pattern as `last_conv_max_scroll: 0,`). Then in the pre-draw block, right after `max_scroll` is computed, store it: `app.last_conv_rect = layout.conversation;` (this is also referenced by Task 7 Step 4). Then add this helper near the other scroll helpers in `crates/zoid/src/main.rs`:

```rust
/// Map a screen row on the scrollbar to an absolute conversation offset and
/// apply it (re-deriving tail-follow), using the last drawn frame's geometry.
fn scrollbar_row_to_offset(app: &mut App, row: u16) {
    let conv = app.last_conv_rect;
    let track_h = conv.height;
    if track_h <= 1 {
        return;
    }
    let max = app.last_conv_max_scroll;
    let rel = row.saturating_sub(conv.y).min(track_h - 1);
    let offset = ((rel as u32 * max as u32 + (track_h as u32 - 1) / 2) / (track_h as u32 - 1)) as u16;
    app.shell.scroll_to_offset(offset, max);
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib scrollbar_grab drag_without_grab hit_test_detects 2>&1 | tail -5`
Expected: PASS.
Then: `cargo build -p zoid 2>&1 | grep -E "error|warning"; echo done` → `done`.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(tui): draggable scrollbar — grab/drag/release maps to scroll offset"
```

---

### Task 7: Cross-zoom position anchoring + retire the reveal animation

**Files:**
- Modify: `crates/zoid/src/main.rs` — `BodyKey`/`BodyCache` (~454-490), `App` (add `pending_zoom_anchor`), `ZoomIn`/`ZoomOut` handlers (~1247-1260), pre-draw block (~786-812), motion-tick guard (~941).
- Test: covered by the pure `msg_at_line`/`line_of_msg` (Task 2) + `msg_starts` correctness (Task 3); add one bin-level unit test for the anchor round-trip if the tests module allows constructing `msg_starts`.

**Interfaces:**
- Consumes: `conversation_view_indexed` (Task 3), `msg_at_line`/`line_of_msg` (Task 2), `apply_follow` (existing).
- Produces: position preserved across zoom; zoom is instant (no reveal).

- [ ] **Step 1: Cache `msg_starts` in `BodyCache`**

In `crates/zoid/src/main.rs`, extend `BodyCache` (line ~470):

```rust
#[derive(Default)]
struct BodyCache {
    key: Option<BodyKey>,
    body: Vec<ratatui::text::Line<'static>>,
    msg_starts: Vec<usize>,
}
```

And in `BodyCache::refresh` (line ~477), replace the `self.body = ...` line with the indexed call:

```rust
        let (body, starts) =
            zoid_tui::chat::conversation_view_indexed(msgs, &view, key.streaming, width);
        self.body = body;
        self.msg_starts = starts;
        self.key = Some(key);
```

- [ ] **Step 2: Add `pending_zoom_anchor` to `App`**

In the `App` struct (near `zoom_changed_at`), add:

```rust
    /// Message index to re-anchor to the top of the viewport after a zoom change
    /// (captured from the old altitude before zooming, applied once the new
    /// altitude's body is built). None when no zoom is pending.
    pending_zoom_anchor: Option<usize>,
```

Initialize it `pending_zoom_anchor: None` at both `App` construction sites (the main one ~673 and the test one ~2126, matching the existing pattern used for `last_conv_max_scroll`).

- [ ] **Step 3: Capture the anchor on zoom, stop arming the reveal animation**

Replace the `ZoomIn`/`ZoomOut` arms (`crates/zoid/src/main.rs:1247-1260`) with:

```rust
        Action::ZoomIn => {
            let before = app.shell.zoom;
            // Anchor to the message at the top of the viewport BEFORE zooming.
            let anchor = zoid_tui::msg_at_line(
                &app.body_cache.msg_starts,
                app.shell.conversation_scroll as usize,
            );
            app.shell.zoom_in(); // re-anchors conversation_scroll to 0 on a real change
            if app.shell.zoom != before {
                app.pending_zoom_anchor = Some(anchor);
            }
        }
        Action::ZoomOut => {
            let before = app.shell.zoom;
            let anchor = zoid_tui::msg_at_line(
                &app.body_cache.msg_starts,
                app.shell.conversation_scroll as usize,
            );
            app.shell.zoom_out();
            if app.shell.zoom != before {
                app.pending_zoom_anchor = Some(anchor);
            }
        }
```

> This removes the `app.zoom_changed_at = Some(Instant::now())` assignments — the zoom reveal animation is retired (zoom is now instant; position is preserved by anchoring instead). Leave `zoom_changed_at`, `ZOOM_ANIM_MS`, and `motion::zoom_reveal` in the tree (dormant).

- [ ] **Step 4: Apply the anchor in the pre-draw block**

In the pre-draw block (`crates/zoid/src/main.rs`), after `body_cache.refresh(...)` and after `max_scroll` is computed but BEFORE `apply_follow`, insert:

```rust
        // Re-anchor after a zoom: map the captured message back to its line at the
        // new altitude. Runs before the draw (body/msg_starts now reflect the new
        // altitude), so the transient reset-to-0 from zoom_in/out never paints.
        if let Some(anchor) = app.pending_zoom_anchor.take() {
            let line = zoid_tui::line_of_msg(&app.body_cache.msg_starts, anchor);
            app.shell.conversation_scroll = (line.min(u16::MAX as usize) as u16).min(max_scroll);
        }
        if app.zoom_changed_at.is_none() {
            app.shell.apply_follow(max_scroll);
        }
```

> `apply_follow` still runs afterward: if the user was following the tail, it overrides the anchor and pins to the bottom (correct); if detached, the anchor holds because `follow_tail` is false. Also set `app.last_conv_rect = layout.conversation;` here if you took the stored-rect approach in Task 6.

- [ ] **Step 5: Write a bin-level anchor round-trip test**

In the `crates/zoid/src/main.rs` tests module, add (uses the pure helpers directly to prove the mapping the bin performs):

```rust
    #[test]
    fn zoom_anchor_maps_top_message_across_altitudes() {
        // Detail body: msgs start at lines [0, 6, 14]; viewport top at line 7 → msg 1.
        let detail_starts = [0usize, 6, 14];
        let anchor = zoid_tui::msg_at_line(&detail_starts, 7);
        assert_eq!(anchor, 1);
        // Summary body: same msgs collapse → msg 1 lives on line 0.
        let summary_starts = [0usize, 0, 1];
        assert_eq!(zoid_tui::line_of_msg(&summary_starts, anchor), 0);
    }
```

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p zoid --lib zoom_anchor 2>&1 | tail -5` → PASS.
Run: `cargo build -p zoid 2>&1 | grep -E "error|warning"; echo done` → `done`.
Run: `cargo test --workspace --no-fail-fast 2>&1 | grep -c "test result: ok"` and confirm no `FAILED`.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(tui): cross-zoom position anchoring; retire zoom reveal animation"
```

---

## Manual verification (after all tasks)

Because scrollbar drag and zoom feel can't be verified headlessly, build and check live:

```bash
cargo run --release -p zoid
```

1. Scrollbar is visible on the right edge of the conversation at all times (full-height thumb when the transcript fits).
2. Thumb size reflects viewport/content ratio; it moves as you wheel-scroll.
3. Click-drag the thumb scrolls; dragging to the bottom re-engages tail-follow (new replies then auto-follow again).
4. Clicking the track jumps the thumb to that position.
5. Scroll up into history (detached), then toggle zoom (Detail↔Normal↔Summary): the message you were reading stays at the top; the transition is instant (no top-down reveal sweep).
6. While following the tail, zoom still snaps to the latest (unchanged from #27).

## Self-review notes

- **Spec coverage:** rendering (Task 1,5), interaction (Task 4,6), cross-zoom anchoring incl. all three altitudes (Task 3,7), reveal retirement (Task 7), testing (each task + manual). All spec sections mapped.
- **Type consistency:** `scrollbar_thumb`, `msg_at_line`, `line_of_msg`, `conversation_view_indexed`, `scroll_to_offset`, `scrollbar_drag`, `pending_zoom_anchor`, `msg_starts` used consistently across tasks.
