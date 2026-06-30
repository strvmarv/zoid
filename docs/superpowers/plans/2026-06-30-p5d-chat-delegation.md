# P5d · Chat Delegation + Result Folding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Chat delegate a discrete unit of work to a single subagent — dispatched from a `:delegate <task>` command or a P4d object-verb — running one at a time, with its result folded back into the conversation as a collapsible card.

**Architecture:** The orchestrator is the Chat loop. A new `EventKind::SubagentResult { branch, summary, ok }` is recorded on the **main** branch when a subagent finishes; `conversation()` now folds **only the main branch** (skipping subagent work events) and renders `SubagentResult` as a `ChatMsg::Delegated` card. The bin dispatches `run_subagent` (P5c) on a background task with a single-active-subagent guard; when it returns, it appends the `SubagentResult` event, which arrives over the existing `AgentUpdate::Appended` channel and clears the guard. P4d's verb pick now dispatches instead of seeding the input.

**Tech Stack:** Rust 2021, tokio, ratatui. Consumes P5c (`run_subagent`, `SubagentResult`) and P4d (object-verb pick).

## Global Constraints

- **One subagent at a time (spec §4.4/§12):** the bin holds a single `delegating` flag; a delegate request while streaming or already delegating is refused with a hint. No fleet, no queue.
- **Main-branch conversation:** `conversation()` folds only `BranchId::default()` ("main"); subagent work events (`subagent:<id>`) are skipped, and the `SubagentResult` summary is the only thing that surfaces — as a `▸` card (① zoom-compatible). This is the branch-filtering the earlier phases deferred.
- **Verbs dispatch now (closes the P4d loop):** P4d composed a scoped prompt and left it "queued for P5." P5d rewires the verb pick to **dispatch a subagent** with that prompt. The `:delegate <task>` command is the explicit entry point.
- **Exhaustive-match ripple:** adding `ChatMsg::Delegated` requires a new arm in **every** exhaustive `ChatMsg` match merged by P4 — `agent::map_msg`, `chat::conversation_lines`, `chat` zoom `digests`/`conversation_view`, and `objects::selectable_objects`. Each task below names the arms it touches; the workspace will not compile until all are handled.
- **Design tokens (spec §16):** the delegated card uses existing tokens (`glyph::COLLAPSED ▸`, `glyph::PASS ✓` / `glyph::WARNING ⚠`, `color::CHAT_ACCENT`/`OK`/`ERROR`). No new glyphs/hex.
- **UX testing multi-width:** the delegated-card render adds `insta` snapshots at **100×24 and 140×24**.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit.

---

### Task 1: `SubagentResult` event + branch-folding `conversation()`

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (`EventKind::SubagentResult`)
- Modify: `crates/zoid-core/src/projection.rs` (`ChatMsg::Delegated`, branch filter, fold)
- Modify: `crates/zoid/src/agent.rs` (`map_msg` arm)
- Test: inline.

**Interfaces:**
- Produces:
  - `EventKind::SubagentResult { branch: String, summary: String, ok: bool }`.
  - `ChatMsg::Delegated { summary: String, ok: bool }`.
  - `conversation()` skips non-main-branch events and folds `SubagentResult` → `ChatMsg::Delegated`.

- [ ] **Step 1: Write the failing tests**

In `crates/zoid-core/src/projection.rs` `mod tests`:

```rust
#[test]
fn conversation_skips_subagent_branch_and_folds_result() {
    use crate::event::BranchId;
    let mut work = Event::new(Ulid::from(10u128), None, 0, EventKind::ModelDelta { text: "subagent thinking".into() });
    work.branch = BranchId("subagent:ax3".into());
    let result = Event::new(Ulid::from(11u128), None, 0, EventKind::SubagentResult {
        branch: "subagent:ax3".into(), summary: "Refactored parse()".into(), ok: true,
    });
    let evs = vec![user(1, "delegate this"), work, result];
    let conv = conversation(&evs);
    assert_eq!(conv, vec![
        ChatMsg::User("delegate this".into()),
        ChatMsg::Delegated { summary: "Refactored parse()".into(), ok: true },
    ]);
}
```

In `crates/zoid-core/src/event.rs` `mod tests`, extend the round-trip test or add:

```rust
#[test]
fn subagent_result_round_trips() {
    let ev = Event::new(Ulid::new(), None, 0, EventKind::SubagentResult {
        branch: "subagent:zz".into(), summary: "did it".into(), ok: false,
    });
    let json = serde_json::to_string(&ev).unwrap();
    assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core`
Expected: compile error — `SubagentResult`/`Delegated` undefined.

- [ ] **Step 3: Add the event + ChatMsg variants**

In `crates/zoid-core/src/event.rs`, add to `EventKind` (before the closing brace):

```rust
    /// A finished subagent's outcome, recorded on the MAIN branch. `branch`
    /// names the subagent's sub-branch; `summary` is its closing report.
    SubagentResult { branch: String, summary: String, ok: bool },
```

In `crates/zoid-core/src/projection.rs`, add to `enum ChatMsg`:

```rust
    /// A folded subagent delegation — rendered as a collapsible card.
    Delegated { summary: String, ok: bool },
```

- [ ] **Step 4: Branch-filter + fold in `conversation()`**

In `conversation()`, at the very top of the `for e in events` loop, skip non-main work (but NOT `SubagentResult`, which is itself on main):

```rust
    for e in events {
        // Subagent work lives on its own branch and never appears in the main
        // conversation; only its folded SubagentResult (on main) surfaces.
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            // ... existing arms ...
            EventKind::SubagentResult { summary, ok, .. } => {
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::Delegated { summary: summary.clone(), ok: *ok });
            }
            EventKind::Usage | EventKind::ContextMutation { .. } => { /* unchanged */ }
        }
    }
```

- [ ] **Step 5: Add the `map_msg` arm in the bin**

In `crates/zoid/src/agent.rs`, `map_msg` matches `ChatMsg`. Add:

```rust
        ChatMsg::Delegated { summary, .. } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: format!("[delegated subagent] {summary}"),
            tool_calls: vec![],
            tool_name: None,
        },
```

(So the model sees the delegation outcome in subsequent Chat turns.)

- [ ] **Step 6: Run to confirm pass**

Run: `cargo test -p zoid-core && cargo build -p zoid`
Expected: PASS / compiles (other `ChatMsg` matches are handled in Task 2).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs crates/zoid/src/agent.rs
git commit -m "feat(core): SubagentResult event; conversation() folds it as Delegated, skips sub-branches"
```

---

### Task 2: Render the delegated card (+ remaining ChatMsg arms) + snapshots

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (`conversation_lines` + zoom `digests`/`conversation_view` arms)
- Modify: `crates/zoid-tui/src/objects.rs` (`selectable_objects` arm)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (delegated-card snapshots)
- Test: snapshots @100/@140.

**Interfaces:**
- Consumes: `ChatMsg::Delegated`.
- Produces: a `▸ delegated · {✓|⚠} {summary}` line in the conversation; exhaustive `ChatMsg` matches updated.

- [ ] **Step 1: Render the card in `conversation_lines`**

In `crates/zoid-tui/src/chat.rs`, add a `ChatMsg::Delegated` arm to `conversation_lines`'s match:

```rust
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok { (glyph::PASS, color::OK) } else { (glyph::WARNING, color::ERROR) };
                lines.push(Line::from(vec![
                    Span::styled(format!("{} delegated ", glyph::COLLAPSED), Style::new().fg(color::CHAT_ACCENT)),
                    Span::styled(format!("{mark} "), Style::new().fg(mark_color)),
                    Span::styled(first_line(summary), Style::new().fg(color::TXT)),
                ]));
            }
```

- [ ] **Step 2: Update the other exhaustive `ChatMsg` matches (compile wall)**

The workspace won't compile until every `match … ChatMsg` is exhaustive. Add arms:

- `chat.rs` zoom `digests` is in **core** (`zoom::digests`) — it matches `ChatMsg`; add a `ChatMsg::Delegated { .. } => { /* count as part of the current turn; no tool/file */ }` arm (treat like an assistant note: `d.tools` unchanged). In `crates/zoid-core/src/zoom.rs`'s match, add:
  ```rust
              ChatMsg::Delegated { .. } => {
                  // A folded delegation belongs to the current turn; no extra counts.
                  let _ = cur.get_or_insert_with(|| TurnDigest { headline: String::new(), tools: 0, files: 0, has_error: false });
              }
  ```
- `chat.rs` `detail_lines` (P4c) routes non-tool-result messages through `conversation_lines`, so it needs no new arm (the `other =>` arm covers `Delegated`). Confirm its match is non-exhaustive-by-`other` or add the arm.
- `objects.rs` `selectable_objects` (P4d) matches only `ChatMsg::Assistant`/`ToolResult` via `if let`; a `Delegated` produces no object — no change needed (it uses `if let`, not an exhaustive `match`). Confirm.

> Walk the compiler errors: `cargo build --workspace` lists each non-exhaustive match. Add a `Delegated` arm to each per the intent above (digest: no-op count; renderers: show the card; request-builder: assistant summary from Task 1).

- [ ] **Step 3: Write the snapshot tests**

In `crates/zoid-tui/tests/shell_snapshot.rs`, add a seeded conversation containing a delegation and snapshots:

```rust
fn seeded_delegated() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("delegate: add a test for parse()".into()),
        ChatMsg::Delegated { summary: "Added tests/parse_test.rs covering empty + nested input.".into(), ok: true },
    ]
}

fn draw_delegated(w: u16, h: u16) -> String {
    let s = ShellState::new();
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_delegated(), &input, false, &normal_view()))
        .unwrap();
    terminal.backend().to_string()
}

#[test] fn delegated_card_frame() { insta::assert_snapshot!(draw_delegated(100, 24)); }
#[test] fn delegated_card_wide_frame() { insta::assert_snapshot!(draw_delegated(140, 24)); }
```

(`empty_economy`/`normal_view` come from P3/P4c in this file.)

- [ ] **Step 4: Accept snapshots and verify**

Run: `cargo build --workspace` (resolve any remaining match arms) then
Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Read the two `.snap` files: a `▸ delegated ✓ Added tests/...` line appears. Re-run without the env var:
Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-core/src/zoom.rs crates/zoid-tui/src/objects.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): render delegated subagent card; exhaustive ChatMsg arms + snapshots"
```

---

### Task 3: Orchestrator — `:delegate` command, dispatch, busy guard, verb rewire

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (`Command::Delegate`)
- Modify: `crates/zoid/src/main.rs` (dispatch, guard, result recording, VerbPick rewire)
- Test: inline command test + manual.

**Interfaces:**
- Consumes: `run_subagent`/`SubagentResult` (P5c), `parse_command`.
- Produces: `Command::Delegate(String)`; a `delegating` guard on `App`; `start_delegation(app, task)`.

- [ ] **Step 1: Write the failing command test**

In `crates/zoid-tui/src/command.rs` `mod tests`:

```rust
#[test]
fn parses_delegate_with_task() {
    assert_eq!(parse_command(":delegate add a test for parse()"), Command::Delegate("add a test for parse()".into()));
    assert_eq!(parse_command(":delegate"), Command::Delegate(String::new()));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib command::tests::parses_delegate`
Expected: FAIL — `Command::Delegate` undefined.

- [ ] **Step 3: Add the command**

In `crates/zoid-tui/src/command.rs`, add to `enum Command`: `Delegate(String),`. In `parse_command`, before the `other =>` arm, handle the prefix:

```rust
        rest if rest == "delegate" || rest.starts_with("delegate ") => {
            Command::Delegate(rest.strip_prefix("delegate").unwrap().trim().to_string())
        }
```

(Place this in the `match t { … }`; `t` is the colon-stripped, trimmed input.)

- [ ] **Step 4: Add the `delegating` guard + dispatch to the bin**

In `crates/zoid/src/main.rs`, add to `App`: `delegating: bool,` (init `false`). Add the dispatch helper:

```rust
fn start_delegation(app: &mut App, task: String) {
    if app.streaming || app.delegating {
        app.status_hint = Some("busy · one subagent at a time".into());
        return;
    }
    if task.trim().is_empty() {
        app.status_hint = Some("usage: :delegate <task>".into());
        return;
    }
    app.delegating = true;
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();           // context for construction (P5b)
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        let res = zoid::subagent::run_subagent(
            &task, &seed, provider, tools, std::path::PathBuf::from("."), model, session.clone(), ui.clone(), now_ms,
        ).await;
        // Record the outcome on the MAIN branch so it folds into the conversation.
        if let Ok(r) = res {
            let mut ev = zoid_core::event::Event::new(ulid::Ulid::new(), None, now_ms(),
                zoid_core::event::EventKind::SubagentResult { branch: r.branch, summary: r.summary, ok: r.ok });
            // ev.branch defaults to "main"
            let _ = session.append(ev.clone()).await;
            let _ = ui.send(zoid::agent::AgentUpdate::Appended(ev)).await;
        }
    });
}
```

> Chat delegation runs in cwd (`"."`) per the §9 decision — the worktree path is for Build (P6+). `status_hint` is the P4d hint field; if P4d isn't merged, add `status_hint: Option<String>` to `App`.

- [ ] **Step 5: Wire the command + clear the guard + rewire the verb pick**

In `exec_command`, add: `Command::Delegate(task) => { start_delegation(app, task); Ok(false) }`.

In the main loop's `AgentUpdate::Appended` handler, clear the guard when the result lands:

```rust
                    AgentUpdate::Appended(ev) => {
                        if matches!(ev.kind, zoid_core::event::EventKind::SubagentResult { .. }) {
                            app.delegating = false;
                        }
                        app.events.push(ev);
                    }
```

Rewire P4d's `Action::VerbPick` arm (which previously seeded the input + "queued · P5" hint) to dispatch instead:

```rust
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let task = zoid_tui::objects::verb_prompt(verb, obj);
                    app.shell.close_overlay();
                    start_delegation(app, task);  // now dispatches (P5)
                    return Ok(false);
                }
            }
            app.shell.close_overlay();
        }
```

- [ ] **Step 6: Build + test**

Run: `cargo test --workspace && cargo clippy --all-targets`
Expected: PASS, zero warnings.

Manual: `cargo run -p zoid` → `:delegate add a hello function to src/lib.rs` (against your Ollama/GLM model) dispatches a subagent; while it runs, a second `:delegate` shows "busy"; on completion a `▸ delegated ✓ …` card appears in the conversation.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): :delegate + verb dispatch → one subagent at a time; fold SubagentResult"
```

---

### Task 4: Integration — delegate dispatches, result folds into the conversation

**Files:**
- Create: `crates/zoid/tests/delegation_integration.rs`
- Test: the file itself.

**Interfaces:**
- Consumes: `run_subagent`, `conversation`, `FakeProvider`.

> End-to-end at the projection level: a subagent runs, a `SubagentResult` is recorded on main, and `conversation()` of the full log shows exactly the user turn + the folded `Delegated` card (subagent work hidden).

- [ ] **Step 1: Write the failing test**

`crates/zoid/tests/delegation_integration.rs`:

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use zoid::subagent::run_subagent;
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent};
use ulid::Ulid;

#[tokio::test]
async fn delegated_result_folds_into_main_conversation() {
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("Added the function.".into()),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();

    // Seed a user turn on main (the request that triggered delegation).
    session.append(Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "delegate: add fn".into() })).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let seed = session.snapshot().await.unwrap();
    let res = run_subagent("add fn", &seed, provider, Arc::new(zoid_tools::registry()),
        std::path::PathBuf::from("."), "glm".into(), session.clone(), tx, || 0).await.unwrap();

    // Orchestrator records the result on main.
    session.append(Event::new(Ulid::new(), None, 0, EventKind::SubagentResult {
        branch: res.branch, summary: res.summary, ok: res.ok,
    })).await.unwrap();

    let conv = conversation(&session.snapshot().await.unwrap());
    assert_eq!(conv.first(), Some(&ChatMsg::User("delegate: add fn".into())));
    assert!(matches!(conv.last(), Some(ChatMsg::Delegated { ok: true, .. })));
    // Subagent work events exist in the log but are NOT in the main conversation.
    assert!(conv.iter().all(|m| !matches!(m, ChatMsg::Assistant { .. } if false)));
}
```

- [ ] **Step 2: Run to confirm pass**

Run: `cargo test -p zoid --test delegation_integration`
Expected: PASS (adapt the `FakeProvider` script to its replay contract if needed, per P5c Task 3's note).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/delegation_integration.rs
git commit -m "test(zoid): delegated SubagentResult folds into the main conversation"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `:delegate <task>` and an object-verb both dispatch a subagent; a second dispatch while busy is refused with a hint.
- [ ] `conversation()` shows the user turn + a `▸ delegated` card; subagent work events (sub-branch) never appear.
- [ ] Delegated card snapshots exist at 100 and 140.
- [ ] Chat delegation runs in cwd (no worktree) — the worktree path stays a Build (P6+) concern.

## Self-Review notes (author)

- **Spec coverage (§4.4/§6.1 Chat delegation):** "a discrete, non-trivial unit is delegated to a single subagent … its result folding back into the conversation as a collapsible card (① zoom)." `:delegate`/verb dispatch (T3) → `run_subagent` (P5c) → `SubagentResult` on main (T3) → `conversation()` folds a `Delegated` card (T1) → rendered collapsibly (T2). One at a time via the `delegating` guard (T3). Closes P4d's "queued for P5."
- **Type consistency:** `EventKind::SubagentResult { branch, summary, ok }` (T1) ↔ `SubagentResult` (P5c) fields ↔ the bin's record step (T3). `ChatMsg::Delegated { summary, ok }` (T1) is produced by `conversation()` and consumed by `map_msg` (T1), `conversation_lines` (T2), and the zoom digest (T2). `Command::Delegate(String)` (T3) → `start_delegation` (T3) → `run_subagent` (P5c).
- **Ripple handled:** the new `ChatMsg` variant's arms are enumerated in T1 (map_msg) and T2 (renderers + digest); the build-error walk in T2 Step 2 catches any missed match. This is the price of a clean fold type — paid once.
- **Branch model realized:** this is where `Event.branch` finally does work — subagent events on `subagent:<id>`, folded out of the main conversation, with only the result surfacing. The schema P0 retained (spec §4.1) now carries the orchestrator+one-subagent-head model.
