# Model Registry (minimal, caps-only) — Design

> **Status:** SPEC ONLY — implementation deferred. This documents the agreed
> minimal shape so it can be built as a standalone task later. Supersedes the
> ad-hoc `contains("glm")` / `contains("claude")` string checks currently
> scattered across `zoid-provider`.

**Goal:** one source of truth mapping a model id → its *stable capabilities*,
replacing the scattered per-model string matching (`model_ceiling`,
`default_model`, provider selection) with a single lookup.

**Scope decision (2026-07-01):** **capabilities only — no cost/pricing.** The
economy is denominated in tokens, never dollars, by explicit spec choice
(core §Non-Goals; chat spec §5: "no per-model price tables to track/drift").
Capabilities (`context_window`, `max_output`, `tools`) are *stable* — they
change only when a model is replaced (a new registry entry). Price is a
*volatile external fact* that drifts on every provider reprice; keeping it out
avoids coupling a stable structure to a fast-decaying one. If dollar accounting
is ever wanted, it is a conscious reversal of that spec decision and gets its
own (separately-sourced, separately-updated) table — not this struct.

---

## Data shape

Lives in `zoid-provider` (it already owns the model constants and provider
selection). New module `crates/zoid-provider/src/model.rs`:

```rust
/// Stable, model-agnostic capabilities of one model. NO cost fields by design
/// (the economy is token-denominated — see scope note above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// Canonical id or family key this entry matches.
    pub id: &'static str,
    /// Context-window ceiling in tokens — the economy ⑤ denominator.
    pub context_window: u64,
    /// Max output tokens per turn (request cap). 0 = "use provider default".
    pub max_output: u64,
    /// Whether the model supports tool/function calling.
    pub tools: bool,
}
```

## Lookup semantics

```rust
/// Resolve `model` to its capabilities. Matching is by family substring
/// (case-insensitive), longest/most-specific key first; unknown models fall
/// back to `ModelInfo::DEFAULT`.
pub fn model_info(model: &str) -> ModelInfo;
```

- **Family match, not exact id.** Providers append tags (`glm-5.2:cloud`,
  `claude-sonnet-4-6`); match on a family key (`"glm"`, `"claude"`) so a new
  point release doesn't need a new entry. Order most-specific-first if two
  keys could both match.
- **Default fallback** (`ModelInfo::DEFAULT`): `context_window: 256_000`,
  `max_output: 0` (provider default), `tools: true`. 256k is the conservative
  interim already shipped in `model_ceiling`; under-estimating a warning
  ceiling is the safe direction (warns early vs. silently blowing past the
  real limit).

## Initial table (v1 of the registry)

| family key | context_window | max_output | tools | notes |
|-----------|---------------:|-----------:|:-----:|-------|
| `claude`  | 200_000 | 0 | true | Anthropic Claude (Sonnet/Opus 200k) |
| `glm`     | **TBD**  | 0 | true | GLM cloud (glm-5.2:cloud). "much larger than 200k" per user; fill in the real window when known — until then it takes DEFAULT (256k) |
| *(default)* | 256_000 | 0 | true | unknown models |

> **Action item when building:** replace GLM's `TBD` with its real context
> window (user reports it is substantially larger than 200k). Until then the
> DEFAULT covers it and `ZOID_CONTEXT_CEILING` supplies exact values per-run.

## How existing call sites fold in

1. **`context_ceiling(model)`** → `ZOID_CONTEXT_CEILING` env override wins,
   else `model_info(model).context_window`. (The env override precedence is
   already implemented and stays.) The current `model_ceiling` private fn is
   deleted — its table moves into `model_info`.
2. **`default_model()`** — unchanged in behavior (still env/provider-driven),
   but the provider-family branch can consult the registry if it grows.
3. **Provider tool advertisement** — `tools: false` entries would let the
   agent loop skip sending `tools[]` to models that don't support them. Not
   wired today (all current models support tools); the field exists so the
   gate is a one-line change when a no-tool model appears. Do NOT build the
   gate until such a model exists (YAGNI).

## Non-goals (this iteration)

- No cost/pricing (see scope note).
- No runtime/wire-derived capabilities. A future refinement can populate
  `context_window` from Ollama's `/api/show` (`model_info.*.context_length`)
  or an Anthropic models endpoint, overriding the static table when the
  provider reports a value. Static table first; wire-derivation later.
- No config-file/TOML model definitions (that rides on the §7.1 config work).

## Test plan (when implemented)

- `model_info` family match: `glm-5.2:cloud` → GLM entry; `claude-sonnet-4-6`
  → Claude entry; unknown → DEFAULT; case-insensitive.
- `context_ceiling` precedence: env override beats table; table beats nothing.
- DEFAULT is 256k and `tools: true`.
