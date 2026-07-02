# Chat-Mode UX & Capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make zoid's Chat-mode agent loop visible, testable, and interactive — in-flight tool indicator, a scriptable test crate, an event-sourced task rail widget, and a model-driven `ask_user` question flow — plus wrap-around list navigation, while leaving clean seams for tool approval and dynamic tool registration.

**Architecture:** Everything but the nav fix rides the existing tool loop (`crates/zoid/src/agent.rs`). A new `ToolKind { Local, Emitting, Interactive }` discriminant routes tool execution in `run_turn_inner`: `Local` runs synchronously via `run_tool` (today's behavior), `Emitting` (`update_tasks`) appends a domain event, `Interactive` (`ask_user`) parks the loop on a `oneshot` while the UI collects an answer. New domain state (`EventKind::Tasks`) folds through a `tasks()` projection into a rail drawer. A dev-only `zoid-testkit` crate exposes a scripted provider + event-log assertions.

**Tech Stack:** Rust 2021 cargo workspace; ratatui 0.29 + crossterm 0.28 + tokio 1 TUI; event-sourced core (`zoid-core`); provider seam (`zoid-provider`, Ollama native `/api/chat`); tools (`zoid-tools`); `insta` snapshots; `serde`/`serde_json`.

## Global Constraints

- **§16 token purity:** No literal glyphs or hex color values in rendered UI code outside `crates/zoid-tui/src/tokens.rs`. Comments and tests are exempt. New display glyphs are added as named tokens and referenced by name. Middle-dot `·` is ordinary punctuation and may be used inline.
- **Event log is faithful:** The core event layer records what the model emitted, verbatim. Validation/enforcement/policy live in the tool layer or prompt text, never by mutating or rejecting recorded truth. `EventKind` must stay `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`.
- **Provider:** Ollama Cloud native `/api/chat` (NDJSON, Bearer `$OLLAMA_API_KEY`, default `glm-5.2:cloud`). Tool calling uses OpenAI/Ollama `tools` + `tool_calls` shape, never Anthropic `tool_use`.
- **Secrets** never land in committed files.
- **Commits:** no `Co-Authored-By`/co-author trailer.
- **Tests:** every task ends green; no test asserts nothing; no `insta` snapshot is committed without inspecting the `.new` first (`cargo insta review` or manual read, then accept).
- **Toolchain:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all` all clean before each commit.

---

## File Structure

**New files:**
- `crates/zoid-testkit/Cargo.toml`, `crates/zoid-testkit/src/lib.rs` — dev-facing scripted provider + event-log assertions (Phase 2).
- `crates/zoid-core/src/tasks.rs` — `TaskItem`, `TaskStatus`, `parse_task_items`, `tasks()` projection (Phase 3).
- `crates/zoid-tools/src/tasks.rs` — `UpdateTasks` emitting tool (Phase 3).
- `crates/zoid-tools/src/ask.rs` — `AskUser` interactive tool (Phase 4).
- `crates/zoid-tui/src/question.rs` — `QuestionState`, `QuestionMode`, `route_question_key`, `render_question` (Phase 4).

**Modified files:**
- `crates/zoid-tui/src/palette.rs` — `nav()` clamp → wrap (Phase 1).
- `crates/zoid-tools/src/lib.rs` — `ToolKind` enum, `Tool::kind()` default, `ToolGate`/`AllowAll`, register new tools (Phases 2–4).
- `crates/zoid/src/agent.rs` — `AgentUpdate::{ToolStarted, AskUser}`, `Answer`, gate param, `match tool.kind()` routing (Phases 2–4).
- `crates/zoid/src/main.rs` — `active_tool` set/clear, gate wiring, `AskUser` overlay wiring, question actions (Phases 2–4).
- `crates/zoid/src/subagent.rs` — pass gate; filter `Interactive` tools (Phases 2, 4).
- `crates/zoid-core/src/event.rs` — `EventKind::Tasks` (Phase 3); `crates/zoid-core/src/lib.rs` — `pub mod tasks`.
- `crates/zoid-core/src/projection.rs` — nothing (tasks projection lives in `tasks.rs`); `crates/zoid-tui/src/state.rs` — `active_tool`, `DrawerId::Tasks`, drawer, `Overlay::Question`, question state, `Action` variants.
- `crates/zoid-tui/src/layout.rs` — `TASKS_BODY_ROWS`, `drawer_body_rows` arm.
- `crates/zoid-tui/src/render.rs` — spinner line, `render_tasks_body`, `render_rail` arm, question overlay dispatch.
- `crates/zoid-tui/src/route.rs` — `Overlay::Question` dispatch.
- `crates/zoid-tui/src/tokens.rs` — reuse existing `RUNNING`/`PENDING`/`PASS`; no new glyph needed unless noted.
- `crates/zoid-tui/src/lib.rs` — `pub mod question;` re-export.
- `Cargo.toml` (root) — add `crates/zoid-testkit` to `members`.

---

## Phase 1 — Wrap-around list navigation (④)

### Task 1: Wrap `nav()` at both ends

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (the `nav` fn at lines ~171–178 and the `nav_clamps` test at ~214–219)

**Interfaces:**
- Produces: `pub fn nav(selected: usize, delta: i32, len: usize) -> usize` — signature UNCHANGED; behavior changes from clamp to wrap. Used by `main.rs` for palette/objects/verbs/sessions selection movement and by `render.rs:443`.

- [ ] **Step 1: Replace the `nav_clamps` test with a wrapping test**

In `crates/zoid-tui/src/palette.rs`, replace the whole `nav_clamps` test with:

```rust
    #[test]
    fn nav_wraps() {
        // Down past the last row wraps to the top; up from the top wraps to the last.
        assert_eq!(nav(2, 1, 3), 0);
        assert_eq!(nav(0, -1, 3), 2);
        // Interior moves are unchanged.
        assert_eq!(nav(1, 1, 3), 2);
        assert_eq!(nav(1, -1, 3), 0);
        // Empty list is a no-op (no panic, no divide-by-zero).
        assert_eq!(nav(0, 1, 0), 0);
        // A multi-step delta still lands in range.
        assert_eq!(nav(0, 5, 3), 2);
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p zoid-tui nav_wraps`
Expected: FAIL — `nav(2,1,3)` returns `2` (clamped), not `0`.

- [ ] **Step 3: Rewrite `nav` to wrap**

Replace the `nav` function body:

```rust
/// Move a selection index by `delta`, wrapping at both ends (opencode-style):
/// stepping past the last row lands on the first, and up from the first lands
/// on the last. Returns 0 for an empty list. `len` is the row count.
pub fn nav(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len_i = len as i64;
    let next = selected as i64 + delta as i64;
    next.rem_euclid(len_i) as usize
}
```

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p zoid-tui nav_wraps`
Expected: PASS.

- [ ] **Step 5: Run the wider suite for regressions**

Run: `cargo test -p zoid-tui`
Expected: PASS. If a config-section/field or sessions test asserted clamping-at-ends, update its expectation to the wrapped value (there should be none; `nav`'s callers pass through selection indices only).

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy -p zoid-tui --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(tui): wrap list selection at both ends (P1 ④)"
```

---

## Phase 2 — Tool visibility, seams & testkit (①)

### Task 2: `ToolKind` seam on the `Tool` trait

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs`

**Interfaces:**
- Produces: `pub enum ToolKind { Local, Emitting, Interactive }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `Tool::kind(&self) -> ToolKind` with a default returning `ToolKind::Local`. Consumed by the agent loop (Task 7, 9) and subagent filtering (Task 9).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-tools/src/lib.rs`:

```rust
    #[test]
    fn registry_tools_are_all_local_by_default() {
        for t in registry() {
            assert_eq!(t.kind(), ToolKind::Local, "{} should default to Local", t.name());
        }
    }
```

- [ ] **Step 2: Run it and watch it fail to compile**

Run: `cargo test -p zoid-tools registry_tools_are_all_local_by_default`
Expected: FAIL — `ToolKind` and `kind()` do not exist.

- [ ] **Step 3: Add the enum and the defaulted trait method**

In `crates/zoid-tools/src/lib.rs`, above `pub trait Tool`:

```rust
/// How the agent loop must execute a tool. `Local` tools run synchronously in
/// the working directory (the v1 default). `Emitting` tools append a domain
/// event instead of doing I/O. `Interactive` tools suspend the loop to collect
/// input from the UI. The loop branches on this BEFORE calling `run()`, so only
/// `Local` tools ever have `run()` invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Local,
    Emitting,
    Interactive,
}
```

Add to `trait Tool`, after `fn run(...)`:

```rust
    /// The execution kind (see [`ToolKind`]). Defaults to `Local`; the five
    /// built-in tools do not override it.
    fn kind(&self) -> ToolKind {
        ToolKind::Local
    }
```

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p zoid-tools`
Expected: PASS (new test + existing tool tests).

- [ ] **Step 5: Lint, format, commit**

```bash
cargo clippy -p zoid-tools --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tools/src/lib.rs
git commit -m "feat(tools): add ToolKind execution-kind seam (P2 ①)"
```

### Task 3: `ToolGate` seam (always-allow) wired into the loop

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs` (trait + `AllowAll`)
- Modify: `crates/zoid/src/agent.rs` (gate param + pre-dispatch check)
- Modify: `crates/zoid/src/main.rs` (`spawn_turn` passes `AllowAll`)
- Modify: `crates/zoid/src/subagent.rs` (pass `AllowAll`)
- Test: `crates/zoid/tests/agent_loop.rs` (deny path)

**Interfaces:**
- Produces: `pub enum Gate { Allow, Deny(String) }`; `pub trait ToolGate: Send + Sync { fn check(&self, call: &zoid_provider::ToolCall) -> Gate; }`; `pub struct AllowAll;` implementing it. `run_agent_turn` and `run_turn_inner` gain a parameter `gate: std::sync::Arc<dyn zoid_tools::ToolGate>` inserted immediately after `tools`.
- Consumes: `zoid_provider::ToolCall` (already a dependency of `zoid-tools`).

- [ ] **Step 1: Add the gate trait + AllowAll with a unit test**

In `crates/zoid-tools/src/lib.rs`, after the `ToolKind` enum:

```rust
use zoid_provider::ToolCall;

/// The decision a [`ToolGate`] returns for a pending tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    /// Block the call; the string is fed back to the model as the tool result.
    Deny(String),
}

/// Consulted once per pending tool call, immediately before dispatch. v1 ships
/// only [`AllowAll`]; this is the insertion point where interactive tool
/// approval will later live (an `ask_user` prompt gating `Deny`).
pub trait ToolGate: Send + Sync {
    fn check(&self, call: &ToolCall) -> Gate;
}

/// The v1 gate: every tool call is allowed.
pub struct AllowAll;
impl ToolGate for AllowAll {
    fn check(&self, _call: &ToolCall) -> Gate {
        Gate::Allow
    }
}
```

Add to the `tests` module:

```rust
    #[test]
    fn allow_all_allows_every_call() {
        let g = AllowAll;
        let call = zoid_provider::ToolCall { id: String::new(), name: "shell".into(), args: json!({}) };
        assert_eq!(g.check(&call), Gate::Allow);
    }
```

Run: `cargo test -p zoid-tools allow_all_allows_every_call` → PASS.

- [ ] **Step 2: Write the failing loop-level deny test**

In `crates/zoid/tests/agent_loop.rs`, add a `DenyAll` gate and a test. First add near the top (after imports):

```rust
struct DenyAll;
impl zoid_tools::ToolGate for DenyAll {
    fn check(&self, _c: &zoid_provider::ToolCall) -> zoid_tools::Gate {
        zoid_tools::Gate::Deny("denied by policy".into())
    }
}
```

Then a test modeled on the existing `agent_loop_runs_tool_then_finishes` (reuse its scripted `write_file` provider setup), but pass `Arc::new(DenyAll)` as the gate and assert the file was NOT written and the log carries a `ToolResult` whose `output` contains `denied by policy` and `is_error == true`:

```rust
#[tokio::test]
async fn gate_deny_blocks_tool_and_feeds_reason_back() {
    // ... identical scripted-provider + session setup to
    // agent_loop_runs_tool_then_finishes, writing to `target` path ...
    let events = zoid::agent::run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        std::sync::Arc::new(DenyAll),   // <-- gate param (new, after tools)
        session,
        seed,
        model,
        ui,
        session_id,
        || 0,
    )
    .await
    .unwrap();

    assert!(!target.exists(), "denied write_file must not touch the filesystem");
    let denied = events.iter().any(|e| matches!(&e.kind,
        zoid_core::event::EventKind::ToolResult { output, is_error, .. }
            if *is_error && output.contains("denied by policy")));
    assert!(denied, "a Deny must surface as an error ToolResult");
}
```

Run: `cargo test -p zoid gate_deny_blocks_tool_and_feeds_reason_back`
Expected: FAIL to compile — `run_agent_turn` has no gate parameter yet.

- [ ] **Step 3: Thread the gate through the loop**

In `crates/zoid/src/agent.rs`:

1. Import: add `use zoid_tools::{Tool, ToolGate, Gate};` (extend the existing `use zoid_tools::Tool;`).
2. Add the parameter to BOTH `run_agent_turn` and `run_turn_inner`, immediately after `tools: Arc<Vec<Box<dyn Tool>>>,`:

```rust
    gate: Arc<dyn ToolGate>,
```

3. In `run_agent_turn`, pass `gate` into the `run_turn_inner(...)` call (add `gate,` right after `tools,`).
4. In `run_turn_inner`, in the tool-execution `for tc in pending` loop, check the gate BEFORE `spawn_blocking`:

```rust
        for tc in pending {
            if let Gate::Deny(reason) = gate.check(&tc) {
                emit(
                    &session, &mut events, ui, &config.branch,
                    EventKind::ToolResult {
                        id: tc.id, name: tc.name,
                        output: reason, is_error: true,
                    },
                    session_id, now,
                )
                .await?;
                continue;
            }
            let tools_for_exec = tools.clone();
            // ... unchanged spawn_blocking + ToolResult emit ...
        }
```

- [ ] **Step 4: Fix the other callers**

In `crates/zoid/src/main.rs` `spawn_turn` (~1537), add `std::sync::Arc::new(zoid_tools::AllowAll),` as the argument after `tools,`.

In `crates/zoid/src/subagent.rs`, find the `run_agent_turn(...)` call and insert `std::sync::Arc::new(zoid_tools::AllowAll),` after its `tools,` argument. Also update any `run_agent_turn` calls in `subagent.rs` tests the same way.

Grep to be exhaustive: `grep -rn "run_agent_turn(" crates/zoid/` — every call site gets the gate argument (existing `agent_loop.rs` happy-path test included: pass `std::sync::Arc::new(zoid_tools::AllowAll)`).

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid` then `cargo test --workspace`
Expected: PASS — the deny test and all migrated call sites green.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tools/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/src/subagent.rs crates/zoid/tests/agent_loop.rs
git commit -m "feat(agent): ToolGate seam (AllowAll) checked before dispatch (P2 ①)"
```

### Task 4: In-flight tool indicator

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`AgentUpdate::ToolStarted`, emit before dispatch)
- Modify: `crates/zoid-tui/src/state.rs` (`active_tool` field)
- Modify: `crates/zoid/src/main.rs` (`ui_rx` set/clear)
- Modify: `crates/zoid-tui/src/render.rs` (spinner line)
- Test: `crates/zoid-tui/src/state.rs` unit test

**Interfaces:**
- Produces: `AgentUpdate::ToolStarted { name: String }`; `ShellState.active_tool: Option<String>` with helpers `set_active_tool(&mut self, name)` / `clear_active_tool(&mut self)`. Rendered using `tokens::glyph::RUNNING`.

- [ ] **Step 1: Add the `AgentUpdate` variant and emit it**

In `crates/zoid/src/agent.rs`, extend `AgentUpdate`:

```rust
pub enum AgentUpdate {
    Appended(Box<Event>),
    /// A Local tool is about to run; the UI shows an in-flight spinner until the
    /// matching `ToolResult` is appended (or the turn completes).
    ToolStarted { name: String },
    TurnComplete,
}
```

In `run_turn_inner`, inside `for tc in pending`, AFTER the gate check passes and BEFORE `spawn_blocking`:

```rust
            let _ = ui.send(AgentUpdate::ToolStarted { name: tc.name.clone() }).await;
```

- [ ] **Step 2: Add `active_tool` state + a failing unit test**

In `crates/zoid-tui/src/state.rs`, add to `ShellState` (near `overlay`):

```rust
    /// Name of the tool currently executing (in-flight indicator), or `None`.
    pub active_tool: Option<String>,
```

Initialize it in `ShellState::new` (in the `Self { ... }` literal): `active_tool: None,`.

Add helpers in `impl ShellState`:

```rust
    pub fn set_active_tool(&mut self, name: impl Into<String>) {
        self.active_tool = Some(name.into());
    }
    pub fn clear_active_tool(&mut self) {
        self.active_tool = None;
    }
```

Add a test in the `state.rs` tests module:

```rust
    #[test]
    fn active_tool_sets_and_clears() {
        let mut s = ShellState::new();
        assert_eq!(s.active_tool, None);
        s.set_active_tool("shell");
        assert_eq!(s.active_tool.as_deref(), Some("shell"));
        s.clear_active_tool();
        assert_eq!(s.active_tool, None);
    }
```

Run: `cargo test -p zoid-tui active_tool_sets_and_clears` → PASS (compiles once the field exists).

- [ ] **Step 3: Set/clear in the `ui_rx` loop**

In `crates/zoid/src/main.rs`, in the `Some(update) = ui_rx.recv()` match (~756):

```rust
                    AgentUpdate::Appended(ev) => {
                        if matches!(ev.kind, EventKind::DelegationResult { .. }) {
                            app.delegating = false;
                            app.shell.status_hint = None;
                        }
                        // A tool result ends the in-flight indicator for that tool.
                        if matches!(ev.kind, EventKind::ToolResult { .. }) {
                            app.shell.clear_active_tool();
                        }
                        app.events.push(*ev);
                    }
                    AgentUpdate::ToolStarted { name } => {
                        app.shell.set_active_tool(name);
                    }
                    AgentUpdate::TurnComplete => {
                        app.streaming = false;
                        app.shell.clear_active_tool();
                    }
```

- [ ] **Step 4: Render the spinner line**

In `crates/zoid-tui/src/render.rs`, in the conversation/stream rendering path (where the streaming caret / tail is drawn), when `state.active_tool` is `Some(name)`, append a dim line built from the token glyph. Add near the other stream-tail rendering:

```rust
    if let Some(name) = &state.active_tool {
        // §16: glyph comes from tokens, not a literal.
        let line = Line::from(vec![
            Span::styled(
                format!("{} ", crate::tokens::glyph::RUNNING),
                Style::default().fg(crate::tokens::color::WARN),
            ),
            Span::styled(
                format!("running · {name} …"),
                Style::default().fg(crate::tokens::color::DIM),
            ),
        ]);
        // push `line` into the same Vec<Line> the conversation body renders.
    }
```

Wire `line` into whichever `Vec<Line>` the chat body pushes to (follow the surrounding code's variable, e.g. `body.push(line);`). Keep it below the last message and above the input.

- [ ] **Step 5: Snapshot the indicator (optional but preferred)**

If a shell snapshot fixture is convenient, add one: build a `ShellState` with `active_tool = Some("shell".into())`, render at 100×24, `insta::assert_snapshot!`. Inspect the `.new` and confirm the `◐ running · shell …` line appears, then accept.

Run: `cargo test -p zoid-tui` and `cargo test -p zoid`
Expected: PASS.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid/src/agent.rs crates/zoid-tui/src/state.rs crates/zoid/src/main.rs crates/zoid-tui/src/render.rs
git add crates/zoid-tui/tests/snapshots 2>/dev/null || true
git commit -m "feat(tui): in-flight tool spinner via AgentUpdate::ToolStarted (P2 ①)"
```

### Task 5: `zoid-testkit` crate (scripted provider + event assertions)

**Files:**
- Create: `crates/zoid-testkit/Cargo.toml`
- Create: `crates/zoid-testkit/src/lib.rs`
- Modify: `Cargo.toml` (root) — add member

**Interfaces:**
- Produces:
  - `pub use zoid_provider::FakeProvider as ScriptedProvider;` plus builder helpers `pub fn script(events: Vec<ProviderEvent>) -> Arc<dyn Provider>` and `pub fn tool_call(name: &str, args: serde_json::Value) -> ProviderEvent` and `pub fn text(s: &str) -> ProviderEvent`.
  - Assertions over a folded log: `pub fn tool_results(events: &[Event]) -> Vec<(String, String, bool)>` returning `(name, output, is_error)` per `ToolResult`, and `pub fn assert_no_tool_errors(events: &[Event])`.
- Consumes: `zoid-core` (`Event`, `EventKind`), `zoid-provider` (`FakeProvider`, `ProviderEvent`, `ToolCall`, `Provider`). **Depends downward only on `zoid-core` + `zoid-provider`** — never `zoid-tools` or `zoid` (keeps the graph acyclic and the kit reusable by any core+provider agent).

- [ ] **Step 1: Create the crate manifest**

`crates/zoid-testkit/Cargo.toml`:

```toml
[package]
name = "zoid-testkit"
version = "0.1.0"
edition.workspace = true

[dependencies]
zoid-core = { path = "../zoid-core" }
zoid-provider = { path = "../zoid-provider" }
serde_json = { workspace = true }

[dev-dependencies]
ulid = { workspace = true }
```

Add `"crates/zoid-testkit"` to the `members` array in the root `Cargo.toml`.

- [ ] **Step 2: Write the crate with a doctest**

`crates/zoid-testkit/src/lib.rs`:

```rust
//! Test harness for zoid agents built on `zoid-core` + `zoid-provider`.
//!
//! Drive an agent loop with a scripted model instead of a live provider, then
//! assert on the resulting event log. Depends only on `zoid-core` and
//! `zoid-provider`, so it works for any agent built on that seam.
//!
//! ```
//! use zoid_testkit::{script, text, tool_call};
//! use serde_json::json;
//! let provider = script(vec![
//!     tool_call("write_file", json!({"path": "a.txt", "content": "hi"})),
//!     text("done"),
//! ]);
//! // hand `provider` to your run_agent_turn; then inspect the log.
//! # let _ = provider;
//! ```

use std::sync::Arc;
use zoid_core::event::{Event, EventKind};
use zoid_provider::{FakeProvider, Provider, ProviderEvent, ToolCall};

pub use zoid_provider::FakeProvider as ScriptedProvider;

/// A model text chunk.
pub fn text(s: &str) -> ProviderEvent {
    ProviderEvent::TextDelta(s.to_string())
}

/// A tool call with an empty id (Ollama-native shape) and parsed args.
pub fn tool_call(name: &str, args: serde_json::Value) -> ProviderEvent {
    ProviderEvent::ToolCall(ToolCall { id: String::new(), name: name.to_string(), args })
}

/// Build a scripted provider from an ordered event list.
pub fn script(events: Vec<ProviderEvent>) -> Arc<dyn Provider> {
    Arc::new(FakeProvider::new(events))
}

/// Extract `(name, output, is_error)` for every `ToolResult` in the log.
pub fn tool_results(events: &[Event]) -> Vec<(String, String, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult { name, output, is_error, .. } => {
                Some((name.clone(), output.clone(), *is_error))
            }
            _ => None,
        })
        .collect()
}

/// Panic if any tool result is an error.
pub fn assert_no_tool_errors(events: &[Event]) {
    for (name, output, is_error) in tool_results(events) {
        assert!(!is_error, "tool `{name}` errored: {output}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn script_builds_a_provider_and_helpers_shape_events() {
        let _p = script(vec![tool_call("search", json!({"q": "x"})), text("ok")]);
        match tool_call("search", json!({"q": "x"})) {
            ProviderEvent::ToolCall(tc) => {
                assert_eq!(tc.name, "search");
                assert_eq!(tc.id, "");
            }
            _ => panic!("expected a ToolCall"),
        }
    }

    #[test]
    fn tool_results_filters_and_flags_errors() {
        // Hand-build a log with one ok result and one error result.
        let mk = |name: &str, err: bool| {
            let kind = EventKind::ToolResult {
                id: String::new(), name: name.into(),
                output: if err { "boom".into() } else { "fine".into() },
                is_error: err,
            };
            Event::new(ulid::Ulid::nil(), None, 0, kind)
        };
        let log = vec![mk("read_file", false), mk("shell", true)];
        let got = tool_results(&log);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1], ("shell".to_string(), "boom".to_string(), true));
    }
}
```

> Note: confirm `Event::new(id, parent, ts, kind)`'s exact signature in `crates/zoid-core/src/event.rs` and adjust the `mk` helper if the argument order differs; `ulid` is already a workspace dep of `zoid-core` and is re-exported through it — if not accessible, add `ulid = { workspace = true }` to the testkit `[dev-dependencies]`.

- [ ] **Step 3: Run the kit's own tests + doctest**

Run: `cargo test -p zoid-testkit`
Expected: PASS (unit tests + the doctest).

- [ ] **Step 4: Prove parity by migrating the existing scripted setup**

In `crates/zoid/Cargo.toml` add `zoid-testkit = { path = "../zoid-testkit" }` under `[dev-dependencies]`. In `crates/zoid/tests/agent_loop.rs`, replace the hand-rolled `FakeProvider::new(vec![...])` construction in `agent_loop_runs_tool_then_finishes` with `zoid_testkit::script(vec![ zoid_testkit::tool_call("write_file", json!({...})), zoid_testkit::text("done"), ProviderEvent::Done ])` (keep the terminal `Done`; the loop also self-sends one, so a trailing `Done` is harmless). Keep every existing assertion.

Run: `cargo test -p zoid agent_loop_runs_tool_then_finishes`
Expected: PASS — behavior identical, now sourced from the kit.

- [ ] **Step 5: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add Cargo.toml crates/zoid-testkit crates/zoid/Cargo.toml crates/zoid/tests/agent_loop.rs
git commit -m "feat(testkit): add zoid-testkit scripted provider + log assertions (P2 ①)"
```

---

## Phase 3 — Task rail widget (③)

### Task 6: Core task model + `tasks()` projection

**Files:**
- Create: `crates/zoid-core/src/tasks.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod tasks;`)
- Modify: `crates/zoid-core/src/event.rs` (`EventKind::Tasks`)

**Interfaces:**
- Produces: `pub enum TaskStatus { Pending, Active, Done }` (`Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize`); `pub struct TaskItem { pub text: String, pub status: TaskStatus }` (same derives minus `Copy`); `EventKind::Tasks { items: Vec<TaskItem> }`; `pub fn parse_task_items(args: &serde_json::Value) -> Result<Vec<TaskItem>, String>`; `pub fn tasks(events: &[Event]) -> Vec<TaskItem>`.
- Consumes: `crate::event::{Event, EventKind}`.

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-core/src/tasks.rs`:

```rust
//! The model's live task list: a full-snapshot event (`EventKind::Tasks`) and
//! the `tasks()` projection that returns the latest snapshot. The event layer
//! is faithful — cardinality (e.g. "one Active") is NOT enforced here.

use serde::{Deserialize, Serialize};
use crate::event::{Event, EventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub text: String,
    pub status: TaskStatus,
}

/// Parse the `update_tasks` argument object into task items. Faithful: accepts
/// any well-formed list; the only errors are structural (missing `tasks`, wrong
/// types, unknown status string).
pub fn parse_task_items(args: &serde_json::Value) -> Result<Vec<TaskItem>, String> {
    let arr = args
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing or non-array `tasks`".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, it) in arr.iter().enumerate() {
        let text = it
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("task[{i}]: missing or non-string `text`"))?
            .to_string();
        let status = match it.get("status").and_then(|v| v.as_str()) {
            Some("pending") => TaskStatus::Pending,
            Some("active") => TaskStatus::Active,
            Some("done") => TaskStatus::Done,
            other => return Err(format!("task[{i}]: bad status {other:?}")),
        };
        out.push(TaskItem { text, status });
    }
    Ok(out)
}

/// The latest task snapshot (last-write-wins), or empty if none was published.
/// Ignores subagent branches, matching the conversation projection.
pub fn tasks(events: &[Event]) -> Vec<TaskItem> {
    events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Tasks { items } => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind};
    use serde_json::json;

    fn tasks_event(items: Vec<TaskItem>) -> Event {
        Event::new(ulid::Ulid::new(), None, 0, EventKind::Tasks { items })
    }

    #[test]
    fn parse_reads_text_and_status() {
        let got = parse_task_items(&json!({"tasks": [
            {"text": "read spec", "status": "done"},
            {"text": "write code", "status": "active"},
            {"text": "test", "status": "pending"},
        ]}))
        .unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].status, TaskStatus::Done);
        assert_eq!(got[1].text, "write code");
    }

    #[test]
    fn parse_rejects_bad_status_and_shape() {
        assert!(parse_task_items(&json!({"tasks": [{"text": "x", "status": "nope"}]})).is_err());
        assert!(parse_task_items(&json!({"nope": []})).is_err());
    }

    #[test]
    fn tasks_returns_latest_snapshot_last_write_wins() {
        let e1 = tasks_event(vec![TaskItem { text: "a".into(), status: TaskStatus::Active }]);
        let e2 = tasks_event(vec![
            TaskItem { text: "a".into(), status: TaskStatus::Done },
            TaskItem { text: "b".into(), status: TaskStatus::Active },
        ]);
        let got = tasks(&[e1, e2]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].status, TaskStatus::Done);
    }

    #[test]
    fn tasks_empty_when_none_published() {
        assert!(tasks(&[]).is_empty());
    }

    #[test]
    fn tasks_event_round_trips_through_serde() {
        let ev = tasks_event(vec![TaskItem { text: "x".into(), status: TaskStatus::Pending }]);
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
```

- [ ] **Step 2: Run — expect a compile failure**

Run: `cargo test -p zoid-core tasks::`
Expected: FAIL — `EventKind::Tasks` does not exist and `mod tasks` is not declared.

- [ ] **Step 3: Add the variant and module**

In `crates/zoid-core/src/event.rs`, add to `EventKind` (after `DelegationResult`):

```rust
    /// The model's full task-list snapshot (last-write-wins). Rendered in the
    /// rail; never inlined into the conversation transcript. Faithful — no
    /// cardinality rules enforced here.
    Tasks {
        items: Vec<crate::tasks::TaskItem>,
    },
```

In `crates/zoid-core/src/lib.rs`, add `pub mod tasks;` alongside the other `pub mod` declarations.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zoid-core tasks::`
Expected: PASS (all five tests).

- [ ] **Step 5: Confirm no projection regressions**

The `conversation()` projection matches specific `EventKind` variants and ignores the rest; `Tasks` should fall through untouched. Verify:

Run: `cargo test -p zoid-core`
Expected: PASS. If a `match` on `EventKind` is non-exhaustive (compiler error `E0004`), add a `EventKind::Tasks { .. } => {}` (or the projection's neutral arm) wherever the compiler points.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy -p zoid-core --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-core/src/tasks.rs crates/zoid-core/src/lib.rs crates/zoid-core/src/event.rs
git commit -m "feat(core): EventKind::Tasks + tasks() snapshot projection (P3 ③)"
```

### Task 7: `update_tasks` emitting tool + loop routing

**Files:**
- Create: `crates/zoid-tools/src/tasks.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (module + registry + export)
- Modify: `crates/zoid/src/agent.rs` (`match tool.kind()` routing; append `Tasks` for `update_tasks`)
- Test: `crates/zoid/tests/tasks_tool.rs` (integration via testkit)

**Interfaces:**
- Produces: `pub struct UpdateTasks;` implementing `Tool` with `name() == "update_tasks"`, `kind() == ToolKind::Emitting`, a `spec()` whose JSON Schema is `{ tasks: [{ text: string, status: "pending"|"active"|"done" }] }` and whose description carries the "at most one Active" guidance. Registered in `registry()`.
- Consumes: `zoid_core::tasks::parse_task_items`, `EventKind::Tasks` (loop side).

- [ ] **Step 1: Add `zoid-core` as a dependency of `zoid-tools`**

`zoid-tools` needs `EventKind`/`TaskItem` names only indirectly; the tool itself only builds a `ToolSpec` and a defensive `run()`. To keep `zoid-tools` free of a `zoid-core` dependency, the tool does NOT parse into core types — the loop does. So **no new dependency**: `UpdateTasks::run` is never called (the loop handles `Emitting`), and its `spec()` is pure JSON.

- [ ] **Step 2: Write the tool with a spec test**

Create `crates/zoid-tools/src/tasks.rs`:

```rust
//! `update_tasks` — an Emitting tool. The agent loop intercepts it (by kind)
//! and appends an `EventKind::Tasks` snapshot; `run()` is a defensive no-op that
//! is never called on the happy path.

use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;
use crate::{Tool, ToolKind, ToolOutput};

pub struct UpdateTasks;

impl Tool for UpdateTasks {
    fn name(&self) -> &str {
        "update_tasks"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "update_tasks".into(),
            description: "Publish your current task list to the user's rail. Send the FULL list \
                every time (it replaces the previous one). Keep at most one task 'active' at a \
                time. Statuses: pending, active, done."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "active", "done"] }
                            },
                            "required": ["text", "status"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // The loop handles Emitting tools; run() is never reached on the happy
        // path. Return an error if somehow dispatched directly.
        ToolOutput::err("update_tasks must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_update_tasks_schema() {
        let s = UpdateTasks.spec();
        assert_eq!(s.name, "update_tasks");
        assert_eq!(UpdateTasks.kind(), ToolKind::Emitting);
        assert!(s.parameters["properties"]["tasks"].is_object());
    }
}
```

In `crates/zoid-tools/src/lib.rs`: add `pub mod tasks;`, add `Box::new(tasks::UpdateTasks)` to the `registry()` vec, and re-export if the file re-exports tools.

- [ ] **Step 3: Run the tool's unit test**

Run: `cargo test -p zoid-tools spec_advertises_update_tasks_schema`
Expected: PASS. Also re-run `registry_has_unique_named_tools` — it now also sees `update_tasks`; add `assert!(names.contains(&"update_tasks"));` to it.

- [ ] **Step 4: Write the failing integration test**

Create `crates/zoid/tests/tasks_tool.rs`, modeled on `agent_loop.rs`'s setup (scripted provider, real `registry()`, temp session). Script a single `update_tasks` call then `text("ok")`:

```rust
// imports + a session/setup helper mirroring agent_loop.rs
#[tokio::test]
async fn update_tasks_appends_a_tasks_event_and_acks() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("update_tasks", serde_json::json!({"tasks": [
            {"text": "step one", "status": "active"},
            {"text": "step two", "status": "pending"},
        ]})),
        zoid_testkit::text("ok"),
        zoid_provider::ProviderEvent::Done,
    ]);
    // ... build tools = Arc::new(zoid_tools::registry()), session, seed with a
    //     UserMessage, ui channel, etc., exactly as agent_loop.rs does ...
    let events = zoid::agent::run_agent_turn(
        zoid::agent::chat_turn_config(), provider, tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session, seed, model, ui, session_id, || 0,
    ).await.unwrap();

    // A Tasks event was appended with the two items, faithfully.
    let snapshot = zoid_core::tasks::tasks(&events);
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].status, zoid_core::tasks::TaskStatus::Active);

    // And a non-error ack ToolResult was fed back.
    let acks = zoid_testkit::tool_results(&events);
    assert!(acks.iter().any(|(n, out, err)| n == "update_tasks" && !err && out.contains("task")));
}
```

Run: `cargo test -p zoid update_tasks_appends_a_tasks_event_and_acks`
Expected: FAIL — the loop still routes `update_tasks` through `run_tool`, which returns the defensive error and appends no `Tasks` event.

- [ ] **Step 5: Route by kind in the loop**

In `crates/zoid/src/agent.rs`, replace the `for tc in pending { ... }` body (after the gate check) with a match on the tool's kind. Look up the tool by name to read its kind:

```rust
        for tc in pending {
            if let Gate::Deny(reason) = gate.check(&tc) {
                emit(&session, &mut events, ui, &config.branch,
                    EventKind::ToolResult { id: tc.id, name: tc.name, output: reason, is_error: true },
                    session_id, now).await?;
                continue;
            }

            let kind = tools.iter().find(|t| t.name() == tc.name).map(|t| t.kind());

            match kind {
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "update_tasks" => {
                    match zoid_core::tasks::parse_task_items(&tc.args) {
                        Ok(items) => {
                            let n = items.len();
                            let active = items.iter()
                                .filter(|i| i.status == zoid_core::tasks::TaskStatus::Active).count();
                            emit(&session, &mut events, ui, &config.branch,
                                EventKind::Tasks { items }, session_id, now).await?;
                            emit(&session, &mut events, ui, &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id, name: tc.name,
                                    output: format!("{n} tasks · {active} active"), is_error: false,
                                },
                                session_id, now).await?;
                        }
                        Err(msg) => {
                            emit(&session, &mut events, ui, &config.branch,
                                EventKind::ToolResult { id: tc.id, name: tc.name, output: msg, is_error: true },
                                session_id, now).await?;
                        }
                    }
                }
                _ => {
                    // Local tools (the default): run in the working directory.
                    let _ = ui.send(AgentUpdate::ToolStarted { name: tc.name.clone() }).await;
                    let tools_for_exec = tools.clone();
                    let name = tc.name.clone();
                    let args = tc.args.clone();
                    let cwd = cwd_for_exec.clone();
                    let out = tokio::task::spawn_blocking(move || {
                        zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd)
                    }).await?;
                    emit(&session, &mut events, ui, &config.branch,
                        EventKind::ToolResult { id: tc.id, name: tc.name, output: out.text, is_error: out.is_error },
                        session_id, now).await?;
                }
            }
        }
```

> The `ToolStarted` emit moves INTO the `Local` arm (Emitting/Interactive tools resolve instantly, so no spinner). Adjust Task 4's placement accordingly if implementing in order — the net result is the code above.

- [ ] **Step 6: Run the integration test + workspace**

Run: `cargo test -p zoid update_tasks_appends_a_tasks_event_and_acks` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tools/src/tasks.rs crates/zoid-tools/src/lib.rs crates/zoid/src/agent.rs crates/zoid/tests/tasks_tool.rs
git commit -m "feat(agent): update_tasks emitting tool appends Tasks snapshot (P3 ③)"
```

### Task 8: Tasks rail drawer

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`DrawerId::Tasks`, drawer in `new`)
- Modify: `crates/zoid-tui/src/layout.rs` (`TASKS_BODY_ROWS`, `drawer_body_rows` arm)
- Modify: `crates/zoid-tui/src/render.rs` (`render_tasks_body`, `render_rail` arm)
- Test: `crates/zoid-tui/tests/` snapshot + `state.rs` unit test

**Interfaces:**
- Consumes: `zoid_core::tasks::{TaskItem, TaskStatus}`; `tokens::glyph::{PENDING, RUNNING, PASS}`.
- Produces: a fourth rail drawer rendered below Context.

- [ ] **Step 1: Add the drawer id + drawer, with a failing test**

In `crates/zoid-tui/src/state.rs`, extend `DrawerId`:

```rust
pub enum DrawerId {
    Repo,
    Session,
    Context,
    Tasks,
}
```

In `ShellState::new`, append after the Context drawer in the `drawers` vec:

```rust
            Drawer {
                id: DrawerId::Tasks,
                title: "tasks".into(),
                open: true,
            },
```

Add a unit test:

```rust
    #[test]
    fn tasks_drawer_is_last_and_open() {
        let s = ShellState::new();
        let last = s.drawers.last().unwrap();
        assert_eq!(last.id, DrawerId::Tasks);
        assert!(last.open);
    }
```

Run: `cargo test -p zoid-tui tasks_drawer_is_last_and_open`
Expected: FAIL to compile until the `layout.rs` match is exhaustive (next step). Add the arm, then it passes.

- [ ] **Step 2: Give the drawer a body height**

In `crates/zoid-tui/src/layout.rs`, add the constant and match arm:

```rust
/// Tasks drawer body rows: up to a handful of the model's current tasks.
pub const TASKS_BODY_ROWS: u16 = 5;
```

```rust
pub fn drawer_body_rows(id: DrawerId) -> u16 {
    match id {
        DrawerId::Repo => REPO_BODY_ROWS,
        DrawerId::Session => SESSION_BODY_ROWS,
        DrawerId::Context => CONTEXT_BODY_ROWS,
        DrawerId::Tasks => TASKS_BODY_ROWS,
    }
}
```

Run: `cargo test -p zoid-tui tasks_drawer_is_last_and_open` → PASS.

- [ ] **Step 3: Render the drawer body**

In `crates/zoid-tui/src/render.rs`, add a `render_tasks_body` following the shape of `render_repo_body`/`render_economy_body` (take the same `frame`, `area`, and a `&[TaskItem]` derived from `tasks(&app.events)` — thread the task list in the same way the economy view is passed, or compute it where the rail is rendered). Body:

```rust
fn render_tasks_body(frame: &mut Frame, area: Rect, items: &[zoid_core::tasks::TaskItem]) {
    use zoid_core::tasks::TaskStatus;
    if items.is_empty() {
        let line = Line::from(Span::styled(
            "no tasks",
            Style::default().fg(crate::tokens::color::DIM),
        ));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let rows: Vec<Line> = items
        .iter()
        .take(area.height as usize)
        .map(|it| {
            let (glyph, color) = match it.status {
                TaskStatus::Pending => (crate::tokens::glyph::PENDING, crate::tokens::color::DIM),
                TaskStatus::Active => (crate::tokens::glyph::RUNNING, crate::tokens::color::WARN),
                TaskStatus::Done => (crate::tokens::glyph::PASS, crate::tokens::color::OK),
            };
            let text_color = if matches!(it.status, TaskStatus::Done) {
                crate::tokens::color::DIM
            } else {
                crate::tokens::color::TXT
            };
            let label = crate::text::truncate(&it.text, area.width.saturating_sub(2) as usize);
            Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(text_color)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
}
```

In `render_rail`'s per-drawer `match` (the arm that dispatches `Context → render_economy_body`, etc.), add:

```rust
                DrawerId::Tasks => render_tasks_body(frame, body_rect, &tasks_items),
```

where `tasks_items = zoid_core::tasks::tasks(events)` is computed once before the drawer loop (pass `events` into `render_rail`, or compute in the caller and thread it in like `economy`). Match the existing signature style; if `render_rail` already receives `&[ChatMsg]` or events, derive from those.

- [ ] **Step 4: Snapshot the drawer (full + empty)**

Add a snapshot test in `crates/zoid-tui/tests/` (follow `session_snapshot.rs`): render a shell with a 3-task list (one Pending, one Active, one Done) at 100×24, and a second with no tasks. `insta::assert_snapshot!` both. Inspect each `.new` — confirm the `☐ / ◐ / ✓` glyphs and the `no tasks` line — then accept.

Run: `cargo test -p zoid-tui`
Expected: PASS (after accepting the new snapshots). Also update `crates/zoid-tui/examples/preview.rs` scene list if it enumerates drawers, and any existing rail snapshot that now shows the extra drawer (inspect + accept the diff — the new drawer appears at the bottom).

- [ ] **Step 5: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/layout.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests
git commit -m "feat(tui): tasks rail drawer under context (P3 ③)"
```

---

## Phase 4 — `ask_user` interactive tool + question overlay (②)

### Task 9: `ask_user` tool, `Answer`, interactive loop routing

**Files:**
- Create: `crates/zoid-tools/src/ask.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (module + registry)
- Modify: `crates/zoid/src/agent.rs` (`Answer`, `AgentUpdate::AskUser`, Interactive arm)
- Modify: `crates/zoid/src/subagent.rs` (filter out Interactive tools)
- Test: `crates/zoid/tests/ask_user.rs` (integration with an inline responder)

**Interfaces:**
- Produces:
  - `pub struct AskUser;` implementing `Tool` — `name() == "ask_user"`, `kind() == ToolKind::Interactive`, `spec()` schema `{ question: string, choices?: string[] }`.
  - `pub enum Answer { Choice(String), FreeText(String), LetYouDecide }` in `agent.rs`.
  - `AgentUpdate::AskUser { question: String, choices: Vec<String>, reply: tokio::sync::oneshot::Sender<Answer> }`.
- Behavior: the loop sends `AskUser` on the `ui` channel, awaits the `oneshot`; maps `Answer` → result string (`Choice`/`FreeText` verbatim, `LetYouDecide` → `"[let you decide]"`); on a dropped sender (Esc-abort) emits `ToolResult { output: "[user aborted]", is_error: false }` for the pending call and `break 'turn`.

- [ ] **Step 1: Write the tool with a spec test**

Create `crates/zoid-tools/src/ask.rs`:

```rust
//! `ask_user` — an Interactive tool. The agent loop intercepts it (by kind),
//! prompts the UI, and awaits the user's answer; `run()` is never called.

use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;
use crate::{Tool, ToolKind, ToolOutput};

pub struct AskUser;

impl Tool for AskUser {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_user".into(),
            description: "Ask the user a question and wait for their answer. Omit `choices` for a \
                free-text answer, or provide `choices` to offer specific options. Use sparingly, \
                when you genuinely need the user to decide or clarify."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "choices": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["question"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Interactive
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("ask_user must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_ask_user_schema() {
        let s = AskUser.spec();
        assert_eq!(s.name, "ask_user");
        assert_eq!(AskUser.kind(), ToolKind::Interactive);
        assert!(s.parameters["properties"]["question"].is_object());
    }
}
```

In `crates/zoid-tools/src/lib.rs`: `pub mod ask;`, add `Box::new(ask::AskUser)` to `registry()`, and extend `registry_has_unique_named_tools` with `assert!(names.contains(&"ask_user"));`.

Run: `cargo test -p zoid-tools` → PASS.

- [ ] **Step 2: Keep interactive tools away from subagents**

In `crates/zoid/src/subagent.rs`, where the subagent's tool set is assembled from a filtered registry, add a filter dropping interactive tools (a headless subagent cannot answer a prompt and would hang):

```rust
    let tools: Vec<Box<dyn Tool>> = zoid_tools::registry()
        .into_iter()
        .filter(|t| profile.allows(t.name()))
        .filter(|t| t.kind() != zoid_tools::ToolKind::Interactive)
        .collect();
```

Match the existing variable/flow names in `subagent.rs`; the added `.filter` is the load-bearing line. Add/extend a subagent test asserting `ask_user` is absent from the assembled specs.

- [ ] **Step 3: Write the failing integration test (with an inline responder)**

Create `crates/zoid/tests/ask_user.rs`. Because `AskUser`/`AgentUpdate` are `zoid`-crate types, the auto-responder lives here (not in `zoid-testkit`), draining the `ui` receiver and answering:

```rust
use zoid::agent::{AgentUpdate, Answer};

#[tokio::test]
async fn ask_user_answer_becomes_the_tool_result() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("ask_user", serde_json::json!({
            "question": "Which DB?", "choices": ["postgres", "sqlite"]
        })),
        zoid_testkit::text("using it"),
        zoid_provider::ProviderEvent::Done,
    ]);
    // ui channel: the loop sends AgentUpdate on `ui_tx`; we drain `ui_rx`.
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AgentUpdate>(64);

    // Responder: answer the first AskUser with a Choice, ignore other updates.
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                let _ = reply.send(Answer::Choice("postgres".into()));
            }
        }
    });

    // ... build tools = Arc::new(zoid_tools::registry()), session, seed, etc. ...
    let events = zoid::agent::run_agent_turn(
        zoid::agent::chat_turn_config(), provider, tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session, seed, model, ui_tx, session_id, || 0,
    ).await.unwrap();
    responder.abort();

    let results = zoid_testkit::tool_results(&events);
    assert!(results.iter().any(|(n, out, err)| n == "ask_user" && !err && out == "postgres"));
}

#[tokio::test]
async fn ask_user_dropped_sender_aborts_turn_with_balanced_result() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("ask_user", serde_json::json!({ "question": "stop?" })),
        zoid_provider::ProviderEvent::Done,
    ]);
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AgentUpdate>(64);
    // Responder DROPS the reply sender (models Esc-abort).
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                drop(reply);
            }
        }
    });
    // ... setup ...
    let events = zoid::agent::run_agent_turn(/* ... */).await.unwrap();
    responder.abort();
    // The turn ended, and the pending ask_user call has a balanced [user aborted] result.
    let results = zoid_testkit::tool_results(&events);
    assert!(results.iter().any(|(n, out, _)| n == "ask_user" && out == "[user aborted]"));
}
```

Run: `cargo test -p zoid ask_user`
Expected: FAIL — `Answer` and `AgentUpdate::AskUser` don't exist; the loop doesn't handle Interactive.

- [ ] **Step 4: Add `Answer`, the `AskUser` update, and the Interactive arm**

In `crates/zoid/src/agent.rs`:

```rust
use tokio::sync::oneshot;

/// The user's answer to an `ask_user` prompt.
pub enum Answer {
    Choice(String),
    FreeText(String),
    /// The user chose to let the agent decide (a positive choice, not a cancel).
    LetYouDecide,
}
```

Extend `AgentUpdate`:

```rust
    /// The model asked the user a question; the loop parks until `reply` resolves.
    /// Dropping `reply` (Esc) aborts the turn.
    AskUser {
        question: String,
        choices: Vec<String>,
        reply: oneshot::Sender<Answer>,
    },
```

Add the Interactive arm to the `match kind` in the tool loop (alongside the `Emitting` arm from Task 7):

```rust
                Some(zoid_tools::ToolKind::Interactive) if tc.name == "ask_user" => {
                    let question = tc.args.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let choices = tc.args.get("choices").and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    let (rtx, rrx) = oneshot::channel::<Answer>();
                    let _ = ui.send(AgentUpdate::AskUser { question, choices, reply: rtx }).await;
                    match rrx.await {
                        Ok(ans) => {
                            let output = match ans {
                                Answer::Choice(s) | Answer::FreeText(s) => s,
                                Answer::LetYouDecide => "[let you decide]".to_string(),
                            };
                            emit(&session, &mut events, ui, &config.branch,
                                EventKind::ToolResult { id: tc.id, name: tc.name, output, is_error: false },
                                session_id, now).await?;
                        }
                        Err(_) => {
                            // Sender dropped == Esc hard-abort. Record a balanced result, end the turn.
                            emit(&session, &mut events, ui, &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id, name: tc.name,
                                    output: "[user aborted]".to_string(), is_error: false,
                                },
                                session_id, now).await?;
                            break 'turn;
                        }
                    }
                }
```

- [ ] **Step 5: Run the integration tests + workspace**

Run: `cargo test -p zoid ask_user` then `cargo test --workspace`
Expected: PASS (both ask_user tests, subagent filter test, all prior).

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tools/src/ask.rs crates/zoid-tools/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/subagent.rs crates/zoid/tests/ask_user.rs
git commit -m "feat(agent): ask_user interactive tool + Answer round-trip (P4 ②)"
```

### Task 10: Question overlay state + routing

**Files:**
- Create: `crates/zoid-tui/src/question.rs`
- Modify: `crates/zoid-tui/src/state.rs` (`Overlay::Question`, `Action` variants)
- Modify: `crates/zoid-tui/src/route.rs` (`Overlay::Question` dispatch)
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod question;`)

**Interfaces:**
- Produces:
  - `pub enum QuestionMode { Pick, FreeText }`.
  - `pub struct QuestionState { pub question: String, pub choices: Vec<String>, pub selected: usize, pub free_text: String, pub mode: QuestionMode }` with `pub fn new(question, choices)`, `pub fn rows(&self) -> Vec<String>` (choices + `Other… (type my own)` + `— let you decide —`), and a resolver `pub fn resolved(&self) -> QuestionOutcome`.
  - `pub enum QuestionOutcome { Choice(String), FreeText(String), LetYouDecide, EnterFreeText }`.
  - `pub fn route_question_key(state: &QuestionState, key: KeyEvent) -> Action`.
  - `Action` variants: `QuestionMove(i32)`, `QuestionSelect`, `QuestionChar(char)`, `QuestionBackspace`, `QuestionAbort`.

- [ ] **Step 1: Write the module with unit tests**

Create `crates/zoid-tui/src/question.rs`:

```rust
//! The `ask_user` question overlay: pick-list (with synthetic "Other…" and
//! "let you decide" rows) or free-text. Selection wraps via `palette::nav`.

use crossterm::event::{KeyCode, KeyEvent};
use crate::state::Action;

const OTHER_LABEL: &str = "Other… (type my own)";
const DECIDE_LABEL: &str = "— let you decide —";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionMode {
    Pick,
    FreeText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionState {
    pub question: String,
    pub choices: Vec<String>,
    pub selected: usize,
    pub free_text: String,
    pub mode: QuestionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionOutcome {
    Choice(String),
    FreeText(String),
    LetYouDecide,
    /// The user picked "Other…": switch the overlay into free-text entry.
    EnterFreeText,
}

impl QuestionState {
    pub fn new(question: impl Into<String>, choices: Vec<String>) -> Self {
        let mode = if choices.is_empty() { QuestionMode::FreeText } else { QuestionMode::Pick };
        Self { question: question.into(), choices, selected: 0, free_text: String::new(), mode }
    }

    /// Rows shown in pick mode: the model's choices + the two synthetic entries.
    pub fn rows(&self) -> Vec<String> {
        let mut r = self.choices.clone();
        r.push(OTHER_LABEL.to_string());
        r.push(DECIDE_LABEL.to_string());
        r
    }

    /// What the current selection / buffer resolves to when committed.
    pub fn resolved(&self) -> QuestionOutcome {
        match self.mode {
            QuestionMode::FreeText => {
                if self.free_text.is_empty() {
                    QuestionOutcome::LetYouDecide
                } else {
                    QuestionOutcome::FreeText(self.free_text.clone())
                }
            }
            QuestionMode::Pick => {
                let rows = self.rows();
                let idx = self.selected.min(rows.len() - 1);
                if idx == rows.len() - 1 {
                    QuestionOutcome::LetYouDecide
                } else if idx == rows.len() - 2 {
                    QuestionOutcome::EnterFreeText
                } else {
                    QuestionOutcome::Choice(rows[idx].clone())
                }
            }
        }
    }
}

/// Map a keypress to an `Action` while the question overlay is up.
pub fn route_question_key(state: &QuestionState, key: KeyEvent) -> Action {
    match state.mode {
        QuestionMode::Pick => match key.code {
            KeyCode::Up => Action::QuestionMove(-1),
            KeyCode::Down => Action::QuestionMove(1),
            KeyCode::Enter => Action::QuestionSelect,
            KeyCode::Esc => Action::QuestionAbort,
            _ => Action::Noop,
        },
        QuestionMode::FreeText => match key.code {
            KeyCode::Enter => Action::QuestionSelect,
            KeyCode::Esc => Action::QuestionAbort,
            KeyCode::Backspace => Action::QuestionBackspace,
            KeyCode::Char(c) => Action::QuestionChar(c),
            _ => Action::Noop,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    #[test]
    fn empty_choices_starts_in_free_text() {
        let q = QuestionState::new("why?", vec![]);
        assert_eq!(q.mode, QuestionMode::FreeText);
    }

    #[test]
    fn pick_rows_append_other_and_decide() {
        let q = QuestionState::new("db?", vec!["pg".into(), "sqlite".into()]);
        let rows = q.rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2], OTHER_LABEL);
        assert_eq!(rows[3], DECIDE_LABEL);
    }

    #[test]
    fn last_row_resolves_to_let_you_decide() {
        let mut q = QuestionState::new("db?", vec!["pg".into()]);
        q.selected = q.rows().len() - 1;
        assert_eq!(q.resolved(), QuestionOutcome::LetYouDecide);
    }

    #[test]
    fn other_row_resolves_to_enter_free_text() {
        let mut q = QuestionState::new("db?", vec!["pg".into()]);
        q.selected = q.rows().len() - 2;
        assert_eq!(q.resolved(), QuestionOutcome::EnterFreeText);
    }

    #[test]
    fn choice_row_resolves_to_that_choice() {
        let q = QuestionState::new("db?", vec!["pg".into(), "sqlite".into()]);
        assert_eq!(q.resolved(), QuestionOutcome::Choice("pg".into()));
    }

    #[test]
    fn empty_free_text_submit_is_let_you_decide() {
        let q = QuestionState::new("why?", vec![]);
        assert_eq!(q.resolved(), QuestionOutcome::LetYouDecide);
    }

    #[test]
    fn esc_routes_to_abort_in_both_modes() {
        let pick = QuestionState::new("db?", vec!["pg".into()]);
        assert_eq!(route_question_key(&pick, k(KeyCode::Esc)), Action::QuestionAbort);
        let free = QuestionState::new("why?", vec![]);
        assert_eq!(route_question_key(&free, k(KeyCode::Esc)), Action::QuestionAbort);
    }
}
```

- [ ] **Step 2: Add the `Overlay` variant, `Action` variants, and module**

In `crates/zoid-tui/src/state.rs`: add `Question` to `Overlay`; add a field to hold the state (`pub question: Option<crate::question::QuestionState>`, initialized `None` in `new`); add to the `Action` enum: `QuestionMove(i32)`, `QuestionSelect`, `QuestionChar(char)`, `QuestionBackspace`, `QuestionAbort`.

In `crates/zoid-tui/src/lib.rs`: `pub mod question;`.

In `crates/zoid-tui/src/route.rs` `route_key`, add to the overlay match:

```rust
        Overlay::Question => {
            return match &state.question {
                Some(q) => crate::question::route_question_key(q, key),
                None => Action::Noop,
            };
        }
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p zoid-tui question::`
Expected: PASS (all seven).

- [ ] **Step 4: Lint, format, commit**

```bash
cargo clippy -p zoid-tui --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tui/src/question.rs crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): question overlay state + key routing (P4 ②)"
```

### Task 11: Question overlay render + main.rs wiring

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render_question`, overlay dispatch)
- Modify: `crates/zoid/src/main.rs` (`AgentUpdate::AskUser` handling; question actions; Esc-abort)
- Test: `crates/zoid-tui/tests/` snapshot (both modes)

**Interfaces:**
- Consumes: `QuestionState`, `QuestionOutcome`, `zoid::agent::{Answer, AgentUpdate}`, `layout::centered`.
- Produces: the App holds `pending_answer: Option<oneshot::Sender<Answer>>`; the question overlay renders and answers round-trip.

- [ ] **Step 1: Render the overlay**

In `crates/zoid-tui/src/render.rs`, add `render_question(frame, area, q: &QuestionState)` — a centered card (reuse `layout::centered`, like the settings card): the question text, then in Pick mode the `q.rows()` with the selected row highlighted (`tokens::color::SEL_BG`), or in FreeText mode the `q.free_text` buffer with a caret glyph and a hint line `⏎ submit · empty = let you decide · Esc take over`. All glyphs from `tokens`. Dispatch it where overlays are drawn:

```rust
    if state.overlay == Overlay::Question {
        if let Some(q) = &state.question {
            render_question(frame, area, q);
        }
    }
```

- [ ] **Step 2: Snapshot both modes**

Add a snapshot test: one `ShellState` with `overlay = Overlay::Question` and `question = Some(QuestionState::new("Which DB?", vec!["postgres".into(),"sqlite".into()]))`, and a second free-text `QuestionState::new("Describe the bug", vec![])`. Render 100×24, `insta::assert_snapshot!`. Inspect `.new` (confirm the two synthetic rows in pick mode, the hint line in free-text), accept.

Run: `cargo test -p zoid-tui`
Expected: PASS after accepting.

- [ ] **Step 3: Wire the App side**

In `crates/zoid/src/main.rs`:

1. Add field to `App`: `pending_answer: Option<tokio::sync::oneshot::Sender<zoid::agent::Answer>>,` (init `None` in both App initializers — the main one near line 569 and the test one near 1874).
2. In the `ui_rx` match, handle the new update:

```rust
                    AgentUpdate::AskUser { question, choices, reply } => {
                        app.shell.question = Some(zoid_tui::question::QuestionState::new(question, choices));
                        app.shell.overlay = zoid_tui::state::Overlay::Question;
                        app.pending_answer = Some(reply);
                    }
```

3. Handle the question actions where other `Action`s are dispatched. Add helper to send an answer and close the overlay:

```rust
fn answer_question(app: &mut App, ans: zoid::agent::Answer) {
    if let Some(tx) = app.pending_answer.take() {
        let _ = tx.send(ans);
    }
    app.shell.question = None;
    app.shell.overlay = zoid_tui::state::Overlay::None;
}
```

Then, in the action match:

```rust
        Action::QuestionMove(d) => {
            if let Some(q) = &mut app.shell.question {
                let len = q.rows().len();
                q.selected = zoid_tui::palette::nav(q.selected, d, len);
            }
        }
        Action::QuestionChar(c) => {
            if let Some(q) = &mut app.shell.question { q.free_text.push(c); }
        }
        Action::QuestionBackspace => {
            if let Some(q) = &mut app.shell.question { q.free_text.pop(); }
        }
        Action::QuestionSelect => {
            use zoid_tui::question::{QuestionMode, QuestionOutcome};
            let outcome = app.shell.question.as_ref().map(|q| q.resolved());
            match outcome {
                Some(QuestionOutcome::EnterFreeText) => {
                    if let Some(q) = &mut app.shell.question { q.mode = QuestionMode::FreeText; }
                }
                Some(QuestionOutcome::Choice(s)) => answer_question(app, zoid::agent::Answer::Choice(s)),
                Some(QuestionOutcome::FreeText(s)) => answer_question(app, zoid::agent::Answer::FreeText(s)),
                Some(QuestionOutcome::LetYouDecide) => answer_question(app, zoid::agent::Answer::LetYouDecide),
                None => {}
            }
        }
        Action::QuestionAbort => {
            // Esc = hard abort: drop the sender (the loop unwinds, records a
            // balanced [user aborted] result and ends the turn) and close.
            app.pending_answer = None; // dropping the Sender signals abort
            app.shell.question = None;
            app.shell.overlay = zoid_tui::state::Overlay::None;
        }
```

- [ ] **Step 4: Run the workspace**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Manual smoke (optional, documented)**

With `OLLAMA_API_KEY` set, run `cargo run -p zoid`, ask the model something that induces an `ask_user` call, confirm: the overlay appears; arrowing wraps through choices + the two synthetic rows; picking a choice resumes the turn; empty free-text submit sends "let you decide"; Esc ends the turn and returns control.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-tui/src/render.rs crates/zoid/src/main.rs crates/zoid-tui/tests
git commit -m "feat(tui): render ask_user question overlay + answer round-trip (P4 ②)"
```

---

## Final verification

- [ ] `cargo test --workspace` — all green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [ ] `cargo fmt --all --check` — clean.
- [ ] Manual: run zoid, exercise wrap-nav in the palette, watch a `shell` call show the `◐ running` line, have the model publish tasks (rail drawer updates), and answer + abort an `ask_user` prompt.

## Notes for the executor (spec deviations to confirm)

1. **Testkit auto-responder placement:** the spec put the `ask_user` auto-responder inside `zoid-testkit`. Because `AgentUpdate`/`Answer` are `zoid`-crate types and `zoid-testkit` must stay dependency-downward (core+provider only), the responder lives as an inline helper in `crates/zoid/tests/ask_user.rs` instead. `zoid-testkit` still provides the scripted provider that emits the `ask_user` call. Flag to the user if they want the kit to instead depend on the `zoid` crate.
2. **No pre-existing turn-abort path:** the spec referenced an "existing streaming-interrupt path" for Esc; there is none (`spawn_turn` is fire-and-forget). Esc-abort is implemented as cooperative cancellation via a dropped `oneshot` while the loop is parked on a question — which only aborts a turn that is currently awaiting an answer. A general "abort any streaming turn" is out of scope.
3. **`ToolStarted` placement:** Task 4 first adds the spinner emit in the tool loop; Task 7 moves it into the `Local` match arm (Emitting/Interactive tools resolve instantly and need no spinner). The end state is the Task 7 code.
