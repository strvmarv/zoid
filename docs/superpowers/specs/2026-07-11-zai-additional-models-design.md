# ZAI Coding Plan: Add glm-5-turbo and glm-4.7 Models

> **Date:** 2026-07-11
> **Status:** Approved
> **Predecessor:** `2026-07-10-zai-coding-plan-provider-design.md` (initial provider + glm-5.2)

## Goal

Add the two remaining GLM Coding Plan models — `glm-5-turbo` and `glm-4.7` — to the `zai-coding-plan` provider entry, with MODEL_CAPS confirmed via live API probing.

## Context

The ZAI Coding Plan endpoint (`https://api.z.ai/api/coding/paas/v4`) officially supports exactly three models (per [docs.z.ai FAQ](https://docs.z.ai/devpack/faq)): GLM-5.2, GLM-5-Turbo, and GLM-4.7. Only glm-5.2 is currently registered. This change adds the other two.

No provider logic changes are needed — `ZaiProvider` delegates to `OpenAICompatProvider` with `path_prefix=""` and works for any model on the endpoint. The `family`-based dispatch in `main.rs` already handles any model in a provider entry's `models` array.

## Live API Probing Results

All three models use the **DeepSeek wire shape** for thinking:

| Param combination | Behavior |
|---|---|
| No thinking params (bare request) | Models think by default (emit `reasoning_content` + `reasoning_tokens`) |
| `thinking: {type:"disabled"}` | No reasoning, clean content output |
| `thinking: {type:"enabled"}` + `reasoning_effort:"high"` | Reasoning enabled, emits `reasoning_content` |

**Implication for zoid:** Our `thinking_params()` function always sends an explicit `thinking` param (either `disabled` or `enabled`), so the "thinks by default" bare-request behavior is never triggered. `ThinkingMode::Off` correctly sends `thinking: {type:"disabled"}`, which suppresses reasoning for both models.

## Model Capabilities

Confirmed via OpenRouter, docs.z.ai, and ZAI marketing pages:

| Model | Context Window | Max Output | Tools | Prompt Cache | Thinking | Wire Shape |
|---|---|---|---|---|---|---|
| `glm-5-turbo` | 262,144 | 131,072 | true | true | `ToggleWithEffort` | `DeepSeek` |
| `glm-4.7` | 200,000 | 131,072 | true | true | `ToggleWithEffort` | `DeepSeek` |

### Source citations

**glm-5-turbo:**
- OpenRouter (`openrouter.ai/z-ai/glm-5-turbo`): 262,144 context, 131,072 max output
- Live API probe confirmed DeepSeek wire shape (thinking + reasoning_effort)
- Described as Opus-level, optimized for OpenClaw scenarios, fast inference

**glm-4.7:**
- OpenRouter (`openrouter.ai/z-ai/glm-4.7`): 202,752 context, 131,072 max output
- Macaron blog (`macaron.im/blog/what-is-glm-4-7`): 200K context, 128K output, 358B params
- Live API probe confirmed DeepSeek wire shape (thinking + reasoning_effort)
- Described as Sonnet-level, enhanced coding + multi-step reasoning, SWE-bench 73.8%
- Context window: using 200,000 (the marketed figure; OpenRouter's 202,752 is likely the raw KV cache ceiling)

**Max output note:** Both models report 131,072 max output. This matches the GLM-5.2 figure and is the actual API ceiling (not the marketed "128K" rounding).

## Changes

### 1. MODEL_CAPS — add two entries

**File:** `crates/zoid-model/src/lib.rs`

Add two entries to the `MODEL_CAPS` const array (after the existing `glm-5.2` entry, before `glm-5.1`):

```rust
// glm-5-turbo: GLM-5 family fast variant, ZAI Coding Plan model.
(
    "glm-5-turbo",
    ModelInfo {
        context_window: 262_144,
        max_output: 131_072,
        tools: true,
        prompt_cache: true,
        thinking: ThinkingSupport::ToggleWithEffort,
        thinking_wire: ThinkingWireShape::DeepSeek,
    },
),
// glm-4.7: Sonnet-level model, ZAI Coding Plan model.
(
    "glm-4.7",
    ModelInfo {
        context_window: 200_000,
        max_output: 131_072,
        tools: true,
        prompt_cache: true,
        thinking: ThinkingSupport::ToggleWithEffort,
        thinking_wire: ThinkingWireShape::DeepSeek,
    },
),
```

### 2. PROVIDERS — expand zai-coding-plan models

**File:** `crates/zoid-model/src/lib.rs`

Update the `zai-coding-plan` entry's `models` array:

```rust
models: &["glm-5.2", "glm-5-turbo", "glm-4.7"],
```

`glm-5.2` stays as the first entry (default model).

### 3. Tests

**File:** `crates/zoid-model/src/lib.rs`

Update `zai_coding_plan_registry_entry_exists_and_is_selectable` test — change the models assertion from `&["glm-5.2"]` to `&["glm-5.2", "glm-5-turbo", "glm-4.7"]` and add `len() == 3` check.

Add a regression lock test for each new model's capabilities.

**File:** `crates/zoid-provider/src/openai_compat.rs`

Add thinking wire-shape tests for each new model (mirroring the existing `glm_5_2_thinking_*` tests):
- `glm_5_turbo_thinking_off_emits_disabled_no_effort`
- `glm_5_turbo_thinking_auto_emits_enabled_high`
- `glm_4_7_thinking_off_emits_disabled_no_effort`
- `glm_4_7_thinking_auto_emits_enabled_high`

### What does NOT change

- `ZaiProvider` — no changes; already works for any model on the endpoint
- `main.rs` — no changes; the `family`-based dispatch handles any model in the entry
- `config_view.rs` — no changes; provider picker count stays at 5
- TUI snapshots — no changes; provider picker still shows 5 providers

## Out of scope

- GLM-4.5, GLM-4.5-Air, or other ZAI models not in the Coding Plan
- Per-model quota/cost tiering (the Coding Plan handles this server-side)
- Model ordering strategy beyond "flagship first"
