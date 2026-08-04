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

## Design

### Approach

A hybrid fix: the provider marks Usage events whose `input` is partial
(uncached-only), and consumers (TUI display, calibration, ledger) fall back to
the `context_window` chars/3 estimate — a full-prompt projection that is always
consistent and monotonic. Calibration also learns the ratio only from
cache-miss turns (where `input` is the real full prompt), keeping the estimate
accurate over time.

The `context_window` projection (chars/3 + overhead) is already computed and
cached in the `ProjectionCache`. It always reflects the full prompt, naturally
handles eviction (re-projects from the event log with evicted turns removed),
and is the existing fallback when no Usage has arrived. The fix extends that
fallback to "when the provider can't report a reliable full-prompt input."

### 1. `TokenStat` gains `input_partial`

`crates/zoid-core/src/event.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenStat {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub thinking: u64,
    /// True when `input` does NOT represent the full prompt size — only the
    /// uncached portion. Ollama sets this on cache-hit turns where
    /// `prompt_eval_count` excludes the warm KV-cache prefix. When true,
    /// consumers (TUI display, calibration, ledger) fall back to the
    /// `context_window` estimate instead of trusting `input`.
    /// `#[serde(default)]` keeps old DB rows deserializing as `false`
    /// (full prompt — the historical assumption).
    #[serde(default)]
    pub input_partial: bool,
}
```

Backward-compatible: `TokenStat` has no `deny_unknown_fields`, so old DB rows
deserialize with `input_partial = false` (the historical assumption that `input`
is the full prompt). No schema migration needed.

### 2. `provider::Usage` gains `input_partial`

`crates/zoid-provider/src/lib.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached: u64,
    pub thinking_tokens: u64,
    /// True when `input_tokens` does not represent the full prompt — only the
    /// uncached portion (Ollama cache-hit turns). Consumers fall back to the
    /// `context_window` estimate when set.
    pub input_partial: bool,
}
```

All non-Ollama providers set `input_partial: false` (the default). Only the
Ollama provider sets it `true`.

### 3. Ollama provider detects partial input

`crates/zoid-provider/src/ollama.rs`, the `done` frame handler:

```rust
let prev = last_prompt_eval.swap(curr, Ordering::Relaxed);
let cached_approx = curr.min(prev);
let input_partial = prev > 0 && curr < prev;
out.push(ProviderEvent::Usage(Usage {
    input_tokens: curr,
    output_tokens: output.unwrap_or(0),
    cached: cached_approx,
    thinking_tokens: 0,
    input_partial,
}));
```

Detection rule: **`prev > 0 && curr < prev`** — if the reported count shrank
from the previous sub-turn and there was a previous sub-turn, the warm prefix
was cached and `curr` is only the uncached tail. If `prev == 0` (first sub-turn
ever) or `curr >= prev` (prompt grew or stayed flat — full eval),
`input_partial = false`.

**Eviction edge case:** after eviction, the prompt shrinks and `curr < prev`,
so this flags as `input_partial = true` — but `curr` is actually the full new
(smaller) prompt. This is a false positive: we fall back to the estimate when
we could have used the real value. This is acceptable because the estimate
after eviction is accurate (it re-projects from the event log with evicted
turns removed) — we trade a small accuracy loss (estimate vs. real) for safety
on the common cache-hit case.

### 4. Agent loop threads `input_partial`

`crates/zoid/src/agent.rs`, the Usage accumulation (`agent.rs:940`):

```rust
ProviderEvent::Usage(u) => {
    turn_usage.input += u.input_tokens;
    turn_usage.output += u.output_tokens;
    turn_usage.cached += u.cached;
    turn_usage.thinking += u.thinking_tokens;
    turn_usage.input_partial |= u.input_partial;
}
```

OR-accumulation: if *any* Usage event in the sub-turn was partial, the summed
`input` is partial. For Ollama this is moot (one Usage event on the `done`
frame), but it is correct for the general case.

The recorded Usage event (`agent.rs:1076`) already carries `turn_usage` as the
`TokenStat`, so `input_partial` flows through automatically — no change at the
emit site.

### 5. TUI display falls back to estimate

`crates/zoid/src/main.rs`, the `last_input_tokens` extraction
(`main.rs:1477`):

```rust
self.last_input_tokens = events
    .iter()
    .rev()
    .find_map(|e| e.tokens.filter(|t| !t.input_partial).map(|t| t.input))
    .filter(|&t| t > 0);
```

Skips Usage events where `input_partial == true` — they don't represent the
full prompt. The scan finds the last *reliable* input. Then `ctx_used`
(`main.rs:2937`) already has the right fallback:

```rust
app.shell.ctx_used = app
    .proj
    .last_input_tokens
    .unwrap_or(app.proj.window.total_tokens);
```

When the last reliable input is a cache-miss turn, `ctx_used` shows the real
~200k. When the last turn was a cache-hit (skipped), it falls back to
`window.total_tokens` — the chars/3 estimate, a consistent full-prompt
projection. On the very first turn (no reliable input yet), it also falls back
to the estimate, same as today.

The status bar gauge (`render.rs:805`: `frac = ctx_used / ctx_ceiling`) is
driven by the same `ctx_used`, so the gauge also reflects the corrected value.

### 6. Calibration skips partial-input turns

`crates/zoid/src/agent.rs`, `record_compactions` (`agent.rs:2796`):

```rust
let effective_tokens = real_input_tokens
    .filter(|&t| t > 0 && !turn_input_partial);
```

`turn_input_partial` is threaded from `turn_usage.input_partial` at the call
site (`agent.rs:1102`). On cache-miss turns (`input_partial = false`), the
ratio is learned as before. On cache-hit turns (`input_partial = true`), the
ratio is left unchanged — the last known ratio persists and `plan_compactions`
continues to scale the estimate by it.

The same guard applies to the forced-eviction retry path (`agent.rs:957`):
when `turn_usage.input_partial` is true, use the estimate-scaled value
(`raw × calibration_ratio × OVERCOUNT_BIAS`) instead of the raw `input`.

### 7. Economy ledger uses estimate for partial-input turns

`crates/zoid-core/src/economy.rs`, `token_ledger`:

On a partial-input turn, `input` is only the uncached tail. To make the ledger
honest, the full prompt size is needed. The only full-prompt projection is
`context_window_with`.

The fix: when `input_partial`, use `max(input, context_window.total_tokens)`
as the `input` contribution for that event. `input_partial` means "input is a
lower bound, the real prompt is at least this large," so `max(input, estimate)`
is a safe honest approximation. On non-partial turns, `input` is the real full
prompt and wins.

`token_ledger` is pure and takes `&[Event]`. It currently sums `TokenStat`
fields directly. The change: when an event's `tokens.input_partial` is true,
substitute `max(input, context_window.total_tokens)` for that event's `input`
contribution. The `context_window` projection is computed by `token_ledger`
internally (calling `context_window_with` on the events seen so far up to and
including the partial event). This keeps `token_ledger` self-contained — no new
parameter, no caller responsibility — and pure (same inputs → same outputs).

### Testing

Three layers, each testable independently:

**Provider layer** (`ollama.rs` tests, existing `parse_seq`):
- Two-sub-turn sequence where the second has a smaller `prompt_eval_count` →
  assert `input_partial = true` on the second Usage.
- A sequence where `prompt_eval_count` grows → `input_partial = false`.
- First sub-turn (no prev) → `input_partial = false`.

**Core layer** (`economy.rs` / `context.rs` tests):
- `token_ledger` with a mix of partial and non-partial Usage events → assert
  partial events contribute the estimate (via `max(input, estimate)`) to the
  total, non-partial contribute `input`.
- `context_window` is already well-tested; no change needed.

**Agent/Display layer** (`main.rs` / `agent.rs` tests):
- `last_input_tokens` extraction skips partial Usage events → falls back to the
  last reliable one, then to `window.total_tokens`.
- `record_compactions` calibration: partial-input turn does not update
  `calibration_ratio`; non-partial turn does.
- Forced-eviction retry uses estimate on partial input.
- Existing TUI snapshot tests may need updated fixtures if the display value
  changes; logic is input-driven so most snapshots should be unaffected.

**Integration** (`zoid-core` round-trip test):
- A Usage event with `input_partial = true` round-trips through the DB
  (serialize → store → load → deserialize) and preserves the flag. Old rows
  (no field) deserialize as `false`.

## Scope

**In scope:**
1. `TokenStat` gains `input_partial: bool` with `#[serde(default)]`
2. `provider::Usage` gains `input_partial: bool`
3. Ollama provider sets `input_partial = prev > 0 && curr < prev`
4. Agent loop OR-accumulates `input_partial` into `turn_usage`
5. TUI `last_input_tokens` skips partial events, falls back to `window.total_tokens`
6. Calibration skips partial-input turns (ratio persists)
7. Preflight forced-eviction retry uses estimate on partial input
8. Economy ledger uses estimate for partial-input turns
9. Tests at all three layers

**Out of scope (YAGNI):**
- No tokenizer dependency — we use the existing chars/3 estimate, not true
  tokenization.
- No changes to non-Ollama providers — they already report full-prompt `input`.
- No schema migration — `#[serde(default)]` makes the new field
  backward-compatible with old DB rows.
- No retroactive fix for historical Usage events in existing DBs — old rows
  have `input_partial = false` and keep their current (undercounted) ledger
  contribution; the fix is forward-looking.
- No UI changes to the status bar/gauge rendering itself — only the `ctx_used`
  value feeding it changes.