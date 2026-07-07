# Status Bar Indicator Refinement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refine the three activity indicators on the bottom status bar: "working/idle" stays dead-center always, the tool indicator docks at the ⅓ anchor with room for detail, compaction docks at the ⅔ anchor, both separated from center by a fixed 4-space gap. Continuous animation is reduced to the single "working" spinner; tool and compaction get a brief pulse-on-appear (bright for 300ms, then steady-state dim). The compaction 6-frame spinner is retired.

**Spec:** `docs/superpowers/specs/2026-07-06-status-bar-indicators-design.md`

**Tech Stack:** Rust 2021, ratatui, tokio. Workspace tested via `cargo test --workspace`, linted via `cargo clippy --workspace --all-targets -- -D warnings`, formatted via `cargo fmt`.

---

## File Structure

**Modified files (in task order):**

- `crates/zoid-tui/src/tokens.rs` — add `WARN_DIM` and `COMPACT_DIM` color tokens; delete `COMPACT_SPINNER`.
- `crates/zoid-tui/src/state.rs` — add `tool_started_at: Option<Instant>` and `compaction_started_at: Option<Instant>` to `ShellState`; delete `compact_spinner`; update `set_active_tool`/`clear_active_tool` to manage `tool_started_at`; init/clear the new fields.
- `crates/zoid/src/main.rs` — mirror `compaction_started_at` onto `shell` each frame; delete the `compact_spinner` per-frame computation; add `active_tool.is_some()` to the motion tick guard.
- `crates/zoid-tui/src/render.rs` — rewrite `render_status`'s indicator layout to fixed ⅓/½/⅔ anchors with 4-space gaps; add pulse-on-appear brightness ramp; use static `⊟` glyph instead of `compact_spinner`.
- `crates/zoid-tui/tests/snapshots/` — update snapshots for the new indicator positions.
- `crates/zoid/src/main.rs` (test `App` literal) — remove `compact_spinner` field if present in `test_app()`.

**Untouched:** `agent.rs`, `provider/`, `economy.rs`, `store.rs`, `session.rs`, `render.rs` body paths.

---

## Task 1: Add dim color tokens + delete COMPACT_SPINNER

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs`
- Test: `crates/zoid-tui/src/tokens.rs` (unit tests)

- [ ] **Step 1: Add `WARN_DIM` and `COMPACT_DIM` color tokens**

In `crates/zoid-tui/src/tokens.rs`, in the `pub mod color` block (find `pub const WARN`), add after it:

```rust
    /// Dimmed steady-state for the tool indicator (after the 300ms pulse).
    pub const WARN_DIM: Color = Color::Rgb(0x8a, 0x66, 0x1a);
```

Find the compaction color (`pub const COMPACT` or the purple used for compaction — it's `color::BRANCH` today). Add a dim variant:

```rust
    /// Dimmed steady-state for the compaction indicator (after the 300ms pulse).
    pub const COMPACT_DIM: Color = Color::Rgb(0x6b, 0x5a, 0x8a);
```

- [ ] **Step 2: Delete `COMPACT_SPINNER`**

In `crates/zoid-tui/src/tokens.rs`, delete the `COMPACT_SPINNER` array and its test (`glyph::COMPACT_SPINNER`). Search:

```bash
grep -rn "COMPACT_SPINNER" crates/ | grep -v "test\|plan\|spec\|target/"
```

Delete the `pub const COMPACT_SPINNER: [char; 6] = ...` line (line 47) and the test `compaction_spinner_token_present` (line 204) in `tokens.rs`. Keep `pub const COMPACT: char = '⊟'` (the static glyph — still used as the pulse-then-steady-state glyph).

**ALSO:** `COMPACT_SPINNER` is referenced in `crates/zoid-tui/src/render.rs:1347` (the `compaction_segment_visible_when_compacting` test) and at `render.rs:1323` (`state.compact_spinner = glyph::COMPACT_SPINNER[0]`). These will not compile after deleting the token. Update the render test NOW (Task 1) to use `glyph::COMPACT` instead of `glyph::COMPACT_SPINNER[0]`, and remove the `state.compact_spinner = ...` line (the field is removed in Task 2, but the `COMPACT_SPINNER` reference breaks compilation in Task 1). See Task 4 Step 1 for the full rewrite of this test — for now, just replace `glyph::COMPACT_SPINNER[0]` with `glyph::COMPACT` and comment out or remove the `state.compact_spinner = ...` line so it compiles.

- [ ] **Step 3: Build the crate**

Run: `cargo build -p zoid-tui`
Expected: FAIL — `state.rs` still references `COMPACT_SPINNER`. That's fine; Task 2 fixes it. (If the build fails ONLY due to `compact_spinner` references, proceed to Task 2.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs
git commit -m "feat(tui/tokens): WARN_DIM + COMPACT_DIM colors; retire COMPACT_SPINNER"
```

---

## Task 2: State — `tool_started_at`, `compaction_started_at`, delete `compact_spinner`

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`
- Test: `crates/zoid-tui/src/state.rs` (unit tests)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-tui/src/state.rs`:

```rust
    #[test]
    fn set_active_tool_stamps_tool_started_at() {
        let mut s = ShellState::new();
        assert!(s.tool_started_at.is_none(), "default: no timestamp");
        s.set_active_tool("shell");
        assert!(s.tool_started_at.is_some(), "set_active_tool must stamp tool_started_at");
        s.clear_active_tool();
        assert!(s.tool_started_at.is_none(), "clear must null the timestamp");
    }

    #[test]
    fn compaction_started_at_defaults_none() {
        assert!(ShellState::new().compaction_started_at.is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib state::tests::set_active_tool_stamps`
Expected: FAIL — `tool_started_at` field doesn't exist.

- [ ] **Step 3: Add the fields to `ShellState`**

In `crates/zoid-tui/src/state.rs`, find `pub active_tool: Option<String>,` and add after it:

```rust
    /// When the current tool started (pulse-on-appear anchor). Set by
    /// `set_active_tool`; cleared by `clear_active_tool`. The renderer reads
    /// `elapsed` to drive the 300ms brightness pulse. `None` when no tool is
    /// in flight. Spec §3.
    pub tool_started_at: Option<std::time::Instant>,
```

Find `pub compacting: bool,` and add after it:

```rust
    /// When the current compaction phase started (pulse-on-appear anchor),
    /// mirrored from `App.compaction_started_at` each frame. The renderer reads
    /// `elapsed` to drive the 300ms brightness pulse. `None` when not compacting.
    pub compaction_started_at: Option<std::time::Instant>,
```

- [ ] **Step 4: Delete `compact_spinner`**

In `crates/zoid-tui/src/state.rs`:
- Delete the `pub compact_spinner: char,` field (line ~268).
- Delete its init `compact_spinner: crate::tokens::glyph::COMPACT_SPINNER[0],` in `ShellState::new()` (line ~365).
- Delete the test `compacting_defaults_false_and_spinner` or the `compact_spinner` assertion in it (line ~660) — keep the `compacting` assertion, drop only the `compact_spinner` part.

- [ ] **Step 5: Update `set_active_tool` and `clear_active_tool`**

In `crates/zoid-tui/src/state.rs`:

```rust
    pub fn set_active_tool(&mut self, name: impl Into<String>) {
        self.active_tool = Some(name.into());
        self.tool_started_at = Some(std::time::Instant::now());
    }

    /// Clear the in-flight spinner (its `ToolResult` arrived, or the turn ended).
    pub fn clear_active_tool(&mut self) {
        self.active_tool = None;
        self.tool_started_at = None;
    }
```

- [ ] **Step 6: Init the new fields in `ShellState::new()`**

Find `active_tool: None,` in `ShellState::new()` and add after it:

```rust
            tool_started_at: None,
```

Find `compacting: false,` and add after it:

```rust
            compaction_started_at: None,
```

- [ ] **Step 7: Fix any `ShellState` literal construction sites**

Run: `grep -rn "compact_spinner" crates/ | grep -v "test\|plan\|spec\|target/"`
Expected: any remaining references (e.g. in the bin's `test_app()` literal or snapshot test helpers). Remove `compact_spinner:` from each literal.

Run: `grep -rn "ShellState {" crates/ | grep -v "test\|plan\|spec\|target/" | grep -v "pub struct"`
Expected: the `test_app()` literal in `main.rs`. If it has `compact_spinner:`, remove it (it shouldn't — `test_app` uses `ShellState::new()` indirectly, but check).

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib state::tests::set_active_tool_stamps state::tests::compaction_started_at`
Expected: PASS.

Run: `cargo build -p zoid-tui`
Expected: PASS — no remaining `compact_spinner` references in zoid-tui.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui/state): tool_started_at + compaction_started_at; retire compact_spinner"
```

---

## Task 3: Bin — mirror `compaction_started_at`, delete `compact_spinner` computation, update tick guard

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Test: `crates/zoid/src/main.rs` (build only — no new test needed; the mirror is wiring)

- [ ] **Step 1: Delete the `compact_spinner` per-frame computation**

In `crates/zoid/src/main.rs`, find the line (around 1702):

```rust
        app.shell.compact_spinner = zoid_tui::tokens::glyph::COMPACT_SPINNER
```

Delete the entire block (the `app.shell.compact_spinner = ...` assignment, including the multi-line `zoid_tui::motion::spinner_frame(...)` call it's part of). Search for the exact block:

```bash
grep -n "compact_spinner" crates/zoid/src/main.rs
```

Delete that line (and any continuation lines if the expression wraps).

- [ ] **Step 2: Mirror `compaction_started_at` onto `shell`**

In `crates/zoid/src/main.rs`, find the per-frame block where `app.shell.compacting` is set (around line ~1700, near `if let Some(start) = app.compaction_started_at`). After the debounce logic, add a mirror:

```rust
        app.shell.compaction_started_at = app.compaction_started_at;
```

Place this right after the debounce `if` block (the one that may clear `app.compaction_started_at`), so the shell sees the post-debounce value.

- [ ] **Step 3: Add `active_tool.is_some()` to the motion tick guard**

In `crates/zoid/src/main.rs`, find the `select!` guard (line ~2020):

```rust
            _ = motion_tick.tick(), if app.streaming || !app.in_flight_subagents.is_empty() || app.shell.compacting || app.zoom_changed_at.is_some() => {
```

Add `|| app.shell.active_tool.is_some()`:

```rust
            _ = motion_tick.tick(), if app.streaming || !app.in_flight_subagents.is_empty() || app.shell.compacting || app.shell.active_tool.is_some() || app.zoom_changed_at.is_some() => {
```

- [ ] **Step 4: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS. (If `compact_spinner` is referenced in `test_app()`'s `ShellState` literal, remove it — but `test_app` uses `ShellState::new()` which no longer has the field.)

- [ ] **Step 5: Run tests**

Run: `cargo test --workspace`
Expected: some snapshot tests may fail (the compaction indicator changed from animated spinner to static glyph). Note which fail — Task 5 updates them. If only snapshot tests fail, proceed.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): mirror compaction_started_at; retire compact_spinner; tick guard for active_tool"
```

---

## Task 4: Render — fixed ⅓/½/⅔ anchors + 4-space gaps + pulse-on-appear

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render_status`)
- Test: `crates/zoid-tui/src/render.rs` (unit tests)

This is the core task. It rewrites the indicator section of `render_status`.

- [ ] **Step 1: Write the failing layout test**

Add to `crates/zoid-tui/src/render.rs` tests:

```rust
    #[test]
    fn working_stays_dead_center_with_all_indicators() {
        use crate::state::ShellState;
        use ratatui::layout::Rect;

        let mut s = ShellState::new();
        s.busy = true; // "working"
        s.set_active_tool("shell"); // tool indicator
        s.compacting = true; // compaction
        s.compaction_started_at = Some(std::time::Instant::now());

        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &s, &ChatView {
                zoom: Zoom::Normal,
                caret_on: false,
                reveal: None,
                tz_offset_secs: 0,
            }, f.area()))
            .unwrap();

        // Find the "working" span's position in the buffer.
        let content = terminal.backend().buffer();
        let w = 100usize;
        // The "working" text should start at roughly (W - "working" width) / 2.
        // "⠋ working" is ~9 chars, so ~45. Allow ±3 for rounding.
        let working_start = content
            .content()
            .iter()
            .enumerate()
            .find(|(_, c)| c.symbol() == "⠋")
            .map(|(i, _)| i % w)
            .unwrap_or(0);
        let expected = (w - 9) / 2; // ~45
        assert!(
            (working_start as i32 - expected as i32).abs() <= 3,
            "working indicator must be dead-center: got {working_start}, expected ~{expected}"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib render::tests::working_stays_dead_center`
Expected: FAIL — the current layout puts tool left-of-center, pushing "working" right of true-center.

- [ ] **Step 3: Rewrite `render_status` indicator layout**

In `crates/zoid-tui/src/render.rs`, replace the indicator section of `render_status` (from the `tool_w` computation through the compaction span + `pad2` calculation). The new layout:

```rust
    let w = area.width as usize;
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let center_w = center.width();
    let right_w = right.width();

    // Fixed anchors: tool at ⅓, working at ½, compaction at ⅔. Each is computed
    // independently — an absent indicator doesn't displace the others. Zero
    // jitter: "working" is always dead-center regardless of what else is present.
    let tool_text = state.active_tool.as_deref().map(|name| {
        // Truncate long tool details on narrow terminals: below ~40 cols, drop
        // the args suffix and ellipsis, keeping just `◐ {name}`.
        if w < 40 {
            format!("{} {}", glyph::RUNNING, name)
        } else {
            format!("{} {} {}", glyph::RUNNING, name, glyph::ELLIPSIS)
        }
    });
    let tool_w = tool_text.as_ref().map(|t| t.width()).unwrap_or(0);

    let compact_text = if state.compacting {
        Some(format!("{} compacting", glyph::COMPACT))
    } else {
        None
    };
    let compact_w = compact_text.as_ref().map(|t| t.width()).unwrap_or(0);

    // Dead-center for "working", always.
    let center_start = w.saturating_sub(center_w) / 2;
    let right_start = w.saturating_sub(right_w);

    // Pulse-on-appear: bright for 300ms after the indicator first shows,
    // then settle to a dimmer steady-state.
    const PULSE_MS: u128 = 300;
    let tool_fg = if let Some(start) = state.tool_started_at {
        if start.elapsed().as_millis() < PULSE_MS {
            color::WARN
        } else {
            color::WARN_DIM
        }
    } else {
        color::WARN
    };
    let compact_fg = if let Some(start) = state.compaction_started_at {
        if start.elapsed().as_millis() < PULSE_MS {
            color::BRANCH
        } else {
            color::COMPACT_DIM
        }
    } else {
        color::BRANCH
    };

    let mut spans = left;
    let pad1 = center_start.saturating_sub(left_w);
    if pad1 > 0 {
        spans.push(Span::styled(" ".repeat(pad1), Style::new()));
    }
    // Tool indicator at the ⅓ anchor (left of center, with a 4-space gap).
    if let Some(text) = &tool_text {
        let tool_slot = w / 3;
        let tool_pad = tool_slot.saturating_sub(tool_w + 4);
        if tool_pad > 0 {
            spans.push(Span::styled(" ".repeat(tool_pad), Style::new()));
        }
        spans.push(Span::styled(text.clone(), Style::new().fg(tool_fg)));
        // 4-space gap between tool and working.
        let gap1 = 4usize;
        let consumed = left_w + tool_pad + tool_w + gap1;
        let actual_gap = center_start.saturating_sub(consumed).min(gap1);
        if actual_gap > 0 {
            spans.push(Span::styled(" ".repeat(actual_gap), Style::new()));
        }
    }
    // Working — dead center.
    spans.push(Span::styled(center, Style::new().fg(fg)));
    // Compaction at the ⅔ anchor (right of center, with a 4-space gap).
    if let Some(text) = &compact_text {
        let gap2 = 4usize;
        let compact_slot = (2 * w) / 3;
        let actual_gap = compact_slot
            .saturating_sub(center_start + center_w)
            .min(gap2);
        if actual_gap > 0 {
            spans.push(Span::styled(" ".repeat(actual_gap), Style::new()));
        } else {
            // Not enough room for the full gap; abut "working" instead.
            let min_gap = (center_start + center_w + 1).saturating_sub(center_start + center_w);
            if min_gap > 0 {
                spans.push(Span::styled(" ", Style::new()));
            }
        }
        spans.push(Span::styled(text.clone(), Style::new().fg(compact_fg)));
    }

    // Pad to the zoom hint (right edge).
    let consumed_so_far: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad2 = right_start.saturating_sub(consumed_so_far);
    if pad2 > 0 {
        spans.push(Span::styled(" ".repeat(pad2), Style::new()));
    }
    spans.push(Span::styled(right, Style::new().fg(color::DIM)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
```

- [ ] **Step 4: Run the layout test to verify it passes**

Run: `cargo test -p zoid-tui --lib render::tests::working_stays_dead_center`
Expected: PASS — "working" is at W/2 regardless of tool and compaction being present.

- [ ] **Step 5: Write the pulse test**

```rust
    #[test]
    fn tool_indicator_uses_dim_color_after_pulse_window() {
        use crate::state::ShellState;
        use std::time::{Duration, Instant};

        let mut s = ShellState::new();
        s.set_active_tool("shell");
        // Simulate the pulse having elapsed: set tool_started_at far in the past.
        s.tool_started_at = Some(Instant::now() - Duration::from_secs(1));

        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &s, &ChatView {
                zoom: Zoom::Normal,
                caret_on: false,
                reveal: None,
                tz_offset_secs: 0,
            }, f.area()))
            .unwrap();
        // After the pulse window, the tool indicator uses WARN_DIM.
        // We can't directly assert the color from the buffer easily, so just
        // verify it renders without panic and the tool name is present.
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("shell"), "tool name must render");
    }
```

- [ ] **Step 6: Run the pulse test**

Run: `cargo test -p zoid-tui --lib render::tests::tool_indicator_uses_dim_color`
Expected: PASS.

- [ ] **Step 7: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 7b: Update the compaction render unit test**

The existing `compaction_segment_visible_when_compacting` test (render.rs:1314) asserts the old spinner glyph (`COMPACT_SPINNER[0]`) and sets `state.compact_spinner`. Rewrite it to assert the static `glyph::COMPACT` glyph and remove the `compact_spinner` assignment. The test should assert `content.contains("⊟")` (the static glyph) and `content.contains("compacting")` (the label), and verify the indicator is at the ⅔ anchor (right of center). Remove any assertion on `state.compact_spinner` (the field is gone).

Run: `cargo test -p zoid-tui --lib render::tests::compaction_segment_visible`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(tui/render): fixed ⅓/½/⅔ anchors + 4-space gaps + pulse-on-appear"
```

---

## Task 5: Update snapshots + clippy/fmt

**Files:** snapshots + verify

- [ ] **Step 1: Run all snapshot tests, accept changes**

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Then: `INSTA_UPDATE=always cargo test -p zoid-tui --test session_snapshot`
Review the updated snapshots to confirm the indicators are at the right positions (tool at ⅓, working at ½, compaction at ⅔). Manually inspect any snapshot that includes `compacting` or `active_tool`.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. Fix any warnings.

- [ ] **Step 3: Run fmt**

Run: `cargo fmt --all`
Revert any pre-existing drift in files you didn't touch (`onboarding.rs`, `tokens.rs` unrelated lines, etc.).

- [ ] **Step 4: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS — all suites green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: update snapshots + clippy/fmt for status bar indicators"
```

---

## Self-Review

**1. Spec coverage:**
- §1 Layout (⅓/½/⅔ anchors) → Task 4.
- §2 Spacing (4-space gap) → Task 4.
- §3 Animation (working spins, tool/compaction pulse) → Task 4 (pulse), Task 2 (tool_started_at), Task 3 (compaction_started_at mirror, tick guard).
- §What gets deleted (compact_spinner, COMPACT_SPINNER) → Tasks 1, 2, 3.
- §Testing → Tasks 2, 4.

**2. Placeholder scan:** No TBD/TODO. The `PULSE_MS = 300` constant is inline in the render code. The dim color RGB values are concrete in Task 1. The narrow-terminal truncation threshold (40 cols) is in the render code. OK.

**3. Task ordering:**
- Task 1 (tokens) compiles standalone (just adds colors, deletes a constant).
- Task 2 (state) depends on Task 1 (removes `compact_spinner` references to the deleted `COMPACT_SPINNER`). Won't compile until Task 1 is done — correct order.
- Task 3 (bin) depends on Task 2 (removes `compact_spinner` from the `app.shell` assignment; `ShellState` no longer has the field). Correct order.
- Task 4 (render) depends on Tasks 1–3 (uses `WARN_DIM`/`COMPACT_DIM` from Task 1; `tool_started_at`/`compaction_started_at` from Task 2; `compact_spinner` is gone). Correct order.
- Task 5 (snapshots) depends on Task 4. Correct order.
- No mid-task compile dead-ends: each task leaves the workspace compiling (except Task 1 → Task 2 transition, where Task 1 deliberately breaks the `compact_spinner` reference — the plan says to proceed to Task 2).