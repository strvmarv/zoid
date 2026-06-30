# P1b — Tools & Tool-Calling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's single-shot streaming chat into a real **agentic loop** — the model can call tools (file read/write/edit, shell, code search) that run in the working directory, see their results, and continue — with tool calls and results recorded as events and rendered inline.

**Architecture:** Extend the streaming `Provider` seam with OpenAI/Ollama-style `tools`/`tool_calls`. Add a `zoid-tools` crate (a `Tool` trait + a curated, cwd-scoped tool set). Add `ToolCall`/`ToolResult` to the event log and a tool-aware `conversation()` projection that both the renderer and the request-builder fold from. Lift the agent loop out of the `tokio::select!` UI loop into a terminal-free, fake-provider-testable function in a new `zoid` **lib** target. The binary's submit handler dispatches a turn; the UI redraws from appended-event notifications.

**Tech Stack:** Rust 2021, tokio, reqwest (rustls), serde_json, rusqlite, ratatui + tui-textarea, insta + ratatui `TestBackend`, proptest, tempfile.

## Global Constraints

- **Tool-calling wire format is OpenAI/Ollama `tools` + `message.tool_calls`** — NOT Anthropic `tool_use`. GLM (`glm-5.2:cloud`) supports this via the native `/api/chat` API. Tool result messages use Ollama's `{"role":"tool","content":…,"tool_name":…}` shape.
- **Provider seam (`zoid-provider`) stays free of any `zoid-core` dependency.** `zoid-tools` may depend on `zoid-provider` (for `ToolSpec`/`ToolCall`); it must NOT depend on `zoid-core`.
- **`serde_json::Value` does not implement `Eq`.** Provider wire types that carry a `Value` (`ToolSpec`, `ToolCall`, `CompletionRequest`, `Message`, `ProviderEvent`) derive `PartialEq` only — drop `Eq`. `EventKind` keeps `Eq` by storing tool args/results as JSON **strings**, not `Value`.
- **Tools in Chat run in the process working directory (cwd), human-in-the-loop, with NO sandbox/path-jail** (spec §9: Chat is safe by human presence; the human sees and drives every turn). Resolve paths relative to cwd; do not add jailing.
- **Tool-iteration cap:** a single user message drives at most `MAX_TOOL_ITERATIONS = 25` tool rounds; exceeding it appends a warning assistant message and ends the turn.
- **Warning-free + clippy-clean:** `cargo build` and `cargo clippy --all-targets` must emit **0 warnings**. When a task removes the last use of an import, remove the import; when a `match` over `EventKind`/`ProviderEvent`/`ChatMsg` gains a variant, handle it explicitly (no blanket `_` arms that would hide future variants — except where this plan says otherwise).
- **TDD:** every task is red → green → commit; steps are 2–5 minutes. Frequent commits.
- **Design tokens + snapshots:** any render change reads glyphs/colors from `zoid_tui::tokens` and ships/updates a `TestBackend` + `insta` snapshot (spec §16).
- **Commits:** Conventional Commits style; **never** add a `Co-Authored-By` or any co-author trailer (user's `~/CLAUDE.md`).

**Non-goals for P1b (do NOT build):**
- **Anthropic tool-calling.** `AnthropicProvider` stays text-only this phase (it ignores `req.tools` and emits no `ToolCall`). Adding Anthropic `tool_use` is a small follow-up (P1b.1) that mirrors this structure. Text streaming via Anthropic must keep working.
- **Interactive peek/zoom expansion** of tool cards (belongs to P2's modal shell + key routing). P1b renders tool interactions inline, statically, with a `→ peek` *hint* only.
- **Parallel/concurrent tool execution or subagent fan-out**, Build mode, the token ledger (P3), worktree sandboxing for tools (Chat = cwd).

---

## File Structure

- **`crates/zoid-provider/src/lib.rs`** — add `ToolSpec`, `ToolCall`; add `tools` to `CompletionRequest`; extend `Message` (tool_calls, tool_name, `MsgRole::Tool`) with constructors; add `ProviderEvent::ToolCall`; drop `Eq` from `Value`-carrying types.
- **`crates/zoid-provider/src/ollama.rs`** — serialize `tools`/assistant `tool_calls`/`tool` messages into the native body; `parse_line` returns `Vec<ProviderEvent>` and emits `ToolCall`.
- **`crates/zoid-tools/`** *(new crate)* — `lib.rs` (`Tool` trait, `ToolOutput`, `registry()`, `run_tool()`), `read.rs`, `write.rs`, `edit.rs`, `search.rs`, `shell.rs`.
- **`crates/zoid-core/src/event.rs`** — add `EventKind::ToolCall`/`ToolResult`.
- **`crates/zoid-core/src/projection.rs`** — add `ChatMsg`, `ToolCallRef`, `conversation()`; later retire `transcript()`/`Turn`.
- **`crates/zoid/src/lib.rs`** *(new)* + **`crates/zoid/src/agent.rs`** *(new)* — terminal-free agent loop (`run_agent_turn`, `AgentUpdate`, request building, `ChatMsg`→`Message` mapping, `SYSTEM_PROMPT`, `MAX_TOOL_ITERATIONS`).
- **`crates/zoid/src/main.rs`** — use `zoid::agent`; dispatch a turn on submit; redraw from `AgentUpdate`.
- **`crates/zoid-tui/src/chat.rs`** + **`crates/zoid-tui/tests/chat_snapshot.rs`** — render `ChatMsg` incl. tool cards; new/updated snapshots.
- **Manifests:** `Cargo.toml` (add `crates/zoid-tools` member), `crates/zoid-tools/Cargo.toml`, `crates/zoid/Cargo.toml` (`[lib]` + `zoid-tools` dep).

---

## Task 1: Provider seam — tool types & message extension

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs`
- Test: same file (`#[cfg(test)] mod tests` / new `mod tool_types_tests`)

**Interfaces:**
- Produces: `ToolSpec { name: String, description: String, parameters: serde_json::Value }`; `ToolCall { id: String, name: String, args: serde_json::Value }`; `MsgRole::Tool`; `Message { role, content, tool_calls: Vec<ToolCall>, tool_name: Option<String> }` with `Message::user/assistant/tool` constructors; `CompletionRequest { …, tools: Vec<ToolSpec> }`; `ProviderEvent::ToolCall(ToolCall)`.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-provider/src/lib.rs` inside a new test module:

```rust
#[cfg(test)]
mod tool_types_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_constructors_set_role_and_fields() {
        let u = Message::user("hi");
        assert_eq!(u.role, MsgRole::User);
        assert_eq!(u.content, "hi");
        assert!(u.tool_calls.is_empty());
        assert_eq!(u.tool_name, None);

        let t = Message::tool("read_file", "file contents");
        assert_eq!(t.role, MsgRole::Tool);
        assert_eq!(t.content, "file contents");
        assert_eq!(t.tool_name.as_deref(), Some("read_file"));
    }

    #[test]
    fn request_carries_tools_and_event_carries_tool_call() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![spec.clone()],
        };
        assert_eq!(req.tools, vec![spec]);

        let ev = ProviderEvent::ToolCall(ToolCall {
            id: "".into(),
            name: "read_file".into(),
            args: json!({"path": "a.txt"}),
        });
        assert_eq!(
            ev,
            ProviderEvent::ToolCall(ToolCall { id: "".into(), name: "read_file".into(), args: json!({"path": "a.txt"}) })
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider tool_types_tests`
Expected: FAIL to **compile** (`ToolSpec`/`ToolCall` undefined, `Message::user` missing, `tools` field missing, `ProviderEvent::ToolCall` missing).

- [ ] **Step 3: Write minimal implementation**

In `crates/zoid-provider/src/lib.rs`, add `use serde_json::Value;` near the top imports. Then:

Change `MsgRole` to add `Tool`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRole {
    User,
    Assistant,
    Tool,
}
```

Replace the `Message` struct (drop `Eq`, add fields + constructors):
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: MsgRole,
    pub content: String,
    /// Populated only on assistant messages that requested tools.
    pub tool_calls: Vec<ToolCall>,
    /// Populated only on `MsgRole::Tool` messages: the tool whose result this is.
    pub tool_name: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: MsgRole::User, content: content.into(), tool_calls: Vec::new(), tool_name: None }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: MsgRole::Assistant, content: content.into(), tool_calls: Vec::new(), tool_name: None }
    }
    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: MsgRole::Tool, content: content.into(), tool_calls: Vec::new(), tool_name: Some(name.into()) }
    }
}
```

Add the tool wire types (after `Usage`):
```rust
/// A tool the model may call (OpenAI/Ollama function shape). `parameters` is a
/// JSON Schema object describing the tool's arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A tool invocation requested by the model. `id` is empty for providers (Ollama
/// native) that don't issue call ids; `args` is the parsed arguments object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}
```

Change `ProviderEvent` (drop `Eq`, add variant):
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCall(ToolCall),
    Usage(Usage),
    Done,
    Error(String),
}
```

Change `CompletionRequest` (drop `Eq`, add `tools`):
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
}
```

Fix the two existing call sites in this file's tests:
- In `selection_tests` — unaffected.
- In `mod tests::fake_streams_scripted_events_in_order`: change the `CompletionRequest { … }` literal to include `tools: vec![]`, and change `messages: vec![Message { role: MsgRole::User, content: "hi".into() }]` to `messages: vec![Message::user("hi")]`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (note: `ollama.rs` tests will now FAIL to compile — that's Task 2; if you must, run `cargo test -p zoid-provider --lib tool_types_tests` to confirm just this module first, but the crate won't build until Task 2). To keep this task green in isolation, also apply the **minimal** `ollama.rs` compile-fixes inline here is NOT required — instead, temporarily ensure the crate compiles by updating the `ollama.rs` `request_body`/`parse_line` call sites is Task 2's job. **Therefore: for Task 1, also do Step 5 below to keep the crate compiling.**

- [ ] **Step 5: Keep the crate compiling — patch `ollama.rs` literals minimally, then commit**

`ollama.rs` constructs `CompletionRequest` literals in its tests and matches `ProviderEvent`. To keep `zoid-provider` building after Task 1 (so the suite is green), make these **mechanical** edits in `crates/zoid-provider/src/ollama.rs`:
- In `request_body`, the `Message` field reads (`m.role`, `m.content`) are unchanged and still compile.
- In its test module, every `CompletionRequest { … }` literal: add `tools: vec![]`.
- Every `Message { role: …, content: … }` literal: replace with the matching `Message::user(...)` / `Message::assistant(...)` constructor (e.g. `Message { role: MsgRole::User, content: "hi".into() }` → `Message::user("hi")`).
- Leave `parse_line` returning `Option<ProviderEvent>` for now (Task 2 changes it).

Run: `cargo test -p zoid-provider`
Expected: PASS.

```bash
git add crates/zoid-provider/src/lib.rs crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): tool types (ToolSpec/ToolCall), tools in request, Tool role"
```

---

## Task 2: Ollama native tool-calling — request body + `tool_calls` parsing

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs`
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ToolSpec`, `ToolCall`, `Message` (with `tool_calls`/`tool_name`), `MsgRole::Tool`, `ProviderEvent::ToolCall` from Task 1.
- Produces: `pub fn parse_line(line: &str) -> Vec<ProviderEvent>` (signature change); `request_body` now serializes `tools`, assistant `tool_calls`, and `tool` messages.

- [ ] **Step 1: Write the failing tests**

Replace/extend the test module in `crates/zoid-provider/src/ollama.rs`. Update existing tests to the new `Vec` return and add tool tests:

```rust
#[test]
fn body_includes_tools_and_tool_messages() {
    use crate::ToolSpec;
    let req = CompletionRequest {
        model: "glm-5.2:cloud".into(),
        system: None,
        messages: vec![
            Message::user("read foo"),
            Message {
                role: MsgRole::Assistant,
                content: "".into(),
                tool_calls: vec![ToolCall { id: "".into(), name: "read_file".into(), args: json!({"path": "foo"}) }],
                tool_name: None,
            },
            Message::tool("read_file", "bar"),
        ],
        max_tokens: 8,
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: json!({"type": "object"}),
        }],
    };
    let body = request_body(&req);
    assert_eq!(body["tools"], json!([{
        "type": "function",
        "function": { "name": "read_file", "description": "read a file", "parameters": {"type": "object"} }
    }]));
    assert_eq!(body["messages"], json!([
        { "role": "user", "content": "read foo" },
        { "role": "assistant", "content": "", "tool_calls": [ { "function": { "name": "read_file", "arguments": {"path": "foo"} } } ] },
        { "role": "tool", "content": "bar", "tool_name": "read_file" },
    ]));
}

#[test]
fn body_without_tools_omits_tools_key() {
    let req = CompletionRequest {
        model: "m".into(), system: None,
        messages: vec![Message::user("x")],
        max_tokens: 8, tools: vec![],
    };
    assert!(request_body(&req).get("tools").is_none());
}

#[test]
fn parses_tool_call_line() {
    let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"a.txt"}}}]},"done":false}"#;
    assert_eq!(
        parse_line(line),
        vec![ProviderEvent::ToolCall(ToolCall { id: "".into(), name: "read_file".into(), args: json!({"path": "a.txt"}) })]
    );
}

#[test]
fn parses_text_then_done_as_two_events() {
    let line = r#"{"message":{"role":"assistant","content":"hi"},"done":true}"#;
    assert_eq!(parse_line(line), vec![ProviderEvent::TextDelta("hi".into()), ProviderEvent::Done]);
}
```

Then update the **existing** parser tests to the `Vec` shape:
- `parses_content_delta_line`: expect `vec![ProviderEvent::TextDelta("Hel".into())]`.
- `thinking_only_line_yields_none`: rename intent — expect `Vec::<ProviderEvent>::new()` (i.e. `assert!(parse_line(line).is_empty())`).
- `done_line_yields_done`: expect `vec![ProviderEvent::Done]`.
- `error_line_yields_error`: expect `vec![ProviderEvent::Error("Unauthorized".into())]`.
- `empty_and_malformed_lines_yield_none`: assert each is `.is_empty()`.
- Keep `native_body_has_stream_and_system_leading_message_no_openai_fields` and `body_without_system_has_no_system_message` (they already pass; ensure the `assert_eq!(body, json!({…}))` in the first still holds — with no tools, no `tools` key is added, so it stays equal).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider -- ollama`
Expected: FAIL to compile (`parse_line` returns `Option`, not `Vec`; `request_body` doesn't emit `tools`/`tool_calls`).

- [ ] **Step 3: Write the implementation**

In `crates/zoid-provider/src/ollama.rs`, replace `request_body` and `parse_line`. First widen the import: `use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall};`.

```rust
pub fn request_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    for m in &req.messages {
        match m.role {
            MsgRole::User => messages.push(json!({ "role": "user", "content": m.content })),
            MsgRole::Assistant => {
                let mut obj = json!({ "role": "assistant", "content": m.content });
                if !m.tool_calls.is_empty() {
                    obj["tool_calls"] = Value::Array(
                        m.tool_calls.iter()
                            .map(|tc| json!({ "function": { "name": tc.name, "arguments": tc.args } }))
                            .collect(),
                    );
                }
                messages.push(obj);
            }
            MsgRole::Tool => messages.push(json!({
                "role": "tool",
                "content": m.content,
                "tool_name": m.tool_name.clone().unwrap_or_default(),
            })),
        }
    }
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools.iter()
                .map(|t| json!({
                    "type": "function",
                    "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
                }))
                .collect(),
        );
    }
    body
}

/// Parse one native NDJSON line into zero or more `ProviderEvent`s, in order:
/// `error` short-circuits to `[Error]`; otherwise non-empty `message.content`
/// → `TextDelta`, each `message.tool_calls[]` → `ToolCall`, then `done:true`
/// → `Done`. Empty/thinking-only/blank/malformed lines → `[]`. Never panics.
pub fn parse_line(line: &str) -> Vec<ProviderEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return vec![ProviderEvent::Error(err.to_string())];
    }

    let mut out = Vec::new();
    if let Some(text) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
        if !text.is_empty() {
            out.push(ProviderEvent::TextDelta(text.to_string()));
        }
    }
    if let Some(calls) = v.get("message").and_then(|m| m.get("tool_calls")).and_then(|c| c.as_array()) {
        for call in calls {
            if let Some(func) = call.get("function") {
                let name = func.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
                if name.is_empty() {
                    continue;
                }
                let args = func.get("arguments").cloned().unwrap_or(Value::Null);
                let id = call.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
                out.push(ProviderEvent::ToolCall(ToolCall { id, name, args }));
            }
        }
    }
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        out.push(ProviderEvent::Done);
    }
    out
}
```

Update the `stream` loop to consume the `Vec`. Replace the two `if let Some(pe) = parse_line(&line)` blocks:

In the main chunk loop:
```rust
while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
    let line: Vec<u8> = buf.drain(..=pos).collect();
    let line = String::from_utf8_lossy(&line);
    for pe in parse_line(&line) {
        let is_done = matches!(pe, ProviderEvent::Done);
        if sink.send(pe).await.is_err() {
            return Ok(());
        }
        if is_done {
            return Ok(());
        }
    }
}
```

In the trailing-line flush:
```rust
if !buf.is_empty() {
    let line = String::from_utf8_lossy(&buf);
    for pe in parse_line(&line) {
        if sink.send(pe).await.is_err() {
            break;
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (whole provider crate).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): Ollama native tools + tool_calls (request body + NDJSON parse)"
```

---

## Task 3: `zoid-tools` crate — trait, registry, `read_file`, `write_file`

**Files:**
- Create: `crates/zoid-tools/Cargo.toml`, `crates/zoid-tools/src/lib.rs`, `crates/zoid-tools/src/read.rs`, `crates/zoid-tools/src/write.rs`
- Modify: `Cargo.toml` (workspace members)
- Test: in `lib.rs`, `read.rs`, `write.rs` test modules (tempdir-based)

**Interfaces:**
- Consumes: `zoid_provider::{ToolSpec}` and `serde_json::Value`.
- Produces: `trait Tool { fn name(&self) -> &str; fn spec(&self) -> ToolSpec; fn run(&self, args: &Value) -> ToolOutput; }`; `struct ToolOutput { pub text: String, pub is_error: bool }`; `fn registry() -> Vec<Box<dyn Tool>>`; `fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value) -> ToolOutput`; tools `ReadFile`, `WriteFile`.

- [ ] **Step 1: Add the workspace member and crate manifest**

Edit root `Cargo.toml` members list:
```toml
members = ["crates/zoid-core", "crates/zoid-provider", "crates/zoid-tui", "crates/zoid-tools", "crates/zoid"]
```

Create `crates/zoid-tools/Cargo.toml`:
```toml
[package]
name = "zoid-tools"
version = "0.0.0"
edition.workspace = true

[dependencies]
zoid-provider = { path = "../zoid-provider" }
serde_json = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tools/src/lib.rs` with the trait + registry + dispatch, and a test:

```rust
//! zoid-tools — the curated, cwd-scoped tool set the agent loop can call.
//! Tools run in the process working directory (Chat is safe by human presence,
//! spec §9); no path-jailing here.

pub mod read;
pub mod write;

use serde_json::Value;
use zoid_provider::ToolSpec;

/// The outcome of running a tool. `text` is fed back to the model as the tool
/// result; `is_error` marks failures (still returned to the model, not panicked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false }
    }
    pub fn err(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

/// A callable tool. `spec()` is sent to the provider; `run()` executes it.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn run(&self, args: &Value) -> ToolOutput;
}

/// The compiled-in tool set (spec §9: fixed curated set in v1).
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::ReadFile),
        Box::new(write::WriteFile),
    ]
}

/// Dispatch a tool call by name. Unknown tools return an error `ToolOutput`
/// (the model sees it and can recover) rather than panicking.
pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value) -> ToolOutput {
    match tools.iter().find(|t| t.name() == name) {
        Some(t) => t.run(args),
        None => ToolOutput::err(format!("unknown tool: {name}")),
    }
}

/// Helper for tools: pull a required string argument.
pub(crate) fn str_arg(args: &Value, key: &str) -> Result<String, ToolOutput> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolOutput::err(format!("missing or non-string argument: {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_has_unique_named_tools() {
        let reg = registry();
        let mut names: Vec<&str> = reg.iter().map(|t| t.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "tool names must be unique");
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
    }

    #[test]
    fn unknown_tool_is_error_not_panic() {
        let reg = registry();
        let out = run_tool(&reg, "nope", &json!({}));
        assert!(out.is_error);
        assert!(out.text.contains("unknown tool"));
    }
}
```

Create `crates/zoid-tools/src/read.rs`:
```rust
use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use zoid_provider::ToolSpec;

/// Read a UTF-8 text file relative to the working directory.
pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Read a UTF-8 text file from the working directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "File path relative to the working directory." } },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => ToolOutput::ok(contents),
            Err(e) => ToolOutput::err(format!("read_file({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn reads_existing_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "hello tools").unwrap();
        let out = ReadFile.run(&json!({ "path": f.path().to_str().unwrap() }));
        assert!(!out.is_error);
        assert_eq!(out.text, "hello tools");
    }

    #[test]
    fn missing_file_is_error() {
        let out = ReadFile.run(&json!({ "path": "/no/such/zoid/file" }));
        assert!(out.is_error);
    }

    #[test]
    fn missing_arg_is_error() {
        let out = ReadFile.run(&json!({}));
        assert!(out.is_error);
        assert!(out.text.contains("path"));
    }
}
```

Create `crates/zoid-tools/src/write.rs`:
```rust
use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use zoid_provider::ToolSpec;

/// Write (create or overwrite) a UTF-8 text file relative to the working dir.
pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Create or overwrite a UTF-8 text file in the working directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the working directory." },
                    "content": { "type": "string", "description": "Full file contents to write." }
                },
                "required": ["path", "content"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let content = match str_arg(args, "content") {
            Ok(c) => c,
            Err(e) => return e,
        };
        match std::fs::write(&path, content.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("wrote {} bytes to {path}", content.len())),
            Err(e) => ToolOutput::err(format!("write_file({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_then_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let out = WriteFile.run(&json!({ "path": path.to_str().unwrap(), "content": "abc" }));
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
    }

    #[test]
    fn missing_content_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        let out = WriteFile.run(&json!({ "path": path.to_str().unwrap() }));
        assert!(out.is_error);
        assert!(out.text.contains("content"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail/then pass**

Run: `cargo test -p zoid-tools`
Expected: the crate compiles and all tests PASS (these are written against the implementation already, so this task is transcription + verification; if anything fails, fix the named file).

- [ ] **Step 4: Verify no warnings**

Run: `cargo build -p zoid-tools && cargo clippy -p zoid-tools --all-targets`
Expected: 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/zoid-tools/
git commit -m "feat(tools): zoid-tools crate — Tool trait, registry, read_file/write_file"
```

---

## Task 4: `edit_file` and `search` tools

**Files:**
- Create: `crates/zoid-tools/src/edit.rs`, `crates/zoid-tools/src/search.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (declare modules + register)
- Test: in `edit.rs`, `search.rs` test modules

**Interfaces:**
- Produces: `EditFile` (`{ path, old, new }` — replaces a unique occurrence), `Search` (`{ query, path? }` — recursive substring search), both registered in `registry()`.

- [ ] **Step 1: Write `edit.rs` with failing tests**

Create `crates/zoid-tools/src/edit.rs`:
```rust
use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use zoid_provider::ToolSpec;

/// Replace the unique occurrence of `old` with `new` in a file. Errors if `old`
/// is absent or appears more than once (forces unambiguous edits).
pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Replace the unique occurrence of `old` with `new` in a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old":  { "type": "string", "description": "Exact text to find (must occur exactly once)." },
                    "new":  { "type": "string", "description": "Replacement text." }
                },
                "required": ["path", "old", "new"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let path = match str_arg(args, "path") { Ok(p) => p, Err(e) => return e };
        let old = match str_arg(args, "old") { Ok(o) => o, Err(e) => return e };
        let new = match str_arg(args, "new") { Ok(n) => n, Err(e) => return e };

        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("edit_file({path}): {e}")),
        };
        let count = contents.matches(&old).count();
        if count == 0 {
            return ToolOutput::err(format!("edit_file({path}): `old` not found"));
        }
        if count > 1 {
            return ToolOutput::err(format!("edit_file({path}): `old` is ambiguous ({count} matches)"));
        }
        let updated = contents.replacen(&old, &new, 1);
        match std::fs::write(&path, updated.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("edited {path}")),
            Err(e) => ToolOutput::err(format!("edit_file({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed(content: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path.to_str().unwrap().to_string())
    }

    #[test]
    fn replaces_unique_occurrence() {
        let (_d, path) = seed("alpha beta gamma");
        let out = EditFile.run(&json!({ "path": path, "old": "beta", "new": "BETA" }));
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha BETA gamma");
    }

    #[test]
    fn ambiguous_match_is_error() {
        let (_d, path) = seed("x x");
        let out = EditFile.run(&json!({ "path": path, "old": "x", "new": "y" }));
        assert!(out.is_error);
        assert!(out.text.contains("ambiguous"));
    }

    #[test]
    fn absent_match_is_error() {
        let (_d, path) = seed("hello");
        let out = EditFile.run(&json!({ "path": path, "old": "zzz", "new": "y" }));
        assert!(out.is_error);
        assert!(out.text.contains("not found"));
    }
}
```

- [ ] **Step 2: Write `search.rs` with failing tests**

Create `crates/zoid-tools/src/search.rs`:
```rust
use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Recursive literal (substring) search over text files under a root directory
/// (default `.`). Skips hidden entries and common build dirs. Returns up to
/// `MAX_RESULTS` `relpath:line: text` matches.
pub struct Search;

impl Tool for Search {
    fn name(&self) -> &str {
        "search"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Recursively search files for a literal substring (like grep -F).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Literal substring to find." },
                    "path":  { "type": "string", "description": "Root directory to search (default '.')." }
                },
                "required": ["query"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let query = match str_arg(args, "query") { Ok(q) => q, Err(e) => return e };
        if query.is_empty() {
            return ToolOutput::err("search: empty query");
        }
        let root = args.get("path").and_then(|v| v.as_str()).unwrap_or(".").to_string();
        let mut hits: Vec<String> = Vec::new();
        walk(Path::new(&root), Path::new(&root), &query, &mut hits);
        if hits.is_empty() {
            ToolOutput::ok(format!("no matches for {query:?}"))
        } else {
            let truncated = hits.len() >= MAX_RESULTS;
            let mut text = hits.join("\n");
            if truncated {
                text.push_str(&format!("\n… (truncated at {MAX_RESULTS} matches)"));
            }
            ToolOutput::ok(text)
        }
    }
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn walk(root: &Path, dir: &Path, query: &str, hits: &mut Vec<String>) {
    if hits.len() >= MAX_RESULTS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    // Deterministic order: collect + sort by path.
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if hits.len() >= MAX_RESULTS {
            return;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip(name) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, query, hits);
        } else if let Ok(contents) = std::fs::read_to_string(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            for (i, line) in contents.lines().enumerate() {
                if line.contains(query) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    if hits.len() >= MAX_RESULTS {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_matches_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\nNEEDLE here\nthree").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "nothing\nalso NEEDLE").unwrap();

        let out = Search.run(&json!({ "query": "NEEDLE", "path": dir.path().to_str().unwrap() }));
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.txt:2:"));
        assert!(out.text.contains("sub/b.txt:2:") || out.text.contains("sub\\b.txt:2:"));
    }

    #[test]
    fn skips_hidden_and_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/x.txt"), "NEEDLE").unwrap();
        let out = Search.run(&json!({ "query": "NEEDLE", "path": dir.path().to_str().unwrap() }));
        assert!(out.text.contains("no matches"));
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "abc").unwrap();
        let out = Search.run(&json!({ "query": "zzz", "path": dir.path().to_str().unwrap() }));
        assert!(!out.is_error);
        assert!(out.text.contains("no matches"));
    }
}
```

- [ ] **Step 3: Register the new tools**

In `crates/zoid-tools/src/lib.rs`, add module declarations near the top:
```rust
pub mod edit;
pub mod read;
pub mod search;
pub mod write;
```
and extend `registry()`:
```rust
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::ReadFile),
        Box::new(write::WriteFile),
        Box::new(edit::EditFile),
        Box::new(search::Search),
    ]
}
```
Also extend the `registry_has_unique_named_tools` test assertions to include `"edit_file"` and `"search"`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tools`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/
git commit -m "feat(tools): edit_file (unique-match) + search (recursive substring)"
```

---

## Task 5: `shell` tool

**Files:**
- Create: `crates/zoid-tools/src/shell.rs`
- Modify: `crates/zoid-tools/src/lib.rs`
- Test: in `shell.rs` test module

**Interfaces:**
- Produces: `Shell` (`{ command: string }` — runs via `sh -c` / `cmd /C`, captures stdout+stderr+exit code), registered in `registry()`.

- [ ] **Step 1: Write `shell.rs` with failing tests**

Create `crates/zoid-tools/src/shell.rs`:
```rust
use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::process::Command;
use zoid_provider::ToolSpec;

/// Run a shell command in the working directory and capture its output.
/// (Chat is safe by human presence, spec §9 — no sandbox.)
pub struct Shell;

impl Tool for Shell {
    fn name(&self) -> &str {
        "shell"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Run a shell command in the working directory; returns stdout, stderr, and exit code.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "command": { "type": "string", "description": "Command line to execute." } },
                "required": ["command"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let command = match str_arg(args, "command") { Ok(c) => c, Err(e) => return e };

        let output = if cfg!(windows) {
            Command::new("cmd").arg("/C").arg(&command).output()
        } else {
            Command::new("sh").arg("-c").arg(&command).output()
        };
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                let code = o.status.code().unwrap_or(-1);
                let mut text = String::new();
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                text.push_str(&format!("\n[exit {code}]"));
                ToolOutput { text, is_error: code != 0 }
            }
            Err(e) => ToolOutput::err(format!("shell({command}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_command_captures_stdout_and_exit() {
        let out = Shell.run(&json!({ "command": "echo hello-zoid" }));
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("hello-zoid"));
        assert!(out.text.contains("[exit 0]"));
    }

    #[test]
    fn nonzero_exit_is_error() {
        let out = Shell.run(&json!({ "command": "exit 3" }));
        assert!(out.is_error);
        assert!(out.text.contains("[exit 3]"));
    }

    #[test]
    fn missing_command_is_error() {
        let out = Shell.run(&json!({}));
        assert!(out.is_error);
        assert!(out.text.contains("command"));
    }
}
```

- [ ] **Step 2: Register and run to verify failure→pass**

In `crates/zoid-tools/src/lib.rs`: add `pub mod shell;` and `Box::new(shell::Shell),` to `registry()`; add `"shell"` to the uniqueness test's assertions.

Run: `cargo test -p zoid-tools`
Expected: PASS.

- [ ] **Step 3: Verify no warnings**

Run: `cargo clippy -p zoid-tools --all-targets`
Expected: 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/
git commit -m "feat(tools): shell tool (sh -c / cmd -C, captured output + exit code)"
```

---

## Task 6: Core — tool events + tool-aware `conversation()` projection

**Files:**
- Modify: `crates/zoid-core/src/event.rs`, `crates/zoid-core/src/projection.rs`
- Test: both files' test modules

**Interfaces:**
- Produces: `EventKind::ToolCall { id: String, name: String, args: String }`; `EventKind::ToolResult { id: String, name: String, output: String, is_error: bool }`; `projection::ChatMsg`; `projection::ToolCallRef`; `pub fn conversation(events: &[Event]) -> Vec<ChatMsg>`.
- Consumes: existing `Event`/`EventKind`.
- Note: `transcript()`/`Turn` are retained this task (still used by the renderer until Task 9). `conversation()` is additive.

- [ ] **Step 1: Add tool event kinds + round-trip test**

In `crates/zoid-core/src/event.rs`, extend `EventKind`:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ModelDelta { text: String },
    /// A tool the model asked to call. `args` is the raw JSON arguments (stored
    /// as a string so `EventKind` keeps `Eq`).
    ToolCall { id: String, name: String, args: String },
    /// The result of running a `ToolCall`. `output` is the tool's text output.
    ToolResult { id: String, name: String, output: String, is_error: bool },
}
```

Add a round-trip test in `event.rs`'s `mod tests`:
```rust
#[test]
fn tool_events_round_trip() {
    let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    let call = Event::new(id, None, 1, EventKind::ToolCall {
        id: "c1".into(), name: "read_file".into(), args: r#"{"path":"a"}"#.into(),
    });
    let res = Event::new(id, None, 2, EventKind::ToolResult {
        id: "c1".into(), name: "read_file".into(), output: "data".into(), is_error: false,
    });
    for ev in [call, res] {
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-core event::`
Expected: FAIL — `transcript()` in `projection.rs` no longer compiles (its `match` over `EventKind` is non-exhaustive). That's expected; fix in Step 3.

- [ ] **Step 3: Make `transcript()` exhaustive + add `conversation()`**

In `crates/zoid-core/src/projection.rs`, first make the existing `transcript()` match exhaustive by ignoring tool events (it remains a text-only view used by the current renderer):
```rust
EventKind::ModelDelta { text } => {
    pending.get_or_insert_with(String::new).push_str(text);
}
// Tool events do not appear in the text-only transcript.
EventKind::ToolCall { .. } | EventKind::ToolResult { .. } => {}
```

Then add the tool-aware projection (append to the same file, above the test module):
```rust
/// A reference to a tool call as folded from the log (args kept as raw JSON
/// text, matching `EventKind::ToolCall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// A conversation item: the tool-aware projection consumed by both the renderer
/// and the provider request builder. An assistant item carries any tool calls it
/// made in the same turn; tool results are their own items, in log order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMsg {
    User(String),
    Assistant { text: String, tool_calls: Vec<ToolCallRef> },
    ToolResult { id: String, name: String, output: String, is_error: bool },
}

/// Fold the event log into ordered `ChatMsg` items. A run of `ModelDelta` plus
/// any `ToolCall`s before the next user/tool-result/assistant boundary collapses
/// into one `Assistant` item; `ToolResult` events become their own items. Pure.
pub fn conversation(events: &[Event]) -> Vec<ChatMsg> {
    let mut out: Vec<ChatMsg> = Vec::new();
    let mut text: Option<String> = None;
    let mut calls: Vec<ToolCallRef> = Vec::new();

    fn flush(text: &mut Option<String>, calls: &mut Vec<ToolCallRef>, out: &mut Vec<ChatMsg>) {
        if text.is_some() || !calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: text.take().unwrap_or_default(),
                tool_calls: std::mem::take(calls),
            });
        }
    }

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text: t } => {
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::User(t.clone()));
            }
            EventKind::AssistantMessage { text: t } => {
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::Assistant { text: t.clone(), tool_calls: Vec::new() });
            }
            EventKind::ModelDelta { text: t } => {
                text.get_or_insert_with(String::new).push_str(t);
            }
            EventKind::ToolCall { id, name, args } => {
                calls.push(ToolCallRef { id: id.clone(), name: name.clone(), args: args.clone() });
            }
            EventKind::ToolResult { id, name, output, is_error } => {
                // The assistant turn that made the call(s) ends here.
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::ToolResult {
                    id: id.clone(), name: name.clone(), output: output.clone(), is_error: *is_error,
                });
            }
        }
    }
    flush(&mut text, &mut calls, &mut out);
    out
}
```

- [ ] **Step 4: Add `conversation()` tests**

In `projection.rs`'s `mod tests`, add helpers + tests:
```rust
fn tcall(id: u128, name: &str, args: &str) -> Event {
    Event::new(Ulid::from(id), None, 0, EventKind::ToolCall {
        id: "".into(), name: name.into(), args: args.into(),
    })
}
fn tres(id: u128, name: &str, output: &str) -> Event {
    Event::new(Ulid::from(id), None, 0, EventKind::ToolResult {
        id: "".into(), name: name.into(), output: output.into(), is_error: false,
    })
}

#[test]
fn conversation_folds_text_calls_results_in_order() {
    let events = vec![
        user(1, "read a"),
        delta(2, "let me "),
        delta(3, "look"),
        tcall(4, "read_file", r#"{"path":"a"}"#),
        tres(5, "read_file", "data"),
        delta(6, "it says data"),
    ];
    let conv = conversation(&events);
    assert_eq!(conv, vec![
        ChatMsg::User("read a".into()),
        ChatMsg::Assistant {
            text: "let me look".into(),
            tool_calls: vec![ToolCallRef { id: "".into(), name: "read_file".into(), args: r#"{"path":"a"}"#.into() }],
        },
        ChatMsg::ToolResult { id: "".into(), name: "read_file".into(), output: "data".into(), is_error: false },
        ChatMsg::Assistant { text: "it says data".into(), tool_calls: vec![] },
    ]);
}

#[test]
fn tool_call_only_turn_has_empty_text() {
    let events = vec![user(1, "go"), tcall(2, "shell", r#"{"command":"ls"}"#), tres(3, "shell", "ok")];
    let conv = conversation(&events);
    assert_eq!(conv[1], ChatMsg::Assistant {
        text: "".into(),
        tool_calls: vec![ToolCallRef { id: "".into(), name: "shell".into(), args: r#"{"command":"ls"}"#.into() }],
    });
}

proptest! {
    #[test]
    fn conversation_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,8}", 0..15)) {
        let events: Vec<Event> = texts.iter().enumerate().map(|(i, t)| user(i as u128 + 1, t)).collect();
        prop_assert_eq!(conversation(&events), conversation(&events));
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-core`
Expected: PASS (existing transcript tests + new conversation tests + event round-trip).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs
git commit -m "feat(core): ToolCall/ToolResult events + tool-aware conversation() projection"
```

---

## Task 7: `zoid` lib — the terminal-free agent loop

**Files:**
- Create: `crates/zoid/src/lib.rs`, `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/Cargo.toml` (add `[lib]`, `zoid-tools` dep), `crates/zoid/src/main.rs` (declare `mod input;` move — see note), `crates/zoid/src/input.rs` (becomes part of the lib)
- Test: `crates/zoid/tests/agent_loop.rs` (new integration test)

**Interfaces:**
- Produces: `pub mod agent` with `pub enum AgentUpdate { Appended(Event), TurnComplete }`; `pub const MAX_TOOL_ITERATIONS: u32`; `pub const SYSTEM_PROMPT: &str`; `pub async fn run_agent_turn(provider: Arc<dyn Provider>, tools: Arc<Vec<Box<dyn Tool>>>, session: SessionHandle, seed_events: Vec<Event>, model: String, ui: mpsc::Sender<AgentUpdate>, now: fn() -> i64) -> anyhow::Result<()>`; `pub fn tool_specs(tools: &[Box<dyn Tool>]) -> Vec<ToolSpec>`; `pub fn build_request(events: &[Event], model: &str, tools: &[Box<dyn Tool>]) -> CompletionRequest`.
- Consumes: `zoid_core` (`Event`, `EventKind`, `SessionHandle`, `conversation`, `ChatMsg`, `ToolCallRef`), `zoid_provider`, `zoid_tools`.

- [ ] **Step 1: Make `zoid` a lib + add the dependency**

Edit `crates/zoid/Cargo.toml`. Add a `[lib]` section and the `zoid-tools` dependency:
```toml
[lib]
name = "zoid"
path = "src/lib.rs"

[[bin]]
name = "zoid"
path = "src/main.rs"

[dependencies]
zoid-core = { path = "../zoid-core" }
zoid-provider = { path = "../zoid-provider" }
zoid-tools = { path = "../zoid-tools" }
zoid-tui = { path = "../zoid-tui" }
anyhow = { workspace = true }
ratatui = { workspace = true }
crossterm = { workspace = true }
tokio = { workspace = true }
tui-textarea = { workspace = true }
ulid = { workspace = true }
futures-util = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Create `crates/zoid/src/lib.rs`:
```rust
//! zoid library surface: the terminal-free agent loop and key classification,
//! reused by the binary and exercised by integration tests against a fake
//! provider (spec §13).

pub mod agent;
pub mod input;
```

Move key classification into the lib: `crates/zoid/src/input.rs` already exists and is `mod input;` in `main.rs`. Change `main.rs`'s `mod input;` to `use zoid::input::{classify, KeyAction};` (and delete the `mod input;` line). The file stays where it is; it is now reached via the lib.

- [ ] **Step 2: Write the failing integration test**

Create `crates/zoid/tests/agent_loop.rs`:
```rust
//! Drives the terminal-free agent loop against a deterministic multi-turn fake
//! provider and the real tool registry, asserting the persisted event log.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Provider, ProviderEvent, ToolCall};

/// A provider that replays one scripted stream per `stream()` call, in order.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(&self, _req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> anyhow::Result<()> {
        let script = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

#[tokio::test]
async fn agent_loop_runs_tool_then_finishes() {
    // Arrange a write_file in a tempdir so the tool actually executes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let path_str = path.to_str().unwrap().to_string();

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model calls write_file, then ends its turn.
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "write_file".into(),
                    args: json!({ "path": path_str, "content": "hi" }),
                }),
                ProviderEvent::Done,
            ],
            // Turn 2: with the tool result in context, the model replies in text.
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(ulid::Ulid::new(), None, 0, EventKind::UserMessage { text: "write hi".into() })];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    // Drain UI updates so the channel never blocks.
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn(provider, tools, session.clone(), seed, "fake".into(), tx, fixed_now)
        .await
        .unwrap();

    let complete = drain.await.unwrap();
    assert!(complete, "loop must emit TurnComplete");

    // The tool actually ran.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");

    // The log records: UserMessage, ToolCall, ToolResult, ModelDelta.
    let log = session.snapshot().await.unwrap();
    let kinds: Vec<&EventKind> = log.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], EventKind::UserMessage { .. }));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ToolCall { name, .. } if name == "write_file")));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ToolResult { is_error: false, .. })));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ModelDelta { text } if text == "done")));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p zoid --test agent_loop`
Expected: FAIL to compile (`zoid::agent` doesn't exist yet).

- [ ] **Step 4: Implement `agent.rs`**

Create `crates/zoid/src/agent.rs`:
```rust
//! The terminal-free agent loop: stream a turn, execute any tool calls in the
//! working directory, record everything as events, and re-request until the
//! model stops calling tools (or the iteration cap trips).

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider, ProviderEvent, ToolCall, ToolSpec};
use zoid_tools::Tool;

/// System prompt for Chat-mode turns.
pub const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     You can call tools to read, write, edit, and search files and run shell \
     commands in the user's working directory. Use them when helpful.";

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 25;

/// UI-facing updates emitted as the turn progresses.
pub enum AgentUpdate {
    /// A new event was persisted; the UI should cache it and redraw.
    Appended(Event),
    /// The turn is finished (model produced no further tool calls / cap / error).
    TurnComplete,
}

/// The tool specs to advertise to the provider.
pub fn tool_specs(tools: &[Box<dyn Tool>]) -> Vec<ToolSpec> {
    tools.iter().map(|t| t.spec()).collect()
}

/// Map a folded `ChatMsg` to a provider `Message`.
fn map_msg(m: ChatMsg) -> Message {
    match m {
        ChatMsg::User(text) => Message::user(text),
        ChatMsg::Assistant { text, tool_calls } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: text,
            tool_calls: tool_calls
                .into_iter()
                .map(|c| ToolCall {
                    id: c.id,
                    name: c.name,
                    args: serde_json::from_str(&c.args).unwrap_or(serde_json::Value::Null),
                })
                .collect(),
            tool_name: None,
        },
        ChatMsg::ToolResult { name, output, .. } => Message::tool(name, output),
    }
}

/// Build a completion request from the current event log.
pub fn build_request(events: &[Event], model: &str, tools: &[Box<dyn Tool>]) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: Some(SYSTEM_PROMPT.to_string()),
        messages: conversation(events).into_iter().map(map_msg).collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}

/// Run one user-message-to-completion agent turn. `seed_events` is the current
/// log snapshot (including the just-appended user message). Every event this
/// produces is persisted via `session` and announced via `ui`.
pub async fn run_agent_turn(
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    session: SessionHandle,
    mut events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<()> {
    let mut iterations: u32 = 0;

    'turn: loop {
        let req = build_request(&events, &model, &tools);

        // Stream one model turn. Spawn the provider so a missing terminal Done
        // (truncated stream) can't hang us — we send our own Done after it ends.
        let (ptx, mut prx) = mpsc::channel::<ProviderEvent>(256);
        let p = provider.clone();
        let stream_task = tokio::spawn(async move {
            let _ = p.stream(&req, ptx.clone()).await;
            let _ = ptx.send(ProviderEvent::Done).await;
        });

        let mut pending: Vec<ToolCall> = Vec::new();
        while let Some(pe) = prx.recv().await {
            match pe {
                ProviderEvent::TextDelta(s) => {
                    emit(&session, &mut events, &ui, EventKind::ModelDelta { text: s }, now).await?;
                }
                ProviderEvent::ToolCall(tc) => {
                    emit(
                        &session,
                        &mut events,
                        &ui,
                        EventKind::ToolCall { id: tc.id.clone(), name: tc.name.clone(), args: tc.args.to_string() },
                        now,
                    )
                    .await?;
                    pending.push(tc);
                }
                ProviderEvent::Usage(_) => { /* token ledger lands in P3 */ }
                ProviderEvent::Error(msg) => {
                    emit(
                        &session,
                        &mut events,
                        &ui,
                        EventKind::AssistantMessage {
                            text: format!("{} {msg}", zoid_tui::tokens::glyph::WARNING),
                        },
                        now,
                    )
                    .await?;
                    let _ = stream_task.await;
                    break 'turn;
                }
                ProviderEvent::Done => break,
            }
        }
        let _ = stream_task.await;

        if pending.is_empty() {
            break 'turn; // model answered without tools — turn complete
        }

        iterations += 1;
        if iterations > MAX_TOOL_ITERATIONS {
            emit(
                &session,
                &mut events,
                &ui,
                EventKind::AssistantMessage {
                    text: format!("{} tool-iteration limit reached", zoid_tui::tokens::glyph::WARNING),
                },
                now,
            )
            .await?;
            break 'turn;
        }

        // Execute each pending tool in the working directory (blocking work off
        // the async runtime), recording its result as an event.
        for tc in pending {
            let tools_for_exec = tools.clone();
            let name = tc.name.clone();
            let args = tc.args.clone();
            let out = tokio::task::spawn_blocking(move || {
                zoid_tools::run_tool(&tools_for_exec, &name, &args)
            })
            .await?;
            emit(
                &session,
                &mut events,
                &ui,
                EventKind::ToolResult { id: tc.id, name: tc.name, output: out.text, is_error: out.is_error },
                now,
            )
            .await?;
        }
        // loop: re-request with the tool results now in context
    }

    let _ = ui.send(AgentUpdate::TurnComplete).await;
    Ok(())
}

/// Persist one event and announce it to the UI, keeping the local log in sync.
async fn emit(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    kind: EventKind,
    now: fn() -> i64,
) -> Result<()> {
    let ev = Event::new(Ulid::new(), None, now(), kind);
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(ev)).await;
    Ok(())
}
```

- [ ] **Step 5: Run the integration test (and the suite) to verify pass**

Run: `cargo test -p zoid`
Expected: PASS (`agent_loop` integration test green; `input` unit tests still green via the lib).

- [ ] **Step 6: Verify no warnings, then commit**

Run: `cargo clippy -p zoid --all-targets`
Expected: 0 warnings. (main.rs still compiles: it now `use`s `zoid::input` — if you haven't yet wired the rest of main to the agent module, that's Task 8; ensure the bin at least builds. If `App::request`/`transcript` usage now conflicts, leave them until Task 8 but keep the bin compiling — the `mod input;`→`use zoid::input` change is the only main.rs edit required here.)

```bash
git add crates/zoid/Cargo.toml crates/zoid/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/tests/agent_loop.rs Cargo.lock
git commit -m "feat(zoid): terminal-free agent loop (tools + re-request), lib target + fake-provider test"
```

---

## Task 8: Wire `main.rs` to the agent loop

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Test: covered by the existing `agent_loop` integration test + manual smoke (the live `run()` is terminal-bound; its logic is the tested `run_agent_turn`).

**Interfaces:**
- Consumes: `zoid::agent::{run_agent_turn, AgentUpdate}`, `zoid_tools::{registry, Tool}`.
- Produces: a `run()` loop that dispatches a turn on submit and redraws from `AgentUpdate`.

- [ ] **Step 1: Replace request-building + streaming wiring in `main.rs`**

Update imports at the top of `crates/zoid/src/main.rs`:
- Remove `use zoid_provider::{CompletionRequest, Message, MsgRole, Provider, ProviderEvent};` and `use zoid_core::projection::{transcript, Role};`.
- Add:
```rust
use std::sync::Arc;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::projection::transcript; // still used by render path until Task 9
use zoid_provider::Provider;
use zoid_tools::Tool;
```
(Keep `use zoid::input::{classify, KeyAction};` from Task 7.)

Change `App` to hold the tool registry and drop the now-unused `request()` method:
```rust
struct App {
    session: SessionHandle,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    model: String,
    textarea: TextArea<'static>,
    streaming: bool,
}
```
Delete the `fn request(&self) -> CompletionRequest { … }` method (its job moved to `agent::build_request`). Keep `record()`.

In `main()`, build the tools:
```rust
let mut app = App {
    session,
    events,
    provider: default_provider(),
    tools: Arc::new(zoid_tools::registry()),
    model,
    textarea: TextArea::default(),
    streaming: false,
};
```
Remove the now-unused `SYSTEM_PROMPT` const from `main.rs` (it lives in `agent.rs` now) and the `const SYSTEM_PROMPT` line.

- [ ] **Step 2: Replace the `run()` select loop's provider channel + handlers**

In `run()`, change the channel type and the submit + receive arms:

Replace the delta channel with the agent-update channel:
```rust
let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);
```

Replace the `KeyAction::Submit` body:
```rust
KeyAction::Submit => {
    if app.streaming { continue; }
    let text = app.textarea.lines().join("\n");
    if text.trim().is_empty() { continue; }
    app.textarea = TextArea::default();
    app.record(EventKind::UserMessage { text }).await?;
    app.streaming = true;

    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = ui_tx.clone();
    tokio::spawn(async move {
        let _ = run_agent_turn(provider, tools, session, seed, model, ui, now_ms).await;
    });
}
```

Replace the provider-receive `select!` arm:
```rust
Some(update) = ui_rx.recv() => {
    match update {
        AgentUpdate::Appended(ev) => { app.events.push(ev); }
        AgentUpdate::TurnComplete => { app.streaming = false; }
    }
}
```

Remove the old `Some(pe) = delta_rx.recv()` arm entirely and the `delta_tx`/`delta_rx` bindings.

- [ ] **Step 3: Build and verify the binary compiles warning-free**

Run: `cargo build -p zoid && cargo clippy -p zoid --all-targets`
Expected: builds; 0 warnings. (`transcript` is still imported for the render call `render_chat(f, &turns, …)` — that's fine until Task 9.)

- [ ] **Step 4: Run the whole workspace test suite**

Run: `cargo test`
Expected: PASS across all crates.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): drive Chat via the agent loop; redraw from AgentUpdate"
```

---

## Task 9: Inline tool rendering (`→ peek`) + retire `transcript()`

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`, `crates/zoid-tui/tests/chat_snapshot.rs`, `crates/zoid/src/main.rs`, `crates/zoid-core/src/projection.rs`
- Test: `crates/zoid-tui/tests/chat_snapshot.rs` (insta snapshots)

**Interfaces:**
- Consumes: `zoid_core::projection::{conversation, ChatMsg, ToolCallRef}`.
- Produces: `render_chat(frame, &[ChatMsg], &TextArea, streaming)` rendering user/assistant text and inline tool cards; `transcript()`/`Turn`/`Role` removed.

- [ ] **Step 1: Update the snapshot test to the new signature + add a tool snapshot**

Rewrite `crates/zoid-tui/tests/chat_snapshot.rs` to drive `render_chat` with `ChatMsg`:
```rust
use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::render_chat;

fn draw(msgs: &[ChatMsg], streaming: bool) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, msgs, &input, streaming)).unwrap();
    terminal.backend().to_string()
}

#[test]
fn empty_chat_frame() {
    insta::assert_snapshot!(draw(&[], false));
}

#[test]
fn seeded_transcript_frame() {
    let msgs = vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant { text: "an unwrapped lookup in the handler.".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}

#[test]
fn streaming_caret_frame() {
    let msgs = vec![
        ChatMsg::User("hi".into()),
        ChatMsg::Assistant { text: "thinking".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, true));
}

#[test]
fn tool_call_and_result_frame() {
    let msgs = vec![
        ChatMsg::User("read a.txt".into()),
        ChatMsg::Assistant {
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef { id: "".into(), name: "read_file".into(), args: r#"{"path":"a.txt"}"#.into() }],
        },
        ChatMsg::ToolResult { id: "".into(), name: "read_file".into(), output: "file body".into(), is_error: false },
        ChatMsg::Assistant { text: "it contains the config.".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}
```

Delete the three stale `.snap` files so insta regenerates them (the layout/height changed to 12 rows and the message type changed):
```bash
rm crates/zoid-tui/tests/snapshots/chat_snapshot__empty_chat_frame.snap \
   crates/zoid-tui/tests/snapshots/chat_snapshot__seeded_transcript_frame.snap \
   crates/zoid-tui/tests/snapshots/chat_snapshot__streaming_caret_frame.snap
```

- [ ] **Step 2: Reimplement `render_chat` over `ChatMsg`**

Rewrite the conversation-building part of `crates/zoid-tui/src/chat.rs`. Change the signature and the body block; keep the title/input/status bars. Replace `use zoid_core::projection::{Role, Turn};` with `use zoid_core::projection::ChatMsg;`.

```rust
pub fn render_chat(frame: &mut Frame, msgs: &[ChatMsg], input: &TextArea<'_>, streaming: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(3), // input box (bordered)
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    // Title bar (unchanged).
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Conversation: user/assistant text turns + inline tool cards.
    let last = msgs.len().saturating_sub(1);
    let body: Vec<Line> = if msgs.is_empty() {
        vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))]
    } else {
        let mut lines: Vec<Line> = Vec::new();
        for (i, m) in msgs.iter().enumerate() {
            match m {
                ChatMsg::User(text) => {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{} ", glyph::USER_TURN), Style::new().fg(color::CHAT_ACCENT)),
                        Span::styled(text.clone(), Style::new().fg(color::TXT)),
                    ]));
                }
                ChatMsg::Assistant { text, tool_calls } => {
                    let mut shown = text.clone();
                    if streaming && i == last && tool_calls.is_empty() {
                        shown.push(glyph::CARET);
                    }
                    if !shown.is_empty() || tool_calls.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("zoid ".to_string(), Style::new().fg(color::DIM)),
                            Span::styled(shown, Style::new().fg(color::TXT)),
                        ]));
                    }
                    for tc in tool_calls {
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {} ", glyph::EDIT), Style::new().fg(color::CHAT_ACCENT)),
                            Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
                            Span::styled(format!("({})", arg_summary(&tc.args)), Style::new().fg(color::DIM)),
                            Span::styled(format!(" {} peek", glyph::RETURN), Style::new().fg(color::DIM)),
                        ]));
                    }
                }
                ChatMsg::ToolResult { name, output, is_error, .. } => {
                    let (mark, mark_color) = if *is_error {
                        (glyph::WARNING, color::ERROR)
                    } else {
                        (glyph::PASS, color::OK)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                        Span::styled(name.clone(), Style::new().fg(color::DIM)),
                        Span::styled(format!(" → {}", first_line(output)), Style::new().fg(color::DIM)),
                    ]));
                }
            }
        }
        lines
    };
    frame.render_widget(Paragraph::new(body), chunks[1]);

    // Input box + status bar (unchanged from the existing implementation).
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(input_block, chunks[2]);
    let inner = chunks[2].inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(input, inner);

    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!("· {}Tab Build · {} send · ^C quit", glyph::SHIFT, glyph::RETURN),
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[3]);
}

/// A compact one-line summary of a tool call's JSON args for the inline card.
fn arg_summary(args_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    }
}

fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 30),
        other => truncate(&other.to_string(), 30),
    }
}

fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 40)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}
```

Add `serde_json` to `crates/zoid-tui/Cargo.toml` dependencies (it parses the args summary):
```toml
serde_json = { workspace = true }
```
(Check the file; add under `[dependencies]` if not present.)

- [ ] **Step 3: Switch `main.rs` to `conversation()` + `render_chat(ChatMsg)`**

In `crates/zoid/src/main.rs`:
- Replace `use zoid_core::projection::transcript;` with `use zoid_core::projection::conversation;`.
- In `run()`, change the per-frame build:
```rust
let msgs = conversation(&app.events);
terminal.draw(|f| render_chat(f, &msgs, &app.textarea, app.streaming))?;
```

- [ ] **Step 4: Retire `transcript()`/`Turn`/`Role` from core**

In `crates/zoid-core/src/projection.rs`, delete `pub enum Role`, `pub struct Turn`, `pub fn transcript()`, and the transcript-specific tests (`consecutive_deltas_fold_into_one_assistant_turn`, `delta_run_ends_at_next_user_message`, `assistant_message_and_delta_run_are_separate_turns`, `maps_events_to_turns_in_order`, and the two `transcript`-based proptests). Keep the `conversation` tests + the `conversation_is_deterministic` proptest. Keep the `user`/`asst`/`delta`/`tcall`/`tres` helpers that the remaining tests use (remove any that become unused to stay warning-free).

Verify nothing else references `transcript`/`Turn`/`Role`:
```bash
grep -rn "transcript\|projection::Turn\|projection::Role\|Role::\|Turn {" crates --include=*.rs
```
Expected: no remaining references outside the deleted definitions.

- [ ] **Step 5: Run tests + generate snapshots**

Run: `cargo test -p zoid-core && cargo test -p zoid-tui`
Expected: `zoid-core` PASS. `zoid-tui` snapshot tests will create **new** `.snap.new` files (insta) and report as failed-pending on first run. Review them:
```bash
cargo insta review   # accept if the rendered frames look correct
# or, if cargo-insta isn't installed, inspect and accept manually:
INSTA_UPDATE=always cargo test -p zoid-tui
```
Re-run `cargo test -p zoid-tui`; Expected: PASS with the four accepted snapshots committed.

- [ ] **Step 6: Full suite + clippy, then commit**

Run: `cargo test && cargo clippy --all-targets`
Expected: all green, 0 warnings.

```bash
git add crates/zoid-tui/ crates/zoid/src/main.rs crates/zoid-core/src/projection.rs
git commit -m "feat(tui): inline tool cards with → peek; render from conversation(); retire transcript()"
```

---

## Final Verification (before whole-branch review)

- [ ] `cargo build --locked` — exit 0 (Cargo.lock committed and current).
- [ ] `cargo test` — all crates green, including the `agent_loop` integration test and the four chat snapshots.
- [ ] `cargo clippy --all-targets` — 0 warnings.
- [ ] Manual smoke (live, real terminal): `OLLAMA_API_KEY=… cargo run -p zoid`, ask it to read a file (e.g. *"show me the contents of Cargo.toml"*) and confirm a tool card + result render inline and the model answers from the result. (The orchestrator runs this, or hands the user the command.)
- [ ] No `Co-Authored-By` / co-author trailer on any commit: `git log --format='%an <%ae>%n%b' <base>..HEAD | grep -i "co-authored" || echo clean`.

## Self-Review (against spec + roadmap)

- **Roadmap P1 "tool-calling; the agent loop; core tools (fs read/write/edit, shell, code search); inline tool rendering with → peek"** → Tasks 1–2 (tool-calling wire), 3–5 (core tools), 6–7 (agent loop + events), 9 (inline rendering with `→ peek`). ✅
- **Spec §9 "Tools run in cwd in Chat; tool calls/results are events"** → tools execute in process cwd (Task 5/7); `ToolCall`/`ToolResult` events (Task 6). ✅
- **Spec §13 "Agent loop tested against a fake provider replaying tool-call sequences"** → `agent_loop.rs` with `ScriptedProvider` (Task 7). ✅
- **Pinned constraint (OpenAI/Ollama tools, not Anthropic tool_use)** → Task 2 native shape; Anthropic explicitly text-only (Non-goals). ✅
- **Type consistency:** `ToolSpec`/`ToolCall` (provider) ↔ `ToolCallRef`/`ChatMsg`/`EventKind::ToolCall` (core) ↔ `ToolOutput`/`Tool` (tools) — args carried as `Value` on the wire, `String` in the log, parsed back at the seam (`agent::map_msg`). ✅
- **Placeholder scan:** every code step carries complete code; no TODO/TBD. ✅
