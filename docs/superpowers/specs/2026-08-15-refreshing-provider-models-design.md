# Refreshing Provider Models — Skill Design

**Date:** 2026-08-15
**Branch:** `cc-open-source-zoid-phase-0-2-3a` (open-source initiative)
**Approach:** B — pure agent reference guidance (no script, no binary CLI).

## Problem

The static provider/model registry in `crates/zoid-model/src/lib.rs` drifts
from reality as providers add, rename, and retire models. Every provider already
exposes a live `/models` (or `/api/tags`) endpoint, and the product already
stores the API keys — but there is no documented procedure for an agent to
query those endpoints and reconcile the registry. Agents asked to "update the
available models" have to re-derive endpoints, auth headers, response shapes,
registry invariants, and verification steps from scratch each time, and get
them wrong.

## Solution

A **reference skill** (`refreshing-provider-models`) that gives an agent
everything it needs to refresh the registry in one pass: a per-provider fetch
table (endpoint, auth, response shape), the exact registry locations to edit,
the invariants the existing tests enforce, the `MODEL_CAPS` field reference for
adding new models, and the verification commands to run.

No helper script, no binary CLI surface — the agent runs `curl` with
credentials from env vars, then edits `crates/zoid-model/src/lib.rs` directly.

## Scope

**In scope:**
- The six `selectable` providers in `PROVIDERS`: `ollama-local`,
  `ollama-cloud`, `opencode-go`, `opencode-zen`, `anthropic-api`,
  `zai-coding-plan`.
- Refreshing three targets in `crates/zoid-model/src/lib.rs`:
  1. Each `ProviderEntry.models: &[&str]` array (the model id list).
  2. `ZEN_MODEL_IDS` (the shared Zen gateway id list).
  3. `MODEL_CAPS` — adding a `ModelInfo` entry for any new model id.
- Verifying provider metadata (`base_url`, `key_url`) is still accurate.
- Reading API keys from the env vars the product already uses.

**Out of scope:**
- `local_seed.rs` (`CURATED_LOCAL_MODELS`) — curated local downloads are a
  separate concern.
- The `google_gemini.rs` provider — implemented but not in the `selectable`
  registry; the skill notes it exists but is out of scope.
- Pricing/cost fields — the registry is caps-only by design.
- Modifying the encrypted secret store or config files — env vars only.
- A script file or binary CLI (Approach B; Approach C is a future product
  feature with its own spec/plan/release cycle).

## Phase 1 — Fetch live model lists

For each selectable provider, the agent runs a `curl` GET against the live
model-list endpoint, authenticated with the env var the product already binds
to that provider family. A provider whose key is missing is skipped (nothing
to fetch with).

### Provider fetch table

| Provider id | Secret env var | Endpoint | Auth header | Response JSON path | Registry field to diff |
|---|---|---|---|---|---|
| `ollama-local` | (none — keyless) | `{base}/api/tags` (default `http://localhost:11434`) | Bearer (optional) | `.models[].name` | `ollama-local` models — skip (free-text local tags) |
| `ollama-cloud` | `OLLAMA_API_KEY` | `https://ollama.com/api/tags` | `Authorization: Bearer $KEY` | `.models[].name` | `PROVIDERS` `ollama-cloud` `models: &[…]` — curated (see 2a) |
| `opencode-go` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/go/v1/models` | `Authorization: Bearer $KEY` | `.data[].id` | `PROVIDERS` `opencode-go` `models: &[…]` |
| `opencode-zen` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/v1/models` | `Authorization: Bearer $KEY` | `.data[].id` | `ZEN_MODEL_IDS` static |
| `anthropic-api` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/models` | `x-api-key: $KEY` + `anthropic-version: 2023-06-01` | `.data[].id` | `PROVIDERS` `anthropic-api` `models: &[…]` |
| `zai-coding-plan` | `ZAI_API_KEY` | `https://api.z.ai/api/coding/paas/v4/models` | `Authorization: Bearer $KEY` | `.data[].id` | `PROVIDERS` `zai-coding-plan` `models: &[…]` |

**Notes:**
- `ollama-local` is keyless; its `models: &[]` array is intentionally empty
  (local tags are arbitrary, free-text). Skip diffing it.
- **`ollama-local` and `ollama-cloud` share the same `OllamaProvider` code path**
  (`crates/zoid-provider/src/ollama.rs`), hitting `{base}/api/tags` and parsing
  `.models[].name`. The only differences are the `base_url` and that local is
  keyless. Neither is OpenAI-compat — do not use `/v1/models` or `.data[].id`
  for either Ollama flavor.
- `opencode-go` and `opencode-zen` share the `OPENCODE_GO_API_KEY` secret.
- The endpoint paths and auth headers above are derived from the provider
  constructors' `list_models` implementations (`crates/zoid-provider/src/`):
  - Ollama (local + cloud): `GET {base}/api/tags`, Bearer, response parsed as
    `parse_ollama_tags` (`{"models":[{"name":…}]}`).
  - Anthropic: `GET {base}/v1/models`, `x-api-key` header, `anthropic-version`
    header, response parsed as `parse_anthropic_models` (`{"data":[{"id":…}]}`).
  - OpenAI-compat (opencode-go, opencode-zen, zai): `GET
    {base}{path_prefix}/models`, `Authorization: Bearer`, response parsed as
    `parse_data_id_models` (`{"data":[{"id":…}]}`). ZAI uses `path_prefix=""`
    (its endpoint is `{base}/chat/completions` with no `/v1/` segment), so the
    models endpoint is `{base}/models` (full URL:
    `https://api.z.ai/api/coding/paas/v4/models`).

### Ready-to-run curl examples

```bash
# ollama-cloud (native Ollama API, not OpenAI-compat)
curl -s -H "Authorization: Bearer $OLLAMA_API_KEY" https://ollama.com/api/tags | jq -r '.models[].name'

# opencode-go
curl -s -H "Authorization: Bearer $OPENCODE_GO_API_KEY" https://opencode.ai/zen/go/v1/models | jq -r '.data[].id'

# opencode-zen
curl -s -H "Authorization: Bearer $OPENCODE_GO_API_KEY" https://opencode.ai/zen/v1/models | jq -r '.data[].id'

# anthropic-api
curl -s -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" https://api.anthropic.com/v1/models | jq -r '.data[].id'

# zai-coding-plan
curl -s -H "Authorization: Bearer $ZAI_API_KEY" https://api.z.ai/api/coding/paas/v4/models | jq -r '.data[].id'
```

The agent prints each provider's live id list alongside the current static
array, noting additions (`live ⊃ static`) and removals (`live ⊂ static`).

## Phase 2 — Diff and update the registry

The agent compares each live id list against the static array and edits
`crates/zoid-model/src/lib.rs`:

### 2a. Model id lists (`PROVIDERS` entries + `ZEN_MODEL_IDS`)

- **Add** ids present live but missing from the static array.
- **Remove** ids in the static array but absent live (provider retired them).
- **Preserve `PROVIDERS` order** — it is the picker display order (convention,
  not test-enforced). Insert new ids in a sensible position (group with
  siblings) but do not reorder existing entries.
- **`ollama-local` stays `&[]`** — never populate it; local tags are free-text.
- **`ollama-cloud` is a curated subset, not a live-list mirror** — its static
  array is `&["glm-5.2:cloud"]`, a single `:cloud`-suffixed id with its own
  `MODEL_CAPS` entry. The live `/api/tags` will return many models with `:cloud`
  suffixes. Do not blindly replace the array with the full live list. Preserve
  the `:cloud` suffix convention; any new cloud id added needs a `MODEL_CAPS`
  entry.
- **`ZEN_MODEL_IDS` first entry is the default model** — this is a product
  decision, **not derivable from the live list** (the `/v1/models` endpoint
  returns ids, not which is the gateway default). Do not change the first entry
  without explicit instruction. The first line carries the `// default model`
  comment.
- **Update the `ZEN_MODEL_IDS` count comment** — the `// All NN Zen model ids`
  comment must match the new count after adding/removing ids.
- **Cross-array duplication is expected** — some ids (e.g. `glm-5.2`) appear in
  both `ZEN_MODEL_IDS` and a Go/ZAI `models` array. Each is a separate
  provider's menu; do not remove cross-provider duplicates. Dedup matters only
  within `MODEL_CAPS` (case-insensitive), not across provider id arrays.

### 2b. `MODEL_CAPS` entries for new model ids

For every id that is **new** (not already in `MODEL_CAPS`, which is
case-insensitive), add an entry. Research the capabilities from the provider's
official docs. **All unknowns fall back to `DEFAULT_MODEL_INFO`** (`lib.rs:640`):
32k context / 0 max_output / tools=true / prompt_cache=false / thinking=None /
thinking_wire=None. That's the conservative floor for truly unknown models.

**Exception — selectable Zen and Go models:** the test
`opencode_zen_model_caps_present` asserts every `opencode-zen` model has
`context_window >= 128_000` (not the 32k `DEFAULT_MODEL_INFO` floor). A new Zen
or Go id **must** have an explicit `MODEL_CAPS` entry with a researched context
window ≥128k — the 32k default is not acceptable for selectable Zen/Go models.

`ModelInfo` field reference (see the `ModelInfo` struct, `lib.rs:15-26`):

| Field | Type | Meaning |
|---|---|---|
| `context_window` | `u64` | Max input+output tokens the model accepts. |
| `max_output` | `u64` | Max output tokens; `0` = use provider default. |
| `tools` | `bool` | Model supports tool/function calling. |
| `prompt_cache` | `bool` | Provider reports token-level prompt cache (cache-read tokens). |
| `thinking` | `ThinkingSupport` | `None` / `Toggle` / `ToggleWithEffort` / `Budget` / `Adaptive`. |
| `thinking_wire` | `ThinkingWireShape` | `None` / `Anthropic` / `DeepSeek` / `OpenAI` / `Ollama`. |

**`thinking_wire` is a per-model researched property, not derivable from
provider family.** Many models routed through `AnthropicProvider` on the
Go/Zen gateways have `thinking_wire: None` (e.g. `minimax-m3`, `qwen3.7-max`,
all Zen Anthropic-Messages leaf models like `claude-sonnet-4-5`). Only the two
direct `anthropic-api` models (`claude-sonnet-4-6`, `claude-opus-4-8`) use
`ThinkingWireShape::Anthropic`. When adding a new model: copy `thinking` and
`thinking_wire` from a researched sibling of the same model family/variant
where one exists in `MODEL_CAPS`; otherwise `None` unless the provider's docs
confirm a thinking wire shape.

Do not duplicate a `MODEL_CAPS` entry — lookup is case-insensitive, so a
duplicate silently shadows.

### 2c. Provider metadata sanity check

While the file is open, verify each `ProviderEntry`'s:
- `transport` `default_base_url` still resolves (the fetch in Phase 1 proved
  the endpoint reachable).
- `key_url` link is still valid. The test
  `key_url_field_present_on_all_providers` is keyed on provider **id**, not
  "key-requiring": `ollama-local` must be `None`, **all others must be
  `Some(_)`**. Changing any provider's `key_url` to `None` (other than
  `ollama-local`) will fail the test.
- `status` is `Available` (if a provider has gone dark, flag it but do not
  remove without confirmation).

## Phase 3 — Verify

Run the registry's own tests as the gate:

```bash
cargo test -p zoid-model
```

Key invariants these tests enforce:
- `selectable_has_six_providers` — exactly six selectable providers (don't
  add/remove `ProviderEntry` rows without updating this test).
- `opencode_go_entry_unchanged` — `opencode-go` has exactly 13 models; adding
 /removing a Go id requires updating this count.
- `opencode_zen_model_caps_present` — every `opencode-zen` model has an explicit
  `MODEL_CAPS` entry with `context_window >= 128_000` (the 32k default is not
  acceptable for selectable Zen models).
- `opencode_go_model_caps_match_reconciled_table` / `opencode_zen_caps_match_table`
  — exact-value lock tests for Go (13) and Zen (39) model caps. These iterate
  their own hardcoded `cases` arrays, so they don't auto-catch a *new* id, but
  they will fail if an existing model's caps change.
- `key_url_field_present_on_all_providers` — `ollama-local` is the sole `None`;
  all others must be `Some(_)`.
- `models_for_by_id_and_alias` — `models_for(id)` returns the right array.
- `model_info_*` — `model_info(id)` returns the right `ModelInfo`.
- `model_info_unknown_falls_back_to_conservative_default` — unknown id → 32k,
  no cache.

Then confirm the provider re-exports still compile:

```bash
cargo build -p zoid-provider
```

If a `MODEL_CAPS` entry was added or a `models` array changed, also run the
provider crate's tests:

```bash
cargo test -p zoid-provider
```

**Wire-shape routing tables (important):** Adding a new id to `ZEN_MODEL_IDS`
requires a corresponding entry in `opencode_zen.rs::ZEN_MODELS` (the
`ZenWireShape` routing table), or `wire_shape_for` silently defaults to
`OpenAIChat` with a `tracing::warn` — the model routes to the wrong wire shape
with no test failure. Likewise, adding a new id to `opencode-go`'s `models`
array requires a corresponding entry in `opencode_go.rs::GO_MODELS` (the
`WireShape` routing table), or it defaults to `OpenAICompat`. These routing
tables are **separate** from the registry's `models` arrays and must be updated
in the provider crate (`crates/zoid-provider/src/`).

## Skill file organization

```
refreshing-provider-models/
  SKILL.md   # Everything inline (reference table + phases + curl examples)
```

Self-contained — no supporting files. The fetch table, curl examples, field
reference, and verification commands all fit inline.

## Frontmatter

```yaml
---
name: refreshing-provider-models
description: Use when refreshing zoid's static provider/model registry against
  live provider endpoints, adding new models to MODEL_CAPS, or reconciling
  model id drift across the supported providers
---
```

## Testing the skill (RED → GREEN)

### RED — baseline (without skill)

Dispatch a subagent with the task: "Refresh the provider model registry in
`crates/zoid-model/src/lib.rs` from the live provider endpoints. Update model
ids, MODEL_CAPS, and verify." Observe and document:
- Does it guess the right endpoints and auth headers?
- Does it preserve `PROVIDERS` order?
- Does it duplicate `MODEL_CAPS` entries?
- Does it populate `ollama-local`'s empty array?
- Does it run the right verification commands?
- Does it know the secret env var names?

### GREEN — with skill

Same task, skill loaded. The agent should:
1. Run the curl commands from the table (skipping providers with missing keys).
2. Diff live vs static id lists.
3. Edit `PROVIDERS`/`ZEN_MODEL_IDS` preserving order and invariants.
4. Add `MODEL_CAPS` entries for new ids using the field reference.
5. Run `cargo test -p zoid-model` and `cargo build -p zoid-provider`.

### REFACTOR

If the agent finds a new rationalization (e.g. "I'll just add the model without
researching caps — the default is fine"), add an explicit counter to the skill.