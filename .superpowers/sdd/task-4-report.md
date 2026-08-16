# Task 4 Report: Update zoid-tui construction sites

## What I implemented

1. **zoid-tui `ChatMsg::ToolResult` construction sites** — Added `error_kind: None,` to every construction site (field-assignment form) across 5 files. Pattern matches using `..` (e.g. `ChatMsg::ToolResult { id, name, is_error: false, .. }`) were left unchanged.

2. **main.rs ProjectionCache fix (Task 3 review item)** — In `crates/zoid/src/main.rs`, the `ProjectionCache::apply_event` match arm for `EventKind::ToolResult` previously used `..` and constructed `ChatMsg::ToolResult` with `error_kind: None`. Changed the pattern to explicitly bind `error_kind` (replacing `..`) and forwarded it as `error_kind: *error_kind` into the `ChatMsg::ToolResult` construction. The match is over `&ev.kind`, so `error_kind` is `&Option<ErrorKind>`; `ErrorKind` is `Copy` (and therefore `Option<ErrorKind>` is `Copy`), so `*error_kind` produces an owned value.

## Sites updated

23 construction sites total:
- `crates/zoid-tui/src/chat.rs` — 8 sites
- `crates/zoid-tui/src/objects.rs` — 5 sites
- `crates/zoid-tui/tests/chat_snapshot.rs` — 3 sites
- `crates/zoid-tui/tests/shell_snapshot.rs` — 3 sites
- `crates/zoid-tui/examples/scenes/mod.rs` — 4 sites

Plus 1 pattern/forwarding fix in `crates/zoid/src/main.rs` (ProjectionCache).

Pattern matches left unchanged (use `..`):
- `crates/zoid-tui/src/chat.rs:196` — `if let ChatMsg::ToolResult { id, name, is_error: false, .. }`
- `crates/zoid-tui/src/chat.rs:287` — match arm with `..`
- `crates/zoid-tui/src/chat.rs:891` — match arm with `..`
- `crates/zoid-tui/src/objects.rs:51` — `if let` with `..`

## Test output

`cargo test -p zoid-tui`:
- 376 passed; 0 failed (lib)
- 9 passed; 0 failed (chat_snapshot)
- 3 passed; 0 failed (broken_mode / other)
- 42 passed; 0 failed (shell_snapshot-related)
- 2 passed (syntax_snapshot)
- 5 passed (tasks_snapshot)
- 0 doc-tests

`cargo test -p zoid --lib`:
- 171 passed; 0 failed; 0 ignored

All tests pass; no compiler errors or warnings introduced.

## Concerns

- The `error_kind: None,` lines inserted via sed use 16-space indentation in some sites where the surrounding fields use 12 spaces. This is purely cosmetic (Rust ignores field indentation) and does not affect compilation or tests. `cargo fmt -p zoid-tui` would normalize it, but running fmt reformatted many unrelated files (the crate had pre-existing style drift), so I deliberately left indentation as-is to keep the diff minimal and scoped to this task. A separate formatting pass could normalize indentation across the codebase.
- No visual/behavioral change in the TUI is required by this task; `error_kind` is now available for future icon/color differentiation (Goal #3).