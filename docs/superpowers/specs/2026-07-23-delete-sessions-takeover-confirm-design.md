# Delete Sessions from Session Picker + Fix Takeover Confirm

## Problem

Two issues, one shared fix:

1. **New feature:** There is no way to delete old or stale sessions from either the
   in-session `:resume` overlay (`Overlay::Sessions`) or the startup pre-TUI picker.
   Sessions accumulate over time and clutter the picker.

2. **Existing bug:** `SessionTakeoverConfirm` raises a `QuestionState` card, but the
   question card renders inside the conversation pane (`chat.rs` →
   `conversation_view` → `render_question_card`). The Sessions overlay draws on top
   with `Clear` (wipes the region behind it), so the card is invisible. Keys route to
   it (routing step 0 catches `state.question` before overlay routing), so the user is
   stuck pressing arrows/Enter on an invisible card — the picker appears frozen.

The fix for both: an **inline confirm state** that renders within the session
overlay itself, not via the conversation-pane question card.

## Design

### §1 Inline confirm mechanism

Introduce a `SessionConfirm` state on `ShellState`:

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

When `state.session_confirm` is `Some`, the session overlay renders an extra line at
the bottom (inside the overlay border, below the session list):

```
 Delete "bug-fix-attempt"? This permanently removes its history. [y]es / [n]o
```

or for takeover:

```
 "main" is active in another instance. Take it over? [y]es / [n]o
```

The confirm line is rendered with a distinct style (amber/yellow foreground) to
signal a destructive or important action.

**Key routing while confirm is active:** While `state.session_confirm` is `Some` and
`state.overlay == Overlay::Sessions`, the session key router intercepts all keys:

- `y` / `Y` / Enter → `Action::SessionConfirmYes`
- `n` / `N` / Esc → `Action::SessionConfirmNo`
- Everything else → `Noop`

This is checked *within* `route_sessions_key` (not in routing step 0), so it is
scoped to the session overlay only. The general question-card routing (step 0) is
untouched — this is a separate mechanism.

**Why not reuse `QuestionState`?** The question card is baked into
`conversation_view` (the chat-pane renderer) and its routing is step-0 global.
Repurposing it to render inside the overlay would require special-casing the
conversation renderer and the routing precedence. A dedicated `SessionConfirm` is
simpler, self-contained, and renders exactly where it needs to — inside the overlay.

### §2 Store layer (`zoid-core`)

#### New `EventStore::delete_session(id: Ulid)`

Transactional SQL, executed within a single `unchecked_transaction`:

```sql
DELETE FROM event_embeddings WHERE event_id IN (SELECT id FROM events WHERE session_id = ?1);
DELETE FROM events_fts     WHERE session_id = ?1;
DELETE FROM events          WHERE session_id = ?1;
DELETE FROM sessions        WHERE id = ?1;
```

Ordering matters: embeddings reference event ids (queried from `events`), so they
must be deleted first. FTS entries are keyed by `session_id` (unindexed column on
the FTS table), so they are deleted independently. Events and the session row
follow. All four statements run in one transaction so a partial failure (disk
error mid-delete) rolls back — the session is either fully gone or fully intact.

Deleting a non-existent session id is a no-op (zero rows affected, no error).

#### New `Cmd::DeleteSession` variant + `SessionHandle::delete_session(id)`

Follows the exact pattern of `rename_session`, `touch_session`, etc.:

- `Cmd::DeleteSession { id, reply: oneshot::Sender<Result<()>> }` in the `Cmd` enum.
- `SessionHandle::delete_session(&self, id: Ulid) -> Result<()>` async method that
  sends the command and awaits the reply.

#### Heartbeat invariant

The `heartbeat` function (spec §2.3, `store.rs:454`) relies on the invariant that
a zero-row match means takeover, not deletion. This is preserved: deletion is
blocked for live sessions (§5), and a non-live session has no heartbeat caller. So
the only way `heartbeat` sees zero rows is a takeover — deletion cannot produce a
zero-row match because no live session is ever deleted, and only the instance
holding a live session calls `heartbeat`.

The existing doc comment on `heartbeat` ("there is no `DELETE FROM sessions`
anywhere in the codebase") is updated to: "deletion is blocked for live sessions,
so a live session's heartbeat caller will never see a zero-row match from
deletion."

### §3 Route layer (`zoid-tui`)

#### New actions

- `Action::SessionDelete` — fired when Delete or Backspace is pressed on a
  highlighted row while the session list is non-empty.
- `Action::SessionConfirmYes` — `y`/`Y`/Enter while a confirm is active.
- `Action::SessionConfirmNo` — `n`/`N`/Esc while a confirm is active.

#### Updated `route_sessions_key`

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
            // Existing live-check logic: live → SessionTakeoverConfirm, else SessionPick.
            // SessionTakeoverConfirm now sets session_confirm instead of question card.
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

### §4 Render layer (`zoid-tui`)

#### Updated `render_sessions_overlay`

After building the session list rows, if `state.session_confirm` is `Some`, append
a confirm line to the rows vector:

```rust
fn render_sessions_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    let mut rows = /* existing list-building logic */;
    let confirm_active = state.session_confirm.is_some();
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
    list_overlay(frame, area, /* title */, &rows, sel);
}
```

The confirm line is the last row in the list. It is rendered in a distinct style
(amber foreground) to signal its importance. When the confirm is active, the
session row highlight stays on the selected session row (so the user can see which
session they're acting on), while the confirm line sits below it in amber.

The current `list_overlay` renders all rows uniformly (selected = reverse video,
unselected = normal text). To give the confirm line its amber style without changing
`list_overlay`'s signature (which other overlays depend on), render the session rows
via `list_overlay` with one fewer row of height, then draw the confirm line directly
into the buffer at the bottom position with the amber style. The confirm line's
position is always known: the bottom inner row of the overlay area.

The title does not change — the confirm line is the visible signal. The overlay
border and title remain " resume session ".

### §5 Bin layer (`crates/zoid/src/main.rs`)

#### `Action::SessionDelete` handler

1. Guard: `if app.streaming || !app.in_flight_subagents.is_empty()` → status hint
   "finish the current turn first", close overlay, return.
2. Get `sid` from `app.session_ids[app.shell.session_selected]`. If index is out of
   range, close overlay and return.
3. Get the session display name from `app.shell.sessions[selected]` (or the raw
   `SessionInfo` name if available).
4. Check liveness via `app.shell.sessions_live[selected]`. If live → status hint
   "can't delete a session that's in use", return (no confirm, overlay stays open).
5. Set `app.shell.session_confirm = Some(SessionConfirm { sid, name, kind: Delete })`.

#### `Action::SessionConfirmYes` handler

1. Read and clear `app.shell.session_confirm`.
2. If `None`, return (no-op).
3. Match `kind`:
   - **Delete:**
     1. `app.session.delete_session(sid).await` — ignore errors with a status hint
        on failure.
     2. Refresh the session list: re-run the populate logic from
        `Command::ResumeSessionPicker` (re-list, re-populate `session_ids`,
        `sessions`, `sessions_live`, clamp `session_selected` to new length).
     3. Clear `session_confirm`.
   - **Takeover:**
     1. Claim: `app.session.set_active(sid, true, self_pid, now_ms()).await`.
     2. Position `session_selected` on the row matching `sid`.
     3. Clear `session_confirm`.
     4. Fall through to `SessionPick` (load the session), via `Box::pin` recursive
        call — same as the current takeover flow.

#### `Action::SessionConfirmNo` handler

1. `app.shell.session_confirm = None` — back to normal picker.
2. Overlay stays open.

#### Replace `SessionTakeoverConfirm` handler

Instead of setting `app.shell.question` + `app.pending_takeover`:

1. Get `sid` from `app.session_ids[app.shell.session_selected]`. If out of range,
   close overlay and return.
2. Set `app.shell.session_confirm = Some(SessionConfirm { sid, name, kind: Takeover })`.

No `QuestionState` is created. No `pending_takeover` is set.

#### Remove dead code from `QuestionSelect`

The `if let Some(sid) = app.pending_takeover.take()` block in `QuestionSelect`
(line ~4314) becomes dead code — takeover no longer goes through the question card.
Remove the block and the `pending_takeover` field from `App`. If other code
references `pending_takeover`, remove those references too.

#### Startup picker (`pick_session`)

The startup picker is a standalone render+input loop pre-TUI, so it cannot use
`ShellState` or `Overlay`. It gets its own local confirm state:

- A `pending_delete: Option<usize>` field in the picker's local state (the index of
  the session row being confirmed for deletion).
- On Delete/Backspace key press (when a real session row is highlighted and it is
  not live), set `pending_delete = Some(selected)`.
- Render a one-line confirm below the session list:
  ` Delete "name"? [y]es / [n]o` (amber style, same as the overlay).
- `y`/`Y`/Enter → delete via `SessionHandle::delete_session`, re-list sessions,
  clear `pending_delete`, clamp `selected`.
- `n`/`N`/Esc → clear `pending_delete`, back to normal picker.
- While `pending_delete` is set, up/down/Enter (except Enter-as-yes) are inert;
  only `y`/`n`/Esc respond.

The startup picker does not have takeover confirm (live sessions get immediate
takeover per the existing spec §2), so it only needs the delete confirm.

### §6 What is not touched

- Session store schema (no new columns or tables — deletion is pure SQL `DELETE`).
- `SessionHandle` actor pattern (new `Cmd` variant follows existing conventions).
- `is_live`, heartbeat mechanism, takeover detection logic.
- Event log, projections, compaction.
- The question card / `QuestionState` system (left intact for its existing uses —
  approval prompts, `ask_user` tool, retry/skip cards).
- Startup picker's resume/create flow.

### §7 Testing

#### Store layer unit tests

- `delete_session` removes the row, all events, FTS entries, and embeddings for
  the target session only — other sessions' data is untouched.
- `delete_session` on a non-existent id is a no-op (zero rows affected, no error).
- `delete_session` is atomic: verify events and session row are both gone after a
  successful call (no orphans).

#### Route layer unit tests

- Delete key on a populated session list → `Action::SessionDelete`.
- Delete key on empty placeholder → `Action::Noop`.
- `y`/`Y`/Enter while confirm active → `Action::SessionConfirmYes`.
- `n`/`N`/Esc while confirm active → `Action::SessionConfirmNo`.
- Other keys while confirm active → `Action::Noop`.
- Confirm routing only fires under `Overlay::Sessions`, not globally (a
  `SessionConfirm` set while another overlay is up does not intercept keys — but
  this should not happen in practice; the guard is defense-in-depth).

#### Bin integration tests

- `SessionDelete` on a non-live session → sets `session_confirm` with `Delete` kind.
- `SessionDelete` on a live session → status hint, no confirm set.
- `SessionDelete` while streaming → status hint, no confirm set.
- `SessionConfirmYes` with `Delete` → calls `delete_session`, refreshes the list,
  confirm cleared.
- `SessionConfirmNo` → confirm cleared, overlay stays open.
- Takeover: `SessionTakeoverConfirm` → sets `session_confirm` with `Takeover` kind
  (no `QuestionState`, no `pending_takeover`).
- Takeover `SessionConfirmYes` → claims session, loads via `SessionPick`.
- Takeover confirm line is visible in the overlay render (the original bug fix).
- `session_selected` clamped to new list length after deletion.
- `pending_takeover` field removed from `App`; `QuestionSelect` no longer has the
  takeover branch.

### §8 Edge cases

- **Delete last session:** List becomes empty → "(no sessions for this repo)"
  placeholder shown, `session_selected` = 0, confirm cleared.
- **Delete while streaming:** Guarded — status hint, no confirm.
- **Esc during confirm:** `SessionConfirmNo` — returns to normal picker, overlay
  stays open. Esc only closes the overlay when no confirm is pending.
- **Startup picker: delete all sessions:** "Create new session" row remains;
  picker still functions.
- **Takeover confirm visible:** The confirm line renders inside the overlay, so the
  user can actually see the prompt — fixing the current invisible-card bug.
- **Deleting the currently active session:** The currently loaded session is live
  (claimed by this instance), so the liveness check blocks it. The user must close
  the session (or use `:session new`) before it can be deleted.
- **Rapid Delete → confirm → Delete:** While a confirm is pending, the Delete key
  is a `Noop` (confirm captures all keys). The user must resolve the current
  confirm before starting another.