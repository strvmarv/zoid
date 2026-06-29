# P1a.1 — Ollama Cloud provider (GLM via OpenAI-compatible API) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add an `OllamaProvider` so zoid can stream from **Ollama Cloud** (model `glm-5.2:cloud`) via the OpenAI-compatible Chat Completions API, selectable alongside the existing Anthropic provider.

**Architecture:** A small extension of the P1a streaming `Provider` seam. New self-contained `ollama` module mirrors the `anthropic` module: a pure OpenAI-shaped request-body builder, a pure OpenAI SSE chunk parser, and an `OllamaProvider` (reqwest + `eventsource-stream`). Provider/model selection moves to the crate root and becomes env-driven across both providers.

**Tech Stack:** Rust 2021 · `reqwest` (rustls) + `eventsource-stream` + `futures-util` (already deps) · `serde_json` · `async-trait` · `tokio`. No new dependencies.

**Builds on (merged P1a, `main` @ 4e11ea6):**
- `zoid_provider` seam: `MsgRole::{User,Assistant}`, `Message{role,content}`, `Usage{input_tokens,output_tokens}`, `ProviderEvent::{TextDelta(String),Usage(Usage),Done,Error(String)}`, `CompletionRequest{model,system,messages,max_tokens}`, `trait Provider: Send+Sync { async fn stream(&self,&CompletionRequest,mpsc::Sender<ProviderEvent>)->Result<()> }`, `FakeProvider`.
- `zoid_provider::anthropic`: `AnthropicProvider`, `AnthropicProvider::new(String)`, `request_body`, `parse_event`, `DEFAULT_MODEL = "claude-sonnet-4-6"`, and `default_provider() -> Arc<dyn Provider>` (selects Anthropic on `ANTHROPIC_API_KEY` else `FakeProvider`). **`default_provider` is relocated to the crate root in Task 3.**
- `zoid` bin `main.rs`: imports `zoid_provider::anthropic::{default_provider, DEFAULT_MODEL}`; `let model = $ZOID_MODEL else DEFAULT_MODEL`. **Rewired in Task 3.**

## Global Constraints

- **Edition 2021**; **no co-author / "Generated with" commit trailers**.
- **No new dependencies** — `reqwest`/`eventsource-stream`/`futures-util`/`serde_json`/`tokio`/`async-trait` are already in `zoid-provider`'s manifest. reqwest stays **rustls** (no native-tls/openssl); single-static-binary intact.
- **The provider seam stays self-contained** (no `zoid-core` dependency); the `ollama` module mirrors `anthropic` in shape.
- **Warning-free `cargo build`** and **clippy-clean**; every commit compiles and is green.
- **Determinism for tests:** the request-body builder and SSE-chunk parser are pure and unit-tested; the network path (`OllamaProvider::stream`) is **not** unit-tested (needs a key + network).
- **OpenAI request shape:** the system prompt is a **leading message** with role `"system"` (OpenAI puts system inside `messages`, unlike Anthropic's top-level `system`). Send `stream: true`; the stream terminator is the literal `data: [DONE]`. Do **not** send `stream_options` (avoid a 400 on servers that reject unknown fields; usage is not needed in P1a).
- **TDD throughout.**

---

### Task 1: Ollama request body + SSE chunk parser (pure)

The pure core of the Ollama provider: build the OpenAI-compatible request body, and parse one OpenAI SSE `data:` payload into a `ProviderEvent`. Both fully unit-testable, no network.

**Files:**
- Create: `crates/zoid-provider/src/ollama.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod ollama;`)

**Interfaces:**
- Consumes: `crate::{CompletionRequest, Message, MsgRole, ProviderEvent, Usage}`.
- Produces:
  - `pub const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2:cloud"`.
  - `pub fn request_body(req: &CompletionRequest) -> serde_json::Value`.
  - `pub fn parse_chunk(data: &str) -> Option<ProviderEvent>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-provider/src/ollama.rs`:

```rust
//! The Ollama Cloud provider via the OpenAI-compatible Chat Completions API
//! (`POST {base}/v1/chat/completions`, SSE streaming, `data: [DONE]` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.
//! Task 1: request body + chunk parser. Task 2: the provider + wiring.

use crate::{CompletionRequest, MsgRole, ProviderEvent, Usage};

/// Default model when `$ZOID_MODEL` is unset (GLM on Ollama Cloud).
pub const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2:cloud";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use serde_json::json;

    #[test]
    fn body_maps_system_to_leading_message_and_sets_stream() {
        let req = CompletionRequest {
            model: "glm-5.2:cloud".into(),
            system: Some("be terse".into()),
            messages: vec![
                Message { role: MsgRole::User, content: "hi".into() },
                Message { role: MsgRole::Assistant, content: "hello".into() },
            ],
            max_tokens: 1024,
        };
        let body = request_body(&req);
        assert_eq!(body, json!({
            "model": "glm-5.2:cloud",
            "stream": true,
            "max_tokens": 1024,
            "messages": [
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        let body = request_body(&req);
        assert_eq!(body["messages"], json!([{ "role": "user", "content": "x" }]));
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn parses_content_delta() {
        let data = r#"{"choices":[{"index":0,"delta":{"content":"Hel"}}]}"#;
        assert_eq!(parse_chunk(data), Some(ProviderEvent::TextDelta("Hel".into())));
    }

    #[test]
    fn parses_done_sentinel() {
        assert_eq!(parse_chunk("[DONE]"), Some(ProviderEvent::Done));
    }

    #[test]
    fn parses_usage_chunk() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":12}}"#;
        assert_eq!(parse_chunk(data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 7, output_tokens: 12 })));
    }

    #[test]
    fn empty_content_and_role_only_chunks_yield_none() {
        // first chunk often carries role but no content
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#), None);
        // explicit empty content
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{"content":""}}]}"#), None);
        // finish chunk with no content/usage
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#), None);
    }

    #[test]
    fn malformed_data_yields_none_not_panic() {
        assert_eq!(parse_chunk("not json"), None);
        assert_eq!(parse_chunk(""), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider body_maps_system_to_leading_message_and_sets_stream`
Expected: FAIL — `request_body`/`parse_chunk` not defined.

- [ ] **Step 3: Implement the builder and parser**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/ollama.rs`:

```rust
use serde_json::{json, Value};

/// Build the OpenAI-compatible request body. The system prompt becomes a
/// leading `{"role":"system"}` message (OpenAI puts system inside `messages`).
/// `stream_options` is intentionally omitted (some servers reject unknown
/// fields; usage is not needed in P1a).
pub fn request_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    for m in &req.messages {
        messages.push(json!({
            "role": match m.role { MsgRole::User => "user", MsgRole::Assistant => "assistant" },
            "content": m.content,
        }));
    }
    json!({
        "model": req.model,
        "stream": true,
        "max_tokens": req.max_tokens,
        "messages": messages,
    })
}

/// Parse one OpenAI-compatible SSE `data:` payload into a `ProviderEvent`.
/// The terminator is the literal `[DONE]`. Content lives at
/// `choices[0].delta.content`; a usage-bearing chunk maps to `Usage`.
/// Empty/role-only/finish chunks and malformed JSON return `None` (never panics).
pub fn parse_chunk(data: &str) -> Option<ProviderEvent> {
    let data = data.trim();
    if data == "[DONE]" {
        return Some(ProviderEvent::Done);
    }
    let v: Value = serde_json::from_str(data).ok()?;

    if let Some(text) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|t| t.as_str())
    {
        if !text.is_empty() {
            return Some(ProviderEvent::TextDelta(text.to_string()));
        }
    }

    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        let input = usage.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        let output = usage.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        return Some(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: output }));
    }

    None
}
```

Add to `crates/zoid-provider/src/lib.rs` (alongside `pub mod anthropic;`):

```rust
pub mod ollama;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS — all new ollama tests plus the existing anthropic + fake tests. `cargo build -p zoid-provider` is warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): Ollama OpenAI-compatible request body + SSE chunk parser (pure)"
```

---

### Task 2: `OllamaProvider` (reqwest + SSE)

Wire the pure halves into a streaming provider against `https://ollama.com/v1/chat/completions` with `Authorization: Bearer $OLLAMA_API_KEY`. Network path not unit-tested (covered by build + Task 1's pure tests).

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`

**Interfaces:**
- Consumes: `crate::{Provider, ProviderEvent, CompletionRequest}`, `request_body`, `parse_chunk`.
- Produces: `OllamaProvider` + `OllamaProvider::new(api_key: String) -> Self`; `impl Provider for OllamaProvider`.

- [ ] **Step 1: Implement the provider**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/ollama.rs` (merge the `crate::` names into the existing `use crate::{...}` line rather than duplicating — the file already imports `CompletionRequest, MsgRole, ProviderEvent, Usage`; add `Provider`):

```rust
use crate::Provider;
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio::sync::mpsc;

/// Streaming Ollama Cloud provider (OpenAI-compatible Chat Completions API).
pub struct OllamaProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://ollama.com".to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&request_body(req))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let _ = sink.send(ProviderEvent::Error(format!("HTTP {status}: {text}"))).await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if let Some(pe) = parse_chunk(&event.data) {
                        let is_done = matches!(pe, ProviderEvent::Done);
                        if sink.send(pe).await.is_err() {
                            break;
                        }
                        if is_done {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Verify build + tests**

Run: `cargo build -p zoid-provider`
Expected: PASS, ZERO warnings.

Run: `cargo test -p zoid-provider`
Expected: PASS — pure ollama tests + existing tests (no network test by design).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): OllamaProvider (reqwest + OpenAI-compatible SSE)"
```

---

### Task 3: Env-driven provider + model selection (crate root) and bin wiring

Relocate `default_provider()` from `anthropic.rs` to the crate root and extend it to prefer Ollama; add `default_model()` so the default model matches the selected provider; rewire the `zoid` binary to use them. Net behavior: `OLLAMA_API_KEY` → Ollama (`glm-5.2:cloud`), else `ANTHROPIC_API_KEY` → Anthropic (`claude-sonnet-4-6`), else offline `FakeProvider`; `$ZOID_MODEL` overrides the model for either.

**Files:**
- Modify: `crates/zoid-provider/src/anthropic.rs` (remove `default_provider` + now-unused imports)
- Modify: `crates/zoid-provider/src/lib.rs` (add `default_provider` + `default_model`)
- Modify: `crates/zoid/src/main.rs` (import + model selection)

**Interfaces:**
- Consumes: `anthropic::{AnthropicProvider, DEFAULT_MODEL}`, `ollama::{OllamaProvider, DEFAULT_OLLAMA_MODEL}`, `FakeProvider`, `Provider`, `ProviderEvent`.
- Produces:
  - `pub fn default_provider() -> std::sync::Arc<dyn Provider>` (crate root).
  - `pub fn default_model() -> &'static str` (crate root).

- [ ] **Step 1: Remove `default_provider` from `anthropic.rs`**

In `crates/zoid-provider/src/anthropic.rs`, DELETE the `pub fn default_provider() -> Arc<dyn Provider> { ... }` function (added in P1a Task 8). Then remove any imports that become unused as a result — specifically `use std::sync::Arc;` and the `FakeProvider` name from `use crate::{FakeProvider, Provider, ProviderEvent};` (keep `Provider` and `ProviderEvent`, which the `AnthropicProvider` impl still uses). After editing, `cargo build -p zoid-provider` must have ZERO warnings (no unused imports).

- [ ] **Step 2: Write the failing test (model selection helper)**

Add a test module at the END of `crates/zoid-provider/src/lib.rs` (after the existing `#[cfg(test)] mod tests` block — give this one a distinct name so it does not clash):

```rust
#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn default_model_constants_are_wired() {
        // The two provider defaults are distinct and non-empty; default_model()
        // returns one of them. (Env-based branch selection is exercised at
        // runtime / manual smoke — env vars are process-global and unsafe to
        // mutate in parallel tests.)
        assert_eq!(anthropic::DEFAULT_MODEL, "claude-sonnet-4-6");
        assert_eq!(ollama::DEFAULT_OLLAMA_MODEL, "glm-5.2:cloud");
        let m = default_model();
        assert!(m == anthropic::DEFAULT_MODEL || m == ollama::DEFAULT_OLLAMA_MODEL);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider default_model_constants_are_wired`
Expected: FAIL — `default_model` not defined.

- [ ] **Step 4: Add `default_provider` + `default_model` to the crate root**

In `crates/zoid-provider/src/lib.rs`, add near the top (after the existing `use` lines; `Arc` is needed):

```rust
use std::sync::Arc;
```

and add these functions at module scope (after the `FakeProvider` impl, before the `#[cfg(test)]` blocks):

```rust
/// Select the provider from the environment:
/// `OLLAMA_API_KEY` → Ollama Cloud; else `ANTHROPIC_API_KEY` → Anthropic;
/// else an offline `FakeProvider` (so the binary always runs).
pub fn default_provider() -> Arc<dyn Provider> {
    if let Ok(key) = std::env::var("OLLAMA_API_KEY") {
        if !key.is_empty() {
            return Arc::new(ollama::OllamaProvider::new(key));
        }
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Arc::new(anthropic::AnthropicProvider::new(key));
        }
    }
    Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("(no OLLAMA_API_KEY / ANTHROPIC_API_KEY — offline echo) ".into()),
        ProviderEvent::TextDelta("hello from zoid's fake provider.".into()),
        ProviderEvent::Done,
    ]))
}

/// The default model id matching the selected provider (overridden by
/// `$ZOID_MODEL` in the binary).
pub fn default_model() -> &'static str {
    if std::env::var("OLLAMA_API_KEY").map(|k| !k.is_empty()).unwrap_or(false) {
        ollama::DEFAULT_OLLAMA_MODEL
    } else {
        anthropic::DEFAULT_MODEL
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zoid-provider`
Expected: PASS — `default_model_constants_are_wired` plus all prior provider tests.

- [ ] **Step 6: Rewire the binary**

In `crates/zoid/src/main.rs`, change the provider/model imports and the model default:

Replace the import line
```rust
use zoid_provider::anthropic::{default_provider, DEFAULT_MODEL};
```
with
```rust
use zoid_provider::{default_model, default_provider};
```

and replace the model resolution
```rust
    let model = std::env::var("ZOID_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
```
with
```rust
    let model = std::env::var("ZOID_MODEL").unwrap_or_else(|_| default_model().to_string());
```

Leave the rest of `main.rs` unchanged (the `App.provider` field is still `Arc<dyn Provider>` from `default_provider()`).

- [ ] **Step 7: Verify the whole workspace**

Run: `cargo build`
Expected: PASS — all crates, `target/debug/zoid` produced, ZERO warnings.

Run: `cargo test`
Expected: PASS — full suite green (the new selection test + all P1a tests).

- [ ] **Step 8: Manual smoke (deferred — needs a real TTY + an Ollama key)**

> For the human, not the subagent (no TTY headlessly). Document as deferred.

```bash
export OLLAMA_API_KEY=...        # your ollama.com key
# ZOID_MODEL defaults to glm-5.2:cloud when OLLAMA_API_KEY is set; override if needed.
ZOID_DB=/tmp/zoid-ollama.db cargo run -p zoid
# Type a message, Enter — watch GLM stream token-by-token. Ctrl-C to quit.
rm -f /tmp/zoid-ollama.db
```

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-provider/src/anthropic.rs crates/zoid-provider/src/lib.rs crates/zoid/src/main.rs
git commit -m "feat(provider): env-driven provider+model selection (Ollama/Anthropic/Fake)"
```

---

## P1a.1 Definition of Done

- `cargo build` warning-free; `cargo test` fully green; clippy clean.
- `OllamaProvider` streams from Ollama Cloud's OpenAI-compatible endpoint (`/v1/chat/completions`, Bearer `$OLLAMA_API_KEY`, `data: [DONE]` terminator); pure request-body + chunk-parser unit-tested; network path not.
- `default_provider()` selects `OLLAMA_API_KEY` → Ollama, else `ANTHROPIC_API_KEY` → Anthropic, else offline Fake; `default_model()` mirrors (`glm-5.2:cloud` / `claude-sonnet-4-6`); `$ZOID_MODEL` overrides.
- Anthropic provider retained and still selectable; seam stays self-contained; no new deps; rustls only.

## Self-Review (against the provider direction + P1a seam)

- **Coverage:** Ollama OpenAI-compatible body (system→leading message, stream:true, no stream_options) ✓ T1; SSE chunk parse incl `[DONE]`, content delta, usage, empty/role-only/finish/malformed→None ✓ T1; `OllamaProvider` reqwest+SSE ✓ T2; env selection + model default + bin wiring ✓ T3.
- **Type consistency:** `request_body`/`parse_chunk` signatures match the anthropic-module shape; `OllamaProvider::new(String)` mirrors `AnthropicProvider::new`; `default_provider() -> Arc<dyn Provider>` and `default_model() -> &'static str` used identically in lib.rs and main.rs; `DEFAULT_OLLAMA_MODEL = "glm-5.2:cloud"`.
- **Hygiene:** Task 3 explicitly removes `default_provider` + now-unused `Arc`/`FakeProvider` imports from `anthropic.rs` to keep the build warning-free (the recurring P1a lesson).
- **Tool-calling note (P1b):** because GLM runs via the OpenAI-compatible API, P1b's tool-calling must use OpenAI `tools` + `tool_calls` (assistant message `tool_calls`, `role:"tool"` results) — not Anthropic `tool_use`/`tool_result` blocks. The provider seam will need a tool-call `ProviderEvent` variant and `CompletionRequest.tools`; both additive.
- **Placeholder scan:** every step has complete code + exact commands; the network path and interactive smoke are flagged as the only non-unit-tested pieces.
