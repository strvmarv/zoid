# Keyboard-Shortcuts Help Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-app keyboard-shortcuts help overlay, reachable via `?` (conversation focus), a command-palette row, and `:help`, plus a discoverability hint on the empty-session screen.

**Architecture:** New `Overlay::Help` variant in the pure `zoid-tui` renderer/router, following the read-only `/mcp` overlay pattern (`render_mcp_overlay` + `route_mcp_key`). Content comes from one pure `help_lines()` builder; the overlay scrolls via a `help_scroll` field that is **clamped per-frame in the bin against the real overlay-rect height** (the same pattern the conversation scroll uses via `conv_max_scroll`). The `zoid` bin wires the open/scroll actions. The hint is added to `onboarding::empty_state_lines`.

**Tech Stack:** Rust, ratatui, crossterm. Crate `zoid-tui` (pure) + crate `zoid` (bin).

## Global Constraints

- **`zoid-tui` stays pure:** no terminal I/O, no filesystem, no state mutation outside the passed `&mut`. Routers return an `Action`/`Command`; the bin applies effects. Match surrounding module style.
- **No new external/workspace dependencies.**
- **Reuse the existing overlay chrome** (`render_mcp_overlay` + `route_mcp_key` are the templates). Do not invent new widgets.
- **Register a new `Overlay` variant at EVERY seam.** Two are compiler-blind and MUST be covered by tests: `layout.rs` `compute()` (missing rect → invisible) and the `render.rs` overlay if/else dispatch (missing branch → invisible). A test that calls `render_help_overlay` directly does NOT cover the dispatch — a test must drive the real render entry (`render_shell` / full-frame snapshot).
- **`?` must not shadow input:** it opens help ONLY from conversation focus; a `?` typed into the message box stays literal (`Focus::Input`'s `_ => Action::Edit(key)`).
- **Every task ends with a green WORKSPACE build**, not just the touched crate: `cargo test --workspace --no-fail-fast` (redirect to a file and check `$status`; never pipe to `tail`) and `cargo clippy --workspace` with no new warnings. `crates/zoid/src/main.rs`'s `match action` (L3223) and `exec_command` (L4539) are **exhaustive with no `_` arm**, so any new `Action`/`Command` variant must be handled in the same task it is introduced or the workspace will not compile.

---

### Task 1: Pure help-content builder (`help_lines`) + viewport constant

**Files:**
- Create: `crates/zoid-tui/src/help.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (add `pub mod help;`)

**Interfaces:**
- Produces:
  - `pub fn help_lines() -> Vec<ratatui::text::Line<'static>>` — the styled help content.
  - `pub const HELP_RECT_W: u16 = 84;` and `pub const HELP_RECT_H: u16 = 26;` — the overlay's target size, referenced by `layout.rs` so the size lives in one place.

- [ ] **Step 1: Write the failing test** — create `crates/zoid-tui/src/help.rs` with the constants, an empty `help_lines`, and tests:

```rust
//! The static keyboard-shortcuts reference shown by the Help overlay
//! (`Overlay::Help`). Pure content: one styled line per row, grouped into
//! sections. Kept in one place so the overlay and its test stay in sync.
//! NOTE: this is a hand-maintained mirror of the keymap in `route.rs`; when a
//! binding changes there, update the matching row here.

use crate::tokens::color;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Target size of the Help overlay rect (clamped to the conversation area by
/// `layout::centered`). Larger than the 72x18 palette box because the help
/// reference is denser. Defined here so the size has a single source of truth.
pub const HELP_RECT_W: u16 = 84;
pub const HELP_RECT_H: u16 = 26;

/// Build the help overlay's content as styled lines: dim section headers,
/// normal-text shortcut rows. Pure; no terminal or state.
pub fn help_lines() -> Vec<Line<'static>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined() -> String {
        help_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect()
    }

    #[test]
    fn lists_core_shortcuts_and_sections() {
        let s = joined();
        for token in [
            "Global", "Input", "Conversation", "Overlays", "Commands",
            "Ctrl+P", "Ctrl+Q", "Shift+Tab", "Alt+P", "Esc", "?", ":help",
        ] {
            assert!(s.contains(token), "help must mention {token:?}: {s:?}");
        }
    }

    #[test]
    fn has_a_dim_section_header() {
        assert!(
            help_lines()
                .iter()
                .any(|l| l.spans.iter().any(|sp| sp.style.fg == Some(color::DIM))),
            "at least one section header must use the DIM color"
        );
    }

    /// Rows must stay compact so they don't clip on a typical (rail-visible,
    /// ~50-60 col) conversation width. Keep the widest logical row modest.
    #[test]
    fn rows_are_reasonably_narrow() {
        for l in help_lines() {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(w <= 56, "row too wide ({w}): {l:?}");
        }
    }
}
```

Add `pub mod help;` to `crates/zoid-tui/src/lib.rs` (next to the other `pub mod` lines).

- [ ] **Step 2: Run — verify it fails**

Run: `cargo test -p zoid-tui help::tests`
Expected: FAIL (`lists_core_shortcuts_and_sections`, empty output).

- [ ] **Step 3: Implement `help_lines`** — compact rows (key column padded to 22, descriptions trimmed so no row exceeds 56 cols):

```rust
pub fn help_lines() -> Vec<Line<'static>> {
    // (section header, then (keys, description) rows). Keep descriptions short.
    let sections: &[(&str, &[(&str, &str)])] = &[
        ("Global", &[
            ("Ctrl+P", "command palette"),
            ("Ctrl+O", "object / action picker"),
            ("Ctrl+Q", "quit zoid"),
            ("Esc / Ctrl+C", "cancel turn (Esc again forces)"),
            ("Shift+Tab", "switch mode"),
            ("Tab", "change focus"),
            ("Alt+P", "switch provider / model"),
            ("Alt+Left / Right", "semantic zoom"),
            ("?", "this help (conversation)"),
        ]),
        ("Input", &[
            ("Enter", "send message"),
            ("Shift+Enter", "newline (or Alt+Enter)"),
            (":", "command palette (empty box)"),
            ("Shift+Del", "delete line"),
            ("Shift+Home / End", "cursor to start / end"),
        ]),
        ("Conversation", &[
            ("j / Down", "scroll down"),
            ("k / Up", "scroll up"),
            ("= / -", "zoom in / out"),
            ("Shift+Home / End", "scroll to top / bottom"),
            ("Esc", "return to input"),
        ]),
        ("Overlays", &[
            ("Up / Down", "move selection"),
            ("Enter", "choose"),
            ("Esc / q", "close"),
        ]),
        ("Commands", &[
            (":help", "this help"),
            (":compact", "condense the session"),
            (":config", "settings"),
            (":feedback", "send feedback"),
            (":mode install superpowers", "install skills"),
            (":q", "quit"),
        ]),
        ("Mouse", &[
            ("scroll", "scroll conversation"),
            ("Ctrl+scroll", "semantic zoom"),
        ]),
    ];

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, (header, rows)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from("")); // blank between sections
        }
        lines.push(Line::from(Span::styled(
            header.to_string(),
            Style::new().fg(color::DIM),
        )));
        for (keys, desc) in *rows {
            let row = format!("  {keys:<22}{desc}");
            lines.push(Line::from(Span::styled(row, Style::new().fg(color::TXT))));
        }
    }
    lines
}
```

> If `rows_are_reasonably_narrow` fails, shorten the offending description — do NOT widen the 56 cap.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p zoid-tui help::tests`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/help.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): pure help-content builder for shortcuts overlay"
```

---

### Task 2: `Overlay::Help` — variant, own rect, render, scroll, close (fully functional, reachable by tests)

This task delivers the whole overlay except the open entry points (Task 3). It is reachable only by tests that set `overlay = Overlay::Help` directly. `Action::ScrollHelp` and its bin handler are BOTH added here so the workspace compiles at the task boundary (C2).

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (enum `Overlay` L47; `ShellState` struct + `new()` L389; `close_overlay()` L500)
- Modify: `crates/zoid-tui/src/layout.rs` (`compute()` overlay rect L273; guard test L467)
- Modify: `crates/zoid-tui/src/render.rs` (overlay dispatch L194; add `render_help_overlay` near L1058)
- Modify: `crates/zoid-tui/src/route.rs` (`Action` enum L104; overlay match L210; add `route_help_key` near L366; `route_paste` selection-only arm L168)
- Modify: `crates/zoid/src/main.rs` (`Action::ScrollHelp` handler in `match action` near L3458; per-frame help-scroll clamp at the `render_shell` call site)

**Interfaces:**
- Consumes: `help::{help_lines, HELP_RECT_W, HELP_RECT_H}`.
- Produces: `Overlay::Help`; `ShellState.help_scroll: usize`; `Action::ScrollHelp(i32)`; `render_help_overlay(frame, state, area)`; `route_help_key(state, key) -> Action`.

- [ ] **Step 1: Enum variant + state field.**

`state.rs` — add `Help` to `Overlay` (after `Feedback`):
```rust
    Feedback,
    Help,
```
Add `pub help_scroll: usize,` to the `ShellState` struct; initialize `help_scroll: 0,` in `new()` (next to `session_selected: 0,`); reset it in `close_overlay()`:
```rust
    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
        self.palette = PaletteState::default();
        self.objects = ObjectState::default();
        self.sessions.clear();
        self.sessions_live.clear();
        self.session_selected = 0;
        self.help_scroll = 0;
    }
```

- [ ] **Step 2: Own overlay rect (per spec) + guard test.**

`layout.rs` `compute()` — Help gets its OWN centered rect (larger than the shared 72x18 palette box), reusing the `palette` field so `render.rs` reads it the same way:
```rust
    let palette = if matches!(
        state.overlay,
        Overlay::Palette | Overlay::Objects | Overlay::Verbs | Overlay::Sessions | Overlay::Mcp
        | Overlay::Feedback
    ) {
        Some(centered(conversation, 72, 18))
    } else if state.overlay == Overlay::Help {
        Some(centered(conversation, crate::help::HELP_RECT_W, crate::help::HELP_RECT_H))
    } else {
        None
    };
```
(`centered` clamps both width and height to `conversation`, so this is panic-safe on any terminal size — verified at `layout.rs:298-299`.)

Add `Overlay::Help` to the guard-test array in `overlay_rect_present_for_object_and_verb_pickers`:
```rust
        for ov in [
            Overlay::Objects,
            Overlay::Verbs,
            Overlay::Sessions,
            Overlay::Mcp,
            Overlay::Help,
        ] {
```

- [ ] **Step 3: Render dispatch + `render_help_overlay`.**

`render.rs` — add a branch after the `Overlay::Feedback` branch (before `conv_max_scroll`):
```rust
    } else if state.overlay == Overlay::Help {
        if let Some(p) = layout.palette {
            render_help_overlay(frame, state, p);
        }
    }
```

Add the render fn next to `render_mcp_overlay`. It clamps display scroll against the actual inner height (defensive; the bin also clamps `help_scroll` per-frame in Step 6):
```rust
/// The read-only keyboard-shortcuts overlay (`Overlay::Help`). Scrolls via
/// `state.help_scroll`; Esc/`q` close it (see `route::route_help_key`). The bin
/// clamps `help_scroll` per-frame against this rect's height; the extra clamp
/// here keeps a stale/oversized value from scrolling into emptiness.
fn render_help_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(
            " keyboard shortcuts — esc to close ",
            Style::new().fg(color::TXT),
        ));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);
    let lines = crate::help::help_lines();
    let vh = inner.height as usize;
    let off = state.help_scroll.min(lines.len().saturating_sub(vh));
    frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
}
```

- [ ] **Step 4: `Action::ScrollHelp` + close/scroll routing.**

`route.rs` — add to the `Action` enum (before `Noop`):
```rust
    /// Scroll the keyboard-shortcuts overlay by N rows (bin clamps the range).
    ScrollHelp(i32),
```
Add to the overlay match in `route_key` (after the `Overlay::Mcp` arm):
```rust
        Overlay::Help => return route_help_key(state, key),
```
Add `route_help_key` next to `route_mcp_key`:
```rust
/// Route keys while the read-only keyboard-shortcuts overlay is up. Esc or `q`
/// close it; Up/Down/j/k and PageUp/PageDown scroll (the bin clamps the range).
fn route_help_key(_state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::CloseOverlay,
        KeyCode::Down | KeyCode::Char('j') => Action::ScrollHelp(1),
        KeyCode::Up | KeyCode::Char('k') => Action::ScrollHelp(-1),
        KeyCode::PageDown => Action::ScrollHelp(10),
        KeyCode::PageUp => Action::ScrollHelp(-10),
        _ => Action::Noop,
    }
}
```
Add `Overlay::Help` to the `route_paste` selection-only arm:
```rust
        Overlay::Objects
        | Overlay::Verbs
        | Overlay::Sessions
        | Overlay::Mcp
        | Overlay::Help
        | Overlay::ProviderSwitch => return PasteTarget::None,
```

- [ ] **Step 5: Bin — handle `Action::ScrollHelp` (saturating) + clamp per-frame.**

`main.rs` — in `match action`, near `Action::OpenObjects` (this arm is REQUIRED for the exhaustive match to compile):
```rust
        Action::ScrollHelp(d) => {
            let cur = app.shell.help_scroll as i64;
            app.shell.help_scroll = (cur + d as i64).max(0) as usize;
            // Upper bound is clamped per-frame against the real rect height
            // (see the render-loop clamp), mirroring conv_max_scroll.
        }
```

At the `render_shell(...)` call site (find it: `grep -n 'render_shell(' crates/zoid/src/main.rs` — it is invoked once per frame inside the `terminal.draw(...)` closure and returns `conv_max_scroll`), add — immediately BEFORE the `render_shell` call — a clamp of `help_scroll` against the actual help-rect height so state and display share one source of truth:
```rust
        // Clamp the help overlay scroll to the real rect height (same idea as
        // conv_max_scroll): the ScrollHelp handler only increments; this pins
        // the ceiling for the current terminal size.
        if app.shell.overlay == zoid_tui::Overlay::Help {
            let vh = layout
                .palette
                .map(|r| r.height.saturating_sub(2) as usize) // borders/margin
                .unwrap_or(0);
            let max = zoid_tui::help::help_lines().len().saturating_sub(vh);
            app.shell.help_scroll = app.shell.help_scroll.min(max);
        }
```
(`layout` is the `ShellLayout` computed for the frame; if the local binding has a different name at that site, use it. If the clamp cannot see `layout`, compute it via `zoid_tui::layout::compute(area, &app.shell)`.)

- [ ] **Step 6: Tests.**

`render.rs` tests — (a) a direct render test AND (b) a dispatch-exercising test. For (b), drive the real render entry so a missing if/else branch is caught (I2). If `render_shell` is awkward to call directly in a unit test, add a full-frame snapshot in `crates/zoid-tui/tests/shell_snapshot.rs` mirroring an existing overlay snapshot but with `overlay = Overlay::Help`, and assert (or snapshot) that `keyboard shortcuts` appears. Direct render test:
```rust
#[test]
fn help_overlay_lists_shortcuts() {
    use crate::state::{Overlay, ShellState};
    let mut s = ShellState::new();
    s.overlay = Overlay::Help;
    let backend = TestBackend::new(84, 26);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_help_overlay(f, &s, f.area())).unwrap();
    let content: String = terminal.backend().buffer().content()
        .iter().map(|c| c.symbol().to_string()).collect();
    assert!(content.contains("Ctrl+P"), "got: {content}");
    assert!(content.contains("keyboard shortcuts"));
}
```

`route.rs` tests — close + scroll routing:
```rust
#[test]
fn help_overlay_close_and_scroll_route() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = ShellState::new();
    s.overlay = Overlay::Help;
    let k = |c| KeyEvent::new(c, KeyModifiers::NONE);
    assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
    assert_eq!(route_key(&s, k(KeyCode::Char('q'))), Action::CloseOverlay);
    assert_eq!(route_key(&s, k(KeyCode::Down)), Action::ScrollHelp(1));
    assert_eq!(route_key(&s, k(KeyCode::Char('k'))), Action::ScrollHelp(-1));
}
```

- [ ] **Step 7: Verify (workspace).**

Run: `cargo test --workspace --no-fail-fast > /tmp/gate.log 2>&1; echo $?` (must be 0) and `cargo clippy --workspace`.
Expected: PASS incl. the new render/route tests, the dispatch/snapshot test, and the extended layout guard; no new clippy warnings. The `Action::ScrollHelp` bin arm makes the exhaustive `match action` compile.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src crates/zoid/src/main.rs
git commit -m "feat: Help overlay — variant, own rect, render, and clamped scroll"
```

---

### Task 3: Open the overlay — `:help`, palette rows, and `?`

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (enum L48; `parse_command` flat commands L88)
- Modify: `crates/zoid-tui/src/render.rs` (exhaustive preview `match cmd` L867)
- Modify: `crates/zoid-tui/src/palette.rs` (`all_items` L400 + its test `all_items_is_flat_curated` L500; `stage1_items` L118 + its test `direct_items_stage1_bare_colon` L666)
- Modify: `crates/zoid-tui/src/route.rs` (`Action` enum L62; `Focus::Conversation` arm L284)
- Modify: `crates/zoid/src/main.rs` (action dispatch L3458; `exec_command` L4778)

**Interfaces:**
- Produces: `Command::OpenHelp`; `Action::OpenHelp`; palette entries; bin handlers setting `overlay = Overlay::Help`.

- [ ] **Step 1: Write the failing tests.**

`command.rs` tests:
```rust
#[test]
fn parses_help_command() {
    assert_eq!(parse_command(":help"), Command::OpenHelp);
    assert_eq!(parse_command("help"), Command::OpenHelp);
}
```
`route.rs` tests (the `?`-doesn't-shadow-input guard):
```rust
#[test]
fn question_mark_opens_help_only_from_conversation() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let mut s = ShellState::new();
    s.focus = Focus::Conversation;
    assert_eq!(route_key(&s, q), Action::OpenHelp);
    s.focus = Focus::Input;
    assert!(matches!(route_key(&s, q), Action::Edit(_)));
}
```

- [ ] **Step 2: Run — verify they fail** (unknown variants → compile error counts as fail).

Run: `cargo test -p zoid-tui command::tests::parses_help_command`
Expected: FAIL.

- [ ] **Step 3: `Command::OpenHelp` + parse + exhaustive preview arm.**

`command.rs` enum (after `Feedback`, before `Unknown`):
```rust
    /// Open the keyboard-shortcuts help overlay (`:help`).
    OpenHelp,
```
`parse_command` flat commands (next to `"config" => Command::OpenConfig,`):
```rust
        "help" => Command::OpenHelp,
```
`render.rs` preview `match cmd` (exhaustive — this arm is REQUIRED to compile):
```rust
                    Command::OpenHelp => "→ Keyboard shortcuts".to_string(),
```

- [ ] **Step 4: Palette rows + update the two exact-match tests (C1).**

`palette.rs` `all_items` — insert AFTER the `"MCP servers…"` push, BEFORE `"Submit feedback…"`:
```rust
    items.push(PaletteItem {
        label: "Keyboard shortcuts…".to_string(),
        command: Command::OpenHelp,
    });
```
Update `all_items_is_flat_curated` (L500): insert `"Keyboard shortcuts…"` into the expected `vec![...]` between `"MCP servers…"` and `"Submit feedback…"`.

`palette.rs` `stage1_items` — insert AFTER the `config` item, BEFORE the `q` item:
```rust
        PaletteItem {
            label: "help".into(),
            command: Command::OpenHelp,
        },
```
Update `direct_items_stage1_bare_colon` (L666): insert `"help"` into the expected `vec![...]` between `"config"` and `"q"`.

- [ ] **Step 5: `Action::OpenHelp` + route `?` from conversation.**

`route.rs` `Action` enum (near `OpenObjects`):
```rust
    /// Open the keyboard-shortcuts help overlay (`?` from conversation focus).
    OpenHelp,
```
`Focus::Conversation` arm (before `_ => Action::Noop,`):
```rust
            KeyCode::Char('?') => Action::OpenHelp,
```

- [ ] **Step 6: Bin handlers.**

`main.rs` `match action` (near `Action::OpenObjects`):
```rust
        Action::OpenHelp => {
            app.shell.overlay = zoid_tui::Overlay::Help;
            app.shell.help_scroll = 0;
        }
```
`main.rs` `exec_command` (near `Command::OpenMcp`):
```rust
        Command::OpenHelp => {
            app.shell.overlay = zoid_tui::Overlay::Help;
            app.shell.help_scroll = 0;
            Ok(false)
        }
```

- [ ] **Step 7: Verify (workspace).**

Run: `cargo test --workspace --no-fail-fast > /tmp/gate.log 2>&1; echo $?` (0) and `cargo clippy --workspace`.
Expected: PASS incl. `parses_help_command`, `question_mark_opens_help_only_from_conversation`, and the UPDATED `all_items_is_flat_curated` / `direct_items_stage1_bare_colon`.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src crates/zoid/src/main.rs
git commit -m "feat: open help overlay via :help, palette, and ? key"
```

---

### Task 4: Empty-state hint

**Files:**
- Modify: `crates/zoid-tui/src/onboarding.rs` (constant L26; `new_user_lines` L86; `returning_user_lines` L104; tests L112)

**Interfaces:**
- Uses the existing `empty_state_lines` signature; adds a hint line to both branches.

- [ ] **Step 1: Write the failing test.**
```rust
#[test]
fn help_hint_shown_in_both_empty_states() {
    let joined = |ls: &[ratatui::text::Line]| {
        ls.iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>()
    };
    assert!(joined(&empty_state_lines(true, true, 80)).contains(":help"));
    assert!(joined(&empty_state_lines(true, false, 80)).contains(":help"));
    assert!(joined(&empty_state_lines(false, false, 80)).contains(":help"));
}
```

- [ ] **Step 2: Run — verify it fails**

Run: `cargo test -p zoid-tui onboarding::tests::help_hint_shown_in_both_empty_states`
Expected: FAIL.

- [ ] **Step 3: Add the hint constant + push in both branches.**

`onboarding.rs` — constant near `SUPERPOWERS_OFFER`:
```rust
/// Discoverability hint pointing at the keyboard-shortcuts overlay. Shown on
/// every empty session (new and returning). Mentions `:help` as well as `?`
/// because the input box is focused by default, where `?` is a literal char.
const HELP_HINT: &str = "Press ? (or run :help) for keyboard shortcuts";
```
`new_user_lines` — append after the superpowers block, before `lines`:
```rust
    lines.push(Line::from(""));
    for w in wrap_title(indent, HELP_HINT, width) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
    }

    lines
}
```
`returning_user_lines` — collect the welcome-back line, then append the hint:
```rust
fn returning_user_lines(width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> =
        crate::render::wrap_plain(&format!("  {RETURNING_HINT}"), width)
            .into_iter()
            .map(|w| Line::from(Span::styled(w, Style::new().fg(color::DIM))))
            .collect();
    lines.push(Line::from(""));
    for w in wrap_title("  ", HELP_HINT, width) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
    }
    lines
}
```

> The existing `returning_user_renders_welcome_back_only` and `superpowers_offer_line_shown_only_when_offered` tests stay green: `HELP_HINT` shares no words with `NEW_USER_PROMPTS`, `NEW_USER_TITLE`, or the literal `"Superpowers"`. Verify.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p zoid-tui onboarding`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/onboarding.rs
git commit -m "feat(tui): empty-state hint pointing at the help overlay"
```

---

## Final Verification (whole feature)

- [ ] `cargo test --workspace --no-fail-fast > /tmp/gate.log 2>&1; echo $?` — must print `0`.
- [ ] `cargo clippy --workspace` — no new warnings.
- [ ] `cargo insta test --accept -p zoid-tui` if any full-frame snapshot changed (the empty-state hint changes onboarding output, and a Help snapshot may have been added in Task 2). Audit the diff: only the added hint line / new Help snapshot.
- [ ] Manual TTY smoke (no automated coverage): launch zoid, press `?` from the conversation pane → overlay opens; j/k and PageUp/Down scroll and clamp at both ends (no dead presses); Esc/`q` close; `:help` and the palette row open it; a `?` typed in the message box stays literal; the empty-session screen shows the hint.

## Self-Review Notes

- **Spec coverage:** overlay content (T1), own rect per spec §Sizing (T2 Step 2), render + dispatch guarded by a real-entry test (T2 Step 6, I2), scroll with single-source-of-truth clamp (T2 Steps 4–5, I1), three entry points (T3), the two exact-match palette tests updated (T3 Step 4, C1), hint in both branches (T4). ✓
- **Compile-at-every-boundary (C2):** `Action::ScrollHelp` + its bin handler both land in Task 2; `Command::OpenHelp` + its exhaustive `match cmd` / `exec_command` arms both land in Task 3. Each task ends on a green `--workspace` build, not just `-p zoid-tui`. ✓
- **Type consistency:** `Overlay::Help`, `Command::OpenHelp`, `Action::OpenHelp`, `Action::ScrollHelp(i32)`, `ShellState.help_scroll: usize`, `help::{help_lines, HELP_RECT_W, HELP_RECT_H}`, `render_help_overlay`, `route_help_key`. ✓
- **`?` routing:** matched only in `Focus::Conversation` (on `key.code`); `Focus::Input`'s `_ => Action::Edit(key)` keeps it literal; no global combo intercepts a modifier-less `?`. Guarded by `question_mark_opens_help_only_from_conversation`. ✓
