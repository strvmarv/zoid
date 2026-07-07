# Compaction status animation implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an animated purple compaction indicator to the status bar, shown only while automated compaction is running, with a 3s minimum display duration.

**Architecture:** Two new `AgentUpdate` variants (`CompactionStarted`/`CompactionComplete`) bracket the agent loop's compaction calls. The bin stores the start time on `App` and enforces a 3s minimum display via a per-frame debounce check. A new `COMPACT_SPINNER` glyph token (6-frame box-shuffle) animates at 120ms in `color::BRANCH` (purple). `render_status` appends the segment right of the center activity indicator — no re-centering math changes.

**Tech Stack:** Rust, ratatui (`Line`, `Span`, `Style`), `zoid-tui` tokens (`color::BRANCH`, new `glyph::COMPACT_SPINNER`), `zoid-tui::motion::spinner_frame`.

## Global Constraints

- **Design tokens (spec §16):** every color and glyph comes from `crates/zoid-tui/src/tokens.rs` — never hardcode a color or character literal. Use `color::BRANCH` (purple `#bc8cff`) for the compaction indicator.
- **`COMPACT_SPINNER` ramp:** `['⊟', '⊞', '⊟', '⊕', '⊞', '⊕']` — 6 frames, animated at 120ms.
- **Minimum display duration:** 3 seconds from `CompactionStarted`, enforced by the bin via `App.compaction_started_at: Option<std::time::Instant>` + `App.compaction_complete: bool`. `CompactionComplete` does not immediately clear `compacting` — the per-frame debounce check does.
- **No re-centering:** the compaction segment is appended after the center activity indicator; the existing `pad2` centering math shrinks to accommodate it.
- **`AgentUpdate` is `zoid`-crate type:** the enum lives in `crates/zoid/src/agent.rs`. Tests that match on it live in `crates/zoid/tests/`.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid-tui/src/tokens.rs` | New `glyph::COMPACT_SPINNER` token + test |
| `crates/zoid-tui/src/state.rs` | New `ShellState` fields: `compacting`, `compact_spinner` + tests |
| `crates/zoid-tui/src/render.rs` | `render_status` appends compaction segment |
| `crates/zoid/src/agent.rs` | New `AgentUpdate` variants + emit inside `record_compactions`/`preflight_gate` (gated on non-empty plan) |
| `crates/zoid/src/main.rs` | New `App` fields, per-frame spinner, debounce check, `AgentUpdate` handler arms, motion tick guard |
| `crates/zoid/tests/economy_integration.rs` | Assert `CompactionStarted`/`CompactionComplete` received |

---

## Task 1: Add `COMPACT_SPINNER` glyph token

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs`
- Test: `crates/zoid-tui/src/tokens.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing
- Produces: `pub const COMPACT_SPINNER: [char; 6]` in `tokens::glyph`

- [ ] **Step 1: Write the failing test**

Add this test to the existing `#[cfg(test)] mod tests` block in `tokens.rs` (after the `acm1_compact_token_present` test):

```rust
    #[test]
    fn compaction_spinner_token_present() {
        assert_eq!(
            glyph::COMPACT_SPINNER,
            ['⊟', '⊞', '⊟', '⊕', '⊞', '⊕']
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib tokens::tests::compaction_spinner_token_present`
Expected: FAIL — `no field or associated item named COMPACT_SPINNER`

- [ ] **Step 3: Add the token**

In `crates/zoid-tui/src/tokens.rs`, in `pub mod glyph`, add after the `pub const COMPACT: char = '⊟';` line:

```rust
    /// Compaction status spinner — a 6-frame box-shuffle ramp, animated at ~120ms
    /// (slower than the working spinner, signaling a different kind of work).
    /// Purple (color::BRANCH). Only shown while automated compaction is running.
    pub const COMPACT_SPINNER: [char; 6] = ['⊟', '⊞', '⊟', '⊕', '⊞', '⊕'];
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib tokens::tests::compaction_spinner_token_present`
Expected: PASS

- [ ] **Step 5: Run full zoid-tui lib test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs
git commit -m "feat(tui): add COMPACT_SPINNER glyph token (6-frame box-shuffle)"
```

---

## Task 2: Add `compacting` and `compact_spinner` fields to `ShellState`

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (struct `ShellState` + `new()`)
- Test: `crates/zoid-tui/src/state.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::tokens::glyph::COMPACT_SPINNER` (from Task 1)
- Produces: `ShellState.compacting: bool` (defaults `false`), `ShellState.compact_spinner: char` (defaults `glyph::COMPACT_SPINNER[0]`)

- [ ] **Step 1: Write the failing test**

Add this test to the existing `#[cfg(test)] mod tests` block in `state.rs` (after the `first_time_user_defaults_false` test):

```rust
    #[test]
    fn compacting_defaults_false() {
        let s = ShellState::new();
        assert!(!s.compacting, "compacting must default to false");
        assert_eq!(
            s.compact_spinner,
            crate::tokens::glyph::COMPACT_SPINNER[0],
            "compact_spinner must default to the first frame"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib state::tests::compacting_defaults_false`
Expected: FAIL — `no field named compacting on type ShellState`

- [ ] **Step 3: Add the fields and defaults**

In the `ShellState` struct definition, add after `pub first_time_user: bool,`:

```rust
    /// Whether automated compaction is currently running. Set by the bin from
    /// `AgentUpdate::CompactionStarted`; cleared by the per-frame debounce check
    /// after `CompactionComplete` + the 3s minimum display duration. Pure
    /// renderer reads this to show/hide the compaction indicator.
    pub compacting: bool,
    /// Current compaction-spinner frame glyph, refreshed by the bin each frame
    /// from wall-clock elapsed at ~120ms. Defaults to the first frame so
    /// snapshot tests are deterministic unless they opt into `compacting`.
    pub compact_spinner: char,
```

In `ShellState::new()`, add the defaults after `first_time_user: false,` in the `Self { ... }` block:

```rust
            compacting: false,
            compact_spinner: crate::tokens::glyph::COMPACT_SPINNER[0],
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib state::tests::compacting_defaults_false`
Expected: PASS

- [ ] **Step 5: Run full zoid-tui lib test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): add compacting + compact_spinner fields to ShellState"
```

---

## Task 3: Render the compaction indicator in `render_status`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (function `render_status`)
- Test: `crates/zoid-tui/src/render.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ShellState.compacting: bool`, `ShellState.compact_spinner: char`, `color::BRANCH` (from Tasks 1+2)
- Produces: the compaction segment rendered in the status bar when `state.compacting` is true

- [ ] **Step 1: Write the failing test**

Add a test module to the bottom of `crates/zoid-tui/src/render.rs` (the file currently has no `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use crate::state::Zoom;

    /// Render a status bar with `compacting: true` and verify the compaction
    /// segment appears in `color::BRANCH` (purple). The compaction spinner
    /// glyph and "compacting" label must both be present.
    #[test]
    fn compaction_segment_visible_when_compacting() {
        let mut state = ShellState::new();
        state.compacting = true;
        state.compact_spinner = glyph::COMPACT_SPINNER[0];
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &state, &view, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.c())
            .collect();
        assert!(
            content.contains("compacting"),
            "status bar must contain 'compacting' when state.compacting is true: got {content:?}"
        );
        assert!(
            content.contains(glyph::COMPACT_SPINNER[0].to_string().as_str()),
            "status bar must contain the compaction spinner glyph: got {content:?}"
        );
        // The spinner glyph is rendered in BRANCH (purple).
        let has_branch = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.fg() == Some(color::BRANCH));
        assert!(
            has_branch,
            "at least one cell must use color::BRANCH (purple) for the compaction indicator"
        );
    }

    /// When `compacting: false`, the compaction segment must NOT appear —
    /// the status bar is byte-identical to the pre-feature layout.
    #[test]
    fn compaction_segment_absent_when_not_compacting() {
        let state = ShellState::new();
        assert!(!state.compacting, "compacting must default to false");
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &state, &view, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.c())
            .collect();
        assert!(
            !content.contains("compacting"),
            "status bar must NOT contain 'compacting' when not compacting: got {content:?}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib render::tests`
Expected: FAIL — `compaction_segment_visible_when_compacting` fails: "compacting" not found in the status bar output (the segment isn't rendered yet)

- [ ] **Step 3: Add the compaction segment to `render_status`**

In `crates/zoid-tui/src/render.rs`, find `render_status`. After the line that pushes the center segment:

```rust
    spans.push(Span::styled(center, Style::new().fg(fg)));
```

Insert immediately after it (before the `let pad2 = ...` line):

```rust
    // Compaction indicator — only while compaction is running. Appended right
    // after the center segment, no re-centering (the pad2 calculation below
    // shrinks to accommodate this extra width, and the zoom hint stays pinned
    // to the right edge).
    if state.compacting {
        spans.push(Span::styled(
            format!("  {} compacting", state.compact_spinner),
            Style::new().fg(color::BRANCH),
        ));
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib render::tests`
Expected: PASS (both tests)

- [ ] **Step 5: Run the full zoid-tui lib test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (existing snapshot tests don't set `compacting`, so the segment is absent — byte-identical output)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(tui): render compaction indicator in status bar (purple, right of center)

Adds a TestBackend render test verifying the 'compacting' segment appears
in color::BRANCH when state.compacting is true, and is absent when false."
```

---

## Task 4: Add `CompactionStarted` / `CompactionComplete` to `AgentUpdate` and emit them

**Files:**
- Modify: `crates/zoid/src/agent.rs` (enum `AgentUpdate`, function `record_compactions`)
- Test: `crates/zoid/tests/economy_integration.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `AgentUpdate::CompactionStarted`, `AgentUpdate::CompactionComplete` — emitted inside `record_compactions` only when compactions are non-empty

**Design note:** `CompactionStarted`/`CompactionComplete` are emitted **inside** `record_compactions` (not at the call sites), gated on `!plan.compactions.is_empty()`. This avoids a false "compacting" signal when the plan is empty (no compactions needed). The `preflight_gate` site has its own separate `plan_compactions` call (not via `record_compactions`) — it emits its own pair, also gated on non-empty plan.

- [ ] **Step 1: Write the failing test**

In `crates/zoid/tests/economy_integration.rs`, add a new test after the existing `oversized_tool_result_is_compacted_when_over_threshold` test:

```rust
#[tokio::test]
async fn compaction_emits_started_and_complete_updates() {
    let command =
        "for i in $(seq 1 2000); do echo \"line $i: filler text to pad out tokens\"; done";

    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("shell", serde_json::json!({ "command": command })),
        zoid_testkit::text("done"),
        ProviderEvent::Done,
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage {
            text: "run the big command".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    let mut config = zoid::agent::chat_turn_config();
    config.policy = ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: false,
        compact_threshold: Some(50),
    };

    let handle = tokio::spawn(async move {
        run_agent_turn(
            config,
            provider,
            Arc::new(zoid_tools::registry()),
            Arc::new(zoid_tools::AllowAll),
            session,
            zoid::eventlog::EventLog::from_vec(seed),
            "fake".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            now,
        )
        .await
        .unwrap();
    });

    let mut saw_started = false;
    let mut saw_complete = false;
    while let Some(update) = rx.recv().await {
        match update {
            AgentUpdate::CompactionStarted => saw_started = true,
            AgentUpdate::CompactionComplete => saw_complete = true,
            _ => {}
        }
    }
    let _ = handle.await;

    assert!(saw_started, "CompactionStarted must be emitted");
    assert!(saw_complete, "CompactionComplete must be emitted");
}

#[tokio::test]
async fn compaction_does_not_emit_updates_when_nothing_compacted() {
    // A turn with a tiny tool-result well below the compaction threshold —
    // plan_compactions returns empty, so no CompactionStarted/Complete.
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("shell", serde_json::json!({ "command": "echo hi" })),
        zoid_testkit::text("done"),
        ProviderEvent::Done,
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage {
            text: "run the tiny command".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    let mut config = zoid::agent::chat_turn_config();
    config.policy = ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: false,
        compact_threshold: Some(50),
    };

    let handle = tokio::spawn(async move {
        run_agent_turn(
            config,
            provider,
            Arc::new(zoid_tools::registry()),
            Arc::new(zoid_tools::AllowAll),
            session,
            zoid::eventlog::EventLog::from_vec(seed),
            "fake".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            now,
        )
        .await
        .unwrap();
    });

    let mut saw_any_compaction = false;
    while let Some(update) = rx.recv().await {
        match update {
            AgentUpdate::CompactionStarted | AgentUpdate::CompactionComplete => {
                saw_any_compaction = true;
            }
            _ => {}
        }
    }
    let _ = handle.await;

    assert!(
        !saw_any_compaction,
        "no CompactionStarted/Complete when nothing was compacted"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --test economy_integration compaction_emits_started_and_complete_updates`
Expected: FAIL — `no variant named CompactionStarted` (compilation error)

- [ ] **Step 3: Add the `AgentUpdate` variants**

In `crates/zoid/src/agent.rs`, in the `pub enum AgentUpdate` definition (after `ModelInfoFetched { ... }`), add:

```rust
    /// Automated compaction is running (before a burst of ToolResultCompacted events).
    CompactionStarted,
    /// Automated compaction finished (after the burst).
    CompactionComplete,
```

- [ ] **Step 4: Emit inside `record_compactions` (gated on non-empty plan)**

In `crates/zoid/src/agent.rs`, find the `record_compactions` function (~line 1165). It currently has this structure:

```rust
    let plan = zoid_core::compaction::plan_compactions(
        events.iter(),
        &config.policy,
        effective_tokens,
        *calibration_ratio,
        overhead,
    );
    for c in &plan.compactions {
        emit(
            session,
            events,
            ui,
            &config.branch,
            EventKind::ToolResultCompacted {
                id: c.id.clone(),
                summary: c.summary.clone(),
                original_tokens: c.original_tokens,
            },
            session_id,
            now,
        )
        .await?;
    }
    Ok(())
}
```

Replace the `for c in &plan.compactions { ... }` loop with a gated version that emits `CompactionStarted`/`Complete` only when the plan is non-empty:

```rust
    if !plan.compactions.is_empty() {
        let _ = ui.send(AgentUpdate::CompactionStarted).await;
    }
    for c in &plan.compactions {
        emit(
            session,
            events,
            ui,
            &config.branch,
            EventKind::ToolResultCompacted {
                id: c.id.clone(),
                summary: c.summary.clone(),
                original_tokens: c.original_tokens,
            },
            session_id,
            now,
        )
        .await?;
    }
    if !plan.compactions.is_empty() {
        let _ = ui.send(AgentUpdate::CompactionComplete).await;
    }
    Ok(())
}
```

- [ ] **Step 5: Emit inside `preflight_gate`'s compaction section (gated on non-empty)**

In `crates/zoid/src/agent.rs`, find `preflight_gate` (~line 1224). Inside the `if est >= band.high_water {` block that does compaction, the code currently looks like:

```rust
        let plan = zoid_core::compaction::plan_compactions(
            events.iter(),
            &gate_policy,
            None,
            *calibration_ratio,
            overhead,
        );
        let compacted = !plan.compactions.is_empty();
        for c in &plan.compactions {
            emit(
                session,
                events,
                ui,
                &config.branch,
                EventKind::ToolResultCompacted {
                    id: c.id.clone(),
                    summary: c.summary.clone(),
                    original_tokens: c.original_tokens,
                },
                session_id,
                now,
            )
            .await?;
        }
        if compacted {
            est = estimate(events);
        }
```

Wrap the compaction loop with `CompactionStarted`/`Complete` gated on `compacted`. Before the `for c in &plan.compactions {` loop, add:

```rust
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionStarted).await;
        }
```

After the `for c in &plan.compactions { ... }` loop (and before the `if compacted { est = estimate(events); }` line), add:

```rust
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionComplete).await;
        }
```

The `compacted` variable already exists (line `let compacted = !plan.compactions.is_empty();`), so this reuses it. The result:

```rust
        let compacted = !plan.compactions.is_empty();
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionStarted).await;
        }
        for c in &plan.compactions {
            emit(
                session,
                events,
                ui,
                &config.branch,
                EventKind::ToolResultCompacted {
                    id: c.id.clone(),
                    summary: c.summary.clone(),
                    original_tokens: c.original_tokens,
                },
                session_id,
                now,
            )
            .await?;
        }
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionComplete).await;
        }
        if compacted {
            est = estimate(events);
        }
```

**Note:** Both call sites of `record_compactions` (the mid-turn site ~line 568 and the post-tool-execution site ~line 1077) now need **no changes** — the emission lives inside `record_compactions` itself. The call sites are unchanged.

- [ ] **Step 6: Verify the workspace compiles**

Run: `cargo check -p zoid`
Expected: PASS

- [ ] **Step 7: Run the compaction tests to verify they pass**

Run: `cargo test -p zoid --test economy_integration compaction_emits_started_and_complete_updates compaction_does_not_emit_updates_when_nothing_compacted`
Expected: PASS (both tests)

- [ ] **Step 8: Run the full test suite + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS (all existing tests still pass — the new variants are additive; existing match arms that use `_ =>` catch them)

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/tests/economy_integration.rs
git commit -m "feat(agent): emit CompactionStarted/Complete around compaction (gated on non-empty plan)"
```

---

## Task 5: Wire the bin — `App` fields, per-frame spinner, debounce, handler arms, motion tick guard

**Files:**
- Modify: `crates/zoid/src/main.rs` (struct `App`, `App` construction, `run()` loop)

**Interfaces:**
- Consumes: `AgentUpdate::CompactionStarted` / `CompactionComplete` (Task 4), `ShellState.compacting` / `compact_spinner` (Task 2), `glyph::COMPACT_SPINNER` (Task 1), `motion::spinner_frame`
- Produces: the per-frame compaction spinner, 3s debounce logic, handler arms, motion tick wake

- [ ] **Step 1: Add new fields to `App` struct**

In `crates/zoid/src/main.rs`, find the `struct App` definition. After the `companion_hub` field (the last field before the closing `}`), add:

```rust
    /// When the current compaction phase started (for the 3s minimum-display
    /// debounce). `None` when no compaction is in flight or the debounce has
    /// cleared. Set by `AgentUpdate::CompactionStarted`; cleared by the
    /// per-frame debounce check after `CompactionComplete` + 3s elapsed.
    compaction_started_at: Option<std::time::Instant>,
    /// `CompactionComplete` arrived; the indicator stays visible until the 3s
    /// minimum display duration elapses (checked per-frame in `run()`).
    compaction_complete: bool,
```

- [ ] **Step 2: Add defaults to `App` construction**

In `crates/zoid/src/main.rs`, find the `let mut app = App { ... }` construction (the main one in `main()`, ~line 1340). After `companion_hub: zoid_companion::CompanionHub::new(),`, add:

```rust
        compaction_started_at: None,
        compaction_complete: false,
```

- [ ] **Step 3: Add defaults to `test_app` construction**

In `crates/zoid/src/main.rs`, find the `test_app()` function in the `#[cfg(test)] mod tests` block (~line 4110). After `companion_hub: zoid_companion::CompanionHub::new(),`, add:

```rust
            compaction_started_at: None,
            compaction_complete: false,
```

- [ ] **Step 4: Add the per-frame compact spinner + debounce check**

In `crates/zoid/src/main.rs`, in `run()`, find the block where `app.shell.spinner` is set (the line starting `app.shell.spinner = zoid_tui::tokens::glyph::SPINNER[`). Immediately after that line, add:

```rust
        app.shell.compact_spinner = zoid_tui::tokens::glyph::COMPACT_SPINNER
            [zoid_tui::motion::spinner_frame(elapsed, 120, 6, app.shell.reduced_motion)];
        // Debounce: if CompactionComplete arrived, keep the indicator visible
        // until 3s have elapsed since CompactionStarted. The motion tick guard
        // wakes while `compacting` is true, so this timer drains without an
        // extra wake source.
        if app.compaction_complete {
            if let Some(start) = app.compaction_started_at {
                if start.elapsed() >= std::time::Duration::from_secs(3) {
                    app.shell.compacting = false;
                    app.compaction_complete = false;
                    app.compaction_started_at = None;
                }
            }
        }
```

- [ ] **Step 5: Add `AgentUpdate` handler arms**

In `crates/zoid/src/main.rs`, in the `run()` function's `Some(update) = ui_rx.recv()` match block, find the `AgentUpdate::ModelInfoFetched { model, info } => { ... }` arm (the last arm before the closing `}`). After that arm's closing `}`, add:

```rust
                    AgentUpdate::CompactionStarted => {
                        app.shell.compacting = true;
                        app.compaction_started_at = Some(std::time::Instant::now());
                        app.compaction_complete = false;
                    }
                    AgentUpdate::CompactionComplete => {
                        app.compaction_complete = true;
                        // Don't clear app.shell.compacting here — the per-frame
                        // debounce check clears it once the 3s minimum has
                        // elapsed.
                    }
```

- [ ] **Step 6: Expand the motion tick guard**

In `crates/zoid/src/main.rs`, in `run()`, find the `tokio::select!` block's motion tick guard:

```rust
            _ = motion_tick.tick(), if app.streaming || app.delegating || app.zoom_changed_at.is_some() => {
```

Replace with:

```rust
            _ = motion_tick.tick(), if app.streaming || app.delegating || app.shell.compacting || app.zoom_changed_at.is_some() => {
```

- [ ] **Step 7: Verify the workspace compiles**

Run: `cargo check -p zoid`
Expected: PASS

- [ ] **Step 8: Run the full test suite**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 9: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS (no warnings)

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): wire compaction indicator — per-frame spinner, 3s debounce, motion tick guard"
```