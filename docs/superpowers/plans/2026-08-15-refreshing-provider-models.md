# Refreshing Provider Models Skill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a reference skill (`refreshing-provider-models`) that guides an agent to refresh zoid's static provider/model registry against live provider endpoints.

**Architecture:** A single `SKILL.md` file in the superpowers skills directory containing a provider fetch table, curl examples, registry-editing invariants, MODEL_CAPS field reference, and verification commands. The skill is tested via TDD-for-skills: a baseline subagent run without the skill (RED), then with the skill (GREEN), then loophole-closing (REFACTOR).

**Tech Stack:** Markdown skill file, subagent-based testing, `cargo test -p zoid-model` / `cargo test -p zoid-provider` as verification gates.

## Global Constraints

- Skill lives at `~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md` (the personal skills directory). In the worktree, create it at the equivalent repo-relative path if one exists; otherwise create it in the config directory.
- Skill name uses only letters, numbers, and hyphens: `refreshing-provider-models`.
- YAML frontmatter: `name` and `description` fields, max 1024 chars total. Description starts with "Use when..." in third person, covers triggering conditions only (no workflow summary).
- Skill body target: <500 words for a non-frequently-loaded reference skill. Push nothing to separate files — this is self-contained.
- The spec is at `docs/superpowers/specs/2026-08-15-refreshing-provider-models-design.md`. All technical content in the skill must match the spec (which was reviewed against source code).
- This is a **reference skill** (not a discipline skill). Test with application/retrieval scenarios, not pressure scenarios. Per writing-skills: "Test with: Application scenarios — can they apply the technique correctly? Gap testing — are common use cases covered?"
- REQUIRED BACKGROUND: You MUST understand superpowers:writing-skills before implementing. That skill defines the TDD-for-skills cycle (RED-GREEN-REFACTOR) and the SKILL.md structure.

---

### Task 1: Baseline test — run registry refresh without the skill (RED)

**Files:**
- No files created yet. This task dispatches a subagent and records observations.

**Interfaces:**
- Produces: a documented set of baseline failures (wrong endpoints, wrong auth, missed invariants, etc.) that the skill must address.

- [ ] **Step 1: Dispatch a baseline subagent**

Dispatch a subagent with the `delegate` agent profile. Give it this task (do NOT mention the skill or the spec — test what an agent does with only the codebase):

```
You are working in the zoid codebase. The static provider/model registry lives
in `crates/zoid-model/src/lib.rs` — it has three things to keep fresh:

1. `PROVIDERS` — a const array of `ProviderEntry` structs, each with a
   `models: &[&str]` field listing the model ids that provider offers.
2. `ZEN_MODEL_IDS` — a static array of all model ids available through the
   opencode-zen gateway.
3. `MODEL_CAPS` — a const array of `(&str, ModelInfo)` entries with per-model
   capabilities (context_window, max_output, tools, prompt_cache, thinking,
   thinking_wire).

Each provider has a `list_models()` implementation in
`crates/zoid-provider/src/` that hits a live HTTP endpoint.

Your task: Refresh the registry from the live provider endpoints. Query each
provider's model-list endpoint, compare against the static arrays, and update
`crates/zoid-model/src/lib.rs` to match. Add MODEL_CAPS entries for any new
models. Then verify with `cargo test -p zoid-model`.

Available env vars for auth: OLLAMA_API_KEY, ANTHROPIC_API_KEY,
OPENCODE_GO_API_KEY, ZAI_API_KEY.
```

Use `dispatch_subagent` with `agent: "delegate"`. Do NOT use a worktree — the subagent will only read files and attempt curl, not edit. If it tries to edit, that's fine — we want to see what it does.

- [ ] **Step 2: Document the baseline failures**

When the subagent completes, review its work and document (in a scratch note, not a committed file):

1. Did it use the correct endpoint for `ollama-cloud`? (Should be `/api/tags` with `.models[].name`, not `/v1/models` with `.data[].id`)
2. Did it know the correct auth header for each provider? (Anthropic needs `x-api-key` + `anthropic-version`, not Bearer)
3. Did it preserve `PROVIDERS` array order?
4. Did it know about the `opencode_zen_model_caps_present` invariant (≥128k context for Zen models)?
5. Did it know that `thinking_wire` is per-model, not per-family?
6. Did it know about the `ZEN_MODELS`/`GO_MODELS` wire-shape routing tables in the provider crate?
7. Did it know `ollama-cloud` is a curated subset, not a live-list mirror?
8. Did it know the `ZEN_MODEL_IDS` default (first entry) is a product choice, not endpoint-derivable?
9. Did it run `cargo test -p zoid-provider` in addition to `cargo test -p zoid-model`?
10. Did it know the `key_url` invariant (ollama-local = None, all others = Some)?

Record the exact mistakes. These are the failures the skill must prevent.

- [ ] **Step 3: Commit the baseline notes**

Write the observations to `docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md` and commit:

```bash
git add docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md
git commit -m "docs(skill): document baseline test failures for refreshing-provider-models"
```

---

### Task 2: Write the SKILL.md (GREEN)

**Files:**
- Create: `~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md`

**Interfaces:**
- Consumes: the baseline failures from Task 1, the spec at `docs/superpowers/specs/2026-08-15-refreshing-provider-models-design.md`
- Produces: the skill file that an agent loads when asked to refresh the provider model registry

- [ ] **Step 1: Create the skill directory and file**

Create the directory and write the SKILL.md. The content below is the complete skill — every section addresses a specific baseline failure from Task 1.

```markdown
---
name: refreshing-provider-models
description: Use when refreshing zoid's static provider/model registry against live provider endpoints, adding new models to MODEL_CAPS, reconciling model id drift, or updating provider metadata across the six supported providers
---

# Refreshing Provider Models

## Overview

Refresh the static provider/model registry in `crates/zoid-model/src/lib.rs`
against live provider endpoints. The registry has three targets: `PROVIDERS`
model id arrays, `ZEN_MODEL_IDS`, and `MODEL_CAPS` (per-model capabilities).

## Phase 1 — Fetch live model lists

Run a `curl` GET per provider. Skip providers whose key is missing.

| Provider id | Secret env var | Endpoint | Auth | Response path | Registry field |
|---|---|---|---|---|---|
| `ollama-local` | (keyless) | `{base}/api/tags` | Bearer (opt) | `.models[].name` | skip (free-text) |
| `ollama-cloud` | `OLLAMA_API_KEY` | `https://ollama.com/api/tags` | Bearer | `.models[].name` | `ollama-cloud` models (curated) |
| `opencode-go` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/go/v1/models` | Bearer | `.data[].id` | `opencode-go` models |
| `opencode-zen` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/v1/models` | Bearer | `.data[].id` | `ZEN_MODEL_IDS` |
| `anthropic-api` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/models` | `x-api-key` + `anthropic-version: 2023-06-01` | `.data[].id` | `anthropic-api` models |
| `zai-coding-plan` | `ZAI_API_KEY` | `https://api.z.ai/api/coding/paas/v4/models` | Bearer | `.data[].id` | `zai-coding-plan` models |

**Critical:** `ollama-local` and `ollama-cloud` share `OllamaProvider` — both
hit `/api/tags` and parse `.models[].name`. Neither is OpenAI-compat. Do not
use `/v1/models` or `.data[].id` for either Ollama flavor.

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

## Phase 2 — Diff and update

### 2a. Model id lists

- Add ids present live but missing from the static array. Remove ids absent
  live (retired).
- Preserve `PROVIDERS` order — it's the picker display order (convention, not
  test-enforced). Insert new ids grouped with siblings.
- `ollama-local` stays `&[]` — never populate it.
- `ollama-cloud` is **curated** (`&["glm-5.2:cloud"]`), not a live-list mirror.
  Preserve the `:cloud` suffix; any new cloud id needs a `MODEL_CAPS` entry.
- `ZEN_MODEL_IDS` first entry is the default model — a **product decision**, not
  endpoint-derivable. Do not change it without explicit instruction. Update the
  `// All NN Zen model ids` count comment to match.
- Cross-array duplication is expected (`glm-5.2` appears in Zen, Go, and ZAI).
  Dedup matters only within `MODEL_CAPS` (case-insensitive), not across
  provider id arrays.

### 2b. MODEL_CAPS for new ids

All unknowns fall back to `DEFAULT_MODEL_INFO` (`lib.rs:640`): 32k / 0 /
tools=true / prompt_cache=false / None / None.

**Exception:** `opencode_zen_model_caps_present` asserts every `opencode-zen`
model has `context_window >= 128_000` — the 32k default is not acceptable for
selectable Zen/Go models. New Zen/Go ids must have an explicit researched
entry.

`ModelInfo` fields (see struct at `lib.rs:15`): `context_window` (u64),
`max_output` (u64, 0 = provider default), `tools` (bool), `prompt_cache`
(bool), `thinking` (ThinkingSupport), `thinking_wire` (ThinkingWireShape).

**`thinking_wire` is per-model, not per-family.** Many Anthropic-routed Go/Zen
models have `thinking_wire: None`. Copy from a researched sibling of the same
family/variant where one exists; otherwise `None`.

Do not duplicate `MODEL_CAPS` entries — lookup is case-insensitive, duplicates
silently shadow.

### 2c. Provider metadata

Verify `default_base_url` still resolves (Phase 1 proved reachability). Verify
`key_url` is still valid — `ollama-local` must be `None`, all others `Some(_)`
(the test is keyed on provider id, not "key-requiring"). Flag dark providers,
do not remove without confirmation.

## Phase 3 — Verify

```bash
cargo test -p zoid-model    # registry invariants
cargo build -p zoid-provider # re-exports compile
cargo test -p zoid-provider  # wire-shape routing tables
```

**Wire-shape routing tables:** Adding a new id to `ZEN_MODEL_IDS` requires a
matching entry in `opencode_zen.rs::ZEN_MODELS`, or it silently defaults to
`OpenAIChat` (wrong wire shape, no test failure). Likewise, new `opencode-go`
ids need an entry in `opencode_go.rs::GO_MODELS`. These are in
`crates/zoid-provider/src/`, separate from the registry's `models` arrays.

Key test invariants:
- `selectable_has_six_providers` — exactly six selectable providers.
- `opencode_go_entry_unchanged` — Go has exactly 13 models.
- `opencode_zen_model_caps_present` — every Zen model ≥128k context.
- `key_url_field_present_on_all_providers` — ollama-local=None, rest=Some.
- `model_info_unknown_falls_back_to_conservative_default` — unknown → 32k.
```

- [ ] **Step 2: Verify word count is under target**

Run:
```bash
wc -w ~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md
```

Expected: under 600 words (reference skill, slightly over the 500-word soft
target due to the fetch table and curl examples — acceptable for a reference
skill where the table IS the value). If over 700, trim the curl examples to
just the two non-obvious ones (ollama-cloud and anthropic-api).

- [ ] **Step 3: Verify frontmatter**

Check the frontmatter manually:
- `name: refreshing-provider-models` — letters, numbers, hyphens only. ✓
- `description` starts with "Use when..." — ✓
- `description` is third person — ✓
- `description` covers triggering conditions, not workflow summary — ✓
- Total frontmatter under 1024 chars — ✓

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(skill): add refreshing-provider-models SKILL.md"
```

Note: the skill file is in `~/.config/zoid/modes/superpowers/`, which may be
outside the repo. If so, copy it into the repo for version control:

```bash
# If the skill dir is outside the worktree, also place a copy in the repo
# for the commit (the runtime reads from ~/.config/zoid/modes/)
mkdir -p docs/superpowers/skills/refreshing-provider-models
cp ~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md \
   docs/superpowers/skills/refreshing-provider-models/SKILL.md
git add docs/superpowers/skills/refreshing-provider-models/SKILL.md
git commit --amend -m "feat(skill): add refreshing-provider-models SKILL.md"
```

---

### Task 3: Verify the skill with a subagent (GREEN verification)

**Files:**
- No new files. This task dispatches a subagent with the skill loaded.

**Interfaces:**
- Consumes: the SKILL.md from Task 2, the same baseline task from Task 1
- Produces: verification that the skill prevents the baseline failures

- [ ] **Step 1: Dispatch a verification subagent WITH the skill**

Dispatch a subagent with the `delegate` agent profile. Give it the same task
as Task 1, but this time include the skill content in the system prompt (or
reference the skill file path so it can read it):

```
You are working in the zoid codebase. The static provider/model registry lives
in `crates/zoid-model/src/lib.rs` — it has three things to keep fresh:

1. `PROVIDERS` — a const array of `ProviderEntry` structs, each with a
   `models: &[&str]` field listing the model ids that provider offers.
2. `ZEN_MODEL_IDS` — a static array of all model ids available through the
   opencode-zen gateway.
3. `MODEL_CAPS` — a const array of `(&str, ModelInfo)` entries with per-model
   capabilities (context_window, max_output, tools, prompt_cache, thinking,
   thinking_wire).

Each provider has a `list_models()` implementation in
`crates/zoid-provider/src/` that hits a live HTTP endpoint.

Your task: Refresh the registry from the live provider endpoints. Query each
provider's model-list endpoint, compare against the static arrays, and update
`crates/zoid-model/src/lib.rs` to match. Add MODEL_CAPS entries for any new
models. Then verify with `cargo test -p zoid-model`.

Available env vars for auth: OLLAMA_API_KEY, ANTHROPIC_API_KEY,
OPENCODE_GO_API_KEY, ZAI_API_KEY.

Read the skill at
~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md
(or docs/superpowers/skills/refreshing-provider-models/SKILL.md in the repo)
before starting — it contains the exact endpoints, auth headers, response
shapes, and invariants you need.
```

- [ ] **Step 2: Check each baseline failure is now addressed**

When the subagent completes, verify against the baseline failures documented
in Task 1:

1. Did it use `/api/tags` + `.models[].name` for ollama-cloud? (not `/v1/models`)
2. Did it use `x-api-key` + `anthropic-version` for Anthropic? (not Bearer)
3. Did it preserve `PROVIDERS` order?
4. Did it know about the ≥128k Zen caps invariant?
5. Did it treat `thinking_wire` as per-model?
6. Did it know about `ZEN_MODELS`/`GO_MODELS` wire-shape tables?
7. Did it treat `ollama-cloud` as curated?
8. Did it leave the `ZEN_MODEL_IDS` default unchanged?
9. Did it run `cargo test -p zoid-provider`?
10. Did it know the `key_url` invariant?

If any failure persists, note it for the REFACTOR task.

- [ ] **Step 3: Document the verification result**

Write a brief pass/fail summary to
`docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md`
(append to the existing file):

```bash
git add docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md
git commit -m "docs(skill): document GREEN verification results for refreshing-provider-models"
```

---

### Task 4: Close loopholes (REFACTOR)

**Files:**
- Modify: `~/.config/zoid/modes/superpowers/refreshing-provider-models/SKILL.md`
  (and the repo copy at `docs/superpowers/skills/refreshing-provider-models/SKILL.md`)

**Interfaces:**
- Consumes: any remaining failures from Task 3's verification
- Produces: a bulletproof skill that closes all identified loopholes

- [ ] **Step 1: Review verification results from Task 3**

If all 10 baseline failures are addressed, the skill is GREEN — skip to Step 4.
If any failures persist, proceed to Step 2.

- [ ] **Step 2: Add explicit counters for each remaining failure**

For each failure that persisted despite the skill, add an explicit callout to
the SKILL.md. Common loopholes to watch for:

- Agent still uses `/v1/models` for ollama-cloud → add a bold "NOT /v1/models"
  warning next to the ollama-cloud row.
- Agent still uses Bearer for Anthropic → add "NOT Bearer" next to the
  anthropic row.
- Agent adds a Zen model without a `ZEN_MODELS` entry → move the wire-shape
  warning higher in the skill (before Phase 3, into Phase 2b).
- Agent changes the `ZEN_MODEL_IDS` default → add a red-flags list.
- Agent populates `ollama-local` → add "NEVER populate" in bold.

Apply only the counters needed for the actual failures observed. Do not add
hypothetical counters.

- [ ] **Step 3: Re-verify with a fresh subagent**

Dispatch the same task as Task 3 Step 1 with the updated skill. Confirm the
remaining failures are now addressed.

- [ ] **Step 4: Commit the refactored skill**

```bash
git add docs/superpowers/skills/refreshing-provider-models/SKILL.md
git commit -m "refactor(skill): close loopholes in refreshing-provider-models"
```

- [ ] **Step 5: Final word count check**

```bash
wc -w docs/superpowers/skills/refreshing-provider-models/SKILL.md
```

Expected: under 700 words. If over, trim redundant content.