## Bug: Subagent-heavy sessions can spiral — model narrates over dispatch, drifts to Chinese, duplicates dispatches

### Symptom

In sessions that run the `subagent-driven-development` SDD workflow (many
back-to-back `dispatch_subagent` calls), the model can:

1. Ignore the `dispatch_subagent` tool result's instruction to end its turn,
   and instead keep generating a fabricated/hallucinated prediction of what
   the subagent will report — sometimes tens of seconds before the real
   result arrives.
2. Have that fabricated continuation drift into Chinese mid-sentence.
3. Once one turn drifts, keep repeating the pattern on most subsequent
   subagent-dispatch turns for the rest of the session (self-reinforcing).
4. Dispatch the same task twice in immediate succession (as little as 270ms
   apart), burning two subagent runs on identical work.

Reported by the user as: "over-complicating subagent usage" and "switched to
Chinese multiple times after asking to use English." User's own words
mid-session (session transcript, ts `1785895471784`): *"you were
micro-managing subagents again, not letting them finish their work,
duplicating their dispatch, etc. I thought we fixed—"*. The behavior recurred
even after that correction and a second explicit "switch back to english
please" (ts `1785895716600`).

### Sessions compared

Data source: `~/.local/share/zoid/zoid.db`, `events` table (event-sourced —
`kind` is the full serialized `EventKind`, `branch` distinguishes `main` from
each `subagent:<id>` sub-conversation).

- **Bad session**: `01KZ7Q2S6KHXX7XH70T866R7MB` — "let's review docs/TODO.md
  items" (2026-08-04 20:02–21:03).
- **Good session**: `01KZ7CZ52QZWD45753ADXSD3W0` — "let's point my local zoid
  at local ollama" (2026-08-04 17:04).
- Both ran the identical config: `model = "glm-5.2"`, `provider =
  "ollama-cloud"` (`~/.config/zoid/config.toml`, unchanged across both — this
  is model non-determinism, not a config regression).

### Root cause investigation (confirmed)

**Ruled out: two concurrent sessions.** Queried `sessions` table for any
other session with `created_ts`/`last_touched_ts` overlapping the incident
window (20:02–21:03) — none found. Only one session was active. Not the
cause.

**Confirmed: `dispatch_subagent`'s "end turn" contract is prompt-only, not
enforced.** The tool result the model receives after dispatching
(`crates/zoid/src/agent.rs:1778-1783`) reads:

> `{"subagent_id": "..."}` — Subagent ... is running in isolation. You will
> be re-invoked automatically with its result; do NOT call list_subagents or
> otherwise check on it. **End your turn now and await the result.**

There is no code-level backstop behind this sentence — no forced stop
sequence, no truncation/discard of assistant text generated after the
dispatch, nothing that checks the next turn is short/empty before allowing
more generation. It is pure natural-language instruction-following.

**Confirmed: exact turn where the bad session tipped over.** At ts
`1785894221239` (~99.6K input tokens into the session — no compaction, no
eviction, nothing structurally unusual; growth up to that point was smooth,
95K→99.6K over ~8 minutes), the model dispatched a `gilfoyle` review
subagent, received the "end your turn now" tool result, and then emitted 300
more `ModelDelta` chunks on the `main` branch fabricating a full review
(`### 规范合规性 (Spec Compliance)... ✅ 符合规范...`) — **77 seconds before**
the real subagent's `DelegationResult` came back (ts `1785894300591`, in
English, with genuinely different content/structure). The fabricated content
closely mirrors the shape of the eventual real review, suggesting the model
is predicting/role-playing the expected tool output rather than waiting for
it.

**Confirmed: self-reinforcing drift, not a one-off.** Checked all 22
`dispatch_subagent` calls in the bad session for CJK content in the
following ~15s of `main`-branch `ModelDelta` text:

```
dispatch #1-5  (ts < 1785893745000): 0 turns with CJK content
dispatch #6-22 (ts >= 1785894221239): 17 of ~17 turns with CJK content
```

Once the first CJK-contaminated turn entered the conversation history, most
subsequent subagent-dispatch turns repeated the pattern — a context-poisoning
feedback loop: the model imitates its own prior "voice" once that voice is
part of its context.

The existing `DirectiveReasserted` re-floor mechanism
(`crates/zoid-core/src/reassert.rs` — periodically re-injects pinned
directives every N cumulative tokens) had already fired once before the
onset (estimated cumulative ~129,919 tokens, ts `1785893505590`) and did not
prevent the drift. The user's own explicit mid-session correction
("switch back to english and review our conversation...") also did not hold
— the model relapsed again minutes later and ignored a second correction.

**Confirmed: duplicate dispatch is a real gap, not a description.**
`dispatch_subagent`'s only guard is a concurrency ceiling
(`"subagent queued (3 running, max 3)"`, see `agent.rs` around the pool
check) — there is no per-task deduplication. In the bad session:

```
1785894434702  gilfoyle review   -> sub-01KZ7SMY3FGYFKWCNQS6J2ZK43
1785894446002  gilfoyle review   -> sub-01KZ7SN90V8BSBQ64JPDM09NVC   (12s later, same task text)
1785894469905  delegate Task 4   -> sub-01KZ7SP0BHYZMG5VKNBEHZS4GG
1785894470175  delegate Task 4   -> sub-01KZ7SP0KV9CQ5RN3G71ZTHM96   (270ms later, same task text)
```

Both dispatches in each pair got distinct subagent IDs and both actually ran
(the pool had headroom); only the 4th-in-flight attempt at ts `1785894536825`
got throttled to `"subagent queued (3 running, max 3)"`. The model dispatched
identical task text twice, and zoid accepted both.

### What's been checked

1. Config parity between good/bad sessions (identical model+provider). OK —
   not a config regression.
2. Concurrent-session overlap in `sessions` table. OK — ruled out.
3. `Usage` event token trajectory around the drift point, filtered to
   `branch='main'` (an earlier pass mixed subagent-branch and main-branch
   `Usage` rows and produced a false "3x token jump" — corrected by
   filtering on `branch`). OK — growth was smooth, no compaction/eviction
   trigger.
4. `dispatch_subagent`'s tool-result text and the code path that emits it
   (`agent.rs:1770-1789`). OK — confirmed prompt-only enforcement.
5. Frequency/onset of CJK content across all 22 dispatch points in the bad
   session, and their `branch` attribution (confirmed `main`, not leaked
   subagent-branch or `ThinkingDelta` content — `ollama.rs`'s `thinking` vs
   `content` field parsing is correctly separated and well-tested, so this
   is genuine model-generated visible-answer text, not a reasoning-trace
   leak). OK.
6. Duplicate-dispatch pairs' tool results, to confirm both actually ran
   (distinct subagent IDs, not a rejected/queued duplicate). OK.
7. Zero CJK content anywhere in the good session's events. OK.

### What's NOT been checked

- Whether this reproduces with a non-GLM model (e.g. Anthropic) under an
  equivalent long SDD subagent chain — would help separate "GLM-specific
  quirk" from "any model, given enough chained dispatches, will do this."
- Whether raising/lowering the reassertion interval
  (`config.economy`/reassert threshold) changes onset frequency.
- Whether `dispatch_subagent`'s "must call `read` on brief/report files
  before reviewing" pattern (present in every gilfoyle task prompt) is itself
  adding enough tokens per subagent turn to matter for onset timing.

### Next steps (not yet decided — options to weigh)

1. **Harden `dispatch_subagent` mechanically**: after emitting the "end your
   turn now" `ToolResult`, discard/suppress further assistant text on that
   turn server-side rather than trusting the model to stop; add per-task
   dedup (reject/collapse a second dispatch whose task text matches an
   in-flight one) alongside the existing concurrency ceiling.
2. **Model-side mitigation**: consider whether the SDD/subagent-heavy
   workflow should pin a different model than the interactive-session
   default, or add a language-drift detector (non-ASCII ratio check) that
   forces a re-roll of the turn.
3. Do nothing further yet — this doc is the persisted diagnosis; revisit
   priority later.
