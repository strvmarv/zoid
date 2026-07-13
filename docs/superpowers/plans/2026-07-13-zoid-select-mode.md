# zoid "Select mode" (mouse-capture toggle) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user toggle terminal mouse capture off/on at runtime ("select mode") so the whole zoid window supports native drag-select + terminal copy of arbitrary text.

**Architecture:** A new `ShellState.select_mode` bool is the single source of truth. Two triggers (`Alt+M` key, `:select`/`:mouse` palette command) flip it. The bin's run loop reconciles the real terminal capture state against `select_mode` once per frame via `execute!(Enable/DisableMouseCapture)` — keeping the terminal side-effect out of the pure `route`/`handle_action` layers. An always-visible `SELECT` pill in the status line shows the state.

**Tech Stack:** Rust, ratatui + crossterm (`ratatui::crossterm`), workspace crates `zoid-tui` (pure UI/routing) and `zoid` (the bin).

## Global Constraints

- `zoid-tui` is **pure**: routing/state/render only — no terminal I/O, no `execute!`. All terminal side-effects live in the `zoid` bin. (route.rs header, spec §13/§14.1.)
- Crate split for crossterm: the **bin** (`zoid`) imports crossterm **directly** (`use crossterm::{…}` at `main.rs:2`) — `execute!`, `EnableMouseCapture`, `DisableMouseCapture` are already in scope for the reconcile. The **`zoid-tui`** crate uses `ratatui::crossterm::…` (e.g. `route.rs:12`). Do not cross the two.
- The `SELECT` pill reuses palette colors from `crate::tokens::color` (spec §16); no new color constants.
- Do not touch the existing OSC 52 code-block copy (`copy_to_clipboard_osc52`, `handle_conversation_click`).
- Follow existing house patterns: `Command` variants have a matching `parse_command` arm + `exec_command` arm; palette rows are `PaletteItem { label, command }`; route tests use the `key(code, mods)` helper; bin tests use `test_app().await` + `exec_command(&mut app, …)`.

---

## File Structure

- `crates/zoid-tui/src/state.rs` — add `ShellState.select_mode: bool` (source of truth).
- `crates/zoid-tui/src/command.rs` — `Command::ToggleSelectMode` + `:select`/`:mouse` parse.
- `crates/zoid-tui/src/route.rs` — `Action::ToggleMouseCapture` + `Alt+M` global combo.
- `crates/zoid-tui/src/render.rs` — `SELECT` pill span in `render_status`.
- `crates/zoid-tui/src/palette.rs` — discoverable toggle row in `all_items` (signature gains `select_mode`).
- `crates/zoid-tui/src/help.rs` — help rows.
- `crates/zoid/src/main.rs` — `toggle_select_mode` helper, `exec_command`/`handle_action` arms, per-frame capture reconcile, `all_items` caller update.

---

## Task 1: Core toggle mechanism (state + command + bin reconcile)

Delivers a working `:select` command that really flips native selection. This is the task that lands the terminal side-effect, so it is meaningful on its own.

**Files:**
- Modify: `crates/zoid-tui/src/state.rs:461` (Default impl; field decl near `:259`)
- Modify: `crates/zoid-tui/src/command.rs` (enum `:55`, parser `:96`, tests `:115+`)
- Modify: `crates/zoid-tui/src/render.rs:902` (the palette-preview `match cmd` — exhaustive, no `_`; needs a new arm or the crate won't build)
- Modify: `crates/zoid/src/main.rs` (loop `:2340`, before-draw reconcile just before `let frame_start` at `:2606`, `exec_command` `:4965`, helper near `:812`)
- Test: `crates/zoid-tui/src/command.rs` (inline `#[cfg(test)]`), `crates/zoid/src/main.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `ShellState.select_mode: bool` (default `false`); `Command::ToggleSelectMode`; `parse_command(":select")` / `parse_command(":mouse")` → `Command::ToggleSelectMode`; free fn `fn toggle_select_mode(app: &mut App)` in `main.rs`.

- [ ] **Step 1: Write the failing command-parse test**

In `crates/zoid-tui/src/command.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn parses_select_mode_command() {
        assert_eq!(parse_command(":select"), Command::ToggleSelectMode);
        assert_eq!(parse_command("select"), Command::ToggleSelectMode);
        assert_eq!(parse_command(":mouse"), Command::ToggleSelectMode);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui parses_select_mode_command`
Expected: FAIL — `no variant named ToggleSelectMode found for enum Command`.

- [ ] **Step 3: Add the `Command` variant**

In `crates/zoid-tui/src/command.rs`, add to the `enum Command` (before `Unknown(String)` at `:55`):

```rust
    /// Toggle "select mode": flip terminal mouse capture so the whole window
    /// supports native drag-select + terminal copy (`:select` / `:mouse`).
    ToggleSelectMode,
```

- [ ] **Step 3b: Add the palette-preview arm (REQUIRED — exhaustive match)**

`crates/zoid-tui/src/render.rs` has a second `match cmd` at `:902` (the `:`-palette preview line) that enumerates **every** `Command` variant with no `_` fallback. Without a new arm the whole `zoid-tui` crate fails to build (`non-exhaustive patterns: Command::ToggleSelectMode not covered`) — so `cargo test -p zoid-tui` in Step 5 won't even compile. Add, alongside the other preview arms (after `Command::WorktreeExit => …`):

```rust
                    Command::ToggleSelectMode => "→ Toggle select mode".to_string(),
```

- [ ] **Step 4: Add the parser arm**

In `parse_command`, in the `// --- flat commands ---` group (after the `"help"` arm at `:101`), add:

```rust
        "select" | "mouse" => Command::ToggleSelectMode,
```

- [ ] **Step 5: Run parse test to verify it passes**

Run: `cargo test -p zoid-tui parses_select_mode_command`
Expected: PASS.

- [ ] **Step 6: Add the `select_mode` state field + default**

In `crates/zoid-tui/src/state.rs`, add to `struct ShellState` next to `companion_on` (`:259`):

```rust
    /// When true, terminal mouse capture is released so the user can natively
    /// select + copy arbitrary text. Toggled by `Alt+M` / `:select`. The bin
    /// reconciles the real capture state against this each frame.
    pub select_mode: bool,
```

And in the `Default`/constructor near `companion_on: false,` (`:461`):

```rust
            select_mode: false,
```

- [ ] **Step 7: Add the `toggle_select_mode` helper in the bin**

In `crates/zoid/src/main.rs`, next to `copy_to_clipboard_osc52` (near `:812`), add:

```rust
/// Flip "select mode" and surface a transient hint. The actual terminal
/// mouse-capture change is applied by the run loop's per-frame reconcile (which
/// holds the `terminal` backend); this only mutates state, so it is safe to call
/// from `handle_action`/`exec_command` where the backend is out of scope.
fn toggle_select_mode(app: &mut App) {
    app.shell.select_mode = !app.shell.select_mode;
    app.shell.status_hint = Some(if app.shell.select_mode {
        "select mode on — drag to select, copy with your terminal".into()
    } else {
        "select mode off".into()
    });
}
```

- [ ] **Step 8: Add the `exec_command` arm**

In `crates/zoid/src/main.rs`, in `exec_command` (near the `Command::CompanionDisable` arm at `:5240`), add:

```rust
        Command::ToggleSelectMode => {
            toggle_select_mode(app);
            Ok(false)
        }
```

- [ ] **Step 9: Add the per-frame capture reconcile in the run loop**

In `crates/zoid/src/main.rs`, declare a tracker immediately before the main `loop {` at `:2340`:

```rust
    // Actual terminal mouse-capture state (true at startup — EnableMouseCapture
    // ran during terminal setup). Reconciled against `shell.select_mode` below.
    let mut mouse_captured = true;
```

Then inside the loop, immediately before `let frame_start = std::time::Instant::now();` (at `:2606`; `terminal` is a live `&mut` binding here — used by `terminal.draw` at `:2607` and `terminal.size()` at `:2690`, no conflicting borrow), add:

```rust
        // Reconcile terminal mouse capture with select mode: while select_mode is
        // on we release the mouse to the terminal for native selection; otherwise
        // we hold it (click-to-copy code, choice clicks, scroll routing).
        let want_capture = !app.shell.select_mode;
        if want_capture != mouse_captured {
            let _ = if want_capture {
                execute!(terminal.backend_mut(), EnableMouseCapture)
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)
            };
            mouse_captured = want_capture;
        }
```

(`EnableMouseCapture`/`DisableMouseCapture` and `execute!` are already imported — `main.rs:4`.)

- [ ] **Step 10: Write the failing exec test**

In `crates/zoid/src/main.rs` `#[cfg(test)]` module (alongside the `exec_command`/`CompactNow` tests near `:7491`), add:

```rust
    #[tokio::test]
    async fn select_command_toggles_select_mode() {
        let mut app = test_app().await;
        assert!(!app.shell.select_mode);
        let quit = exec_command(&mut app, zoid_tui::command::Command::ToggleSelectMode)
            .await
            .unwrap();
        assert!(!quit);
        assert!(app.shell.select_mode, ":select must turn select mode on");
        let _ = exec_command(&mut app, zoid_tui::command::Command::ToggleSelectMode)
            .await
            .unwrap();
        assert!(!app.shell.select_mode, ":select again must turn it off");
    }
```

- [ ] **Step 11: Run the workspace build + tests**

Run: `cargo test -p zoid-tui && cargo test -p zoid select_command_toggles_select_mode`
Expected: PASS (and the workspace compiles — `exec_command`'s match is now exhaustive over the new variant).

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/command.rs crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(tui): select mode — :select toggles runtime mouse capture"
```

---

## Task 2: Alt+M keybinding

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (Action enum `:17`, global combos `:255`, tests `:561`)
- Modify: `crates/zoid/src/main.rs` (`handle_action` match near `:3637`)

**Interfaces:**
- Consumes: `toggle_select_mode(app)` (Task 1).
- Produces: `Action::ToggleMouseCapture`; `route_key(state, Alt+M)` → `Action::ToggleMouseCapture`.

- [ ] **Step 1: Write the failing route test**

In `crates/zoid-tui/src/route.rs` `mod tests`, add:

```rust
    #[test]
    fn alt_m_toggles_mouse_capture() {
        let s = ShellState::new();
        assert_eq!(
            route_key(&s, key(KeyCode::Char('m'), KeyModifiers::ALT)),
            Action::ToggleMouseCapture
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui alt_m_toggles_mouse_capture`
Expected: FAIL — `no variant named ToggleMouseCapture found for enum Action`.

- [ ] **Step 3: Add the `Action` variant**

In `crates/zoid-tui/src/route.rs`, add to `enum Action` (after `ZoomOut,` at `:45`):

```rust
    /// Toggle terminal mouse capture ("select mode"). Applied by the bin's run
    /// loop (needs the terminal backend); flips `ShellState.select_mode`.
    ToggleMouseCapture,
```

- [ ] **Step 4: Add the `Alt+M` global combo**

In `route_key`, immediately after the `alt(&key, 'p')` block at `:255-257`, add:

```rust
    if alt(&key, 'm') {
        return Action::ToggleMouseCapture;
    }
```

- [ ] **Step 5: Run route test to verify it passes**

Run: `cargo test -p zoid-tui alt_m_toggles_mouse_capture`
Expected: PASS.

- [ ] **Step 6: Add the `handle_action` arm in the bin**

In `crates/zoid/src/main.rs` `handle_action` (near `Action::ScrollbarRelease` at `:3637`), add:

```rust
        Action::ToggleMouseCapture => {
            toggle_select_mode(app);
        }
```

(If that match's arms return `Ok(...)` explicitly rather than falling through, mirror the neighboring arm's form — e.g. end with `Ok(false)` if required. Match the surrounding arms exactly.)

- [ ] **Step 7: Build the workspace + run tests**

Run: `cargo test -p zoid-tui && cargo build`
Expected: PASS / clean build (`handle_action` is exhaustive again).

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(tui): Alt+M toggles select mode"
```

---

## Task 3: SELECT pill indicator

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render_status`, mode-chip push at `:369-372`)
- Test: `crates/zoid-tui/src/render.rs` (inline `#[cfg(test)]`) — or a snapshot per repo convention.

**Interfaces:**
- Consumes: `ShellState.select_mode` (Task 1), `crate::tokens::color::{BRANCH, DIM, CHAT_BG}`.

- [ ] **Step 1: Write the failing render test**

In `crates/zoid-tui/src/render.rs`, add an inline test (same module as `render_status`, which is private — that's fine). `ChatView` has **no `Default`**; build it with the struct literal the file's existing tests use (copy the shape at `render.rs:1658`). Assert on cell style via `c.style().fg == Some(color::BRANCH)` — the convention this file already uses (`render.rs:1690`), not a bare `c.fg`:

```rust
    #[test]
    fn select_pill_color_tracks_mode() {
        use ratatui::{backend::TestBackend, Terminal};
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let mut on = ShellState::new();
        on.select_mode = true;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| render_status(f, &on, &view, f.area())).unwrap();
        let has_branch = term
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.style().fg == Some(color::BRANCH));
        assert!(has_branch, "SELECT pill must use BRANCH fg when select_mode on");
    }
```

(If the `ChatView` literal drifts from `render.rs:1658`, copy that current field set verbatim — do not invent fields. `Zoom` is `crate::state::Zoom`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui select_pill_color_tracks_mode`
Expected: FAIL — no `BRANCH`-colored cell (pill not rendered yet).

- [ ] **Step 3: Add the SELECT pill span**

In `crates/zoid-tui/src/render.rs`, in `render_status`, immediately after the mode-chip span is pushed onto `left` (`:369-372`, the `let mut left = vec![Span::styled(chip, …)];`), add:

```rust
    // Always-visible SELECT pill, right of the mode pill. Purple when select
    // mode is on (native selection/copy live), dimmed when off. Same padded /
    // CHAT_BG style as the mode chip so they read as a matched pair.
    let select_fg = if state.select_mode { color::BRANCH } else { color::DIM };
    left.push(Span::styled(
        " SELECT ",
        Style::new().fg(select_fg).bg(color::CHAT_BG),
    ));
```

- [ ] **Step 4: Run render test to verify it passes**

Run: `cargo test -p zoid-tui select_pill_color_tracks_mode`
Expected: PASS.

- [ ] **Step 5: Refresh snapshots if the crate uses `insta`/golden files**

Run: `cargo test -p zoid-tui` (status/shell snapshot tests may now differ).
If snapshot diffs appear and are the expected new ` SELECT ` pill, accept them (`cargo insta accept` or update the golden file per repo convention). Inspect the diff to confirm it is only the added pill.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/snapshots
git commit -m "feat(tui): SELECT status pill (purple on / dim off)"
```

---

## Task 4: Palette row + help discoverability

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (`all_items` `:342`, companion-row block `:359-364`, push near `:420-423`, tests)
- Modify callers of `all_items`: `crates/zoid-tui/src/route.rs`, `crates/zoid-tui/src/render.rs`, `crates/zoid/src/main.rs`
- Modify: `crates/zoid-tui/src/help.rs` (`Global` + `Commands` sections `:22`, `:52`; test `:98`)

**Interfaces:**
- Consumes: `ShellState.select_mode` (Task 1), `Command::ToggleSelectMode` (Task 1).
- Produces: `all_items(active_mode, mode_names, companion_on, select_mode)` (new 4th param).

- [ ] **Step 1: Write the failing palette test**

In `crates/zoid-tui/src/palette.rs` `mod tests`, add (mirroring the existing companion on/off test):

```rust
    #[test]
    fn all_items_offers_opposite_select_label() {
        let off = all_items("Chat", &names(), false, false);
        assert!(off.iter().any(|i| i.label == "Enable select mode"
            && i.command == Command::ToggleSelectMode));
        let on = all_items("Chat", &names(), false, true);
        assert!(on.iter().any(|i| i.label == "Disable select mode"
            && i.command == Command::ToggleSelectMode));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui all_items_offers_opposite_select_label`
Expected: FAIL — `all_items` takes 3 args, not 4 / no such row.

- [ ] **Step 3: Extend `all_items` signature + add the row**

In `crates/zoid-tui/src/palette.rs`, change the signature (`:342`):

```rust
pub fn all_items(
    active_mode: &str,
    mode_names: &[String],
    companion_on: bool,
    select_mode: bool,
) -> Vec<PaletteItem> {
```

After the companion push (`:420-423`), add:

```rust
    let select_label = if select_mode {
        "Disable select mode"
    } else {
        "Enable select mode"
    };
    items.push(PaletteItem {
        label: select_label.to_string(),
        command: Command::ToggleSelectMode,
    });
```

- [ ] **Step 4: Update the three `all_items` callers**

Append `, state.select_mode` to each call. In `crates/zoid-tui/src/route.rs` and `crates/zoid-tui/src/render.rs`:

```rust
    let items = all_items(&state.active_mode, &state.mode_names, state.companion_on, state.select_mode);
```

In `crates/zoid/src/main.rs` the `all_items(` call is at `:3555` and uses `app.shell.*`, so append `, app.shell.select_mode`.

Update the existing companion/palette tests in `palette.rs` that call `all_items("Chat", &names(), …)` to pass a 4th `false` argument.

- [ ] **Step 5: Run palette tests to verify they pass**

Run: `cargo test -p zoid-tui palette`
Expected: PASS (new + updated tests).

- [ ] **Step 6: Write the failing help test**

The existing test `lists_core_shortcuts_and_sections` (`help.rs:98`) builds a joined `String` named **`s`** via a `joined()` helper (`help.rs:90`) and loops `for token in [ … ] { assert!(s.contains(token), …) }`. Append `"Alt+M"` and `":select"` to that existing token array — do not introduce a new `text` variable:

```rust
        for token in [
            "Global", "Input", "Conversation", "Overlays", "Commands",
            "Ctrl+P", "Ctrl+Q", "Shift+Tab", "Alt+P", "Esc", "?", ":help",
            "Alt+M", ":select",
        ] {
            assert!(s.contains(token), "help must mention {token:?}: {s:?}");
        }
```

- [ ] **Step 7: Run help test to verify it fails**

Run: `cargo test -p zoid-tui -- help`
Expected: FAIL — `Alt+M` / `:select` not present.

- [ ] **Step 8: Add the help rows**

In `crates/zoid-tui/src/help.rs`, add to the `("Global", &[…])` rows (after `Alt+Left / Right`, `:30`):

```rust
            ("Alt+M", "select mode (native copy)"),
```

And to the `("Commands", &[…])` rows (`:52`):

```rust
            (":select", "toggle select mode"),
```

- [ ] **Step 9: Run help test to verify it passes**

Run: `cargo test -p zoid-tui -- help`
Expected: PASS.

- [ ] **Step 10: Full crate + workspace check**

Run: `cargo test -p zoid-tui && cargo build`
Expected: PASS / clean.

- [ ] **Step 11: Commit**

```bash
git add crates/zoid-tui/src/palette.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/render.rs crates/zoid-tui/src/help.rs crates/zoid/src/main.rs
git commit -m "feat(tui): select mode discoverable in palette + help"
```

---

## Final Verification (manual, in a real terminal)

- [ ] Run `cargo run -p zoid` in Kitty. Confirm a dimmed ` SELECT ` pill sits right of the mode pill.
- [ ] Press `Alt+M`: pill turns purple, hint reads "select mode on…". Drag-select prose text and confirm your terminal copies it (with `copy_on_select` or `Ctrl+Shift+C`).
- [ ] Press `Alt+M` again: pill dims. Confirm click-to-copy on a code block and scroll-wheel work again.
- [ ] `Ctrl+P` → confirm an "Enable/Disable select mode" row toggles it too; `:select` and `:mouse` from the `:` box do the same.
- [ ] `?` help overlay lists `Alt+M` and `:select`.
