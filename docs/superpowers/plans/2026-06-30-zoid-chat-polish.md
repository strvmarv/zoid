# zoid Chat Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish the Chat surface — fix the input box (underline, ⇧⏎ newline, grow/shrink), consolidate the mode/branch chrome, drop rail keybind labels, and render markdown + fenced code in messages — to match `docs/ux/chat-mode.html`.
**Architecture:** `zoid-tui` is a pure render/state/layout library (ratatui); the `zoid` bin owns the terminal, the event loop, and the `TextArea`. Views render only from `tokens.rs` (spec §16); `layout::compute` is the single source of rects for both drawing and mouse hit-testing.
**Tech Stack:** Rust 2021, ratatui 0.29, crossterm 0.28, tui-textarea 0.7, pulldown-cmark 0.13 (new), insta (snapshots).
**Prerequisites:** none (all edits to existing code, plus one new dependency and one new module). This is Plan 1 of 3; Plan 2 (Sessions & DB) and Plan 3 (P5 delegation) build on top and are independent documents.

## Global Constraints
- Rust edition 2021; cargo workspace crates: zoid-core, zoid-provider, zoid-tui, zoid-tools, zoid-syntax, and the `zoid` bin. Release profile is size-optimized (opt-level="z", LTO, panic=abort) — keep new dependencies minimal (pulldown-cmark is added with `default-features = false`).
- §16 Design tokens: NO literal glyphs or raw color hex anywhere outside `crates/zoid-tui/src/tokens.rs`. Every view renders from tokens. New glyphs/colors are added to `tokens.rs` FIRST. (Numeric layout constants are NOT tokens; they live in `layout.rs` alongside `RAIL_WIDTH`.)
- TDD is the default workflow.
- Every new/changed TUI screen ships an `insta` snapshot test using `format!("{:#?}", terminal.backend().buffer())` (Buffer Debug — captures style), built to match its `docs/ux/` mockup. Snapshots live in `crates/zoid-tui/tests/snapshots/`. The canonical mock for Chat is `docs/ux/chat-mode.html`.
- The first `cargo insta test` run creates a pending snapshot (review it) rather than failing red; for those tasks the "fails" step is "snapshot pending/review". Requires `cargo-insta` (`cargo install cargo-insta` if missing).
- Commit messages END with the trailer `Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY`. NEVER add a "Co-Authored-By" or any co-author trailer.

## Out of scope (belongs to Plan 2 — Sessions & DB)
Do NOT do these here: the rail restructure to **repo / session / context** drawers, the working-tree "changes" line, the session widget (name/model/duration/tokens/cwd), dropping the **files** drawer, and moving model/provider/tokens off the title bar into the session widget. This plan keeps the current drawer set (economy / files / branch) and only removes their keybind labels; branch keeps its current home in the Branch drawer.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/zoid/src/main.rs` | modify | Input helper (`make_input`, underline off); keyboard-enhancement flags on setup/teardown; set `shell.input_rows` each frame. |
| `crates/zoid-tui/src/route.rs` | modify | Map `Shift+Enter` → `Newline` (Alt+Enter kept as fallback); route unit test. |
| `crates/zoid-tui/src/layout.rs` | modify | `MAX_INPUT_ROWS` const + `input_height()` helper; variable input-box height in `compute`. |
| `crates/zoid-tui/src/state.rs` | modify | Add `input_rows` field; remove `Drawer.keybind` field + assignments. |
| `crates/zoid-tui/src/tokens.rs` | modify | Add `glyph::IDLE` (activity); add markdown tokens (`glyph::BULLET`, `glyph::QUOTE_BAR`, `color::MD_CODE`, `color::MD_LINK`). |
| `crates/zoid-tui/src/render.rs` | modify | `render_title` = app name + activity indicator; `render_status` = mode chip + minimal hint; drop drawer keybind span. |
| `crates/zoid-tui/src/markdown.rs` | **create** | Parse markdown → `Vec<Line>` styled from tokens; fenced code via `highlight_lines`. |
| `crates/zoid-tui/src/chat.rs` | modify | Wire markdown into `conversation_lines` via a `push_message` helper. |
| `crates/zoid-tui/src/lib.rs` | modify | `pub mod markdown;`. |
| `crates/zoid-tui/Cargo.toml` | modify | Add `pulldown-cmark`. |
| `crates/zoid-tui/tests/shell_snapshot.rs` | modify | New snapshots: grown input box, markdown message. |

---

### Task 1: Input cursor-line underline fix (spec §2.2, §9)
The input is a `tui_textarea::TextArea` created in three places in the bin: the `App` initializer (`main.rs:123`), on submit (`main.rs:309`), and from a verb prompt (`main.rs:353`). None calls `set_cursor_line_style`, so tui-textarea's DEFAULT cursor-line style (an underline) shows. Fix with one helper used at all three sites.

**Files:**
- Modify: `crates/zoid/src/main.rs` (add helper near top; three call sites; add `#[cfg(test)] mod tests`)
- Test: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `tui_textarea::TextArea` (already imported, `main.rs:14`).
- Produces: `fn make_input(textarea: TextArea<'static>) -> TextArea<'static>` (used by Tasks 2–3 wiring too).

- [ ] **Step 1: Write the failing test.** Append to the end of `crates/zoid/src/main.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use ratatui::{backend::TestBackend, style::Modifier, Terminal};
      use tui_textarea::TextArea;

      /// Render the textarea into a scratch buffer and report whether any cell
      /// carries the UNDERLINED modifier (tui-textarea's default cursor line).
      fn has_underline(ta: &TextArea<'static>) -> bool {
          let backend = TestBackend::new(20, 3);
          let mut term = Terminal::new(backend).unwrap();
          term.draw(|f| f.render_widget(ta, f.area())).unwrap();
          term.backend()
              .buffer()
              .content()
              .iter()
              .any(|c| c.modifier.contains(Modifier::UNDERLINED))
      }

      #[test]
      fn make_input_disables_cursor_line_underline() {
          // Sanity: the tui-textarea default underlines the cursor line.
          let default = TextArea::from(vec!["hello".to_string()]);
          assert!(has_underline(&default), "default TextArea underlines the cursor line");
          // make_input turns it off.
          let plain = make_input(TextArea::from(vec!["hello".to_string()]));
          assert!(!has_underline(&plain), "make_input must disable the cursor-line underline");
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid make_input_disables_cursor_line_underline`
  Expected: compile error `cannot find function `make_input` in this scope` (the helper does not exist yet).

- [ ] **Step 3: Write minimal implementation.** Add the helper just below `now_ms` (after `main.rs:48`):
  ```rust
  /// Build the message input with the tui-textarea cursor-line **underline**
  /// disabled (spec §2.2/§9): the default underline clutters the calm box.
  fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
      let mut textarea = textarea;
      textarea.set_cursor_line_style(ratatui::style::Style::default());
      textarea
  }
  ```
  Then wrap the three creation sites:
  - `main.rs:123` — change `textarea: TextArea::default(),` to `textarea: make_input(TextArea::default()),`
  - `main.rs:309` — change `app.textarea = TextArea::default();` to `app.textarea = make_input(TextArea::default());`
  - `main.rs:353` — change `app.textarea = TextArea::from(prompt.lines().map(String::from).collect::<Vec<_>>());` to `app.textarea = make_input(TextArea::from(prompt.lines().map(String::from).collect::<Vec<_>>()));`

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid make_input_disables_cursor_line_underline`
  Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Commit.**
  ```
  git add crates/zoid/src/main.rs
  git commit -m "fix(input): disable tui-textarea cursor-line underline (§2.2/§9)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 2: Shift+Enter newline — keyboard-enhancement flags + keymap (spec §2.2, §9)
`⏎` submits; `⇧⏎` should insert a newline, with `Alt+⏎` kept as the fallback. Terminals only report `Shift+Enter` distinctly when crossterm's keyboard-enhancement flags (Kitty protocol `DISAMBIGUATE_ESCAPE_CODES`) are active — enable them at startup (guarded by `supports_keyboard_enhancement()`, degrade gracefully) and pop them on exit. The pure keymap lives in `route.rs`.

**Files:**
- Modify: `crates/zoid/src/main.rs` (crossterm imports; setup/teardown `main.rs:131-142`)
- Modify: `crates/zoid-tui/src/route.rs` (Input arm `route.rs:110-115`)
- Test: `crates/zoid-tui/src/route.rs` (`input_focus_edits_and_submits`, `route.rs:250-256`)

**Interfaces:**
- Consumes: `Action::Newline`, `route_key` (existing, `route.rs`).
- Produces: no new public symbols; behavioral change to `route_key` for `KeyCode::Enter + SHIFT`.

- [ ] **Step 1: Write the failing test.** Extend `input_focus_edits_and_submits` in `crates/zoid-tui/src/route.rs` (add the SHIFT line after the ALT line, `route.rs:254`):
  ```rust
      #[test]
      fn input_focus_edits_and_submits() {
          let s = ShellState::new(); // focus Input
          assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::Submit);
          assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::ALT)), Action::Newline);
          assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::SHIFT)), Action::Newline);
          assert!(matches!(route_key(&s, key(KeyCode::Char('h'), KeyModifiers::NONE)), Action::Edit(_)));
      }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui input_focus_edits_and_submits`
  Expected: `assertion `left == right` failed` — `Shift+Enter` currently returns `Action::Edit(...)` (falls through), not `Action::Newline`.

- [ ] **Step 3: Write minimal implementation.** In `route.rs` the Input focus arm (`route.rs:111-115`) — extend the newline guard to accept SHIFT as well as ALT:
  ```rust
          Focus::Input => match (key.code, key.modifiers) {
              // ⇧⏎ (keyboard-enhancement flags on) or Alt+⏎ (fallback) → newline.
              (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::SHIFT) => Action::Newline,
              (KeyCode::Enter, _) => Action::Submit,
              _ => Action::Edit(key),
          },
  ```
  Then enable the flags in the bin. Update the crossterm import block (`main.rs:2-6`) to:
  ```rust
  use crossterm::{
      event::{
          DisableMouseCapture, EnableMouseCapture, Event as CEvent, EventStream,
          KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
      },
      execute,
      terminal::{
          disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
          LeaveAlternateScreen,
      },
  };
  ```
  And the setup/teardown block (`main.rs:131-142`) becomes:
  ```rust
      enable_raw_mode()?;
      let mut out = stdout();
      execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
      // Kitty keyboard protocol: lets the terminal report ⇧⏎ distinctly from ⏎ so
      // route.rs can map Shift+Enter → newline. Degrade gracefully — only push the
      // flags when supported; otherwise the Alt+⏎ fallback stands.
      let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
      if kbd_enhanced {
          execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
      }
      let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

      let result = run(&mut terminal, &mut app, &mut ui_rx).await;

      // Restore the terminal on every exit path — drive through errors, don't bail.
      if kbd_enhanced {
          let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
      }
      let _ = disable_raw_mode();
      let _ = execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
      let _ = terminal.show_cursor();
      result
  ```

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui input_focus_edits_and_submits` (expected `ok. 1 passed`) then `cargo build -p zoid` (expected: builds clean — verifies the crossterm imports + setup/teardown compile).

- [ ] **Step 5: Commit.**
  ```
  git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
  git commit -m "feat(input): ⇧⏎ newline via keyboard-enhancement flags, Alt+⏎ fallback (§2.2)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 3: Growing / shrinking message box (spec §2.2, §9)
The input box renders at a fixed 3 rows (`layout::compute`, `Constraint::Length(3)`, `layout.rs:53`). Make its height track the textarea's line count — grow to a cap, then let tui-textarea scroll internally, and shrink back after submit (the textarea is recreated empty on submit, so this recomputes naturally). `compute` only sees `ShellState`, so the line count is threaded through a new `input_rows` field the bin sets each frame.

**Files:**
- Modify: `crates/zoid-tui/src/layout.rs` (const + helper `layout.rs:17-19`; input constraint `layout.rs:50-57`)
- Modify: `crates/zoid-tui/src/state.rs` (add `input_rows`, `state.rs:82-133`)
- Modify: `crates/zoid/src/main.rs` (set `shell.input_rows` each loop, before `terminal.draw`, `main.rs:163-203`)
- Test: `crates/zoid-tui/src/layout.rs`; snapshot `crates/zoid-tui/tests/shell_snapshot.rs`

**Interfaces:**
- Consumes: `ShellState` (`state.rs`), `compute` (`layout.rs`).
- Produces: `pub const MAX_INPUT_ROWS: u16`; `pub fn input_height(lines: u16) -> u16`; `ShellState.input_rows: u16` (default 1).

- [ ] **Step 1: Write the failing test.** Add to `crates/zoid-tui/src/layout.rs` `mod tests` (after `narrow_hides_rail`, ~`layout.rs:144`):
  ```rust
      #[test]
      fn input_height_grows_and_clamps() {
          assert_eq!(input_height(1), 3, "one line → 3 rows (content + 2 borders); post-submit resting height");
          assert_eq!(input_height(4), 6, "grows with content");
          assert_eq!(input_height(MAX_INPUT_ROWS), MAX_INPUT_ROWS + 2, "at the cap");
          assert_eq!(input_height(MAX_INPUT_ROWS + 5), MAX_INPUT_ROWS + 2, "clamps past the cap");
          assert_eq!(input_height(0), 3, "min one content row");
      }

      #[test]
      fn compute_input_area_tracks_input_rows() {
          let mut s = ShellState::new();
          s.input_rows = 4;
          let l = compute(area(100, 30), &s);
          assert_eq!(l.input.height, input_height(4));
      }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui input_height_grows_and_clamps`
  Expected: compile error `cannot find function `input_height`` (and `MAX_INPUT_ROWS` / `input_rows` unresolved).

- [ ] **Step 3: Write minimal implementation.**
  Add the constant + helper to `layout.rs` (after `ECONOMY_BODY_ROWS`, ~`layout.rs:19`):
  ```rust
  /// Message-box max content rows before it stops growing and scrolls internally
  /// (spec §2.2). Not a §16 token — a numeric layout constant, like RAIL_WIDTH.
  pub const MAX_INPUT_ROWS: u16 = 8;

  /// Total input-box height (content + top/bottom borders) for a wrapped line
  /// count: grows with content, clamps at MAX_INPUT_ROWS, min one content row.
  pub fn input_height(lines: u16) -> u16 {
      lines.clamp(1, MAX_INPUT_ROWS) + 2
  }
  ```
  In `compute`, change the input row constraint (`layout.rs:53`) from `Constraint::Length(3),` to `Constraint::Length(input_height(state.input_rows)),`.
  Add the field to `ShellState` (after `status_hint`, `state.rs:105`):
  ```rust
      /// Wrapped line count of the message input, sampled by the bin each frame so
      /// `layout::compute` can grow/shrink the box (spec §2.2). Default 1 (resting).
      pub input_rows: u16,
  ```
  Initialize it in `ShellState::new` (after `status_hint: None,`, `state.rs:131`):
  ```rust
              status_hint: None,
              input_rows: 1,
  ```
  In the bin, sample it at the top of the event loop, immediately BEFORE `terminal.draw` (`main.rs:163-164`):
  ```rust
      loop {
          app.shell.input_rows = app.textarea.lines().len().max(1) as u16;
          terminal.draw(|f| {
  ```

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui input_height_grows_and_clamps compute_input_area_tracks_input_rows` (expected `ok. 2 passed`), then `cargo build -p zoid` (expected clean — verifies the bin sample line compiles).

- [ ] **Step 5: Add the grown-box snapshot.** Append to `crates/zoid-tui/tests/shell_snapshot.rs`:
  ```rust
  /// The message box grows with its content (spec §2.2). A 3-line input yields a
  /// 5-row box (3 content + 2 borders); Buffer-Debug captures the taller frame.
  #[test]
  fn growing_message_box_frame() {
      let mut s = ShellState::new();
      s.input_rows = 3;
      let input = TextArea::from(vec!["line one".to_string(), "line two".to_string(), "line three".to_string()]);
      let backend = TestBackend::new(100, 24);
      let mut terminal = Terminal::new(backend).unwrap();
      terminal
          .draw(|f| render_shell(f, &s, &empty_economy(), &seeded(), &input, false, &normal_view()))
          .unwrap();
      insta::assert_snapshot!(format!("{:#?}", terminal.backend().buffer()));
  }
  ```
  Run `cargo insta test -p zoid-tui --review`. Expected: one **new** pending snapshot `growing_message_box_frame`. Accept it after eyeballing that the box is taller (5 rows) and the three lines render — matching `docs/ux/chat-mode.html`.

- [ ] **Step 6: Commit.**
  ```
  git add crates/zoid-tui/src/layout.rs crates/zoid-tui/src/state.rs crates/zoid/src/main.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
  git commit -m "feat(input): grow/shrink message box with content, cap + internal scroll (§2.2)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 4: Drop rail drawer keybind labels (spec §2.1)
The `Drawer` struct has a `keybind` field (`state.rs:59`) set to `^5`/`^F`/`^B` in `ShellState::new` (`state.rs:113-115`) and rendered in the rail header (`render.rs:150`). Remove the label from the render and the now-unused field. **Do NOT touch `PaletteItem.keybind`** (`palette.rs:16`, rendered `render.rs:268`) — palette rows still teach their shortcuts (§4).

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render.rs:145-152`)
- Modify: `crates/zoid-tui/src/state.rs` (`state.rs:55-61`, `state.rs:111-116`, comment `state.rs:222`)
- Test: `crates/zoid-tui/tests/shell_snapshot.rs` (snapshot review)

**Interfaces:**
- Consumes: `state.drawer(id)`, `Drawer { id, title, open }` (after edit).
- Produces: `Drawer` without a `keybind` field (Plan 2 relies on this trimmed struct).

- [ ] **Step 1: Write the failing test.** The behavior is "no keybind label in the rail" — assert the old labels are absent from a rail frame. Add to `crates/zoid-tui/tests/shell_snapshot.rs`:
  ```rust
  /// Rail drawer headers show title + chevron only — no keybind labels (spec §2.1).
  #[test]
  fn rail_headers_have_no_keybind_labels() {
      let s = ShellState::new();
      let out = draw(&s, &seeded(), 100, 24);
      assert!(!out.contains("^5"), "economy keybind label must be gone");
      assert!(!out.contains("^F"), "files keybind label must be gone");
      assert!(!out.contains("^B"), "branch keybind label must be gone");
  }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui rail_headers_have_no_keybind_labels`
  Expected: `assertion failed: !out.contains("^5")` — the labels currently render in the rail.

- [ ] **Step 3: Write minimal implementation.**
  In `render.rs` the drawer header (`render.rs:148-151`) — drop the keybind span:
  ```rust
          let hdr = Line::from(vec![
              Span::styled(format!("{chevron} {}", d.title), Style::new().fg(if d.open { color::TXT } else { color::DIM })),
          ]);
  ```
  In `state.rs`, remove the field from the struct (`state.rs:55-61`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Drawer {
      pub id: DrawerId,
      pub title: String,
      pub open: bool,
  }
  ```
  And drop `keybind:` from the three constructors (`state.rs:113-115`):
  ```rust
          let drawers = vec![
              Drawer { id: DrawerId::Economy, title: "context · tokens".into(), open: true },
              Drawer { id: DrawerId::Files,   title: "files".into(),             open: false },
              Drawer { id: DrawerId::Branch,  title: "branch".into(),            open: false },
          ];
  ```
  Update the stale comment (`state.rs:222`) from `// Chat rail set, in order, with the canonical keybinds.` to `// Chat rail set, in order (no keybind labels — §2.1).`

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui rail_headers_have_no_keybind_labels` (expected `ok. 1 passed`), then `cargo test -p zoid-tui --lib` (expected: all `state`/`render` unit tests still pass — proves nothing else referenced `keybind`).

- [ ] **Step 5: Update affected snapshots.** `cargo insta test -p zoid-tui --review`
  Expected: the rail-bearing shell snapshots (`chat_with_rail_frame`, `chat_wide_gutter_frame`, `files_drawer_open_frame`, `economy_drawer_frame`, `economy_drawer_wide_frame`, `palette_overlay_frame`, `command_line_frame`, `zoom_*`, `object_*`, `verb_*`) lose the `^5`/`^F`/`^B` header suffix. Accept each after confirming only the keybind text disappeared, matching `docs/ux/chat-mode.html`. (`chat_snapshot.rs` frames use the legacy `render_chat` with no rail — unchanged.)

- [ ] **Step 6: Commit.**
  ```
  git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/state.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
  git commit -m "refactor(rail): drop drawer keybind labels + field (§2.1)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 5: Chrome consolidation — title activity indicator + minimal status (spec §2.2)
Move each chrome element to one home. Title bar = **app name + live activity indicator** (`● idle` when waiting; `⠿ running` while streaming), dropping the mode chip and branch. Status bar = **mode chip (left) + `zoom <level> · ^P palette`**, dropping branch, `⇧Tab → Build`, and `^C quit`. `render_shell` already carries `streaming: bool`; thread it to the title.

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` (add `glyph::IDLE`; new token test)
- Modify: `crates/zoid-tui/src/render.rs` (`render_title` `render.rs:34,80-91`; `render_status` `render.rs:113-129`)
- Test: `crates/zoid-tui/src/tokens.rs`; snapshot `crates/zoid-tui/tests/shell_snapshot.rs`

**Interfaces:**
- Consumes: `render_shell`'s `streaming: bool` (`render.rs:29`), `view.zoom.label()` (`state.rs:30`), `glyph::STREAM` (`tokens.rs:10`).
- Produces: `glyph::IDLE`; `render_title(frame: &mut Frame, streaming: bool, area: Rect)` (state param dropped).

- [ ] **Step 1: Write the failing test.** Add the activity token test to `crates/zoid-tui/src/tokens.rs` `mod tests` (after `p4c_collapse_token_present`, ~`tokens.rs:110`):
  ```rust
      #[test]
      fn chat_polish_activity_token_present() {
          assert_eq!(glyph::IDLE, '●'); // title-bar idle activity indicator (§2.2)
          assert_eq!(glyph::STREAM, '⠿'); // running indicator reuses the stream glyph
      }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui chat_polish_activity_token_present`
  Expected: compile error `no associated item named `IDLE` found` for `glyph`.

- [ ] **Step 3: Write minimal implementation.**
  Add the glyph to `tokens.rs` `glyph` module (after `ELLIPSIS`, `tokens.rs:31`):
  ```rust
      pub const IDLE: char = '●';        // title-bar activity — waiting for the user (§2.2)
  ```
  Rewrite `render_title` (`render.rs:80-91`) — drop `state`, add `streaming`:
  ```rust
  fn render_title(frame: &mut Frame, streaming: bool, area: Rect) {
      // App name + a live activity indicator only (spec §2.2): idle when waiting
      // for the user, running while the agent streams or a tool runs. The mode chip
      // lives in the status bar; branch lives in the rail.
      let (icon, label, fg) = if streaming {
          (glyph::STREAM, "running", color::CHAT_ACCENT)
      } else {
          (glyph::IDLE, "idle", color::DIM)
      };
      let title = Line::from(vec![
          Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
          Span::styled(format!("{icon} {label}"), Style::new().fg(fg)),
      ]);
      frame.render_widget(Paragraph::new(title), area);
  }
  ```
  Update the call site (`render.rs:34`) from `render_title(frame, state, layout.title);` to `render_title(frame, streaming, layout.title);`.
  Trim the Chat status hint in `render_status` (`render.rs:114-124`) — keep the chip, drop branch / `⇧Tab → Build` / `^C quit`:
  ```rust
      let mut spans = match state.mode {
          Mode::Chat => vec![
              Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG)),
              Span::styled(
                  format!(" zoom {} · ^P palette", view.zoom.label()),
                  Style::new().fg(color::DIM),
              ),
          ],
          Mode::Build => vec![
              Span::styled(" BUILD ", Style::new().fg(color::BUILD_ACCENT).bg(color::BUILD_BG)),
              Span::styled(" phase —/— · esc → Chat", Style::new().fg(color::DIM)),
          ],
      };
  ```
  (Leave the transient `status_hint` append, `render.rs:132-134`, unchanged. The Build arm is unchanged.)

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui chat_polish_activity_token_present` (expected `ok. 1 passed`), then `cargo build -p zoid-tui` (expected clean — verifies the `render_title` call-site + `glyph::BRANCH` no longer needed there compiles; note `glyph`/`color` imports are unchanged).

- [ ] **Step 5: Update affected snapshots.** `cargo insta test -p zoid-tui --review`
  Expected: **every** shell snapshot changes (title bar drops the `CHAT`/`⎇ main` chrome and shows `● idle`; status bar drops `⎇ main`, `⇧Tab → Build`, `^C quit`, and now reads `zoom normal · ^P palette`). Accept each after confirming the title = `zoid  ● idle` and the status = chip + `zoom <level> · ^P palette`, matching `docs/ux/chat-mode.html`. (`chat_snapshot.rs` uses `render_chat`, which this task does not touch — unchanged.)

- [ ] **Step 6: Commit.**
  ```
  git add crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests/snapshots/
  git commit -m "feat(chrome): title activity indicator; minimal status; one home per element (§2.2)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 6: Markdown tokens + dependency + renderer module (spec §3.5)
Add `pulldown-cmark`, the markdown design tokens, and a pure `markdown.rs` that parses markdown into styled `Vec<Line>` — headings, bold/italic, inline code, lists, blockquotes, links → tokens; fenced ```lang blocks → the existing Ⓡ3 `highlight_lines`. No wiring into chat yet (Task 7). Depth-capped; plaintext fallback.

**Files:**
- Modify: `crates/zoid-tui/Cargo.toml`
- Modify: `crates/zoid-tui/src/tokens.rs` (markdown tokens + test)
- Create: `crates/zoid-tui/src/markdown.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod markdown;`)
- Test: `crates/zoid-tui/src/tokens.rs`, `crates/zoid-tui/src/markdown.rs`

**Interfaces:**
- Consumes: `highlight_lines(source, lang)` (`syntax_view.rs:25`), `zoid_syntax::Language` (variants `Rust`/`Toml`/`Json`/`Yaml`/`Markdown`/`PlainText`), `color::*`, `glyph::*`.
- Produces: `pub fn render_markdown(source: &str) -> Vec<ratatui::text::Line<'static>>` (consumed by Task 7); `glyph::BULLET`, `glyph::QUOTE_BAR`, `color::MD_CODE`, `color::MD_LINK`.

- [ ] **Step 1: Write the failing test.** First the tokens, then the module.
  Add to `tokens.rs` `mod tests`:
  ```rust
      #[test]
      fn markdown_tokens_present() {
          assert_eq!(glyph::BULLET, '•');
          assert_eq!(glyph::QUOTE_BAR, '│');
          assert_eq!(color::MD_CODE, color::SYN_STRING); // inline/`code` reuses the string hue
          assert_eq!(color::MD_LINK, color::CHAT_ACCENT); // links use the Chat accent
      }
  ```
  Create `crates/zoid-tui/src/markdown.rs` with ONLY the test module for now (the impl arrives in Step 3):
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use ratatui::style::Modifier;

      fn spans(lines: &[ratatui::text::Line<'static>]) -> Vec<(String, ratatui::style::Style)> {
          lines.iter().flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style))).collect()
      }

      #[test]
      fn plain_prose_is_one_txt_line() {
          let lines = render_markdown("just a sentence.");
          assert_eq!(lines.len(), 1);
          assert!(lines[0].spans.iter().all(|s| s.style.fg == Some(color::TXT)));
      }

      #[test]
      fn heading_is_accent_bold() {
          let lines = render_markdown("# Title");
          let (_, style) = spans(&lines).into_iter().find(|(t, _)| t.contains("Title")).unwrap();
          assert_eq!(style.fg, Some(color::CHAT_ACCENT));
          assert!(style.add_modifier.contains(Modifier::BOLD));
      }

      #[test]
      fn bold_and_inline_code_are_styled() {
          let lines = render_markdown("a **b** `c`");
          let s = spans(&lines);
          assert!(s.iter().any(|(t, st)| t == "b" && st.add_modifier.contains(Modifier::BOLD)));
          assert!(s.iter().any(|(t, st)| t == "c" && st.fg == Some(color::MD_CODE)));
      }

      #[test]
      fn list_items_render_with_bullets() {
          let lines = render_markdown("- one\n- two");
          assert_eq!(lines.len(), 2);
          let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
          assert!(text.iter().any(|t: &String| t.contains(glyph::BULLET) && t.contains("one")));
      }

      #[test]
      fn fenced_code_is_highlighted_by_language() {
          let lines = render_markdown("```rust\nfn x() {}\n```");
          let has_kw = lines.iter().any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::SYN_KEYWORD)));
          assert!(has_kw, "a rust fence must be syntax-highlighted");
      }

      #[test]
      fn unknown_fence_is_plain_text() {
          let lines = render_markdown("```\nplain body\n```");
          assert!(lines.iter().all(|l| l.spans.iter().all(|s| s.style.fg == Some(color::TXT))));
      }
  }
  ```
  Add `pub mod markdown;` to `crates/zoid-tui/src/lib.rs` (after `pub mod chat;`, `lib.rs:6`).

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui markdown`
  Expected: compile errors — `render_markdown` unresolved and `pulldown-cmark` / markdown tokens not yet present. (The `markdown_tokens_present` token test also fails to compile: `no associated item named `BULLET``.)

- [ ] **Step 3: Write minimal implementation.**
  Add the dependency to `crates/zoid-tui/Cargo.toml` `[dependencies]` (after the `unicode-width` line):
  ```toml
  # Message-body markdown → styled spans (spec §3.5). Pure-Rust, no runtime;
  # default features off (drops the html/getopts extras) to respect the size budget.
  pulldown-cmark = { version = "0.13", default-features = false }
  ```
  Add the markdown tokens. In `tokens.rs` `glyph` module (after `IDLE` from Task 5):
  ```rust
      pub const BULLET: char = '•';      // markdown unordered-list marker (§3.5)
      pub const QUOTE_BAR: char = '│';   // markdown blockquote bar (§3.5)
  ```
  In `tokens.rs` `color` module (after the `SYN_*` block, `tokens.rs:59`):
  ```rust
      // Markdown message rendering (spec §3.5) — reuse the existing palette so the
      // visual language stays uniform: inline/fenced `code` = string hue, links = accent.
      pub const MD_CODE: Color = SYN_STRING;
      pub const MD_LINK: Color = CHAT_ACCENT;
  ```
  Prepend the implementation ABOVE the `#[cfg(test)] mod tests` in `crates/zoid-tui/src/markdown.rs`:
  ```rust
  //! Render assistant/user message bodies from markdown to ratatui `Line`s
  //! (spec §3.5). `pulldown-cmark` parses; inline styles (headings, bold, italic,
  //! inline `code`, lists, blockquotes, links) map to §16 design tokens, and
  //! fenced ```lang blocks reuse the Ⓡ3 highlighter (`highlight_lines`). Nesting is
  //! depth-capped; anything unexpected falls back to plain text. Wrapping is the
  //! caller's job (`Wrap { trim: false }`) — we only build styled spans/lines.

  use crate::syntax_view::highlight_lines;
  use crate::tokens::{color, glyph};
  use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
  use ratatui::style::{Modifier, Style};
  use ratatui::text::{Line, Span};
  use zoid_syntax::Language;

  /// Max container nesting (lists + blockquotes) before we bail to plain text.
  const MAX_DEPTH: usize = 8;

  /// Render markdown `source` into owned ratatui `Line`s. Non-empty input yields
  /// at least one line; empty input yields an empty vec.
  pub fn render_markdown(source: &str) -> Vec<Line<'static>> {
      let mut b = Builder::default();
      let mut opts = Options::empty();
      opts.insert(Options::ENABLE_STRIKETHROUGH);
      for ev in Parser::new_ext(source, opts) {
          b.event(ev);
          if b.bail {
              return plain_lines(source);
          }
      }
      b.finish()
  }

  /// One TXT-styled `Line` per source row — the parse-issue / over-nesting fallback.
  fn plain_lines(source: &str) -> Vec<Line<'static>> {
      source
          .split('\n')
          .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(color::TXT))))
          .collect()
  }

  /// Resolve a fenced-code info string ("rust", "rs", "toml", …) to a Language;
  /// unknown/empty → PlainText (renders without highlighting).
  fn lang_from_fence(info: &str) -> Language {
      match info
          .split_whitespace()
          .next()
          .unwrap_or("")
          .to_ascii_lowercase()
          .as_str()
      {
          "rust" | "rs" => Language::Rust,
          "toml" => Language::Toml,
          "json" => Language::Json,
          "yaml" | "yml" => Language::Yaml,
          "md" | "markdown" => Language::Markdown,
          _ => Language::PlainText,
      }
  }

  #[derive(Default)]
  struct Builder {
      lines: Vec<Line<'static>>,
      cur: Vec<Span<'static>>,
      bold: u32,
      italic: u32,
      code: bool,
      link: bool,
      heading: bool,
      quote: u32,
      list: Vec<Option<u64>>, // per level: next ordinal (Some) or bullet (None)
      fence: Option<Language>,
      code_buf: String,
      bail: bool,
  }

  impl Builder {
      fn style(&self) -> Style {
          let mut fg = color::TXT;
          if self.heading {
              fg = color::CHAT_ACCENT;
          }
          if self.quote > 0 {
              fg = color::DIM;
          }
          if self.code {
              fg = color::MD_CODE;
          }
          if self.link {
              fg = color::MD_LINK;
          }
          let mut m = Modifier::empty();
          if self.bold > 0 || self.heading {
              m |= Modifier::BOLD;
          }
          if self.italic > 0 {
              m |= Modifier::ITALIC;
          }
          if self.link {
              m |= Modifier::UNDERLINED;
          }
          Style::new().fg(fg).add_modifier(m)
      }

      fn text(&mut self, t: &str) {
          self.cur.push(Span::styled(t.to_string(), self.style()));
      }

      fn flush(&mut self) {
          if self.cur.is_empty() {
              return;
          }
          let mut spans: Vec<Span<'static>> = Vec::new();
          for _ in 0..self.quote {
              spans.push(Span::styled(
                  format!("{} ", glyph::QUOTE_BAR),
                  Style::new().fg(color::DIM),
              ));
          }
          spans.append(&mut self.cur);
          self.lines.push(Line::from(spans));
      }

      fn event(&mut self, ev: Event) {
          match ev {
              Event::Start(tag) => self.start(tag),
              Event::End(end) => self.end(end),
              Event::Text(t) => {
                  if self.fence.is_some() {
                      self.code_buf.push_str(&t);
                  } else {
                      self.text(&t);
                  }
              }
              Event::Code(c) => {
                  self.code = true;
                  self.text(&c);
                  self.code = false;
              }
              Event::SoftBreak => self.text(" "),
              Event::HardBreak => self.flush(),
              _ => {}
          }
      }

      fn start(&mut self, tag: Tag) {
          match tag {
              Tag::Paragraph => self.flush(),
              Tag::Heading { .. } => {
                  self.flush();
                  self.heading = true;
              }
              Tag::Strong => self.bold += 1,
              Tag::Emphasis => self.italic += 1,
              Tag::Link { .. } => self.link = true,
              Tag::BlockQuote(_) => {
                  self.flush();
                  self.quote += 1;
                  if self.quote as usize + self.list.len() > MAX_DEPTH {
                      self.bail = true;
                  }
              }
              Tag::List(start) => {
                  self.list.push(start);
                  if self.quote as usize + self.list.len() > MAX_DEPTH {
                      self.bail = true;
                  }
              }
              Tag::Item => {
                  self.flush();
                  let depth = self.list.len().saturating_sub(1);
                  let indent = "  ".repeat(depth);
                  let marker = match self.list.last_mut() {
                      Some(Some(n)) => {
                          let m = format!("{n}. ");
                          *n += 1;
                          m
                      }
                      _ => format!("{} ", glyph::BULLET),
                  };
                  self.cur
                      .push(Span::styled(format!("{indent}{marker}"), Style::new().fg(color::DIM)));
              }
              Tag::CodeBlock(kind) => {
                  self.flush();
                  self.fence = Some(match kind {
                      CodeBlockKind::Fenced(info) => lang_from_fence(&info),
                      CodeBlockKind::Indented => Language::PlainText,
                  });
              }
              _ => {}
          }
      }

      fn end(&mut self, end: TagEnd) {
          match end {
              TagEnd::Paragraph => self.flush(),
              TagEnd::Heading(_) => {
                  self.flush();
                  self.heading = false;
              }
              TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
              TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
              TagEnd::Link => self.link = false,
              TagEnd::BlockQuote(_) => {
                  self.flush();
                  self.quote = self.quote.saturating_sub(1);
              }
              TagEnd::List(_) => {
                  self.list.pop();
              }
              TagEnd::Item => self.flush(),
              TagEnd::CodeBlock => {
                  let lang = self.fence.take().unwrap_or(Language::PlainText);
                  let code = std::mem::take(&mut self.code_buf);
                  self.lines.extend(highlight_lines(&code, lang));
              }
              _ => {}
          }
      }

      fn finish(mut self) -> Vec<Line<'static>> {
          self.flush();
          self.lines
      }
  }
  ```
  (API note: the `Tag`/`TagEnd`/`CodeBlockKind` variants and `Event` shape above are validated against `pulldown-cmark` 0.13.4. `Tag::BlockQuote`/`TagEnd::BlockQuote` are tuple variants carrying an `Option<BlockQuoteKind>` in 0.13 — matched with `(_)`.)

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui markdown` (expected: `plain_prose_is_one_txt_line`, `heading_is_accent_bold`, `bold_and_inline_code_are_styled`, `list_items_render_with_bullets`, `fenced_code_is_highlighted_by_language`, `unknown_fence_is_plain_text` all pass) and `cargo test -p zoid-tui markdown_tokens_present` (expected `ok`).

- [ ] **Step 5: Commit.**
  ```
  git add crates/zoid-tui/Cargo.toml Cargo.lock crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/markdown.rs crates/zoid-tui/src/lib.rs
  git commit -m "feat(markdown): pulldown-cmark renderer → styled spans + fenced highlight (§3.5)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

### Task 7: Wire markdown into conversation rendering (spec §3.5)
Message bodies currently render as one plain `color::TXT` span (`chat.rs:34` user, `chat.rs:46` assistant). Route them through `render_markdown`. Because `conversation_lines` feeds both Normal (`conversation_view`) and Detail (`detail_lines`) altitudes, wiring it there covers both; Summary keeps its structural digest (untouched). Plain prose maps to exactly one line, so existing plain-prose snapshots and the `normal_matches_conversation_lines` invariant are preserved.

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (`conversation_lines` `chat.rs:20-73`; add `push_message` helper)
- Test: `crates/zoid-tui/src/chat.rs` (existing tests hold); snapshot `crates/zoid-tui/tests/shell_snapshot.rs`

**Interfaces:**
- Consumes: `crate::markdown::render_markdown` (Task 6), `unicode_width::UnicodeWidthStr` (dep `unicode-width`, `Cargo.toml:14`), `glyph::USER_TURN`, `glyph::CARET`, `color::{TXT, CHAT_ACCENT, DIM}`.
- Produces: `fn push_message<'a>(out: &mut Vec<Line<'a>>, prefix: Vec<Span<'static>>, body: Vec<Line<'static>>)` (private helper).

- [ ] **Step 1: Write the failing test.** Add a markdown-in-conversation unit test to `crates/zoid-tui/src/chat.rs` `mod tests`:
  ```rust
      #[test]
      fn assistant_body_renders_markdown() {
          use crate::tokens::color;
          let msgs = vec![ChatMsg::Assistant {
              text: "run **now**\n\n```rust\nfn x() {}\n```".into(),
              tool_calls: vec![],
              ts: 0,
          }];
          let lines = conversation_lines(&msgs, false, true, 0);
          let spans: Vec<(String, Style)> = lines
              .iter()
              .flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style)))
              .collect();
          // bold inline text survived markdown
          assert!(spans.iter().any(|(t, st)| t == "now" && st.add_modifier.contains(ratatui::style::Modifier::BOLD)));
          // the fenced rust block was syntax-highlighted
          assert!(spans.iter().any(|(_, st)| st.fg == Some(color::SYN_KEYWORD)));
          // the "zoid " role prefix still leads the first line
          assert!(spans.iter().any(|(t, _)| t == "zoid "));
      }
  ```

- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui assistant_body_renders_markdown`
  Expected: `assertion failed` — the body currently renders as a single `TXT` span, so no BOLD/keyword-colored spans exist (`**now**` and the fence render literally).

- [ ] **Step 3: Write minimal implementation.**
  Add the helper near the top of `crates/zoid-tui/src/chat.rs` (after the `conversation_lines` fn, before `ChatView`, ~`chat.rs:73`):
  ```rust
  /// Push a message (user/assistant) into `out`: the `prefix` (stamp + role) leads
  /// the first body line; continuation lines are indented under the text column so
  /// wrapped markdown/lists stay aligned. `body` comes from the markdown renderer.
  fn push_message<'a>(out: &mut Vec<Line<'a>>, prefix: Vec<Span<'static>>, body: Vec<Line<'static>>) {
      use unicode_width::UnicodeWidthStr;
      if body.is_empty() {
          out.push(Line::from(prefix));
          return;
      }
      let indent_w: usize = prefix.iter().map(|s| s.content.width()).sum();
      let indent = " ".repeat(indent_w);
      for (i, line) in body.into_iter().enumerate() {
          let mut spans: Vec<Span<'static>> = if i == 0 {
              prefix.clone()
          } else {
              vec![Span::styled(indent.clone(), Style::new())]
          };
          spans.extend(line.spans);
          out.push(Line::from(spans));
      }
  }
  ```
  Replace the User arm body (`chat.rs:30-36`):
  ```rust
              ChatMsg::User { text, ts } => {
                  let prefix = vec![
                      stamp(*ts),
                      Span::styled(format!("{} ", glyph::USER_TURN), Style::new().fg(color::CHAT_ACCENT)),
                  ];
                  push_message(&mut lines, prefix, crate::markdown::render_markdown(text));
              }
  ```
  Replace the Assistant text-line push (`chat.rs:37-48`) — keep the caret logic and the `for tc in tool_calls` block below it unchanged:
  ```rust
              ChatMsg::Assistant { text, tool_calls, ts } => {
                  let mut shown = text.clone();
                  if streaming && caret_on && i == last && tool_calls.is_empty() {
                      shown.push(glyph::CARET);
                  }
                  if !shown.is_empty() || tool_calls.is_empty() {
                      let prefix = vec![
                          stamp(*ts),
                          Span::styled("zoid ".to_string(), Style::new().fg(color::DIM)),
                      ];
                      push_message(&mut lines, prefix, crate::markdown::render_markdown(&shown));
                  }
                  for tc in tool_calls {
                      // ... unchanged tool-card push (chat.rs:49-56) ...
                  }
              }
  ```
  (Leave the `stamp` closure `chat.rs:26`, the `ToolResult` arm `chat.rs:58-69`, and the function signature `conversation_lines<'a>(...) -> Vec<Line<'a>>` as-is. `render_markdown` returns `Line<'static>`, which each combined `Line` coerces into the `'a` return by value — validated to compile.)

- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui assistant_body_renders_markdown` (expected `ok. 1 passed`), then `cargo test -p zoid-tui --lib chat` (expected: `caret_shows_only_when_streaming_and_caret_on`, `normal_matches_conversation_lines`, `summary_*`, `detail_*`, `reveal_caps_line_count` all still pass — plain prose is unchanged).

- [ ] **Step 5: Add the markdown message snapshot + review.** Append to `crates/zoid-tui/tests/shell_snapshot.rs`:
  ```rust
  /// Markdown message rendering (spec §3.5) — heading, bold, inline code, a list,
  /// and a fenced rust block. Buffer-Debug captures the styled spans + syntax hues.
  fn seeded_markdown() -> Vec<ChatMsg> {
      vec![
          ChatMsg::User { text: "how do I read a file?".into(), ts: 0 },
          ChatMsg::Assistant {
              text: "Use **read_file**. Steps:\n\n- open the path\n- return `String`\n\n```rust\nfn read(p: &str) -> String { String::new() }\n```".into(),
              tool_calls: vec![],
              ts: 0,
          },
      ]
  }

  #[test]
  fn markdown_message_frame() {
      let s = ShellState::new();
      let input = TextArea::default();
      let backend = TestBackend::new(100, 24);
      let mut terminal = Terminal::new(backend).unwrap();
      terminal
          .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_markdown(), &input, false, &normal_view()))
          .unwrap();
      insta::assert_snapshot!(format!("{:#?}", terminal.backend().buffer()));
  }
  ```
  Run `cargo insta test -p zoid-tui --review`. Expected: one **new** pending snapshot `markdown_message_frame`; the pre-existing plain-prose shell snapshots should NOT change (plain prose → one line, byte-identical). Accept the new snapshot after confirming `read_file` is bold, `` `String` `` uses the code hue, the list shows `•`, and `fn`/`String` in the fence carry syntax colors — matching `docs/ux/chat-mode.html` §3.5. If any plain-prose snapshot unexpectedly changed, STOP and investigate (a plain sentence must render identically).

- [ ] **Step 6: Full suite + commit.** `cargo test --workspace` (expected: all pass) then:
  ```
  git add crates/zoid-tui/src/chat.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
  git commit -m "feat(chat): render message bodies as markdown + fenced code (§3.5)

  Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
  ```

---

## Final verification
- [ ] `cargo test --workspace` — all green.
- [ ] `cargo build -p zoid --release` — size budget respected (pulldown-cmark added with `default-features = false`).
- [ ] Manual: run `zoid`, confirm no input underline, `⇧⏎` inserts a newline (or `Alt+⏎` on unsupported terminals), the box grows/shrinks, the title shows `● idle` → `⠿ running` during a turn, the status bar reads `CHAT  zoom normal · ^P palette`, rail headers have no `^5`/`^F`/`^B`, and a markdown reply renders bold/code/lists/fences — all against `docs/ux/chat-mode.html`.
