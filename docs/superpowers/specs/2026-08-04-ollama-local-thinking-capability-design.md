# Ollama Local Thinking Capability

## Problem

`qwythos:latest` supports thinking — Ollama's `/api/show` reports the `thinking`
capability — but zoid sends `think: false` to it on every turn. The model is
given no reasoning scratchpad, so it spends its turn narrating intent in prose
and stops without calling a tool.

### Empirical evidence

Queried session `01KZ7DA32AVGKZGECQFWK287A4` (qwythos, 2026-08-04). Turn 1:

| ts              | event                          | notes                              |
|-----------------|--------------------------------|------------------------------------|
| 1785881500394   | UserMessage "summarize this repo" |                                    |
| 1785881509430+  | 39 ModelDelta fragments (~40 tokens) | "I'll read the repository's README…" |
| 1785881510317   | Usage `{"input":3692,"output":40,"thinking":0}` | model emitted `done:true` |
| (35s gap)       |                                | user had to type "continue"        |
| 1785881546316   | UserMessage "continue"         | only then did qwythos call `ls`    |

Zero `ThinkingDelta` events in the entire session. The model ended its turn
(`done:true`, `thinking: 0`) after a 40-token preamble with no tool call — the
"announce-then-stop" failure mode of a chatty small model denied a reasoning
channel.

### Root cause (two compounding defects)

**1. `fetch_model_info` discards the daemon's `thinking` capability.**
`ollama.rs:507` already calls `/api/show` to read the context window. The same
response body carries a top-level `capabilities` array (e.g.
`["completion","tools","thinking","vision"]`). But `fetch_model_info` hardcodes
`thinking: ThinkingSupport::None` (ollama.rs:530), so the `ModelInfo` stored on
`app.fetched_model_info` always reports "no thinking" regardless of what the
daemon says.

**2. The thinking default is `false`.** `ThinkingConfig` derives `Default`
(config.rs:14); `bool` defaults to `false`. A fresh `ollama-local` user with no
`[thinking]` section gets thinking off, even for a capable model. This default
is correct for cloud providers (thinking costs money/tokens on Anthropic/GLM)
but wrong for local models, where thinking is free and the model was chosen
*because* it reasons.

### Why the obvious fix doesn't work

The "obvious" fix — flip the `ollama-local` config default to `true` at the
point where the merged config is finalized — fails on inspection. There is no
single such site: the `context_target` clamp is duplicated across boot
(main.rs:2491) and config-reload (main.rs:4254), and `load_config` (where
provenance is computed, main.rs:215) is a free function with no provider
context. A config-mutation approach would require applying the flip at both
sites and keeping the invariant "every `app.config` mutation routes through the
flip" by convention — a footgun. Worse, provenance tracks the `enabled` *key*
(config.rs:771), not the `[thinking]` section, so a user who writes
`[thinking]\neffort = "high"` with no `enabled` key has
`thinking_enabled == Source::Default`, and a naive "absent section" gate
misfires. The fix belongs in `resolve_thinking`, the single function both call
sites already pass through.

### Why the existing plumbing doesn't already fix this

The infrastructure for "fetched capability overrides the static table" is fully
built and wired:

1. `spawn_model_info_fetch` (main.rs:1186) calls `provider.fetch_model_info` at
   boot.
2. `AgentUpdate::ModelInfoFetched` stores the result on
   `app.fetched_model_info` (main.rs:3743).
3. `resolve_thinking` is called with `app.fetched_model_info.map(|i| i.thinking)`,
   **preferring the fetched value over the static `MODEL_CAPS` table**
   (main.rs:7434-7437).

The only gap is that the fetched value is always `None`. The daemon knows the
answer; zoid asks for it, receives it, and throws it away.

## Design

### Decision: trust the daemon for local models (option A)

For `ollama-local`, the daemon is the source of truth for model capabilities.
Local tags (`qwythos:latest`, `hf.co/.../Q4_K_M`, free-text) are never in the
static `MODEL_CAPS` registry, so the fetched value is the only signal. There is
no static-table entry to conflict with, and no "known-bad-thinking" local model
to protect against — if a model thinks badly, the user disables it with
`[thinking] enabled = false`, which is the existing, documented override and the
first thing `resolve_thinking` checks.

### Decision: provider-aware thinking default (option B2)

The `ollama-local` default for `thinking.enabled` becomes `true` when the
`[thinking]` section is absent. This is a provider-aware adjustment at the
config-finalization layer — not a change to `ThinkingConfig`'s `Default` derive
(cloud providers keep `false`), and not a change to `resolve_thinking` (which
stays pure and provider-agnostic). The pattern mirrors `wake.enabled`, which
already defaults `true` and is overridden by users who want it off.

This only affects *new* setups with no `[thinking]` section. Users with an
explicit `[thinking] enabled = false` keep thinking off; users with
`enabled = true` (the current qwythos config) are unaffected — they already get
thinking once defect 1 is fixed.

### Change 1: `fetch_model_info` reads the `capabilities` array

New pure helper, mirroring the existing `parse_ollama_context_window` (same
file, same lenient contract — unknown/!json → `None`, never panics):

```rust
/// Parse the Ollama `/api/show` `capabilities` array for thinking support.
/// Returns `ThinkingSupport::Toggle` when the array contains `"thinking"`,
/// `None` otherwise (including absent, non-array, or malformed). Lenient:
/// the caller falls back to "no thinking" on any parse failure.
pub fn parse_ollama_thinking(body: &str) -> ThinkingSupport {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return ThinkingSupport::None,
    };
    let caps = match v.get("capabilities").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return ThinkingSupport::None,
    };
    if caps.iter().any(|c| c.as_str() == Some("thinking")) {
        ThinkingSupport::Toggle
    } else {
        ThinkingSupport::None
    }
}
```

`fetch_model_info` (ollama.rs:518) gains one line alongside the existing context
window parse:

```rust
let thinking = parse_ollama_thinking(&body);
Ok(window.map(|w| crate::model::ModelInfo {
    context_window: /* unchanged num_ctx clamp */,
    max_output: 0,
    tools: true,
    prompt_cache: true,
    thinking,
    thinking_wire: crate::model::ThinkingWireShape::None,
}))
```

`ThinkingSupport::Toggle` is the correct variant: the Ollama native API's
`think` field is a boolean toggle (no budget/effort knob), which is exactly what
`Toggle` means in the model registry (contrast `Budget` for Anthropic,
`Adaptive` for opus, `ToggleWithEffort` for OpenAI-compat thinking models).

Note: `fetch_model_info` returns `None` (not an error) when `/api/show` omits
the context window — `window.map(...)` means a body with capabilities but no
context window still yields `None`. This is the **existing behavior** and is
out of scope here; the context-window parse already gates the whole result. If a
model reports `thinking` but not a context window, the static fallback applies
and thinking stays off. Acceptable: every real Ollama model reports both.

### Change 2: provider-aware thinking default inside `resolve_thinking`

The provider-aware default lives in `resolve_thinking` itself — the single
function both call sites (turn, main.rs:7437; subagent, main.rs:7339) already
pass through. The signature widens to take the provenance and provider:

```rust
fn resolve_thinking(
    config_thinking: &zoid_core::config::ThinkingConfig,
    thinking_enabled_src: zoid_core::config::Source,
    provider: &str,
    model_support: zoid_provider::model::ThinkingSupport,
) -> zoid_provider::ThinkingMode
```

The new logic, applied **before** the existing capability gate: when
`thinking_enabled_src == Source::Default` (the user set no `enabled` key in any
layer) and `provider == "ollama-local"`, treat `enabled` as `true`. Then the
existing match on `model_support` runs unchanged — if the fetched capability is
`None` (non-thinking model, or daemon-down so the static fallback applies),
`Off` is still returned regardless of the default. The default makes thinking
*available*; the capability gate decides whether it's *used*.

```rust
fn resolve_thinking(
    config_thinking: &zoid_core::config::ThinkingConfig,
    thinking_enabled_src: Source,
    provider: &str,
    model_support: ThinkingSupport,
) -> ThinkingMode {
    // Effective enabled flag: the user's value, or true for ollama-local when
    // the user set no [thinking].enabled key (provenance Default). An explicit
    // enabled = false (provenance != Default) always wins.
    let enabled = if thinking_enabled_src == Source::Default
        && zoid_provider::model::canonical_id(provider) == "ollama-local"
    {
        true
    } else {
        config_thinking.enabled
    };
    match model_support {
        ThinkingSupport::None => ThinkingMode::Off,
        _ if !enabled => ThinkingMode::Off,
        _ => match &config_thinking.effort {
            None => ThinkingMode::Auto,
            Some(e) => /* existing effort mapping */,
        },
    }
}
```

**Why `Source::Default` is the right gate, not "section absent."** Provenance
tracks the `enabled` key (config.rs:771), not the `[thinking]` section. A user
who writes `[thinking]\neffort = "high"` with no `enabled` key has
`thinking_enabled == Source::Default` but `thinking_effort != Source::Default`
— they touched the section but left `enabled` to defaults. Gating on
`thinking_enabled == Source::Default` means: "the user did not explicitly
choose an `enabled` value." That is exactly the case where a provider-aware
default is appropriate. The user's `effort = "high"` still flows through (the
effort match runs after the enabled check), so an `effort`-only config on
`ollama-local` yields `enabled = true` (provider default) + `Effort(High)`
(user value) — which is what an `effort`-only config means: "I want
high-effort thinking; leave the master switch to the default."

**Why this is better than a config-mutation approach.** Both call sites have
`app` in scope, so passing `app.prov.thinking_enabled` and `&app.config.provider`
is trivial. There is no boot-vs-reload duplication — both already route through
`resolve_thinking`. The subagent path (main.rs:7339) is correct by construction
(same function, same args) rather than by coincidence. And the "remember to
mutate `app.config` before every read" footgun disappears entirely.

### What does not change

- **`resolve_thinking`'s contract** — still forces `Off` when
  `ThinkingSupport::None`; still honors an explicit `enabled = false`. The
  signature widens (two new args: `thinking_enabled_src`, `provider`) and gains
  the provider-aware default as a leading clause, but the capability gate and
  effort mapping are unchanged. It remains pure (no IO, no global state).
- **`request_body`'s defensive re-check** (ollama.rs:86) — stays as the
  last line of defense against `think: true` reaching a non-thinking model.
- **The static `MODEL_CAPS` table** — untouched. Local tags are never in it.
- **The user override** — `[thinking] enabled = false` works for any user, any
  provider, any model. `thinking_enabled_src != Source::Default` → the
  provider-aware default is skipped and the user's `false` wins.
- **`ZOID_THINKING` env** — `parse_thinking_env` (main.rs:6850) produces a
  `PartialThinking` with `enabled: Some(...)`, which the merge records with
  `Source::Env` (config.rs:773). That is `!= Source::Default`, so the
  provider-aware flip is correctly skipped — env wins, as it should.
- **Non-`ollama-local` providers** — completely unaffected. The provider-aware
  default is gated on `canonical_id(provider) == "ollama-local"`; the
  `fetch_model_info` new parse runs only for the Ollama provider.

### Data flow (after)

```
boot:  spawn_model_info_fetch → POST /api/show
         → parse_ollama_context_window (existing) + parse_ollama_thinking (new)
         → ModelInfo { thinking: Toggle }
         → AgentUpdate::ModelInfoFetched → app.fetched_model_info = Some(info)

turn:   resolve_thinking(
           &app.config.thinking,        // { enabled: false, effort: None } (Default config)
           app.prov.thinking_enabled,   // Source::Default (user wrote no [thinking].enabled)
           &app.config.provider,        // "ollama-local"
           Toggle,                      // from app.fetched_model_info
         )
         → enabled = true (provider-aware default) → ThinkingMode::Auto
         → request_body: think = (info.thinking != None) = true
         → POST /api/chat { "model":"qwythos:latest", "think": true, ... }
         → qwythos reasons in the thinking channel, then calls a tool

       (user who set [thinking] enabled = false:)
         thinking_enabled_src = Source::UserGlobal → enabled = false → Off
       (daemon-down, capability unknown:)
         fetched_model_info = None → model_support = None (static fallback)
         → resolve_thinking returns Off regardless of the default → think: false
```

### Error handling

- **`/api/show` fails or omits `capabilities`:** `parse_ollama_thinking` returns
  `ThinkingSupport::None` (lenient). The existing static-table fallback applies
  — thinking stays off. No regression vs. today.
- **`/api/show` reports `thinking` but the model thinks badly** (hallucinated
  reasoning, wasted budget): user sets `[thinking] enabled = false`. The
  existing override; `resolve_thinking` honors it first.
- **`/api/show` reports `thinking` but no context window:** existing
  `window.map(...)` gate means the whole `ModelInfo` is `None`; static fallback
  applies, thinking stays off. Acceptable (see Change 1 note).
- **Network error during `fetch_model_info`:** already non-fatal
  (`spawn_model_info_fetch` swallows errors, keeps the static fallback). The
  provider-aware default (`enabled = true`) still flows to `resolve_thinking`,
  but the `None` capability (from the static `DEFAULT_MODEL_INFO` fallback)
  immediately returns `Off` regardless. Safe: a daemon-down boot doesn't send
  `think: true` to a model whose capabilities are unknown. (Note: on this path
  `enabled = true` does nothing useful — the capability gate is the entire
  safety story.)

### Testing

- **`parse_ollama_thinking`** — unit tests against fixture `/api/show` bodies:
  capabilities with `thinking`, without, absent array, `null`, malformed JSON.
  Mirrors the existing `parse_ollama_context_window` tests (ollama.rs test
  module). Never panics on any input.
- **`fetch_model_info`** — the returned `ModelInfo.thinking` is `Toggle` when
  the capabilities array includes `thinking`, `None` otherwise. Test via a
  mock response body (the existing `fetch_model_info` tests already use fixture
  bodies for the context-window path).
- **`resolve_thinking` provider-aware default** — pure unit tests (the function
  is pure, so no `App` needed). Cases:
  - `Source::Default` + `ollama-local` + `Toggle` → `Auto` (default flips on).
  - `Source::Default` + `ollama-cloud` + `Toggle` → `Off` (cloud keeps `false`).
  - `Source::Default` + `ollama-local` + `None` → `Off` (capability gate wins).
  - `Source::UserGlobal` (explicit `enabled = false`) + `ollama-local` + `Toggle`
    → `Off` (user override wins).
  - `Source::Env` (`ZOID_THINKING=off`) + `ollama-local` + `Toggle` → `Off`.
  - `Source::Default` + `ollama-local` + `Toggle` + `effort = "high"` →
    `Effort(High)` (effort flows through; the `effort`-only section case).
- **`resolve_thinking` regression** — the existing tests (main.rs:7499-7547)
  update to the widened signature (add `Source::Default`, `"anthropic-api"` as
  the provider arg, keeping their original intent) and stay green. The
  capability-gate and effort-mapping behavior is unchanged.

## Scope

One provider's `fetch_model_info` gains a one-line parse; `resolve_thinking`
widens its signature with two args and gains a provider-aware default clause
(`ollama-local` only, gated on `Source::Default`). No new provider, no registry
edits, no protocol changes, no UI changes, no config-mutation footgun. The
turn-1 stall falls out: qwythos gets `think: true`, gets a reasoning scratchpad,
and stops spending its turn on preamble prose.