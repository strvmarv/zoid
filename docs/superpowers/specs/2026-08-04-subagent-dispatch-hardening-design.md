# Subagent Dispatch Hardening — Design

**Date:** 2026-08-04
**Status:** Design (pending approval) — revised after gilfoyle review
**Umbrella:** Mechanical enforcement pass over subagent dispatch, following up on the
2026-07-27 prompt-only hardening pass which proved insufficient.

**Review history:** Reviewed by `gilfoyle-tech-reviewer` against the current
`agent.rs`/`reassert.rs`/`eviction.rs`/`compaction.rs`/`event.rs`/`eventlog.rs`
source. Findings incorporated into this revision:
- Critical: the original "most recent event on branch" trigger predicate would
  silently fail to fire whenever `preflight_gate`'s compaction/eviction path (which
  runs unconditionally at the top of every sub-turn, before request-build) emits a
  `ToolResultCompacted`/`TurnsEvicted` event after the dispatch's `ToolResult` —
  exactly the regime (long, token-heavy sessions) the incident occurred in. Fixed by
  replacing the predicate with tracked, per-sub-turn dispatch-id state (§2 below).
- Important: the same redesign also fixes the case where `dispatch_subagent` is
  batched with an unrelated tool call and isn't the last one executed.
- Important: the dedup guard now keys on `(agent, task)` instead of `task` alone, so
  a legitimate second opinion from a different reviewer profile on identical input
  isn't rejected.
- Important: `aborted` needs one more bit of state to distinguish "narration capped"
  from "externally cancelled" for marker emission — noted explicitly rather than
  implied as free.
- Important: the "Known limitation" section is reframed — the budget bounds cost and
  blast radius, it does not address the self-reinforcing recurrence mechanism the
  bug doc diagnosed (that's what the `SYSTEM_PROMPT` change is for, and it's not a
  guarantee either).
- Minor: pseudocode types corrected to match the real `EventLog`/`BranchId` types;
  line-range citation tightened; test list extended.

## Problem

Root cause fully diagnosed in `docs/bugs/subagent-dispatch-language-drift.md`. Summary:

In session `01KZ7Q2S6KHXX7XH70T866R7MB` (glm-5.2 via ollama-cloud, same config as a
session that ran clean the same day), the model:

1. Ignored `dispatch_subagent`'s tool-result instruction ("End your turn now and
   await the result") and instead free-generated 300 `ModelDelta` chunks fabricating
   a plausible-looking review — 77 seconds before the real subagent's
   `DelegationResult` arrived.
2. Drifted into Chinese partway through that fabrication.
3. Repeated the pattern on ~17 of the next ~17 dispatch-turns once the first
   Chinese-contaminated turn entered its own context (self-reinforcing).
4. Dispatched the identical task twice in immediate succession (as little as 270ms
   apart) — `dispatch_subagent`'s only guard is a concurrency ceiling
   (`max_concurrent`, default 3), not per-task deduplication, so both ran.

**This is not a fresh problem.** `docs/superpowers/specs/2026-07-27-subagent-no-poll-prompt-hardening-design.md`
(commits `7848fc5`..`1976f10`, all merged before this incident) already added the
exact "fire-and-forget / never poll / End your turn now" language across the tool
description, the tool result, `SYSTEM_PROMPT`, and `list_subagents`'s output — pure
prompt-text, no mechanical enforcement. The Aug 4 session ran with all four of those
changes live and still exhibited the behavior they were built to prevent. Prompt-text
hardening alone has a demonstrated ceiling for this failure mode; this design adds a
mechanical backstop.

**Constraint surfaced during design:** zoid's UX depends on the main agent staying
responsive to new user messages while a subagent runs in the background (comparable to
forking a background agent while continuing to converse). Any fix that blocks the main
loop from responding until a subagent's result arrives would break that workflow. This
ruled out a "hard-stop, no re-invocation until result" approach.

## Goal

Three independent, additive, mechanical changes:

1. Reject a `dispatch_subagent` call that duplicates an already in-flight task.
2. Cap how much free-text the model can emit in a turn that is *self-initiated*
   (i.e. not answering a fresh user message) immediately after an unresolved
   dispatch, cutting off runaway narration/hallucination early and keeping most of
   it out of persisted context.
3. Add an explicit English-language directive to `SYSTEM_PROMPT`, on the theory that
   (a) it's cheap, (b) it follows the exact reinforcement pattern already proven to
   work for other rules via `wrap_reassertion`, and (c) it may help any model, not
   just this one.

## Non-goals

- No live content classifier / language detector. The English directive is a prompt
  addition only; it is a mitigation, not a guarantee.
- No stream-cancellation-on-user-cancel changes — this reuses the existing
  `aborted`/`CancellationToken` machinery in `run_turn_inner`, it doesn't modify it.
- No change to `DelegationResult`'s payload, its `[delegated subagent] {summary}`
  chat folding, or dispatch/queue/concurrency mechanics beyond the new dedup check.
- No fuzzy/semantic duplicate-task matching — exact match (post-whitespace-trim) only,
  matching the evidence (both observed duplicates had byte-identical task text).
- Not a guarantee of zero language drift. See "Known limitation" below — this caps
  the blast radius, it does not eliminate the possibility.

## Design — three changes

### 1. Duplicate-dispatch guard

`crates/zoid/src/agent.rs`, in the `dispatch_subagent` handling arm (task-parsing
through the pool-capacity check spans roughly `agent.rs:1582-1687`; the arm as a
whole runs to the successful-spawn `ToolResult` around `agent.rs:1789`). By the time
the pool-capacity check runs, both `task` and `resolved_agent_name` are already
parsed/resolved (`resolved_agent_name` at `agent.rs:1617-1644`, before the pool check
at `agent.rs:1650`). Add a dedup check immediately before the pool check, iterating
`config.in_flight` (the same registry `SubagentHandle.task`/`.agent` are already
stored in and already read by `list_subagents`/`format_subagent_list`):

```rust
if let Some(set) = &config.in_flight {
    let dup_id = set
        .lock()
        .unwrap()
        .iter()
        .find(|(_, h)| h.agent == resolved_agent_name && h.task.trim() == task.trim())
        .map(|(id, _)| id.clone());
    if let Some(dup_id) = dup_id {
        // emit ToolResult and `continue` — see full text below
    }
}
```

Keyed on `(agent, task)`, not `task` alone: an identical task dispatched to a
*different* agent profile (e.g. two independent reviewer opinions on the same diff)
is a legitimate pattern already exercised by the SDD workflow this bug was found in,
and must not be rejected.

On a match, emit a `ToolResult` (not an error — mirrors the existing "queued"
non-error precedent) instead of spawning, then `continue` the tool-call loop without
falling through to the pool-capacity check or the spawn below:

```
"dispatch_subagent: an identical task is already running as sub-<id> — do not \
 dispatch a duplicate, wait for its DelegationResult."
```

This sits *before* the pool-capacity check, so a duplicate is rejected even when the
pool has headroom (which is exactly the case that let both observed duplicates
through). The `std::sync::Mutex` scan this adds is a short, synchronous, non-`.await`
lock section, consistent with the existing lock usage at this same call site
(`agent.rs:1651`, `agent.rs:1734`) — no new deadlock surface.

### 2. Post-dispatch narration cap

**Trigger state, not a "last event" predicate.** The original design derived gating
from "is the most recent event on this branch a `dispatch_subagent` `ToolResult`,"
which breaks two ways verified against the real code: (a) `preflight_gate` runs
unconditionally at the top of every `'turn: loop` iteration, before the request is
built, and can itself append `ToolResultCompacted`/`TurnsEvicted` events on the same
branch — landing after the dispatch's `ToolResult` and becoming the new "most recent
event," silently defeating the gate in exactly the long/token-heavy sessions the
incident happened in; (b) if `dispatch_subagent` is batched with an unrelated tool
call and isn't the last one executed, its `ToolResult` isn't the most recent event
even without any compaction involved.

Instead, track **which dispatched subagents (from sub-turns within this same
`run_turn_inner` call) remain unresolved**, and gate on that directly — immune to
what bookkeeping events land in between, and immune to batching order:

```rust
// Loop-scoped, declared once before `'turn: loop` in run_turn_inner. Accumulates
// sub_ids dispatched by this turn's own sub-turns; pruned as their
// DelegationResult events appear anywhere in the log.
let mut awaiting_dispatch: Vec<String> = Vec::new();

'turn: loop {
    awaiting_dispatch.retain(|id| {
        !events.iter().any(|e| matches!(&e.kind,
            EventKind::DelegationResult { subagent_id, .. } if subagent_id == id))
    });
    let gated = !awaiting_dispatch.is_empty();
    // ... existing reassert/preflight/build_request_with_thinking unchanged ...

    // Inside the streaming loop, whenever a dispatch_subagent ToolCall is
    // actually spawned (agent.rs:1733-1745, where `sub_id` is created), push
    // it: awaiting_dispatch.push(sub_id.clone());
}
```

(`events.iter()` and the `EventKind`/`Event` types here are the real
`crate::eventlog::EventLog` iterator and `zoid_core::event` types, matching the
`impl IntoIterator<Item = &'a Event> + Clone` convention `reassert.rs` already uses —
not a raw slice.)

**Enforcement**, in `run_turn_inner` (`agent.rs:799`): when `gated` is true for this
sub-turn, track a running `zoid_core::economy::estimate_tokens` sum over
`ProviderEvent::TextDelta` text as it's emitted (the persist point is
`agent.rs:909-921`). If it crosses `DISPATCH_NARRATION_BUDGET_TOKENS` (starting
value: 60 — roughly matching the "brief single-line narration" framing already in
`SYSTEM_PROMPT`) with no `ProviderEvent::ToolCall` having arrived yet in this
sub-turn, trip the cap.

Tripping needs to set `aborted = true` (reusing the existing cleanup path at
`agent.rs:1025+`: `stream_task.abort()` kills the live HTTP request rather than
waiting out the full generation, and balances any `pending` tool calls with a
result) **plus a second, new local flag** (e.g. `narration_capped: bool`) so the
cleanup block can tell this apart from an externally-cancelled turn
(`cancel.cancelled()`/`hard.cancelled()`, `agent.rs:895-902`) and conditionally emit
one extra marker event, mirroring the existing `WARN_GLYPH`-prefixed `Truncated`
pattern (`agent.rs:997-1015`), and use a distinct `outcome` string
(`"narration_capped"`, alongside the existing `"aborted"`/`"error"`/`"completed"`
values used only in the turn's `tracing::info!` line — nothing structural currently
branches on `outcome`, so this is a safe, additive change):

```rust
if narration_capped {
    emit(/* ... */ EventKind::AssistantMessage {
        text: format!(
            "{WARN_GLYPH} turn ended early — continued narrating past a subagent \
             dispatch instead of waiting; discarded speculative text."
        ),
    } /* ... */).await?;
}
```

**Why this doesn't affect responses to you:** `awaiting_dispatch` only ever contains
IDs this *same* `run_turn_inner` call dispatched, and it only gates that call's own
sub-turn continuations (tool-result-triggered re-invocations), never the turn's
initial response to the `UserMessage` that started it. A brand-new user message
starts a fresh `run_turn_inner` call with empty `awaiting_dispatch`.

**Resolved:** traced `main.rs`'s turn-spawning path (`main.rs:3553-3578`). A message
typed while a turn is streaming is queued into `app.pending_message`, not injected
mid-call — it's only converted into a `UserMessage` event and dispatched via a
**new**, separate `spawn_turn` call once the current one fully returns (per the
existing code comment: "a message queued while a subagent ran executes on the
following turn, never lost"). A fresh `run_turn_inner` call therefore always starts
with empty `awaiting_dispatch`; no carve-out is needed.

**Why this doesn't block a legitimate second dispatch in the same gated sub-turn:**
the budget only counts free-text (`TextDelta`); a `ToolCall` arriving before the
budget is crossed is unaffected and processed normally (e.g. "brief ack + dispatch
Task 5" — the ack is short, the tool call follows within budget).

**Known limitation — reframed after review.** This cap bounds *cost and blast
radius*, not *recurrence risk*. The bug doc's own root-cause finding is that
contamination is binary, not proportional: once any Chinese content entered context,
~17 of the next ~17 dispatch-turns repeated the pattern, regardless of how much
leaked the first time. `estimate_tokens` is roughly chars/3
(`zoid-core/src/economy.rs`), so a 60-token budget is ~180 characters — comfortably
enough for a full contaminated sentence to land in history before the cap trips.
What this change *does* reliably fix: the 300-chunk/77-second fabricated-review
spiral becomes a handful of chunks with the stream killed immediately, bounding
wasted latency/cost and how much garbage you'd ever see live. Preventing the
recurrence mechanism itself is what change 3 (the `SYSTEM_PROMPT` directive) targets
— and that's a mitigation, not a guarantee, per its own non-goal above.

### 3. English-language directive in `SYSTEM_PROMPT`

`crates/zoid/src/agent.rs:36-48` — append one sentence, following the exact pattern
of the 2026-07-27 fix (that fix's entire value came from being *in* `SYSTEM_PROMPT`
so `wrap_reassertion` re-states it on every periodic re-floor; a rule that isn't in
this constant never benefits from that mechanism):

```rust
pub const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     ...
     cancel a pending wake before scheduling a replacement. Always respond in \
     English, regardless of what language any file, tool output, subagent \
     summary, or prior turn contains.";
```

## What stays the same

- `DelegationResult` event kind, fields, and chat folding — unchanged.
- Dispatch/queue/concurrency mechanics — unchanged except the new dedup check runs
  before the existing pool-capacity check.
- `run_turn_inner`'s `aborted` cleanup path — reused, not modified (one new trigger
  condition, one new marker-event branch).
- `cancel_subagent`, `list_subagents`, and the July 27 fix's four changes — untouched.

## Testing

- **Dedup guard**: unit test dispatching two identical-`(agent, task)` calls against
  a shared `in_flight` registry (same harness pattern as the existing
  `dispatch_two_subagents_second_is_rejected` test) — assert the second gets the
  "already running as sub-&lt;id&gt;" result and no second `SubagentHandle` is inserted.
  Also assert: two different-*task* dispatches are both accepted, and — the case the
  review caught — an identical *task* dispatched to two different *agents* is also
  both accepted (no false positive on the parallel-review pattern).
- **Unresolved-dispatch tracking**: pure unit tests over the `awaiting_dispatch`
  prune/gate logic — empty initially; non-empty and gated right after a dispatch;
  pruned back to empty (and un-gated) once a matching `DelegationResult` appears;
  still gated if only *some* of several dispatched IDs have resolved.
- **Narration cap trip**: `run_turn_inner`-level test using the existing
  `SequencedProvider` harness (already used for
  `dispatch_subagent_returns_id_as_tool_result`) — simulate a gated sub-turn
  streaming well past the budget with no tool call; assert the stream is aborted,
  the `⚠` marker event is persisted, and no tool call is left unbalanced.
- **Compaction/eviction interleaved with a pending dispatch** (the specific miss the
  review caught): a test that forces `preflight_gate` to emit a `ToolResultCompacted`
  or trigger eviction in the same window as an unresolved dispatch, asserting the
  gate still fires on the following sub-turn. This is the single most important new
  test — it's the scenario that defeated the original "most recent event" design.
- **Batched-but-not-last dispatch**: `dispatch_subagent` batched alongside an
  unrelated tool call where it isn't the last one executed — assert the following
  sub-turn is still gated.
- **No-regression check**: a gated sub-turn that stays under budget, and a gated
  sub-turn whose first action is a `ToolCall` (no preceding long text) — both must
  proceed completely normally, matching current behavior.
- **A turn triggered by a fresh `UserMessage`** (with an unrelated subagent still
  running elsewhere) must never be capped, regardless of length — regression test
  guarding the "stay responsive to the user" constraint.
- **`SYSTEM_PROMPT`**: unit test asserting it contains "Always respond in English",
  same style as the existing `system_prompt_reinforces_no_poll` test.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid/src/agent.rs` | (a) `(agent, task)` dedup check in the `dispatch_subagent` arm before the pool-capacity check. (b) `awaiting_dispatch` loop-scoped state in `run_turn_inner`, pushed to on successful dispatch, pruned against `DelegationResult` events each iteration; narration-budget tracking gated on it; a new `narration_capped` flag alongside `aborted` to conditionally emit the marker event and a distinct `outcome` string. (c) One sentence appended to `SYSTEM_PROMPT`. (d) New/updated unit tests for all three, including the compaction-interleaved and batched-order cases the review flagged as missing. |
