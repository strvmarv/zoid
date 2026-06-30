# P5c · Subagent Executor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run a discrete task as an isolated subagent — its own constructed context, its own working directory, its own event branch — and return a `SubagentResult` the orchestrator can fold back. One subagent at a time.

**Architecture:** Generalize the existing Chat agent loop instead of forking it (DRY): `run_agent_turn` gains a `TurnConfig { system, cwd, branch }` and returns the events it produced. A subagent is then just that loop, **seeded with the P5b constructed prompt as a single `UserMessage`** on a `BranchId("subagent:<ulid>")`, with the subagent system prompt and a `cwd`. `run_subagent` runs it, then reads the produced events to extract a one-paragraph `SubagentResult { summary, ok }`. Chat delegation (P5d) drives this.

**Tech Stack:** Rust 2021, tokio. Consumes P5a (`cwd` seam) + P5b (`build_subagent_request`, `subagent_policy`).

## Global Constraints

- **Generalize, don't fork (DRY):** there is ONE agent loop. `run_agent_turn` takes a `TurnConfig` and serves both Chat (orchestrator) and subagents. Do not copy-paste the streaming/tool/iteration logic into a second function.
- **One subagent at a time (spec §4.4/§12):** the executor is sequential — no fleet, no parallel dispatch. P5d enforces the single-active-subagent guard; P5c just runs one.
- **Subagent events on their own branch:** every event a subagent produces carries `BranchId("subagent:<id>")` (via `TurnConfig.branch`); the orchestrator's `conversation()` (P5d) folds only the main branch. Chat keeps `BranchId::default()` ("main") — behavior unchanged.
- **Never session history (spec §4.4):** the subagent is seeded ONLY with the constructed prompt (task + relevant code from P5b), not the chat log. Its loop folds only its own (seed + produced) events.
- **cwd from P5a:** the subagent's tools run in `TurnConfig.cwd`. In Chat delegation (P5d) this is the process cwd (`"."`); Build (P6+) passes a worktree path. P5c is cwd-agnostic — it just threads it.
- **Provider-agnostic:** uses the existing `Provider` (Ollama/GLM). `SubagentResult.ok` is derived from whether the loop emitted an error message — provider-neutral.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit.

---

### Task 1: Generalize the agent loop — `TurnConfig`, threaded cwd/branch/system, returns events

**Files:**
- Modify: `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/src/main.rs` (Chat caller builds a `TurnConfig`)
- Modify: `crates/zoid/tests/*` (existing agent-loop / economy-integration tests adopt the new signature)
- Test: inline + existing tests stay green; one new branch-tagging test.

**Interfaces:**
- Consumes: P5a's `run_tool(.., cwd)`.
- Produces:
  - `pub struct TurnConfig { pub system: String, pub cwd: std::path::PathBuf, pub branch: zoid_core::event::BranchId }`.
  - `pub fn build_request(events, model, tools, system: &str) -> CompletionRequest` (gains `system`).
  - `pub async fn run_agent_turn(config: TurnConfig, provider, tools, session, events: Vec<Event>, model, ui, now) -> Result<Vec<Event>>` — now returns the events it accumulated (seed + produced).
  - `pub fn chat_turn_config() -> TurnConfig` — `{ SYSTEM_PROMPT, ".", BranchId::default() }`.

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/agent.rs` `mod tests` (add the module if absent; otherwise extend):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::event::BranchId;

    #[test]
    fn chat_turn_config_is_main_branch_cwd_dot() {
        let c = chat_turn_config();
        assert_eq!(c.branch, BranchId::default());
        assert_eq!(c.cwd, std::path::PathBuf::from("."));
        assert_eq!(c.system, SYSTEM_PROMPT);
    }

    #[test]
    fn build_request_uses_the_given_system_prompt() {
        let req = build_request(&[], "m", &zoid_tools::registry(), "CUSTOM SYS");
        assert_eq!(req.system.as_deref(), Some("CUSTOM SYS"));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid agent::tests`
Expected: FAIL — `TurnConfig`/`chat_turn_config` undefined; `build_request` arity.

- [ ] **Step 3: Implement the generalization**

In `crates/zoid/src/agent.rs`:

Add the config + helper (near `SYSTEM_PROMPT`):

```rust
use std::path::PathBuf;
use zoid_core::event::BranchId;

/// How one agent turn is run: its system prompt, working directory, and the
/// event branch its output is recorded on. Chat uses the main branch + cwd;
/// a subagent uses its own branch + (optionally) a worktree.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
}

/// The orchestrator (Chat) turn config: main branch, process cwd, Chat prompt.
pub fn chat_turn_config() -> TurnConfig {
    TurnConfig {
        system: SYSTEM_PROMPT.to_string(),
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
    }
}
```

Change `build_request` to take the system prompt:

```rust
pub fn build_request(events: &[Event], model: &str, tools: &[Box<dyn Tool>], system: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: Some(system.to_string()),
        messages: conversation(events).into_iter().map(map_msg).collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}
```

Change `run_agent_turn` + `run_turn_inner` to take `config: TurnConfig` and return `Result<Vec<Event>>`:

```rust
pub async fn run_agent_turn(
    config: TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    session: SessionHandle,
    events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<Vec<Event>> {
    let result = run_turn_inner(&config, provider, tools, session, events, model, &ui, now).await;
    let _ = ui.send(AgentUpdate::TurnComplete).await;
    result
}
```

In `run_turn_inner`: take `config: &TurnConfig`; use `config.system` in `build_request`; pass `&config.cwd` to `run_tool`; pass `config.branch.clone()` to `emit`; and `Ok(events)` at the end (and the loop's `break 'turn` paths fall through to it). Concretely:
- `let req = build_request(&events, &model, &tools, &config.system);`
- tool exec: `zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd_for_exec)` where `let cwd_for_exec = config.cwd.clone();` is captured before the `spawn_blocking` (a `PathBuf` moved into the closure).
- change the function's return type to `Result<Vec<Event>>` and replace the final `Ok(())` with `Ok(events)`.

Thread the branch through `emit`/`emit_with_tokens` — add a `branch: &BranchId` parameter and set it on the event:

```rust
async fn emit_with_tokens(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    tokens: Option<zoid_core::event::TokenStat>,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind);
    ev.branch = branch.clone();
    ev.tokens = tokens;
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(ev)).await;
    Ok(())
}
```

Update `emit` to forward `branch`, and every `emit(...)` / `emit_with_tokens(...)` call inside `run_turn_inner` to pass `&config.branch`.

- [ ] **Step 4: Update the Chat caller**

In `crates/zoid/src/main.rs`, in `spawn_turn`, pass the Chat config and ignore the returned events:

```rust
fn spawn_turn(app: &App) {
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        let _ = run_agent_turn(zoid::agent::chat_turn_config(), provider, tools, session, seed, model, ui, now_ms).await;
    });
}
```

(Adjust the `run_agent_turn` import/path to match how `main.rs` currently references it.)

- [ ] **Step 5: Update existing bin tests**

In `crates/zoid/tests/` the agent-loop / economy-integration tests call `run_agent_turn(provider, tools, session, seed, model, tx, now)`. Update each to:
- prepend `zoid::agent::chat_turn_config()` (or build a `TurnConfig`) as the first argument, and
- bind the now-returned `Result<Vec<Event>>` (e.g. `let _events = run_agent_turn(...).await.unwrap();`).

The assertions about the ledger / appended events are unchanged (Chat still uses the main branch + cwd `"."`).

- [ ] **Step 6: Run to confirm pass**

Run: `cargo test -p zoid && cargo clippy -p zoid --all-targets`
Expected: PASS (existing behavior preserved; 2 new agent tests green), zero warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/tests/
git commit -m "feat(zoid): generalize agent loop — TurnConfig (system/cwd/branch), returns events"
```

---

### Task 2: `run_subagent` + `SubagentResult`

**Files:**
- Modify: `crates/zoid/src/subagent.rs`
- Test: inline `mod tests` (with the fake provider).

**Interfaces:**
- Consumes: `build_subagent_request`/`subagent_policy` (P5b), `agent::{run_agent_turn, TurnConfig, AgentUpdate}`, `SessionHandle`, `Provider`.
- Produces:
  - `pub struct SubagentResult { pub branch: String, pub summary: String, pub ok: bool }`.
  - `pub async fn run_subagent(task, context_events, provider, tools, cwd, model, session, ui, now) -> Result<SubagentResult>`.

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/subagent.rs` `mod tests`:

```rust
#[tokio::test]
async fn subagent_runs_constructed_task_and_returns_summary() {
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use zoid_core::session::SessionHandle;
    use zoid_provider::{FakeProvider, ProviderEvent};

    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("Refactored parse() into two functions.".into()),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let res = run_subagent(
        "refactor parse()",
        &[],
        provider,
        Arc::new(zoid_tools::registry()),
        std::path::PathBuf::from("."),
        "glm".into(),
        session.clone(),
        tx,
        || 0,
    )
    .await
    .unwrap();

    assert!(res.ok, "no error emitted → ok");
    assert!(res.summary.contains("Refactored parse()"), "summary = subagent's final text");
    assert!(res.branch.starts_with("subagent:"));
    // The subagent's work is persisted on its own branch.
    let snap = session.snapshot().await.unwrap();
    assert!(snap.iter().any(|e| e.branch.0 == res.branch));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid subagent::tests::subagent_runs`
Expected: FAIL — `run_subagent`/`SubagentResult` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid/src/subagent.rs` add imports and the executor:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::Provider;

use crate::agent::{run_agent_turn, AgentUpdate, TurnConfig};
```

```rust
/// The outcome of a dispatched subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}

/// Run `task` as an isolated subagent: build its constructed context (P5b),
/// seed it as the first user message on a fresh `subagent:<id>` branch, run the
/// generalized agent loop in `cwd`, and distill a `SubagentResult` from what it
/// produced. Sequential — the caller dispatches one at a time.
pub async fn run_subagent(
    task: &str,
    context_events: &[Event],
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    cwd: PathBuf,
    model: String,
    session: SessionHandle,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<SubagentResult> {
    let branch = BranchId(format!("subagent:{}", Ulid::new()));

    // The constructed prompt (task + relevant code) becomes the seed user turn.
    let req = build_subagent_request(task, context_events, &subagent_policy(), &model, &tools);
    let prompt = req.messages[0].content.clone();
    let mut seed = Event::new(Ulid::new(), None, now(), EventKind::UserMessage { text: prompt });
    seed.branch = branch.clone();
    session.append(seed.clone()).await?;

    let config = TurnConfig {
        system: SUBAGENT_SYSTEM_PROMPT.to_string(),
        cwd,
        branch: branch.clone(),
    };
    let produced = run_agent_turn(config, provider, tools, session, vec![seed], model, ui, now).await?;

    // Distill the result: last assistant text = summary; an emitted ⚠ = not ok.
    let msgs = conversation(&produced);
    let summary = msgs
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let ok = !summary.starts_with('\u{26A0}'); // ⚠ = error message

    Ok(SubagentResult { branch: branch.0, summary, ok })
}
```

> `Tool` is already imported at the top of `subagent.rs` (P5b). `SUBAGENT_SYSTEM_PROMPT`/`build_subagent_request`/`subagent_policy` are defined in this module (P5b).

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid subagent`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "feat(zoid): run_subagent — isolated branch + cwd; returns SubagentResult"
```

---

### Task 3: Integration — subagent edits a file in its cwd and reports

**Files:**
- Create: `crates/zoid/tests/subagent_integration.rs`
- Test: the file itself.

**Interfaces:**
- Consumes: `run_subagent`, `FakeProvider` (scripted tool call), `zoid_tools::registry`.

> Proves the whole P5a+P5b+P5c chain: a subagent receives a constructed task, calls a tool **in its cwd** (P5a seam), records work on its branch, and returns a result.

- [ ] **Step 1: Write the failing integration test**

`crates/zoid/tests/subagent_integration.rs`:

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use zoid::subagent::run_subagent;
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent, ToolCall};

#[tokio::test]
async fn subagent_writes_a_file_in_its_cwd() {
    let dir = tempfile::tempdir().unwrap();

    // Scripted provider: first turn asks to write a file; after the tool result,
    // second turn produces a summary and stops.
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::ToolCall(ToolCall {
            id: "w1".into(),
            name: "write_file".into(),
            args: serde_json::json!({ "path": "out.txt", "content": "made by subagent" }),
        }),
        ProviderEvent::Done,
        // FakeProvider replays this batch on the next request (see its test usage).
        ProviderEvent::TextDelta("Wrote out.txt.".into()),
        ProviderEvent::Done,
    ]));

    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let res = run_subagent(
        "create out.txt",
        &[],
        provider,
        Arc::new(zoid_tools::registry()),
        dir.path().to_path_buf(),   // subagent cwd = the temp dir (P5a seam)
        "glm".into(),
        session,
        tx,
        || 0,
    )
    .await
    .unwrap();

    // The write landed in the subagent's cwd, NOT the process cwd.
    assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "made by subagent");
    assert!(res.ok);
}
```

> If `FakeProvider` does not replay batched events across successive `stream()` calls, adapt the test to its actual contract — check `crates/zoid-provider/src/` for the fake's behavior (the P3 economy-integration test is a working reference). The essential assertion is that the file lands in `dir`, proving the cwd seam.

- [ ] **Step 2: Run to confirm failure, then pass**

Run: `cargo test -p zoid --test subagent_integration`
Expected: initially FAIL if the fake's replay contract differs; once the script matches the fake's behavior, PASS with `out.txt` written into the temp dir.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/subagent_integration.rs
git commit -m "test(zoid): subagent writes a file in its cwd (P5a+P5b+P5c integration)"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] ONE agent loop: `run_agent_turn(TurnConfig, …)` serves both Chat and subagents (no duplicated loop).
- [ ] Subagent events carry `BranchId("subagent:<id>")`; Chat events stay on `"main"`.
- [ ] A subagent's tools run in its `cwd` (integration test writes into a temp dir, not the process cwd).
- [ ] `SubagentResult { branch, summary, ok }` — `summary` is the subagent's final text; `ok` flips on an emitted ⚠.

## Self-Review notes (author)

- **Spec coverage (§4.4/§7 L1 runtime):** the **subagent executor** runs an agent turn in isolation (own branch, own cwd) and reports back — exactly the reusable executor §4.4 describes. It reuses the constructed context (P5b) and the cwd seam (P5a). One at a time (P5c runs one; P5d guards single-active).
- **Type consistency:** `TurnConfig { system, cwd, branch }` (T1) is threaded into `build_request`/`run_tool`/`emit` and consumed by `run_subagent` (T2). `run_agent_turn(config, …) -> Result<Vec<Event>>` (T1) is what `run_subagent` calls and distills. `SubagentResult { branch, summary, ok }` (T2) is the type P5d folds into the main log.
- **DRY:** no second loop — the generalization makes Chat a `TurnConfig` and a subagent another. The constructed prompt is reused as a seed `UserMessage`, so the existing `conversation` → `build_request` fold builds every subagent request too.
- **Deferred to P5d:** the single-active-subagent guard, the `SubagentResult` event + branch-filtered `conversation()`, and the folded card — P5c returns the result value; P5d records and renders it.
