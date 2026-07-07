# Subagent Dispatch Tool

**Date:** 2026-07-06

## Goal

Add a `dispatch_subagent` tool to zoid's tool registry so the model can spawn
subagents during an agent turn, and a `subagent_diff` tool to retrieve a
completed subagent's diff. This makes the subagent-driven-development skill
functional — the model can dispatch, collect results, and review without
requiring the user to type `:delegate` per task.

## Background

Zoid has `run_subagent()` in `crates/zoid/src/subagent.rs` — it builds
constructed context (task + relevant code, never session history), runs an
isolated agent turn, and returns a `SubagentResult` (branch, summary, ok).

Today, `run_subagent` is only reachable via the `:delegate` TUI command
(`start_delegation` in `main.rs`), which the *user* types. The model has no
tool to dispatch a subagent during a turn. The SDD skill instructs the model
to "dispatch an implementer subagent" using a `Subagent (general-purpose):`
template syntax borrowed from Claude Code — that mechanism doesn't exist in
zoid, so the entire SDD loop is non-functional from the model's side.

The `:delegate` command stays as-is for the user. The new tool gives the model
the same capability programmatically.

## Scope

**In scope:**
- `crates/zoid-tools/src/subagent_dispatch.rs` — the `dispatch_subagent` tool
- `crates/zoid-tools/src/subagent_diff.rs` — the `subagent_diff` tool
- `crates/zoid-tools/src/lib.rs` — add both to `registry()`
- `crates/zoid/src/agent.rs` — `Emitting` arm for `dispatch_subagent` (spawns
  the subagent, returns the ID immediately as a tool result)
- `crates/zoid/src/agent.rs` — `Local` handling for `subagent_diff` (runs git,
  returns the diff)
- `crates/zoid/src/main.rs` — replace the single `app.delegating: bool` with
  in-flight subagent tracking (count or set of IDs); add both tools to
  `chat_tools`
- `crates/zoid-core/src/event.rs` — add `subagent_id` to `DelegationResult`
  for correlation
- `crates/zoid/src/subagent.rs` — `run_subagent` returns the subagent ID for
  correlation
- `skills/subagent-driven-development/SKILL.md` — replace
  `Subagent (general-purpose):` references with `dispatch_subagent` calls;
  update the concurrency/red-flag guidance
- `skills/dispatching-parallel-agents/SKILL.md` — update the parallel-dispatch
  section to use `dispatch_subagent` with `worktree: true`

**Out of scope:**
- Interactive approval gating for subagent dispatch (AllowAll for v1)
- Subagent resource limits (memory, time, turn count) beyond the existing
  `SUBAGENT_MAX_TOKENS` and `SUBAGENT_CONTEXT_CEILING`
- A subagent-results panel in the TUI (results arrive as conversation cards
  via `DelegationResult` events, same as today)
- Changes to `run_subagent`'s context construction or eviction policy

## Tool 1: `dispatch_subagent`

### Interface

```
name: dispatch_subagent
kind: Emitting (executed by the agent loop, run() never called)
arguments:
  task (string, required) — the task description for the subagent
  worktree (boolean, optional, default false) — isolate in a git worktree
  model (string, optional) — model override; omitted = inherit session model
```

### Behavior

When the agent loop encounters a `dispatch_subagent` tool call:

1. Generate a subagent ID (`sub-<ULID>`).
2. If `worktree: true`, call `create_worktree` (same as `start_delegation`
   today). If it fails, surface a warning hint but proceed in the main tree
   (same fallback as `:delegate`). If `worktree: false`, use the process cwd.
3. Resolve the worktree path to an absolute path
   (`$(cd "$path" && pwd -P)`) before passing it as the subagent's `cwd`
   (the fix we just shipped in the skills; the runtime should do the same).
4. Spawn `run_subagent` via `tokio::spawn` (async — fire-and-forget).
5. Return a tool result immediately: `{"subagent_id": "sub-01HZ..."}`. The
   model knows the subagent is running and can continue its turn.
6. Record the subagent ID in the in-flight set.

When the subagent completes (the `tokio::spawn` task finishes), it appends a
`DelegationResult` event (same as `start_delegation` does today) with the
subagent ID included for correlation. The event arrives as a conversation
card.

### Concurrency

The model can dispatch multiple subagents in one turn — each `dispatch_subagent`
call spawns independently. The in-flight set tracks all running subagents.
The status hint shows "N subagents running…" while any are in flight.

`:delegate` (the user command) continues to enforce one-at-a-time via its
own `app.delegating` check — it checks the in-flight set and refuses if any
subagent is running.

### Isolation

When `worktree: true`: each subagent gets its own worktree at
`.zoid/worktrees/<subagent-id>`, on its own branch (`subagent:<id>`). The
`WorktreeGuard` drops when the spawned task completes, cleaning up the
worktree.

When `worktree: false`: the subagent runs in the process cwd. The model is
responsible for not dispatching two `worktree: false` subagents that edit the
same files.

### Subagent tool set

Subagents get the base registry (read, write, edit, search, shell,
update_tasks), filtered to exclude Interactive tools — same as today. They
do NOT get `dispatch_subagent` (subagents can't spawn subagents), `invoke_skill`,
`recall`, `show`, or `ask_user`.

## Tool 2: `subagent_diff`

### Interface

```
name: subagent_diff
kind: Local (synchronous — runs git, returns text)
arguments:
  subagent_id (string, required) — the ID returned by dispatch_subagent
```

### Behavior

1. Parse the subagent ID to find the branch name (`subagent:<id>`).
2. Run `git log --oneline` + `git diff -U10` for the branch's commits, or
   `git diff <merge-base>..<branch-head>` if the branch is still reachable.
   Runs in the main repo's cwd (not the worktree, which may be gone).
3. Return the diff as text (commit list, stat summary, full diff — same
   format as `scripts/review-package`).
4. If the branch is gone (worktree cleaned up, branch deleted): return an
   error: "subagent <id> history not found — it may have been cleaned up."

### Use in SDD

After a `DelegationResult` event arrives, the model calls `subagent_diff`
to get the diff, then applies the task-reviewer rubric (or dispatches a
gilfoyle reviewer via another `dispatch_subagent` call with a review task).

## Event Changes

### `DelegationResult`

Add `subagent_id: String` to the `DelegationResult` event kind:

```rust
DelegationResult {
    subagent_id: String,  // NEW — "sub-01HZ..." for correlation
    branch: String,       // "subagent:01HZ..." (unchanged)
    summary: String,
    ok: bool,
}
```

The model correlates: the `dispatch_subagent` tool result returned
`{"subagent_id": "sub-01HZ..."}`, and the `DelegationResult` event carries
the same ID. The `branch` field already contains the ULID but in a different
prefix; the `subagent_id` makes correlation explicit and format-independent.

### `run_subagent` return

`SubagentResult` gains an `id` field:

```rust
pub struct SubagentResult {
    pub id: String,      // NEW
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}
```

`start_delegation` (the `:delegate` path) populates `id` with the same ULID
it uses for the branch, so both paths produce identical events.

## In-Flight Tracking

Replace `app.delegating: bool` with a set of in-flight subagent IDs:

```rust
struct App {
    // ...
    // Was: delegating: bool,
    // Now:
    in_flight_subagents: std::collections::HashSet<String>,
}
```

- `dispatch_subagent` tool arm: insert the ID before spawning, remove it when
  the spawned task completes.
- `:delegate` (`start_delegation`): checks if the set is non-empty; if so,
  refuses with "busy · subagents running". This preserves :delegate's
  one-at-a-time behavior.
- Status hint: `"{n} subagents running…"` when the set is non-empty.
- `app.shell.busy` (the spinner): true while the set is non-empty OR while
  streaming — same condition as today, just generalized.

## Skill Updates

### `subagent-driven-development/SKILL.md`

- Process flowchart: replace "Dispatch implementer subagent
  (./implementer-prompt.md)" with "Dispatch implementer via
  `dispatch_subagent` tool". Same for the reviewer and final-reviewer nodes.
- File Handoffs: the controller dispatches via `dispatch_subagent` with the
  task brief path in the `task` argument; the subagent's summary arrives as a
  `DelegationResult` event; the controller calls `subagent_diff` to get the
  diff for review.
- Red Flags: remove "Never dispatch multiple implementation subagents in
  parallel (conflicts)". Replace with: "Subagents editing the same files
  will conflict. Use `worktree: true` for isolation, or dispatch sequentially
  for tasks touching shared files."
- Example Workflow: rewrite to show `dispatch_subagent` tool calls and
  `subagent_diff` for review, instead of `Subagent (general-purpose):`
  template syntax.

### `dispatching-parallel-agents/SKILL.md`

- "Dispatch in Parallel" section: replace the template syntax with
  multiple `dispatch_subagent` calls in one turn, each with `worktree: true`.
- "Review and Integrate" section: collect `DelegationResult` events as they
  arrive, call `subagent_diff` for each, verify no conflicts.

## Testing

### Unit Tests

- `dispatch_subagent` tool: spec has name `dispatch_subagent`, `task` is
  required, `worktree` defaults to false, `model` is optional, kind is
  `Emitting`.
- `subagent_diff` tool: spec has name `subagent_diff`, `subagent_id` is
  required, kind is `Local`.
- In-flight tracking: inserting/removing IDs from the set; `:delegate`
  refuses when the set is non-empty; status hint shows the count.

### Integration Tests

- Dispatch a subagent with `FakeProvider` returning a known summary →
  `DelegationResult` event arrives with matching `subagent_id`, `branch`,
  `summary`, `ok`.
- Dispatch two concurrently → two distinct `DelegationResult` events with
  distinct IDs and branches.
- `subagent_diff` returns the diff for a completed subagent's branch;
  returns an error for a non-existent ID.

### Manual Smoke Test

In the zoid TUI with a real provider: run SDD on a small 2-task plan. The
model calls `dispatch_subagent` for Task 1 (worktree: true), the subagent
runs, the result arrives as a card, the model calls `subagent_diff` and
reviews. Dispatch Task 2 concurrently. Both results arrive. No manual
`:delegate` needed.

### Skill Verification

After skill updates: `:mode reload`, verify SDD references
`dispatch_subagent` (not `Subagent (general-purpose):`). Invoke
`invoke_skill("subagent-driven-development")` and confirm the body
references the tool.

## Non-Goals

- No interactive approval gating for subagent dispatch (AllowAll for v1).
- No subagent resource limits beyond existing token/ceiling caps.
- No TUI subagent-results panel (results are conversation cards).
- No changes to context construction or eviction in `run_subagent`.
- No nested subagent dispatch (subagents can't call `dispatch_subagent`).