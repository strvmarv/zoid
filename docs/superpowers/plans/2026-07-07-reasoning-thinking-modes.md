# Reasoning / Thinking Modes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable reasoning/thinking modes for models that support them — the model reasons internally and produces higher-quality answers, with reasoning content silently discarded in Phase 1.

**Architecture:** A provider-agnostic `ThinkingMode` enum on `CompletionRequest` maps to each provider's native wire shape (Anthropic thinking blocks, DeepSeek reasoning_effort, OpenAI reasoning_effort, Ollama think). A `[thinking]` config table + `ZOID_THINKING` env var drive the setting. The model registry (`zoid-model`) gains `ThinkingSupport` + `ThinkingWireShape` capability flags so providers know which params to emit and the agent loop gates thinking for unsupported models.

**Tech Stack:** Rust, workspace crates: `zoid-model` (model registry), `zoid-provider` (provider seam), `zoid-core` (config), `zoid` (agent loop + main binary). TDD with `cargo test`.

## Global Constraints

- No new workspace dependencies. Use existing serde/serde_json/anyhow/tokio.
- Non-thinking path must stay byte-identical: `ThinkingMode::Off` (the default) produces the exact same wire JSON as today.
- Unknown models default to `ThinkingSupport::None` / `ThinkingWireShape::None` — never send thinking params to a model that might not handle them.
- `budget_tokens` must be `< max_tokens` for Anthropic budget models.
- Phase 1 does NOT add any new `ProviderEvent` variant — reasoning is consumed and discarded by the parse layer.
- The `EffortLevel` type is defined in `zoid-provider` (on the provider seam) and re-used by `zoid-core`'s config via a re-export. This avoids a circular dependency (zoid-core already depends on zoid-provider).
- `max_tokens` when thinking is on is capped at `min(model.max_output, 16384)` — 16384 is enough for reasoning + answer in coding tasks, and avoids exceeding DeepSeek's 64K thinking-mode limit.
- **Verify-during-implementation:** Whether `anthropic-beta: extended-thinking-2025-05-14` header is needed for each Claude model. The plan adds it for Budget models; if newer models don't need it, the verify step confirms and the code can skip it for Adaptive models.
- **Note on `ThinkingConfig` name collision:** `zoid-provider/src/anthropic/types.rs` defines `ThinkingConfig` (the wire struct: `{type, budget_tokens, effort}`) and `zoid-core/src/config.rs` defines `ThinkingConfig` (the config struct: `{enabled, effort}`). These are different types in different crates. The config one could be renamed `ThinkingSettings` but that would break the `EconomyConfig` naming convention. Leave both as `ThinkingConfig` — they're disambiguated by their crate path.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/zoid-provider/src/lib.rs` | `ThinkingMode`, `EffortLevel` enums; `CompletionRequest.thinking` field |
| `crates/zoid-model/src/lib.rs` | `ThinkingSupport`, `ThinkingWireShape` enums; `ModelInfo.thinking` + `.thinking_wire` fields; `MODEL_CAPS` entries |
| `crates/zoid-provider/src/anthropic/types.rs` | Extended `ThinkingType` (add `Disabled`, `Adaptive`); `ThinkingConfig` (add `budget_tokens: Option`, `effort: Option`) |
| `crates/zoid-provider/src/anthropic/request.rs` | `build()` maps `ThinkingMode` → `ThinkingConfig` using `ThinkingSupport` |
| `crates/zoid-provider/src/anthropic/mod.rs` | Pass thinking-derived betas to the provider when thinking is enabled on budget models |
| `crates/zoid-provider/src/openai_compat.rs` | `request_body()` emits DeepSeek or OpenAI thinking params based on `ThinkingWireShape`; discard test for `reasoning_content` |
| `crates/zoid-provider/src/ollama.rs` | `request_body()` emits `think: bool` based on `ThinkingMode` |
| `crates/zoid-core/src/config.rs` | `ThinkingConfig` struct; `PartialThinking`; `[thinking]` TOML table; merge; `Provenance` entries |
| `crates/zoid/src/agent.rs` | `TurnConfig.thinking` field; `build_request` passes `thinking` + derives `max_tokens` |
| `crates/zoid/src/main.rs` | `ZOID_THINKING` env override; `resolve_thinking()`; `spawn_turn` wiring; config UI rows; `field_target`/`current_write`/`ConfigToggle` mappings |
| `crates/zoid-tui/src/config_view.rs` | `build_sections` adds thinking rows to the Provider & Model section |

---

### Task 1: `ThinkingMode` and `EffortLevel` on `CompletionRequest`

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs`

**Interfaces:**
- Produces: `pub enum ThinkingMode { Off, Auto, Effort(EffortLevel) }`, `pub enum EffortLevel { Low, Medium, High, Max }`, `CompletionRequest.thinking: ThinkingMode`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-provider/src/lib.rs`:

```rust
#[test]
fn thinking_mode_off_is_default() {
    let req = CompletionRequest {
        model: "m".into(),
        system: None,
        messages: vec![Message::user("hi")],
        max_tokens: 8,
        tools: vec![],
        thinking: ThinkingMode::Off,
    };
    assert_eq!(req.thinking, ThinkingMode::Off);
}

#[test]
fn effort_level_variants_exist() {
    assert_ne!(EffortLevel::Low, EffortLevel::High);
    assert_ne!(EffortLevel::Medium, EffortLevel::Max);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider thinking_mode_off_is_default`
Expected: FAIL with "missing field `thinking`" or "cannot find type `ThinkingMode`"

- [ ] **Step 3: Write minimal implementation**

Add after the `ToolCall` struct definition in `crates/zoid-provider/src/lib.rs`:

```rust
/// Reasoning effort level for models that support granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

/// Controls whether and how the model reasons (thinks) before answering.
/// Phase 1: reasoning content is consumed and discarded by each provider's
/// parse layer — never surfaced to the agent loop or UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Thinking disabled (today's behavior — the default).
    Off,
    /// Thinking enabled; derive budget/effort from model capabilities + context.
    Auto,
    /// Thinking enabled at a specific effort level.
    Effort(EffortLevel),
}

impl Default for ThinkingMode {
    fn default() -> Self {
        ThinkingMode::Off
    }
}
```

Add `thinking: ThinkingMode` to `CompletionRequest`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
    pub thinking: ThinkingMode,
}
```

Now fix every `CompletionRequest { ... }` construction site that doesn't set `thinking`. Search for them:

Run: `grep -rn "CompletionRequest {" crates/ src/ --include="*.rs" | grep -v "thinking"`

Add `thinking: ThinkingMode::Off` (or `thinking: ThinkingMode::default()`) to every construction site that lacks it. The main ones are in test code across:
- `crates/zoid-provider/src/lib.rs` (the `tests` module `fake_streams_scripted_events_in_order` test)
- `crates/zoid-provider/src/openai_compat.rs` (multiple test `probe_req()` and test functions)
- `crates/zoid-provider/src/ollama.rs` (test `probe_req()` and test functions)
- `crates/zoid-provider/src/anthropic/mod.rs` (test `probe_req()`)
- `crates/zoid-provider/src/opencode_go.rs` (test functions)
- `crates/zoid/src/agent.rs` (test functions and `build_request`)
- `crates/zoid/src/main.rs` (test `test_app()` and `probe_req` if any)

For `build_request` in `crates/zoid/src/agent.rs`, set `thinking: ThinkingMode::Off` for now (Task 8 will replace this).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider thinking_mode`
Expected: PASS

- [ ] **Step 5: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass (every construction site has been fixed).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add ThinkingMode/EffortLevel enums to CompletionRequest

ThinkingMode::{Off, Auto, Effort(level)} defaults to Off so the
non-thinking path stays byte-identical. Every CompletionRequest
construction site is updated with the new field."
```

---

### Task 2: `ThinkingSupport` and `ThinkingWireShape` on `ModelInfo`

**Files:**
- Modify: `crates/zoid-model/src/lib.rs`

**Interfaces:**
- Produces: `ThinkingSupport` enum, `ThinkingWireShape` enum, `ModelInfo.thinking: ThinkingSupport`, `ModelInfo.thinking_wire: ThinkingWireShape`
- Consumes: nothing from earlier tasks

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-model/src/lib.rs`:

```rust
#[test]
fn claude_models_have_thinking_support() {
    // claude-sonnet-4-6: budget-style thinking (older generation)
    let sonnet = model_info("claude-sonnet-4-6");
    assert_eq!(sonnet.thinking, ThinkingSupport::Budget);
    assert_eq!(sonnet.thinking_wire, ThinkingWireShape::Anthropic);

    // claude-opus-4-8: adaptive thinking (newest generation)
    let opus = model_info("claude-opus-4-8");
    assert_eq!(opus.thinking, ThinkingSupport::Adaptive);
    assert_eq!(opus.thinking_wire, ThinkingWireShape::Anthropic);
}

#[test]
fn deepseek_models_have_thinking_support() {
    let pro = model_info("deepseek-v4-pro");
    assert_eq!(pro.thinking, ThinkingSupport::ToggleWithEffort);
    assert_eq!(pro.thinking_wire, ThinkingWireShape::DeepSeek);

    let flash = model_info("deepseek-v4-flash");
    assert_eq!(flash.thinking, ThinkingSupport::ToggleWithEffort);
    assert_eq!(flash.thinking_wire, ThinkingWireShape::DeepSeek);
}

#[test]
fn glm_models_have_no_thinking() {
    let glm = model_info("glm-5.2");
    assert_eq!(glm.thinking, ThinkingSupport::None);
    assert_eq!(glm.thinking_wire, ThinkingWireShape::None);
}

#[test]
fn unknown_model_defaults_to_no_thinking() {
    let info = model_info("some-unknown-model");
    assert_eq!(info.thinking, ThinkingSupport::None);
    assert_eq!(info.thinking_wire, ThinkingWireShape::None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-model claude_models_have_thinking`
Expected: FAIL with "no field `thinking` on type `ModelInfo`" or similar

- [ ] **Step 3: Write minimal implementation**

Add the enums after the `ModelInfo` struct definition in `crates/zoid-model/src/lib.rs`:

```rust
/// Whether and how a model supports reasoning/thinking modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingSupport {
    /// Model doesn't support thinking.
    None,
    /// On/off only (Ollama).
    Toggle,
    /// On/off + effort levels (DeepSeek, OpenAI).
    ToggleWithEffort,
    /// On/off + token budget (Anthropic older models — 4.5, earlier).
    Budget,
    /// Always-on adaptive; effort controls depth (Anthropic newest).
    Adaptive,
}

/// Which native param shape the provider emits for thinking.
/// Drives the OpenAI-compat builder to distinguish DeepSeek from OpenAI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingWireShape {
    /// No thinking params on the wire.
    None,
    /// Anthropic: thinking: {type, budget_tokens?, effort?}
    Anthropic,
    /// DeepSeek: thinking: {type} + reasoning_effort
    DeepSeek,
    /// OpenAI: reasoning_effort
    OpenAI,
    /// Ollama: think: bool
    Ollama,
}
```

Add fields to `ModelInfo`:

```rust
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output: u64,
    pub tools: bool,
    pub prompt_cache: bool,
    pub thinking: ThinkingSupport,
    pub thinking_wire: ThinkingWireShape,
}
```

Update `DEFAULT_MODEL_INFO`:

```rust
const DEFAULT_MODEL_INFO: ModelInfo = ModelInfo {
    context_window: 32_000,
    max_output: 0,
    tools: true,
    prompt_cache: false,
    thinking: ThinkingSupport::None,
    thinking_wire: ThinkingWireShape::None,
};
```

Update every `MODEL_CAPS` entry. For each model:

- `claude-sonnet-4-6`: `thinking: ThinkingSupport::Budget, thinking_wire: ThinkingWireShape::Anthropic`
- `claude-opus-4-8`: `thinking: ThinkingSupport::Adaptive, thinking_wire: ThinkingWireShape::Anthropic`
- `glm-5.2:cloud`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `deepseek-v4-pro`: `thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::DeepSeek`
- `glm-5.2`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `glm-5.1`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `kimi-k2.7-code`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `kimi-k2.6`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `deepseek-v4-flash`: `thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::DeepSeek`
- `mimo-v2.5`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `mimo-v2.5-pro`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `minimax-m3`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `minimax-m2.7`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `minimax-m2.5`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `qwen3.7-max`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`
- `qwen3.7-plus`: `thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None`

Also update the `fetch_model_info` override in `crates/zoid-provider/src/ollama.rs` (around line 426) — the `ModelInfo` construction there needs the new fields:

```rust
Ok(window.map(|w| crate::model::ModelInfo {
    context_window: w,
    max_output: 0,
    tools: true,
    prompt_cache: true,
    thinking: crate::model::ThinkingSupport::None,
    thinking_wire: crate::model::ThinkingWireShape::None,
}))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-model`
Expected: PASS (all existing + new tests)

- [ ] **Step 5: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add ThinkingSupport + ThinkingWireShape to ModelInfo

claude-sonnet-4-6 → Budget/Anthropic, claude-opus-4-8 → Adaptive/Anthropic,
deepseek-v4-{pro,flash} → ToggleWithEffort/DeepSeek. All other models
default to None/None. Ollama fetch_model_info override updated."
```

---

### Task 3: Ollama `think` request param

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`

**Interfaces:**
- Consumes: `ThinkingMode` from Task 1
- Produces: `request_body()` emits `think: true` when thinking is enabled

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-provider/src/ollama.rs`:

```rust
#[test]
fn body_emits_think_true_when_thinking_auto() {
    let req = CompletionRequest {
        model: "m".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 8,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = request_body(&req);
    assert_eq!(body["think"], json!(true));
}

#[test]
fn body_emits_think_false_when_thinking_off() {
    let req = CompletionRequest {
        model: "m".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 8,
        tools: vec![],
        thinking: crate::ThinkingMode::Off,
    };
    let body = request_body(&req);
    // think: false is emitted explicitly (not omitted) so the API gets a clear signal
    assert_eq!(body["think"], json!(false));
}

#[test]
fn body_emits_think_true_when_effort_high() {
    let req = CompletionRequest {
        model: "m".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 8,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::High),
    };
    let body = request_body(&req);
    assert_eq!(body["think"], json!(true));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider ollama::tests::body_emits_think`
Expected: FAIL — `think` key not present in the body

- [ ] **Step 3: Write minimal implementation**

In `request_body()` in `crates/zoid-provider/src/ollama.rs`, add the `think` field to the body JSON. The provider is defensive: it only emits `think` when the model supports it (checked via `ThinkingSupport`), even though the capability gate in `resolve_thinking` should have already caught unsupported models:

```rust
    // Only emit `think` for models that support thinking. The capability gate
    // in resolve_thinking should have caught unsupported models, but this is
    // defensive — never send `think: true` to a model that might not handle it.
    let info = crate::model::model_info(&req.model);
    let think = match req.thinking {
        crate::ThinkingMode::Off => false,
        crate::ThinkingMode::Auto | crate::ThinkingMode::Effort(_) => {
            info.thinking != crate::model::ThinkingSupport::None
        }
    };
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "messages": messages,
        "keep_alive": "30m",
        "think": think,
    });
```

This replaces the existing `let mut body = json!({ ... "keep_alive": "30m", });` block.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider ollama::tests::body_emits_think`
Expected: PASS

- [ ] **Step 5: Run full ollama test suite**

Run: `cargo test -p zoid-provider ollama`
Expected: All pass (existing `native_body_has_stream_and_system_leading_message_no_openai_fields` test will need updating — it asserts exact JSON equality and now includes `think: false`. Update the expected JSON in that test to include `"think": false`:

```rust
    let body = request_body(&req);
    assert_eq!(
        body,
        json!({
            "model": "glm-5.2:cloud",
            "stream": true,
            "messages": [
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
            "keep_alive": "30m",
            "think": false,
        })
    );
```

)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ollama): emit think param based on ThinkingMode

think: true when thinking is Auto or Effort, false when Off.
Existing native-body equality test updated to include think: false."
```

---

### Task 4: OpenAI-compat DeepSeek + OpenAI thinking params

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs`

**Interfaces:**
- Consumes: `ThinkingMode` from Task 1, `ThinkingWireShape` from Task 2
- Produces: `request_body()` emits DeepSeek or OpenAI thinking params

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-provider/src/openai_compat.rs`:

```rust
#[test]
fn deepseek_body_emits_thinking_and_effort_when_auto() {
    let req = CompletionRequest {
        model: "deepseek-v4-pro".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = request_body(&req);
    assert_eq!(body["thinking"]["type"], json!("enabled"));
    assert_eq!(body["reasoning_effort"], json!("high"));
}

#[test]
fn deepseek_body_emits_disabled_when_off() {
    let req = CompletionRequest {
        model: "deepseek-v4-flash".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Off,
    };
    let body = request_body(&req);
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn deepseek_body_emits_max_effort() {
    let req = CompletionRequest {
        model: "deepseek-v4-pro".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
    };
    let body = request_body(&req);
    assert_eq!(body["thinking"]["type"], json!("enabled"));
    assert_eq!(body["reasoning_effort"], json!("max"));
}

#[test]
fn deepseek_body_low_effort_maps_to_high() {
    let req = CompletionRequest {
        model: "deepseek-v4-pro".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Low),
    };
    let body = request_body(&req);
    assert_eq!(body["reasoning_effort"], json!("high"));
}

#[test]
fn openai_body_emits_reasoning_effort_when_auto() {
    let req = CompletionRequest {
        model: "o3".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = request_body(&req);
    assert_eq!(body["reasoning_effort"], json!("medium"));
    assert!(body.get("thinking").is_none(), "OpenAI shape must NOT emit a thinking key");
}

#[test]
fn openai_body_emits_xhigh_for_max() {
    let req = CompletionRequest {
        model: "o3".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
    };
    let body = request_body(&req);
    assert_eq!(body["reasoning_effort"], json!("xhigh"));
}

#[test]
fn non_thinking_model_emits_nothing_when_off() {
    let req = CompletionRequest {
        model: "glm-5.2".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Off,
    };
    let body = request_body(&req);
    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn non_thinking_model_emits_nothing_even_when_thinking_on() {
    // A model with ThinkingWireShape::None gets no thinking params even if
    // the request says thinking is on — the capability gate should have
    // caught this, but the provider is defensive.
    let req = CompletionRequest {
        model: "glm-5.2".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 4096,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = request_body(&req);
    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider openai_compat::tests::deepseek_body`
Expected: FAIL — no `thinking` or `reasoning_effort` keys emitted

- [ ] **Step 3: Write minimal implementation**

Add a helper function in `crates/zoid-provider/src/openai_compat.rs` (before `request_body`):

```rust
/// Map `ThinkingMode` + `ThinkingWireShape` to the OpenAI-compat thinking
/// params. Returns `None` for models without thinking support (defensive —
/// the capability gate should have caught this earlier).
fn thinking_params(req: &CompletionRequest) -> Option<Vec<(&'static str, Value)>> {
    let wire = crate::model::model_info(&req.model).thinking_wire;
    match wire {
        crate::model::ThinkingWireShape::DeepSeek => {
            // deepseek-v4-pro is thinking-only: Off silently becomes Auto.
            // The docs say it ignores the toggle; we map Off → enabled to
            // avoid sending a "disabled" that might 400.
            let model_info = crate::model::model_info(&req.model);
            let is_thinking_only = model_info.thinking == crate::model::ThinkingSupport::ToggleWithEffort
                && req.model == "deepseek-v4-pro";
            let effective_thinking = if is_thinking_only && matches!(req.thinking, crate::ThinkingMode::Off) {
                tracing::warn!(model = %req.model, "thinking-only model: Off silently becomes Auto");
                crate::ThinkingMode::Auto
            } else {
                req.thinking
            };
            let mut params = Vec::new();
            let (thinking_type, has_effort) = match effective_thinking {
                crate::ThinkingMode::Off => ("disabled", false),
                crate::ThinkingMode::Auto => ("enabled", true),
                crate::ThinkingMode::Effort(_) => ("enabled", true),
            };
            params.push((
                "thinking",
                json!({ "type": thinking_type }),
            ));
            if has_effort {
                let effort = match effective_thinking {
                    crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "max",
                    _ => "high", // Auto, Low, Medium, High all map to "high"
                };
                params.push(("reasoning_effort", json!(effort)));
            }
            Some(params)
        }
        crate::model::ThinkingWireShape::OpenAI => {
            let effort = match req.thinking {
                crate::ThinkingMode::Off => return Some(vec![("reasoning_effort", json!("none"))]),
                crate::ThinkingMode::Auto => "medium",
                crate::ThinkingMode::Effort(crate::EffortLevel::Low) => "low",
                crate::ThinkingMode::Effort(crate::EffortLevel::Medium) => "medium",
                crate::ThinkingMode::Effort(crate::EffortLevel::High) => "high",
                crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "xhigh",
            };
            Some(vec![("reasoning_effort", json!(effort))])
        }
        _ => None, // None, Anthropic (not used here), Ollama (not used here)
    }
}
```

At the end of `request_body()`, after the tools block, add:

```rust
    if let Some(params) = thinking_params(req) {
        for (key, val) in params {
            body[key] = val;
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider openai_compat::tests::deepseek_body`
Run: `cargo test -p zoid-provider openai_compat::tests::openai_body`
Run: `cargo test -p zoid-provider openai_compat::tests::non_thinking_model`
Expected: All PASS

- [ ] **Step 5: Run full openai_compat test suite**

Run: `cargo test -p zoid-provider openai_compat`
Expected: All pass (existing tests don't set `thinking` so it defaults to `Off`; models used in existing tests like "glm-5.2" and "m" have `ThinkingWireShape::None`, so no params are emitted)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(openai-compat): emit DeepSeek + OpenAI thinking params

DeepSeek: thinking:{type} + reasoning_effort (low/med→high, max→max).
OpenAI: reasoning_effort (none/low/medium/high/xhigh).
Models with ThinkingWireShape::None get no thinking params."
```

---

### Task 5: OpenAI-compat `reasoning_content` discard test

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs`

**Interfaces:**
- Consumes: nothing new
- Produces: regression-guard test pinning that `delta.reasoning_content` is discarded

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-provider/src/openai_compat.rs`:

```rust
#[test]
fn parse_chunk_reasoning_content_is_discarded() {
    // DeepSeek streams reasoning_content alongside content in the same delta.
    // It must NOT produce a ProviderEvent — only delta.content surfaces.
    let data = r#"{"choices":[{"delta":{"content":"answer","reasoning_content":"thinking..."}}]}"#;
    let events = parse_chunk(data, &mut ToolCallAccumulator::new());
    assert_eq!(
        events,
        vec![ProviderEvent::TextDelta("answer".into())],
        "reasoning_content must be silently discarded"
    );
}

#[test]
fn parse_chunk_reasoning_content_alone_yields_nothing() {
    // A delta with ONLY reasoning_content (no content) yields no events.
    let data = r#"{"choices":[{"delta":{"reasoning_content":"deep thoughts"}}]}"#;
    let events = parse_chunk(data, &mut ToolCallAccumulator::new());
    assert!(events.is_empty(), "reasoning-only delta must produce nothing");
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zoid-provider openai_compat::tests::parse_chunk_reasoning`
Expected: PASS (the existing `parse_chunk` already only extracts `delta.content` — `reasoning_content` is naturally discarded. These tests pin that behavior as a regression guard.)

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(openai-compat): pin reasoning_content discard as regression guard

parse_chunk already ignores delta.reasoning_content (only extracts
delta.content). These tests explicitly pin that behavior so a future
change can't accidentally surface reasoning content as a ProviderEvent."
```

---

### Task 6: Anthropic thinking request params + beta header

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/types.rs`
- Modify: `crates/zoid-provider/src/anthropic/request.rs`
- Modify: `crates/zoid-provider/src/anthropic/mod.rs`

**Interfaces:**
- Consumes: `ThinkingMode` from Task 1, `ThinkingSupport` from Task 2
- Produces: `request::build()` maps `ThinkingMode` → `ThinkingConfig` on the wire; `mod.rs` dynamically adds the `extended-thinking` beta header when thinking is enabled on Budget models

**Interfaces:**
- Consumes: `ThinkingMode` from Task 1, `ThinkingSupport` from Task 2
- Produces: `request::build()` maps `ThinkingMode` → `ThinkingConfig` on the wire

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-provider/src/anthropic/request.rs`:

```rust
#[test]
fn thinking_off_emits_no_thinking_key() {
    let r = req(vec![Message::user("x")], vec![], None);
    // req() uses ThinkingMode::Off by default
    let body = serde_json::to_value(build(&r)).unwrap();
    assert!(body.get("thinking").is_none());
}

#[test]
fn thinking_auto_budget_model_emits_enabled_with_budget() {
    let r = CompletionRequest {
        model: "claude-sonnet-4-6".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 16000,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = serde_json::to_value(build(&r)).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
    assert!(budget > 0, "budget must be positive");
    assert!(budget < 16000, "budget must be < max_tokens");
}

#[test]
fn thinking_auto_adaptive_model_emits_adaptive() {
    let r = CompletionRequest {
        model: "claude-opus-4-8".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 16000,
        tools: vec![],
        thinking: crate::ThinkingMode::Auto,
    };
    let body = serde_json::to_value(build(&r)).unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(body.get("budget_tokens").is_none() || body["thinking"].get("budget_tokens").is_none());
}

#[test]
fn thinking_effort_high_budget_model_maps_to_60pct() {
    let r = CompletionRequest {
        model: "claude-sonnet-4-6".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 10000,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::High),
    };
    let body = serde_json::to_value(build(&r)).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
    assert_eq!(budget, 6000, "High effort = 60% of max_tokens");
}

#[test]
fn thinking_effort_max_adaptive_model_emits_effort() {
    let r = CompletionRequest {
        model: "claude-opus-4-8".into(),
        system: None,
        messages: vec![Message::user("x")],
        max_tokens: 16000,
        tools: vec![],
        thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
    };
    let body = serde_json::to_value(build(&r)).unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["effort"], "max");
}
```

Also update the existing `req()` helper to accept a `thinking` parameter, or add a new helper:

```rust
fn req_with_thinking(
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    system: Option<&str>,
    thinking: crate::ThinkingMode,
) -> CompletionRequest {
    CompletionRequest {
        model: "claude-sonnet-4-6".into(),
        system: system.map(String::from),
        messages,
        max_tokens: 1024,
        tools,
        thinking,
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider anthropic::request::tests::thinking_auto`
Expected: FAIL — `thinking` is always `None` in the current `build()`

- [ ] **Step 3: Write minimal implementation**

First, update `ThinkingType` and `ThinkingConfig` in `crates/zoid-provider/src/anthropic/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ThinkingConfig {
    pub r#type: ThinkingType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingType {
    Enabled,
    Disabled,
    Adaptive,
}
```

Then update `build()` in `crates/zoid-provider/src/anthropic/request.rs`:

```rust
pub fn build(req: &CompletionRequest) -> AnthropicRequest {
    let messages: Vec<AnthropicMessage> = req.messages.iter().map(map_message).collect();
    let thinking = build_thinking(req);
    let mut out = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        stream: true,
        messages,
        system: req.system.as_ref().map(|s| {
            vec![SystemBlock {
                kind: SystemBlockKind::Text,
                text: s.clone(),
                cache_control: None,
            }]
        }),
        tools: req.tools.iter().map(tool_def).collect(),
        thinking,
    };
    place_breakpoints(&mut out);
    out
}

/// Map `ThinkingMode` + model capability → `ThinkingConfig` (or `None`).
fn build_thinking(req: &CompletionRequest) -> Option<ThinkingConfig> {
    let info = crate::model::model_info(&req.model);
    match req.thinking {
        crate::ThinkingMode::Off => None,
        crate::ThinkingMode::Auto => match info.thinking {
            crate::model::ThinkingSupport::Budget => {
                let budget = (req.max_tokens as f64 * 0.6) as u32;
                let budget = budget.min(req.max_tokens.saturating_sub(2048));
                Some(ThinkingConfig {
                    r#type: ThinkingType::Enabled,
                    budget_tokens: Some(budget),
                    effort: None,
                })
            }
            crate::model::ThinkingSupport::Adaptive => Some(ThinkingConfig {
                r#type: ThinkingType::Adaptive,
                budget_tokens: None,
                effort: None,
            }),
            _ => None, // model doesn't support thinking
        },
        crate::ThinkingMode::Effort(level) => match info.thinking {
            crate::model::ThinkingSupport::Budget => {
                let pct = match level {
                    crate::EffortLevel::Low => 0.20,
                    crate::EffortLevel::Medium => 0.40,
                    crate::EffortLevel::High => 0.60,
                    crate::EffortLevel::Max => 0.80,
                };
                let budget = (req.max_tokens as f64 * pct) as u32;
                let budget = budget.min(req.max_tokens.saturating_sub(2048));
                Some(ThinkingConfig {
                    r#type: ThinkingType::Enabled,
                    budget_tokens: Some(budget),
                    effort: None,
                })
            }
            crate::model::ThinkingSupport::Adaptive => {
                let effort = match level {
                    crate::EffortLevel::Low => "low",
                    crate::EffortLevel::Medium => "medium",
                    crate::EffortLevel::High => "high",
                    crate::EffortLevel::Max => "max",
                };
                Some(ThinkingConfig {
                    r#type: ThinkingType::Adaptive,
                    budget_tokens: None,
                    effort: Some(effort.into()),
                })
            }
            _ => None,
        },
    }
}
```

Add `use super::types::*;` is already there; add `ThinkingConfig` to the imports if needed. The `ThinkingConfig` import is already via `super::types::*`.

Update the existing `anthropic_request_serializes_minimal_body` test in `types.rs` — the `ThinkingConfig` struct now has `Option` fields, so the existing `ThinkingConfig { r#type: ThinkingType::Enabled, budget_tokens: 10000 }` construction sites need updating. Search for them:

Run: `grep -rn "ThinkingConfig {" crates/`

Fix each to include `effort: None` and make `budget_tokens` an `Option`:

```rust
ThinkingConfig {
    r#type: ThinkingType::Enabled,
    budget_tokens: Some(10000),
    effort: None,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider anthropic`
Expected: All pass

- [ ] **Step 4b: Add dynamic beta header for thinking**

The `AnthropicProvider` holds a static `betas: Vec<String>` set at construction time. We need to dynamically add the `extended-thinking-2025-05-14` beta when thinking is enabled on Budget models. The provider's `stream_with_retries` method calls `request::build(req)` for the body and `self.request_headers()` for the headers. We need to merge per-request betas with the provider's static betas.

Add a helper in `crates/zoid-provider/src/anthropic/request.rs`:

```rust
/// The beta flags needed for thinking on this model, if any.
/// Budget models need `extended-thinking-2025-05-14`; Adaptive models
/// may not need it (verify per model — the header is harmless if sent
/// unnecessarily, so we include it for all thinking-enabled Anthropic requests).
pub fn thinking_betas(req: &CompletionRequest) -> Vec<String> {
    let info = crate::model::model_info(&req.model);
    match req.thinking {
        crate::ThinkingMode::Off => Vec::new(),
        crate::ThinkingMode::Auto | crate::ThinkingMode::Effort(_) => {
            match info.thinking {
                crate::model::ThinkingSupport::Budget
                | crate::model::ThinkingSupport::Adaptive => {
                    vec!["extended-thinking-2025-05-14".into()]
                }
                _ => Vec::new(),
            }
        }
    }
}
```

In `crates/zoid-provider/src/anthropic/mod.rs`, update `stream_with_retries` to merge per-request betas with the provider's static betas. In the `request_headers()` method (or inline in `stream_with_retries`), compute the combined beta list:

```rust
    fn request_headers_with_thinking(&self, req: &CompletionRequest) -> reqwest::header::HeaderMap {
        let mut headers = self.request_headers();
        // Merge per-request thinking betas with the provider's static betas.
        let thinking_betas = request::thinking_betas(req);
        if !thinking_betas.is_empty() {
            let mut all_betas = self.betas.clone();
            for b in &thinking_betas {
                if !all_betas.contains(b) {
                    all_betas.push(b.clone());
                }
            }
            if let Ok(v) = all_betas.join(",").parse() {
                headers.insert("anthropic-beta", v);
            }
        }
        headers
    }
```

Then in `stream_with_retries`, replace `self.request_headers()` with `self.request_headers_with_thinking(req)`:

```rust
        let send = self
            .client
            .post(&url)
            .headers(self.request_headers_with_thinking(req))
            .json(&body)
            .send();
```

Add a test:

```rust
    #[test]
    fn thinking_betas_returns_extended_thinking_for_budget_models() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let betas = request::thinking_betas(&req);
        assert_eq!(betas, vec!["extended-thinking-2025-05-14".to_string()]);
    }

    #[test]
    fn thinking_betas_empty_when_off() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 16000,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        assert!(request::thinking_betas(&req).is_empty());
    }
```

Run: `cargo test -p zoid-provider anthropic::tests::thinking_betas`
Expected: PASS

- [ ] **Step 5: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(anthropic): map ThinkingMode to thinking config on the wire

Budget models: enabled + budget_tokens (60% auto, 20/40/60/80% by effort).
Adaptive models: adaptive type + optional effort string.
Off: no thinking key on the wire."
```

---

### Task 7: Config `[thinking]` table

**Files:**
- Modify: `crates/zoid-core/src/config.rs`

**Interfaces:**
- Consumes: `EffortLevel` from Task 1 (re-exported from zoid-provider)
- Produces: `ThinkingConfig` struct on `Config`, `PartialThinking` on `PartialConfig`, merge/provenance entries

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-core/src/config.rs`:

```rust
#[test]
fn thinking_section_parses_and_merges() {
    let (p, _) = parse_toml("[thinking]\nenabled = true\neffort = \"high\"").unwrap();
    assert!(p.thinking.enabled.unwrap());
    assert_eq!(p.thinking.effort.as_deref(), Some("high"));
    let (cfg, prov) = merge(&[(Source::UserGlobal, p)]);
    assert!(cfg.thinking.enabled);
    assert_eq!(cfg.thinking.effort, Some("high".to_string()));
    assert_eq!(prov.thinking_enabled, Source::UserGlobal);
    assert_eq!(prov.thinking_effort, Source::UserGlobal);
}

#[test]
fn thinking_defaults_to_disabled() {
    let (cfg, prov) = merge(&[]);
    assert!(!cfg.thinking.enabled);
    assert!(cfg.thinking.effort.is_none());
    assert_eq!(prov.thinking_enabled, Source::Default);
    assert_eq!(prov.thinking_effort, Source::Default);
}

#[test]
fn thinking_enabled_without_effort_is_auto() {
    let (p, _) = parse_toml("[thinking]\nenabled = true").unwrap();
    let (cfg, _) = merge(&[(Source::UserGlobal, p)]);
    assert!(cfg.thinking.enabled);
    assert!(cfg.thinking.effort.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core thinking_section`
Expected: FAIL — no `thinking` field on `PartialConfig` / `Config`

- [ ] **Step 3: Write minimal implementation**

Add to `crates/zoid-core/src/config.rs`:

The `ThinkingConfig` struct (for the resolved `Config`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkingConfig {
    pub enabled: bool,
    pub effort: Option<String>,
}
```

Add `pub thinking: ThinkingConfig` to `Config`:

```rust
pub struct Config {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: String,
    pub economy: EconomyConfig,
    pub reduced_motion: bool,
    pub skills: SkillsConfig,
    pub modes: ModesConfig,
    pub companion: CompanionConfig,
    pub thinking: ThinkingConfig,
}
```

Update `Config::default()`:

```rust
thinking: ThinkingConfig::default(),
```

Add `PartialThinking`:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialThinking {
    pub enabled: Option<bool>,
    pub effort: Option<String>,
}
```

Add `pub thinking: PartialThinking` to `PartialConfig`.

Add provenance fields to `Provenance`:

```rust
pub thinking_enabled: Source,
pub thinking_effort: Source,
```

Update `Provenance` construction in `merge()`:

```rust
thinking_enabled: Source::Default,
thinking_effort: Source::Default,
```

In the `merge()` loop, add:

```rust
if let Some(v) = p.thinking.enabled {
    cfg.thinking.enabled = v;
    prov.thinking_enabled = *src;
}
if let Some(v) = &p.thinking.effort {
    cfg.thinking.effort = Some(v.clone());
    prov.thinking_effort = *src;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core thinking`
Expected: PASS

- [ ] **Step 5: Run full zoid-core test suite**

Run: `cargo test -p zoid-core`
Expected: All pass (existing tests that construct `Config::default()` or `Provenance { ... }` need the new fields. Fix them.)

- [ ] **Step 6: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass (fix any `Provenance { ... }` construction sites in main.rs tests)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(config): add [thinking] table with enabled + effort

ThinkingConfig { enabled: bool, effort: Option<String> } defaults to
disabled. PartialThinking + provenance tracking follow the existing
merge pattern."
```

---

### Task 8: `ZOID_THINKING` env override

**Files:**
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `ThinkingConfig` from Task 7
- Produces: `ZOID_THINKING` env var parsed into the env config layer

- [ ] **Step 1: Write the failing test**

This is integration code in `load_config()` which is hard to unit-test (it reads env vars). Instead, write a pure parsing helper and test that. Add a helper function near `load_config()`:

```rust
/// Parse a `ZOID_THINKING` env value into a `PartialThinking` override.
/// Pure — no env access. Returns `None` for empty/unparseable values.
fn parse_thinking_env(val: &str) -> Option<zoid_core::config::PartialThinking> {
    let val = val.trim();
    if val.is_empty() {
        return None;
    }
    let mut pt = zoid_core::config::PartialThinking::default();
    match val.to_ascii_lowercase().as_str() {
        "off" | "disabled" | "false" | "0" => {
            pt.enabled = Some(false);
        }
        "auto" | "on" | "true" | "1" => {
            pt.enabled = Some(true);
        }
        "low" => {
            pt.enabled = Some(true);
            pt.effort = Some("low".into());
        }
        "medium" => {
            pt.enabled = Some(true);
            pt.effort = Some("medium".into());
        }
        "high" => {
            pt.enabled = Some(true);
            pt.effort = Some("high".into());
        }
        "max" => {
            pt.enabled = Some(true);
            pt.effort = Some("max".into());
        }
        _ => return None,
    }
    Some(pt)
}
```

Test:

```rust
#[test]
fn parse_thinking_env_maps_values() {
    use zoid_core::config::PartialThinking;
    assert_eq!(
        parse_thinking_env("off"),
        Some(PartialThinking { enabled: Some(false), effort: None })
    );
    assert_eq!(
        parse_thinking_env("auto"),
        Some(PartialThinking { enabled: Some(true), effort: None })
    );
    assert_eq!(
        parse_thinking_env("high"),
        Some(PartialThinking { enabled: Some(true), effort: Some("high".into()) })
    );
    assert_eq!(
        parse_thinking_env("max"),
        Some(PartialThinking { enabled: Some(true), effort: Some("max".into()) })
    );
    assert!(parse_thinking_env("").is_none());
    assert!(parse_thinking_env("garbage").is_none());
    // case-insensitive
    assert_eq!(
        parse_thinking_env("HIGH"),
        Some(PartialThinking { enabled: Some(true), effort: Some("high".into()) })
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid parse_thinking_env`
Expected: FAIL — function not defined

- [ ] **Step 3: Write minimal implementation**

Add the `parse_thinking_env` function (shown in Step 1) to `crates/zoid/src/main.rs`, near the `load_config()` function.

In `load_config()`, in the env-layer block, add after the existing env vars:

```rust
    if let Ok(v) = std::env::var("ZOID_THINKING") {
        if let Some(pt) = parse_thinking_env(&v) {
            envp.thinking = pt;
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid parse_thinking_env`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add ZOID_THINKING env override

off/disabled/false/0 → disabled, auto/on/true/1 → enabled (auto effort),
low/medium/high/max → enabled + effort. Case-insensitive."
```

---

### Task 9: Agent loop — `TurnConfig.thinking` + `build_request` + `max_tokens`

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Interfaces:**
- Consumes: `ThinkingMode` from Task 1, `ThinkingSupport` from Task 2
- Produces: `TurnConfig.thinking` field, `build_request` passes thinking + derives max_tokens

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid/src/agent.rs`:

```rust
#[test]
fn build_request_passes_thinking_off() {
    let req = build_request_with_thinking(
        &crate::eventlog::EventLog::new(),
        "m",
        &zoid_tools::registry(),
        "SYS",
        zoid_provider::ThinkingMode::Off,
    );
    assert_eq!(req.thinking, zoid_provider::ThinkingMode::Off);
    assert_eq!(req.max_tokens, 4096, "Off keeps the existing max_tokens");
}

#[test]
fn build_request_passes_thinking_auto_raises_max_tokens() {
    let req = build_request_with_thinking(
        &crate::eventlog::EventLog::new(),
        "claude-sonnet-4-6",
        &zoid_tools::registry(),
        "SYS",
        zoid_provider::ThinkingMode::Auto,
    );
    assert_eq!(req.thinking, zoid_provider::ThinkingMode::Auto);
    assert!(req.max_tokens > 4096, "thinking on raises max_tokens");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid build_request_passes_thinking`
Expected: FAIL — `build_request_with_thinking` not defined

- [ ] **Step 3: Write minimal implementation**

Add `thinking: ThinkingMode` to `TurnConfig` in `crates/zoid/src/agent.rs`:

```rust
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
    pub policy: zoid_core::assembler::ContextPolicy,
    pub eviction: zoid_core::eviction::EvictionPolicy,
    pub thinking: ThinkingMode,
}
```

Update `chat_turn_config_with()` to set `thinking: ThinkingMode::Off`:

```rust
pub fn chat_turn_config_with(profile: &AgentProfile, skill_menu: &str) -> TurnConfig {
    // ... existing ...
    TurnConfig {
        system,
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
        policy: zoid_core::assembler::ContextPolicy::default(),
        eviction: zoid_core::eviction::EvictionPolicy::disabled(),
        thinking: ThinkingMode::Off,
    }
}
```

Update `build_request` to accept `thinking` and derive `max_tokens`:

```rust
pub fn build_request(
    events: &crate::eventlog::EventLog,
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
) -> CompletionRequest {
    build_request_with_thinking(events, model, tools, system, ThinkingMode::Off)
}

pub fn build_request_with_thinking(
    events: &crate::eventlog::EventLog,
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
    thinking: ThinkingMode,
) -> CompletionRequest {
    let system = match zoid_core::eviction::eviction_breadcrumb(events.iter()) {
        Some(bc) => format!("{system}\n\n{bc}"),
        None => system.to_string(),
    };
    let max_tokens = match thinking {
        ThinkingMode::Off => 4096,
        ThinkingMode::Auto | ThinkingMode::Effort(_) => {
            let info = zoid_provider::model::model_info(model);
            // Cap at 16384: enough for reasoning + answer in coding tasks,
            // and avoids exceeding DeepSeek's 64K thinking-mode limit when
            // max_output is 384K.
            if info.max_output > 0 {
                (info.max_output as u32).min(16384)
            } else {
                16384
            }
        }
    };
    CompletionRequest {
        model: model.to_string(),
        system: Some(system),
        messages: conversation(events.iter())
            .into_iter()
            .map(map_msg)
            .collect(),
        max_tokens,
        tools: tool_specs(tools),
        thinking,
    }
}
```

Update the call site in `run_turn_inner` (around line 408):

```rust
let req = build_request_with_thinking(&events, &model, &tools, &config.system, config.thinking);
```

Fix any other `build_request(` call sites in tests — they use the zero-arg `build_request` which still works (defaults to `Off`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid build_request_passes_thinking`
Expected: PASS

- [ ] **Step 5: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(agent): add thinking to TurnConfig and build_request

TurnConfig.thinking defaults to Off (byte-identical behavior).
build_request_with_thinking derives max_tokens from model's max_output
(or 16384) when thinking is on, keeping 4096 when off."
```

---

### Task 10: `resolve_thinking()` + `spawn_turn` wiring

**Files:**
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `ThinkingConfig` from Task 7, `ThinkingSupport` from Task 2, `ThinkingMode` from Task 1
- Produces: `resolve_thinking()` function; `spawn_turn` sets `turn_config.thinking`

- [ ] **Step 1: Write the failing test**

Add a pure `resolve_thinking` function near `spawn_turn()`:

```rust
/// Resolve the final `ThinkingMode` from config + model capability.
/// Pure — takes explicit args so it's unit-testable. If the model's
/// `ThinkingSupport` is `None`, thinking is forced off even if config
/// says enabled (safety guard).
fn resolve_thinking(
    config_thinking: &zoid_core::config::ThinkingConfig,
    model_support: zoid_provider::model::ThinkingSupport,
) -> ThinkingMode {
    match model_support {
        zoid_provider::model::ThinkingSupport::None => ThinkingMode::Off,
        _ => {
            if !config_thinking.enabled {
                ThinkingMode::Off
            } else {
                match &config_thinking.effort {
                    None => ThinkingMode::Auto,
                    Some(e) => {
                        let level = match e.as_str() {
                            "low" => EffortLevel::Low,
                            "medium" => EffortLevel::Medium,
                            "high" => EffortLevel::High,
                            "max" => EffortLevel::Max,
                            _ => EffortLevel::High, // unknown → default to high
                        };
                        ThinkingMode::Effort(level)
                    }
                }
            }
        }
    }
}
```

Test:

```rust
#[test]
fn resolve_thinking_forces_off_when_unsupported() {
    let cfg = zoid_core::config::ThinkingConfig {
        enabled: true,
        effort: Some("high".into()),
    };
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::None);
    assert_eq!(mode, zoid_provider::ThinkingMode::Off);
}

#[test]
fn resolve_thinking_off_when_config_disabled() {
    let cfg = zoid_core::config::ThinkingConfig {
        enabled: false,
        effort: None,
    };
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::Budget);
    assert_eq!(mode, zoid_provider::ThinkingMode::Off);
}

#[test]
fn resolve_thinking_auto_when_enabled_no_effort() {
    let cfg = zoid_core::config::ThinkingConfig {
        enabled: true,
        effort: None,
    };
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::Budget);
    assert_eq!(mode, zoid_provider::ThinkingMode::Auto);
}

#[test]
fn resolve_thinking_effort_when_enabled_with_effort() {
    let cfg = zoid_core::config::ThinkingConfig {
        enabled: true,
        effort: Some("max".into()),
    };
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::Adaptive);
    assert_eq!(mode, zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::Max));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid resolve_thinking`
Expected: FAIL — function not defined

- [ ] **Step 3: Write minimal implementation**

Add the `resolve_thinking` function (shown in Step 1) to `crates/zoid/src/main.rs`.

In `spawn_turn()`, after the `turn_config.eviction = ...` block, add:

```rust
    // Resolve thinking mode from config + model capability.
    let model_support = app
        .fetched_model_info
        .map(|info| info.thinking)
        .unwrap_or_else(|| zoid_provider::model::model_info(&app.model).thinking);
    turn_config.thinking = resolve_thinking(&app.config.thinking, model_support);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid resolve_thinking`
Expected: PASS

- [ ] **Step 5: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: resolve_thinking gates thinking by model capability

spawn_turn resolves ThinkingMode from config + model's ThinkingSupport.
Unsupported models force Off even when config says enabled (safety guard)."
```

---

### Task 11: Config UI — thinking toggle + effort picker

**Files:**
- Modify: `crates/zoid-tui/src/config_view.rs`
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `ThinkingConfig` from Task 7, `Provenance` thinking fields from Task 7
- Produces: thinking rows in the config screen; `field_target`/`current_write`/`ConfigToggle`/`ConfigDrillOpen` mappings

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-tui/src/config_view.rs`:

```rust
#[test]
fn thinking_rows_appear_when_enabled() {
    let cfg = Config {
        thinking: zoid_core::config::ThinkingConfig {
            enabled: true,
            effort: Some("high".into()),
        },
        ..Config::default()
    };
    let prov = Provenance {
        thinking_enabled: Source::UserGlobal,
        thinking_effort: Source::UserGlobal,
        ..Provenance::default_test()
    };
    let sections = build_sections(&cfg, &prov, &[]);
    // The thinking rows should be in one of the sections
    let thinking_enabled_row = sections
        .iter()
        .flat_map(|s| s.rows.iter())
        .find(|r| r.label == "thinking")
        .expect("thinking row must exist");
    assert!(matches!(thinking_enabled_row.kind, FieldKind::Bool));
    assert_eq!(thinking_enabled_row.value, "on");
}
```

Note: `Provenance::default_test()` doesn't exist — use the full `Provenance { ... }` construction with all `Source::Default` and the new fields set.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui thinking_rows`
Expected: FAIL — no "thinking" row in the sections

- [ ] **Step 3: Write minimal implementation**

In `build_sections()` in `crates/zoid-tui/src/config_view.rs`, add thinking rows to the "Provider & Model" section (after the connection row):

```rust
            connection_row,
            FieldRow {
                label: "thinking",
                value: onoff(cfg.thinking.enabled),
                kind: FieldKind::Bool,
                source: prov.thinking_enabled,
                env_shadowed: prov.thinking_enabled == Source::Env,
            },
            FieldRow {
                label: "effort",
                value: cfg.thinking.effort.clone().unwrap_or_else(|| "(auto)".into()),
                kind: FieldKind::Pick,
                source: prov.thinking_effort,
                env_shadowed: prov.thinking_effort == Source::Env,
            },
        ],
    };
```

In `crates/zoid/src/main.rs`, the `thinking` bool persists via the `ConfigToggle` action (same pattern as `auto-evict cold` and `reduced motion`). Do NOT add a `field_target` entry for `thinking` — `field_target` is only for text-edit fields, and `TomlTy` has no `Bool` variant. Add it to the `ConfigToggle` handler instead:

```rust
        Action::ConfigToggle => {
            // ... existing ...
            if let Some((label, _kind)) = current_config_field(app) {
                let write = match label {
                    "auto-evict cold" => Some((
                        "economy.auto_evict_cold",
                        !app.config.economy.auto_evict_cold,
                    )),
                    "reduced motion" => Some(("reduced_motion", !app.config.reduced_motion)),
                    "thinking" => Some((
                        "thinking.enabled",
                        !app.config.thinking.enabled,
                    )),
                    _ => None,
                };
                // ... existing apply ...
            }
        }
```

Add the `effort` picker to `ConfigDrillOpen`:

```rust
        Action::ConfigDrillOpen => {
            // ... existing ...
            app.shell.config_picker = match label {
                "provider" => zoid_tui::config_view::provider_options(&app.config.provider),
                "model" => zoid_tui::config_view::model_options(&app.config.provider, &app.config.model),
                "effort" => {
                    let cur = app.config.thinking.effort.clone().unwrap_or_default();
                    vec![
                        zoid_tui::config_view::PickOption {
                            id: "".into(),
                            label: "(auto)".into(),
                            detail: String::new(),
                            selectable: true,
                            is_current: cur.is_empty(),
                        },
                        zoid_tui::config_view::PickOption {
                            id: "low".into(),
                            label: "low".into(),
                            detail: String::new(),
                            selectable: true,
                            is_current: cur == "low",
                        },
                        zoid_tui::config_view::PickOption {
                            id: "medium".into(),
                            label: "medium".into(),
                            detail: String::new(),
                            selectable: true,
                            is_current: cur == "medium",
                        },
                        zoid_tui::config_view::PickOption {
                            id: "high".into(),
                            label: "high".into(),
                            detail: String::new(),
                            selectable: true,
                            is_current: cur == "high",
                        },
                        zoid_tui::config_view::PickOption {
                            id: "max".into(),
                            label: "max".into(),
                            detail: String::new(),
                            selectable: true,
                            is_current: cur == "max",
                        },
                    ]
                },
                _ => Vec::new(),
            };
            // ... existing ...
        }
```

Add the `effort` picker selection to `ConfigPickerSelect`:

```rust
        Action::ConfigPickerSelect => {
            // ... existing ...
            if let Some(id) = chosen {
                if label == "provider" {
                    // ... existing ...
                } else if label == "model" {
                    // ... existing ...
                } else if label == "effort" {
                    use zoid_core::config::TomlValue;
                    if id.is_empty() {
                        // "(auto)" → unset effort
                        apply_config_write(app, "thinking.effort", TomlValue::Unset, false);
                    } else {
                        apply_config_write(app, "thinking.effort", TomlValue::Str(id), false);
                    }
                    app.shell.config_picker.clear();
                    app.shell.config_col = ConfigCol::Fields;
                }
            }
        }
```

Add `current_write` entries:

```rust
        "thinking" => (
            "thinking.enabled",
            TomlValue::Bool(app.config.thinking.enabled),
        ),
        "effort" => (
            "thinking.effort",
            app.config
                .thinking
                .effort
                .clone()
                .map(TomlValue::Str)
                .unwrap_or(TomlValue::Unset),
        ),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui thinking_rows`
Expected: PASS

- [ ] **Step 5: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All pass (fix any `Provenance { ... }` construction sites in tests that don't include the new `thinking_enabled` / `thinking_effort` fields)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ui): add thinking toggle + effort picker to config screen

thinking: Bool toggle (persists via ConfigToggle).
effort: Pick with (auto)/low/medium/high/max options.
Both write to the [thinking] table in config.toml."
```

---

### Task 12: Agent loop integration test — `RecordingProvider`

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-10
- Produces: integration test verifying `max_tokens` + `thinking` mode reach the provider

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid/src/agent.rs`:

```rust
/// A provider that records the last `CompletionRequest` it received.
struct RecordingProvider {
    last_req: std::sync::Arc<std::sync::Mutex<Option<CompletionRequest>>>,
    scripted: Vec<ProviderEvent>,
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        *self.last_req.lock().unwrap() = Some(req.clone());
        for ev in &self.scripted {
            if sink.send(ev.clone()).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn thinking_auto_raises_max_tokens_and_passes_mode() {
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(None));
    let provider = std::sync::Arc::new(RecordingProvider {
        last_req: recorded.clone(),
        scripted: vec![
            ProviderEvent::TextDelta("done".into()),
            ProviderEvent::Done,
        ],
    });
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::from(1u128),
        None,
        1,
        EventKind::UserMessage { text: "hi".into() },
    )];
    for e in &seed {
        session.append(e.clone()).await.unwrap();
    }
    let mut cfg = chat_turn_config();
    cfg.thinking = ThinkingMode::Auto;
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let _ = run_agent_turn(
        cfg,
        provider,
        std::sync::Arc::new(zoid_tools::registry()),
        std::sync::Arc::new(zoid_tools::AllowAll),
        session,
        crate::eventlog::EventLog::from_vec(seed),
        "claude-sonnet-4-6".into(),
        tx,
        Ulid::new(),
        zoid_companion::CompanionHub::new(),
        || 0,
    )
    .await
    .unwrap();
    let captured = recorded.lock().unwrap().clone().expect("provider was called");
    assert_eq!(captured.thinking, ThinkingMode::Auto);
    assert!(
        captured.max_tokens > 4096,
        "thinking on should raise max_tokens, got {}",
        captured.max_tokens
    );
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zoid thinking_auto_raises_max_tokens`
Expected: PASS (all the pieces are in place from Tasks 1-10)

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(agent): verify thinking mode + raised max_tokens reach provider

RecordingProvider captures the CompletionRequest. ThinkingMode::Auto on
a Budget model raises max_tokens above 4096 and passes the mode through."
```

---

### Task 13: Capability gating test

**Files:**
- Modify: `crates/zoid/src/main.rs` (tests module)

**Interfaces:**
- Consumes: `resolve_thinking` from Task 10

- [ ] **Step 1: Write the test**

This was already covered by the `resolve_thinking_forces_off_when_unsupported` test in Task 10. Add one more test for model switch:

```rust
#[test]
fn model_switch_from_thinking_to_non_thinking_forces_off() {
    // Config says enabled + high effort
    let cfg = zoid_core::config::ThinkingConfig {
        enabled: true,
        effort: Some("high".into()),
    };
    // Budget model → thinking active
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::Budget);
    assert_eq!(mode, zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::High));
    // Switch to None model → forced off
    let mode = resolve_thinking(&cfg, zoid_provider::model::ThinkingSupport::None);
    assert_eq!(mode, zoid_provider::ThinkingMode::Off);
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zoid model_switch_from_thinking`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: capability gating forces thinking off on model switch"
```

---

### Task 14: Final workspace test + clippy

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No new warnings

- [ ] **Step 3: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix clippy warnings from thinking modes implementation"
```

- [ ] **Step 4: Final commit (if nothing to fix)**

The implementation is complete. All tasks are done.