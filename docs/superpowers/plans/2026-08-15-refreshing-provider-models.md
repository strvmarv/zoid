# Refreshing Provider Models Skill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a reference skill (`refreshing-provider-models`) that guides an agent to refresh zoid's static provider/model registry against live provider endpoints, bundled into the zoid binary as a built-in skill.

**Architecture:** The skill body is authored as a `const &str` in `crates/zoid-core/src/skill.rs` and registered in `SkillRegistry::builtin()` alongside the existing `spike-plan`, `spike-implement`, and `feedback` skills. It is tested via TDD-for-skills: a baseline subagent run without the skill (RED), then with the skill (GREEN), then loophole-closing (REFACTOR).

**Tech Stack:** Rust `const &str` in `zoid-core`, subagent-based skill testing, `cargo test -p zoid-core` / `cargo test -p zoid-model` / `cargo test -p zoid-provider` as verification gates.

## Global Constraints

- The skill is **bundled into the binary** — a `const` string constant in `crates/zoid-core/src/skill.rs`, registered in `SkillRegistry::builtin()`. This is the same pattern as `FEEDBACK_SKILL_BODY` (skill.rs:91-138). Built-in skills have `base_dir: None`.
- The skill `name` (frontmatter `name:` field) must match the `name` field in the `Skill` struct entry in `builtin()`: `refreshing-provider-models`.
- The `description` field in the `Skill` struct must start with "Use when..." in third person, covering triggering conditions only (no workflow summary).
- The skill body is the markdown content AFTER the frontmatter — in the `const` string, there is no frontmatter; the `name` and `description` are set directly on the `Skill` struct.
- The body must be under ~600 words (reference skill; the fetch table and curl examples are the core value).
- The spec is at `docs/superpowers/specs/2026-08-15-refreshing-provider-models-design.md`. All technical content must match the spec (which was reviewed against source code).
- This is a **reference skill** (not a discipline skill). Test with application/retrieval scenarios, not pressure scenarios.
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

Use `dispatch_subagent` with `agent: "delegate"`. Do NOT use a worktree — the subagent will only read files and attempt curl, not edit.

- [ ] **Step 2: Document the baseline failures**

When the subagent completes, review its work and document the 10 expected
failures below (these are the known pitfalls the skill must address — check
whether the subagent fell into each one):

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

If the subagent had no API keys available (all providers skipped), document
the expected failures synthetically from the spec's known pitfalls rather than
leaving the baseline empty — the 10 items above are the known pitfalls
regardless of whether live keys were present.

Record the exact mistakes (or the expected pitfalls if keys were missing).

- [ ] **Step 3: Commit the baseline notes**

Create the baseline file and commit:

```bash
cat > docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md << 'EOF'
# Refreshing Provider Models — Baseline Test Results

[Document the subagent's actual failures, or the 10 expected pitfalls if
no API keys were available.]
EOF
git add docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md
git commit -m "docs(skill): document baseline test failures for refreshing-provider-models"
```

---

### Task 2: Write the skill body and register it (GREEN)

**Files:**
- Modify: `crates/zoid-core/src/skill.rs` — add the `const` body string and a `Skill` entry in `builtin()`

**Interfaces:**
- Consumes: the baseline failures from Task 1, the spec at `docs/superpowers/specs/2026-08-15-refreshing-provider-models-design.md`
- Produces: a built-in skill registered in `SkillRegistry::builtin()` that an agent can `invoke_skill("refreshing-provider-models")`

- [ ] **Step 1: Write the failing test**

Add a test to `crates/zoid-core/src/skill.rs` in the `tests` module that asserts the new skill exists in `builtin()` and has the right name/description. This test will fail until the skill is added.

```rust
    #[test]
    fn builtin_includes_refreshing_provider_models_skill() {
        let r = SkillRegistry::builtin();
        let s = r.get("refreshing-provider-models")
            .expect("refreshing-provider-models must be a built-in skill");
        assert!(
            s.description.starts_with("Use when"),
            "description must start with 'Use when'"
        );
        assert!(
            s.body.contains("ollama-cloud"),
            "skill body must mention ollama-cloud"
        );
        assert!(
            s.body.contains("/api/tags"),
            "skill body must mention /api/tags for Ollama"
        );
        assert!(
            s.body.contains("anthropic-version"),
            "skill body must mention anthropic-version header"
        );
        assert!(
            s.body.contains("MODEL_CAPS"),
            "skill body must reference MODEL_CAPS"
        );
        assert!(
            s.body.contains("opencode_zen_model_caps_present"),
            "skill body must reference the Zen caps invariant test"
        );
        assert!(
            s.body.contains("thinking_wire"),
            "skill body must reference thinking_wire"
        );
        assert!(s.base_dir.is_none(), "built-in skills have no base_dir");
    }
```

**Also update every existing test that asserts on builtin skill count/names.**
Adding a 4th builtin breaks exact-count assertions. Update each:

1. `builtin_has_both_spike_skills_that_chain` — add `"refreshing-provider-models".to_string()` to the expected `r.names()` vec.
2. `builtin_includes_feedback_skill` — its `assert_eq!(r.names(), vec![…])` with 3 entries will break. **Refactor** the exact-vec assertion to a membership check: replace the `assert_eq!(r.names(), …)` block with `assert!(r.get("feedback").is_some())`. Keep the body-contains and `base_dir` assertions below it (those don't depend on count).
3. `menu_renders_one_line_per_skill` — change `assert_eq!(menu.lines().count(), 3)` to `4`. Add `assert!(menu.contains("- refreshing-provider-models: "));`.
4. `all_exposes_every_skill_in_order` — add `"refreshing-provider-models"` to the expected `names` vec.
5. `builtin_skills_have_no_base_dir` — safe (only checks 3 named skills, not exhaustive). Optionally add an assertion for the new skill's `base_dir` for symmetry, but the new test in Step 1 already covers this.

**Downstream-consumer safety (verified):** No other workspace test asserts on `builtin()` count/names. `invoke_skill.rs:144` uses `contains("spike-plan")` (membership). `agent.rs` test fixtures use `SkillRegistry::builtin()` but never assert on count. `agent.rs:3473` passes a literal menu string, not `builtin().menu()`. All safe — no changes needed outside `skill.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core skill::tests`
Expected: FAIL — `refreshing-provider-models` not found in registry.

- [ ] **Step 3: Add the skill body constant and register the skill**

Add a `const` string for the skill body after `FEEDBACK_SKILL_BODY` in `crates/zoid-core/src/skill.rs`. Use `concat!` of string-literal lines (avoids `\n\` continuation and `\"` escaping — cleaner and less error-prone than the `FEEDBACK_SKILL_BODY` idiom). The body is the markdown content (no frontmatter — the `name` and `description` are set on the `Skill` struct directly):

```rust
/// The body of the built-in `refreshing-provider-models` skill. Guides an
/// agent to refresh the static provider/model registry in `zoid-model` against
/// live provider endpoints, add MODEL_CAPS entries for new models, and verify.
const REFRESHING_PROVIDER_MODELS_BODY: &str = concat!(
    "# Refreshing Provider Models\n\n",
    "Refresh the static provider/model registry in `crates/zoid-model/src/lib.rs`\n",
    "against live provider endpoints. Three targets: `PROVIDERS` model id arrays,\n",
    "`ZEN_MODEL_IDS`, and `MODEL_CAPS` (per-model capabilities).\n\n",
    "## Phase 1 — Fetch live model lists\n\n",
    "Run a `curl` GET per provider. Skip providers whose key is missing.\n\n",
    "| Provider id | Secret env var | Endpoint | Auth | Response path | Registry field |\n",
    "|---|---|---|---|---|---|\n",
    "| `ollama-local` | (keyless) | `{base}/api/tags` | Bearer (opt) | `.models[].name` | skip (free-text) |\n",
    "| `ollama-cloud` | `OLLAMA_API_KEY` | `https://ollama.com/api/tags` | Bearer | `.models[].name` | `ollama-cloud` models (curated) |\n",
    "| `opencode-go` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/go/v1/models` | Bearer | `.data[].id` | `opencode-go` models |\n",
    "| `opencode-zen` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/v1/models` | Bearer | `.data[].id` | `ZEN_MODEL_IDS` |\n",
    "| `anthropic-api` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/models` | `x-api-key` + `anthropic-version: 2023-06-01` | `.data[].id` | `anthropic-api` models |\n",
    "| `zai-coding-plan` | `ZAI_API_KEY` | `https://api.z.ai/api/coding/paas/v4/models` | Bearer | `.data[].id` | `zai-coding-plan` models |\n\n",
    "**Critical:** `ollama-local` and `ollama-cloud` share `OllamaProvider` — both\n",
    "hit `/api/tags` and parse `.models[].name`. Neither is OpenAI-compat. Do not\n",
    "use `/v1/models` or `.data[].id` for either Ollama flavor.\n\n",
    "```bash\n",
    "# ollama-cloud (native Ollama API, not OpenAI-compat)\n",
    "curl -s -H \"Authorization: Bearer $OLLAMA_API_KEY\" https://ollama.com/api/tags | jq -r '.models[].name'\n",
    "# anthropic-api (NOT Bearer — uses x-api-key)\n",
    "curl -s -H \"x-api-key: $ANTHROPIC_API_KEY\" -H \"anthropic-version: 2023-06-01\" https://api.anthropic.com/v1/models | jq -r '.data[].id'\n",
    "# opencode-zen\n",
    "curl -s -H \"Authorization: Bearer $OPENCODE_GO_API_KEY\" https://opencode.ai/zen/v1/models | jq -r '.data[].id'\n",
    "```\n\n",
    "## Phase 2 — Diff and update\n\n",
    "### 2a. Model id lists\n\n",
    "- Add ids present live but missing. Remove ids absent live (retired).\n",
    "- Preserve `PROVIDERS` order — picker display order (convention). Insert new\n",
    "  ids grouped with siblings.\n",
    "- `ollama-local` stays `&[]` — never populate it.\n",
    "- `ollama-cloud` is **curated** (`&[\"glm-5.2:cloud\"]`), not a live-list\n",
    "  mirror. Preserve the `:cloud` suffix; new cloud ids need MODEL_CAPS entries.\n",
    "- `ZEN_MODEL_IDS` first entry is the default model — a **product decision**,\n",
    "  not endpoint-derivable. Do not change without explicit instruction. The\n",
    "  `// All NN Zen model ids` count comment (currently 52: 13 Anthropic +\n",
    "  17 OpenAI Responses + 19 OpenAI Chat + 3 Gemini) must be updated to match.\n",
    "- Cross-array duplication is expected (`glm-5.2` appears in Zen, Go, ZAI).\n",
    "  Dedup matters only within `MODEL_CAPS` (case-insensitive), not across\n",
    "  provider id arrays.\n\n",
    "### 2b. MODEL_CAPS for new ids\n\n",
    "All unknowns fall back to `DEFAULT_MODEL_INFO` (`lib.rs:640`): 32k / 0 /\n",
    "tools=true / prompt_cache=false / None / None.\n\n",
    "**Exception:** `opencode_zen_model_caps_present` asserts every `opencode-zen`\n",
    "model has `context_window >= 128_000` — the 32k default is not acceptable for\n",
    "selectable Zen/Go models. New Zen/Go ids must have an explicit researched\n",
    "entry. (The `opencode_zen_caps_match_table` lock test has 39 cases — the 13\n",
    "that overlap with Go are excluded; it doesn't auto-catch *new* ids, but\n",
    "`opencode_zen_model_caps_present` does via the >=128k gate.)\n\n",
    "`ModelInfo` fields (see struct at `lib.rs:15`): `context_window` (u64),\n",
    "`max_output` (u64, 0 = provider default), `tools` (bool), `prompt_cache`\n",
    "(bool), `thinking` (ThinkingSupport), `thinking_wire` (ThinkingWireShape).\n\n",
    "**`thinking_wire` is per-model, not per-family.** Many Anthropic-routed Go/Zen\n",
    "models have `thinking_wire: None`. Copy from a researched sibling of the same\n",
    "family/variant where one exists; otherwise `None`.\n\n",
    "Do not duplicate `MODEL_CAPS` entries — lookup is case-insensitive, duplicates\n",
    "silently shadow.\n\n",
    "### 2c. Provider metadata\n\n",
    "Verify `default_base_url` still resolves (Phase 1 proved reachability). Verify\n",
    "`key_url` is still valid — `ollama-local` must be `None`, all others `Some(_)`\n",
    "(the test is keyed on provider id). Flag dark providers, do not remove without\n",
    "confirmation.\n\n",
    "## Phase 3 — Verify\n\n",
    "```bash\n",
    "cargo test -p zoid-model    # registry invariants\n",
    "cargo build -p zoid-provider # re-exports compile\n",
    "cargo test -p zoid-provider  # wire-shape routing tables\n",
    "```\n\n",
    "**Wire-shape routing tables:** Adding a new id to `ZEN_MODEL_IDS` requires a\n",
    "matching entry in `opencode_zen.rs::ZEN_MODELS`, or it silently defaults to\n",
    "`OpenAIChat` (wrong wire shape, no test failure). Likewise, new `opencode-go`\n",
    "ids need an entry in `opencode_go.rs::GO_MODELS`. These are in\n",
    "`crates/zoid-provider/src/`, separate from the registry's `models` arrays.\n\n",
    "Key test invariants:\n",
    "- `selectable_has_six_providers` — exactly six selectable providers.\n",
    "- `opencode_go_entry_unchanged` — Go has exactly 13 models.\n",
    "- `opencode_zen_model_caps_present` — every Zen model >= 128k context.\n",
    "- `key_url_field_present_on_all_providers` — ollama-local=None, rest=Some.\n",
    "- `model_info_unknown_falls_back_to_conservative_default` — unknown -> 32k.\n",
);

Then add the `Skill` entry to the `vec!` in `builtin()` (after the `feedback` skill):

```rust
            Skill {
                name: "refreshing-provider-models".into(),
                description: "Use when refreshing zoid's static provider/model \
                    registry against live provider endpoints, adding new models \
                    to MODEL_CAPS, reconciling model id drift, or updating \
                    provider metadata across the six supported providers".into(),
                body: REFRESHING_PROVIDER_MODELS_BODY.into(),
                base_dir: None,
            },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core skill::tests`
Expected: PASS — all tests including the new `builtin_includes_refreshing_provider_models_skill`.

- [ ] **Step 5: Verify the full workspace compiles**

Run: `cargo build -p zoid-core`
Expected: compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/skill.rs
git commit -m "feat(skill): bundle refreshing-provider-models as a built-in skill

Add the skill body as a const string in SkillRegistry::builtin() alongside
spike-plan, spike-implement, and feedback. The skill guides an agent to refresh
the static provider/model registry against live endpoints, update MODEL_CAPS
for new models, and verify with cargo test."
```

---

### Task 3: Verify the skill with a subagent (GREEN verification)

**Files:**
- No new files. This task dispatches a subagent with the skill loaded.

**Interfaces:**
- Consumes: the registered skill from Task 2, the same baseline task from Task 1
- Produces: verification that the skill prevents the baseline failures

- [ ] **Step 1: Dispatch a verification subagent WITH the skill**

Dispatch a subagent with the `delegate` agent profile. Give it the same task as Task 1, but this time include the skill body in the prompt (extract it from the `const` in `skill.rs`):

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

A skill named \"refreshing-provider-models\" is available. Here is its body:

$(Extract the body from the REFRESHING_PROVIDER_MODELS_BODY const you wrote in
Task 2 Step 3. To get the exact content, run:
  cargo run -p zoid-core --example print_skill_body 2>/dev/null
or simply copy the string-literal contents from the concat!(…) block — each
line is a separate string literal; concatenate them mentally to reconstruct
the markdown body. Paste the reconstructed body here.)
```

- [ ] **Step 2: Check each baseline failure is now addressed**

When the subagent completes, verify against the baseline failures documented in Task 1:

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

Append the pass/fail summary to
`docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md`:

```bash
git add docs/superpowers/specs/2026-08-15-refreshing-provider-models-baseline.md
git commit -m "docs(skill): document GREEN verification results for refreshing-provider-models"
```

---

### Task 4: Close loopholes (REFACTOR)

**Files:**
- Modify: `crates/zoid-core/src/skill.rs` — update `REFRESHING_PROVIDER_MODELS_BODY`

**Interfaces:**
- Consumes: any remaining failures from Task 3's verification
- Produces: a bulletproof skill that closes all identified loopholes

- [ ] **Step 1: Review verification results from Task 3**

If all 10 baseline failures are addressed, the skill is GREEN — skip to Step 4.
If any failures persist, proceed to Step 2.

- [ ] **Step 2: Add explicit counters for each remaining failure**

For each failure that persisted despite the skill, add an explicit callout to the `REFRESHING_PROVIDER_MODELS_BODY` const. Common loopholes to watch for:

- Agent still uses `/v1/models` for ollama-cloud → add a bold "NOT /v1/models" warning next to the ollama-cloud row.
- Agent still uses Bearer for Anthropic → add "NOT Bearer" next to the anthropic row.
- Agent adds a Zen model without a `ZEN_MODELS` entry → move the wire-shape warning higher (into Phase 2b, before Phase 3).
- Agent changes the `ZEN_MODEL_IDS` default → add a red-flags list.
- Agent populates `ollama-local` → add "NEVER populate" in bold.

Apply only the counters needed for the actual failures observed.

- [ ] **Step 3: Re-verify with a fresh subagent**

Dispatch the same task as Task 3 Step 1 with the updated skill body. Confirm the remaining failures are now addressed.

- [ ] **Step 4: Run all skill tests and commit (only if changes were made)**

Run: `cargo test -p zoid-core skill::tests`
Expected: PASS.

If Step 1 found no loopholes and no changes were made in Steps 2-3, skip the
commit — there is nothing to stage. Otherwise:

```bash
git add crates/zoid-core/src/skill.rs
git commit -m "refactor(skill): close loopholes in refreshing-provider-models"
```