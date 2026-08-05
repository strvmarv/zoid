# Top Bar Chip Rearrangement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the SELECT chip from the bottom status bar to the top bar, add a YOLO warning chip next to it, and recenter the version+wordmark as a combined block.

**Architecture:** `ShellState` gains a `yolo: bool` field mirrored from `App.yolo` each frame. `title_line` is rewritten to render SELECT + conditional YOLO pills flush-left and the `zoid VERSION` block centered on the full width, with a saturating guard against overlap. `render_status` loses the SELECT pill spans. Existing tests are rewritten to match the new layout; snapshots are regenerated.

**Tech Stack:** Rust, ratatui, insta (snapshot tests), unicode-width

## Global Constraints

- Hard terminal minimum is 160×40 (`layout::MIN_WIDTH` / `MIN_HEIGHT`); below that the "too small" overlay renders instead of the shell, so renderers can assume ≥160 columns.
- All glyphs and colors come from `tokens.rs` (`crate::tokens::{color, glyph}`). Never introduce raw hex colors.
- `ShellState::Default` delegates to `new()` (state.rs:906), so adding a field to `new()` covers both.
- Snapshot tests use `insta`. Regenerate with `cargo insta test --accept -p zoid-tui`.
- Full test suite: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` (or `cargo test --workspace --no-fail-fast` as fallback).

---

### Task 1: Add `yolo` field to `ShellState`

**Files:**
- Modify: `crates/zoid-tui/src/state.rs:490` (add field after `select_mode`)
- Modify: `crates/zoid-tui/src/state.rs:704` (add default in `new()`)

**Interfaces:**
- Produces: `ShellState.yolo: bool` — read by `title_line` in Task 3, written by the bin sync in Task 5.

- [ ] **Step 1: Add the field to the struct**

In `crates/zoid-tui/src/state.rs`, find the `select_mode` field (line ~490):

```rust
    pub select_mode: bool,
```

Add immediately after it:

```rust
    pub select_mode: bool,
    /// Whether YOLO mode (auto-approve all tool calls) is active. Mirrors
    /// `App.yolo` so the pure renderer can show a warning chip. Synced by
    /// the bin each frame.
    pub yolo: bool,
```

- [ ] **Step 2: Add the default in `new()`**

In `crates/zoid-tui/src/state.rs`, find the `select_mode: false,` line in `ShellState::new()` (line ~704):

```rust
            select_mode: false,
```

Add immediately after it:

```rust
            select_mode: false,
            yolo: false,
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p zoid-tui`
Expected: PASS (no errors — the field has a default, and no code reads it yet)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): add yolo field to ShellState"
```

---

### Task 2: Rewrite `title_line` with SELECT, YOLO, and centered version

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:287–322` (rewrite `title_line` function)
- Modify: `crates/zoid-tui/src/render.rs:324–326` (update `render_title` signature)

**Interfaces:**
- Consumes: `ShellState.yolo: bool` (from Task 1), `ShellState.select_mode: bool` (existing)
- Produces: `title_line(w: usize, select_mode: bool, yolo: bool) -> Line<'static>` — called by `render_title` and unit tests.
- Produces: `render_title(frame: &mut Frame, state: &ShellState, area: Rect)` — called by `render_shell` at line 207.

- [ ] **Step 1: Write the failing tests**

In `crates/zoid-tui/src/render.rs`, in the `#[cfg(test)] mod tests` module (line ~2180), add these tests. First, add a `title_buffer` helper and a `find_word_style` helper after the existing `line_text` helper (line ~2581):

```rust
    /// Render `title_line` into a buffer at the given width for testing.
    fn title_buffer(w: u16, select_mode: bool, yolo: bool) -> ratatui::buffer::Buffer {
        use ratatui::{backend::TestBackend, Terminal};
        let line = title_line(w as usize, select_mode, yolo);
        let mut term = Terminal::new(TestBackend::new(w, 1)).unwrap();
        term.draw(|f| f.render_widget(ratatui::widgets::Paragraph::new(line), f.area()))
            .unwrap();
        term.backend().buffer().clone()
    }

    /// Scan the buffer for a word and return (start_column, style_of_first_glyph).
    /// `None` if the word is not found. Guards against row-wrap by checking
    /// the run fits within the row.
    fn find_word(
        buf: &ratatui::buffer::Buffer,
        word: &str,
    ) -> Option<(usize, ratatui::style::Style)> {
        let w = buf.area.width as usize;
        let cells = buf.content();
        let word_chars: Vec<char> = word.chars().collect();
        let len = word_chars.len();
        for start in 0..cells.len().saturating_sub(len) {
            if start % w > w.saturating_sub(len) {
                continue;
            }
            let found: String = (0..len).map(|k| cells[start + k].symbol()).collect();
            if found == word {
                return Some((start % w, cells[start].style()));
            }
        }
        None
    }
```

Now add the test functions (after the existing title tests, before `sessions_overlay_shows_confirm_line_when_pending`):

```rust
    #[test]
    fn title_select_on_is_flush_left_and_filled() {
        let buf = title_buffer(160, true, false);
        let (col, style) = find_word(&buf, "SELECT").expect("SELECT must be present");
        assert_eq!(col, 1, "SELECT must be flush-left (col 1, after the leading space): col={col}");
        assert_eq!(style.fg, Some(color::BRANCH), "ON: fg must be BRANCH");
        assert_eq!(style.bg, Some(color::SELECT_BG), "ON: bg must be SELECT_BG");
    }

    #[test]
    fn title_select_off_is_recessive() {
        let buf = title_buffer(160, false, false);
        let (col, style) = find_word(&buf, "SELECT").expect("SELECT must be present");
        assert_eq!(col, 1, "SELECT must be flush-left even when off: col={col}");
        assert_eq!(style.fg, Some(color::DIM), "OFF: fg must be DIM");
        assert_ne!(style.bg, Some(color::SELECT_BG), "OFF: bg must not be SELECT_BG");
    }

    #[test]
    fn title_yolo_shown_when_enabled() {
        let buf = title_buffer(160, true, true);
        let (_, style) = find_word(&buf, "YOLO").expect("YOLO must be present when yolo=true");
        assert_eq!(style.fg, Some(color::WARN), "YOLO fg must be WARN");
        assert_eq!(style.bg, Some(color::WARN_DIM), "YOLO bg must be WARN_DIM");
    }

    #[test]
    fn title_yolo_hidden_when_disabled() {
        let buf = title_buffer(160, true, false);
        assert!(find_word(&buf, "YOLO").is_none(), "YOLO must not appear when yolo=false");
    }

    #[test]
    fn title_version_centered_with_wordmark() {
        let buf = title_buffer(160, false, false);
        let (zoid_col, _) = find_word(&buf, "zoid").expect("zoid wordmark must be present");
        // The combined block is "zoid v0.9.0" (wordmark + space + VERSION).
        // It's centered on the full width w: center_start = (w - combined_w) / 2.
        // Compute combined_w from the actual VERSION constant.
        let combined = format!("zoid {}", VERSION);
        let combined_w = combined.width();
        let expected_start = (160usize).saturating_sub(combined_w) / 2;
        assert_eq!(
            zoid_col, expected_start,
            "zoid must start at the centered position: got {zoid_col}, expected {expected_start}"
        );
        // Version must be immediately after the wordmark + space.
        let ver_str = VERSION;
        let (_, _) = find_word(&buf, &ver_str[1..])  // search without leading 'v' to avoid matching other v's
            .or_else(|| find_word(&buf, ver_str))
            .expect("version must be present in the title bar");
    }

    #[test]
    fn title_left_zone_does_not_overlap_centered_block() {
        // Worst case: SELECT on + YOLO on + 160 cols.
        let buf = title_buffer(160, true, true);
        let (zoid_col, _) = find_word(&buf, "zoid").expect("zoid must be present");
        // Left zone = " SELECT " (7) + " " (1 gap) + " ⚠ YOLO " (width via .width()).
        let select_span = Span::styled(" SELECT ", Style::new());
        let gap_span = Span::raw(" ");
        let yolo_span = Span::styled(" \u{26a0} YOLO ", Style::new());
        let left_zone_w = select_span.content.width() + gap_span.content.width() + yolo_span.content.width();
        assert!(
            zoid_col > left_zone_w,
            "centered block (col {zoid_col}) must not overlap left zone (width {left_zone_w})"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui -- title_select title_yolo title_version title_left_zone 2>&1 | head -30`
Expected: FAIL — `title_line` takes 1 arg, tests call it with 3; `find_word` and `title_buffer` reference `title_line` with wrong signature.

- [ ] **Step 3: Rewrite `title_line`**

In `crates/zoid-tui/src/render.rs`, replace the entire `title_line` function (lines 287–322) and the `render_title` function (lines 324–326) with:

```rust
/// Build the one-row top status bar for inner width `w`.
///
/// Three zones on a single line: SELECT + optional YOLO pills flush-left,
/// the `zoid VERSION` wordmark+version centered on the full width, and the
/// palette hint flush-right.
///
/// The left zone (SELECT pill always, YOLO pill when `yolo`) fills from column
/// 0. The centered block (`zoid {VERSION}`) is positioned at
/// `(w - combined_w) / 2`. A saturating guard fires if `left_zone_w >=
/// center_start`: the centered block left-aligns at `left_zone_w + 1` instead.
/// (Won't fire at the 160-col minimum, but protects against future growth.)
fn title_line(w: usize, select_mode: bool, yolo: bool) -> Line<'static> {
    let wordmark = "zoid";
    let palette_hint = "Esc interrupt · : command · ^P palette";

    // --- Left zone: SELECT pill (always) + 1-cell gap + YOLO pill (if yolo) ---
    let select_style = if select_mode {
        Style::new().fg(color::BRANCH).bg(color::SELECT_BG)
    } else {
        Style::new().fg(color::DIM)
    };
    let select_span = Span::styled(" SELECT ", select_style);

    let yolo_span = if yolo {
        Some(Span::styled(
            format!(" {} YOLO ", glyph::WARNING),
            Style::new().fg(color::WARN).bg(color::WARN_DIM),
        ))
    } else {
        None
    };

    // Left zone width: SELECT + gap (if YOLO follows) + YOLO.
    let gap_w = if yolo.is_some() { 1 } else { 0 };
    let left_zone_w = select_span.content.width()
        + gap_w
        + yolo_span.as_ref().map(|s| s.content.width()).unwrap_or(0);

    // --- Center zone: "zoid {VERSION}" centered on the full width ---
    let combined = format!("{} {}", wordmark, VERSION);
    let combined_w = combined.width();
    let center_start = w.saturating_sub(combined_w) / 2;

    // Guard: if the left zone would overlap the centered block, left-align
    // the centered block right after the left zone instead.
    let center_col = if left_zone_w >= center_start {
        left_zone_w + 1
    } else {
        center_start
    };

    // --- Build spans ---
    let mut spans = Vec::new();

    // Left zone.
    spans.push(select_span);
    if let Some(ys) = yolo_span {
        spans.push(Span::raw(" ".repeat(gap_w)));
        spans.push(ys);
    }

    // Pad from left zone to the centered block.
    let pad_to_center = center_col.saturating_sub(left_zone_w);
    if pad_to_center > 0 {
        spans.push(Span::raw(" ".repeat(pad_to_center)));
    }
    spans.push(Span::styled(combined, Style::new().fg(color::DIM)));

    // Pad from centered block to the palette hint.
    let used = center_col + combined_w;
    let right_pad = w.saturating_sub(used).saturating_sub(palette_hint.width());
    if right_pad > 0 {
        spans.push(Span::raw(" ".repeat(right_pad)));
    }
    spans.push(Span::styled(
        palette_hint.to_string(),
        Style::new().fg(color::DIM),
    ));

    Line::from(spans)
}

fn render_title(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(
        Paragraph::new(title_line(area.width as usize, state.select_mode, state.yolo)),
        area,
    );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui -- title_select title_yolo title_version title_left_zone 2>&1 | tail -20`
Expected: PASS — all 6 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(tui): rewrite title_line with SELECT+YOLO chips and centered version"
```

---

### Task 3: Remove SELECT pill from `render_status`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:384–397` (remove SELECT spans from `render_status`)

**Interfaces:**
- Consumes: nothing new (removes code)
- Produces: `render_status` no longer renders the SELECT pill.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-tui/src/render.rs`, in the test module, add after the `title_left_zone_does_not_overlap_centered_block` test:

```rust
    #[test]
    fn status_bar_has_no_select_pill() {
        // After SELECT moved to the title bar, the bottom status bar must
        // not contain "SELECT" anywhere.
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let mut state = ShellState::new();
        state.select_mode = true; // even when ON, it must not appear in the status bar
        let backend = ratatui::backend::TestBackend::new(160, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &state, &view, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            !content.contains("SELECT"),
            "status bar must NOT contain 'SELECT': got {content:?}"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui -- status_bar_has_no_select_pill 2>&1 | tail -10`
Expected: FAIL — the status bar still renders "SELECT".

- [ ] **Step 3: Remove the SELECT pill from `render_status`**

In `crates/zoid-tui/src/render.rs`, in `render_status` (line ~371), find this block:

```rust
    // A blank cell separates the two pills — adjacent spans sharing a bg would
    // merge into one block, so the gap is what makes them read as two badges.
    left.push(Span::raw(" "));
    // Always-visible SELECT pill, right of the mode pill. It's the purple
    // sibling of the (blue) mode pill: ON = light-purple BRANCH glyph on the
    // dark-purple SELECT_BG fill, mirroring CHAT_ACCENT-on-CHAT_BG. OFF drops
    // the fill entirely (dim glyph on the bar background) so it reads as
    // recessive, not a second lit badge.
    let select_style = if state.select_mode {
        Style::new().fg(color::BRANCH).bg(color::SELECT_BG)
    } else {
        Style::new().fg(color::DIM)
    };
    left.push(Span::styled(" SELECT ", select_style));
```

Delete the entire block (the gap span, the comment, the `select_style` conditional, and the SELECT span). The `left` vector now starts with just the mode pill `Span::styled(chip, …)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui -- status_bar_has_no_select_pill 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "refactor(tui): remove SELECT pill from bottom status bar"
```

---

### Task 4: Update existing tests for the new layout

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:2231–2310` (rewrite `select_pill_style`, `status_buffer`, and the two SELECT pill tests)
- Modify: `crates/zoid-tui/src/render.rs:2583–2621` (rewrite/remove the two `title_line` tests)
- Modify: `crates/zoid-tui/src/render.rs:2312–2313` (update compaction test docstring)

**Interfaces:**
- Consumes: `title_buffer` and `find_word` helpers from Task 2.

- [ ] **Step 1: Remove old `select_pill_style` and `status_buffer` helpers**

In `crates/zoid-tui/src/render.rs`, find the `select_pill_style` function (line ~2231) and the `status_buffer` function (line ~2252). Delete both entirely — they are replaced by `find_word` and `title_buffer` from Task 2.

- [ ] **Step 2: Rewrite `select_pill_on_is_filled_purple` to use `title_buffer`**

Replace the test (line ~2270) with:

```rust
    /// ON: the SELECT pill is the filled purple badge — `BRANCH` glyph on the
    /// `SELECT_BG` fill (the purple sibling of the mode pill's blue pair).
    #[test]
    fn select_pill_on_is_filled_purple() {
        let buf = title_buffer(160, true, false);
        let (_, style) = find_word(&buf, "SELECT").expect("SELECT pill must be present");
        assert_eq!(
            style.fg,
            Some(color::BRANCH),
            "ON pill glyph must be BRANCH (purple)"
        );
        assert_eq!(
            style.bg,
            Some(color::SELECT_BG),
            "ON pill must fill with SELECT_BG (dark purple)"
        );
    }
```

- [ ] **Step 3: Rewrite `select_pill_off_is_recessive_no_fill` to use `title_buffer`**

Replace the test (line ~2288) with:

```rust
    /// OFF: the pill is recessive — `DIM` glyphs with no fill. `SELECT_BG` must
    /// appear on no cell, so it never reads as a second lit badge.
    #[test]
    fn select_pill_off_is_recessive_no_fill() {
        let buf = title_buffer(160, false, false);
        let (_, style) = find_word(&buf, "SELECT").expect("SELECT pill must be present");
        assert_eq!(
            style.fg,
            Some(color::DIM),
            "OFF pill glyph must be DIM"
        );
        assert_ne!(
            style.bg,
            Some(color::SELECT_BG),
            "OFF pill glyph must not carry the SELECT_BG fill"
        );
        let any_fill = buf
            .content()
            .iter()
            .any(|c| c.style().bg == Some(color::SELECT_BG));
        assert!(
            !any_fill,
            "OFF pill must not fill any cell with SELECT_BG"
        );
    }
```

- [ ] **Step 4: Rewrite `title_shows_version_flush_left_and_keeps_wordmark_centered`**

Replace the test (line ~2583) with:

```rust
    #[test]
    fn title_shows_version_centered_with_wordmark() {
        let buf = title_buffer(160, false, false);
        // Version must be present and adjacent to the wordmark.
        let (zoid_col, _) = find_word(&buf, "zoid").expect("wordmark present");
        let ver_str = VERSION;
        // Version text should start at zoid_col + 4 + 1 (wordmark "zoid" + space).
        let expected_ver_col = zoid_col + 4 + 1;
        let (ver_col, _) = find_word(&buf, &ver_str[1..])
            .or_else(|| find_word(&buf, ver_str))
            .expect("version present");
        assert_eq!(
            ver_col, expected_ver_col,
            "version must be immediately after 'zoid ': got {ver_col}, expected {expected_ver_col}"
        );
        // Wordmark is centered: (w - combined_w) / 2.
        let combined = format!("zoid {}", VERSION);
        let combined_w = combined.width();
        let expected_zoid = (160usize).saturating_sub(combined_w) / 2;
        assert_eq!(
            zoid_col, expected_zoid,
            "wordmark must be centered: got {zoid_col}, expected {expected_zoid}"
        );
        // Palette hint stays flush-right.
        let text: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            text.trim_end().ends_with("palette"),
            "hint stays flush-right: {text:?}"
        );
    }
```

- [ ] **Step 5: Remove `title_drops_version_when_left_pad_too_narrow`**

Delete the entire test (line ~2605 through ~2621). The version-drop logic no longer exists.

- [ ] **Step 6: Update the `compaction_segment_absent_when_not_compacting` docstring**

Find the test at line ~2312. Change the docstring from:

```rust
    /// When `compacting: false`, the compaction segment must NOT appear —
    /// the status bar is byte-identical to the pre-feature layout.
```

to:

```rust
    /// When `compacting: false`, the compaction segment must NOT appear.
```

- [ ] **Step 7: Run all render tests**

Run: `cargo test -p zoid-tui -- render::tests 2>&1 | tail -30`
Expected: PASS — all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "test(tui): update tests for SELECT-in-title-bar and centered version"
```

---

### Task 5: Add `yolo` sync in the bin

**Files:**
- Modify: `crates/zoid/src/main.rs:3153` (add `yolo` sync after `busy` sync)

**Interfaces:**
- Consumes: `App.yolo` (existing, main.rs:2099), `ShellState.yolo` (from Task 1)
- Produces: `app.shell.yolo` is kept in sync each frame so the renderer sees the current value.

- [ ] **Step 1: Add the sync line**

In `crates/zoid/src/main.rs`, find line 3153:

```rust
        app.shell.busy = app.streaming;
```

Add immediately after it:

```rust
        app.shell.busy = app.streaming;
        app.shell.yolo = app.yolo;
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p zoid`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: sync app.yolo to shell state for renderer"
```

---

### Task 6: Regenerate snapshots and verify full test suite

**Files:**
- Regenerate: `crates/zoid-tui/tests/snapshots/*.snap`

- [ ] **Step 1: Confirm no test harness sets `yolo: true`**

Run: `grep -r 'yolo' crates/zoid-tui/tests/`
Expected: No matches (or only the `ShellState` struct reference). If any test constructs `ShellState` with `yolo: true`, note it — the snapshots must show the YOLO pill in those cases.

- [ ] **Step 2: Run snapshot tests to see failures**

Run: `cargo insta test -p zoid-tui 2>&1 | tail -30`
Expected: FAIL — snapshots have the old top bar (version flush-left, no SELECT) and old bottom bar (SELECT present).

- [ ] **Step 3: Regenerate snapshots**

Run: `cargo insta test --accept -p zoid-tui`
Expected: All snapshots updated.

- [ ] **Step 4: Verify the snapshot diff is mechanical**

Run: `git diff --stat crates/zoid-tui/tests/snapshots/`
Expected: Many `.snap` files changed. Inspect a few diffs to confirm:
- Top bar: ` SELECT ` now appears flush-left, `zoid v0.9.0` centered (not version flush-left)
- Bottom bar: ` SELECT ` no longer appears between ` CHAT ` and the status hint

Run: `git diff crates/zoid-tui/tests/snapshots/shell_snapshot__chat_with_rail_frame.snap`
Expected: Line 5 (top bar) changes from `"v0.9.0 ... zoid ... palette"` to `" SELECT  ... zoid v0.9.0 ... palette"`, and line 44 (bottom bar) changes from `" CHAT   SELECT ..."` to `" CHAT ..."` (SELECT removed).

- [ ] **Step 5: Run the full test suite**

Run: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast 2>&1 | tail -30`
Expected: PASS (all tests pass, including the regenerated snapshots).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/tests/snapshots/
git commit -m "test(tui): regenerate snapshots for top bar chip rearrangement"
```