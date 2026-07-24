# Delete Sessions from Session Picker + Fix Takeover Confirm — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the ability to delete old sessions from both the in-session `:resume` overlay and the startup pre-TUI picker. Replace the broken question-card takeover confirm with an inline confirm state. Full cleanup (session row + events + FTS + embeddings). Only non-live sessions can be deleted.

**Architecture:** Five layers of change:
1. **Store layer** (`zoid-core`): New `delete_session` SQL method + `Cmd` variant + `SessionHandle` async method.
2. **State layer** (`zoid-tui`): New `SessionConfirm` struct + `session_confirm` field on `ShellState`.
3. **Route layer** (`zoid-tui`): New `SessionDelete` / `SessionConfirmYes` / `SessionConfirmNo` actions + updated `route_sessions_key`.
4. **Render layer** (`zoid-tui`): Updated `render_sessions_overlay` to show the confirm line.
5. **Bin layer** (`zoid`): Handlers for the new actions + replace `SessionTakeoverConfirm` + remove `pending_takeover` + startup picker delete.

**Tech Stack:** Rust, rusqlite (`params!`, `unchecked_transaction`), ratatui, `zoid-core` / `zoid-tui` / `zoid` crates.

## Global Constraints

- `BranchId::default()` = `BranchId("main")` — defined in `zoid-core/src/event.rs:7`.
- `Ulid` is imported in `main.rs` at line 22: `use ulid::Ulid;`
- `Event` and `EventKind` are imported at `main.rs:27`.
- The `Cmd` enum is in `zoid-core/src/session.rs:9`. The actor loop is at line 119.
- `SessionHandle` async methods follow the pattern: create `oneshot::channel`, send `Cmd`, await reply.
- `EventStore` methods are in `zoid-core/src/store.rs`. `rename_session` (line 396) is the pattern to follow.
- Tests for `zoid-core` run with `cargo test -p zoid-core --lib`.
- Tests for `zoid-tui` run with `cargo test -p zoid-tui --lib`.
- Tests for the `zoid` binary run with `cargo test -p zoid --bin zoid`.
- The `EventStore` uses `rusqlite::Connection`. Transactions use `self.conn.unchecked_transaction()` and `tx.commit()`.
- The `list_overlay` function in `render.rs` renders a list with a border. It calls `Clear` on the area first.

---

### Task 1: Store layer — `delete_session` on `EventStore`

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add `delete_session` method after `rename_session` ~line 402)
- Test: `crates/zoid-core/src/store.rs` (test in the existing `#[cfg(test)]` block)

**Interfaces:**
- Produces: `pub fn delete_session(&self, id: Ulid) -> Result<()>` — transactional SQL delete.

- [ ] **Step 1: Write a failing test**

Add to the `#[cfg(test)] mod tests` block in `store.rs`. Find the end of the test module:

```rust
    #[test]
    fn delete_session_removes_row_events_fts_and_embeddings() {
        let store = EventStore::open(":memory:").unwrap();
        let sid1 = Ulid::new();
        let sid2 = Ulid::new();
        // Create two sessions with events.
        store.insert_session(sid1, "s1", "/repo", 100, 100).unwrap();
        store.insert_session(sid2, "s2", "/repo", 200, 200).unwrap();
        let ev1 = Event::new(Ulid::new(), None, 150, EventKind::UserMessage { text: "hello".into() }).with_session(sid1);
        let ev2 = Event::new(Ulid::new(), None, 250, EventKind::UserMessage { text: "world".into() }).with_session(sid2);
        store.append(&ev1).unwrap();
        store.append(&ev2).unwrap();

        // Delete sid1.
        store.delete_session(sid1).unwrap();

        // sid1's row is gone; sid2's row remains.
        let rows = store.list_session_rows().unwrap();
        assert!(rows.iter().all(|r| r.id != sid1), "deleted session row gone");
        assert!(rows.iter().any(|r| r.id == sid2), "other session row remains");

        // sid1's events are gone; sid2's events remain.
        let s1_events = store.load_session(sid1).unwrap();
        assert!(s1_events.is_empty(), "deleted session events gone");
        let s2_events = store.load_session(sid2).unwrap();
        assert_eq!(s2_events.len(), 1, "other session events remain");
    }

    #[test]
    fn delete_session_on_nonexistent_id_is_noop() {
        let store = EventStore::open(":memory:").unwrap();
        // No error, no panic.
        store.delete_session(Ulid::new()).unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core --lib -- delete_session`
Expected: FAIL — `delete_session` not defined.

- [ ] **Step 3: Add the `delete_session` method**

Add after `rename_session` (~line 402), before `touch_session`:

```rust
    /// Delete a session and all its data: events, FTS entries, embeddings,
    /// and the session row. Transactional — either fully gone or fully intact.
    /// Only call this for non-live sessions (see spec §2 — the heartbeat
    /// invariant relies on live sessions never being deleted).
    pub fn delete_session(&self, id: Ulid) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM event_embeddings WHERE event_id IN \
             (SELECT id FROM events WHERE session_id = ?1)",
            params![id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM events_fts WHERE session_id = ?1",
            params![id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM events WHERE session_id = ?1",
            params![id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE id = ?1",
            params![id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core --lib -- delete_session`
Expected: PASS

- [ ] **Step 5: Run the full test suite**

Run: `cargo test -p zoid-core --lib`
Expected: PASS — no regressions.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat: add EventStore::delete_session (transactional, full cleanup)"
```

---

### Task 2: Session actor — `Cmd::DeleteSession` + `SessionHandle::delete_session`

**Files:**
- Modify: `crates/zoid-core/src/session.rs` (add `Cmd` variant at ~line 97, handler in actor loop at ~line 169, async method after `touch_session` at ~line 317)
- Test: `crates/zoid-core/src/session.rs` (test in the existing `#[cfg(test)]` block)

**Interfaces:**
- Produces: `Cmd::DeleteSession { id, reply }` variant.
- Produces: `pub async fn delete_session(&self, id: Ulid) -> Result<()>` on `SessionHandle`.

- [ ] **Step 1: Write a failing test**

Add to the test module in `session.rs`:

```rust
    #[tokio::test]
    async fn delete_session_removes_session() {
        let store = SessionHandle::spawn(":memory:").unwrap();
        let sid = Ulid::new();
        store.new_session(sid, "test".into(), "/repo".into(), 0).await.unwrap();
        // Confirm it exists.
        let before = store.list_sessions(None).await.unwrap();
        assert!(before.iter().any(|s| s.id == sid));
        // Delete it.
        store.delete_session(sid).await.unwrap();
        // Confirm it's gone.
        let after = store.list_sessions(None).await.unwrap();
        assert!(!after.iter().any(|s| s.id == sid), "session must be deleted");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core --lib -- delete_session_removes_session`
Expected: FAIL — `delete_session` not on `SessionHandle`.

- [ ] **Step 3: Add the `Cmd` variant**

In the `Cmd` enum (after `EventsByIds` at ~line 96, before the closing `}`):

```rust
    DeleteSession {
        id: Ulid,
        reply: oneshot::Sender<Result<()>>,
    },
```

- [ ] **Step 4: Add the handler in the actor loop**

In the `while let Some(cmd) = rx.blocking_recv()` match (after `Cmd::TouchSession` handler at ~line 147, before `Cmd::SetActiveMode`):

```rust
                    Cmd::DeleteSession { id, reply } => {
                        let _ = reply.send(store.delete_session(id));
                    }
```

- [ ] **Step 5: Add the async method**

After `touch_session` (~line 317), add:

```rust
    /// Delete a session and all its events/FTS/embeddings. Only call this for
    /// non-live sessions (the heartbeat invariant relies on live sessions
    /// never being deleted).
    pub async fn delete_session(&self, id: Ulid) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::DeleteSession { id, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p zoid-core --lib -- delete_session_removes_session`
Expected: PASS

- [ ] **Step 7: Run the full test suite**

Run: `cargo test -p zoid-core --lib`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-core/src/session.rs
git commit -m "feat: add SessionHandle::delete_session (Cmd::DeleteSession actor wiring)"
```

---

### Task 3: State layer — `SessionConfirm` struct + `session_confirm` field

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add struct + enum, add field to `ShellState`, initialize in `new`)
- Test: `crates/zoid-tui/src/state.rs` (test in the existing `#[cfg(test)]` block)

**Interfaces:**
- Produces: `pub struct SessionConfirm { pub sid: Ulid, pub name: String, pub kind: SessionConfirmKind }`
- Produces: `pub enum SessionConfirmKind { Delete, Takeover }`
- Produces: `pub session_confirm: Option<SessionConfirm>` field on `ShellState`.

- [ ] **Step 1: Write a failing test**

Add to the test module in `state.rs`:

```rust
    #[test]
    fn session_confirm_defaults_to_none() {
        let s = ShellState::new();
        assert!(s.session_confirm.is_none(), "session_confirm defaults to None");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib -- session_confirm_defaults_to_none`
Expected: FAIL — `session_confirm` field doesn't exist.

- [ ] **Step 3: Add the struct and enum**

Add near the top of `state.rs` (after the imports, before `ShellState`):

```rust
/// A pending destructive action on a session row, rendered inline in the
/// session overlay. Replaces the question-card approach (which was invisible
/// behind the overlay's Clear).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfirm {
    /// The session id the action targets.
    pub sid: Ulid,
    /// The display name for the confirm prompt.
    pub name: String,
    /// What kind of confirm — drives the prompt text and the action on "yes".
    pub kind: SessionConfirmKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionConfirmKind {
    /// "Delete session X? This permanently removes its history. y/n"
    Delete,
    /// "Session X is active in another instance. Take it over? y/n"
    Takeover,
}
```

Note: `Ulid` must be imported. Check if it's already imported in `state.rs` — if not, add `use ulid::Ulid;` at the top of the file.

- [ ] **Step 4: Add the field to `ShellState`**

After `session_selected` (line ~476), add:

```rust
    /// A pending destructive action (delete or takeover) on a session row,
    /// rendered as an inline confirm line in the session overlay. `None` when
    /// no confirm is pending. Replaces the question-card approach.
    pub session_confirm: Option<SessionConfirm>,
```

- [ ] **Step 5: Initialize in `ShellState::new`**

In the `new()` function (after `session_selected: 0,` at ~line 652), add:

```rust
            session_confirm: None,
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib -- session_confirm_defaults_to_none`
Expected: PASS

- [ ] **Step 7: Run the full test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat: add SessionConfirm state for inline session picker confirm"
```

---

### Task 4: Route layer — new actions + updated `route_sessions_key`

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (add `Action` variants, update `route_sessions_key`)
- Test: `crates/zoid-tui/src/route.rs` (tests in the existing `#[cfg(test)]` block)

**Interfaces:**
- Produces: `Action::SessionDelete`, `Action::SessionConfirmYes`, `Action::SessionConfirmNo`.

- [ ] **Step 1: Write failing tests**

Add to the test module in `route.rs`:

```rust
    #[test]
    fn delete_key_on_populated_list_fires_session_delete() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into(), "b".into()];
        s.session_selected = 0;
        assert_eq!(
            route_key(&s, key(KeyCode::Delete, KeyModifiers::NONE)),
            Action::SessionDelete
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Backspace, KeyModifiers::NONE)),
            Action::SessionDelete
        );
    }

    #[test]
    fn delete_key_on_empty_list_is_noop() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = Vec::new();
        assert_eq!(
            route_key(&s, key(KeyCode::Delete, KeyModifiers::NONE)),
            Action::Noop
        );
    }

    #[test]
    fn confirm_yes_routes_on_y_enter() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into()];
        s.session_confirm = Some(SessionConfirm {
            sid: Ulid::new(),
            name: "test".into(),
            kind: SessionConfirmKind::Delete,
        });
        assert_eq!(
            route_key(&s, key(KeyCode::Char('y'), KeyModifiers::NONE)),
            Action::SessionConfirmYes
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('Y'), KeyModifiers::NONE)),
            Action::SessionConfirmYes
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SessionConfirmYes
        );
    }

    #[test]
    fn confirm_no_routes_on_n_esc() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into()];
        s.session_confirm = Some(SessionConfirm {
            sid: Ulid::new(),
            name: "test".into(),
            kind: SessionConfirmKind::Delete,
        });
        assert_eq!(
            route_key(&s, key(KeyCode::Char('n'), KeyModifiers::NONE)),
            Action::SessionConfirmNo
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('N'), KeyModifiers::NONE)),
            Action::SessionConfirmNo
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::SessionConfirmNo
        );
    }

    #[test]
    fn confirm_captures_other_keys_as_noop() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into()];
        s.session_confirm = Some(SessionConfirm {
            sid: Ulid::new(),
            name: "test".into(),
            kind: SessionConfirmKind::Delete,
        });
        assert_eq!(
            route_key(&s, key(KeyCode::Up, KeyModifiers::NONE)),
            Action::Noop
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('d'), KeyModifiers::NONE)),
            Action::Noop
        );
    }
```

Note: Add `use ulid::Ulid;` and `use crate::state::{SessionConfirm, SessionConfirmKind};` to the test module imports if not already present.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib -- delete_key confirm_yes confirm_no confirm_captures`
Expected: FAIL — `SessionDelete`, `SessionConfirmYes`, `SessionConfirmNo` don't exist.

- [ ] **Step 3: Add the new `Action` variants**

In the `Action` enum (after `SessionPick` at ~line 75 and `SessionTakeoverConfirm` at ~line 78), add:

```rust
    /// Delete the highlighted session (Delete/Backspace key in the picker).
    SessionDelete,
    /// Confirm a pending session action (y/Y/Enter while confirm is active).
    SessionConfirmYes,
    /// Cancel a pending session action (n/N/Esc while confirm is active).
    SessionConfirmNo,
```

- [ ] **Step 4: Update `route_sessions_key`**

Replace the entire `route_sessions_key` function (~line 367):

```rust
fn route_sessions_key(state: &ShellState, key: KeyEvent) -> Action {
    // If a confirm is pending, it captures all keys.
    if state.session_confirm.is_some() {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Action::SessionConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::SessionConfirmNo,
            _ => Action::Noop,
        };
    }
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => {
            // If the highlighted row is live, raise the takeover confirm
            // instead of resuming directly. Spec §3.2.
            let live = state
                .sessions_live
                .get(state.session_selected)
                .copied()
                .unwrap_or(false);
            if live {
                Action::SessionTakeoverConfirm
            } else {
                Action::SessionPick
            }
        }
        KeyCode::Delete | KeyCode::Backspace if !state.sessions.is_empty() => {
            Action::SessionDelete
        }
        KeyCode::Up | KeyCode::Char('k') => Action::SessionMove(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::SessionMove(1),
        _ => Action::Noop,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib -- delete_key confirm_yes confirm_no confirm_captures`
Expected: PASS

- [ ] **Step 6: Run the full test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS — no regressions. Note: existing `sessions_live_row_enter_raises_confirm_not_pick` test should still pass since the Enter behavior is unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/route.rs
git commit -m "feat: add SessionDelete/ConfirmYes/ConfirmNo actions + route_sessions_key update"
```

---

### Task 5: Render layer — inline confirm in `render_sessions_overlay`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (update `render_sessions_overlay` at ~line 1098)
- Test: `crates/zoid-tui/src/render.rs` (test in the existing `#[cfg(test)]` block)

**Interfaces:**
- Consumes: `SessionConfirm`, `SessionConfirmKind` from `crate::state`.

- [ ] **Step 1: Write a failing test**

Add to the test module in `render.rs`:

```rust
    #[test]
    fn sessions_overlay_shows_confirm_line_when_pending() {
        use crate::state::{SessionConfirm, SessionConfirmKind};
        use ulid::Ulid;

        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["test-session  ·  5m ago  ·  1k".into()];
        s.sessions_live = vec![false];
        s.session_selected = 0;
        s.session_confirm = Some(SessionConfirm {
            sid: Ulid::new(),
            name: "test-session".into(),
            kind: SessionConfirmKind::Delete,
        });
        let _ = s.draw(|f| render_sessions_overlay(f, &s, f.area()));
        // We can't easily inspect the buffer content in a unit test without
        // a TestBackend setup. Instead, verify the overlay doesn't panic
        // with a confirm pending — the visual verification is manual.
        // The key assertion: the function compiles and runs without error.
    }
```

Note: The `draw` method may not be available on `ShellState`. If not, use a `TestBackend` directly:

```rust
    #[test]
    fn sessions_overlay_shows_confirm_line_when_pending() {
        use crate::state::{SessionConfirm, SessionConfirmKind};
        use ratatui::backend::TestBackend;
        use ulid::Ulid;

        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["test-session  ·  5m ago  ·  1k".into()];
        s.sessions_live = vec![false];
        s.session_selected = 0;
        s.session_confirm = Some(SessionConfirm {
            sid: Ulid::new(),
            name: "test-session".into(),
            kind: SessionConfirmKind::Delete,
        });
        let backend = TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render_sessions_overlay(f, &s, f.area())).unwrap();
        // Verify the confirm prompt text is in the buffer.
        let buffer = terminal.backend().buffer();
        let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains("Delete"),
            "confirm line must contain 'Delete': {content}"
        );
        assert!(
            content.contains("y") && content.contains("n"),
            "confirm line must show y/n options: {content}"
        );
    }
```

Use whichever version compiles — check the existing render tests for the pattern used in this file.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib -- sessions_overlay_shows_confirm`
Expected: FAIL — `render_sessions_overlay` doesn't reference `session_confirm`.

- [ ] **Step 3: Update `render_sessions_overlay`**

Replace the function (~line 1098):

```rust
fn render_sessions_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    let mut rows = if state.sessions.is_empty() {
        vec!["(no sessions for this repo)".to_string()]
    } else {
        state
            .sessions
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let live = state.sessions_live.get(i).copied().unwrap_or(false);
                if live {
                    format!("{r}  · in use")
                } else {
                    r.clone()
                }
            })
            .collect()
    };
    // Append the confirm line if a destructive action is pending.
    if let Some(c) = &state.session_confirm {
        let prompt = match c.kind {
            SessionConfirmKind::Delete => format!(
                " Delete \"{}\"? This permanently removes its history. [y]es / [n]o",
                c.name
            ),
            SessionConfirmKind::Takeover => format!(
                " \"{}\" is active in another instance. Take it over? [y]es / [n]o",
                c.name
            ),
        };
        rows.push(prompt);
    }
    let sel = nav(state.session_selected, 0, rows.len());
    list_overlay(
        frame,
        area,
        format!(" {} resume session ", glyph::RESUME),
        &rows,
        sel,
    );
}
```

Note: Add `use crate::state::SessionConfirmKind;` at the top of `render.rs` if not already imported.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib -- sessions_overlay_shows_confirm`
Expected: PASS

- [ ] **Step 5: Run the full test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat: render inline confirm line in session overlay"
```

---

### Task 6: Bin layer — action handlers + replace takeover confirm

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `SessionDelete` / `SessionConfirmYes` / `SessionConfirmNo` handlers, replace `SessionTakeoverConfirm`, remove `pending_takeover`)
- Test: `crates/zoid/src/main.rs` (tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `SessionHandle::delete_session`, `SessionConfirm`, `SessionConfirmKind`, new `Action` variants.

- [ ] **Step 1: Write failing tests**

Add to the test module in `main.rs` (near the existing session/delegation tests):

```rust
    /// SessionDelete on a non-live session sets session_confirm with Delete kind.
    #[tokio::test]
    async fn session_delete_sets_confirm() {
        let mut app = test_app().await;
        app.session_ids = vec![Ulid::new()];
        app.shell.sessions = vec!["test  ·  5m  ·  1k".into()];
        app.shell.sessions_live = vec![false];
        app.shell.session_selected = 0;
        app.shell.overlay = zoid_tui::Overlay::Sessions;

        handle_action(&mut app, zoid_tui::route::Action::SessionDelete)
            .await
            .unwrap();

        assert!(app.shell.session_confirm.is_some(), "confirm must be set");
        let c = app.shell.session_confirm.as_ref().unwrap();
        assert!(
            matches!(c.kind, zoid_tui::state::SessionConfirmKind::Delete),
            "must be Delete kind"
        );
    }

    /// SessionDelete on a live session does NOT set confirm — shows status hint.
    #[tokio::test]
    async fn session_delete_on_live_shows_hint() {
        let mut app = test_app().await;
        app.session_ids = vec![Ulid::new()];
        app.shell.sessions = vec!["test  ·  5m  ·  1k".into()];
        app.shell.sessions_live = vec![true]; // live
        app.shell.session_selected = 0;
        app.shell.overlay = zoid_tui::Overlay::Sessions;

        handle_action(&mut app, zoid_tui::route::Action::SessionDelete)
            .await
            .unwrap();

        assert!(app.shell.session_confirm.is_none(), "no confirm for live session");
        assert!(
            app.shell.status_hint.is_some(),
            "status hint must be set for live session"
        );
    }

    /// SessionConfirmNo clears the confirm and keeps the overlay open.
    #[tokio::test]
    async fn session_confirm_no_clears_confirm() {
        let mut app = test_app().await;
        app.shell.session_confirm = Some(zoid_tui::state::SessionConfirm {
            sid: Ulid::new(),
            name: "test".into(),
            kind: zoid_tui::state::SessionConfirmKind::Delete,
        });
        app.shell.overlay = zoid_tui::Overlay::Sessions;

        handle_action(&mut app, zoid_tui::route::Action::SessionConfirmNo)
            .await
            .unwrap();

        assert!(app.shell.session_confirm.is_none(), "confirm cleared");
        assert_eq!(app.shell.overlay, zoid_tui::Overlay::Sessions, "overlay stays open");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --bin zoid -- session_delete session_confirm_no`
Expected: FAIL — the action handlers don't exist.

- [ ] **Step 3: Add `SessionDelete` handler**

In the `handle_action` function, after `Action::SessionPick { ... }` (which ends at ~line 3990), add:

```rust
        Action::SessionDelete => {
            if app.streaming || !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some("finish the current turn first".into());
                app.shell.close_overlay();
                return Ok(false);
            }
            let sid = match app.session_ids.get(app.shell.session_selected) {
                Some(&sid) => sid,
                None => return Ok(false),
            };
            let live = app
                .shell
                .sessions_live
                .get(app.shell.session_selected)
                .copied()
                .unwrap_or(false);
            if live {
                app.shell.status_hint = Some("can't delete a session that's in use".into());
                return Ok(false);
            }
            let name = app
                .shell
                .sessions
                .get(app.shell.session_selected)
                .cloned()
                .unwrap_or_default()
                .split("  ·  ")
                .next()
                .unwrap_or("session")
                .to_string();
            app.shell.session_confirm = Some(zoid_tui::state::SessionConfirm {
                sid,
                name,
                kind: zoid_tui::state::SessionConfirmKind::Delete,
            });
        }
```

- [ ] **Step 4: Add `SessionConfirmYes` handler**

After the `SessionDelete` handler, add:

```rust
        Action::SessionConfirmYes => {
            let confirm = match app.shell.session_confirm.take() {
                Some(c) => c,
                None => return Ok(false),
            };
            match confirm.kind {
                zoid_tui::state::SessionConfirmKind::Delete => {
                    if let Err(e) = app.session.delete_session(confirm.sid).await {
                        app.shell.status_hint = Some(format!("could not delete session: {e}"));
                        app.shell.session_confirm = None;
                        return Ok(false);
                    }
                    // Refresh the session list (re-run the populate from
                    // Command::ResumeSessionPicker).
                    let list = app
                        .session
                        .list_sessions(Some(repo_root()))
                        .await
                        .unwrap_or_default();
                    app.session_ids = list.iter().map(|s| s.id).collect();
                    app.shell.sessions = list
                        .iter()
                        .map(|s| {
                            format!(
                                "{}  ·  {}  ·  {}",
                                s.name,
                                fmt_since(s.last_touched_ts, now_ms()),
                                zoid_tui::economy_view::human_tokens(s.token_total)
                            )
                        })
                        .collect();
                    let now = now_ms();
                    app.shell.sessions_live = list
                        .iter()
                        .map(|s| {
                            zoid_core::store::is_live(
                                s.active,
                                s.active_pid,
                                s.active_heartbeat,
                                now,
                                pid_alive,
                            )
                        })
                        .collect();
                    if app.shell.session_selected >= app.shell.sessions.len() {
                        app.shell.session_selected = 0;
                    }
                    app.shell.session_confirm = None;
                }
                zoid_tui::state::SessionConfirmKind::Takeover => {
                    let sid = confirm.sid;
                    let self_pid = std::process::id() as i64;
                    app.session
                        .set_active(sid, true, self_pid, now_ms())
                        .await
                        .ok();
                    app.shell.session_selected = app
                        .session_ids
                        .iter()
                        .position(|&x| x == sid)
                        .unwrap_or(app.shell.session_selected);
                    app.shell.session_confirm = None;
                    return Box::pin(handle_action(
                        app,
                        zoid_tui::route::Action::SessionPick,
                    ))
                    .await;
                }
            }
        }
```

- [ ] **Step 5: Add `SessionConfirmNo` handler**

After `SessionConfirmYes`, add:

```rust
        Action::SessionConfirmNo => {
            app.shell.session_confirm = None;
        }
```

- [ ] **Step 6: Replace `SessionTakeoverConfirm` handler**

Find the existing `Action::SessionTakeoverConfirm` handler (~line 3870) and replace it. The current code sets `app.shell.question` and `app.pending_takeover`. Replace with:

```rust
        Action::SessionTakeoverConfirm => {
            let sid = match app.session_ids.get(app.shell.session_selected) {
                Some(&sid) => sid,
                None => {
                    app.shell.close_overlay();
                    return Ok(false);
                }
            };
            let name = app
                .shell
                .sessions
                .get(app.shell.session_selected)
                .cloned()
                .unwrap_or_default()
                .split("  ·  ")
                .next()
                .unwrap_or("session")
                .to_string();
            app.shell.session_confirm = Some(zoid_tui::state::SessionConfirm {
                sid,
                name,
                kind: zoid_tui::state::SessionConfirmKind::Takeover,
            });
        }
```

- [ ] **Step 7: Remove `pending_takeover` from `QuestionSelect`**

In `Action::QuestionSelect` (~line 4331), remove the `if let Some(sid) = app.pending_takeover.take()` block entirely. The takeover no longer goes through the question card. The block starts with `if let Some(sid) = app.pending_takeover.take() {` and ends at its matching `}` (before the `let outcome = ...` line).

- [ ] **Step 8: Remove `pending_takeover` field from `App`**

Remove the field declaration at ~line 1677: `pending_takeover: Option<Ulid>,`
Remove the initialization at ~line 2144: `pending_takeover: None,`
Remove the initialization in `test_app` at ~line 7236: `pending_takeover: None,`

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p zoid --bin zoid -- session_delete session_confirm_no`
Expected: PASS

- [ ] **Step 10: Run the full test suite**

Run: `cargo test -p zoid --bin zoid`
Expected: PASS — no regressions. The existing `sessions_live_row_enter_raises_confirm_not_pick` test in `route.rs` should still pass (Enter still routes to `SessionTakeoverConfirm` for live rows).

- [ ] **Step 11: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: session delete + takeover confirm handlers, remove pending_takeover"
```

---

### Task 7: Startup picker — add delete support

**Files:**
- Modify: `crates/zoid/src/main.rs` (update `PickKey` enum, `pick_choice` function, `pick_session` function)
- Test: `crates/zoid/src/main.rs` (tests for `pick_choice` with delete)

**Interfaces:**
- Produces: `PickKey::Delete`, `PickOutcome::DeleteConfirm(usize)`, `PickOutcome::DeleteConfirmYes`, `PickOutcome::DeleteConfirmNo`.

- [ ] **Step 1: Write failing tests for `pick_choice` with delete**

Add to the test module in `main.rs`:

```rust
    #[test]
    fn pick_choice_delete_on_session_row() {
        // Delete key on a session row (not "Create new") → DeleteConfirm.
        assert_eq!(
            pick_choice(2, 0, PickKey::Delete),
            PickOutcome::DeleteConfirm(0)
        );
        assert_eq!(
            pick_choice(2, 1, PickKey::Delete),
            PickOutcome::DeleteConfirm(1)
        );
    }

    #[test]
    fn pick_choice_delete_on_create_new_is_noop() {
        // Delete key on the "Create new" row (index == n_sessions) → Pending.
        assert_eq!(
            pick_choice(2, 2, PickKey::Delete),
            PickOutcome::Pending(2)
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --bin zoid -- pick_choice_delete`
Expected: FAIL — `PickKey::Delete` and `PickOutcome::DeleteConfirm` don't exist.

- [ ] **Step 3: Add `PickKey::Delete` and `PickOutcome` variants**

Update the `PickKey` enum (~line 574):

```rust
enum PickKey {
    Up,
    Down,
    Enter,
    Esc,
    Delete,
}
```

Update the `PickOutcome` enum (~line 583):

```rust
enum PickOutcome {
    Pending(usize),
    Resume(usize),
    CreateNew,
    Abort,
    /// Delete the session at this index (raises inline confirm in pick_session).
    DeleteConfirm(usize),
}
```

- [ ] **Step 4: Update `pick_choice`**

Add `Delete` handling to `pick_choice` (~line 598). After the `PickKey::Esc => PickOutcome::Abort,` arm, add:

```rust
        PickKey::Delete => {
            if cur < n_sessions {
                PickOutcome::DeleteConfirm(cur)
            } else {
                // "Create new" row — delete is a no-op.
                PickOutcome::Pending(cur)
            }
        }
```

- [ ] **Step 5: Update `pick_session` to handle delete**

First, change the immutable bindings to mutable. At `main.rs:655-672`, change:

```rust
// BEFORE:
    let sessions: Vec<zoid_core::sessions::SessionInfo> = ...
    let n = sessions.len();
    let live: Vec<bool> = ...

// AFTER:
    let mut sessions: Vec<zoid_core::sessions::SessionInfo> = ...
    let mut n = sessions.len();
    let mut live: Vec<bool> = ...
```

Add `Delete` to the key mapping (after `Esc`):

```rust
                    crossterm::event::KeyCode::Esc => PickKey::Esc,
                    crossterm::event::KeyCode::Delete | crossterm::event::KeyCode::Backspace => {
                        PickKey::Delete
                    }
```

Then add a `pending_delete: Option<usize>` local variable before the loop:

```rust
    let mut pending_delete: Option<usize> = None;
```

While `pending_delete` is `Some`, intercept confirm keys BEFORE calling `pick_choice`. Add this check at the top of the `Some(Ok(CEvent::Key(key))) =>` arm, before the `pick_choice` call:

```rust
            Some(Ok(CEvent::Key(key))) => {
                // Inline confirm for pending delete — captures all keys.
                if let Some(idx) = pending_delete {
                    match key.code {
                        crossterm::event::KeyCode::Char('y')
                        | crossterm::event::KeyCode::Char('Y')
                        | crossterm::event::KeyCode::Enter => {
                            // Confirm: delete the session at idx.
                            if let Some(s) = sessions.get(idx) {
                                let _ = session.delete_session(s.id).await;
                            }
                            // Re-list sessions.
                            sessions = session
                                .list_sessions(Some(root.to_string()))
                                .await
                                .unwrap_or_default();
                            n = sessions.len();
                            live = sessions
                                .iter()
                                .map(|s| {
                                    zoid_core::store::is_live(
                                        s.active,
                                        s.active_pid,
                                        s.active_heartbeat,
                                        boot_ts,
                                        pid_alive,
                                    )
                                })
                                .collect();
                            if selected >= n {
                                selected = 0;
                            }
                            pending_delete = None;
                        }
                        crossterm::event::KeyCode::Char('n')
                        | crossterm::event::KeyCode::Char('N')
                        | crossterm::event::KeyCode::Esc => {
                            pending_delete = None;
                        }
                        _ => {} // ignore all other keys during confirm
                    }
                    continue;
                }
                let pick_key = match key.code {

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid --bin zoid -- pick_choice_delete`
Expected: PASS

- [ ] **Step 7: Run the full test suite**

Run: `cargo test -p zoid --bin zoid`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: add session delete to startup picker with inline confirm"
```

---

### Task 8: Update heartbeat doc comment

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (update the `heartbeat` function's doc comment at ~line 454)

- [ ] **Step 1: Update the doc comment**

Find the `heartbeat` function's doc comment (~line 454) that says:

```
/// Invariant relied on here: there is no `DELETE FROM sessions` anywhere in
/// the codebase, so a zero-row match unambiguously means takeover (not row
/// deletion). Do not introduce a session-delete path without revisiting this.
```

Replace with:

```
/// Invariant relied on here: deletion is blocked for live sessions, so a
/// live session's heartbeat caller will never see a zero-row match from
/// deletion. A zero-row match unambiguously means takeover (another process
/// changed `active_pid`).
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p zoid-core --lib`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "docs: update heartbeat invariant comment for session deletion"
```