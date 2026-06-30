## Task 10 Report: Wire the modal shell into the binary

### What was wired

1. **Deleted `crates/zoid/src/input.rs`** (old `classify`/`KeyAction` classifier) and removed `pub mod input;` from `crates/zoid/src/lib.rs`.

2. **Extended `App`** with `shell: zoid_tui::ShellState` and `ui_tx: mpsc::Sender<AgentUpdate>`. Added `current_branch()` (reads `.git/HEAD`) and `cwd_files(limit)` (reads cwd, filters hidden entries, sorts). Both are populated before `App` construction in `main()`.

3. **Mouse capture**: added `EnableMouseCapture` after `EnterAlternateScreen` on enter; `DisableMouseCapture` before `LeaveAlternateScreen` on teardown.

4. **Rendering**: replaced `render_chat` with `render_shell(f, &app.shell, &msgs, &app.textarea, app.streaming)`.

5. **Event routing**: key events go through `route_key(&app.shell, key)` → `handle_action`; mouse events compute the layout from `terminal.size()` (see API adjustment below) then call `route_mouse(&app.shell, &layout, me)` → `handle_action`.

6. **`handle_action`**: full Action interpreter — all 19 arms including Quit, SwitchMode, FocusNext, FocusRegion, OpenPalette, OpenCommandLine, CloseOverlay, ToggleDrawer, PaletteMove/Char/Backspace/Run, CmdlineChar/Backspace, RunCommand, ScrollConversation, Newline, Edit, Submit, Noop.

7. **`exec_command`**: interprets `Command::{Quit, SwitchMode, OpenDrawer, Unknown}`.

8. **`spawn_turn`**: extracted from old Submit arm; clones provider/tools/session/seed/model/ui_tx and spawns the async agent turn.

9. **`AgentUpdate` receive arm**: unchanged — `Appended` pushes the event, `TurnComplete` clears `streaming`.

### API adjustments

**`ui_tx` ownership**: The brief's `spawn_turn` signature reads from `app.ui_tx`. Moving channel construction before `App` construction and storing `ui_tx` on `App` (with `ui_rx` as a local passed to `run`) was necessary. The `run` function was given a third parameter `ui_rx: &mut mpsc::Receiver<AgentUpdate>` rather than creating the channel inside `run`, so that `ui_tx` could live on `App` for `spawn_turn` to clone.

**Terminal area for mouse layout**: `terminal.get_frame()` is only valid inside the draw closure. Used the brief's suggested workaround: `terminal.size().map(|s| Rect { x: 0, y: 0, width: s.width, height: s.height }).unwrap_or_default()`. In ratatui 0.29, `Terminal::size()` returns `io::Result<Size>` with `width`/`height` fields, which maps cleanly to a `Rect`.

**No other adjustments**: crossterm 0.28 is workspace-pinned and unified across the bin, ratatui's re-export, and tui-textarea — so `KeyEvent`/`MouseEvent` pass through route functions and `textarea.input()` without conversion.

### Build / test / clippy results

- `cargo build -p zoid`: **clean** (0 errors, 0 warnings)
- `cargo test --workspace`: **95 tests pass** — 2 agent_loop integration tests, 12 zoid-core, 1 round_trip, 24 zoid-provider, 18 zoid-tools, 33 zoid-tui unit tests, 5 chat snapshots, 5 shell snapshots — all pass.
- `cargo clippy --all-targets -- -D warnings`: **0 warnings** across the workspace.

### Smoke test

No TTY available in this environment. Manual smoke-test was skipped per the brief's guidance ("If no TTY is available, skip and note it — the routing is covered by Task 7's unit tests."). The routing is fully covered by the 33 zoid-tui unit tests (route, layout, palette, state) and the 5 shell snapshot tests.

### Commit

`4ae503c feat(zoid): drive the modal shell — route events, palette, command line, mouse`

### Fix: Honor quit signal from mouse-routed actions

**Change**: In `run()`'s event loop (line 157), the mouse arm was discarding `handle_action`'s quit signal (`Ok(bool)` return). Changed from:
```rust
let _ = handle_action(app, route_mouse(&app.shell, &layout, me)).await?;
```
to:
```rust
if handle_action(app, route_mouse(&app.shell, &layout, me)).await? {
    return Ok(());
}
```
This matches the key-event arm's behavior and allows mouse actions to properly trigger quit.

**Build results**:
- `cargo build -p zoid`: **clean** (Finished in 1.07s, 0 errors)
- `cargo clippy --all-targets -- -D warnings`: **0 warnings** (Finished in 0.32s)
- `cargo test --workspace`: **all pass** (5 zoid snapshots + all others pass, 0 failures)

**Commit**: `181e568 fix(zoid): honor quit signal from mouse-routed actions`

---

## Fix: §16 tokenize palette/⑤/⎇ glyphs; esc exits Build; apply conversation scroll

### Changes (commit `d1e1213`)

**Fix 1 — §16 tokenize literal glyphs (single source of truth)**

- `crates/zoid-tui/src/tokens.rs`: Added 7 new char consts to `pub mod glyph`: `CONTEXT ('⑤')`, `MODE_SWITCH ('⇢')`, `UNDO ('⤺')`, `OPEN ('▤')`, `EVICT ('✕')`, `SETTINGS ('◆')`, `RECIPE ('▷')`. Existing `EDIT ('●')` and `BRANCH ('⎇')` are reused; no duplicates.
- `crates/zoid-tui/src/palette.rs`: Changed `PaletteItem.group` from `&'static str` to `String`. In `all_items`, replaced all literal icon chars with token constants (`glyph::MODE_SWITCH`, `glyph::UNDO`, `glyph::OPEN`, `glyph::EDIT`, `glyph::EVICT`, `glyph::SETTINGS`, `glyph::RECIPE`) and built group strings via `format!` for the two dynamic groups (`"branch {} · post-v1"` with `glyph::BRANCH` and `"context {} · P3"` with `glyph::CONTEXT`) and `.to_string()` for the static groups.
- `crates/zoid-tui/src/state.rs`: Added `use crate::tokens::glyph;`. Changed the Economy drawer title from the literal `"⑤ context · tokens"` to `format!("{} context · tokens", glyph::CONTEXT)`.
- `crates/zoid-tui/src/render.rs`: Updated `render_palette` to track `last_group` as `String` (was `&'static str`), comparing with `!=` and assigning via `.clone()`. The rendered uppercase text is unchanged.

**Fix 2 — Esc exits Build mode (spec §6.2)**

- `crates/zoid-tui/src/route.rs`: Imported `Mode` from state. Inserted a mode-level guard before the focus-contextual match: if `state.mode == Mode::Build && key.code == KeyCode::Esc`, returns `Action::SwitchMode`. This fires only when no overlay is active (the overlay gate returns early above it).
- Added two tests: `esc_exits_build_mode` (Build + Esc → `SwitchMode`) and `esc_in_chat_conversation_focus_returns_to_input` (Chat + Conversation focus + Esc → `FocusRegion(Input)`, unchanged behavior).

**Fix 3 — Apply conversation scroll offset**

- `crates/zoid-tui/src/render.rs`: Changed `Paragraph::new(body)` to `Paragraph::new(body).scroll((state.conversation_scroll, 0))` in the Chat branch of `render_shell`.

### Snapshot verification

All 10 snapshots are unchanged (no `.snap.new` files generated). The glyph substitutions are character-identical; `conversation_scroll` is 0 in all snapshot tests so `.scroll((0,0))` is a no-op.

### Test / clippy results

- `cargo test -p zoid-tui`: **35 unit tests pass, 5 chat snapshots pass, 5 shell snapshots pass** — no pending `.snap.new` files.
- `cargo test --workspace`: **all pass** (97 tests total across all crates).
- `cargo clippy --all-targets -- -D warnings`: **0 warnings**.

**Commit**: `d1e1213 fix(tui): §16 tokenize palette/⑤/⎇ glyphs; esc exits Build; apply conversation scroll`
