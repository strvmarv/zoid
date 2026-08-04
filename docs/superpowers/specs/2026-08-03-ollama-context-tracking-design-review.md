# Re-Review: Ollama Context Tracking Fix (n3 — provider-side reconstruction)

**Spec:** `docs/superpowers/specs/2026-08-03-ollama-context-tracking-design.md`
**Reviewer:** Gilfoyle
**Prior review:** hybrid (flag + fallback) — raised B1 (blocking, no empirical
evidence), M1–M4 (consumer-side complexity), and proposed n3 as an alternative.
**This review:** the spec has been rewritten *to* n3. Verifying the rewrite.

## Verdict

**Approve with nits.** The n3 rewrite is the correct call. It collapses a
six-site, O(n²)-ledger, schema-touching, consumer-rewiring disaster into a
single-function change in one provider, and it does so on the back of actual
measurements rather than vibes. The previous review's central objection — that
the spec asserted a model of `prompt_eval_count` with zero evidence — is now
answered with a DB query showing the uncached-tail model is the only one
consistent with the data (3.8k→199k jump in one sub-turn; 0 of 520 events at
input=0). That's how you settle a modeling dispute. Good.

The approach is sound, the test predictions are arithmetically correct (I
traced every case), and `cached = prev - curr` cannot go negative or exceed
`input_tokens` under the stated guard. The findings below are nits, one
miscount, and one redundancy — none blocking.

## Status of prior findings

| Prior | Status | Notes |
|-------|--------|-------|
| **B1** (blocking: no empirical evidence) | **Resolved** | §"Empirical evidence" adds the DB rows. The 52× single-sub-turn jump is dispositive against the full-prompt model; the 0/520 at input=0 is dispositive against the "reports 0 on cached" model in `agent.rs:738`. Both wrong-code-comment models are now falsified with data. |
| **M1** (incremental `apply_event` path) | **Moot** | Confirmed: no consumer-side change. `apply_event` (`main.rs:1571`) just does `self.last_input_tokens = Some(t.input)` (`main.rs:1586`) — it stores whatever `input` the provider emits. Under n3 that value is ≈full prompt on cache-hit turns, so the incremental path is *fixed for free* with zero edits. |
| **M2** (second `record_compactions` call site) | **Moot** | Both call sites (`agent.rs:1095`, `agent.rs:2655`) pass `turn_usage.input` unchanged. n3 makes that value sane on cache-hit turns, so both improve without edits. |
| **M3** (`churn_timeline`) | **Moot** | `economy.rs:117` sums `t.input + t.output` per turn. No `context_window_with` call. n3 feeds it ≈full-prompt `input`; the sparkline just becomes accurate. |
| **M4** (O(n²) ledger) | **Eliminated** | `token_ledger` (`economy.rs:21`) is a flat sum over `e.tokens` — no `context_window_with` anywhere in `economy.rs` (grep confirmed: zero hits). The hybrid's per-event O(n) fallback is gone. |
| **n3** (the alternative I proposed) | **Adopted** | This *is* the spec now. Verified correct below. |

## Focus 1 — Is n3 sound? Does `input_tokens = prev` approximate the full prompt?

**Yes, and it's a better approximation than the hybrid's chars/3 fallback.**

The reconstruction rests on one fact the empirical section now establishes: on a
cache-miss turn, `prompt_eval_count` *is* the full prompt (curr = full). On the
*next* cache-hit turn, the warm prefix is not re-counted, so curr = uncached
tail only. The full prompt at that point is "prev (last known full) + the new
turn's tokens." n3 reports `prev`, which omits exactly one turn of growth.

- Magnitude of the error: one user turn ≈ a few hundred to low thousands of
  tokens; `prev` for a long session is ~200k (per the empirical row 993141). The
  approximation is off by <1% on the cache-hit turns that matter (the ones
  currently displaying 5–15k instead of ~200k — a 92–97% error today). Replacing
  a 92–97% error with a <1% error is not "approximate"; it's a fix.
- vs. the hybrid: the fallback was chars/3, documented at ±15–20% (`lib.rs`
  estimate_tokens). `prev` is a *measured* token count from one sub-turn ago —
  strictly more accurate than a character heuristic, and it carries no
  tokenizer dependency. The spec makes this exact point (§"Why not the hybrid",
  point 3) and it's correct.

The detection rule `prev > 0 && curr < prev` is the right trigger: a cache hit
is precisely the condition where the evaluated count *shrinks* relative to the
prior full prompt (you evaluate less, not more, because the prefix is warm). A
growing `curr` (the normal append-only conversation between two full-eval
turns) correctly falls through to the `else` (cache miss). This matches the
empirical pattern (993137 tail 3.8k < 993141 full 199k → hit).

## Focus 2 — Eviction false-positive: acceptable? Is the self-correction real?

**Acceptable, and the self-correction is real — but the spec undersells *when*
it corrects, and that's worth one sentence.**

The false positive: after an eviction the prompt genuinely shrinks, so
`curr < prev` holds, and n3 reconstructs `input = prev` (the pre-eviction size)
when the real prompt is now `curr` (post-eviction). Overcount.

- **Bounded**: `prev` is one sub-turn stale at most. After eviction the prompt
  *shrinks*, so `prev` is an upper bound on the real size, and the overcount is
  `prev - curr` — exactly one eviction's worth. It does not compound: the very
  next turn, `prev` is now the (large, wrong) reconstructed value, and unless
  *another* eviction happens, `curr` next turn is the full re-evaluated prompt
  which is ≥ the post-eviction size... wait. Let me be precise, because this is
  the one place the spec is slightly hand-wavy.

  The spec says "the next cache-miss turn corrects it." A cache-miss turn is
  `curr >= prev`. After the false-positive turn, `last_prompt_eval` holds the
  *reconstructed* `prev` (the overcounted value), because `swap` stores `curr`
  (the raw value), **not** the reconstructed `input_tokens`. Look at the code:
  `let prev = last_prompt_eval.swap(curr, ...)` — it swaps in `curr`, the raw
  reported count, every time. So the stored state is always the *raw*
  `prompt_eval_count`, never the reconstruction. That's the right invariant:
  the next turn compares against the real prior `prompt_eval_count`, not a
  fabricated one.

  Consequence: on the turn *after* the eviction false-positive, `prev` = the
  post-eviction raw `curr` (small), and the next `curr` (the next full prompt,
  which has grown by one turn) is ≥ that. So `curr >= prev` → cache-miss branch
  → `input = curr` (the true full prompt). **Correction happens on the very next
  turn, unconditionally** — it does not require a subsequent cache-miss to
  "happen," because *any* normal-growing turn after an eviction is a `curr >=
  prev` cache-miss by definition. The spec's phrasing "the next cache-miss
  corrects it" is true but makes it sound contingent; in practice the next
  turn corrects it unless another eviction fires back-to-back.

- **Back-to-back evictions** are the only way the overcount persists more than
  one turn, and even then it's bounded by the cumulative evicted amount, not
  unbounded growth. This is the same bound the hybrid would have had, as the
  spec notes. Fine.

  One real cost the spec doesn't quantify: the false-positive turn feeds
  `input = prev` (overcount) into `turn_usage.input` → `record_compactions`
  learns `calibration_ratio = prev / current_est`. Post-eviction, `prev` is
  *larger* than the real prompt, so the ratio is *over*-estimated → future
  preflight estimates run *hot* → slightly more aggressive eviction. That's the
  *safe* direction (over-evicting after an eviction is nearly a no-op), and it
  self-corrects the turn after. Not worth coding around; worth a sentence in the
  spec so the next reader doesn't chase it.

## Focus 3 — Are the test changes correct? (traced against actual code)

I read the helpers (`parse_first` at `ollama.rs:609`, `parse_seq` at
`ollama.rs:615`) and the three `implicit_cache_approx_*` tests
(`ollama.rs:1159`, `:1179`, `:1209`), then walked each through the proposed n3
body (`spec:128-156`). The n3 logic, restated:

```
prev = last_prompt_eval.swap(curr, Relaxed)   // returns OLD, stores curr
if prev > 0 && curr < prev: (input=prev,  cached=prev-curr)
else:                        (input=curr,  cached=0)
```

| Test | Inputs (seq) | n3 trace | Spec's predicted assertion | Match? |
|------|--------------|----------|----------------------------|--------|
| `…first_subturn…` (`parse_first`, prev=0) | curr=12000 | prev=0 → else → input=12000, cached=0 | "unchanged: input=12000, cached=0" | ✅ Matches current assertion exactly. Genuinely unchanged. |
| `…second_subturn_credits_overlap` (seq 12000→13000) | T1: prev=0→else (in=12000,c=0); T2: prev=12000, curr=13000, `13000<12000`? No → else (in=13000, c=0) | "changed: cache miss, input=13000, cached=0" | ✅ Current asserts cached=12000 — must change to cached=0. Spec correct. |
| `…shrinking_prompt…` (seq 50000→30000) | T1: prev=0→else (in=50000,c=0); T2: prev=50000, curr=30000, `30000<50000`? Yes → (in=50000, c=20000) | "changed: input=50000 (prev), cached=20000 (prev-curr)" | ✅ Current asserts in=30000/c=30000 — must change to in=50000/c=20000. Spec correct. |
| **New A** (deep hit, 200000→5000) | T2: prev=200000, curr=5000 → (in=200000, c=195000) | "input=200000, cached=195000" | ✅ Arithmetically exact. |
| **New B** (post-eviction FP, curr<prev, curr is real full) | same path as revised test 3 | "still reports input=prev (the known false positive)" | ✅ path-correct, but see Nit-2. |

**All five predictions are arithmetically exact.** No surprises in the trace.

Two things the spec gets slightly wrong in *counting* these (not in their
values):

- **Nit-1 (scope miscount):** Scope item 3 says "Update three existing Ollama
  tests." Only **two** existing tests change (`…second_subturn…`, `…shrinking…`).
  The first test (`…first_subturn…`) uses `parse_first` (prev=0) and is
  **unchanged** — the spec's testing section correctly says "unchanged" for it,
  but the Scope bullet lumps it into "three updated." Fix the Scope count to
  "two existing tests updated, one unchanged, two new."

- **Nit-2 (redundant coverage):** New test B (post-eviction false positive)
  exercises the *identical code path* as the revised `…shrinking_prompt…` test
  (both: `curr < prev` → input=prev). The only difference is the *narrative*
  attached to the same numbers. New test A (deep hit, 200000→5000) is the one
  that adds genuinely new coverage (a 40× ratio, far from test 3's 1.67×).
  Either (a) drop B as redundant, or (b) make B *distinguish* the false positive
  by asserting the *next* turn self-corrects — i.e., a three-line seq
  `[50000, 30000(FP), 31000(corrects)]` asserting turn 3 yields input=31000,
  cached=0. That would actually verify the mitigation the spec claims, which
  no current test does. As written, B is test 3 with a sad title.

**Cross-check — other tests that use `parse_first` are safe:** I grepped every
`parse_first`/`parse_seq` call site (`ollama.rs:835–1009, 1162, 1183, 1213`).
All non-approx tests use `parse_first` (fresh atomic=0 → else branch → identical
to today). Only the two `parse_seq` tests change. So "no other provider test
breaks" is verified, not asserted.

## Focus 4 — Remaining gaps / consumers that would still break

**None that break. All improve or stay neutral.** Walked the full consumer
surface:

1. **Status bar `ctx_used`** (`main.rs:2937`): `last_input_tokens.unwrap_or(window.total_tokens)`. The `last_input_tokens` refresh (`main.rs:1477-1480`) does `.filter(|&t| t > 0)`. Under the old code, cache-hit turns stored the tiny tail (passes the `>0` filter, but is wrong). Under n3 they store `prev` (>0, ≈full). So the filter still passes and the value is now right. The incremental twin (`apply_event`, `main.rs:1586`) stores `Some(t.input)` with no filter — also now right. ✅
2. **`calibration_ratio`** (`agent.rs:2798-2801`): learns `real / window.total` whenever `turn_usage.input > 0` (gated at `agent.rs:1102`). Old: on cache-hit turns `turn_usage.input` was the tail → ratio ≈ 0.02 → preflight under-estimated → failed to evict (the spec's "collateral damage"). n3: `turn_usage.input` ≈ full → ratio ≈ true bias → learned on *every* non-reassert turn, not just misses. This is a strict improvement, and it's the second bug (beyond the display bug) that n3 fixes for free. The spec's "Collateral damage" section documents it; good. ✅
3. **Economy ledger** (`economy.rs:21-33`): flat sum of `t.input`. No `context_window_with` (grep-confirmed: 0 hits in `economy.rs`). n3 feeds it ≈full input; the session token spend stops undercounting. ✅
4. **`churn_timeline`** (`economy.rs:117`): sums `t.input + t.output` per turn; `p.cached += t.cached` (`economy.rs:118`). The per-turn sparkline (`main.rs:464`) and overview `cache_hit_pct` (`main.rs:458`, `cached*100/input`) now get a *meaningful* synthetic `cached` instead of the old `min(curr,prev)` which was also synthetic but semantically muddled. ✅ (see Focus 5 for the `cached` contract)
5. **Overview** (`main.rs:456-482`): `cache_hit_pct = cached*100/checked_div(input).min(100)`. Under n3, per-turn `cached ≤ input` (Focus 5), so the percentage is in [0,100] and the `.min(100)` clamp is belt-and-suspenders. `cache_read = ledger.cached` is now a real "warm-prefix tokens" figure. ✅
6. **Non-Ollama providers**: untouched by construction — the change is inside the Ollama `done`-frame handler only. ✅
7. **DB / schema**: no `TokenStat` or `Usage` field added/removed (grep of `lib.rs:130-146` confirms `Usage` is unchanged). No migration. Historical rows keep undercounted values (spec marks this out-of-scope, forward-looking only — correct call). ✅

The only *informational* gap, not a breakage: the spec doesn't note that the
`done_line_with_counts_yields_usage_then_done` test (`ollama.rs:851`, uses
`parse_first`, prev=0) and `partial_counts_default_missing_side_to_zero`
(`ollama.rs:870`, `parse_first`, prev=0) remain green purely because they use a
*fresh* atomic. If anyone later refactors those to `parse_seq`, the
`partial_counts` case (no `prompt_eval_count` → `curr=0`) with a warm `prev`
would flip to `input=prev, cached=prev` (100%-hit claim). That's not a bug — it's
"absent field ⇒ treat as 0 ⇒ if prev warm, assume full cache hit," which is a
defensible reading — but the spec should mention the `curr==0` edge so a future
reader doesn't rediscover it as a mystery. (See Focus 5.)

## Focus 5 — Is `cached = prev - curr` sane? Negative? Exceeds `input_tokens`?

**Sane on both counts, by construction.**

- **Negative?** No. The branch is `if prev > 0 && curr < prev`. `curr < prev`
  strictly, so `prev - curr ≥ 1`. The subtraction is on `u64`; without the
  strict guard it would panic on underflow, but the guard is exactly the
  precondition that prevents it. ✅ (Worth a one-line code comment at the
  subtraction site so a future "let me simplify this guard" doesn't reintroduce
  a panic — the spec's proposed comment at `spec:140-142` covers the *semantics*
  but not the *u64-underflow* reliance. Add "NB: the `curr < prev` guard is also
  what keeps this `u64` subtraction from underflowing — don't 'simplify' it.")

- **Exceeds `input_tokens`?** No. `input_tokens = prev` and `cached = prev -
  curr`. `cached < input_tokens` iff `curr > 0`. Empirically `curr > 0` on all
  520 events, so in practice `cached < input` strictly. In the *limiting* case
  `curr == 0` (absent `prompt_eval_count` field, or a hypothetical zero report),
  `cached = prev = input_tokens` — *equal*, not exceeding. So `cached ≤
  input_tokens` always; `cached == input_tokens` only at `curr == 0`. ✅

  This matters because `cache_hit_pct` (`main.rs:458`) does
  `cached*100/checked_div(input).min(100)`. Even the `cached == input` case
  yields 100%, clamped — no overflow, no >100%. The proptest at `economy.rs:191`
  uses *independent* `cached`/`input` ranges (so synthetic data can have
  `cached > input`), but that's test-only; the provider guarantees `cached ≤
  input` at the source. No invariant violated downstream.

  The `curr == 0` case (→ `cached == input`, a 100%-cache-hit claim) is the one
  edge the spec doesn't name. It is benign: an absent/zero `prompt_eval_count`
  with a warm `prev` reads as "everything was cached," which is the most
  charitable interpretation and harms nothing (it just inflates `cache_hit_pct`
  for that one turn). Flag it as a documented edge, not a fix.

## Severity summary

| Sev | Finding | Where |
|-----|---------|-------|
| **Nit** | Scope miscount: "three existing tests updated" — only two change; the first is unchanged. | spec §Scope item 3 |
| **Nit** | New test B (post-eviction FP) duplicates revised test 3's code path; adds no coverage. Make it a 3-turn seq asserting self-correction, or drop it. | spec §Testing, "New test: cache-miss after eviction" |
| **Nit** | "Next cache-miss corrects it" undersells the self-correction: the *next turn* corrects unconditionally (any `curr >= prev` is a miss), unless evictions fire back-to-back. Add one sentence. | spec §"Eviction edge case" |
| **Nit** | The `curr == 0` edge (→ `cached == input_tokens`, 100%-hit claim) is undocumented. Benign; name it so it isn't a future mystery. | spec §"1. Ollama provider…" |
| **Nit** | The `u64` subtraction `prev - curr` relies on the `curr < prev` guard to avoid underflow panic. The proposed comment covers semantics, not the underflow reliance. Add a "don't simplify the guard" note. | spec code block, `cached = prev - curr` |
| **Nit** | Line citations drift by ±1 (spec says `ollama.rs:300-306` field doc; actual 300-307; `agent.rs:736-741` → 736-742). Cosmetic. | spec §"Correct the wrong code comments" |

**No blocking findings. No high/medium findings.** The n3 rewrite retires every
prior M-finding by deleting the consumer surface that created them, settles B1
with measurements, and the one function it does change is arithmetically
verified against all five test cases. Ship it after the nits.

---

*— Gilfoyle. The previous spec tried to fix a one-function bug by rewiring six
consumers and inventing a schema field. This one fixes a one-function bug by
fixing the one function. Progress.*