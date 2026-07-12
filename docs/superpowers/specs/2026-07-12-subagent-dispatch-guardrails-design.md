# Subagent Dispatch Safety — Umbrella Index

**Status:** Decomposed 2026-07-12 into three sibling specs (they share almost
nothing at runtime; the seam is the `WakeTimer` primitive, which lives entirely
in Spec 1). Build order 1 → 2 → 3.

| Spec | Scope | Status |
|------|-------|--------|
| **1** | Subagent runtime guardrails — timeout, kill tool, Esc, `[subagent]` config → [`2026-07-12-subagent-guardrails-design.md`](2026-07-12-subagent-guardrails-design.md) | Design written |
| **2** | Worktree tooling correctness — **WT-1 / WT-2** (git/cwd) + **WT-3** (rail reflects worktree) + **WT-4** (drop bottom hint) → [`2026-07-12-worktree-tooling-fixes-design.md`](2026-07-12-worktree-tooling-fixes-design.md) | Design written |
| **3** | Scheduled wake-ups — agent self-scheduling subsystem (new `EventKind`s + watcher + synthetic-turn injection + `schedule_wake`/`cancel_wake` tools) → [`2026-07-12-scheduled-wakeups-design.md`](2026-07-12-scheduled-wakeups-design.md) | Design written |

The sections below are the original observations, retained as raw material for
Specs 2 and 3. Note: the two "spin-out" incidents (#1, #2) were most likely
observed in the *Claude Code* harness, not zoid (zoid caps subagents at 25
iterations, has no `gilfoyle` agent) — but the *gaps* they point at are real in
zoid, which is what Spec 1 addresses.

---

**Original status:** Idea — noted 2026-07-12, not yet specced. Circle back after opencode-zen ships.

## Observed failures (this session)

### 1. The 26-minute spin-out
A plan-writing subagent (`sub-01KXA67J64KRFRFR1PZCT6GT47`) ran for 26 minutes and 1000 tool iterations without writing a single line of output. It was only stopped by the hard 1k tool-iteration limit. The code's `SUBAGENT_MAX_ITERATIONS = 25` (subagent.rs line 29) was apparently not the binding constraint — either it doesn't apply to this dispatch path, or the 1k is a separate runtime limit. **Cost:** ~26 min of model time + API spend on a task that produced nothing.

### 2. The DelegationResult delivery gap
The gilfoyle-tech-reviewer subagent (`sub-01KXA83QGZZ1A3ZNYKCZ9WBHEF`) completed and produced a full review, but the result came back as a bare `[delegated subagent]` message instead of properly firing the main loop's `DelegationResult` handler. The subagent hit `⚠ one or more tool calls errored` near the end — likely the output was large enough to cause a tool error mid-stream, and the `DelegationResult` event was malformed or never emitted. **Cost:** the orchestrator didn't know the subagent finished.

## Candidate guardrails (to brainstorm)

1. **Wall-clock timeout per subagent** (e.g., 5 min default, configurable). Kills a spinning subagent and emits a failure `DelegationResult` instead of burning unbounded time.

2. **Read-to-write ratio heuristic.** If a subagent has only read and never written/edited after N iterations, abort with a "produced no output" error (mirrors `distill`'s empty-summary failure path).

3. **Prompt-level budget hint.** Inject "you have a limited action budget; produce your deliverable early" into the dispatch prompt.

4. **Output-size awareness.** Investigate whether large subagent outputs cause the DelegationResult delivery failure (failure #2). If so, cap/summarize before emission.

5. **Kill capability for the orchestrator.** There is currently no way for the orchestrating agent (or user) to kill/cancel a running subagent mid-flight. A subagent that's spinning (failure #1) or producing an unexpectedly large output (failure #2) runs to completion or until the hard iteration cap — there's no interactive stop. Add a kill signal (CancellationToken or similar) threaded into `spawn_subagent`, surfaced as a user command or agent-initiated abort, that cancels the subagent's active turn and emits a failure `DelegationResult`.

## Investigation needed

- Why did `SUBAGENT_MAX_ITERATIONS = 25` not catch the 26-min spin-out? Is the 1k limit a different layer? Check the dispatch path.
- What exactly causes the DelegationResult delivery gap? Reproduce with a known-large output.

## Follow-up TODOs (separate from guardrails design)

- **Output token cap truncation (recurring tooling bug).** When a single tool response or assistant turn exceeds the output token limit, the response is silently truncated (e.g., "⚠ response truncated — hit the output token cap"). This has happened multiple times this session and earlier. File as a zoid bug report (strvmarv/zoid-releases): the truncation should either (a) auto-continue/paginate, (b) surface a clear error to the orchestrator so it can retry with a smaller scope, or (c) dispatch to a subagent that has its own output budget. Currently the only workaround is to dispatch subagents for large outputs.

- **DelegationResult not firing main loop on subagent completion.** The gilfoyle reviewer subagent completed and produced output, but the result arrived as a bare message and did not trigger the main loop's DelegationResult handler. Reproduce, root-cause, and fix the event-delivery path. May be related to large-output tool errors near end-of-run.

## Worktree tooling bugs (observed 2026-07-12)

Two critical bugs in the `enter_worktree` / `exit_worktree` tooling that render worktrees completely useless for isolation:

### WT-1: `enter_worktree` does not create an isolated branch — commits land on the parent branch
`enter_worktree("opencode-zen-impl")` changed the CWD to `.zoid/worktrees/opencode-zen-impl` and created the worktree + branch, but all subsequent commits went to `main` (the parent branch), not to `opencode-zen-impl`. The worktree's HEAD pointed at the pre-implementation commit while `main` advanced with all 6 implementation commits. `git worktree list` showed the worktree stuck at the old commit while `main` moved forward. **Root cause to investigate:** the worktree is created but the tool doesn't switch the shell's git context to the new branch — the shell process continues operating on the parent's `.git` refs. The worktree dir is a CWD change, not a git-context change. This makes worktrees useless for parallel work (the entire point of using one here was isolation from the parallel agent).

### WT-2: `exit_worktree` orphans the shell CWD — all tools break
After `exit_worktree`, the shell tool's process CWD was left pointing at the deleted worktree directory. Every subsequent `shell`, `read`, `write`, `edit`, and `grep` call failed with `No such file or directory (os error 2)` because the process CWD didn't exist. Even `cd /home/gomanjoe/source/zoid` failed — the shell can't resolve the `cd` target because the process-level CWD is already invalid (the kernel can't resolve relative paths from a deleted directory). Only absolute-path file reads survived. The shell tool was permanently broken for the rest of the session.

**Fix:** `exit_worktree` must restore the parent CWD (via an absolute path) BEFORE deleting the worktree directory, or the shell tool must chdir to an absolute path on every invocation (defensive). The current implementation appears to delete first, then try to restore — backwards.
