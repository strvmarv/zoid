# `:compact` Command Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `:compact` command (and palette entry) that explicitly triggers context compaction on demand, reusing the existing `plan_compactions` + `record_compactions` machinery and the existing `CompactionStarted`/`CompactionComplete` `AgentUpdate` variants.

**Spec:** `docs/superpowers/specs/2026-07-06-compact-command-design.md`

**Tech Stack:** Rust 2021, tokio. Workspace tested via `cargo test --workspace`, linted via `cargo clippy --workspace --all-targets -- -D warnings`, formatted via `cargo fmt`.

---

## File Structure

- `crates/zoid-tui/src/command.rs` — add `Command::CompactNow` variant + `"compact"` parser arm.
- `crates/zoid-tui/src/palette.rs` — add `"compact"` entry to `all_items`.
- `crates/zoid/src/main.rs` — add `Command::CompactNow` arm to `exec_command`: guard + spawn async compaction task.

---

## Task 1: Command variant + parser

**Files:** `crates/zoid-tui/src/command.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-tui/src/command.rs`:

```rust
    #[test]
    fn parses_compact_command() {
        assert_eq!(parse_command(":compact"), Command::CompactNow);
        assert_eq!(parse_command("compact"), Command::CompactNow);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib command::tests::parses_compact`
Expected: FAIL — `CompactNow` variant doesn't exist.

- [ ] **Step 3: Add the variant + parser arm**

In `crates/zoid-tui/src/command.rs`:

Add to the `Command` enum (before `Unknown`):

```rust
    /// Explicitly trigger context compaction on the current event log.
    CompactNow,
```

Add to `parse_command` (before the `other =>` catch-all):

```rust
        "compact" => Command::CompactNow,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --lib command::tests::parses_compact`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/command.rs
git commit -m "feat(command): CompactNow variant + :compact parser"
```

---

## Task 2: Palette entry

**Files:** `crates/zoid-tui/src/palette.rs`

- [ ] **Step 1: Find the `all_items` function and its existing entries**

Run: `grep -n "pub fn all_items" crates/zoid-tui/src/palette.rs`
Read the function to see the pattern for adding an entry.

- [ ] **Step 2: Add the "compact" entry**

Add a `PaletteItem` for "compact" in `all_items`, alongside the existing entries (companion, delegate, etc.):

```rust
        PaletteItem {
            label: "compact".into(),
            command: Command::CompactNow,
        },
```

- [ ] **Step 3: Build the crate**

Run: `cargo build -p zoid-tui`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): add 'compact' to the fuzzy command list"
```

---

## Task 3: Bin — `exec_command` arm (spawn async compaction task)

**Files:** `crates/zoid/src/main.rs`

This is the core task. It adds the `Command::CompactNow` arm to `exec_command` and spawns an async task that calls `plan_compactions`, emits the compaction events, and sends `CompactionStarted`/`CompactionComplete`.

- [ ] **Step 1: Write the failing integration test**

Add to `crates/zoid/src/main.rs` tests:

```rust
    #[tokio::test]
    async fn compact_command_emits_compaction_events() {
        use zoid_core::event::{Event, EventKind, TokenStat};
        use zoid_core::compaction::plan_compactions;
        use zoid_core::context::ContextOverhead;

        let mut app = test_app().await;
        // plan_compactions returns an empty plan when compact_threshold_pct
        // is 0 (the default). Set a non-zero threshold so the large tool result
        // below is eligible for compaction.
        app.economy.compact_threshold_pct = 50;
        // Seed an event log with at least one uncompacted tool result.
        let tc_id = Ulid::new();
        app.record(EventKind::UserMessage { text: "do something".into() }).await.unwrap();
        app.record(EventKind::ToolCall {
            id: tc_id.to_string(),
            name: "shell".into(),
            args: "{}".into(),
        }).await.unwrap();
        app.record(EventKind::ToolResult {
            id: tc_id.to_string(),
            name: "shell".into(),
            output: "x".repeat(5000), // large enough to be worth compacting
            is_error: false,
        }).await.unwrap();

        // Run :compact.
        let quit = exec_command(&mut app, zoid_tui::command::Command::CompactNow).await.unwrap();
        assert!(!quit);

        // Give the spawned task time to run.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // At least one ToolResultCompacted event should now be in the log.
        assert!(
            app.events.iter().any(|e| matches!(e.kind, EventKind::ToolResultCompacted { .. })),
            ":compact must emit at least one ToolResultCompacted event"
        );
    }

    #[tokio::test]
    async fn compact_command_blocked_while_already_compacting() {
        let mut app = test_app().await;
        app.shell.compacting = true;
        let quit = exec_command(&mut app, zoid_tui::command::Command::CompactNow).await.unwrap();
        assert!(!quit);
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("already compacting"),
            ":compact while compacting should surface the hint"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid --bin zoid compact_command`
Expected: FAIL — `Command::CompactNow` arm doesn't exist in `exec_command`.

- [ ] **Step 3: Add the `CompactNow` arm to `exec_command`**

In `crates/zoid/src/main.rs`, in `exec_command`, find the `Command::Unknown(_) => Ok(false)` arm and add before it:

```rust
        Command::CompactNow => {
            if app.shell.compacting {
                app.shell.status_hint = Some("already compacting".into());
                return Ok(false);
            }
            // Spawn the compaction task (non-blocking; chat turns are not blocked).
            let session = app.session.clone();
            let session_id = app.session_id;
            let ui_tx = app.ui_tx.clone();
            let events = app.events.snapshot();
            let policy = app.economy.clone();
            let context_target = app.context_target;
            let overhead = {
                let system_tokens = zoid_core::economy::estimate_tokens(
                    &app.base_profile.system_prompt,
                );
                let tools_tokens: u64 = zoid::invoke_skill::chat_tools(
                    app.skills.clone(),
                )
                .iter()
                .map(|t| {
                    let spec = t.spec();
                    let spec_str = format!(
                        "{}\n{}\n{}",
                        spec.name,
                        spec.description,
                        serde_json::to_string(&spec.parameters).unwrap_or_default()
                    );
                    zoid_core::economy::estimate_tokens(&spec_str)
                })
                .sum();
                zoid_core::context::ContextOverhead {
                    system_tokens,
                    tools_tokens,
                }
            };
            // Build the policy for plan_compactions (same as the agent loop's
            // record_compactions path).
            let ctx_policy = policy_from_config(&app.economy, context_target);
            tokio::spawn(async move {
                let plan = zoid_core::compaction::plan_compactions(
                    events.iter(),
                    &ctx_policy,
                    None, // no real_input_tokens (no in-flight turn)
                    None, // no calibration ratio
                    &overhead,
                );
                let _ = ui_tx.send(AgentUpdate::CompactionStarted).await;
                for c in &plan.compactions {
                    let ev = Event::new(
                        Ulid::new(),
                        None,
                        now_ms(),
                        EventKind::ToolResultCompacted {
                            id: c.id.clone(),
                            summary: c.summary.clone(),
                            original_tokens: c.original_tokens,
                        },
                    )
                    .with_session(session_id);
                    let _ = session.append(ev.clone()).await;
                    let _ = ui_tx.send(AgentUpdate::Appended(Box::new(ev))).await;
                }
                let _ = ui_tx.send(AgentUpdate::CompactionComplete).await;
            });
            Ok(false)
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid --bin zoid compact_command`
Expected: PASS — both tests green.

- [ ] **Step 5: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): :compact command spawns explicit compaction task"
```

---

## Task 4: clippy/fmt + full workspace test

- [ ] **Step 1: clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 2: fmt**

Run: `cargo fmt --all`
Revert any pre-existing drift in files not touched by this plan.

- [ ] **Step 3: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS — all suites green.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: clippy + fmt for :compact command"
```

---

## Self-Review

**1. Spec coverage:**
- Command variant + parser → Task 1.
- Palette entry → Task 2.
- exec_command arm (guard + spawn) → Task 3.
- Empty plan → emits CompactionStarted + CompactionComplete (no Appended in between) → Task 3 (the spawned task sends both unconditionally).
- Already compacting → guard + hint → Task 3.
- Non-blocking → Task 3 (tokio::spawn, no Submit guard change).
- Calibration None → Task 3 (passed as None to plan_compactions).

**2. Placeholder scan:** No TBD/TODO. The overhead computation in Task 3 mirrors the agent loop's overhead computation exactly.

**3. Task ordering:** Task 1 (command) compiles standalone. Task 2 (palette) depends on Task 1 (references `Command::CompactNow`). Task 3 (bin) depends on Tasks 1–2. Task 4 is verify-only.