# Subagent Dispatch Safety / Runtime Guardrails

**Status:** Idea — noted 2026-07-12, not yet specced. Circle back after opencode-zen ships.

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

## Investigation needed

- Why did `SUBAGENT_MAX_ITERATIONS = 25` not catch the 26-min spin-out? Is the 1k limit a different layer? Check the dispatch path.
- What exactly causes the DelegationResult delivery gap? Reproduce with a known-large output.

## Follow-up TODOs (separate from guardrails design)

- **Output token cap truncation (recurring tooling bug).** When a single tool response or assistant turn exceeds the output token limit, the response is silently truncated (e.g., "⚠ response truncated — hit the output token cap"). This has happened multiple times this session and earlier. File as a zoid bug report (strvmarv/zoid-releases): the truncation should either (a) auto-continue/paginate, (b) surface a clear error to the orchestrator so it can retry with a smaller scope, or (c) dispatch to a subagent that has its own output budget. Currently the only workaround is to dispatch subagents for large outputs.

- **DelegationResult not firing main loop on subagent completion.** The gilfoyle reviewer subagent completed and produced output, but the result arrived as a bare message and did not trigger the main loop's DelegationResult handler. Reproduce, root-cause, and fix the event-delivery path. May be related to large-output tool errors near end-of-run.
