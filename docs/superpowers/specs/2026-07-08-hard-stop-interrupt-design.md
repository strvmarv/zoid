# Hard-Stop Interrupt — Design Spec

**Date:** 2026-07-08
**Status:** Approved (ready for implementation plan)
**Author:** brainstormed with strvmarv

## Problem

Pressing `Esc` (or `Ctrl-C`) during a running agent turn fires a single
`CancellationToken` (`App::turn_cancel`) that is checked only at *cooperative*
boundaries — between sub-turns (`agent.rs:423`), in a `select!` on the provider
channel (`agent.rs:456`), and before each tool in a batch (`agent.rs:658`). The
provider **stream** is therefore interruptible today, but a **running tool is
not**: the Local-tool call site awaits `spawn_blocking(run_tool).await`
(`agent.rs:1221`) with no `select!` on the token, and the shell tool underneath
runs `std::process::Command::output()` which spawns, waits, and drops the
`Child` internally. Result: a long `shell` command (e.g. `make`) or a hung MCP
call wedges the turn — `Esc` does nothing until the tool returns on its own.

## Goal

Give the user a two-tier interrupt that can stop in-flight work mid-execution,
including killing a running OS process tree, without corrupting conversation
history.

## Core principle (governs every ambiguity)

**Graceful (first `Esc`) abandons *network* waits and stops starting new work.
Hard (second `Esc`) force-kills the *running local* tool.**

The split follows from what is safe to tear down: a network wait has no local
side effect to lose, so first `Esc` drops it immediately; a running shell
command has side effects, so first `Esc` lets it finish and only a deliberate
second `Esc` kills it.

| In-flight operation | Esc #1 — graceful | Esc #2 — hard |
|---|---|---|
| Provider token stream | interrupt *(already works today)* | — |
| MCP call (network wait) | **abandon the await** *(new)* | — |
| Shell child (`make`, etc.) | let it finish | **SIGKILL its process group** *(new)* |
| Other blocking local tool (`search`, `read_file`, git-diff) | let it finish | abandon the wait; benign bg op finishes orphaned |

**Balancing invariant (load-bearing):** every `tool_use` that was started must
receive a synthesized `tool_result` when interrupted — `[killed: hard-stop]` for
a hard-killed tool, `[skipped: turn aborted]` for a skipped/abandoned one. The
existing balanced-drain logic (`agent.rs:1147`, `agent.rs:562`) is preserved and
extended to cover the newly-interruptible awaits. No unbalanced
`tool_use`/`tool_result` pair may reach the next provider request.

## UX

No new keybinding. `Esc` still produces `Action::CancelTurn` (`route.rs:149`,
Ctrl-C aliased). Escalation lives entirely in the `main.rs` handler
(`Action::CancelTurn`, currently `main.rs:3053`):

- If the graceful token is **not** yet fired → fire graceful; status hint:
  `cancelling… (Esc again to force)`.
- If the graceful token **is** already fired (turn still running) → fire hard;
  status hint: `force-stopping…`.

No time window: a second `Esc` at any point while the turn is still running and
graceful is already fired escalates to hard. `App::turn_control.is_some()` (the
replacement for `turn_cancel.is_some()`) still drives `state.cancellable`
(`main.rs:2070`), and is cleared on `TurnComplete` (`main.rs:2276`).

## Architecture

### 1. `zoid-tools` — killable shell + kill registry

- **`KillSlot`** — new newtype wrapping `Arc<Mutex<Option<u32>>>` (holds one
  process-group id). Methods:
  - `new() -> KillSlot`
  - `register(pgid: u32)` — record the running child's pgid.
  - `clear()` — called when the child exits normally.
  - `kill()` — Unix: if a pgid is present, `killpg(pgid, SIGKILL)` (signal the
    **negative** pgid / whole group); ESRCH (already gone) is ignored. Non-Unix:
    best-effort `child.kill()` of the immediate child (see fallback below).
  A single slot is sufficient because Local tools in a batch run
  **sequentially** — at most one shell child exists at a time.

- **`shell.rs`** — replace `Command::output()` (`shell.rs:43`) with:
  - `spawn()` after setting the child into its own process group:
    `std::os::unix::process::CommandExt::process_group(0)` (stable since Rust
    1.64; makes the child's pid its pgid).
  - `register(pgid)` into the `KillSlot`, then collect output via
    `wait_with_output()`, then `clear()`.
  - On Unix, killing the negative pgid takes down the entire subtree
    (`sh -c "make"` → `cc` grandchildren). This is the reason for the process
    group: killing only `sh`'s pid would orphan the grandchildren.

- **`ShellTool`** — constructed holding a clone of the shared `KillSlot`. The
  `Tool` trait signature (`fn run(&self, args, cwd) -> ToolOutput`,
  `lib.rs:63`) is **unchanged** — all 11 tools keep their signature; the shell
  tool carries the kill handle internally as struct state.

- **Cargo** — add a unix-gated dependency for `killpg`/`SIGKILL`
  (`nix` with the `signal` feature, or `libc`). `process_group` itself is std.

- **Non-Unix fallback** — `process_group`/`killpg` are Unix-only. On non-Unix,
  `kill()` falls back to killing the immediate child only (grandchildren may
  orphan); documented limitation. zoid's primary target is Linux.

### 2. `zoid` `agent.rs` — two tokens + interruptible awaits

- **Turn control** — replace the single `CancellationToken` threaded through
  the loop (`agent.rs:325/337/414`) with a two-token control: `graceful` and
  `hard` (both `tokio_util::sync::CancellationToken`). `run_agent_turn`
  (uninterruptible entry, `agent.rs:299`) passes two never-fired tokens.

- **Local-tool call site** (`agent.rs:1221`) — wrap the
  `spawn_blocking(run_tool).await` in `tokio::select!`:
  - `handle` completes → normal `ToolOutput`.
  - `hard.cancelled()` fires → call `kill_slot.kill()` (unblocks the blocking
    `wait` in shell.rs), synthesize `ToolOutput::err("[killed: hard-stop]")`,
    return control immediately. The orphaned `spawn_blocking` finishes quickly
    after the SIGKILL (or, for non-process tools, finishes its bounded work in
    the background — benign).
  - The `KillSlot` is threaded into the turn (see main.rs) so the call site can
    invoke `kill()`.

- **MCP dispatch** (`agent.rs:1176`, `m.call_tool(...).await`) — wrap in
  `tokio::select!` on `graceful.cancelled()`; on fire, drop the future
  (abandons the pending `oneshot` in `client.rs:125`) and synthesize
  `[skipped: turn aborted]`. The MCP **server is not killed** — it is persistent
  and shared across tools; only this one request is abandoned.

- **MCP timeout (defense in depth)** — add a wall-clock timeout to the MCP call
  path (currently none), so a hung server cannot wedge a turn even without
  `Esc`. Reuse the existing MCP `REQUEST_TIMEOUT` constant if present; otherwise
  add one. On timeout, synthesize an error `tool_result`.

- **Balancing** — extend the existing drain so both the hard-killed local tool
  and the abandoned MCP call leave a synthesized result; remaining unstarted
  tools in the batch keep the existing `[skipped: turn aborted]` treatment.

### 3. `zoid` `main.rs` — wiring

- Build one shared `KillSlot` at startup; give one clone to the shell tool in
  the Local tools list and one clone to the turn (via `TurnConfig`).
- `App::turn_cancel: Option<CancellationToken>` (`main.rs:1398`) becomes
  `App::turn_control: Option<TurnControl>` holding `{ graceful, hard }`.
  `spawn_turn` (`main.rs:4432`) mints both tokens.
- `Action::CancelTurn` handler (`main.rs:3053`) does the escalation described in
  **UX** and sets the two status-hint strings. The second fire site in the
  question-overlay path (`main.rs:2308`) is updated to the graceful token.

### 4. `zoid-tui` — status text only

- Two status-hint strings (`cancelling… (Esc again to force)` and
  `force-stopping…`). No routing change; `Action::CancelTurn` and the
  `state.cancellable` gate are unchanged.

## Data flow (hard-stop of a running `make`)

1. User presses `Esc` #1 → `CancelTurn` → graceful token fired → provider
   stream (if any) already done; the running `shell` child keeps going; hint
   shows `cancelling… (Esc again to force)`.
2. User presses `Esc` #2 → `CancelTurn` → handler sees graceful already fired →
   hard token fired → hint `force-stopping…`.
3. The Local-tool `select!` at `agent.rs:1221` takes the `hard.cancelled()`
   branch → `kill_slot.kill()` → `killpg(pgid, SIGKILL)` reaps `sh` + `make` +
   `cc`.
4. The blocking `wait_with_output()` in shell.rs returns; `spawn_blocking`
   finishes; the call site has already synthesized
   `ToolResult { is_error: true, output: "[killed: hard-stop]" }`.
5. Drain balances any remaining tool calls; turn ends; `turn_control` cleared;
   `cancellable` goes false.

## Error handling

- `kill()` on an empty slot (tool finished as hard fired) → no-op.
- `killpg` ESRCH (group already exited) → ignored.
- Race where the tool completes normally the same instant hard fires →
  whichever `select!` branch wins; both produce a balanced result.
- Non-Unix → immediate-child kill fallback; grandchildren may orphan
  (documented).
- MCP abandon leaves the server running; the persistent connection is reused by
  later turns.

## Testing

- **Escalation state machine** — pure test over the handler logic: first
  `CancelTurn` fires graceful, second fires hard; `cancellable` reflects
  `turn_control.is_some()`.
- **Process-group kill (the critical one)** — spawn via the shell tool a
  command that starts a grandchild which writes a sentinel file only *after* a
  sleep (e.g. `sh -c 'sleep 5 && touch SENTINEL'` wrapped so a grandchild owns
  the sleep); call `KillSlot::kill()`; assert the sentinel **never appears**
  within a generous window — proving the whole group died, not just `sh`.
  Mirrors the MCP transport reap test (`sleep 30`).
- **Hard-cancel mid-shell** — a long-running shell tool interrupted by the hard
  token yields `is_error` `ToolResult` with `[killed: hard-stop]` and the batch
  stays balanced.
- **Graceful-cancel mid-MCP-call** — using the fake MCP server fixture, a
  graceful cancel during an in-flight `call_tool` synthesizes
  `[skipped: turn aborted]` and abandons the request.
- **MCP timeout** — a fake server that never replies causes the call path to
  time out with an error `tool_result` (no `Esc` needed).
- **Balancing invariant** — an interrupted batch has exactly one
  `tool_result` for every started `tool_use`.

## Scope boundaries (YAGNI)

- Only `shell` gets true child-kill. `subagent_diff`'s `git` subprocess calls
  and `search`/`read_file` get abandon-wait (bounded, benign) rather than their
  own kill plumbing — flagged, not built.
- Streaming's existing graceful-interrupt behavior is left exactly as-is.
- No configurable escalation window, no third tier, no per-tool kill policy.

## Global constraints

- env values are NEVER logged (they may carry secrets).
- No `Co-Authored-By` / co-author trailer on commits.
- `kill()` must be best-effort and panic-free on the hot path (no `unwrap` on a
  possibly-exited process).
- The `Tool` trait signature must not change.
- Provider message history must never contain an unbalanced
  `tool_use`/`tool_result` pair after an interrupt.
