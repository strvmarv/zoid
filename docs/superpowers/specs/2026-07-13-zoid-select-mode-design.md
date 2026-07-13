# zoid "Select mode" — runtime mouse-capture toggle

**Date:** 2026-07-13
**Status:** Design (approved for planning)
**Area:** `crates/zoid-tui` (route/state/render/palette), `crates/zoid` (bin run loop)

## Problem

zoid enables terminal mouse capture at startup
(`crates/zoid/src/main.rs:2176`, `EnableMouseCapture`). While capture is on, the
terminal emulator forwards all click/drag events to the app instead of doing its
own text selection, so a normal drag inside the zoid window selects nothing.

Holding **Shift** is the terminal's built-in bypass (native selection), which is
why users *can* select that way — but copying that selection is then the
terminal's job via a terminal-specific gesture (`Ctrl+Shift+C`, `copy_on_select`,
middle-click). Users trying to grab **arbitrary partial text** (a path, a URL, a
sentence fragment) find the flow awkward and, depending on terminal config,
copy appears to do nothing.

Existing copy support is limited to **code blocks**: `handle_conversation_click`
(`main.rs:826`) copies a clicked code block's source via `copy_to_clipboard_osc52`
(`main.rs:812`, OSC 52, SSH-safe). There is no path for arbitrary sub-message text.

## Goal

Let the user toggle mouse capture off at runtime ("select mode") so the entire
zoid window behaves like a plain terminal — native drag-select and the terminal's
own copy (`copy_on_select` / middle-click / `Ctrl+Shift+C`) work across the whole
window with no Shift required — then toggle it back on to restore zoid's mouse
features.

### Non-goals (YAGNI)

- No `arboard`/platform clipboard dependency.
- No new "copy whole message" feature; the existing code-block OSC 52 copy is
  untouched.
- No persistence of the mode across restarts (always starts with capture **on**,
  today's behavior).
- No momentary/hold-to-select; the toggle is sticky.

## Design

### Triggers (both)

- **Key:** `Alt+M` (mnemonic **M**ouse). `Alt` is already zoid's global-toggle
  modifier (`Alt+P` provider switch, `Alt+←/→` zoom). Avoids `Ctrl` pitfalls:
  `Ctrl+S` = XOFF flow-control freeze, `Ctrl+B` = tmux prefix, and
  `Ctrl+C/Q/P/O` are already bound (`route.rs:233-246`).
- **Palette:** `:select` (alias `:mouse`) — added to `parse_command`
  (`command.rs`) and surfaced as a discoverable row in `all_items`
  (`palette.rs`), mirroring the existing companion on/off row (the row offers the
  *opposite* of the current state: "Enable select mode" / "Disable select mode").

### State

- New field `ShellState.select_mode: bool`, default `false` (= capture on).
- `false` → mouse capture enabled (today's behavior: click-to-copy code,
  choice-clicks, scroll-wheel routing all active).
- `true` → mouse capture disabled; those three features are suspended and the
  terminal owns the mouse for native selection.

### Control flow

1. `route.rs`:
   - Global combo: `Alt+M` → new `Action::ToggleMouseCapture` (added near the
     other `alt(...)` global combos, ~`route.rs:255`).
   - `:select` / `:mouse` parse to a new `Command` variant (e.g.
     `Command::ToggleSelectMode`).
2. The `zoid` bin's run loop intercepts `Action::ToggleMouseCapture` **in the
   loop itself** — not in `handle_action` — because applying it needs the
   terminal backend handle, exactly like `Action::ConversationClick` is resolved
   in the loop (`main.rs:2697`). `handle_action`/`route_key` do not hold the
   backend.
3. On toggle the loop:
   - flips `app.shell.select_mode`;
   - runs `execute!(terminal.backend_mut(), EnableMouseCapture)` or
     `DisableMouseCapture` accordingly (the runtime inverse of the startup
     enable — crossterm writes the DECSET/DECRST escape; no alt-screen re-init);
   - sets a transient `status_hint` ("select mode on — native copy" /
     "select mode off").
4. The palette `Command::ToggleSelectMode` resolves to the same effect (the bin
   maps it onto the same toggle path).

### Feedback

- **Persistent indicator** while `select_mode` is on, rendered in the footer/
  status area in `render.rs` from `state.select_mode` (distinct from the
  transient `status_hint` used for "copied N lines"): a short marker such as
  `SELECT`. It must be persistent because this is a *mode* — otherwise the user
  cannot tell why click/scroll gestures stopped responding.
- **Help entry:** one line in `help.rs` — "Alt+M — select mode: native text
  selection & copy (suspends mouse click/scroll)".
- **Exit:** no restore logic needed — the existing teardown
  `DisableMouseCapture` (`main.rs:2194`) runs on clean exit regardless of mode.

## Testing

- **route unit test:** `Alt+M` → `Action::ToggleMouseCapture`;
  `parse_command(":select")` and `parse_command(":mouse")` →
  `Command::ToggleSelectMode`.
- **palette unit test:** `all_items(..)` includes the select-mode row, and it
  offers the opposite label for `select_mode` true vs false (mirrors the
  existing companion-row test).
- **snapshot test:** footer shows the `SELECT` indicator when
  `select_mode == true` and hides it when false.
- **manual verification (Kitty on CachyOS):** toggle on → drag-select arbitrary
  text and confirm `copy_on_select`/`Ctrl+Shift+C` copies it; toggle off →
  confirm click-to-copy-code and scroll-wheel return.

## Affected files

- `crates/zoid-tui/src/route.rs` — `Action::ToggleMouseCapture`, `Alt+M` combo.
- `crates/zoid-tui/src/command.rs` — `Command::ToggleSelectMode`, parse entries.
- `crates/zoid-tui/src/state.rs` — `ShellState.select_mode`.
- `crates/zoid-tui/src/palette.rs` — discoverable toggle row in `all_items`.
- `crates/zoid-tui/src/render.rs` — persistent `SELECT` indicator.
- `crates/zoid-tui/src/help.rs` — help line.
- `crates/zoid/src/main.rs` — run-loop interception + `execute!` toggle;
  palette `Command::ToggleSelectMode` handling.
