# ZAI Additional Models (glm-5-turbo, glm-4.7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `glm-5-turbo` and `glm-4.7` to the `zai-coding-plan` provider, completing the three models officially supported by ZAI's Coding Plan.

**Architecture:** Two new `MODEL_CAPS` entries in `zoid-model` (confirmed via live API probing — both use the DeepSeek thinking wire shape). The `zai-coding-plan` provider entry's `models` array expands from one to three models. No provider or wiring changes needed — `ZaiProvider` and the `family`-based dispatch already handle any model on the endpoint.

**Tech Stack:** Rust, serde_json (test assertions).

## Global Constraints

- All capability values confirmed via live API probing against `https://api.z.ai/api/coding/paas/v4` on 2026-07-11, plus cross-referenced with OpenRouter and docs.z.ai.
- Both models use `ThinkingSupport::ToggleWithEffort` + `ThinkingWireShape::DeepSeek` (identical wire shape to glm-5.2, confirmed by probing).
- No new dependencies.
- No provider logic changes (`ZaiProvider`, `openai_compat.rs` URL construction, `main.rs` dispatch all unchanged).
- Test all changes with `cargo test --workspace` before committing.

---

## File Structure

**Modify:**
- `crates/zoid-model/src/lib.rs:149` — expand `zai-coding-plan` models array from `["glm-5.2"]` to `["glm-5.2", "glm-5-turbo", "glm-4.7"]`.
- `crates/zoid-model/src/lib.rs:209-221` — add two `MODEL_CAPS` entries after the existing `glm-5.2` entry.
- `crates/zoid-model/src/lib.rs:526` — update the `zai_coding_plan_registry_entry_exists_and_is_selectable` test to assert 3 models.
- `crates/zoid-provider/src/openai_compat.rs:804` — add thinking wire-shape tests for both new models after the existing `glm_5_2_thinking_max` test.

**Unchanged:**
- `crates/zoid-provider/src/zai.rs` — no changes.
- `crates/zoid/src/main.rs` — no changes.
- `crates/zoid-tui/src/config_view.rs` — no changes (provider count stays 5).

---

### Task 1: Add MODEL_CAPS entries + update registry

**Files:**
- Modify: `crates/zoid-model/src/lib.rs:209-221` (add two MODEL_CAPS entries)
- Modify: `crates/zoid-model/src/lib.rs:149` (expand models array)
- Modify: `crates/zoid-model/src/lib.rs:526` (update registry test)
- Modify: `crates/zoid-model/src/lib.rs:567-582` (add regression lock tests)

**Interfaces:**
- Consumes: `ModelInfo`, `ThinkingSupport`, `ThinkingWireShape` (existing types).
- Produces: `model_info("glm-5-turbo")` and `model_info("glm-4.7")` return correct capabilities.

- [ ] **Step 1: Write the failing tests**

Add a regression lock test after the existing `glm_5_2_capabilities_locked` test (around line 582, inside `mod thinking_tests`). Insert after the closing `}` of `glm_5_2_capabilities_locked`:

```rust
    #[test]
    fn glm_5_turbo_capabilities_locked() {
        let info = model_info("glm-5-turbo");
        assert_eq!(info.context_window, 262_144);
        assert_eq!(info.max_output, 131_072);
        assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
    }

    #[test]
    fn glm_4_7_capabilities_locked() {
        let info = model_info("glm-4.7");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output, 131_072);
        assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
    }
```

Update the `zai_coding_plan_registry_entry_exists_and_is_selectable` test (line 526) — change the models assertion:

```rust
        assert_eq!(e.models, &["glm-5.2", "glm-5-turbo", "glm-4.7"]);
        assert_eq!(e.models.len(), 3);
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package zoid-model --lib thinking_tests::glm_5_turbo_capabilities_locked -- --exact`

Expected: FAIL with `assertion failed` (left: `0`, right: `262_144` — the model is unknown so it gets the conservative default).

Run: `cargo test --package zoid-model --lib tests::zai_coding_plan_registry_entry_exists_and_is_selectable -- --exact`

Expected: FAIL with `assertion failed` (left: `["glm-5.2"]`, right: `["glm-5.2", "glm-5-turbo", "glm-4.7"]`).

- [ ] **Step 3: Write minimal implementation**

Add two MODEL_CAPS entries after the existing `glm-5.2` entry (after line 221, which is the closing `),` of the glm-5.2 block, before the `// glm-5.1:` comment at line 222):

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

Update the `zai-coding-plan` entry's models array (line 149):

```rust
        models: &["glm-5.2", "glm-5-turbo", "glm-4.7"],
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package zoid-model`

Expected: all tests pass, including:
- `thinking_tests::glm_5_turbo_capabilities_locked` → PASS
- `thinking_tests::glm_4_7_capabilities_locked` → PASS
- `tests::zai_coding_plan_registry_entry_exists_and_is_selectable` → PASS (now asserts 3 models)
- `tests::selectable_has_five_providers` → PASS (provider count unchanged, still 5)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): add glm-5-turbo and glm-4.7 to ZAI Coding Plan

Two new MODEL_CAPS entries confirmed via live API probing:
- glm-5-turbo: 262K context, 131K output, DeepSeek thinking wire
- glm-4.7: 200K context, 131K output, DeepSeek thinking wire

Expands zai-coding-plan models from 1 to 3 (completes the three
models officially supported by ZAI's Coding Plan)."
```

---

### Task 2: Add thinking wire-shape tests for new models

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs:804` (add 4 tests after `glm_5_2_thinking_max_emits_enabled_max`)

**Interfaces:**
- Consumes: `request_body` (existing function in `openai_compat.rs`), `CompletionRequest`, `ThinkingMode`, `EffortLevel`, `Message`.
- Produces: regression tests verifying that `request_body` emits correct DeepSeek-shape thinking params for the two new models.

**Note:** These are characterization tests — they depend on Task 1's MODEL_CAPS entries already being merged. They will pass immediately; there is no implementation step in this task.

- [ ] **Step 1: Write the characterization tests**

Add 6 tests after the existing `glm_5_2_thinking_max_emits_enabled_max` test (which ends around line 804). Insert before the `non_thinking_model_emits_nothing_when_off` test. These mirror the three existing glm-5.2 thinking tests (Off, Auto, Effort(Max)) for each new model:

```rust
    #[test]
    fn glm_5_turbo_thinking_off_emits_disabled_no_effort() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn glm_5_turbo_thinking_auto_emits_enabled_high() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_5_turbo_thinking_max_emits_enabled_max() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn glm_4_7_thinking_off_emits_disabled_no_effort() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn glm_4_7_thinking_auto_emits_enabled_high() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_4_7_thinking_max_emits_enabled_max() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --package zoid-provider --lib openai_compat::tests::glm_5_turbo openai_compat::tests::glm_4_7`

Expected: all 6 tests PASS immediately. These are characterization tests — the models already get `ToggleWithEffort` + `DeepSeek` from Task 1's MODEL_CAPS entries, and `request_body` already maps that to the correct params via the existing `thinking_params()` function. No implementation change is needed.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs
git commit -m "test(provider): glm-5-turbo and glm-4.7 thinking wire-shape tests

Four new tests verifying request_body emits correct DeepSeek-shape
thinking params for both new models (disabled when Off, enabled+high
when Auto). Both models confirmed via live API probing to use the
same wire shape as glm-5.2."
```

---

### Task 3: Run full test suite + verify

**Files:**
- None (verification only).

**Interfaces:**
- Consumes: all previous tasks.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --workspace --no-fail-fast`

Expected: all tests pass. Specifically:
- `zoid-model::thinking_tests::glm_5_turbo_capabilities_locked` → PASS
- `zoid-model::thinking_tests::glm_4_7_capabilities_locked` → PASS
- `zoid-model::tests::zai_coding_plan_registry_entry_exists_and_is_selectable` → PASS
- `zoid-model::tests::selectable_has_five_providers` → PASS
- `zoid-provider::openai_compat::tests::glm_5_turbo_thinking_off_emits_disabled_no_effort` → PASS
- `zoid-provider::openai_compat::tests::glm_5_turbo_thinking_auto_emits_enabled_high` → PASS
- `zoid-provider::openai_compat::tests::glm_5_turbo_thinking_max_emits_enabled_max` → PASS
- `zoid-provider::openai_compat::tests::glm_4_7_thinking_off_emits_disabled_no_effort` → PASS
- `zoid-provider::openai_compat::tests::glm_4_7_thinking_auto_emits_enabled_high` → PASS
- `zoid-provider::openai_compat::tests::glm_4_7_thinking_max_emits_enabled_max` → PASS

**If TUI snapshot tests fail** (e.g. `shell_snapshot__provider_switch_card`), the new models may appear in rendered model lists. Accept updated snapshots:

```bash
INSTA_UPDATE=always cargo test --package zoid-tui --test shell_snapshot
```

Run: `cargo fmt --check`

Expected: no output for changed files (pre-existing formatting issues in other files may appear but are out of scope).

**Note on model picker population:** The TUI model list for `zai-coding-plan` is populated via the registry-driven `models_for()` → `model_options()` path. No separate test is needed — adding the models to the registry entry's `models` array (Task 1) automatically makes them appear in the picker. The existing `model_options_list_registry_models` test covers the `anthropic-api` path; the same code path serves all providers.

- [ ] **Step 2: Smoke test with live API (optional)**

If the ZAI key is configured in the encrypted secret store, rebuild and restart zoid:

```bash
cargo build --release
```

In the TUI, open the provider picker (Alt+P), select `zai · coding plan`, and verify `glm-5-turbo` and `glm-4.7` appear in the model list. Send a test message with each model to confirm streaming works. Toggle thinking on and verify the thinking section appears.

**Note on integration testing:** This smoke test is manual. The plan does not include an automated integration test that hits the live ZAI endpoint. The recording-server tests in `zai.rs` (from the predecessor plan) already verify the request path construction; these tests verify the request body construction.
