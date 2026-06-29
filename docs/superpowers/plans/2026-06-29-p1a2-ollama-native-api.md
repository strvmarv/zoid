# P1a.2 — Ollama native `/api/chat` (NDJSON) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fix `OllamaProvider` to use Ollama Cloud's **native `/api/chat`** endpoint with **NDJSON** streaming — the only path that authenticates with the user's key (the OpenAI-compatible `/v1/chat/completions` returns `Unauthorized`, verified by live test).

**Why:** P1a.1 built the Ollama provider against `/v1/chat/completions` + SSE (`data: [DONE]`). A live test proved Ollama Cloud rejects this key on `/v1` but accepts the **native** `POST https://ollama.com/api/chat` (Bearer auth), which streams **newline-delimited JSON** objects (`{"message":{"content":"…","thinking":"…"},"done":false}` … final line `"done":true`), not SSE. This plan replaces the `/v1`+SSE path with native NDJSON.

**Architecture:** Contained to `crates/zoid-provider/src/ollama.rs`. The pure request-body builder switches to the native shape; the SSE `parse_chunk` is replaced by a native NDJSON `parse_line`; `OllamaProvider::stream` POSTs to `/api/chat` and splits the byte stream into lines (UTF-8-safe newline split on raw bytes) instead of using `eventsource-stream`. The `Provider` seam, `default_provider`/`default_model` selection, and the binary are unchanged. `eventsource-stream` stays a dependency (still used by `anthropic.rs`).

**Tech Stack:** Rust 2021 · `reqwest` (rustls) `bytes_stream()` + `futures-util::StreamExt` · `serde_json`. No new/removed dependencies.

**Builds on (merged P1a.1, `main` @ f478e16):**
- `zoid_provider` seam: `CompletionRequest{model,system,messages,max_tokens}`, `Message{role:MsgRole,content}`, `MsgRole::{User,Assistant}`, `ProviderEvent::{TextDelta(String),Usage(Usage),Done,Error(String)}`, `Provider` trait.
- `crates/zoid-provider/src/ollama.rs` currently has: `DEFAULT_OLLAMA_MODEL = "glm-5.2:cloud"`, `request_body` (OpenAI shape), `parse_chunk` (SSE/OpenAI), `OllamaProvider` (`/v1/chat/completions` + `eventsource-stream`), and a `#[cfg(test)] mod tests`. **This file is reworked.**
- `default_provider()`/`default_model()` in `lib.rs` already route `OLLAMA_API_KEY` → `OllamaProvider` — unchanged by this plan.

## Global Constraints

- Edition 2021; **no co-author / "Generated with" trailers**.
- **No dependency changes.** `reqwest` stays rustls. `eventsource-stream` REMAINS in the manifest (used by `anthropic.rs`) — only `ollama.rs` stops importing it.
- The `ollama` module stays self-contained (no `zoid-core` dep).
- **Warning-free `cargo build`**; clippy-clean; every commit compiles and is green.
- The native request body sends `{"model","messages","stream":true}`; system prompt is a **leading `{"role":"system"}` message**; do NOT send OpenAI-only fields (`max_tokens`, `stream_options`).
- NDJSON line parsing must be **UTF-8-safe across chunk boundaries**: split the raw byte buffer on `b'\n'` (newline never occurs inside a multibyte UTF-8 sequence) and only then lossy-decode each complete line.
- `message.thinking` (GLM reasoning) is **ignored** in P1a.2 (stream only `message.content`); surfacing reasoning is a later phase.
- The network path (`OllamaProvider::stream`) is **not** unit-tested; the pure `request_body`/`parse_line` are.
- TDD throughout.

---

### Task 1: Native request body + NDJSON line parser (pure)

Replace the OpenAI-shaped `request_body` and the SSE `parse_chunk` with the native equivalents, fully unit-tested.

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`

**Interfaces:**
- Consumes: `crate::{CompletionRequest, MsgRole, ProviderEvent}` (Usage no longer needed by the parser — see note).
- Produces:
  - `DEFAULT_OLLAMA_MODEL` unchanged (`"glm-5.2:cloud"`).
  - `pub fn request_body(req: &CompletionRequest) -> serde_json::Value` — native shape.
  - `pub fn parse_line(line: &str) -> Option<ProviderEvent>` — replaces `parse_chunk`.

- [ ] **Step 1: Replace the test module**

In `crates/zoid-provider/src/ollama.rs`, replace the ENTIRE existing `#[cfg(test)] mod tests { ... }` block with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use serde_json::json;

    #[test]
    fn native_body_has_stream_and_system_leading_message_no_openai_fields() {
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
            "messages": [
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
        // native body must NOT carry OpenAI-only fields
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(), system: None,
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        assert_eq!(request_body(&req)["messages"], json!([{ "role": "user", "content": "x" }]));
    }

    #[test]
    fn parses_content_delta_line() {
        let line = r#"{"model":"glm-5.2:cloud","message":{"role":"assistant","content":"Hel"},"done":false}"#;
        assert_eq!(parse_line(line), Some(ProviderEvent::TextDelta("Hel".into())));
    }

    #[test]
    fn thinking_only_line_yields_none() {
        let line = r#"{"message":{"role":"assistant","content":"","thinking":"reasoning"},"done":false}"#;
        assert_eq!(parse_line(line), None);
    }

    #[test]
    fn done_line_yields_done() {
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":58}"#;
        assert_eq!(parse_line(line), Some(ProviderEvent::Done));
    }

    #[test]
    fn error_line_yields_error() {
        assert_eq!(parse_line(r#"{"error":"Unauthorized"}"#),
            Some(ProviderEvent::Error("Unauthorized".into())));
    }

    #[test]
    fn empty_and_malformed_lines_yield_none() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("not json"), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider native_body_has_stream_and_system_leading_message_no_openai_fields`
Expected: FAIL — old `request_body` includes `max_tokens`/differs; `parse_line` not defined.

- [ ] **Step 3: Replace `request_body` and swap `parse_chunk` → `parse_line`**

In `crates/zoid-provider/src/ollama.rs`, REPLACE the existing `request_body` function and the existing `parse_chunk` function (the two pure functions above the provider impl) with:

```rust
/// Build the native Ollama `/api/chat` request body. System prompt is a leading
/// `{"role":"system"}` message. Only `model`/`messages`/`stream` are sent — the
/// native API does not take OpenAI's `max_tokens`/`stream_options`.
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
        "messages": messages,
    })
}

/// Parse one native NDJSON line into a `ProviderEvent`.
/// `{"done":true,...}` → `Done`; `{"error":"..."}` → `Error`;
/// `message.content` non-empty → `TextDelta`; everything else
/// (thinking-only/empty/blank/malformed) → `None`. Never panics.
pub fn parse_line(line: &str) -> Option<ProviderEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Some(ProviderEvent::Error(err.to_string()));
    }
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        return Some(ProviderEvent::Done);
    }
    if let Some(text) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
        if !text.is_empty() {
            return Some(ProviderEvent::TextDelta(text.to_string()));
        }
    }
    None
}
```

If the top-of-file `use crate::{...}` now imports `Usage` but it is no longer referenced anywhere in the module after Task 2, remove `Usage` from the import to keep the build warning-free. (If still referenced, leave it. Verify at the end of Task 2.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS — the seven ollama tests + existing anthropic/fake/selection tests. (The provider impl still references the old `parse_chunk` name until Task 2 — if the crate fails to COMPILE here because `OllamaProvider::stream` calls `parse_chunk`, that is expected; proceed to Task 2, which updates the impl. To keep this commit compiling, you MAY in this task also rename the call site in `stream` from `parse_chunk` to `parse_line` as a minimal touch — but the full stream rework is Task 2. Easiest: do Step 3 + the Task 2 stream rework together before running the full suite; commit Task 1 only after `cargo test -p zoid-provider` is green.)

> Implementer note: Task 1 and Task 2 both edit `ollama.rs` and are coupled (renaming `parse_chunk` breaks the `stream` call site). Implement Task 1's pure functions + tests, then immediately Task 2's stream rework, ensuring the crate compiles and `cargo test -p zoid-provider` is green before committing Task 1. Commit the pure-function changes as Task 1, then the stream changes as Task 2 — OR, if cleaner, the controller may merge these into one commit. Follow the per-task commit messages below; if merged, use Task 2's message.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): native Ollama /api/chat request body + NDJSON line parser (pure)"
```

---

### Task 2: `OllamaProvider::stream` over native `/api/chat` (NDJSON)

Rework the provider to POST the native endpoint and parse the NDJSON byte stream line-by-line, dropping `eventsource-stream`.

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`

**Interfaces:**
- Consumes: `crate::{Provider, ProviderEvent, CompletionRequest}`, `request_body`, `parse_line`.
- Produces: updated `impl Provider for OllamaProvider` (same `OllamaProvider`/`new` API).

- [ ] **Step 1: Replace the provider impl + imports**

In `crates/zoid-provider/src/ollama.rs`:
- Remove `use eventsource_stream::Eventsource;` from this module (keep `use futures_util::StreamExt;`).
- Replace the `#[async_trait] impl Provider for OllamaProvider { ... }` block with:

```rust
#[async_trait]
impl Provider for OllamaProvider {
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
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

        // Native /api/chat streams newline-delimited JSON. Buffer raw bytes and
        // split on b'\n' (safe: newline never appears inside a multibyte UTF-8
        // sequence), decoding only complete lines.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        if let Some(pe) = parse_line(&line) {
                            let is_done = matches!(pe, ProviderEvent::Done);
                            if sink.send(pe).await.is_err() {
                                return Ok(());
                            }
                            if is_done {
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    return Ok(());
                }
            }
        }

        // Flush any trailing line without a final newline.
        if !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf);
            if let Some(pe) = parse_line(&line) {
                let _ = sink.send(pe).await;
            }
        }
        Ok(())
    }
}
```

The `OllamaProvider` struct and `new` (base_url `https://ollama.com`) are unchanged.

- [ ] **Step 2: Verify build + tests + clippy**

Run: `cargo build -p zoid-provider`
Expected: PASS, ZERO warnings (no leftover `eventsource_stream`/`Usage` unused import in this module).

Run: `cargo test -p zoid-provider`
Expected: PASS — all pure tests (no network test by design).

Run: `cargo clippy -p zoid-provider --all-targets`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): OllamaProvider streams native /api/chat NDJSON"
```

---

## P1a.2 Definition of Done

- `cargo build` warning-free; `cargo test` green; clippy clean.
- `OllamaProvider` POSTs `https://ollama.com/api/chat` (native, Bearer `$OLLAMA_API_KEY`), streams NDJSON; `message.content` deltas become `TextDelta`, `"done":true` → `Done`, `{"error":..}` → `Error`; `message.thinking` ignored.
- Native request body (`model`/`messages`/`stream` only; system as leading message); no OpenAI-only fields.
- `eventsource-stream` no longer imported by `ollama.rs` (still a crate dep for `anthropic.rs`); no dependency manifest change; reqwest stays rustls; module self-contained.
- Pure `request_body`/`parse_line` unit-tested (incl. content/thinking/done/error/empty/malformed); network path validated manually against Ollama Cloud (live test already confirmed the wire format and that `glm-5.2:cloud` responds).

## Self-Review

- **Coverage:** native body (T1), NDJSON parse incl thinking/done/error/empty/malformed (T1), native stream loop with UTF-8-safe line splitting + trailing-line flush (T2). The `/v1`+SSE path and `parse_chunk` are fully removed.
- **Type consistency:** `request_body(&CompletionRequest)->Value` and `parse_line(&str)->Option<ProviderEvent>` mirror the prior names' shapes; `OllamaProvider`/`new`/`impl Provider` signatures unchanged; selection in `lib.rs` and the bin need no edits.
- **Hygiene:** drop `eventsource_stream::Eventsource` (and `Usage` if now unused) from `ollama.rs` to keep the build warning-free — the recurring lesson.
- **Tool-calling note (P1b):** native `/api/chat` supports `tools` in the body and returns `message.tool_calls` in NDJSON lines; `parse_line` must be extended for `message.tool_calls` then (additive). The reasoning `thinking` field is also available to surface later.
- **Placeholder scan:** complete code + exact commands throughout; network path flagged as the only non-unit-tested piece (already live-verified).
