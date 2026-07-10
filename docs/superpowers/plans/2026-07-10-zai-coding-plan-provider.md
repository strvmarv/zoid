# ZAI Coding Plan Provider + GLM 5.2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `zai-coding-plan` provider with `glm-5.2` as the sole model, using ZAI's OpenAI-compatible Coding Plan endpoint at `https://api.z.ai/api/coding/paas/v4`.

**Architecture:** A thin `ZaiProvider` module delegates to the existing `OpenAICompatProvider` with a new configurable `path_prefix` field (default `"/v1"`), overridden to `""` for ZAI. The `glm-5.2` MODEL_CAPS entry is updated with confirmed capabilities (1M context, 131K max output, DeepSeek-shape thinking). Main.rs wires `zai-coding-plan` via the existing `family`-based dispatch.

**Tech Stack:** Rust, reqwest (HTTP client), tokio (async runtime), serde_json (JSON parsing).

## Global Constraints

- All provider logic lives in `crates/zoid-provider/src/zai.rs`.
- Registry changes in `crates/zoid-model/src/lib.rs`.
- Wiring in `crates/zoid/src/main.rs`.
- No new dependencies.
- No new HTTP/parsing logic — ZAI reuses `openai_compat.rs` entirely.
- Test all changes with `cargo test --workspace` before committing.

---

## File Structure

**Create:**
- `crates/zoid-provider/src/zai.rs` — `ZaiProvider` struct, delegates to `OpenAICompatProvider` with `path_prefix=""`.

**Modify:**
- `crates/zoid-provider/src/lib.rs:1-50` — add `pub mod zai;`.
- `crates/zoid-provider/src/openai_compat.rs:281-310` — add `path_prefix: String` field (default `"/v1"`) and `with_path_prefix(prefix)` builder. Update the two `format!` sites (line 325, line 448) to use `self.path_prefix` instead of hardcoded `"/v1"`.
- `crates/zoid-model/src/lib.rs:88-143` — add `ProviderEntry` for `zai-coding-plan` to `PROVIDERS` array (after `opencode-go`, before `anthropic-api`).
- `crates/zoid-model/src/lib.rs:203-211` — update `glm-5.2` MODEL_CAPS entry: `max_output: 0` → `131_072`, `thinking: ThinkingSupport::None` → `ToggleWithEffort`, `thinking_wire: ThinkingWireShape::None` → `DeepSeek`.
- `crates/zoid/src/main.rs:864-872` — add `Some("zai") => Some("ZAI_API_KEY")` arm to `key_env_for`.
- `crates/zoid/src/main.rs:913-921` — add `"zai" =>` arm to `select_provider` match (mirroring `opencode-go` arm).
- `crates/zoid/src/main.rs:1017-1019` — add `"zai" =>` arm to `provider_for_id` match (mirroring `opencode-go` arm).
- `crates/zoid/src/main.rs:3140-3144` — add `("ZAI_API_KEY", status("ZAI_API_KEY"))` to `key_status` array.

---

### Task 1: Add `path_prefix` field to `OpenAICompatProvider`

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs:281-310` (struct definition + `new` + `with_base_url` + `with_idle_timeout`)
- Modify: `crates/zoid-provider/src/openai_compat.rs:325` (stream POST URL)
- Modify: `crates/zoid-provider/src/openai_compat.rs:448` (list_models GET URL)

**Interfaces:**
- Consumes: nothing (foundational change).
- Produces: `OpenAICompatProvider::with_path_prefix(prefix: impl Into<String>) -> Self`. The default prefix is `"/v1"` (backward-compat).

- [ ] **Step 1: Write the failing test**

Add to the bottom of `crates/zoid-provider/src/openai_compat.rs` (inside the existing `#[cfg(test)] mod tests` block, which starts around line 700):

```rust
#[test]
fn default_path_prefix_is_v1() {
    let p = OpenAICompatProvider::new("k".into());
    assert_eq!(p.path_prefix, "/v1");
}

#[test]
fn with_path_prefix_overrides_default() {
    let p = OpenAICompatProvider::new("k".into()).with_path_prefix("");
    assert_eq!(p.path_prefix, "");
}

#[tokio::test]
async fn default_path_prefix_emits_v1_chat_completions() {
    // Regression: default prefix must still emit /v1/chat/completions.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let recorded_clone = recorded.clone();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req_text = String::from_utf8_lossy(&buf[..n]);
            let first_line = req_text.lines().next().unwrap_or("").to_string();
            *recorded_clone.lock().await = Some(first_line);
            let body = "data: [DONE]\r\n\r\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    let provider = OpenAICompatProvider::new("k".into())
        .with_base_url(format!("http://{addr}"))
        .with_idle_timeout(std::time::Duration::from_secs(2));
    let req = CompletionRequest {
        model: "m".into(),
        system: None,
        messages: vec![crate::Message::user("hi")],
        max_tokens: 8,
        tools: vec![],
        thinking: crate::ThinkingMode::Off,
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let _ = provider.stream(&req, tx).await;
    let first = recorded.lock().await.clone().unwrap_or_default();
    assert!(
        first.contains("/v1/chat/completions"),
        "default prefix must emit /v1/chat/completions, got: {first}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package zoid-provider --lib openai_compat::tests::default_path_prefix_is_v1 -- --exact`

Expected: FAIL with `no field \`path_prefix\` on type \`OpenAICompatProvider\``.

- [ ] **Step 3: Write minimal implementation**

In `crates/zoid-provider/src/openai_compat.rs`, update the struct (around line 281):

```rust
pub struct OpenAICompatProvider {
    api_key: String,
    base_url: String,
    path_prefix: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}
```

Update `new` (around line 289):

```rust
pub fn new(api_key: String) -> Self {
    Self {
        api_key,
        base_url: DEFAULT_BASE_URL.to_string(),
        path_prefix: "/v1".to_string(),
        client: crate::http_client(),
        idle_timeout: crate::stream_idle_timeout(),
    }
}
```

Add `with_path_prefix` builder (after `with_idle_timeout`, around line 308):

```rust
pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
    self.path_prefix = prefix.into();
    self
}
```

Update the two `format!` sites:

Line 325 (inside `stream`):

```rust
.post(format!("{}{}/chat/completions", self.base_url, self.path_prefix))
```

Line 448 (inside `list_models`):

```rust
.get(format!("{}{}/models", self.base_url, self.path_prefix))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package zoid-provider --lib openai_compat::tests -- --exact`

Expected: all three new tests pass; all existing `openai_compat` tests still pass (default `"/v1"` prefix preserves old behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs
git commit -m "feat(provider): parameterize OpenAI-compat path prefix

Add path_prefix field to OpenAICompatProvider (default '/v1') with
with_path_prefix builder. Two format! sites (chat/completions, models)
now use the field instead of hardcoded /v1. Default behavior unchanged;
this enables ZAI's endpoint which uses /chat/completions without /v1."
```

---

### Task 2: Create `ZaiProvider` module

**Files:**
- Create: `crates/zoid-provider/src/zai.rs`
- Modify: `crates/zoid-provider/src/lib.rs:1-50` (add `pub mod zai;`)

**Interfaces:**
- Consumes: `OpenAICompatProvider::new`, `with_base_url`, `with_path_prefix`, `with_idle_timeout`, `stream`, `list_models` (from Task 1).
- Produces: `pub struct ZaiProvider` with `new(api_key)`, `with_base_url(base_url)`, `with_idle_timeout(idle)`, implementing `Provider` trait.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-provider/src/zai.rs` with the test module at the bottom:

```rust
//! The ZAI Coding Plan provider: delegates to OpenAICompatProvider with
//! path_prefix="" (ZAI's endpoint is {base}/chat/completions, no /v1/ segment).

use crate::openai_compat::OpenAICompatProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct ZaiProvider {
    api_key: String,
    base_url: String,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl ZaiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("zai-coding-plan")
                .unwrap_or("https://api.z.ai/api/coding/paas/v4")
                .to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }

    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }
}

#[async_trait]
impl Provider for ZaiProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")
            .with_idle_timeout(self.idle_timeout)
            .stream(req, sink)
            .await
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")
            .with_idle_timeout(self.idle_timeout)
            .list_models()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn new_uses_default_base_url() {
        let p = ZaiProvider::new("k".into());
        assert_eq!(p.base_url, "https://api.z.ai/api/coding/paas/v4");
    }

    #[test]
    fn with_base_url_overrides_and_trims_trailing_slash() {
        let p = ZaiProvider::new("k".into()).with_base_url("https://proxy.test/zai/");
        assert_eq!(p.base_url, "https://proxy.test/zai");
    }

    #[tokio::test]
    async fn zai_list_models_hits_models_without_v1_prefix() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = r#"{"data":[{"id":"glm-5.2"}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = ZaiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let models = provider.list_models().await.unwrap();
        assert_eq!(models, vec!["glm-5.2"]);
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/models") && !first.contains("/v1/models"),
            "ZAI list_models must hit /models (no /v1/), got: {first}"
        );
    }

    #[tokio::test]
    async fn zai_stream_hits_chat_completions_without_v1_prefix() {
        // Recording server: capture the request line, then respond with [DONE].
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: [DONE]\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = ZaiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/chat/completions") && !first.contains("/v1/chat/completions"),
            "ZAI must hit /chat/completions (no /v1/), got: {first}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package zoid-provider --lib zai::tests::new_uses_default_base_url -- --exact`

Expected: FAIL with `unresolved import \`crate::zai\`` or `module \`zai\` not found`.

- [ ] **Step 3: Write minimal implementation**

In `crates/zoid-provider/src/lib.rs`, add `pub mod zai;` near the top (around line 10-20, alongside the other `pub mod` declarations):

```rust
pub mod zai;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package zoid-provider --lib zai::tests`

Expected: all four tests pass (`new_uses_default_base_url`, `with_base_url_overrides_and_trims_trailing_slash`, `zai_list_models_hits_models_without_v1_prefix`, `zai_stream_hits_chat_completions_without_v1_prefix`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/zai.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): add ZaiProvider module

Thin provider that delegates to OpenAICompatProvider with path_prefix='',
reaching ZAI's Coding Plan endpoint at {base}/chat/completions (no /v1/
segment). Includes recording-server test verifying the path."
```

---

### Task 3: Add `zai-coding-plan` to the provider registry

**Files:**
- Modify: `crates/zoid-model/src/lib.rs:88-143` (add `ProviderEntry` to `PROVIDERS` array)

**Interfaces:**
- Consumes: nothing.
- Produces: `zai-coding-plan` entry in `PROVIDERS`, reachable via `entry("zai-coding-plan")`, `default_base_url("zai-coding-plan")`, `models_for("zai-coding-plan")`.

- [ ] **Step 1: Write the failing test**

Add to the bottom of `crates/zoid-model/src/lib.rs` (inside the existing `#[cfg(test)] mod tests` block, after line 502):

```rust
#[test]
fn zai_coding_plan_registry_entry_exists_and_is_selectable() {
    let e = entry("zai-coding-plan").expect("zai-coding-plan entry must exist");
    assert_eq!(e.id, "zai-coding-plan");
    assert_eq!(e.family, "zai");
    assert_eq!(e.status, Status::Available);
    assert_eq!(
        e.transport,
        Transport::Http {
            default_base_url: "https://api.z.ai/api/coding/paas/v4"
        }
    );
    assert_eq!(e.models, &["glm-5.2"]);
    let ids: Vec<&str> = selectable().map(|e| e.id).collect();
    assert!(ids.contains(&"zai-coding-plan"));
}

#[test]
fn selectable_has_five_providers() {
    let ids: Vec<&str> = selectable().map(|e| e.id).collect();
    assert_eq!(ids.len(), 5);
    assert!(ids.contains(&"ollama-local"));
    assert!(ids.contains(&"ollama-cloud"));
    assert!(ids.contains(&"opencode-go"));
    assert!(ids.contains(&"anthropic-api"));
    assert!(ids.contains(&"zai-coding-plan"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package zoid-model --lib tests::zai_coding_plan_registry_entry_exists_and_is_selectable -- --exact`

Expected: FAIL with `zai-coding-plan entry must exist: None`.

- [ ] **Step 3: Write minimal implementation**

In `crates/zoid-model/src/lib.rs`, add the `ProviderEntry` to the `PROVIDERS` array (after the `opencode-go` entry at line 132, before the `anthropic-api` entry at line 133):

```rust
    ProviderEntry {
        id: "zai-coding-plan",
        display: "zai · coding plan",
        family: "zai",
        transport: Transport::Http {
            default_base_url: "https://api.z.ai/api/coding/paas/v4",
        },
        models: &["glm-5.2"],
        status: Status::Available,
    },
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package zoid-model --lib tests::zai_coding_plan_registry_entry_exists_and_is_selectable -- --exact`

Expected: PASS. Also run `cargo test --package zoid-model --lib tests::selectable_has_five_providers -- --exact` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): add zai-coding-plan provider entry

Registry entry for ZAI Coding Plan: id='zai-coding-plan', family='zai',
base_url='https://api.z.ai/api/coding/paas/v4', models=['glm-5.2'].
Updates selectable count from 4 to 5."
```

---

### Task 4: Update `glm-5.2` MODEL_CAPS entry

**Files:**
- Modify: `crates/zoid-model/src/lib.rs:203-211` (update `glm-5.2` entry)
- Modify: `crates/zoid-model/src/lib.rs:540-544` (update `glm_models_have_no_thinking` test)
- Modify: `crates/zoid-model/src/lib.rs:585-587` (update `opencode_go_model_caps_match_reconciled_table` test)
- Modify: `crates/zoid-model/src/lib.rs` (add `glm_5_2_capabilities_locked` regression lock test after thinking tests)

**Interfaces:**
- Consumes: nothing.
- Produces: `model_info("glm-5.2")` returns `max_output: 131_072`, `thinking: ToggleWithEffort`, `thinking_wire: DeepSeek`.

- [ ] **Step 1: Write the failing test**

Update the existing `glm_models_have_no_thinking` test (line 540-544) to `glm_5_2_has_thinking_with_effort`:

```rust
#[test]
fn glm_5_2_has_thinking_with_effort() {
    let glm = model_info("glm-5.2");
    assert_eq!(glm.thinking, ThinkingSupport::ToggleWithEffort);
    assert_eq!(glm.thinking_wire, ThinkingWireShape::DeepSeek);
    assert_eq!(glm.max_output, 131_072);
}
```

Update the `opencode_go_model_caps_match_reconciled_table` test (line 587) — change the `glm-5.2` row:

```rust
("glm-5.2", 1_000_000, 131_072, true, true),
```

Add a regression lock test after the thinking tests (around line 550):

```rust
#[test]
fn glm_5_2_capabilities_locked() {
    let info = model_info("glm-5.2");
    assert_eq!(info.context_window, 1_000_000);
    assert_eq!(info.max_output, 131_072);
    assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
    assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package zoid-model --lib thinking_tests::glm_5_2_has_thinking_with_effort -- --exact`

Expected: FAIL with `assertion failed: (left == right)` (left: `None`, right: `ToggleWithEffort`).

- [ ] **Step 3: Write minimal implementation**

Update the `glm-5.2` MODEL_CAPS entry (line 203-211):

```rust
(
    "glm-5.2",
    ModelInfo {
        context_window: 1_000_000,
        max_output: 131_072,
        tools: true,
        prompt_cache: true,
        thinking: ThinkingSupport::ToggleWithEffort,
        thinking_wire: ThinkingWireShape::DeepSeek,
    },
),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package zoid-model --lib thinking_tests::glm_5_2_has_thinking_with_effort -- --exact`

Expected: PASS. Also run:
- `cargo test --package zoid-model --lib opencode_go_tests::opencode_go_model_caps_match_reconciled_table -- --exact` → PASS
- `cargo test --package zoid-model --lib thinking_tests::glm_5_2_capabilities_locked -- --exact` → PASS
- `cargo test --package zoid-model` → all tests pass (verifies no other tests regress)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(model): update glm-5.2 capabilities from ZAI docs

max_output: 0 → 131_072 (confirmed via OpenRouter / NVIDIA NIM).
thinking: None → ToggleWithEffort, DeepSeek wire shape (confirmed via
live API probing: thinking:{type:'enabled'|'disabled'} + reasoning_effort).
Updates shared glm-5.2 entry used by opencode-go and zai-coding-plan."
```

---

### Task 5: Wire `zai-coding-plan` in main.rs

**Files:**
- Modify: `crates/zoid/src/main.rs:864-872` (`key_env_for`)
- Modify: `crates/zoid/src/main.rs:913-921` (`select_provider`)
- Modify: `crates/zoid/src/main.rs:1017-1019` (`provider_for_id`)
- Modify: `crates/zoid/src/main.rs:3140-3144` (`key_status`)
- Modify: `crates/zoid/src/main.rs:6800-6808` (add test for `key_env_for("zai-coding-plan")`)

**Interfaces:**
- Consumes: `ZaiProvider` (from Task 2), `entry("zai-coding-plan")` (from Task 3).
- Produces: `zai-coding-plan` is selectable in the UI, `ZAI_API_KEY` is recognized as its secret env var, and the provider is constructed when selected.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `crates/zoid/src/main.rs` (after line 6808):

```rust
#[test]
fn key_env_for_zai_coding_plan_is_zai_api_key() {
    assert_eq!(key_env_for("zai-coding-plan"), Some("ZAI_API_KEY"));
}

#[test]
fn entry_requires_key_zai_coding_plan_is_true() {
    assert!(entry_requires_key("zai-coding-plan"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package zoid --lib tests::key_env_for_zai_coding_plan_is_zai_api_key -- --exact`

Expected: FAIL with `assertion failed: (left == right)` (left: `Some("OLLAMA_API_KEY")`, right: `Some("ZAI_API_KEY")`).

- [ ] **Step 3: Write minimal implementation**

**1. `key_env_for` (line 864-872):**

Add the `"zai"` arm to the match on `entry(id).map(|e| e.family)`:

```rust
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("opencode-go") => Some("OPENCODE_GO_API_KEY"),
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        Some("zai") => Some("ZAI_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}
```

**2. `select_provider` (line 913-921):**

Add the `"zai"` arm to the `match family` block (after the `"opencode-go"` arm at line 921, before the `"anthropic"` arm at line 923):

```rust
        "zai" => match key_for("ZAI_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::zai::ZaiProvider::new(k).with_base_url(base_url),
                ),
                "zai",
                true,
            ),
            None => (default_provider(), "zai", false),
        },
```

**3. `provider_for_id` (line 1017-1019):**

Add the `"zai"` arm to the `match family` block (after the `"opencode-go"` arm at line 1020, before the `"anthropic"` arm at line 1021):

```rust
        "zai" => key_for("ZAI_API_KEY").map(|k| {
            Arc::new(zoid_provider::zai::ZaiProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
```

**4. `key_status` (line 3140-3144):**

Add the `ZAI_API_KEY` row to the array:

```rust
    let key_status = [
        ("OLLAMA_API_KEY", status("OLLAMA_API_KEY")),
        ("ANTHROPIC_API_KEY", status("ANTHROPIC_API_KEY")),
        ("OPENCODE_GO_API_KEY", status("OPENCODE_GO_API_KEY")),
        ("ZAI_API_KEY", status("ZAI_API_KEY")),
    ];
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package zoid --lib tests::key_env_for_zai_coding_plan_is_zai_api_key -- --exact`

Expected: PASS. Also run `cargo test --package zoid --lib tests::entry_requires_key_zai_coding_plan_is_true -- --exact` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(main): wire zai-coding-plan provider

Add ZAI_API_KEY secret env var, select_provider + provider_for_id arms
for the 'zai' family (builds ZaiProvider), and ZAI_API_KEY row to the
config UI key_status array."
```

---

### Task 6: Run full test suite + smoke test

**Files:**
- None (verification only).

**Interfaces:**
- Consumes: all previous tasks.
- Produces: confidence that the implementation is complete and correct.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --workspace`

Expected: all tests pass. Specifically:
- `zoid-provider::openai_compat::tests::default_path_prefix_is_v1` → PASS
- `zoid-provider::openai_compat::tests::with_path_prefix_overrides_default` → PASS
- `zoid-provider::openai_compat::tests::default_path_prefix_emits_v1_chat_completions` → PASS
- `zoid-provider::zai::tests::new_uses_default_base_url` → PASS
- `zoid-provider::zai::tests::with_base_url_overrides_and_trims_trailing_slash` → PASS
- `zoid-provider::zai::tests::zai_list_models_hits_models_without_v1_prefix` → PASS
- `zoid-provider::zai::tests::zai_stream_hits_chat_completions_without_v1_prefix` → PASS
- `zoid-model::tests::zai_coding_plan_registry_entry_exists_and_is_selectable` → PASS
- `zoid-model::tests::selectable_has_five_providers` → PASS
- `zoid-model::thinking_tests::glm_5_2_has_thinking_with_effort` → PASS
- `zoid-model::thinking_tests::glm_5_2_capabilities_locked` → PASS
- `zoid-model::opencode_go_tests::opencode_go_model_caps_match_reconciled_table` → PASS
- `zoid::tests::key_env_for_zai_coding_plan_is_zai_api_key` → PASS
- `zoid::tests::entry_requires_key_zai_coding_plan_is_true` → PASS

Run: `cargo fmt --check`

Expected: no output (all code is properly formatted).

Run: `cargo clippy -- -D warnings`

Expected: no warnings (all code passes clippy lint checks).

- [ ] **Step 2: Smoke test with a real ZAI API key (optional)**

If you have a `ZAI_API_KEY` env var set:

```bash
cargo run --package zoid
```

In the TUI, open the config screen (usually `Ctrl+,` or similar), navigate to the provider picker, and verify `zai · coding plan` appears in the list. Select it, enter your API key (or ensure `ZAI_API_KEY` is set in your env), and verify `glm-5.2` is the default model. Send a test message to confirm streaming works.

**Note on integration testing:** This smoke test is manual. The plan does not include an automated integration test that exercises the full flow (main.rs → ZaiProvider → OpenAICompatProvider → HTTP request). Future work should add such a test to catch integration issues automatically.

- [ ] **Step 3: Final commit (if any smoke-test fixes were needed)**

```bash
git add -A
git commit -m "chore: final smoke-test fixes"
```

(Only if Step 2 revealed issues; otherwise skip.)
