# Chat worktree — design

> **Status:** design (settled, ready for implementation planning). Introduces a
> user- and model-facing capability for the top-level Chat agent to create and
> **enter** a persistent, isolated git worktree, and to leave it reversibly.
> Builds on the existing subagent-only worktree machinery
> (`crates/zoid/src/worktree.rs`). Implementation should follow this document
> and update it if reality diverges.

## Goal

Let zoid's **Chat agent** spin up an isolated git worktree and relocate itself
into it to do work, then leave reversibly — the same ergonomics Claude Code
offers via `EnterWorktree`/`ExitWorktree`. Today zoid can only create worktrees
as an **ephemeral side effect of subagent dispatch** (`dispatch_subagent(worktree:
true)`), rooted at `.zoid/worktrees/<name>` and torn down the instant the
subagent finishes (`WorktreeGuard::drop`). There is no way for the top-level
agent — or the user — to create a worktree, work in it, and keep it.

### The core problem this solves

The Chat agent's working directory is **hardcoded to `"."`**
(`chat_turn_config_with()` in `crates/zoid/src/agent.rs` sets
`cwd: PathBuf::from(".")` unconditionally). Every tool receives `cwd: &Path` and
returns only a `ToolOutput`; a `Local` tool **cannot change the turn's working
directory**, and there is no `set_current_dir` anywhere in the codebase. So even
if the agent shells `git worktree add` (which succeeds and is ungated), it is
then stranded: it created a directory it cannot move itself into. Relocation is
necessarily a **session-state operation owned by the agent loop**, not something
a tool can do mid-turn.

## Scope

Add a **loop-owned, tracked, reversible** worktree relocation for the Chat
agent, reachable from two entrypoints that share one code path:

| Surface | Entrypoint | Kind |
|---------|-----------|------|
| Model | `enter_worktree { name }` tool | Emitting |
| Model | `exit_worktree {}` tool | Emitting |
| User | `:worktree <name>` command | TUI command → same loop handler |
| User | `:exit` command | TUI command → same loop handler |

### Out of scope (v1)

- **Remote worktrees / non-git isolation.** Git worktrees only; requires a git
  repo, as `dispatch_subagent` already checks for `.git`.
- **Nested worktrees.** Entering a worktree while already inside one is refused
  (must exit first). Matches Claude Code's `EnterWorktree` behavior.
- **Relocating a running subagent.** Relocation is a between-turns session op;
  it is refused while a subagent is mid-run.
- **Path jailing.** Unchanged from the rest of zoid's tools — "safe by human
  presence," no sandboxing.

## Design rationale — why loop-owned, not a Local tool

`dispatch_subagent` is an **`Emitting`** tool: instead of doing I/O in `run()`,
it appends a domain event that the agent loop interprets between turns —
including computing the subagent's `cwd` (worktree path if requested) *before*
spawning. That is the exact seam this feature reuses. `enter_worktree` /
`exit_worktree` are Emitting tools that append `EnterWorktree` / `ExitWorktree`
events; the **loop** performs the relocation. The user commands `:worktree` /
`:exit` emit the same events, so autonomous (model) and manual (user) use funnel
through one handler.

## Components

### Reused: `crates/zoid/src/worktree.rs`

`create_worktree(repo_root, name) -> Result<WorktreeGuard>` already opens the
repo via git2, checks out at `repo_root/.zoid/worktrees/<name>` branched from
HEAD, and returns a `WorktreeGuard { name, path, repo_root }`. This is extended,
not replaced (see Lifecycle for the required Drop change).

### New: session-held worktree state

The app/loop gains:

```rust
struct WorktreeSession {
    guard: WorktreeGuard,   // held here so it is NOT dropped on relocation
    prior_cwd: PathBuf,     // the cwd to restore on exit (normally ".")
}
// on the top-level app/loop state:
active_worktree: Option<WorktreeSession>,
```

Holding the `WorktreeGuard` in long-lived session state (rather than in a
subagent's stack frame) is what makes the worktree **persistent**: it is not
dropped — and therefore not removed — until an explicit exit decision.

### New: two Emitting tools + two commands

- `enter_worktree { name: string }` and `exit_worktree {}` — Emitting; each
  appends its event. Registered as **Chat-only** tools (like `dispatch_subagent`
  / `subagent_diff`), not in the base `registry()`, because subagents must not
  relocate the session.
- `:worktree <name>` and `:exit` TUI commands emit the same events.

### New: loop handling of the events

- `EnterWorktree { name }`: validate (git repo; not already in a worktree; no
  active subagent), then `create_worktree()` (or attach to an existing one — see
  Errors), store `WorktreeSession { guard, prior_cwd }`, and set the turn config
  cwd to the worktree path for subsequent turns.
- `ExitWorktree`: restore `prior_cwd`; apply the keep/remove decision (see
  Lifecycle); clear `active_worktree`.

## Data flow

**Enter:** `enter_worktree{name}` (or `:worktree name`) → `EnterWorktree` event →
loop (between turns): validate → `create_worktree` → store `WorktreeSession` →
set `TurnConfig.cwd = worktree path` → subsequent turns and all tool calls run
in the worktree.

**Exit:** `exit_worktree` (or `:exit`) → `ExitWorktree` event → loop: determine
clean/dirty → keep/remove (prompt if dirty) → restore `prior_cwd` → clear state.

## Lifecycle — the critical change

Today `WorktreeGuard::drop` removes the worktree dir, prunes it, and deletes the
branch — all on drop. Persistence requires **decoupling removal from `Drop`**:
while a `WorktreeSession` is held in app state, dropping it on relocation must
**not** delete anything. Removal happens only on an explicit decision:

- **Exit, worktree clean/untouched:** auto-remove (dir + prune + branch),
  restore cwd.
- **Exit, worktree dirty (uncommitted changes):** prompt **keep or remove** via
  the existing `ask_user` / question-overlay path.
  - *Keep* → worktree + branch persist under `.zoid/worktrees/<name>`; only the
    session relocation is undone.
  - *Remove* → delete worktree + branch (the one destructive act; guarded by
    this prompt).
- **Session end while still inside a worktree:** same keep/remove prompt.

Implementation note: introduce an explicit removal method (e.g.
`WorktreeGuard::remove()` / a `keep` flag) so that dropping the guard without
calling it leaves the worktree on disk. This is the inverse of today's
"always-remove-on-drop" and must be covered by tests.

## Errors & guardrails

- **Not a git repo** → clear error (mirror `dispatch_subagent`'s `.git` check).
- **Already inside a worktree** (`active_worktree.is_some()`) → refuse
  `enter_worktree` with "exit the current worktree first." No nesting.
- **Name collision** (`.zoid/worktrees/<name>` already exists) → **enter the
  existing worktree** (idempotent), do not error.
- **Active subagent running** → refuse relocation with a clear message;
  relocation is a between-turns op and must not yank cwd from under a subagent.
- **`exit_worktree` when not in a worktree** → no-op with an informational
  message.
- Uncommitted changes in the *main* checkout are irrelevant: the worktree
  branches from the HEAD **commit**, leaving main's working-tree changes in main.

## Approval-gate posture

Relocation (enter) creates a branch and moves the session — reversible and
non-destructive — so it needs no blacklist entry; it is allowed like
`dispatch_subagent` (which falls through to `Gate::Allow`). The only destructive
act is removing a dirty worktree, and that is already guarded by the keep/remove
prompt. No new `approval.rs` tier entry is required.

## Testing strategy

TDD, mirroring the existing `worktree_test.rs` / `subagent_integration.rs`
density.

Unit (`crates/zoid`):

- `create_worktree` guard held in session state is **not** removed when the
  turn/subagent that created it ends (persistence).
- `WorktreeGuard` dropped **without** an explicit remove leaves the worktree on
  disk; with remove, deletes dir + branch.
- Enter sets the turn cwd to the worktree path; exit restores `prior_cwd`.
- Dirty worktree on exit triggers the keep/remove prompt; keep leaves worktree +
  branch; remove deletes them.
- Name collision enters the existing worktree (no error, no duplicate).
- Enter refused when already in a worktree (no nesting).
- Enter refused while a subagent is running.
- Not-a-git-repo → error.

Integration:

- Model `enter_worktree` tool → `EnterWorktree` event → loop relocates the
  session cwd.
- `:worktree <name>` command routes through the **same** loop handler as the
  tool.
- Enter → do work → exit round-trip restores the original cwd and (on remove)
  cleans up.

## Migration / compatibility

The existing `dispatch_subagent(worktree: true)` path is unchanged — it keeps
its ephemeral, drop-on-finish worktrees for subagent isolation. This feature
adds a *separate*, persistent, session-level worktree owned by the loop. The
`WorktreeGuard` Drop change (decoupling removal) must preserve subagent behavior:
subagent worktrees are still removed when their session ends, because the
subagent path will explicitly request removal (or continue to rely on drop with
removal enabled). The exact mechanism (keep-flag default vs. explicit
`remove()`) is pinned at plan time so subagent cleanup is not regressed.
