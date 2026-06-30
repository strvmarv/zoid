# P5b · Constructed-Context Builder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the P3 constructed-context assembler into a request builder — turn the event log + a task into a precise subagent `CompletionRequest` containing the task and exactly the relevant code, **never the session history** (spec §4.4).

**Architecture:** A pure core helper `file_contents(events)` resolves each File context item's key (`file:{path}`) to its latest tool-result content. The bin's `build_subagent_request(task, events, policy, …)` runs `context_window` → `assemble_context` (P3), keeps the **included File** items, resolves their content, and composes a task-focused system prompt + a single user message of `{task + relevant files}`. Messages and tool-results (session history) are deliberately excluded — the subagent starts clean. This is the assembler-to-dispatch wiring P3 deferred; the executor that *runs* the request is P5c.

**Tech Stack:** Rust 2021. Consumes P3 (`assemble_context`, `context_window`, `ContextPolicy`) and the provider request types.

## Global Constraints

- **Crates & dep direction:** `file_contents` is **pure** in `crates/zoid-core/src/context.rs` (no provider). `build_subagent_request` lives in the `zoid` bin (`crates/zoid/src/subagent.rs` — new) because it emits a provider `CompletionRequest`, exactly like `agent::build_request`. No new crate (P5 decision).
- **Never session history (spec §4.4):** the subagent context is **task + relevant code files only**. Do NOT include conversation messages or tool-result transcripts — the subagent gets a precisely-constructed context, not the chat log. (This is the whole point of ⑤ feeding context construction.)
- **Reuse the P3 assembler (DRY):** selection of *what* is relevant is `assemble_context(window, policy)` — do not reimplement pin/evict/cold/ceiling logic. `build_subagent_request` only *resolves and formats* the already-selected items.
- **Key parity:** `file_contents` must key by `format!("file:{path}")`, matching `context_window`'s File-item keys exactly, so `ContextItem.key` lookups resolve.
- **Provider-agnostic:** the request targets the existing `Provider` seam (works against the Ollama/GLM endpoint); no provider-specific fields.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit.

---

### Task 1: `file_contents()` — resolve File item keys to content (core)

**Files:**
- Modify: `crates/zoid-core/src/context.rs`
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `Event`, `EventKind`, `economy::tool_path`.
- Produces: `pub fn file_contents(events: &[Event]) -> std::collections::HashMap<String, String>` — maps `"file:{path}"` → the latest **non-error** tool-result output for that path (latest wins), matching `context_window`'s File keys.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-core/src/context.rs` `mod tests` (reuse the existing `u`/`call`/`result` helpers there):

```rust
#[test]
fn file_contents_resolves_latest_output_by_path_key() {
    let evs = vec![
        u("go"),
        call("c1", "read_file", "src/a.rs"),
        result("c1", "read_file", "fn one() {}"),
        call("c2", "read_file", "src/a.rs"),       // re-read → latest wins
        result("c2", "read_file", "fn two() {}"),
        call("c3", "read_file", "src/b.rs"),
        result("c3", "read_file", "// b"),
        call("c4", "shell", "n/a"),                 // not a file → not keyed
        result("c4", "shell", "shell out"),
    ];
    let map = file_contents(&evs);
    assert_eq!(map.get("file:src/a.rs").map(String::as_str), Some("fn two() {}"));
    assert_eq!(map.get("file:src/b.rs").map(String::as_str), Some("// b"));
    assert!(!map.keys().any(|k| k.starts_with("tool:")));
}

#[test]
fn file_contents_skips_errored_results() {
    let evs = vec![
        u("go"),
        call("c1", "read_file", "x.rs"),
        Event::new(ulid::Ulid::new(), None, 0, EventKind::ToolResult {
            id: "c1".into(), name: "read_file".into(), output: "boom".into(), is_error: true,
        }),
    ];
    assert!(file_contents(&evs).get("file:x.rs").is_none());
}
```

> If the test helpers `u`/`call`/`result` are named differently in the merged `context.rs`, use the local equivalents — they construct `UserMessage`, `ToolCall { id, name, args:{"path":..} }`, and `ToolResult { id, name, output, is_error:false }` events.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core context::tests::file_contents`
Expected: FAIL — `file_contents` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid-core/src/context.rs` (after `context_window`):

```rust
/// Resolve each File context item to its content: `"file:{path}"` → the latest
/// non-error tool-result output for that path. Mirrors `context_window`'s File
/// keying so a `ContextItem.key` looks up here. Used by the subagent
/// context builder (P5) to fetch relevant code without the chat transcript.
pub fn file_contents(events: &[Event]) -> HashMap<String, String> {
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut out: HashMap<String, String> = HashMap::new();
    for e in events {
        match &e.kind {
            EventKind::ToolCall { id, args, .. } => {
                if let Some(p) = tool_path(args) {
                    call_path.insert(id.clone(), p);
                }
            }
            EventKind::ToolResult { id, output, is_error, .. } => {
                if !is_error {
                    if let Some(p) = call_path.get(id) {
                        out.insert(format!("file:{p}"), output.clone()); // latest wins
                    }
                }
            }
            _ => {}
        }
    }
    out
}
```

(`HashMap` and `tool_path` are already imported at the top of `context.rs`.)

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-core context::tests::file_contents`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/context.rs
git commit -m "feat(core): file_contents — resolve File item keys to latest content (subagent ctx)"
```

---

### Task 2: `build_subagent_request()` — assembler → CompletionRequest (bin)

**Files:**
- Create: `crates/zoid/src/subagent.rs`
- Modify: `crates/zoid/src/lib.rs` (`pub mod subagent;`)
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `zoid_core::context::{context_window, file_contents, ItemKind}`, `zoid_core::assembler::{assemble_context, ContextPolicy}`, `zoid_provider::{CompletionRequest, Message}`, `crate::agent::tool_specs`, `zoid_tools::Tool`.
- Produces:
  - `pub const SUBAGENT_SYSTEM_PROMPT: &str` — a task-focused autonomous prompt.
  - `pub fn build_subagent_request(task: &str, events: &[Event], policy: &ContextPolicy, model: &str, tools: &[Box<dyn Tool>]) -> CompletionRequest`.

- [ ] **Step 1: Write the failing test**

`crates/zoid/src/subagent.rs` (test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::event::{Event, EventKind};
    use zoid_core::assembler::ContextPolicy;
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
    fn call(id: &str, path: &str) -> Event {
        ev(EventKind::ToolCall { id: id.into(), name: "read_file".into(), args: format!(r#"{{"path":"{path}"}}"#) })
    }
    fn result(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult { id: id.into(), name: "read_file".into(), output: out.into(), is_error: false })
    }

    #[test]
    fn request_carries_task_and_relevant_file_not_history() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "secret chat history".into() }),
            call("c1", "src/ast.rs"),
            result("c1", "fn parse() {}"),
        ];
        let tools = zoid_tools::registry();
        let req = build_subagent_request("refactor parse()", &evs, &ContextPolicy::default(), "glm", &tools);

        assert_eq!(req.model, "glm");
        assert_eq!(req.system.as_deref(), Some(SUBAGENT_SYSTEM_PROMPT));
        assert_eq!(req.messages.len(), 1, "subagent gets one constructed user message");
        let body = &req.messages[0].content;
        assert!(body.contains("refactor parse()"), "task present");
        assert!(body.contains("fn parse() {}"), "relevant file content present");
        assert!(body.contains("src/ast.rs"), "file labeled by path");
        assert!(!body.contains("secret chat history"), "session history excluded (spec §4.4)");
        assert!(!req.tools.is_empty(), "tools advertised");
    }

    #[test]
    fn request_without_files_is_just_the_task() {
        let req = build_subagent_request("do a thing", &[], &ContextPolicy::default(), "glm", &zoid_tools::registry());
        assert!(req.messages[0].content.contains("do a thing"));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid subagent`
Expected: compile error — module/fn undefined.

- [ ] **Step 3: Implement**

`crates/zoid/src/subagent.rs`:

```rust
//! The subagent runtime (spec §4.4/§7, L1). This module builds a subagent's
//! constructed context (task + relevant code, never session history) and — in
//! P5c — runs it. The orchestrator (the Chat loop) dispatches one at a time.

use zoid_core::assembler::{assemble_context, ContextPolicy};
use zoid_core::context::{context_window, file_contents, ItemKind};
use zoid_core::event::Event;
use zoid_provider::{CompletionRequest, Message};
use zoid_tools::Tool;

use crate::agent::tool_specs;

/// A subagent works one discrete task autonomously with a precise context.
pub const SUBAGENT_SYSTEM_PROMPT: &str =
    "You are a zoid subagent. You are given ONE discrete task and the relevant \
     code. Complete the task end to end using the tools (read, write, edit, \
     search, shell). Work autonomously — do not ask questions. When done, give \
     a one-paragraph summary of what you changed.";

/// Per-subagent max output tokens (mirrors the Chat loop's budget).
const SUBAGENT_MAX_TOKENS: u32 = 4096;

/// Build a subagent `CompletionRequest`: the P3 assembler selects the relevant
/// context items from `events`; we keep the included **File** items, resolve
/// their content, and compose a task-focused prompt. Session messages/tool
/// transcripts are intentionally excluded (spec §4.4: never session history).
pub fn build_subagent_request(
    task: &str,
    events: &[Event],
    policy: &ContextPolicy,
    model: &str,
    tools: &[Box<dyn Tool>],
) -> CompletionRequest {
    let window = context_window(events);
    let selection = assemble_context(&window, policy);
    let contents = file_contents(events);

    let mut ctx = String::new();
    for item in selection.included.iter().filter(|i| i.kind == ItemKind::File) {
        if let Some(c) = contents.get(&item.key) {
            ctx.push_str(&format!("\n// {}\n{}\n", item.label, c));
        }
    }

    let user = if ctx.is_empty() {
        format!("Task:\n{task}")
    } else {
        format!("Task:\n{task}\n\nRelevant files:\n{ctx}")
    };

    CompletionRequest {
        model: model.to_string(),
        system: Some(SUBAGENT_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user(user)],
        max_tokens: SUBAGENT_MAX_TOKENS,
        tools: tool_specs(tools),
    }
}
```

In `crates/zoid/src/lib.rs`, add `pub mod subagent;`. (Confirm `agent::tool_specs` is `pub` — it is; if not, widen it.)

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid subagent`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): build_subagent_request — assembler → constructed context (task + relevant code)"
```

---

### Task 3: A subagent context policy with a token ceiling

**Files:**
- Modify: `crates/zoid/src/subagent.rs`
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `ContextPolicy`.
- Produces: `pub fn subagent_policy() -> ContextPolicy` — `auto_evict_cold = true` (default) **plus** a token ceiling so a subagent's constructed context stays bounded.

> The constructed context must fit a budget — an unbounded dump of every file defeats the economy. `subagent_policy()` is the default `build_subagent_request` callers (P5d) use.

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/subagent.rs` `mod tests`:

```rust
#[test]
fn subagent_policy_has_a_ceiling_and_evicts_cold() {
    let p = subagent_policy();
    assert!(p.auto_evict_cold, "cold items dropped from a subagent's context");
    assert!(p.token_ceiling.is_some(), "subagent context is token-bounded");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid subagent::tests::subagent_policy`
Expected: FAIL — `subagent_policy` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid/src/subagent.rs`:

```rust
/// Default context budget for a dispatched subagent: drop cold items and cap
/// the constructed context so it stays a *precise* slice, not a dump.
pub fn subagent_policy() -> ContextPolicy {
    ContextPolicy {
        token_ceiling: Some(SUBAGENT_CONTEXT_CEILING),
        auto_evict_cold: true,
        compact_threshold: None,
    }
}

/// Token ceiling for a subagent's constructed context (≈ half a 64k window,
/// leaving room for the task, tool round-trips, and output).
const SUBAGENT_CONTEXT_CEILING: u64 = 32_000;
```

> Confirm `ContextPolicy`'s field names (`token_ceiling`, `auto_evict_cold`, `compact_threshold`) match P3's definition; the struct is `Copy + Default`.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid subagent`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "feat(zoid): subagent_policy — token-bounded, cold-evicting context budget"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `build_subagent_request` includes relevant **file content** + the task, and **excludes** session messages/tool transcripts (grep the test assertion).
- [ ] `file_contents` keys match `context_window`'s `"file:{path}"` form (a `ContextItem.key` resolves).
- [ ] Selection reuses `assemble_context` (no reimplemented pin/evict/ceiling logic).

## Self-Review notes (author)

- **Spec coverage (§4.4 context construction):** `build_subagent_request` is the "orchestrator assembles exactly what a task needs (plan task + relevant code, never session history)" mechanism — it reuses ⑤'s `context_window` + `assemble_context` (the glass-box machinery P3 built) and resolves File content via `file_contents`. The token ceiling (`subagent_policy`, T3) keeps it precise.
- **Type consistency:** `file_contents(events) -> HashMap<String,String>` keyed `"file:{path}"` (T1) is consumed by `build_subagent_request` (T2) via `ContextItem.key`. `build_subagent_request(task, events, policy, model, tools) -> CompletionRequest` (T2) is the signature P5c's executor will call with `subagent_policy()` (T3). `tool_specs` is reused from `agent.rs` (DRY).
- **Deliberate exclusion:** only `ItemKind::File` items are inlined; Message/ToolResult items are session history and excluded by design — the test asserts the chat text is absent. This is the spec's hard line, not an oversight.
- **Next:** P5c seeds this request as the first turn of an isolated agent loop (its own branch + cwd) and returns a `SubagentResult`.
