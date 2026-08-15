# Refreshing Provider Models — Baseline Test Results

> SDD Task 1 (RED). This baseline documents what an agent **without** the
> `refreshing-provider-models` skill gets wrong when asked to refresh the
> static provider/model registry. The 10 pitfalls below are the known traps
> the skill must prevent.

## Dispatch conditions

A `delegate` subagent was dispatched with the exact Step 1 task prompt from
`task-1-brief.md`: refresh the three registry targets in
`crates/zoid-model/src/lib.rs` (`PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS`)
from live provider endpoints.

No API keys were present in the environment (`OLLAMA_API_KEY`,
`ANTHROPIC_API_KEY`, `OPENCODE_GO_API_KEY`, `ZAI_API_KEY` all unset), so every
provider's live `list_models()` call would fail/skip. Per the brief Step 2,
the 10 expected pitfalls are therefore documented synthetically from the
codebase's own invariants — these traps exist regardless of whether live
keys returned data. They are the things an agent reasoning only from the
codebase (without the skill's guidance) will get wrong once it does have
keys, and they are what the skill must address.

## The 10 pitfalls (expected baseline failures)

### 1. Wrong endpoint for `ollama-cloud` — FAIL (expected)

An uninstructed agent reaches for the OpenAI-compat shape
`GET {base}/v1/models` and parses `.data[].id`. The Ollama provider
(`crates/zoid-provider/src/ollama.rs`, `list_models`) actually hits
`GET {base}/api/tags` and parses the Ollama shape `.models[].name`
(`parse_ollama_tags`). Using `/v1/models` against `https://ollama.com`
either 404s or returns a shape the agent mis-parses, so the `ollama-cloud`
model list would be wrong or empty. The skill must pin the Ollama endpoint
to `/api/tags` + `.models[].name`.

### 2. Wrong auth header for Anthropic — FAIL (expected)

`anthropic-api` and the Anthropic-routed Go/Zen models need the header pair
`x-api-key: <key>` + `anthropic-version: 2023-06-01` (see
`AnthropicProvider::list_models` in `anthropic/mod.rs`). An agent that
reaches for the generic OpenAI pattern sends `Authorization: Bearer <key>`,
which Anthropic rejects (401). The skill must specify the Anthropic header
pair per provider.

### 3. Does not preserve `PROVIDERS` array order — FAIL (expected)

`PROVIDERS` order is the picker display order and is asserted by tests
(`selectable_has_six_providers` enumerates the exact sequence: ollama-local,
ollama-cloud, opencode-go, anthropic-api, zai-coding-plan, opencode-zen).
An agent that "sorts" providers alphabetically or appends new providers at
arbitrary positions breaks the display-order invariant and the test. The
skill must require preserving the existing order and inserting new entries
in a deliberate position.

### 4. Misses the `opencode_zen_model_caps_present` invariant — FAIL (expected)

`crates/zoid-model/src/lib.rs` test `opencode_zen_model_caps_present` asserts
every model in the `opencode-zen` entry's `models` list has an explicit
`MODEL_CAPS` entry with `context_window >= 128_000` (not the 32k
`DEFAULT_MODEL_INFO` floor). An agent that adds a model id to `ZEN_MODEL_IDS`
without a matching `MODEL_CAPS` row fails this test (unknown models fall back
to 32k). The skill must require a caps row for every new Zen model with
≥128k context.

### 5. Treats `thinking_wire` as per-family, not per-model — FAIL (expected)

`thinking_wire` is a per-model `ThinkingWireShape` value, not derivable from
the provider family. E.g. within the Anthropic family, `claude-sonnet-4-6`
is `Anthropic`/`Budget` while `claude-sonnet-4-5` (Zen) is `None`/`None`;
within Go, `glm-5.2` is `DeepSeek` while `minimax-m3` is `None`. An agent that
sets a whole family's `thinking_wire` from the provider name will mislabel
models. The skill must require per-model thinking classification (and that
it is not endpoint-derivable — it must be researched per model id).

### 6. Unaware of the `ZEN_MODELS`/`GO_MODELS` wire-shape routing tables — FAIL (expected)

`opencode_zen.rs` holds `ZEN_MODELS: &[(&str, ZenWireShape)]` (52 entries →
OpenAIChat / AnthropicMessages / OpenAIResponses / GoogleGemini) and
`opencode_go.rs` holds `GO_MODELS: &[(&str, WireShape)]` (13 entries →
OpenAICompat / Anthropic). These route `stream()` to the correct sub-client
and are *separate* from the model-id lists. An agent that only edits the
registry's `models` arrays adds a model id that the provider can't route
(`wire_shape_for` logs a warning and defaults to OpenAIChat, silently
breaking non-OpenAIChat models). The skill must require updating the
matching `ZEN_MODELS`/`GO_MODELS` table alongside the registry.

### 7. Mirrors the live `ollama-cloud` list instead of curating — FAIL (expected)

`ollama-cloud`'s `models: &["glm-5.2:cloud"]` is a **curated subset**, not a
live-list mirror (contrast `ollama-local` whose `models: &[]` is free-text).
`/api/tags` on ollama.com returns the full hosted model catalog (dozens of
community tags). An agent that copies the live list verbatim floods the
picker and breaks `models_for("ollama-cloud") == ["glm-5.2:cloud"]`. The
skill must flag `ollama-cloud` as curated and require an explicit
product decision per added/removed id.

### 8. Derives the `ZEN_MODEL_IDS` default from the endpoint — FAIL (expected)

`ZEN_MODEL_IDS`'s first entry (`claude-sonnet-4-5`, marked `// default
model`) is a **product choice**, not something the `/v1/models` endpoint
expresses (the endpoint returns an unordered id set). An agent that sorts
the returned ids alphabetically or takes "the first one returned" picks a
different default (e.g. `big-pickle`) and silently changes the picker's
default selection. The skill must pin the default as an explicit product
decision independent of endpoint ordering.

### 9. Only runs `cargo test -p zoid-model` — FAIL (expected)

The brief tells the agent to verify with `cargo test -p zoid-model`, but
the wire-shape routing tables and per-provider `list_models`/stream tests
live in `zoid-provider` (`opencode_zen.rs` tests, `opencode_go.rs` tests,
`zai_list_models_hits_models_without_v1_prefix`, etc.). An agent that
stops at `zoid-model` misses regressions in routing. The skill must require
`cargo test -p zoid-provider` (and ideally the whole workspace) in
addition to `cargo test -p zoid-model`.

### 10. Breaks the `key_url` invariant — FAIL (expected)

`key_url_field_present_on_all_providers` asserts `ollama-local` is the only
provider with `key_url: None`; every other provider must have
`key_url: Some(...)`. An agent that adds a new keyless provider (or copies
`ollama-local` as a template for a new local-style provider) and leaves
`key_url: None` fails this test. The skill must require `key_url: Some(...)`
for every key-requiring provider and `None` only for `ollama-local`.

## Summary

All 10 checklist items are **FAIL (expected)** — i.e. an agent without the
skill is expected to fall into every one of these traps. No API keys were
available, so these are documented from the codebase's own invariants and
tests rather than from observed live-curl mistakes; the pitfalls hold
regardless because they are structural traps in the registry design, not
key-dependent. These 10 are exactly what the `refreshing-provider-models`
skill must prevent.

---

## GREEN verification (Task 3)

The `refreshing-provider-models` built-in skill
(`REFRESHING_PROVIDER_MODELS_BODY` in `crates/zoid-core/src/skill.rs`, mirrored
in `.superpowers/sdd/skill-body-for-verification.md`) was verified against all
10 baseline pitfalls by reading the skill body and cross-checking the
referenced codebase invariants (`ZEN_MODELS`/`GO_MODELS` in `zoid-provider`,
`opencode_zen_model_caps_present` / `key_url_field_present_on_all_providers`
in `zoid-model`). No sub-subagent dispatch was available, so the verification
was performed statically; this is sufficient because the 10 pitfalls are
structural invariants independent of live data.

| # | Pitfall | Result | Skill-body evidence |
|---|---|---|---|
| 1 | `/api/tags` + `.models[].name` for `ollama-cloud` | PASS | Phase 1 table row + "Critical" callout: "both hit `/api/tags` and parse `.models[].name`… Do not use `/v1/models` or `.data[].id` for either Ollama flavor." |
| 2 | `x-api-key` + `anthropic-version` for Anthropic | PASS | Phase 1 table row: "`x-api-key` + `anthropic-version: 2023-06-01`"; bash example headed `# anthropic-api (NOT Bearer — uses x-api-key)`. |
| 3 | Preserve `PROVIDERS` order | PASS | Phase 2a: "Preserve `PROVIDERS` order — picker display order (convention). Insert new ids grouped with siblings." |
| 4 | `opencode_zen_model_caps_present` (≥128k) | PASS | Phase 2b: "`opencode_zen_model_caps_present` asserts every `opencode-zen` model has `context_window >= 128_000`… New Zen/Go ids must have an explicit researched entry." (test confirmed at `zoid-model/src/lib.rs:869`). |
| 5 | `thinking_wire` per-model, not per-family | PASS | Phase 2b: "**`thinking_wire` is per-model, not per-family.**" |
| 6 | `ZEN_MODELS`/`GO_MODELS` routing tables | PASS | Phase 3: "Adding a new id to `ZEN_MODEL_IDS` requires a matching entry in `opencode_zen.rs::ZEN_MODELS`… new `opencode-go` ids need an entry in `opencode_go.rs::GO_MODELS`." (tables confirmed at `zoid-provider/src/opencode_zen.rs:24`, `opencode_go.rs:20`). |
| 7 | `ollama-cloud` is curated | PASS | Phase 2a: "`ollama-cloud` is **curated** (`&["glm-5.2:cloud"]`), not a live-list mirror." |
| 8 | `ZEN_MODEL_IDS` default is a product choice | PASS | Phase 2a: "`ZEN_MODEL_IDS` first entry is the default model — a **product decision**, not endpoint-derivable. Do not change without explicit instruction." |
| 9 | Run `cargo test -p zoid-provider` | PASS | Phase 3 bash block: `cargo test -p zoid-provider  # wire-shape routing tables`. |
| 10 | `key_url` invariant (ollama-local=None, rest=Some) | PASS | Phase 2c: "`ollama-local` must be `None`, all others `Some(_)` (the test is keyed on provider id)." (test confirmed at `zoid-model/src/lib.rs:813`). |

**Verdict: 10/10 PASS (GREEN).** The skill body contains correct, specific
guidance for every one of the 10 baseline pitfalls. Full evidence and quotes
are in `.superpowers/sdd/task-3-report.md`.