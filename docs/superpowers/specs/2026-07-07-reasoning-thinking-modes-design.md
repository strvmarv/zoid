# Reasoning / Thinking Modes

**Date:** 2026-07-07
**Status:** Design

## Overview

Add support for reasoning/thinking modes for models that support them. The
feature is phased: Phase 1 enables thinking at the request level and silently
consumes the reasoning content (the model's answer quality improves, but the
user never sees the intermediate reasoning). Phase 2 (future) layers
visibility — streaming reasoning into the UI as a collapsible section per
assistant turn, with persistence and replay.

The core challenge is **variability**: every provider exposes reasoning
differently — different request parameters, different response formats,
different replay rules, and different control granularities. This design
defines a single provider-agnostic abstraction that maps to each provider's
native shape.

## Research: Per-Provider API Shapes

Verified against official API docs on 2026-07-07.

### Anthropic (Claude)

**Source:** [docs.anthropic.com/en/docs/build-with-claude/extended-thinking](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking)

**Model-dependent behavior** — the config shape depends on the model generation:

| Model | Manual (`budget_tokens`) | Recommended |
|---|---|---|
| Claude Fable 5 / Mythos 5 | Not supported (400) | Adaptive, always on; use `effort` |
| Claude Mythos Preview | Supported | Adaptive, on by default |
| Claude Opus 4.8 / 4.7 / Sonnet 5 | Not supported (400) | Adaptive with `effort` |
| Claude Opus 4.6 / Sonnet 4.6 | Deprecated | Adaptive with `effort` |
| Claude Opus 4.5 / Haiku 4.5 / earlier | Supported | Manual `budget_tokens` |

**Request (older models, budget_tokens):**
```json
{
  "thinking": {"type": "enabled", "budget_tokens": 10000},
  "max_tokens": 16000
}
```
- `budget_tokens` must be < `max_tokens`.
- Can disable: `thinking: {type: "disabled"}`.
- Requires `anthropic-beta: extended-thinking-2025-05-14` header (may not be needed on newer models — verify per model during implementation).

**Request (newer models, adaptive):**
```json
{
  "thinking": {"type": "adaptive"},
  "max_tokens": 16000
}
```
- `effort` field controls depth: `"low"|"medium"|"high"|"max"`.
- `thinking: {type: "disabled"}` → 400 error on Fable 5 / Mythos 5 (can't turn off).
- `display: "summarized"|"omitted"` — `omitted` skips streaming thinking tokens (just signature), reducing latency.

**Response:** `thinking` content blocks with `thinking` text + `signature`.
Streaming: `thinking_delta` and `signature_delta` events.

**Replay rules (critical):**
- During tool use, thinking blocks **MUST be preserved** and passed back unmodified for the last assistant message. The signature is required.
- Can't toggle thinking mid-turn (including during tool-use loops). The entire assistant turn operates in a single thinking mode.
- Graceful degradation: if the conversation history is incompatible (e.g. prior turns lack thinking blocks), the API silently disables thinking for that request rather than erroring.

**max_tokens:** up to 128K on Opus 4.6+/Sonnet 5/Opus 4.8; 64K on Haiku 4.5.

### DeepSeek (V4-pro, V4-flash)

**Source:** [api-docs.deepseek.com/guides/thinking_mode](https://api-docs.deepseek.com/guides/thinking_mode)

**Two layers of control:**

**Request (OpenAI format):**
- Toggle: `{"thinking": {"type": "enabled"|"disabled"}}` (default: `enabled`). Passed in `extra_body` with the OpenAI SDK; top-level key when building raw JSON.
- Effort: `{"reasoning_effort": "high"|"max"}`. Values `low`/`medium` are **mapped to `high`**; `xhigh` is **mapped to `max`**. So effectively two levels: `high` (default for regular requests) and `max` (auto-set for agent clients like Claude Code / OpenCode).

**Request (Anthropic format):** `{"output_config": {"effort": "high"|"max"}}`

**Response:** `reasoning_content` field at the same level as `content` in streamed deltas.

**Replay rules (critical):**
- No tool call between turns → `reasoning_content` from prior turns is **ignored** (don't replay).
- Tool call between turns → `reasoning_content` **MUST be replayed** in all subsequent turns.
- Including `reasoning_content` in input messages when you shouldn't → **400 error**.

**Model differences:**
- `deepseek-v4-flash`: supports both thinking and non-thinking modes (toggle works).
- `deepseek-v4-pro`: **thinking-only** — no toggle (thinking always on).
- `temperature`/`top_p`/`presence_penalty`/`frequency_penalty` silently ignored in thinking mode.
- `max_tokens` includes the CoT (max 64K).

### OpenAI (o-series, GPT-5.x)

**Source:** [platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning)

**Request (Responses API):** `"reasoning": {"effort": "none"|"low"|"medium"|"high"|"xhigh"}`

**Request (Chat Completions API):** `"reasoning_effort": "low"|"medium"|"high"` (fewer levels).

**Effort levels:** `none` (no reasoning), `low`, `medium` (default), `high`, `xhigh` (deep research, very long rollouts).

**Response:** Reasoning is **NOT returned** — internal only. Reasoning tokens count against `max_output_tokens`. Optional `reasoning.summary: "auto"` can return a summary, but not full reasoning.

**Replay:** N/A (reasoning not returned).

### Ollama

**Source:** [ollama.com/blog/thinking](https://ollama.com/blog/thinking)

**Request:** `"think": true|false` in the `/api/chat` body (boolean toggle, no effort levels).

**Response:** `message.thinking` field in NDJSON frames (alongside `message.content`).

**Replay:** None documented. Simple on/off — no budget, no effort.

### Summary Table

| Dimension | Anthropic | DeepSeek | OpenAI | Ollama |
|---|---|---|---|---|
| Toggle | `thinking.type: enabled\|disabled\|adaptive` | `thinking.type: enabled\|disabled` (default: enabled) | `effort: "none"` = off | `think: true\|false` |
| Effort/Budget | Old: `budget_tokens: N`. New: `effort` (adaptive) | `reasoning_effort: high\|max` (low/med→high, xhigh→max) | `effort: none\|low\|medium\|high\|xhigh` | None — on/off only |
| Reasoning returned? | Yes — thinking blocks + signature | Yes — `reasoning_content` field | No — internal only | Yes — `message.thinking` |
| Replay rules | Must preserve thinking blocks during tool use; signature required | No tool call: ignore. Tool call: MUST replay `reasoning_content` | N/A | None |
| max_tokens constraint | `budget_tokens < max_tokens`; can't use `max_tokens:0` | `max_tokens` includes CoT (max 64K) | Reasoning counts against output | None special |
| Beta header? | `anthropic-beta: extended-thinking-…` (may not be needed on newer models) | No | No | No |
| Streaming format | `thinking_delta` + `signature_delta` events | `reasoning_content` in delta chunks | N/A | `message.thinking` in NDJSON |

## Design

### The Provider Seam (`zoid-provider`)

#### `ThinkingMode` on `CompletionRequest`

A new field on the provider-agnostic `CompletionRequest`:

```rust
pub enum ThinkingMode {
    Off,                        // user disabled (today's behavior)
    Auto,                       // default when enabled: derive budget/effort from model + context
    Effort(EffortLevel),        // user explicitly set a level
}

pub enum EffortLevel {
    Low, Medium, High, Max,     // Max = xhigh/max
}
```

`CompletionRequest` gains `pub thinking: ThinkingMode`, defaulting to `Off`.
This means zero behavior change unless thinking is explicitly enabled — the
non-thinking path stays byte-identical.

#### `ThinkingSupport` on `ModelInfo`

The model registry (`zoid-model`) gains a capability flag:

```rust
pub enum ThinkingSupport {
    None,               // model doesn't support thinking
    Toggle,             // on/off only (Ollama)
    ToggleWithEffort,   // on/off + effort levels (DeepSeek, OpenAI, Anthropic new)
    Budget,             // on/off + token budget (Anthropic old — 4.5, earlier)
    Adaptive,           // always-on adaptive, effort controls depth (Anthropic newest)
}
```

`ModelInfo` gains `pub thinking: ThinkingSupport`. Unknown models default to
`None` (thinking disabled, safe — never sends thinking params to a model that
can't handle them).

`ThinkingSupport` tells the agent loop **whether** to offer the thinking knob.
A companion field tells the provider **which native param shape** to emit:

```rust
pub enum ThinkingWireShape {
    None,           // no thinking params on the wire
    Anthropic,      // thinking: {type, budget_tokens?, effort?}
    DeepSeek,       // thinking: {type} + reasoning_effort
    OpenAI,         // reasoning_effort (Chat Completions) / reasoning.effort (Responses)
    Ollama,         // think: bool
}
```

`ModelInfo` gains `pub thinking_wire: ThinkingWireShape`. This lets the
OpenAI-compat request builder distinguish a DeepSeek model
(`ThinkingWireShape::DeepSeek`) from a real OpenAI model
(`ThinkingWireShape::OpenAI`) without substring-matching model ids. Both may
carry `ThinkingSupport::ToggleWithEffort`, but they emit different JSON.

#### `ProviderEvent` — no new variant in Phase 1

Phase 1 does NOT add a `ThinkingDelta` event variant. Each provider's parse
layer consumes and discards reasoning content internally — which is what it
already does today (Anthropic's `parse::event` drops `ThinkingDelta`,
Ollama's `parse_line` drops `message.thinking`). The OpenAI-compat parser
needs to learn to discard `reasoning_content` from streamed deltas (it
already only extracts `delta.content`, so this is mostly an explicit test
guard).

#### `max_tokens` interaction

When thinking is on, `max_tokens` must increase to accommodate reasoning +
answer. The agent loop's `build_request` currently hardcodes `max_tokens:
4096`. When `thinking != Off`, derive `max_tokens` from the model's
`max_output` (or 16384 if `max_output` is 0). When `thinking == Off`, keep
`max_tokens: 4096` (today's behavior, unchanged).

### Per-Provider Request Mapping

Each provider's request builder translates `ThinkingMode` into its native
wire shape. The mapping is owned by the provider, not the agent loop.

#### Anthropic (`anthropic/request.rs`)

The existing `ThinkingConfig` struct extends:

```rust
pub enum ThinkingType {
    Enabled,    // existing — budget_tokens required
    Disabled,   // new
    Adaptive,   // new — for newer Claude models
}

pub struct ThinkingConfig {
    pub r#type: ThinkingType,
    pub budget_tokens: Option<u32>,  // required for Enabled, ignored for Adaptive/Disabled
    pub effort: Option<String>,      // new — "low"|"medium"|"high"|"max" for Adaptive
}
```

The `build()` function maps `ThinkingMode`:
- **`Off`** → `thinking: None` (no thinking key on the wire). For models where `disabled` would 400, omitting the thinking block entirely is the safe path.
- **`Auto`** → depends on model's `ThinkingSupport`:
  - `Budget` models (Sonnet 4.6, Opus 4.5): `type: "enabled", budget_tokens: <derived>`. Derived as `min(max_tokens - 2048, max_tokens * 0.6)` — leaving room for the answer.
  - `Adaptive` models (Opus 4.8): `type: "adaptive"` (no effort, let the model decide).
- **`Effort(level)`** →
  - `Budget` models: map effort to a budget percentage. `Low` → 20%, `Medium` → 40%, `High` → 60%, `Max` → 80% of `max_tokens`.
  - `Adaptive` models: `type: "adaptive", effort: "low"|"medium"|"high"|"max"`.

The `anthropic-beta` header: when thinking is enabled, add
`extended-thinking-2025-05-14` to the betas list if the model needs it (older
models). Newer models with adaptive thinking may not need the beta header —
verify per model during implementation. The existing `with_betas()` mechanism
supports this.

The provider needs to know the model's `ThinkingSupport` to choose between
`Budget` and `Adaptive` mapping. This is looked up from the `MODEL_CAPS`
registry via `model::model_info(req.model).thinking`. The Anthropic provider
already has access to `zoid_model` via the `model` re-export.

#### DeepSeek via OpenAI-compat (`openai_compat.rs`)

The request body gains two optional fields:
- `thinking: {type: "enabled"|"disabled"}` (top-level key)
- `reasoning_effort: "high"|"max"` (top-level key)

Mapping:
- **`Off`** → `thinking: {type: "disabled"}`
- **`Auto`** → `thinking: {type: "enabled"}, reasoning_effort: "high"`
- **`Effort(High)`** → `reasoning_effort: "high"`
- **`Effort(Max)`** → `reasoning_effort: "max"`
- **`Effort(Low|Medium)`** → mapped to `"high"` (DeepSeek collapses these)

Note: `deepseek-v4-pro` is thinking-only (no toggle). For that model, `Off`
silently becomes `Auto` — the provider logs a warning. The `ThinkingSupport`
registry flag for v4-pro is `ToggleWithEffort` but with a note that off isn't
supported; the provider handles the `Off`→`Auto` fallback.

The OpenAI-compat request builder needs to know whether the model is a
DeepSeek reasoning model (to emit `thinking` + `reasoning_effort`) vs a real
OpenAI model (to emit `reasoning_effort` directly) vs a non-reasoning model
(to emit neither). This is driven by the `ThinkingWireShape` field on
`ModelInfo` (defined above): `DeepSeek` → emit DeepSeek shape, `OpenAI` →
emit OpenAI shape, `None` → emit neither. This avoids substring-matching
model ids.

#### OpenAI (`openai_compat.rs`)

- **`Off`** → `reasoning_effort: "none"` (or omit for non-reasoning models)
- **`Auto`** → `reasoning_effort: "medium"` (OpenAI's default)
- **`Effort(level)`** → `Low`→`"low"`, `Medium`→`"medium"`, `High`→`"high"`, `Max`→`"xhigh"`

OpenAI doesn't return reasoning — no parse-side changes needed.

#### Ollama (`ollama.rs`)

- **`Off`** → `think: false` (or omit, since false is the default for non-thinking models)
- **`Auto`** / **`Effort(_)`** → `think: true` (effort level ignored — Ollama has no granularity)

The existing `parse_line` already discards `message.thinking` — no parse-side
change needed.

#### OpenCode-Go (`opencode_go.rs`)

This provider delegates to `OpenAICompatProvider` or `AnthropicProvider` based
on the model's wire shape. `ThinkingMode` flows through `CompletionRequest`
unchanged — the sub-client's request builder handles the mapping. No change
needed beyond ensuring `ThinkingMode` is passed through (it already is, since
it's on `CompletionRequest`).

#### DeepSeek `reasoning_content` parsing

The OpenAI-compat `parse_chunk` needs to discard `reasoning_content` from
streamed deltas. Today it only looks at `delta.content` and `delta.tool_calls`
— `delta.reasoning_content` is already not extracted. Add an explicit test
pinning that `reasoning_content` is discarded, to prevent a future regression.

### Config Layer (`zoid-core`)

#### Config schema

A new `[thinking]` table in `config.toml`:

```toml
[thinking]
enabled = true           # bool, default: false
effort = "high"           # "low"|"medium"|"high"|"max", optional, default: unset (auto)
```

- `enabled = false` (or table absent) → `ThinkingMode::Off`
- `enabled = true`, no `effort` → `ThinkingMode::Auto`
- `enabled = true`, `effort = "high"` → `ThinkingMode::Effort(High)`

#### `Config` struct

```rust
pub struct ThinkingConfig {
    pub enabled: bool,
    pub effort: Option<EffortLevel>,  // None = auto-derive
}
```

Added to `Config` as `pub thinking: ThinkingConfig`, defaulting to `enabled:
false, effort: None`. The `PartialConfig` / `merge` machinery gains a
`thinking` field following the existing pattern (each layer overrides
`enabled` and `effort`, with provenance tracking).

#### Env override

`ZOID_THINKING` env var:
- `ZOID_THINKING=off` → `enabled: false`
- `ZOID_THINKING=auto` → `enabled: true, effort: None`
- `ZOID_THINKING=high` → `enabled: true, effort: High`
- `ZOID_THINKING=max` → `enabled: true, effort: Max`

Follows the existing `ZOID_MODEL` / `ZOID_CONTEXT_CEILING` env-override pattern.

#### Model capability gating

The config screen and quick-switch only show the thinking toggle when the
active model supports it. `ModelInfo.thinking` (the `ThinkingSupport` enum)
drives this:
- `ThinkingSupport::None` → hide the toggle, force `ThinkingMode::Off` regardless of config
- Any other variant → show the toggle

The agent loop's `build_request` resolves the final `ThinkingMode` by
intersecting config with model capability: if `ThinkingSupport::None`,
thinking is forced off even if config says enabled. This is a safety guard.

#### Config UI

The config screen (`config_view::build_sections`) gains:
- `thinking` — a toggle (`FieldKind::Bool`) for enabled/disabled
- `effort` — a picker with options: `(auto)`, `low`, `medium`, `high`, `max` — shown only when thinking is enabled

The `field_target` and `current_write` mappings in `main.rs` gain entries for
`thinking.enabled` and `thinking.effort` following the existing pattern.
Effort is a picker (uses the `ConfigDrillOpen`/`ConfigPickerSelect` path like
provider/model).

### Agent Loop Integration (`zoid/src/agent.rs` + `main.rs`)

#### `build_request` changes

1. **Resolve `ThinkingMode` from config + model capability.** Add a `thinking:
   ThinkingMode` field to `TurnConfig` (which already carries `system`, `cwd`,
   `branch`, `policy`, `eviction`). The turn config is built once per turn in
   `spawn_turn` (in `main.rs`), where config→`ThinkingMode` resolution happens.

2. **`max_tokens` derivation.** When `thinking != Off`, raise `max_tokens`:
   - If `ModelInfo.max_output > 0`, use that.
   - Else use 16384.
   - When `thinking == Off`, keep `max_tokens: 4096` (unchanged).

3. **Pass `thinking` through to `CompletionRequest`:**
   ```rust
   CompletionRequest {
       model: model.to_string(),
       system: Some(system),
       messages: ...,
       max_tokens: <derived>,
       tools: tool_specs(tools),
       thinking: config.thinking,
   }
   ```

#### `spawn_turn` resolution (`main.rs`)

`spawn_turn` builds `TurnConfig` from `app.config` + `app.economy`. It gains:

```rust
let thinking = resolve_thinking(&app.config, &app.model, &app.fetched_model_info);
turn_config.thinking = thinking;
```

Where `resolve_thinking`:
1. Reads `app.config.thinking`.
2. Looks up `ThinkingSupport` from `fetched_model_info` (dynamic) or `MODEL_CAPS` (static fallback).
3. If `ThinkingSupport::None` → `ThinkingMode::Off` (guard).
4. If `enabled: false` → `ThinkingMode::Off`.
5. If `enabled: true, effort: None` → `ThinkingMode::Auto`.
6. If `enabled: true, effort: Some(level)` → `ThinkingMode::Effort(level)`.

#### Subagent turns

Subagents inherit the same thinking config as the main turn — thinking is a
global setting, not per-turn-type. The subagent's `TurnConfig.thinking` is set
from the same `resolve_thinking` call. Per-mode thinking overrides are a future
refinement.

### Replay Semantics — The Phase 1 / Phase 2 Boundary

#### Phase 1 limitation: no reasoning continuity across tool-use loops

Reasoning is not persisted in Phase 1, so it can't be replayed into subsequent
requests. This means:

- **Anthropic**: Each sub-turn within a tool-use loop sends thinking enabled,
  but prior assistant turns lack thinking blocks. The API degrades gracefully —
  thinking may be silently disabled for that request. The model still reasons
  on the *current* sub-turn (where it decides the next tool call or final
  answer), which is where reasoning matters most.
- **DeepSeek**: `reasoning_content` is never replayed (not persisted). For
  non-tool-call turns this is correct (API ignores it). For tool-call turns
  this violates the API contract — the API may return a 400 error. The DeepSeek
  provider's request builder strips `reasoning_content` from all assistant
  messages in the history (the safe path for non-tool turns). If the API 400s
  on tool turns without replayed reasoning, we catch that error and surface it
  as a warning. This is a known DeepSeek-specific risk.
- **Ollama / OpenAI**: Fully correct, no trade-off.

#### Phase 2 seam (what Phase 1 must not block)

Phase 2 adds:
1. `ProviderEvent::ThinkingDelta(String)` — reasoning text streamed from the provider.
2. `EventKind::ModelThinking { text: String }` — persisted to the event log.
3. `ChatMsg::Assistant { thinking: Option<String>, .. }` — the projection carries reasoning.
4. TUI rendering: a collapsible "thinking" section above each assistant message.
5. **Replay**: the provider request builders emit thinking blocks / `reasoning_content` from persisted reasoning when building subsequent requests. For Anthropic, this also requires persisting the `signature`. For DeepSeek, the builder checks whether the assistant turn had tool calls (it did if there are `ToolCall` events for that turn) and conditionally includes `reasoning_content`.

The Phase 1 design must not block these:
- `ThinkingMode` enum and `CompletionRequest.thinking` are stable — Phase 2 adds events and persistence, not request-side changes.
- `Message` struct does NOT need a `thinking` field in Phase 1. Phase 2 adds `pub thinking: Option<String>` (and `pub thinking_signature: Option<String>` for Anthropic). Adding fields later is backwards-compatible.
- `ThinkingSupport` enum is stable — doesn't change between phases.

The request-side abstraction (`ThinkingMode` → native wire shape) is complete
in Phase 1. The response-side (consuming reasoning) is discard-only in Phase 1
and becomes capture-and-persist in Phase 2. The two are decoupled.

## Testing Strategy

### Unit tests per provider (request-side mapping)

Table-driven tests mapping every `ThinkingMode` variant to expected wire JSON:

- **Anthropic**: `Off` → no `thinking` key; `Auto` (Budget model) →
  `thinking: {type: "enabled", budget_tokens: N}` with `N < max_tokens`; `Auto`
  (Adaptive model) → `thinking: {type: "adaptive"}`; `Effort(High)` (Budget
  model) → budget = 60% of max_tokens; `Effort(Max)` (Adaptive model) →
  `effort: "max"`. Verify `anthropic-beta` header is set when thinking is
  enabled on budget models.
- **OpenAI-compat (DeepSeek)**: `Off` → `thinking: {type: "disabled"}`; `Auto`
  → `thinking: {type: "enabled"}, reasoning_effort: "high"`; `Effort(Max)` →
  `reasoning_effort: "max"`; `Effort(Low)` → maps to `"high"`.
- **OpenAI-compat (OpenAI)**: `Off` → `reasoning_effort: "none"` (or omitted);
  `Auto` → `"medium"`; `Effort(Max)` → `"xhigh"`.
- **Ollama**: `Off` → `think: false` (or omitted); `Auto`/`Effort(_)` →
  `think: true`.
- **`max_tokens` derivation**: `Off` → 4096; `Auto` → 16384 (or model's
  `max_output`); verify `budget_tokens < max_tokens` for Anthropic.

### Parse-side discard tests

- **Anthropic**: existing `thinking_delta_emits_nothing` and
  `signature_delta_emits_nothing` tests already pin this. No new tests needed.
- **OpenAI-compat**: new test feeding a chunk with `delta.reasoning_content`
  and asserting it produces no `ProviderEvent`. Key regression guard for
  DeepSeek.
- **Ollama**: existing `thinking_only_line_yields_none` test pins this.

### Config tests

- `ThinkingConfig` parsing from TOML: `enabled = true` without `effort` →
  `Auto`; with `effort = "high"` → `Effort(High)`; `enabled = false` → `Off`
  regardless of effort.
- Env override: `ZOID_THINKING=high` → `Effort(High)`; `ZOID_THINKING=off` →
  `Off`; `ZOID_THINKING=auto` → `Auto`.
- Merge precedence: project layer overrides user-global, env overrides all.

### Capability gating tests

- `resolve_thinking` with `ThinkingSupport::None` → always `Off` even if config
  says enabled.
- `resolve_thinking` with `ThinkingSupport::Toggle` + `Effort(High)` →
  `Effort(High)` (effort ignored by Ollama provider, but the mode passes
  through — the provider decides).
- Model switch from reasoning to non-reasoning model → thinking forced off.

### Agent loop integration test

A `RecordingProvider` test double that delegates to `FakeProvider` but stores
the last `CompletionRequest`'s `max_tokens` and `thinking` mode in a shared
`Arc<Mutex<...>>`. Test: thinking is enabled, the model produces a text
response. Verify `max_tokens` is 16384 (not 4096) and `thinking` is the
expected `ThinkingMode`.

## Scope Boundaries

### In scope (Phase 1)

- `ThinkingMode` / `EffortLevel` enums on `CompletionRequest`.
- `ThinkingSupport` + `ThinkingWireShape` on `ModelInfo`.
- Per-provider request-side mapping (Anthropic, DeepSeek, OpenAI, Ollama).
- Parse-side discard of reasoning content (OpenAI-compat `reasoning_content`).
- `max_tokens` derivation when thinking is on.
- `[thinking]` config table + `ZOID_THINKING` env var.
- Config UI: thinking toggle + effort picker.
- Capability gating: hide/disable thinking for unsupported models.
- `resolve_thinking` in the agent loop.

### Out of scope (Phase 2 — future)

- `ProviderEvent::ThinkingDelta` — streaming reasoning to the UI.
- `EventKind::ModelThinking` — persisting reasoning text.
- TUI rendering of reasoning (collapsible "thinking" sections).
- Replay of thinking blocks / `reasoning_content` in subsequent requests.
- Anthropic signature persistence and replay.
- DeepSeek conditional replay (tool-call-aware `reasoning_content` inclusion).
- Per-mode thinking overrides (different thinking config per zoid mode).
- `display: "omitted"` optimization for Anthropic (skip streaming thinking
  tokens when not surfacing them — a latency optimization for Phase 1 that can
  be added once the request-side is stable).

### Verify-during-implementation

These items need verification against real API docs / live testing during
implementation, not hardcoding from this spec:

- Whether `anthropic-beta: extended-thinking-2025-05-14` is needed for each
  specific Claude model (Sonnet 4.6, Opus 4.8) or whether newer models accept
  thinking without the beta header.
- The exact `budget_tokens` derivation formula (the 60%/40%/20%/80% mapping is
  a reasonable default but should be tuned against real model behavior).
- Whether `deepseek-v4-pro` truly ignores the `thinking: {type: "disabled"}`
  toggle (the docs say it's thinking-only) or returns an error.
- Whether the OpenAI Chat Completions API (vs Responses API) supports
  `reasoning_effort` with the same values, or needs a different parameter
  name/shape. The zoid OpenAI-compat provider uses Chat Completions.