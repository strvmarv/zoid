# Subagent Reliability — Re-enable dispatch_subagent

> **Status:** design (ready for implementation planning). Resolves the issues
> that caused `dispatch_subagent` / `subagent_diff` to be disabled in commit
> `61ca909` ("model override 404 + worktree lifecycle"). The model-override-404
> issue is already fixed (`f9bf4db`); this spec addresses the remaining four
> gaps: branch lifecycle, iteration cap, concurrency guard, and success
> detection. Builds on the subagent machinery in `crates/zoid/src/subagent.rs`
> and the `WorktreeGuard` lifecycle design from
> `2026-07-08-chat-worktree-design.md`.

## Goal

Re-enable `dispatch_subagent` and `subagent_diff` in `chat_tools`, with safety
rails that prevent the four failure modes observed before the disable:
subagents that loop indefinitely, branches deleted before diff retrieval,
unguarded concurrent dispatch corrupting shared state, and silent successes
from subagents that produced no useful output.

## Background

The subagent runtime is built and integration-tested. `run_subagent` builds
constructed context (task + relevant files, never session history), runs an
isolated agent turn on a `subagent:<id>` branch, and distills a summary. The
Emitting handler in `run_agent_turn_cancellable` (`agent.rs:1190`) spawns it
fire-and-forget. `subagent_diff` retrieves the branch's commits for review.

The disable was triggered by two issues. The first — model override 404 — was
fixed the same day (`f9bf4db`: subagents inherit the session model, no `model`
param). The second — "worktree lifecycle" — was not. Three additional gaps
(lack of iteration cap, unguarded concurrency, brittle success detection) were
latent and would resurface on re-enable.

## Decisions (recap of the assessment)

- **Branch lifecycle (Gap 1):** decouple worktree-dir removal from branch
  retention. Reuse the `into_kept()` design from the chat-worktree spec. The
  subagent's commits persist on the branch after the worktree dir is removed,
  so `subagent_diff` can retrieve them. Cleanup is explicit, not automatic.
- **Iteration cap (Gap 2):** subagents get a tight cap (25), not the main
  loop's 1000. A confused subagent stops fast.
- **Concurrency (Gap 3):** v1 enforces sequential dispatch. `dispatch_subagent`
  refuses if any subagent is in flight. Parallel dispatch is deferred to a
  later phase (requires a solid event-ordering story for shared-session
  concurrent appends).
- **Success detection (Gap 4):** `ok` requires a non-empty summary AND no
  errored tool results in the subagent's final state. Empty/no-op → `ok =
  false`.

## Gap 1 — Branch lifecycle: keep commits for diff retrieval

### The problem

`WorktreeGuard::drop` (`worktree.rs:44–61`) unconditionally removes the
worktree dir, prunes the git registration, **and deletes the branch**. But
`subagent_diff` (`subagent_diff.rs:39–46`) needs the branch (`subagent:<id>`)
to exist to run `git rev-parse --verify` and `git log/diff`. When the subagent
finishes and its `WorktreeGuard` drops (in `spawn_subagent.rs:48`), the branch
is deleted — so the orchestrator's subsequent `subagent_diff` call fails with
"history not found — it may have been cleaned up."

### The fix

Reuse the `into_kept()` design from `2026-07-08-chat-worktree-design.md` §
Lifecycle. Add to `WorktreeGuard`:

```rust
impl WorktreeGuard {
    /// Consume the guard, removing the worktree DIR but KEEPING the branch
    /// (with the subagent's commits) so `subagent_diff` can retrieve it.
    /// Returns (path, branch_name) for the caller to remember for later
    /// cleanup. The worktree registration is pruned; the branch ref is not.
    pub fn into_kept_branch(self) -> (PathBuf, String) {
        let path = self.path.clone();
        let name = self.name.clone();
        let repo_root = self.repo_root.clone();
        // Remove the worktree dir + prune the registration, but do NOT delete
        // the branch. We suppress Drop (which would delete the branch) and do
        // the dir/prune ourselves.
        std::mem::forget(self);
        let _ = std::fs::remove_dir_all(&path);
        if let Ok(repo) = git2::Repository::open(&repo_root) {
            if let Ok(wt) = repo.find_worktree(&name) {
                let mut po = git2::WorktreePruneOptions::new();
                po.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut po));
            }
        }
        (path, name)
    }
}
```

`Drop` stays **unconditional full removal** (dir + prune + branch delete) —
unchanged for the subagent error/panic path and for any future caller that
wants full cleanup.

### Subagent spawn path change

`spawn_subagent.rs` currently does `drop(wt)` (line 48), which triggers full
`Drop`. Change it to call `into_kept_branch()` when the subagent succeeds, so
the branch is retained:

```rust
// In spawn_subagent, after run_subagent returns Ok(r):
if let Some(wt) = wt {
    let (_path, branch_name) = wt.into_kept_branch();
    // The branch ref persists. It's cleaned up later (see Cleanup).
    r.retained_branch = Some(branch_name);
}
```

On subagent **error** (the `Err` arm), `drop(wt)` runs full cleanup (dir +
branch) — an errored subagent's partial work is discarded, which is correct.

### Cleanup of retained branches

Retained branches accumulate. Cleanup is **orchestrator-driven**, not TTL-based:

- `subagent_diff` already handles the "branch gone" case gracefully (returns an
  error message, not a panic). So a missing branch is safe.
- Add a `cleanup_subagent_branch(subagent_id)` path (a new `Local` tool or a
  `:cleanup` TUI command) that the orchestrator calls after it has reviewed the
  diff and no longer needs the branch. This does `git branch -D subagent:<id>`.
- v1 can defer the explicit cleanup tool and rely on the existing
  `subagent_diff` "not found" graceful path + manual `git branch -D`. The
  branches are lightweight refs; they don't bloat the repo. A cleanup tool is
  a Phase 2 refinement.

## Gap 2 — Iteration cap: stop confused subagents fast

### The problem

`run_subagent` calls `run_agent_turn`, which uses `MAX_TOOL_ITERATIONS = 1000`
(`agent.rs:147`). A headless subagent with no human to interrupt can loop
hundreds of times on a failed tool call before hitting the cap — burning tokens
and time. This is the "confused agents" symptom.

### The fix

Add a subagent-specific iteration cap. The cleanest seam: `TurnConfig` already
threads per-turn settings; add an optional field:

```rust
pub struct TurnConfig {
    // ... existing fields ...
    /// Hard cap on tool-call sub-turns. The main chat loop uses
    /// MAX_TOOL_ITERATIONS (1000); subagents override this to a tighter bound
    /// so a confused headless agent stops fast. None = MAX_TOOL_ITERATIONS.
    pub max_iterations: Option<u32>,
}
```

`run_agent_turn_cancellable` changes its cap check (`agent.rs:768`) from:

```rust
if iterations > MAX_TOOL_ITERATIONS {
```

to:

```rust
let cap = config.max_iterations.unwrap_or(MAX_TOOL_ITERATIONS);
if iterations > cap {
```

`run_subagent` sets it in its `TurnConfig`:

```rust
let config = TurnConfig {
    // ... existing fields ...
    max_iterations: Some(SUBAGENT_MAX_ITERATIONS),
};
```

```rust
/// Hard cap on a subagent's tool-call iterations (spec §Gap 2). A headless
/// subagent with no human to interrupt must stop fast when confused.
const SUBAGENT_MAX_ITERATIONS: u32 = 25;
```

The main chat loop's `TurnConfig` (built in `spawn_turn`) leaves
`max_iterations` as `None` → unchanged behavior (1000).

When the cap trips, the existing cap path emits an `AssistantMessage` with
`"{WARN_GLYPH} tool-iteration limit reached"` and sets `outcome = "cap"`. The
subagent's summary distillation picks this up (it starts with the warn glyph)
→ `ok = false`. So a capped subagent correctly reports failure.

## Gap 3 — Concurrency: enforce sequential dispatch (v1)

### The problem

`dispatch_subagent` spawns via `tokio::spawn` and the main turn continues
immediately (fire-and-forget). The model can dispatch multiple subagents in one
turn. All share `session.clone()` and `ui.clone()`. Two `worktree: false`
subagents editing the same files corrupt each other; the shared-session event
appends interleave non-deterministically.

### The fix (v1: sequential)

Gate `dispatch_subagent` on `in_flight_subagents.is_empty()`. The Emitting
handler (`agent.rs:1190`) gains a guard at the top:

```rust
Some(zoid_tools::ToolKind::Emitting) if tc.name == "dispatch_subagent" => {
    // v1: sequential dispatch. Parallel dispatch (with worktree: true) is
    // deferred — it needs a solid event-ordering story for concurrent
    // shared-session appends.
    if !in_flight.is_empty() {
        emit(
            &session, &mut events, ui, &config.branch,
            EventKind::ToolResult {
                id: tc.id,
                name: tc.name,
                output: "dispatch_subagent: a subagent is already running. \
                         Wait for its DelegationResult before dispatching another."
                    .into(),
                is_error: true,
            },
            session_id, now,
        ).await?;
        continue;
    }
    // ... existing dispatch logic ...
}
```

Wait — the Emitting handler is inside `run_agent_turn_cancellable`, which
receives `events` (the in-memory event log) but not `in_flight_subagents`
(that's on `App`, in the main loop). The handler can't see the in-flight set
directly.

**Resolution:** the concurrency guard lives in the **main `run()` loop**, not
in the Emitting handler. The flow:

1. The Emitting handler dispatches as it does today (spawns the subagent,
   returns the ID immediately).
2. The spawned subagent sends `AgentUpdate::SubagentStarted { id, task }` (it
   already does — `agent.rs:1224`).
3. The main `run()` loop's `SubagentStarted` arm (`main.rs:2835`) pushes to
   `in_flight_subagents`.

The guard must be **before** the spawn. So the Emitting handler needs to know
whether a subagent is in flight. Two options:

- **(a)** Pass an `in_flight` count/sender into `run_agent_turn_cancellable` (a
  new parameter or on `TurnConfig`). The handler checks it before spawning.
- **(b)** Move the concurrency check to the main loop by having the Emitting
  handler send the dispatch *request* as an `AgentUpdate` and letting the main
  loop decide (like the chat-worktree `WorktreeRequested` pattern). This is
  cleaner but a bigger refactor.

**v1 uses (a):** add `in_flight_subagents: Arc<AtomicUsize>` (or a shared
`HashSet`) threaded into `run_agent_turn_cancellable` via `TurnConfig`. The
handler checks `in_flight.load() > 0` before spawning. This is minimal and
unblocks the sequential guard. (b) is the Phase 2 refactor when we want the
dispatch logic fully in the main loop.

The `:delegate` command's existing one-at-a-time check is unchanged (it checks
`in_flight_subagents` on `App` directly).

### Deferred (Phase 2): parallel dispatch

Parallel dispatch with `worktree: true` is safe at the *file* level (each
subagent in its own worktree), but the shared-session concurrent appends still
need an ordering guarantee for correct projection. Phase 2 addresses this once
the event-log concurrency story is solid. v1's sequential guard is the safe
default.

## Gap 4 — Success detection: require real output

### The problem

`run_subagent` distills `ok` as `!summary.starts_with(WARN_GLYPH)`
(`subagent.rs:214`), where `summary` is the last non-empty assistant text. If a
subagent's final turn is an empty assistant message (ended on a tool result, no
summary text), `summary` is empty and `ok` is `true` — a no-op subagent reports
success.

### The fix

Tighten the distillation in `run_subagent` (`subagent.rs:194–214`):

1. **Require a non-empty summary.** If no non-empty assistant text is found,
   `summary = "{WARN_GLYPH} subagent produced no output"` and `ok = false`.
2. **Check for errored tool results.** Scan the subagent's branch events for
   any `ToolResult { is_error: true }` in the final state. If any exist, append
   a note to the summary and set `ok = false`.

```rust
let summary = conversation(&branch_events)
    .iter()
    .rev()
    .find_map(|m| match m {
        ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
    .unwrap_or_else(|| format!("{WARN_GLYPH} subagent produced no output"));

let has_errors = branch_events.iter().any(|e| {
    matches!(&e.kind, EventKind::ToolResult { is_error: true, .. })
});

let ok = !summary.starts_with(WARN_GLYPH) && !has_errors;

let summary = if has_errors && !summary.starts_with(WARN_GLYPH) {
    format!("{summary}\n\n{WARN_GLYPH} one or more tool calls errored")
} else {
    summary
};
```

This makes `ok` trustworthy: the orchestrator can rely on it to decide whether
to retry, review, or proceed.

## Gap 5 — Re-enable both tools together

`subagent_diff` was disabled as collateral (it's a pure `Local` tool with no
dispatch dependency; its tests pass). Both tools are re-enabled together in
`chat_tools` (`invoke_skill.rs:100`): uncomment the two `tools.push` lines,
flip the test back to `chat_tools_includes_dispatch_and_diff`.

The `#[ignore]` attributes on the two agent.rs integration tests
(`dispatch_subagent_returns_id_as_tool_result`,
`dispatch_two_subagents_concurrently`) are removed once the sequential
concurrency guard (Gap 3) lands — the concurrent test (`dispatch_two_subagents`)
becomes a sequential-rejection test instead.

## Implementation surface (summary)

1. **`crates/zoid/src/worktree.rs`** — add `into_kept_branch()`. `Drop`
   unchanged. (Gap 1)
2. **`crates/zoid/src/spawn_subagent.rs`** — call `into_kept_branch()` on
   success, `drop(wt)` on error. Store the retained branch name. (Gap 1)
3. **`crates/zoid/src/agent.rs`** — `TurnConfig.max_iterations`; cap check uses
   it. (Gap 2)
4. **`crates/zoid/src/subagent.rs`** — set `max_iterations: Some(25)`; tighten
   distillation. (Gaps 2, 4)
5. **`crates/zoid/src/agent.rs`** — Emitting handler checks shared in-flight
   count before spawning. (Gap 3) — requires threading an `Arc<AtomicUsize>`
   or shared set into the turn.
6. **`crates/zoid/src/main.rs`** — wire the in-flight count to `TurnConfig` /
   the turn call. (Gap 3)
7. **`crates/zoid/src/invoke_skill.rs`** — uncomment the two `tools.push` lines;
   flip the test. (Gap 5)
8. **`crates/zoid/src/agent.rs`** — un-ignore the two integration tests; adapt
   the concurrent test to assert sequential rejection. (Gap 5)

No changes to `zoid-tools` (the tool definitions are correct as-is), `zoid-core`
(events/projection unchanged), or the skill files (SDD already references
`dispatch_subagent`).

## Testing

TDD, mirroring the existing `subagent_integration.rs` /
`delegation_integration.rs` density.

**Unit:**
- `into_kept_branch()` removes the worktree dir but the branch ref survives;
  `git rev-parse --verify subagent:<id>` succeeds after the call. `Drop`
  (error path) still deletes both.
- `TurnConfig.max_iterations = Some(25)` caps a looping provider at 25
  iterations; `None` falls back to 1000.
- Subagent distillation: empty assistant output → `ok = false`, summary
  contains the warn glyph. Errored tool result → `ok = false`, summary notes
  the error. Normal output → `ok = true`.

**Integration:**
- Dispatch a subagent that writes a file in a worktree → completes →
  `into_kept_branch` retains the branch → `subagent_diff` retrieves the diff
  successfully (no "history not found").
- Dispatch while another is in flight → second dispatch returns an error tool
  result ("already running"); only one subagent spawns.
- Subagent that hits the 25-iteration cap → `DelegationResult.ok = false`,
  summary contains "tool-iteration limit reached".
- Un-ignore `dispatch_subagent_returns_id_as_tool_result`; adapt
  `dispatch_two_subagents_concurrently` to assert the second is rejected.

## Non-goals

- Parallel dispatch with `worktree: true` (Phase 2 — needs event-ordering
  story for concurrent shared-session appends).
- A `cleanup_subagent_branch` tool / `:cleanup` command (Phase 2 — branches are
  lightweight refs; the `subagent_diff` "not found" path is graceful).
- Changes to context construction or eviction in `run_subagent`.
- Changes to the TUI subagents drawer (re-shown when the tools re-enable).
