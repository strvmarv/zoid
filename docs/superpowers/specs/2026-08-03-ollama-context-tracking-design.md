# Ollama Context Tracking Fix

## Problem

When using `ollama-cloud`, the `ctx_used` value displayed in the TUI status bar
is inaccurate on cache-hit turns. It shows ~5-15k tokens when the real context is
~200k, and only shows the real ~200k value on cache-miss turns.

### Root cause

Ollama's `/api/chat` `done` frame reports `prompt_eval_count` as "how many input
tokens were **processed**" — tokens actually *evaluated*, not tokens served from
the warm KV cache. On a cache-hit turn, the prompt prefix is warm in KV cache and
only the new (uncached) tail is evaluated, so `prompt_eval_count` is a tiny
fraction of the real prompt. On a cache-miss turn (cold cache, first turn, or
post-eviction), `prompt_eval_count` is the full prompt size.

The chain:

1. `ollama.rs:196` maps `prompt_eval_count` → `input_tokens`. On cache-hit
   turns this is only the uncached tail, not the full prompt.
2. `agent.rs:941` accumulates this into `turn_usage.input`, recorded as the
   Usage event's `input` field.
3. `main.rs:1477` extracts `last_input_tokens` = the last Usage event's `input`.
4. `main.rs:2937` sets `ctx_used = last_input_tokens.unwrap_or(window.total_tokens)`
   → the status bar shows the uncached-only value.

### Empirical evidence

Queried the last active session (`01KZ53Z8HKR5SAP7YZ1GQWMEKR`), 520 Usage events:

| rowid   | input (shown) | cached    | reality                          |
|---------|---------------|-----------|----------------------------------|
| 993137  | 3,801         | 3,801     | cache HIT — only tail evaluated  |
| 993141  | 199,024       | 3,801     | cache MISS — full prompt         |
| 993143  | 4,784         | 4,784     | cache HIT again                  |

The input jumps 3.8k→199k in one sub-turn — impossible under the "full-prompt"
model (append-only conversation can't grow 52× in one sub-turn), but exactly
what the "uncached-tail" model predicts. 0 of 520 events have input=0, ruling
out the "reports 0 on cached" model in `agent.rs:738`.

### Existing code comments are wrong

The codebase has three conflicting models of `prompt_eval_count`:

1. `ollama.rs:300` — documents it as "full prompt size" (**wrong**)
2. `agent.rs:738` — "Ollama reports 0 on cached prompts" (**wrong** — 0 events
   with input=0 in 520 Usage events)
3. The spec's model — "uncached tail only" (**correct**, per empirical data)

All three comments should be corrected as part of this fix.

### Provider survey

Every provider except Ollama reports the full prompt as `input`:

| Provider        | `input_tokens` source                          | Full prompt? | `cached` source                         |
|-----------------|------------------------------------------------|--------------|------------------------------------------|
| Anthropic       | `input_tokens + cache_read + cache_creation`  | Yes          | `cache_read_input_tokens` (real)         |
| OpenAI-compat   | `prompt_tokens`                               | Yes          | `prompt_tokens_details.cached_tokens`   |
| OpenAI-responses| `input_tokens`                                | Yes          | `input_tokens_details.cached_tokens`     |
| Gemini          | `promptTokenCount`                            | Yes          | (none — 0)                               |
| **Ollama**      | `prompt_eval_count`                            | **No**       | `min(curr, prev)` (synthetic)            |

Ollama is the only provider where `input` does not represent the full prompt.

### Collateral damage

The `calibration_ratio` (`agent.rs:2798`) learns `real_input /
context_window.total_tokens`. When `real_input` is only the uncached portion,
the learned ratio is far too low → the preflight eviction/compaction gate
underestimates context size → may fail to evict/compact in time.

The economy ledger (`economy.rs:21`) sums `input` across Usage events into the
session's total token spend. On partial-input turns, the summed `input`
undercounts the real prompt cost.

`churn_timeline` (`economy.rs:117`) sums `t.input + t.output` per turn and feeds
the economy drawer sparkline — same undercount on partial turns.

## Design

### Approach: provider-side reconstruction (n3)

The fix lives entirely in the Ollama provider (`ollama.rs`). On a cache-hit turn
(where `prompt_eval_count` is only the uncached tail), reconstruct the full
prompt size from the previous sub-turn's known full prompt:

- **Cache-miss turn** (`curr >= prev`): `input_tokens = curr` (the full prompt),
  `cached = 0`. This is today's behavior — unchanged.
- **Cache-hit turn** (`prev > 0 && curr < prev`): `input_tokens = prev` (the
  last known full prompt size), `cached = prev - curr` (the portion served from
  cache). The real prompt is approximately `prev` (it grew by the new turn's
  tokens, but `curr` — the evaluated tail — is a lower bound on that growth, and
  `prev` is much closer than `curr`).

No new field on `TokenStat` or `Usage`. No consumer-side fallback. No new
parameter to `record_compactions`. Every consumer — TUI display, calibration,
ledger, churn timeline, overview — works correctly because `input_tokens`
now always represents (approximately) the full prompt, same as every other
provider.

### Why not the hybrid approach (flag + fallback)

An earlier draft proposed marking Usage events as `input_partial` and having
consumers fall back to the `context_window` chars/3 estimate. This was rejected
because:

1. **O(n²) ledger**: `token_ledger` (`economy.rs:21`) would need to call
   `context_window_with` (O(n)) per partial event. `token_ledger` is called
   every frame on the overview panel (`main.rs:456`), so a 25k-event session
   with 30 partials would do 778k event-touches/frame — ~47M/sec at 60fps.
   A 50k-event session would freeze the UI.
2. **6+ consumer sites to patch**: bulk refresh (`main.rs:1477`), incremental
   `apply_event` (`main.rs:1585`), both `record_compactions` call sites
   (`agent.rs:1095`, `agent.rs:2655`), `token_ledger` (`economy.rs:25`),
   `churn_timeline` (`economy.rs:117`), and the incremental ledger path
   (`main.rs:1582`).
3. **The estimate is approximate** (chars/3, ±15-20%) and adds complexity for
   no accuracy gain over `prev` (which is the real prompt size from one
   sub-turn ago).

### 1. Ollama provider reconstructs full prompt on cache-hit

`crates/zoid-provider/src/ollama.rs`, the `done` frame handler (lines 195-213):

```rust
if input.is_some() || output.is_some() {
    let curr = input.unwrap_or(0);
    use std::sync::atomic::Ordering;
    let prev = last_prompt_eval.swap(curr, Ordering::Relaxed);

    // prompt_eval_count reports only the tokens *evaluated* (the uncached
    // tail), not the full prompt — the warm KV-cache prefix is not counted.
    // On a cache-hit turn (curr < prev), reconstruct the full prompt from the
    // previous sub-turn's known size. On a cache-miss turn (curr >= prev),
    // curr is the full prompt.
    let (input_tokens, cached) = if prev > 0 && curr < prev {
        // Cache hit: prev was the full prompt, curr is the uncached tail.
        // The real prompt is ~prev (it grew by the new turn's tokens, but
        // prev is far closer than curr). cached = the warm prefix.
        (prev, prev - curr)
    } else {
        // Cache miss or first turn: curr is the full prompt.
        (curr, 0)
    };

    out.push(ProviderEvent::Usage(Usage {
        input_tokens,
        output_tokens: output.unwrap_or(0),
        cached,
        thinking_tokens: 0,
    }));
}
```

**Detection rule**: `prev > 0 && curr < prev` — if the reported count shrank
from the previous sub-turn and there was a previous sub-turn, the warm prefix
was cached and `curr` is only the uncached tail.

**Eviction edge case**: after eviction, the prompt shrinks and `curr < prev`,
so this reconstructs `input = prev` (overcount — the real prompt is now
`curr`, the smaller post-eviction size). This is a false positive: we report
`prev` when `curr` is the real full prompt. This is acceptable because:
- It's the same false positive the hybrid approach would have.
- The overcount is bounded (prev is one sub-turn's growth above the real
  size), and the next cache-miss turn corrects it.
- The alternative (trusting `curr` on all `curr < prev` turns) would massively
  *undercount* on the common cache-hit case — far worse than a small overcount
  after the rare eviction.

### 2. Correct the wrong code comments

Three comments in the codebase document `prompt_eval_count` incorrectly. All
should be corrected:

- `ollama.rs:300-306` — the `last_prompt_eval` field doc says "full prompt
  size." It should say "the previous sub-turn's `prompt_eval_count` (uncached
  tail on cache-hit turns, full prompt on cache-miss turns)."
- `ollama.rs:197-203` — the inline comment about `cached = min(curr, prev)`
  assumes `prompt_eval_count` is the full prompt. Rewrite to reflect the
  uncached-tail model.
- `agent.rs:736-741` — the calibration comment says "when the provider
  reports 0 (Ollama cached prompt)." Ollama never reports 0; it reports the
  uncached tail. Correct the comment.

### Testing

The fix is in one function, tested at the provider layer:

**Provider layer** (`ollama.rs` tests, existing `parse_seq`):

- `implicit_cache_approx_first_subturn_has_zero_cached` — unchanged: first
  sub-turn, prev=0, input=curr, cached=0.
- `implicit_cache_approx_second_subturn_credits_overlap` — **changed**: second
  sub-turn has `curr=13000 >= prev=12000`, so it's a cache miss: input=13000,
  cached=0. (Previously asserted cached=12000 via `min(curr, prev)` — no longer
  applies.)
- `implicit_cache_approx_shrinking_prompt_credits_smaller_overlap` —
  **changed**: second sub-turn has `curr=30000 < prev=50000`, so it's a cache
  hit: input=50000 (prev), cached=20000 (prev - curr). (Previously asserted
  input=30000, cached=30000.)
- **New test**: cache-hit sequence where `curr` is much smaller than `prev`
  (e.g. prev=200000, curr=5000) → input=200000, cached=195000.
- **New test**: cache-miss after eviction where `curr < prev` but `curr` is the
  real full prompt → still reports input=prev (the known false positive).
  Documents the edge case explicitly.

No core, agent, or TUI test changes needed — `input_tokens` now always
represents (approximately) the full prompt, so all consumers work as-is.

## Scope

**In scope:**
1. Ollama provider reconstructs `input_tokens = prev` on cache-hit turns
   (`curr < prev`), `cached = prev - curr`.
2. Correct three wrong code comments about `prompt_eval_count` semantics.
3. Update three existing Ollama tests for the new behavior.
4. Add two new tests (deep cache hit, post-eviction false positive).

**Out of scope (YAGNI):**
- No new field on `TokenStat` or `provider::Usage`.
- No consumer-side changes — display, calibration, ledger, churn all work
  as-is because `input` is now (approximately) the full prompt.
- No changes to non-Ollama providers — they already report full-prompt `input`.
- No schema migration — no DB format change.
- No retroactive fix for historical Usage events in existing DBs — old rows
  keep their current (undercounted) values; the fix is forward-looking.
- No tokenizer dependency — `prev` is the real prompt size from the previous
  sub-turn, not an estimate.