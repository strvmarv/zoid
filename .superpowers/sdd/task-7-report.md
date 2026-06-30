# Task 7 Report: App-Framework Floor — Key Routing + Mouse Hit-Testing

## Files Changed

| File | Action |
|------|--------|
| `crates/zoid-tui/src/route.rs` | Created — `Action`/`Target` enums, `route_key`, `hit_test`, `route_mouse`, `palette_selected_command`, 9 tests |
| `crates/zoid-tui/src/lib.rs` | Added `pub mod route;` |

## TDD

The brief delivers the complete implementation and test suite as one unit (module did not exist before). First compile was the green run — no red phase.

**Green:** `cargo test -p zoid-tui route` — 9/9 pass on first compile:
```
test route::tests::backtab_switches_mode_tab_cycles_focus ... ok
test route::tests::ctrl_c_quits_and_ctrl_p_opens_palette ... ok
test route::tests::cmdline_enter_parses_command ... ok
test route::tests::colon_opens_cmdline_only_when_not_input ... ok
test route::tests::input_focus_edits_and_submits ... ok
test route::tests::overlay_captures_keys_first ... ok
test route::tests::mouse_click_toggles_drawer_and_focuses ... ok
test route::tests::click_outside_overlay_dismisses ... ok
test route::tests::hit_test_drawer_header_and_panes ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured
```

## MouseEvent Struct Field Check

Verified against `/home/gomanjoe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/crossterm-0.28.1/src/event.rs` line 582. The struct fields are exactly `kind`, `column`, `row`, `modifiers` — matching the brief verbatim. No adjustments needed.

## Clippy

`cargo clippy -p zoid-tui --all-targets -- -D warnings` → clean (zero warnings, zero errors).

## Commit

`0053c59` — `feat(tui): app-framework floor — contextual key routing + mouse hit-test`

---

## Fix Section (Review findings addressed)

### Changes made to `crates/zoid-tui/src/route.rs`

**Fix 1 — Mouse scroll respects overlay gate:**
`route_mouse` previously returned `ScrollConversation` from early-return arms before the overlay check, allowing scrolling to leak through to the conversation while a palette or command-line overlay was open. Rewrote the function to gate on `state.overlay != Overlay::None` first: any left-click, `ScrollDown`, or `ScrollUp` while an overlay is up now returns `Action::CloseOverlay`; all other mouse events return `Action::Noop`. The no-overlay path is behavior-identical to before.

**Fix 2 — Split redundant combined focus arm:**
The `Focus::Conversation | Focus::Rail` arm in `route_key` re-discriminated on `state.focus` internally. Split into two separate arms:
- `Focus::Conversation =>` handles `:` → `OpenCommandLine`, `j`/Down → `ScrollConversation(1)`, `k`/Up → `ScrollConversation(-1)`, Esc → `FocusRegion(Input)`, `_` → `Noop`.
- `Focus::Rail =>` handles `:` → `OpenCommandLine`, Esc → `FocusRegion(Input)`, `_` → `Noop` (j/k rail item-nav deferred to P3).
No behavior change — readability refactor only.

**Fix 3 — Test fixes and additions:**
- Updated comment in `overlay_captures_keys_first` from "it's a non-char combo → Noop" to "the CONTROL guard rejects it from the char arm → Noop".
- Added assertion in `overlay_captures_keys_first` that `^C` → `Noop` under `Overlay::CommandLine` as well.
- Replaced hardcoded `l.drawer_headers[1]` index lookups in `hit_test_drawer_header_and_panes` and `mouse_click_toggles_drawer_and_focuses` with `l.drawer_headers.iter().find(|(id, _)| *id == DrawerId::Files).unwrap()`.
- Added `route_mouse_scroll_moves_conversation`: asserts `ScrollDown` → `ScrollConversation(1)` and `ScrollUp` → `ScrollConversation(-1)` with no overlay; also asserts `ScrollDown` with `Overlay::Palette` → `CloseOverlay` (directly proves Fix 1).
- Added `palette_selected_command_resolves_highlighted_row`: builds a `ShellState` (Chat mode), sets `palette.query = "build"` and `palette.selected = 0`, asserts `palette_selected_command(&s) == Some(Command::SwitchMode(Mode::Build))`.

### Test run

Command: `cargo test -p zoid-tui route`

```
running 11 tests
test route::tests::backtab_switches_mode_tab_cycles_focus ... ok
test route::tests::colon_opens_cmdline_only_when_not_input ... ok
test route::tests::cmdline_enter_parses_command ... ok
test route::tests::input_focus_edits_and_submits ... ok
test route::tests::ctrl_c_quits_and_ctrl_p_opens_palette ... ok
test route::tests::overlay_captures_keys_first ... ok
test route::tests::palette_selected_command_resolves_highlighted_row ... ok
test route::tests::hit_test_drawer_header_and_panes ... ok
test route::tests::click_outside_overlay_dismisses ... ok
test route::tests::mouse_click_toggles_drawer_and_focuses ... ok
test route::tests::route_mouse_scroll_moves_conversation ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured
```

### Clippy

`cargo clippy -p zoid-tui --all-targets -- -D warnings` → clean (zero warnings, zero errors).
