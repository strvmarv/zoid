# Worktree Tooling Correctness — Design

**Date:** 2026-07-12
**Status:** Design (approved shape; pending spec review)
**Umbrella:** Spec 2 of 3 under *subagent dispatch safety*
(`2026-07-12-subagent-dispatch-guardrails-design.md` is the parent index).
Independent of Spec 1 (guardrails) and Spec 3 (scheduled wake-ups).

## Goal

The agent-facing `enter_worktree` / `exit_worktree` tools are currently unusable
for isolation. Fix four defects so entering a worktree actually redirects the
agent's work, exiting cleanly returns it, and the TUI reflects the active
worktree:

- **WT-1** — after `enter_worktree`, commits land on the **parent** branch, not
  the worktree's branch; the worktree HEAD stays at its start commit.
- **WT-2** — after `exit_worktree`, every subsequent tool call fails with
  `ENOENT` (`No such file or directory`) — the whole session's tooling breaks.
- **WT-3** — the right-rail **Repo** widget never reflects the active worktree
  (it keeps showing the main checkout's branch/worktree).
- **WT-4** — the redundant worktree hint in the bottom-left status bar.

These affect only the **main Chat agent** (the only agent with the worktree
tools; subagents use the base `registry()` and their own internal
`.zoid/worktrees/sub-*`, which are out of scope here).

## Root cause — one bug behind WT-1 and WT-2

The tool-exec cwd is a **per-turn snapshot**: `let cwd_for_exec = config.cwd.clone()`
(`agent.rs:917`), taken once at turn start and never re-read for the rest of the
turn. `enter_worktree`/`exit_worktree` are `ToolKind::Emitting` (`worktree_enter.rs:37`):
`run_turn_inner` sends `AgentUpdate::WorktreeRequested`, emits an **optimistic**
synthetic success `ToolResult`, and **continues the turn without ending it**
(`agent.rs:1448-1501`). The main-thread handler `handle_worktree_request`
(`main.rs:5261-5355`) creates/removes the worktree and sets `app.active_worktree`
+ `app.shell.cwd` (a display string) + `TurnConfig.cwd` — but `TurnConfig.cwd`
is only consumed by the **next** `spawn_turn` (`main.rs:5396-5403`).

So in the usual "enter, then immediately `git commit`" flow, same-turn tool
calls keep running in the stale pre-switch cwd:

- **WT-1:** the commit runs in the main checkout → lands on the parent branch;
  the worktree branch (correctly created by `create_worktree`, `worktree.rs:71-85`)
  sits untouched.
- **WT-2:** `remove_worktree` deletes the dir (`remove_dir_all`, `worktree.rs:107`)
  while `cwd_for_exec` still points at it; the next tool runs
  `Command::current_dir(<deleted path>)` (`shell.rs:86,103`) → `ENOENT`. Even
  `cd` fails, because the child process cannot spawn with a nonexistent cwd.

The cwd is a **threaded `PathBuf`**, not a process `chdir` (no `set_current_dir`
anywhere), so the fix is tractable: make the worktree switch **visible to the
in-flight turn**.

## The fix — synchronous worktree switch

Turn the fire-and-forget emit into a **request/response** so the switch is
atomic and mid-turn-correct.

### Component 1 — synchronous switch (WT-1 + WT-2)

- `AgentUpdate::WorktreeRequested` gains a `reply: tokio::sync::oneshot::Sender<Result<PathBuf, String>>`
  alongside the existing action (`Enter { name }` / `Exit`).
- In `run_turn_inner` (`agent.rs:1448-1501`): create the oneshot, send the
  request, **`await` the reply**, then:
  - `Ok(new_cwd)` → reassign a now-`mut cwd_for_exec = new_cwd` (`agent.rs:917`)
    and emit a **real** success `ToolResult`.
  - `Err(msg)` → emit a real **error** `ToolResult` carrying `msg`; leave
    `cwd_for_exec` unchanged.
  The optimistic synthetic result is removed. The main `select!` loop keeps
  draining `ui_rx` while the turn awaits, so there is no deadlock.
- In `handle_worktree_request` (`main.rs:5261-5355`): do the git work, then
  `reply.send(Ok(new_cwd))`:
  - **Enter** → `new_cwd` = the worktree's **absolute** path.
  - **Exit** → `new_cwd` = the **absolute** repo root (computed *before*
    `remove_worktree` deletes the dir), replacing today's relative
    `app.shell.cwd = "."` (`main.rs:5352`). The reply is sent, then the dir is
    removed — so no tool ever runs in a deleted cwd.
  - Guard failures (already-in-worktree, subagent running, not-a-git-repo,
    not-in-worktree) → `reply.send(Err(msg))`; state unchanged.

### Component 2 — rail reflects the active worktree (WT-3)

The Repo widget (`render_repo_body`, `render.rs:644-687`) reads
`ShellState::{branch, worktree}` (`state.rs:240-248`), which are written only by
a 5 s background git poller (`main.rs:2216-2241`) that opens the repo at `"."` —
the process cwd, which never changes. Two coordinated changes:

- **Immediate:** `handle_worktree_request` sets `shell.worktree` (the session
  name on enter, `"(none)"` on exit) and `shell.branch` (from the worktree's
  HEAD on enter, the main branch on exit) at the transition, so the rail updates
  with no lag.
- **Durable:** make the poller **worktree-aware** so it confirms rather than
  reverts. Add a `tokio::sync::watch::<Option<PathBuf>>` cell the handler updates
  on enter/exit; each tick the poller opens `active.as_deref().unwrap_or(Path::new("."))`
  for `current_branch()`/`worktree_label()` instead of the hardcoded `"."`.

`shell.cwd` (Session drawer) is already updated at the handler and needs no
change beyond the exit-path absolute-root fix from Component 1.

### Component 3 — drop the bottom-left hint (WT-4)

`status_hint` (`state.rs:263`, rendered `render.rs:375-380`) is a **shared**
transient slot written from ~50 unrelated sites. Remove **only** the ~9
worktree-specific `app.shell.status_hint = Some(...)` writes in
`handle_worktree_request` (`main.rs:5267-5349`) — both the success confirmations
(now redundant with the rail, WT-3) and the guard/error messages (now carried by
the real error `ToolResult`, Component 1). The slot, its render path, and every
other producer are untouched.

## Error handling

- **Worktree op failures are now first-class:** a guard rejection or a git error
  becomes an error `ToolResult` the agent sees and can react to — instead of a
  fake success plus a status-bar whisper.
- **Exit never strands the turn:** the absolute repo root is returned before the
  dir is deleted; the in-flight turn's `cwd_for_exec` is repointed atomically.
- **Reply channel safety:** if the turn drops its receiver (turn aborted between
  send and await), `handle_worktree_request`'s `reply.send` returns `Err` and is
  ignored — the worktree state is still consistent for the next turn.
- **Dirty worktree on exit:** existing behavior preserved — a worktree with
  uncommitted changes is kept (not removed); the reply still returns the repo
  root so tooling keeps working, and the (now real) `ToolResult` reports the kept
  branch.

## Testing

- **`worktree.rs`** create/remove unit tests remain the git-level guarantee (a
  worktree branch is created from HEAD; removal prunes dir + branch).
- **Switch semantics:** a headless test drives `handle_worktree_request` and
  asserts the reply cwd — Enter → the worktree's absolute path, Exit → the
  absolute repo root (not `"."`), guard cases → `Err`.
- **WT-1 (integration):** enter a worktree, run a commit tool in the same turn,
  assert the commit is on the worktree branch and the parent branch is unmoved.
- **WT-2 (integration):** after exit, assert a following tool call runs in the
  repo root and succeeds (no `ENOENT`).
- **WT-3:** after enter, `shell.worktree` = session name and `shell.branch` =
  worktree branch; the poller's watch cell holds the worktree path; after exit
  both reset and the cell is `None`.
- **WT-4:** no `status_hint` is written on any worktree op (success or guard);
  other `status_hint` producers are unaffected.

## Non-goals (YAGNI)

- Subagent-dispatch worktrees (`.zoid/worktrees/sub-*`) and their rail display —
  out of scope; the rail indicator is for the main loop only.
- Making tool cwd a real process `chdir` — the threaded `PathBuf` is correct and
  keeps subagents/tools isolated; only its mid-turn visibility is fixed.
- Replacing the 5 s poller with a fully event-driven git-state feed — the poller
  is kept and made worktree-aware.
- Any change to the guardrails (Spec 1) or scheduled-wake (Spec 3) subsystems.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid/src/agent.rs` | `AgentUpdate::WorktreeRequested` gains a `oneshot` reply; `run_turn_inner` awaits it, updates `mut cwd_for_exec`, emits a real success/error `ToolResult` (drop the optimistic one) |
| `crates/zoid/src/main.rs` | `handle_worktree_request` replies with the new absolute cwd (worktree path / repo root), computed before `remove_worktree`; sets `shell.branch`/`shell.worktree` on transition; updates the poller's `watch<Option<PathBuf>>`; removes the ~9 worktree `status_hint` writes; make the 5 s poller open the active-worktree path |
| `crates/zoid/src/worktree.rs` | none expected (git-level create/remove already correct); confirm `remove_worktree` ordering only |
| `crates/zoid-tools/src/worktree_enter.rs`, `worktree_exit.rs` | Emitting stubs unchanged (still signal via the turn loop) |
| `crates/zoid-tui/src/render.rs` / `state.rs` | none (rail already renders `shell.branch`/`worktree`; fix is upstream in what feeds them) |
