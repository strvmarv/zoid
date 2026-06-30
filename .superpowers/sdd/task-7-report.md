# Task 7 Report: Terminal-Free Agent Loop

## Files Changed

| File | Action |
|------|--------|
| `crates/zoid/Cargo.toml` | Added `[lib]` section, `zoid-tools` + `serde_json` to `[dependencies]`, `[dev-dependencies]` with `tempfile` + `async-trait` |
| `crates/zoid/src/lib.rs` | Created — `pub mod agent; pub mod input;` |
| `crates/zoid/src/agent.rs` | Created — terminal-free agent loop per brief |
| `crates/zoid/src/main.rs` | Removed `mod input;`, replaced `use input::{...}` with `use zoid::input::{...}` |
| `crates/zoid/tests/agent_loop.rs` | Created — integration test with ScriptedProvider |

## Deviations from Brief

- `serde_json` was added to `[dependencies]` (not just `[dev-dependencies]`) because `agent.rs` uses it for `from_str`/`Value::Null` in `map_msg` and is part of the lib crate.
- The brief listed `serde_json` only in `[dev-dependencies]`. The fix was moving it to `[dependencies]` while keeping `async-trait` in `[dev-dependencies]`.

## Test Output

```
cargo test -p zoid

running 4 tests (lib unit tests)
test input::tests::ctrl_c_quits ... ok
test input::tests::plain_char_is_edit ... ok
test input::tests::plain_enter_submits_alt_enter_newlines ... ok
test input::tests::shift_tab_toggles_mode ... ok
test result: ok. 4 passed; 0 failed

running 1 test (integration)
test agent_loop_runs_tool_then_finishes ... ok
test result: ok. 1 passed; 0 failed
```

Full workspace: 70 tests, 0 failures.

## Clippy Output

```
cargo clippy -p zoid --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s)
```
0 warnings.

## Review Fix Report (patch applied on top of task-7 baseline)

### Finding 1 — CRITICAL: TurnComplete on every exit path

**Change:** Extracted the loop body into `async fn run_turn_inner(...) -> Result<()>`. `run_agent_turn` now calls `run_turn_inner`, then sends `TurnComplete` via `let _ = ui.send(AgentUpdate::TurnComplete).await` (best-effort, ignoring channel-closed errors), then returns the inner result. The `?` propagation inside `run_turn_inner` is preserved so session errors still surface to the caller — they are not silently swallowed.

### Finding 2 — IMPORTANT: Remove `zoid_tui` dep from `agent.rs`

**Change:** Added `const WARN_GLYPH: char = '⚠';` at module level in `agent.rs`. Replaced both `zoid_tui::tokens::glyph::WARNING` references (Error arm + iteration-cap message) with `{WARN_GLYPH}`. The `zoid_tui` crate still appears in `Cargo.toml` (needed by the binary/rendering layer) but `agent.rs` no longer imports it.

Verification: `grep -n zoid_tui crates/zoid/src/agent.rs` → no matches.

### Finding 3 — MINOR: Assert re-request includes tool result

**Change:** Added `requests: Mutex<Vec<CompletionRequest>>` to `ScriptedProvider`. In `stream`, cloned `req` and pushed it into `requests` before the `for ev in script` loop; no lock is held across any `.await`. Added assertion after `run_agent_turn` in `agent_loop_runs_tool_then_finishes`:
```
assert_eq!(captured.len(), 2, "expected exactly 2 provider requests");
assert!(captured[1].messages.iter().any(|m| m.role == MsgRole::Tool), ...);
```

### Finding 4 — MINOR: Error-path test

**Change:** Added `agent_loop_returns_ok_and_emits_turn_complete_on_error_event` to `tests/agent_loop.rs`. Scripted provider sends `ProviderEvent::Error("boom")`. Asserts: `run_agent_turn` returns `Ok(())`, a `TurnComplete` was received, and the session log contains an `AssistantMessage` whose text contains `"boom"`.

### Bonus fix: deterministic ULID ordering in existing test

The existing test used `Ulid::new()` for the seed event. When two tests run in parallel within the same millisecond, ULID random bits can cause the seed to sort after events emitted by `run_agent_turn`, breaking `assert!(matches!(kinds[0], EventKind::UserMessage { .. }))`. Fixed by using `Ulid::from(1u128)` for the seed — a epoch-0 ULID that always sorts before any wall-clock ULID generated today.

### Test output (`cargo test -p zoid`)

```
running 4 tests (lib unit tests)
test input::tests::ctrl_c_quits ... ok
test input::tests::plain_char_is_edit ... ok
test input::tests::plain_enter_submits_alt_enter_newlines ... ok
test input::tests::shift_tab_toggles_mode ... ok
test result: ok. 4 passed; 0 failed

running 2 tests (integration)
test agent_loop_returns_ok_and_emits_turn_complete_on_error_event ... ok
test agent_loop_runs_tool_then_finishes ... ok
test result: ok. 2 passed; 0 failed
```

Full workspace: all test result lines show 0 failed.

### Clippy output (`cargo clippy -p zoid --all-targets`)

```
Finished `dev` profile [unoptimized + debuginfo] target(s)
```
0 warnings (including no `await_holding_lock`).

### `agent.rs` zoid_tui-free confirmation

`grep -n zoid_tui crates/zoid/src/agent.rs` → no output (no matches).

### Uncovered: iteration cap test

Adding a test for `MAX_TOOL_ITERATIONS` (= 25) would require scripting 26 tool-call turns; skipped per brief instructions.
