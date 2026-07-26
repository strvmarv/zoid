# Startup Picker: "Create new" at the top — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the "Create new session" row from the bottom of the startup picker to the top (below the title), so it is always visible regardless of session count, while keeping the cursor on the most-recent session by default.

**Architecture:** The picker is a self-contained render+input loop in `pick_session` (`crates/zoid/src/main.rs`). It calls the pure function `pick_choice` for keystroke logic and maps `PickOutcome`s to actions. The reorder flips the logical boundary in `pick_choice` from "`cur < n_sessions` means session" to "`cur == 0` means Create-new", then updates the render order, initial cursor, outcome-to-session mapping, delete-clamp, and scroll-offset line math in `pick_session`.

**Tech Stack:** Rust, ratatui 0.30 (`Paragraph::scroll`), crossterm. Tests are `#[test]` unit tests in the `tests` module of `crates/zoid/src/main.rs`, run via `cargo test -p zoid --bin zoid`.

## Global Constraints

- The picker only opens for 2+ sessions for the current CWD (`BootPath::Picker`); 0 or 1 session never reaches `pick_session`. Delete can reduce the count below 2 inside the loop, so all clamps must handle `n == 0` gracefully.
- `pick_choice` must stay pure (no IO) — it takes `(n_sessions, selected, key)` and returns a `PickOutcome`.
- The wrap math (`total = n_sessions + 1`, Up from 0 → `total - 1`, Down from `total - 1` → 0) is unchanged — only the session/Create-new boundary moves.
- No new dependencies. No changes to the session store, liveness, CLI flags, `BootPath`, or the in-session `:resume`/`:new` overlays.

---

## File Structure

All changes are in a single file: `crates/zoid/src/main.rs`.

- **`pick_choice` (lines ~597-629):** the pure keystroke handler. Boundary flip: `cur == 0` → Create-new; `cur >= 1` → session.
- **`pick_session` (lines ~651-862):** the render+input loop. Render order, initial `selected`, outcome mapping, delete-clamp, scroll line math all update.
- **`tests` module (lines ~9029-9088):** the `pick_choice` unit tests. Boundary-asserting tests update to the new index convention.

No files are created or deleted.

---

## Task 1: Flip the session/Create-new boundary in `pick_choice`

**Files:**
- Modify: `crates/zoid/src/main.rs` — `pick_choice` function (~lines 597-629) and its doc comment (~lines 597-600)
- Test: `crates/zoid/src/main.rs` — `pick_choice` tests in the `tests` module (~lines 9029-9088)

**Interfaces:**
- Produces: `pick_choice(n_sessions: usize, selected: usize, key: PickKey) -> PickOutcome` with the new convention — index 0 is "Create new", indices `1..=n_sessions` are session rows. `PickOutcome::Resume(idx)` / `DeleteConfirm(idx)` now carry a logical index where `idx >= 1` (session rows), and `CreateNew` / no-op-delete fire at `idx == 0`.

- [ ] **Step 1: Update the boundary-asserting tests to the new convention**

Replace the entire `// --- pick_choice tests ---` block (from the comment line through `pick_choice_delete_on_create_new_is_noop`) with:

```rust
    // --- pick_choice tests ---
    // Convention: logical index 0 = "Create new", indices 1..=n = session rows.
    // The wrap math is unchanged from the old layout; only the
    // session/Create-new boundary moved from "cur < n" to "cur == 0".

    #[test]
    fn pick_choice_down_advances_selection() {
        // Down from Create-new (0) → first session (1).
        assert_eq!(pick_choice(3, 0, PickKey::Down), PickOutcome::Pending(1));
    }

    #[test]
    fn pick_choice_up_wraps() {
        // n_sessions=3 → total rows = 4 (0..3). Up from 0 (Create new) → 3 (last session).
        assert_eq!(pick_choice(3, 0, PickKey::Up), PickOutcome::Pending(3));
    }

    #[test]
    fn pick_choice_down_wraps() {
        // n_sessions=3 → total rows = 4 (0,1,2,3). Down from 3 (last session) → 0 (Create new).
        assert_eq!(pick_choice(3, 3, PickKey::Down), PickOutcome::Pending(0));
    }

    #[test]
    fn pick_choice_enter_on_session_resumes() {
        // Index 1 is the first session row. Enter → Resume(1).
        assert_eq!(pick_choice(3, 1, PickKey::Enter), PickOutcome::Resume(1));
        // Index 3 is the last session row (n_sessions=3). Enter → Resume(3).
        assert_eq!(pick_choice(3, 3, PickKey::Enter), PickOutcome::Resume(3));
    }

    #[test]
    fn pick_choice_enter_on_create_new() {
        // Index 0 is "Create new". Enter → CreateNew.
        assert_eq!(pick_choice(3, 0, PickKey::Enter), PickOutcome::CreateNew);
    }

    #[test]
    fn pick_choice_esc_aborts() {
        assert_eq!(pick_choice(3, 0, PickKey::Esc), PickOutcome::Abort);
    }

    #[test]
    fn pick_choice_clamps_selection_to_total_rows() {
        // If selected is somehow past the end, Down should wrap to 0.
        assert_eq!(pick_choice(2, 5, PickKey::Down), PickOutcome::Pending(0));
    }

    #[test]
    fn pick_choice_delete_on_session_row() {
        // Indices 1 and 2 are session rows (n_sessions=2). Delete → DeleteConfirm.
        assert_eq!(
            pick_choice(2, 1, PickKey::Delete),
            PickOutcome::DeleteConfirm(1)
        );
        assert_eq!(
            pick_choice(2, 2, PickKey::Delete),
            PickOutcome::DeleteConfirm(2)
        );
    }

    #[test]
    fn pick_choice_delete_on_create_new_is_noop() {
        // Index 0 is "Create new". Delete is a no-op → Pending(0).
        assert_eq!(
            pick_choice(2, 0, PickKey::Delete),
            PickOutcome::Pending(0)
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid --bin zoid -- pick_choice 2>&1 | tail -20`
Expected: FAIL — the tests assert the new convention but `pick_choice` still uses the old `cur < n_sessions` boundary. `pick_choice_enter_on_create_new` (`pick_choice(3, 0, Enter)`) expects `CreateNew` but gets `Resume(0)`. `pick_choice_delete_on_create_new_is_noop` (`pick_choice(2, 0, Delete)`) expects `Pending(0)` but gets `DeleteConfirm(0)`.

- [ ] **Step 3: Update `pick_choice` to flip the boundary**

Replace the `pick_choice` function **and its doc comment** (the block starting `/// Handle one keystroke in the startup picker.` through the closing `}` of `pick_choice`) with:

```rust
/// Handle one keystroke in the startup picker. `n_sessions` is the number of
/// session rows; the total row count is `n_sessions + 1`. Logical index 0 is
/// the "Create new" row (rendered at the top); indices `1..=n_sessions` are
/// session rows (most-recent first). `selected` is the current cursor index.
/// Pure — no IO, no terminal.
fn pick_choice(n_sessions: usize, selected: usize, key: PickKey) -> PickOutcome {
    let total = n_sessions + 1; // sessions + "Create new"
    let cur = selected.min(total.saturating_sub(1));
    match key {
        PickKey::Up => {
            let next = if cur == 0 { total - 1 } else { cur - 1 };
            PickOutcome::Pending(next)
        }
        PickKey::Down => {
            let next = if cur + 1 >= total { 0 } else { cur + 1 };
            PickOutcome::Pending(next)
        }
        PickKey::Enter => {
            // Index 0 = "Create new"; indices 1..=n_sessions = session rows.
            if cur == 0 {
                PickOutcome::CreateNew
            } else {
                PickOutcome::Resume(cur)
            }
        }
        PickKey::Esc => PickOutcome::Abort,
        PickKey::Delete => {
            // Can't delete "Create new" (index 0) — no-op.
            if cur == 0 {
                PickOutcome::Pending(cur)
            } else {
                PickOutcome::DeleteConfirm(cur)
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid --bin zoid -- pick_choice 2>&1 | tail -20`
Expected: PASS — all 9 `pick_choice` tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "refactor(picker): flip session/Create-new boundary in pick_choice

Index 0 is now 'Create new' (rendered at the top); session rows occupy
indices 1..=n_sessions. Wrap math unchanged. Updates the boundary-asserting
tests to the new convention."
```

---

## Task 2: Reorder the render loop and remap outcomes in `pick_session`

**Files:**
- Modify: `crates/zoid/src/main.rs` — `pick_session` function (~lines 651-862)

**Interfaces:**
- Consumes: the new `pick_choice` convention from Task 1 (index 0 = Create-new, indices 1..=n = sessions).
- Produces: `pick_session` renders "Create new" at the top (line 2), sessions below it, and maps `PickOutcome::Resume(idx)` / `DeleteConfirm(idx)` to `sessions[idx - 1]`.

- [ ] **Step 1: Change the initial cursor to the first session**

In `pick_session`, find the line (currently ~line 707):

```rust
    let mut selected: usize = 0;
```

Replace with:

```rust
    // Index 0 = "Create new" (top row); index 1 = first session. Start on the
    // most-recent session so the common case (resume recent) is one Enter away.
    let mut selected: usize = if n > 0 { 1 } else { 0 };
```

- [ ] **Step 2: Reorder the render — move "Create new" above the session rows**

In the `terminal.draw` closure, find the block that currently renders session rows first, then the delete-confirm line, then "Create new", then blank + hint (approximately lines 722-761). Replace the entire block from the session-row `for` loop through the hint `lines.push` with:

```rust
            // "Create new" is pinned at the top (line 2), directly under the
            // title/blank header, so it is always visible regardless of how
            // many sessions exist.
            let create_text = "  Create new session".to_string();
            let create_style = if selected == 0 {
                Style::new()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(create_text, create_style)));

            for (i, s) in sessions.iter().enumerate() {
                // Session rows occupy logical indices 1..=n, so session `i`
                // (0-indexed in `sessions`) is at logical index `i + 1`.
                let logical = i + 1;
                let age = fmt_since(s.last_touched_ts, boot_ts);
                let tokens = human_tokens(s.token_total);
                let live_marker = if live[i] { " ●" } else { "" };
                let row_text = format!("  {}  ·  {}  ·  {}{}", s.name, age, tokens, live_marker);
                let style = if logical == selected {
                    Style::new()
                        .fg(Color::White)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(row_text, style)));
            }

            if let Some(idx) = pending_delete {
                // `idx` is a logical index into the session space (1..=n).
                if let Some(s) = sessions.get(idx - 1) {
                    lines.push(Line::from(Span::styled(
                        format!(" Delete \"{}\"? [y]es / [n]o", s.name),
                        Style::new().fg(Color::Yellow),
                    )));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " ↑↓ move · ⏎ select · esc abort",
                Style::new().fg(Color::DarkGray),
            )));
```

- [ ] **Step 3: Update the scroll-offset line math for the new layout**

In the same `terminal.draw` closure, find the `selected_line` / `visible_height` / `scroll_y` block (currently ~lines 766-786). Replace the comment + `selected_line` calc with:

```rust
            // Keep the selected row on screen when the list is taller than the
            // terminal. Layout: line 0 = title, 1 = blank, 2 = "Create new",
            // 3.. = session rows. "Create new" (selected == 0) is at line 2 —
            // always within the first screen. Session rows (selected >= 1) are
            // at line 2 + selected. The delete-confirm line renders below the
            // session rows so it never shifts the selected row's line.
            let selected_line = if selected == 0 {
                2
            } else {
                2 + selected
            };
            // Visible height = inner area (borders take 2 rows).
            let visible_height = area.height.saturating_sub(2) as usize;
            let scroll_y = picker_scroll_offset(selected_line, visible_height);
            f.render_widget(
                Paragraph::new(lines).scroll((scroll_y, 0)).block(block),
                area,
            );
```

- [ ] **Step 4: Remap `PickOutcome` handling to the new index convention**

In the input-handling section of `pick_session` (currently ~lines 839-859), find the `match pick_choice(n, selected, pick_key) { ... }` block. Replace it with:

```rust
                match pick_choice(n, selected, pick_key) {
                    PickOutcome::Pending(new_sel) => selected = new_sel,
                    PickOutcome::Resume(idx) => {
                        // idx is a logical index (1..=n); session `idx - 1`.
                        let s = &sessions[idx - 1];
                        return Ok(PickResult::Resume {
                            id: s.id,
                            name: s.name.clone(),
                            created_ts: s.created_ts,
                        });
                    }
                    PickOutcome::CreateNew => return Ok(PickResult::CreateNew),
                    PickOutcome::Abort => {
                        anyhow::bail!("startup picker aborted");
                    }
                    PickOutcome::DeleteConfirm(idx) => {
                        // idx is a logical index (1..=n); session `idx - 1`.
                        let sess_idx = idx - 1;
                        if live.get(sess_idx).copied().unwrap_or(false) {
                            continue;
                        }
                        pending_delete = Some(idx);
                    }
                }
```

- [ ] **Step 5: Update the delete-confirmation reset/clamp for the new convention**

In the delete-confirmation handler (the `if let Some(idx) = pending_delete { ... }` block, currently ~lines 791-829), `idx` is a logical index stored in `pending_delete` (set in Step 4 as the logical `idx`). After `delete_session`, the code re-fetches `sessions` and recomputes `n`. The delete-confirm block accesses `sessions.get(idx)` — but `idx` is now a logical index, so it must use `sessions.get(idx - 1)`. Replace the entire `Char('y') | Char('Y') | Enter` arm (main.rs ~793-820, from the `Char('y')` line through the closing `pending_delete = None;` line) with:

```rust
                        crossterm::event::KeyCode::Char('y')
                        | crossterm::event::KeyCode::Char('Y')
                        | crossterm::event::KeyCode::Enter => {
                            // `idx` is a logical index (1..=n); session `idx - 1`.
                            if let Some(s) = sessions.get(idx - 1) {
                                let _ = session.delete_session(s.id).await;
                            }
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
                            // After a delete, clamp the cursor to a valid
                            // session row (1..=n). If no sessions remain,
                            // land on "Create new" (index 0). Never reset to 0
                            // when sessions still exist — index 0 is "Create
                            // new", not the first session.
                            if n == 0 {
                                selected = 0;
                            } else if selected > n {
                                selected = n;
                            }
                            pending_delete = None;
                        }
```

- [ ] **Step 6: Build and run the full test suite**

Run: `cargo test -p zoid --bin zoid 2>&1 | tail -15`
Expected: PASS — all tests green (the `pick_choice` tests from Task 1, the `picker_scroll_offset` tests which Task 3 Step 1 will have updated, and all other tests). The only pre-existing warning is `unused variable: root` in a worktree test, unrelated to this change.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(picker): move 'Create new' to the top of the startup picker

Pins 'Create new' at line 2 (below the title/blank header) so it is always
visible regardless of session count. Cursor starts on the most-recent
session (index 1); 'Create new' (index 0) is visible but unselected. Remaps
Resume/DeleteConfirm outcomes to sessions[idx - 1] and updates the
delete-confirmation clamp so a delete never dumps the cursor on 'Create new'
when sessions still exist."
```

---

## Task 3: Update stale `picker_scroll_offset` tests and doc comments to the new layout

**Files:**
- Modify: `crates/zoid/src/main.rs` — `picker_scroll_offset` doc comment (~lines 631-637), `pick_session` doc comment (~lines 669-675), and the `picker_scroll_offset` test block (~lines 9090-9142)

**Interfaces:**
- Consumes: the new row order from Task 2 ("Create new" at line 2, sessions at lines 3+).
- Produces: tests and doc comments that describe the actual layout, not the old one. The pure `picker_scroll_offset` function and its signature are unchanged.

The `picker_scroll_offset` tests pass numerically under the new layout (the pure function is arithmetic-only), but their **comments and one test name describe the old layout** — e.g. `scroll_offset_keeps_create_new_row_visible` asserts "create-new is at line 2+10=12," which is now false (create-new is pinned at line 2). Leaving them stale creates an intent/documentation regression: a future maintainer reads the test name, believes create-new needs scroll protection, and "fixes" something that isn't broken. The plan's Self-Review previously claimed these tests were "verified unchanged" — that was wrong.

- [ ] **Step 1: Rewrite the `picker_scroll_offset` test block to reflect the new layout**

Replace the entire `// --- picker_scroll_offset tests ---` block (from the comment line at ~line 9090 through `scroll_offset_zero_when_visible_height_exceeds_content` at ~line 9142) with:

```rust
    // --- picker_scroll_offset tests ---
    // The startup picker can list more sessions than fit on screen. The pure
    // y-offset keeps the selected row within the visible window. Layout (new):
    // line 0 = title, 1 = blank, 2 = "Create new", 3.. = session rows.
    // "Create new" is pinned at line 2 and can never clip — the offset's job is
    // to keep the selected *session* row visible, not to rescue "Create new".

    #[test]
    fn scroll_offset_zero_when_everything_fits() {
        // A short list fits entirely within a tall terminal; no scrolling.
        assert_eq!(picker_scroll_offset(2, 20), 0); // "Create new" at line 2
        assert_eq!(picker_scroll_offset(5, 20), 0); // a session row at line 5
    }

    #[test]
    fn scroll_offset_advances_when_cursor_moves_below_view() {
        // visible_height=4. Lines 0..3 visible initially. Selecting line 5
        // (a session row in a long list) must scroll so line 5 is the last
        // visible row → offset = 5 - 4 + 1 = 2.
        assert_eq!(picker_scroll_offset(5, 4), 2);
    }

    #[test]
    fn scroll_offset_keeps_last_session_row_visible() {
        // 10 sessions → last session row is at line 2 + 10 = 12. visible_height=5.
        // Offsetting by 12-5+1=8 puts line 12 as the last visible row.
        // (Under the old layout this test was named "keeps_create_new_row_visible"
        // — but "Create new" is now pinned at line 2 and never needs this.)
        assert_eq!(picker_scroll_offset(12, 5), 8);
    }

    #[test]
    fn scroll_offset_create_new_never_triggers_scroll() {
        // "Create new" is at line 2 — always within the first screen regardless
        // of visible_height, so selecting it always yields offset 0.
        assert_eq!(picker_scroll_offset(2, 20), 0);
        assert_eq!(picker_scroll_offset(2, 5), 0);
        assert_eq!(picker_scroll_offset(2, 3), 0);
    }

    #[test]
    fn scroll_offset_only_grows_never_shrinks_jumps_back() {
        // Once scrolled down, moving the cursor back up should pull the offset
        // back so the selected row is the *first* visible one when it would
        // otherwise be clipped at the top. visible_height=4, selected=3 →
        // offset 0 (line 3 is the last of the 0..3 window). selected=2 → 0.
        assert_eq!(picker_scroll_offset(3, 4), 0);
        assert_eq!(picker_scroll_offset(2, 4), 0);
    }

    #[test]
    fn scroll_offset_clamps_when_cursor_far_above_window() {
        // If the selected line is behind the current natural offset, the offset
        // must drop so the selected line becomes visible. selected=2, h=4 → 0.
        assert_eq!(picker_scroll_offset(2, 4), 0);
    }

    #[test]
    fn scroll_offset_zero_when_visible_height_exceeds_content() {
        // selected line within the first screen even for huge heights.
        assert_eq!(picker_scroll_offset(0, 100), 0);
        assert_eq!(picker_scroll_offset(50, 100), 0);
    }
```

- [ ] **Step 2: Update the `picker_scroll_offset` doc comment to describe the new layout**

Find the doc comment on `picker_scroll_offset` (~lines 631-637):

```rust
/// Compute the vertical scroll offset (in lines) for the startup picker's
/// `Paragraph` so that the selected row stays within the visible window.
///
/// `selected_line` is the index of the cursor row within the `lines` Vec the
/// picker builds (title + blank + session rows + optional delete-confirm +
/// "Create new" + blank + hint). `visible_height` is the inner area height
/// (the block's bordered area minus 2).
```

Replace the paragraph in parentheses (the line starting `` `selected_line` is the index``) with:

```rust
/// `selected_line` is the index of the cursor row within the `lines` Vec the
/// picker builds (title + blank + "Create new" + session rows + optional
/// delete-confirm + blank + hint). `visible_height` is the inner area height
/// (the block's bordered area minus 2).
```

- [ ] **Step 3: Update the `pick_session` doc comment to describe the new layout**

Find the doc comment on `pick_session` (~lines 669-675):

```rust
/// The startup session picker (spec §2). A self-contained render+input loop
/// entered after crossterm raw mode is set up but before `run()`. Shows one row
/// per session for the current CWD (name, age, tokens, live marker) plus a
/// trailing "Create new session" row. Arrow keys move, Enter selects, Esc
```

Replace the phrase `plus a trailing "Create new session" row` with:

```rust
/// per session for the current CWD (name, age, tokens, live marker) plus a
/// leading "Create new session" row (pinned at the top, always visible). Arrow
```

(That is: change `trailing` → `leading`, and insert the parenthetical so the doc reflects the pinning behavior.)

- [ ] **Step 4: Run the full test suite**

Run: `cargo test -p zoid --bin zoid 2>&1 | tail -20`
Expected: PASS — all `picker_scroll_offset` tests green under their new names/comments, plus the `pick_choice` tests and everything else. The pure function's arithmetic is unchanged, so the assertions still hold; only the test data/comments/names changed.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "docs(picker): update stale scroll-offset tests and doc comments

The picker_scroll_offset tests and two doc comments described the old
bottom-anchored 'Create new' layout. Under the new top-pinned layout those
descriptions are wrong (e.g. create-new is at line 2, never line 2+n).
Renames scroll_offset_keeps_create_new_row_visible →
scroll_offset_keeps_last_session_row_visible, adds
scroll_offset_create_new_never_triggers_scroll, and fixes the comments on
picker_scroll_offset and pick_session to match the actual row order."
```

---

## Self-Review

**1. Spec coverage:**
- §1 New row order → Task 2 Step 2 (render reorder). ✓
- §2 Logical index remap / `pick_choice` boundary flip → Task 1. ✓
- §2 `selected` init to 1 → Task 2 Step 1. ✓
- §2 `PickOutcome` handling remap → Task 2 Step 4. ✓
- §3 Render changes (highlight tests) → Task 2 Step 2 (`selected == 0` for Create-new, `logical == selected` for sessions). ✓
- §4 Scroll offset line math → Task 2 Step 3. ✓
- §4 drop `+ pending_delete` term → Task 2 Step 3 (the new calc has no `pending_delete` term). ✓
- §5 Initial cursor → Task 2 Step 1. ✓
- §6 What is not touched → no tasks touch those areas. ✓
- §7 Testing → Task 1 updates `pick_choice` boundary tests; Task 3 updates the `picker_scroll_offset` tests (their comments and one test name encoded the old layout — the pure function is unchanged but the test *data and prose* did not match the new layout) and the stale doc comments on `picker_scroll_offset` / `pick_session`. ✓

**2. Placeholder scan:** No TBD/TODO. All steps contain exact code. ✓

**3. Type consistency:** `pick_choice` signature unchanged (`(usize, usize, PickKey) -> PickOutcome`). `PickOutcome::Resume(idx)` / `DeleteConfirm(idx)` still carry `usize`; the meaning shifts (idx is now logical, `>= 1` for sessions) but the type is identical. `pick_session` maps `idx - 1` to index `sessions`, consistent across Steps 4 and 5. `picker_scroll_offset` signature unchanged. ✓

**4. Edge case check — delete down to 0 sessions:** Task 2 Step 5's clamp handles `n == 0 → selected = 0` (Create new, the only row). With `n == 0`, `pick_choice(0, 0, key)`: `total = 1`, Up from 0 → 0, Down from 0 → 0, Enter on 0 → `CreateNew`. Correct — the only row is "Create new". ✓

**5. Edge case check — delete down to 1 session:** `n == 1`, `selected` was the deleted row. If `selected > n` (e.g. `selected = 2`, now `n = 1`) → `selected = 1` (the remaining session). If `selected = 1` (still valid) → unchanged. The picker shows 1 session + "Create new" (2 rows). ✓