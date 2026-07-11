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
| User | `:worktree exit` command | TUI command → same loop handler |

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
it sends an `AgentUpdate` that the main loop interprets between turns —
including computing the subagent's `cwd` (worktree path if requested) *before*
spawning. That is the exact seam this feature reuses. `enter_worktree` /
`exit_worktree` are Emitting tools that send a `WorktreeRequested`
`AgentUpdate`; the **main loop** performs the relocation (see Relocation
site). The user commands `:worktree <name>` / `:worktree exit` send the same
`AgentUpdate`, so autonomous (model) and manual (user) use funnel through one
handler.

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
    path: PathBuf,       // the worktree's absolute path (from into_kept)
    name: String,        // branch name (from into_kept), for removal on exit
    prior_cwd: PathBuf,  // the cwd to restore on exit (normally ".")
}
// on the top-level app/loop state:
active_worktree: Option<WorktreeSession>,
```

The `WorktreeGuard` is consumed via `into_kept()` at enter time (see
Lifecycle); `WorktreeSession` holds the resulting `(path, name)`, not the
guard itself. This is what makes the worktree **persistent**: the guard's
`Drop` (unconditional removal) never fires while the session owns it, because
the guard no longer exists. Removal happens only on an explicit exit decision.

### New: two Emitting tools + two commands

- `enter_worktree { name: string }` and `exit_worktree {}` — Emitting; each
  appends its event. Registered as **Chat-only** tools (like `dispatch_subagent`
  / `subagent_diff`), not in the base `registry()`, because subagents must not
  relocate the session.
- `:worktree <name>` and `:worktree exit` TUI commands emit the same events.
  (`:worktree exit`, not bare `:exit`, avoids colliding with a user's
  expectation that `:exit` quits the application.)

### New: loop handling of the relocation request

The `WorktreeRequested` `AgentUpdate` is handled in the main `run()` loop
(see Relocation site for the exact flow):

- **`Enter { name }`**: validate (git repo; not already in a worktree; no
  active subagent), then `create_worktree()`, call `.into_kept()` to consume
  the guard without removal, store `WorktreeSession { path, name, prior_cwd }`.
  The cwd change takes effect on the next `spawn_turn`, which reads
  `app.active_worktree`.
- **`Exit`**: restore `prior_cwd`; apply the keep/remove decision (see
  Lifecycle); clear `active_worktree`.

## Data flow

**Enter:** `enter_worktree{name}` (or `:worktree name`) → Emitting handler
sends `AgentUpdate::WorktreeRequested { Enter }` + `ToolResult` echo → main
`run()` loop: validate → `create_worktree` → `into_kept()` → store
`WorktreeSession` → next `spawn_turn` reads `app.active_worktree` and sets
`turn_config.cwd = worktree path` → subsequent turns and all tool calls run
in the worktree.

**Exit:** `exit_worktree` (or `:worktree exit`) → `WorktreeRequested { Exit }`
→ loop: determine clean/dirty → keep/remove (prompt if dirty) → restore
`prior_cwd` → clear `active_worktree`.

## Relocation site — where the handler lives (resolves the "between turns" question)

The spec says relocation is "a between-turns session operation owned by the
agent loop." This section pins exactly where, because there are two candidate
sites with different semantics and only one is correct.

**Wrong site — in-turn, as an `Emitting` tool handler inside
`run_agent_turn_cancellable`.** This is where `dispatch_subagent` is handled
(`agent.rs:1190`). But `TurnConfig` is passed **by value** to the turn function
and consumed; mutating `config.cwd` mid-turn changes nothing for the *next*
turn. The handler can append a `ToolResult`, but it cannot durably relocate the
session.

**Correct site — the main `run()` loop (`main.rs:2172`), reacting to a new
`AgentUpdate` variant.** The flow:

1. `enter_worktree` / `exit_worktree` (Emitting tools) and `:worktree <name>` /
   `:worktree exit` (TUI commands) each send a new **`AgentUpdate::WorktreeRequested {
   action: WorktreeAction }`** to the UI channel. (`WorktreeAction` is an enum:
   `Enter { name }` / `Exit`.) The Emitting-tool handler in
   `run_agent_turn_cancellable` does only this send + a `ToolResult` echo —
   same shape as `dispatch_subagent`, which also sends `AgentUpdate::SubagentStarted`
   and then a result.
2. The main `run()` loop's `AgentUpdate` match gains a `WorktreeRequested`
   arm (alongside the existing `TurnComplete` / `SubagentStarted` arms at
   `main.rs:2625+`). **This arm** performs the relocation: validate, call
   `create_worktree` (or detach on exit), set `app.active_worktree`, and —
   critically — do nothing else. The cwd change takes effect on the *next*
   `spawn_turn(app)` call, because:
3. `spawn_turn` (which already reads `app` fields to build `turn_config`) gains
   one line after `chat_turn_config_with`: if `app.active_worktree` is `Some`,
   override `turn_config.cwd` to the worktree's absolute path. This is the seam
   — `TurnConfig.cwd` is built fresh each turn from `App` state, so a
   session-level field on `App` is how the new cwd reaches every subsequent
   turn and every tool call within it.

**Why this works:** `dispatch_subagent` computes the subagent's cwd *before*
spawning and passes it as a parameter. The Chat agent can't do that for itself
(there's no outer spawner), so instead it stores the intended cwd in `App` and
lets the next `spawn_turn` read it. No tool ever mutates a live `TurnConfig`;
the relocation is strictly between-turns.

## Signal type — `AgentUpdate`, not `EventKind` (resolves persistence question)

The relocation signals (`WorktreeRequested { Enter | Exit }`) are
**`AgentUpdate` variants, not `EventKind` variants.** This is a deliberate
decision, not a deferral:

- `EventKind` (`event.rs:70`) is **persisted to SQLite** — every variant is
  stored and replayed on session resume. Worktree relocation is inherently
  ephemeral: on a fresh process launch the worktree may have been removed, the
  process cwd is different, and re-entering a stale worktree would be
  surprising and possibly broken. Making relocation an `EventKind` would force
  the resume path to decide whether to re-enter (and handle the
  worktree-missing case), for no benefit.
- `AgentUpdate` (`agent.rs:168`) is **ephemeral** — an in-memory channel from
  the agent loop to the main `run()` loop, never persisted. A relocation is
  naturally lost on restart, which is the correct default: a resumed session
  starts in the process cwd ("."), same as every other session. The worktree
  dir and branch persist on disk (if kept), so the user can re-enter explicitly.
- The `dispatch_subagent` Emitting path appends a plain `ToolResult` event
  (not a custom `EventKind`) and communicates the side-effect via
  `AgentUpdate::SubagentStarted`. The worktree tools follow the same pattern:
  a `ToolResult` echo + an `AgentUpdate::WorktreeRequested` for the side-effect.

The `ToolResult` that the tool returns to the model IS persisted (it's a normal
`EventKind::ToolResult`), so the conversation transcript records "entered
worktree X" / "exited worktree" — just the *action* of relocating is
ephemeral. This matches how `dispatch_subagent` works: the result text is in
the transcript, but the subagent's existence is tracked via `AgentUpdate`.

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

Implementation note: introduce an explicit ownership-transfer method so the
session-held guard is **never dropped** while the worktree should persist, and
the subagent path's unconditional-drop cleanup is **unchanged**. Specifically:

```rust
impl WorktreeGuard {
    /// Consume the guard WITHOUT removing the worktree. Used by
    /// `WorktreeSession` to take ownership of a persistent worktree: the
    /// guard is moved into session state and held until an explicit exit
    /// decision, so its `Drop` never fires while the session owns it.
    /// Returns the (path, branch_name) for the session to remember.
    pub fn into_kept(self) -> (PathBuf, String) {
        let path = self.path.clone();
        let name = self.name.clone();
        std::mem::forget(self); // suppress Drop's removal
        (path, name)
    }

    /// Explicitly remove the worktree (dir + prune + branch). Called by the
    /// exit handler when the decision is "remove." This is the same logic
    /// `Drop` performs today, factored out so the exit path can call it
    /// directly.
    pub fn remove(self) {
        // identical to today's Drop body
        drop(self); // Drop runs the removal
    }
}
```

The default `Drop` behavior stays **unconditional removal** — exactly as today.
The subagent path (`spawn_subagent.rs:48`, `drop(wt)`) is completely
unaffected: it drops the guard, `Drop` removes the worktree, cleanup works as
before. **No default is flipped; no subagent behavior changes.**

The persistent-worktree path uses `into_kept()`: the guard is consumed (its
`Drop` suppressed via `mem::forget`), and the `(path, name)` is stored in
`WorktreeSession`. On exit, if the decision is "remove," the handler calls the
removal logic directly (dir + prune + branch delete, the same operations
`Drop` does). If "keep," it does nothing — the worktree + branch are already on
disk and unregistered-from-the-guard. This avoids the risk the Migration
section previously deferred: there is no "keep flag" whose default could leak
subagent worktrees, because the subagent path never calls `into_kept()`.

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
- `WorktreeGuard::into_kept()` consumes the guard without removing the
  worktree (Drop suppressed); the worktree persists on disk. `remove()` or
  plain `drop()` removes dir + branch (the subagent path uses `drop`).
- Enter sets the turn cwd to the worktree path; exit restores `prior_cwd`.
- Dirty worktree on exit triggers the keep/remove prompt; keep leaves worktree +
  branch; remove deletes them.
- Name collision enters the existing worktree (no error, no duplicate).
- Enter refused when already in a worktree (no nesting).
- Enter refused while a subagent is running.
- Not-a-git-repo → error.

Integration:

- Model `enter_worktree` tool → `AgentUpdate::WorktreeRequested` → loop
  relocates the session cwd.
- `:worktree <name>` command routes through the **same** loop handler as the
  tool.
- Enter → do work → exit round-trip restores the original cwd and (on remove)
  cleans up.

## Companion / `show` tool

If the companion HTTP server is running and publishing snapshots, relocating
cwd changes what it sees (file paths, the `git status` the background watcher
polls at `main.rs:2144`). No explicit notification is needed: the companion
reads from the shared `CompanionHub`, which is refreshed per-frame from `App`
state in the main `run()` loop. Once `app.active_worktree` is set and
`spawn_turn` picks up the new cwd, the next render cycle naturally reflects the
worktree's paths and git status in the published snapshot. The `show` tool is
Chat-only and never in the subagent registry; it renders the current frame, so
it picks up the relocated cwd the same way. No companion-side change required.

## Migration / compatibility

The existing `dispatch_subagent(worktree: true)` path is unchanged — it keeps
its ephemeral, drop-on-finish worktrees for subagent isolation. This feature
adds a *separate*, persistent, session-level worktree owned by the loop. The
`WorktreeGuard` changes (adding `into_kept()` / `remove()`, keeping `Drop` as
unconditional removal) do not affect the subagent path at all: subagents still
`drop(wt)`, `Drop` still removes, cleanup is identical. The `into_kept()` path
is only used by `WorktreeSession`, which only the Chat-agent relocation
handler creates.
