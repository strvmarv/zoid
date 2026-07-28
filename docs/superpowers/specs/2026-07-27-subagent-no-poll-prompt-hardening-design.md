# Subagent No-Poll Prompt Hardening — Design

**Date:** 2026-07-27
**Status:** Design (approved)
**Umbrella:** Prompt-engineering pass over subagent dispatch tooling.

## Problem

The LLM repeatedly ignores the "do not poll / do not micro-manage subagents"
guidance. It calls `list_subagents` to check on dispatched subagents, or
attempts other status-checking behavior, instead of ending its turn and
awaiting the `DelegationResult` event.

This is an instruction-following problem with four root causes, all confirmed
in the current code:

1. **The prohibition is buried mid-paragraph in the tool description.**
   `subagent_dispatch.rs:15-22` is one run-on sentence where "Do NOT poll for
   completion" is a mid-description clause, not a leading rule. Models weight
   early/leading content; a mid-paragraph negative is low-salience.

2. **The tool result gives an affordance with no counter-instruction.**
   `dispatch_subagent` returns `{"subagent_id": "sub-01HZ..."}` — an ID and
   nothing else. The model sees "a thing I started that has an ID" and its
   dominant prior is "check on things I started." The result gives no positive
   directive on what to do instead (e.g. "end your turn").

3. **The re-assertion mechanism never carries the no-poll rule.**
   `wrap_reassertion` (`agent.rs:49-57`) periodically re-states the base
   `SYSTEM_PROMPT` (`agent.rs:36-43`), which contains **zero subagent
   guidance**. So the built-in periodic reinforcement system never touches
   this rule.

4. **Polling is behaviorally rewarded.** `list_subagents` succeeds and returns
   useful data; nothing in the runtime signals that polling is wrong. The model
   gets a "useful" answer, which reinforces the behavior the prompt forbids.

## Goal

Solidify the no-poll discipline through four mutually-reinforcing changes that
address salience (tool description), the critical-moment affordance (tool
result), periodic reinforcement (system prompt), and the behavioral reward
loop (list_subagents nudge). No runtime/architecture change — purely prompt +
tool-result text.

## Non-goals

- No config flags or gating — the hardening is unconditional and universal.
- No hard refusal from `list_subagents` (decided: return data + reminder, not
  an error). A refusal risks the model treating it as a failure and retrying or
  derailing.
- No change to the `DelegationResult` event payload or its `[delegated subagent]
  {summary}` chat folding (`agent.rs:502-508`) — that's a post-completion
  message; the problem is pre-completion polling, not post-completion wording.
- No change to the dispatch/queue/concurrency mechanics.

## Design — four changes

### 1. Restructure the `dispatch_subagent` tool description (salience)

`crates/zoid-tools/src/subagent_dispatch.rs:15-22` — lead with the behavioral
rule, not the mechanism. Reframe the tool's *nature* as fire-and-forget so the
prohibition is structural, not a parenthetical.

**Current** (one run-on, rule buried mid-sentence):

> Dispatch a subagent to execute a task in isolation. Returns the subagent's
> ID immediately; the result arrives later as a DelegationResult event. Up to
> max_concurrent subagents (default 3) may run simultaneously — additional
> dispatches are queued and start when a slot frees. Do NOT poll for completion
> or edit files in the main worktree while a subagent is running (they share
> the working directory unless worktree: true). Wait for the DelegationResult
> event. Use worktree: true for file isolation when subagents might edit the
> same files.

**New** (rule first, "fire-and-forget" framing, mechanism after):

> Fire-and-forget: dispatch a subagent to execute a task in isolation, then
> STOP. The result arrives later as a DelegationResult event that re-invokes
> you automatically — never poll for status, never call list_subagents to
> check progress, and do not edit files in the main worktree while a subagent
> runs (they share the working directory unless worktree: true). Returns the
> subagent ID immediately. Up to max_concurrent subagents (default 3) may run
> simultaneously — additional dispatches are queued and start when a slot
> frees. Use worktree: true for file isolation when subagents might edit the
> same files.

### 2. Inject a positive directive into the `dispatch_subagent` tool result (critical-moment affordance)

`agent.rs:1740` — the tool result that returns to the model immediately after
dispatch. Currently bare JSON:

```rust
output: format!("{{\"subagent_id\": \"{sub_id}\"}}"),
```

**New** — JSON prefix preserved (some models/tests may parse it) followed by an
explicit positive directive:

```rust
output: format!(
    "{{\"subagent_id\": \"{sub_id}\"}} — Subagent {sub_id} is running in isolation. \
     You will be re-invoked automatically with its result; do NOT call \
     list_subagents or otherwise check on it. End your turn now and await the result."
),
```

This hits at the exact moment the "I have an ID, let me check" affordance fires,
and gives the positive action ("end your turn now") instead of only a negative.

The existing test `dispatch_subagent_returns_id_as_tool_result`
(`agent.rs:4794`) asserts the result `contains` the subagent ID — still
satisfied (the ID appears both in the JSON and the prose). Update the assertion
to also check the directive is present.

### 3. Add subagent discipline to `SYSTEM_PROMPT` (periodic reinforcement)

`agent.rs:36-43` — append one sentence so `wrap_reassertion` carries the rule
on every periodic re-statement:

> ...Don't restate what the tool calls and diffs already showed. Subagents are
> fire-and-forget: dispatch, then end your turn and await the DelegationResult
> event — never poll for status or call list_subagents to check on a subagent
> you dispatched.

This is the only change that makes the existing re-assertion mechanism
(`wrap_reassertion`, `agent.rs:49-57`) actually reinforce the no-poll rule.
Without it, the periodic nudge repeats a prompt with no subagent content.

### 4. Append a soft reminder to `list_subagents` output when subagents are running (behavioral reward loop)

`agent.rs:2016-2021` — when the registry is non-empty, append a one-line
reminder to the existing data output. **Always** (decided: simplest, harmless
even when the user explicitly asked "what's running?").

**Current:**

```
Running subagents (1):
- sub-01HZ... [delegate]: fix the tests
```

**New:**

```
Running subagents (1):
- sub-01HZ... [delegate]: fix the tests

Reminder: subagents are fire-and-forget. You will be re-invoked with each
result automatically — do not poll or call this tool repeatedly to check
progress. End your turn and await the DelegationResult.
```

The reminder is appended only when subagents are running (the non-empty branch).
The empty case (`"No subagents currently running."`) is unchanged — no
reminder needed when nothing is in flight.

This converts "polling returns useful data and feels rewarding" into "polling
returns data + a reprimand," weakening the reinforcement loop without breaking
the model's flow or causing retry storms.

## What stays the same

- `DelegationResult` event kind, its fields, and its `[delegated subagent]
  {summary}` folding into chat — unchanged.
- `wrap_reassertion` mechanics — unchanged; it already re-states
  `SYSTEM_PROMPT` verbatim, so adding the sentence to `SYSTEM_PROMPT` is the
  only wiring needed.
- Dispatch, queue, concurrency, worktree, guardrail, and kill mechanics —
  unchanged.
- `cancel_subagent` tool — unchanged (canceling is a legitimate orchestrator
  action, not polling).

## Testing

- **`dispatch_subagent` tool-result text** (`agent.rs` test
  `dispatch_subagent_returns_id_as_tool_result`): assert the result still
  contains the subagent ID **and** now contains the directive ("do NOT call
  list_subagents" / "End your turn"). Update the existing assertion.
- **`dispatch_subagent` spec test** (`subagent_dispatch.rs`): no functional
  change to the spec struct, but update the description-string assertion if any
  exists (currently none checks the description prose — the spec test checks
  name/params/kind only). Add an assertion that the description leads with
  "Fire-and-forget" so a future edit can't silently re-bury the rule.
- **`list_subagents` output** (`agent.rs` test `list_subagents_formats_id_and_task`):
  extend to assert the reminder line is present when the map is non-empty, and
  absent in the empty case.
- **`SYSTEM_PROMPT`**: add a unit test asserting it contains "fire-and-forget"
  and "never poll" so a future edit can't drop the reinforcement sentence.

No integration tests needed — all four changes are string-content changes
verified by unit tests on the exact strings the model receives.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid-tools/src/subagent_dispatch.rs` | Restructure tool description: lead with "Fire-and-forget" + rule (change 1). Add spec-test assertion that the description leads with "Fire-and-forget". |
| `crates/zoid/src/agent.rs` | (a) Append subagent discipline sentence to `SYSTEM_PROMPT` (change 3). (b) Inject positive directive into the `dispatch_subagent` tool result at `:1740` (change 2). (c) Append reminder to `list_subagents` non-empty output at `:2016-2021` (change 4). (d) Update/extend the three affected unit tests. |