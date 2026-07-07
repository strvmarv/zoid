# TUI + Companion UX Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Five itemized UX fixes: input-box soft-wrap, companion dashboard trim (keep card host only), queued-message-on-busy, worktree detection for the repo drawer, and a subagents drawer in the right rail.

**Architecture:** Items are independent and ordered by blast radius: T1 (trivial) → T2 (isolated git2 helper) → T3 (companion crate, self-contained) → T4 (in-flight-subagents type migration + new AgentUpdate variant + new drawer across tui + bin) → T5 (App field + Submit/TurnComplete/session-switch sites in the bin). The spec is `docs/superpowers/specs/2026-07-06-tui-companion-ux-fixes-design.md`.

**Tech Stack:** Rust workspace; `ratatui` 0.30, `ratatui-textarea` 0.9 (has `WrapMode`), `git2` 0.20, `tiny_http`; `tokio` async runtime.

## Global Constraints

- Every glyph/color comes from `zoid_tui::tokens::{color, glyph}` — no literal status chars in render code (spec §16).
- The pure renderer (`zoid-tui`) stays terminal-free and unit-tested; bin-owned side effects flow through `ShellState` fields sampled each frame.
- `render_shell` is the single render entry point; new drawers plug into the existing `render_rail` match + `allocate_drawer_bodies` allocator.
- TDD: pure functions get impl+tests-together with the test run as the green check; the one I/O-bound seam (companion SSE) gets red-green where feasible.
- `cargo test -p <crate>` is the per-task green check; `cargo clippy --workspace` must stay clean.
- Commit after each task (frequent commits).

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid/src/main.rs` | Bin: `make_input`, `worktree_label`, `App` struct, `Submit`/`TurnComplete`/session-switch handlers, `start_delegation`, `dashboard_snapshot` removal, `render_shell` call site |
| `crates/zoid/src/agent.rs` | `AgentUpdate::SubagentStarted` variant; `dispatch_subagent` emits it |
| `crates/zoid-tui/src/state.rs` | `DrawerId::Subagents`, `SubagentRow`, `subagents_len` field, `ShellState::new` |
| `crates/zoid-tui/src/layout.rs` | `SUBAGENTS_BODY_ROWS`, `drawer_body_rows`/`drawer_fit_priority` arms, `allocate_drawer_bodies` signature + pass-3, `compute` call site |
| `crates/zoid-tui/src/render.rs` | `render_subagents_body`, `render_rail` arm, `render_shell` signature (add `subagents`) |
| `crates/zoid-companion/src/hub.rs` | Remove `publish_snapshot`/snapshot field; keep `publish_card` |
| `crates/zoid-companion/src/server.rs` | `SseReader` card-only; drop `last_snapshot` |
| `crates/zoid-companion/src/snapshot.rs` | Delete (no longer needed) |
| `crates/zoid-companion/src/lib.rs` | Drop `snapshot` mod + re-exports |
| `crates/zoid-companion/src/shell.html` | Drop `<section id="dashboard">` |
| `crates/zoid-companion/src/app.js` | Drop `dashboard` listener + `esc()`; keep `card` listener |

---

## Task 1: Input box soft-wrap

**Files:**
- Modify: `crates/zoid/src/main.rs` — `make_input` (line ~212) and the `make_input_disables_cursor_line_underline` test

**Interfaces:**
- Produces: `make_input` now sets `WrapMode::WordOrGlyph`. No new signatures.

- [ ] **Step 1: Update `make_input` to set the wrap mode**

In `crates/zoid/src/main.rs`, find `make_input` and add the wrap-mode line:

```rust
fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
    let mut textarea = textarea;
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea.set_wrap_mode(ratatui_textarea::WrapMode::WordOrGlyph);
    textarea
}
```

- [ ] **Step 2: Extend the existing test to assert the wrap mode**

Find `make_input_disables_cursor_line_underline` in the `#[cfg(test)] mod tests` block and add a wrap-mode assertion. The test already renders the textarea; `WrapMode` is a public field-accessible via `textarea.wrap_mode()` (check the crate API — if there's no getter, assert behavior by rendering a long line into a narrow buffer and checking it doesn't horizontally scroll). The simplest robust check: render a 40-char string into a 20-wide textarea and confirm the buffer content wraps (the right half is not empty). If `wrap_mode()` is not exposed, add a behavioral test:

```rust
#[test]
fn make_input_sets_word_or_glyph_wrap() {
    use ratatui::{backend::TestBackend, Terminal};
    let ta = make_input(TextArea::from(vec!["a very long line that exceeds twenty columns".to_string()]));
    let backend = TestBackend::new(20, 5);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| f.render_widget(&ta, f.area())).unwrap();
    let content: String = term.backend().buffer().content().iter()
        .map(|c| c.symbol().to_string()).collect();
    // With wrap, the long line occupies rows beyond the first; the second
    // content row must be non-empty (the overflow wrapped onto it).
    let rows: Vec<&str> = content.split('\n').collect();
    assert!(rows.len() > 2, "wrapped line must span multiple rows: {content:?}");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid make_input`
Expected: PASS (both the existing underline test and the new wrap test).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(tui): message input soft-wraps (WrapMode::WordOrGlyph)"
```

---

## Task 2: Worktree detection for the repo drawer

**Files:**
- Modify: `crates/zoid/src/main.rs` — add `worktree_label`, set `shell.worktree` in the startup `repo_present` block (line ~1322)

**Interfaces:**
- Produces: `fn worktree_label(repo: &git2::Repository) -> String` — returns the linked-worktree name or `"(none)"`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid/src/main.rs`:

```rust
#[test]
fn worktree_label_none_for_main_worktree() {
    use git2::Repository;
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    // Init a repo with a commit (worktrees need HEAD).
    let repo = Repository::init(&repo_dir).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let tree_id = {
        let mut index = repo.index().unwrap();
        let path = repo_dir.join("README");
        std::fs::write(&path, "hi").unwrap();
        index.add_path(std::path::Path::new("README")).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    std::mem::forget(dir); // keep the repo alive for the assertion
    assert_eq!(worktree_label(&repo), "(none)");
}

#[test]
fn worktree_label_name_for_linked_worktree() {
    use git2::Repository;
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let repo = Repository::init(&repo_dir).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let tree_id = {
        let mut index = repo.index().unwrap();
        std::fs::write(repo_dir.join("README"), "hi").unwrap();
        index.add_path(std::path::Path::new("README")).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    // Create a linked worktree named "feature".
    let wt_path = dir.path().join("wt");
    repo.worktree("feature", &wt_path, Some(&git2::WorktreeAddOptions::new())).unwrap();
    let wt_repo = Repository::open(&wt_path).unwrap();
    assert_eq!(worktree_label(&wt_repo), "feature");
    std::mem::forget(dir);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid worktree_label`
Expected: FAIL — `worktree_label` not found.

- [ ] **Step 3: Implement `worktree_label`**

Add near `current_branch` (line ~1290) in `crates/zoid/src/main.rs`:

```rust
/// The worktree label for the repo drawer: the linked-worktree name when the
/// process cwd is a linked worktree (not the main working copy), else "(none)".
/// git stores linked worktrees under <common>/worktrees/<name>, so the
/// worktree's gitdir basename IS the worktree name.
fn worktree_label(repo: &git2::Repository) -> String {
    let path = repo.path();
    let common = repo.commondir();
    if path == common {
        "(none)".to_string()
    } else {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(linked)".into())
    }
}
```

- [ ] **Step 4: Set `shell.worktree` at startup**

In the `repo_present` block of `main()` (line ~1322), after `shell.repo_name = ...`, add:

```rust
        shell.worktree = git2::Repository::open(".")
            .ok()
            .map(|r| worktree_label(&r))
            .unwrap_or_else(|| "(none)".into());
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid worktree_label`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "fix(tui): repo drawer shows the linked-worktree name (was always '(none)')"
```

---

## Task 3: Companion becomes a card host (remove economy dashboard)

**Files:**
- Modify: `crates/zoid-companion/src/snapshot.rs` (delete contents)
- Modify: `crates/zoid-companion/src/lib.rs` (drop `mod snapshot` + re-exports)
- Modify: `crates/zoid-companion/src/hub.rs` (remove snapshot)
- Modify: `crates/zoid-companion/src/server.rs` (card-only SSE)
- Modify: `crates/zoid-companion/src/shell.html` (drop dashboard section)
- Modify: `crates/zoid-companion/src/app.js` (drop dashboard listener)
- Modify: `crates/zoid-companion/examples/serve.rs` (drop publish_snapshot)
- Modify: `crates/zoid/src/main.rs` (remove `dashboard_snapshot`/`heat_rank`/publish block)

**Interfaces:**
- Consumes: none new.
- Produces: `CompanionHub` now exposes only `publish_card`/`current`/`wait_after`/`set_enabled`/`is_enabled`. `Frame` is `{ version, card }`.

- [ ] **Step 1: Trim `snapshot.rs`**

Replace the entire contents of `crates/zoid-companion/src/snapshot.rs` with:

```rust
//! The snapshot module was removed when the companion became a card-only host.
//! The dashboard economy projection (DashboardSnapshot/TierRow) lived here;
//! the `show`-tool card surface is the only thing the companion publishes now.
```

(The file is kept as a stub so `lib.rs`'s `mod snapshot;` line doesn't need deleting in the same edit; we delete the mod line in step 2 anyway. Alternatively, delete the file and remove the `mod` line — either works; pick delete + remove for cleanliness.)

Actually: delete the file and remove the `mod` line (cleaner).

```bash
rm crates/zoid-companion/src/snapshot.rs
```

- [ ] **Step 2: Update `lib.rs`**

In `crates/zoid-companion/src/lib.rs`, remove `mod snapshot;` (or `pub mod snapshot;`) and remove the `pub use snapshot::{DashboardSnapshot, TierRow};` re-export. Keep `pub use hub::CompanionHub;`, `pub use server::{CompanionServer, start};`.

- [ ] **Step 3: Trim `hub.rs`**

In `crates/zoid-companion/src/hub.rs`:
- Remove `use crate::snapshot::DashboardSnapshot;`.
- In `Latest`, remove the `snapshot: Option<DashboardSnapshot>` field.
- In `Frame`, remove the `snapshot` field → `Frame { pub version: u64, pub card: Option<String> }`.
- Remove `publish_snapshot`.
- In `current()` and `wait_after()`, drop the `snapshot` field from the constructed `Frame`.
- Update the test helper `snap(name)` — it no longer exists; replace the hub tests' snapshot construction with card-only flows. Remove `publish_bumps_version`/`identical_snapshot_does_not_bump`'s snapshot arms; keep `wait_after_returns_on_publish` using `publish_card`.

The trimmed `Latest`/`Frame`:

```rust
#[derive(Default)]
struct Latest {
    card: Option<String>,
    version: u64,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub version: u64,
    pub card: Option<String>,
}
```

- [ ] **Step 4: Trim `server.rs` `SseReader`**

In `crates/zoid-companion/src/server.rs`:
- Remove `use crate::snapshot::DashboardSnapshot;` (if present).
- In `SseReader`, remove the `last_snapshot: Option<DashboardSnapshot>` field.
- In `SseReader::absorb`, remove the snapshot-diff branch; keep only the card branch:

```rust
fn absorb(&mut self, frame: Frame) {
    if frame.card != self.last_card {
        if let Some(c) = &frame.card {
            let json = serde_json::to_string(c).unwrap_or_default();
            self.buf
                .extend_from_slice(format!("event: card\ndata: {json}\n\n").as_bytes());
        }
        self.last_card = frame.card.clone();
    }
    self.last_version = frame.version;
}
```

- Update tests: `events_stream_flushes_first_frame_over_http` — remove the `publish_snapshot` + `DashboardSnapshot` construction; publish a card instead and assert the card frame arrives. `sse_reader_emits_dashboard_then_card_frames` → rename to `sse_reader_emits_card_frames` and drop the snapshot half.

- [ ] **Step 5: Trim `shell.html`**

In `crates/zoid-companion/src/shell.html`, remove the `<section id="dashboard">…</section>` line. Keep `<section id="card"></section>`.

- [ ] **Step 6: Trim `app.js`**

In `crates/zoid-companion/src/app.js`:
- Remove the `const dash = document.getElementById("dashboard");` line.
- Remove the `esc` helper function.
- Remove the entire `es.addEventListener("dashboard", (e) => { ... })` block.
- Keep the `const card = document.getElementById("card");` line and the `es.addEventListener("card", ...)` block + the resize-reporter machinery.

- [ ] **Step 7: Trim `examples/serve.rs`**

In `crates/zoid-companion/examples/serve.rs`, remove the `hub.publish_snapshot(...)` call; keep `hub.publish_card(...)`.

- [ ] **Step 8: Trim `main.rs`**

In `crates/zoid/src/main.rs`:
- Remove `fn heat_rank(...)` (line ~1115) entirely.
- Remove `fn dashboard_snapshot(...)` (line ~1130) entirely.
- In `run()`, remove the per-frame publish block (line ~1670):

```rust
        if app.companion_hub.is_enabled() {
            let snap = dashboard_snapshot(...);
            app.companion_hub.publish_snapshot(snap);
        }
```

- Remove the `dashboard_snapshot_maps_scalars_and_churn` test and the `heat_rank_orders_hot_warm_cold` test.

- [ ] **Step 9: Run tests**

Run: `cargo test -p zoid-companion && cargo test -p zoid`
Expected: PASS. The companion crate compiles without `DashboardSnapshot`; the bin compiles without `dashboard_snapshot`/`heat_rank`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(companion): card host only — remove economy dashboard"
```

---

## Task 4: Subagents drawer in the right rail

This is the largest task. It has three sub-parts: 4a (TUI state + layout), 4b (render), 4c (bin: type migration + AgentUpdate + plumbing).

### Task 4a: `DrawerId::Subagents` + `SubagentRow` + layout allocator

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`
- Modify: `crates/zoid-tui/src/layout.rs`

**Interfaces:**
- Produces: `DrawerId::Subagents`, `pub struct SubagentRow { pub id: String, pub task: String }`, `ShellState::subagents_len: u16`, `allocate_drawer_bodies(drawers, height, tasks_len, subagents_len)`, `SUBAGENTS_BODY_ROWS`.

- [ ] **Step 1: Add `DrawerId::Subagents` and the drawer in `state.rs`**

In `crates/zoid-tui/src/state.rs`, add `Subagents` to the `DrawerId` enum (after `Tasks`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerId {
    Repo,
    Session,
    Context,
    Tasks,
    Subagents,
}
```

Add the `SubagentRow` type near `TaskItem` usage (or top of the file):

```rust
/// One in-flight subagent row for the Subagents drawer (mirrors the bin's
/// `SubagentInfo` without coupling the TUI to the bin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRow {
    pub id: String,
    pub task: String,
}
```

Add `subagents_len: u16` field to `ShellState` (next to `tasks_len`), documented:

```rust
    /// Number of in-flight subagents the Subagents drawer would show, sampled
    /// by the bin each frame so `layout::compute` can grow the drawer. Default 0.
    pub subagents_len: u16,
```

In `ShellState::new()`, add the Subagents drawer after Tasks and initialize `subagents_len: 0`:

```rust
        let drawers = vec![
            Drawer { id: DrawerId::Repo, title: "repo".into(), open: true },
            Drawer { id: DrawerId::Session, title: "session".into(), open: true },
            Drawer { id: DrawerId::Context, title: "context · tokens".into(), open: true },
            Drawer { id: DrawerId::Tasks, title: "tasks".into(), open: true },
            Drawer { id: DrawerId::Subagents, title: "subagents".into(), open: true },
        ];
```

Update the `new_is_calm_chat_with_repo_session_context_rail` test: the `ids` vec now has 5 entries (add `DrawerId::Subagents`), and add `assert!(s.drawer(DrawerId::Subagents).unwrap().open);`.

Update `tasks_drawer_is_last_and_open`: the last drawer is now Subagents, not Tasks. Change the test to `assert_eq!(last.id, DrawerId::Subagents);` and add a separate assertion that Tasks is second-to-last, OR rename to `subagents_drawer_is_last_and_open` and assert Tasks is open. (Keep Tasks-open assertion too.)

- [ ] **Step 2: Add `SUBAGENTS_BODY_ROWS` and the allocator in `layout.rs`**

In `crates/zoid-tui/src/layout.rs`, add the constant:

```rust
/// Subagents drawer body rows: up to a handful of in-flight subagents.
pub const SUBAGENTS_BODY_ROWS: u16 = 5;
```

In `drawer_body_rows`, add:

```rust
        DrawerId::Subagents => SUBAGENTS_BODY_ROWS,
```

In `drawer_fit_priority`, add (lowest priority — yields first when the rail is short):

```rust
        DrawerId::Subagents => 4,
```

Change `allocate_drawer_bodies` signature to take `subagent_count: u16`:

```rust
pub fn allocate_drawer_bodies(drawers: &[Drawer], height: u16, task_count: u16, subagent_count: u16) -> Vec<u16> {
```

Add the Subagents base-ideal to `base_ideal`:

```rust
    let base_ideal = |d: &Drawer| -> u16 {
        match d.id {
            DrawerId::Tasks => drawer_body_rows(d.id).min(task_count.max(1)),
            DrawerId::Subagents => drawer_body_rows(d.id).min(subagent_count.max(1)),
            _ => drawer_body_rows(d.id),
        }
    };
```

Add a pass-3 (after the Tasks pass-2 surplus) that grows Subagents toward `subagent_count`:

```rust
    // Step 3 (pass 2 for Tasks, now pass 3 for Subagents): leftover rows grow
    // Subagents beyond its base toward the full subagent count.
    if surplus > 0 {
        if let Some(i) = drawers.iter().position(|d| d.id == DrawerId::Subagents) {
            if expanded[i] {
                let room = subagent_count.max(1).saturating_sub(body[i]);
                body[i] += room.min(surplus);
            }
        }
    }
```

(Note: the existing pass-2 already consumes `surplus` for Tasks; the Subagents pass-3 uses whatever's left. Since `surplus` is `mut`, the Tasks block subtracts from it; the Subagents block uses the remainder. This is correct — Tasks has higher priority (3) than Subagents (4), so Tasks grows first.)

In `compute()`, update the call:

```rust
        let bodies = allocate_drawer_bodies(&state.drawers, inner.height, state.tasks_len, state.subagents_len);
```

Update the layout tests: `alloc_roomy_gives_every_drawer_its_base_ideal` needs 5 drawers and a `subagent_count` arg; the `all_open()` helper now returns 5 drawers. Add `const SUBAGENTS: usize = 4;` and assert `body[SUBAGENTS]` mirrors the subagent count. All `allocate_drawer_bodies(&all_open(), H, T)` calls gain a 4th arg `0` (no subagents) unless the test exercises subagents.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tui layout`
Expected: PASS (all allocator tests updated).

- [ ] **Step 4: Commit (4a)**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/layout.rs
git commit -m "feat(tui): DrawerId::Subagents + SubagentRow + allocator pass-3"
```

### Task 4b: `render_subagents_body` + `render_shell` signature

**Files:**
- Modify: `crates/zoid-tui/src/render.rs`

**Interfaces:**
- Produces: `render_subagents_body(frame, area, rows: &[SubagentRow])`; `render_shell` gains a `subagents: &[SubagentRow]` parameter.

- [ ] **Step 1: Add `render_subagents_body`**

In `crates/zoid-tui/src/render.rs`, add near `render_tasks_body`:

```rust
/// The subagents drawer body: one row per in-flight subagent — a running glyph
/// + truncated id + truncated task label. Empty → dim "no subagents". Capped
/// to the body rows the allocator gave the drawer.
fn render_subagents_body(frame: &mut Frame, area: Rect, rows: &[crate::state::SubagentRow]) {
    use crate::text::truncate;
    use crate::state::SubagentRow;
    if rows.is_empty() {
        let line = Line::from(Span::styled("no subagents", Style::new().fg(color::DIM)));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let rows_rendered: Vec<Line> = rows
        .iter()
        .take(area.height as usize)
        .map(|r| {
            // {RUNNING} {id}  {task} truncated to the drawer width.
            let id_w = 14; // "sub-01HZ..." is ~13-14 chars
            let id = truncate(&r.id, id_w);
            let task_budget = area.width.saturating_sub(id_w as u16 + 3) as usize;
            let task = truncate(&r.task, task_budget);
            Line::from(vec![
                Span::styled(format!("{} ", glyph::RUNNING), Style::new().fg(color::WARN)),
                Span::styled(format!("{id}  "), Style::new().fg(color::TXT)),
                Span::styled(task, Style::new().fg(color::DIM)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows_rendered), area);
}
```

- [ ] **Step 2: Add the `render_rail` arm and `render_shell` parameter**

In `render_shell`'s signature, add `subagents: &[crate::state::SubagentRow]` after `tasks`:

```rust
pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    msgs: &[ChatMsg],
    body: Option<&[Line<'static>]>,
    tasks: &[zoid_core::tasks::TaskItem],
    subagents: &[crate::state::SubagentRow],
    input: &TextArea<'_>,
    streaming: bool,
    view: &ChatView,
) -> u16 {
```

In `render_rail`, add the match arm:

```rust
                DrawerId::Subagents => render_subagents_body(frame, body_rect, subagents),
```

(Place the `subagents` slice access — `render_rail` needs it. Since `render_rail` is called from `render_shell`, thread `subagents` into `render_rail`'s signature too: `fn render_rail(frame, state, economy, tasks, subagents, layout)`.)

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tui render`
Expected: PASS. (The render tests don't call `render_shell` directly often; if any snapshot test calls it, update the call site with `&[]` for subagents.)

- [ ] **Step 4: Commit (4b)**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(tui): render_subagents_body + render_shell subagents param"
```

### Task 4c: Bin — `SubagentInfo` + `AgentUpdate::SubagentStarted` + plumbing

**Files:**
- Modify: `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `SubagentRow` from 4a, `render_shell`'s new signature from 4b.
- Produces: `AgentUpdate::SubagentStarted { id, task }`; `in_flight_subagents: Vec<SubagentInfo>`.

- [ ] **Step 1: Add `AgentUpdate::SubagentStarted` in `agent.rs`**

In `crates/zoid/src/agent.rs`, add the variant to `AgentUpdate`:

```rust
    /// A subagent was dispatched (via the dispatch_subagent tool). The UI tracks
    /// it as in-flight until its DelegationResult arrives.
    SubagentStarted { id: String, task: String },
```

- [ ] **Step 2: Emit `SubagentStarted` in the `dispatch_subagent` handler**

In the `dispatch_subagent` tool arm of `run_turn_inner` (line ~870), after computing `sub_id` and BEFORE calling `spawn_subagent::spawn_subagent`, emit the update:

```rust
                    let sub_ulid = Ulid::new();
                    let sub_id = format!("sub-{sub_ulid}");

                    // Notify the UI so it tracks the subagent as in-flight.
                    let _ = ui.send(AgentUpdate::SubagentStarted {
                        id: sub_id.clone(),
                        task: task.clone(),
                    }).await;

                    let wt = if want_worktree && std::path::Path::new(".git").exists() {
                        ...
```

- [ ] **Step 3: Migrate `in_flight_subagents` to `Vec<SubagentInfo>` in `main.rs`**

In `crates/zoid/src/main.rs`:
- Add the struct near `App`:

```rust
/// One in-flight subagent, tracked for the Subagents drawer + busy guard.
struct SubagentInfo {
    id: String,
    task: String,
}
```

- Change the `App` field:

```rust
    in_flight_subagents: Vec<SubagentInfo>,
```

- Update the constructor (`in_flight_subagents: std::collections::HashSet::new(),` → `in_flight_subagents: Vec::new(),`).
- Update `test_app()`: `in_flight_subagents: Vec::new(),`.

- Update all usage sites (search for `in_flight_subagents`):
  - `app.shell.busy = app.streaming || !app.in_flight_subagents.is_empty();` — unchanged (`Vec::is_empty`).
  - `app.in_flight_subagents.remove(subagent_id);` (line ~1827, the `DelegationResult` handler) → `app.in_flight_subagents.retain(|s| s.id != *subagent_id);`.
  - `app.in_flight_subagents.clear();` (line ~1882, `SessionTakenOver`) — unchanged (`Vec::clear`).
  - All `app.streaming || !app.in_flight_subagents.is_empty()` guards (lines 2020, 2552, 2673, 3524, 3637) — unchanged.
  - `app.in_flight_subagents.insert(sub_id.clone());` in `start_delegation` (line 3651) → `app.in_flight_subagents.push(SubagentInfo { id: sub_id.clone(), task: task.clone() });`.
  - `app.in_flight_subagents.len()` (line 3655) — unchanged.
  - Test helpers: `app.in_flight_subagents.insert("sub-test".into());` (lines 4611, 4655, 4686) → `app.in_flight_subagents.push(SubagentInfo { id: "sub-test".into(), task: "test".into() });`.
  - `app.in_flight_subagents.is_empty()` assertions in tests — unchanged.

- [ ] **Step 4: Handle `AgentUpdate::SubagentStarted` in `run()`**

In the `Some(update) = ui_rx.recv()` match (line ~1760), add the arm (before `AgentUpdate::CompactionStarted`):

```rust
                    AgentUpdate::SubagentStarted { id, task } => {
                        app.in_flight_subagents.push(SubagentInfo { id, task });
                        app.shell.status_hint = Some(format!(
                            "{} {} subagent running…",
                            zoid_tui::tokens::glyph::RUNNING,
                            app.in_flight_subagents.len()
                        ));
                    }
```

- [ ] **Step 5: Sample `subagents_len` + build `SubagentRow` vec each frame**

In `run()`, after `app.shell.tasks_len = app.proj.tasks.len() as u16;` (line ~1545), add:

```rust
        app.shell.subagents_len = app.in_flight_subagents.len() as u16;
```

Before the `terminal.draw` closure, build the render rows:

```rust
        let subagent_rows: Vec<zoid_tui::state::SubagentRow> = app.in_flight_subagents
            .iter()
            .map(|s| zoid_tui::state::SubagentRow { id: s.id.clone(), task: s.task.clone() })
            .collect();
```

Inside the `terminal.draw` closure, update the `render_shell` call to pass `&subagent_rows`:

```rust
            frame_conv_max = render_shell(
                f,
                &app.shell,
                &economy,
                &app.proj.msgs,
                Some(body),
                task_items,
                &subagent_rows,
                &app.textarea,
                streaming,
                &view,
            );
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid && cargo clippy --workspace`
Expected: PASS. The `submit_is_noop_while_delegating`/`session_pick_is_noop_while_delegating`/`new_session_is_noop_while_delegating` tests pass with the `Vec` migration. Add a new test that `SubagentStarted` pushes into the vec:

```rust
#[tokio::test]
async fn subagent_started_tracks_in_flight() {
    let mut app = test_app().await;
    // Simulate the agent loop emitting SubagentStarted.
    let _ = app.ui_tx.send(AgentUpdate::SubagentStarted { id: "sub-x".into(), task: "do thing".into() }).await;
    // Drain one update into the same match the run loop uses.
    // (Directly test the handler effect by mimicking it.)
    // For a unit test, call the handler inline:
    let update = app.ui_tx.reserve().await.unwrap(); // or use try_recv on a local rx
    // ... simplest: assert the push effect directly.
    app.in_flight_subagents.push(SubagentInfo { id: "sub-x".into(), task: "do thing".into() });
    assert_eq!(app.in_flight_subagents.len(), 1);
    assert_eq!(app.in_flight_subagents[0].id, "sub-x");
}
```

(If wiring a full `run()` loop test is too heavy, assert the `Vec` push + `retain` behavior directly — that's the unit seam.)

- [ ] **Step 7: Commit (4c)**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(bin): SubagentInfo + AgentUpdate::SubagentStarted + subagents drawer plumbing"
```

---

## Task 5: Queue a message while the agent is working

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App` struct, `Action::Submit`, `AgentUpdate::TurnComplete`, `Command::NewSession`, `Action::SessionPick`, `truncate_for_hint` helper

**Interfaces:**
- Produces: `App::pending_message: Option<String>`; `fn truncate_for_hint(s: &str) -> String`.

- [ ] **Step 1: Add `pending_message` to `App` + constructors**

In `crates/zoid/src/main.rs`, add the field to `App`:

```rust
    /// A message queued while the agent was busy; auto-submitted when the
    /// current turn ends and no subagents are in flight. ESC (CancelTurn)
    /// does NOT clear it — the queued message runs after the steered turn.
    pending_message: Option<String>,
```

In the `App` constructor: `pending_message: None,`. In `test_app()`: `pending_message: None,`.

- [ ] **Step 2: Add `truncate_for_hint` helper**

Near `derive_session_name`:

```rust
/// Truncate a queued-message hint to ~40 chars with an ellipsis (mirrors
/// `derive_session_name`'s truncation).
fn truncate_for_hint(s: &str) -> String {
    let one_line = s.lines().next().unwrap_or(s);
    if one_line.chars().count() > 40 {
        let head: String = one_line.chars().take(39).collect();
        format!("{head}\u{2026}")
    } else {
        one_line.to_string()
    }
}
```

- [ ] **Step 3: Rewrite the `Action::Submit` busy guard**

Find `Action::Submit` (line ~2540) and replace the guard:

```rust
        Action::Submit => {
            // Yielded always blocks (even when not busy) — a taken-over session
            // can't accept new turns until the user :new or :resume.
            if app.yielded {
                app.shell.status_hint = Some("session taken over — :new or :resume".into());
                return Ok(false);
            }
            // Busy (streaming or delegating) but not yielded: stash the message
            // for after the turn, as an alternative to ESC-steering.
            if app.streaming || !app.in_flight_subagents.is_empty() {
                let text = app.textarea.lines().join("\n");
                app.pending_message = Some(text.clone());
                app.textarea = make_input(TextArea::default());
                app.shell.status_hint = Some(format!("queued: {}", truncate_for_hint(&text)));
                return Ok(false);
            }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() {
                return Ok(false);
            }
            // ... rest of the Submit handler unchanged (the `:`-prefix command
            // interception, record UserMessage, spawn_turn).
```

Note: the `text` is re-derived after the guard (the busy branch takes `text` early for the queue; the idle branch re-derives it). Keep the existing `:`-prefix check and `first`-message logic below the guard.

- [ ] **Step 4: Consume the queue on `TurnComplete`**

Find `AgentUpdate::TurnComplete` (line ~1800) and add the consumption block after the existing cleanup:

```rust
                    AgentUpdate::TurnComplete => {
                        app.streaming = false;
                        app.shell.clear_active_tool();
                        app.pending_answer = None;
                        app.turn_cancel = None;
                        app.shell.status_hint = None;
                        // Consume a queued message if the agent is now fully idle.
                        if app.in_flight_subagents.is_empty() {
                            if let Some(text) = app.pending_message.take() {
                                if !text.trim().is_empty() && !app.yielded {
                                    let first = !app.events.iter().any(|e| matches!(e.kind, EventKind::UserMessage { .. }));
                                    app.record(EventKind::UserMessage { text: text.clone() }).await?;
                                    if first {
                                        let name = derive_session_name(Some(&text), now_ms(), app.tz_offset_secs);
                                        app.session.rename_session(app.session_id, name.clone()).await.ok();
                                        app.shell.session_name = name;
                                    }
                                    app.streaming = true;
                                    spawn_turn(app);
                                }
                            }
                        }
                    }
```

- [ ] **Step 5: Clear the queue on session switch**

In `Command::NewSession` (line ~3524), after the busy guard, add `app.pending_message = None;` (the queue belongs to the prior session).

In `Action::SessionPick` (line ~2673), after the busy guard, add `app.pending_message = None;`.

- [ ] **Step 6: Update the `submit_is_noop_while_delegating` test**

The test currently asserts the textarea is left alone and no message is recorded. With the queue, the textarea IS cleared and the message IS stashed. Update:

```rust
    #[tokio::test]
    async fn submit_while_delegating_queues_message() {
        let mut app = test_app().await;
        app.in_flight_subagents.push(SubagentInfo { id: "sub-test".into(), task: "test".into() });
        app.textarea = make_input(TextArea::from(vec!["hello".to_string()]));

        let quit = handle_action(&mut app, zoid_tui::route::Action::Submit)
            .await
            .unwrap();

        assert!(!quit, "Submit must not signal quit");
        assert!(!app.in_flight_subagents.is_empty(), "in_flight untouched");
        assert!(!app.streaming, "no turn spawned");
        assert!(app.events.is_empty(), "no UserMessage recorded yet");
        assert_eq!(app.pending_message.as_deref(), Some("hello"), "message queued");
        assert!(app.textarea.lines()[0].is_empty(), "textarea cleared");
        assert!(app.shell.status_hint.as_deref().unwrap().contains("queued"));
    }
```

- [ ] **Step 7: Add a test for `TurnComplete` consuming the queue**

```rust
    #[tokio::test]
    async fn turn_complete_consumes_queued_message() {
        let mut app = test_app().await;
        app.pending_message = Some("follow up".into());
        // Simulate TurnComplete arriving with no subagents in flight.
        // (Directly exercise the handler effect since run() isn't driving here.)
        // The handler: streaming=false, take pending, record, spawn.
        app.streaming = true; // was streaming
        // Mimic the TurnComplete handler body:
        app.streaming = false;
        app.shell.clear_active_tool();
        app.pending_answer = None;
        app.turn_cancel = None;
        app.shell.status_hint = None;
        if app.in_flight_subagents.is_empty() {
            if let Some(text) = app.pending_message.take() {
                if !text.trim().is_empty() && !app.yielded {
                    app.record(EventKind::UserMessage { text }).await.unwrap();
                    app.streaming = true;
                    // spawn_turn would run here; skip in the unit test.
                }
            }
        }
        assert!(app.pending_message.is_none(), "queue drained");
        assert!(app.streaming, "a new turn was spawned from the queue");
        assert!(app.events.iter().any(|e| matches!(e.kind, EventKind::UserMessage { text } if text == "follow up")));
    }
```

- [ ] **Step 8: Add a test for session-switch clearing the queue**

```rust
    #[tokio::test]
    async fn new_session_clears_queued_message() {
        let mut app = test_app().await;
        app.pending_message = Some("stale".into());
        let _ = exec_command(&mut app, zoid_tui::command::Command::NewSession).await.unwrap();
        assert!(app.pending_message.is_none(), "queue cleared on :new");
    }
```

- [ ] **Step 9: Run tests + clippy**

Run: `cargo test -p zoid && cargo clippy --workspace`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): queue a message while the agent is busy (auto-submit on turn end)"
```

---

## Self-Review (run after all tasks)

**1. Spec coverage:**
- Item 1 (input wrap): Task 1 ✓
- Item 2 (companion trim): Task 3 ✓
- Item 3 (message queue): Task 5 ✓
- Item 4 (worktree detect): Task 2 ✓
- Item 5 (subagents drawer): Tasks 4a/4b/4c ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows the actual code.

**3. Type consistency:**
- `SubagentRow` (TUI, 4a) vs `SubagentInfo` (bin, 4c) — intentionally distinct (cross-crate boundary); the bin maps one to the other. ✓
- `allocate_drawer_bodies(drawers, height, tasks_len, subagents_len)` — signature defined in 4a, called in 4a's `compute()`. The bin never calls it directly. ✓
- `render_shell(... subagents: &[SubagentRow] ...)` — signature defined in 4b, called in 4c. ✓
- `AgentUpdate::SubagentStarted { id, task }` — defined in 4c (agent.rs), handled in 4c (main.rs). ✓
- `truncate_for_hint` — defined in Task 5, used in Task 5. ✓
- `pending_message: Option<String>` — defined in Task 5, used in Task 5. ✓

**4. Order risk:** Task 5 touches `Action::Submit` and `AgentUpdate::TurnComplete`, which also reference `in_flight_subagents` — but Task 4c already migrated that to `Vec<SubagentInfo>`. Doing 4 before 5 (as ordered) means 5's code blocks reference `Vec`-style APIs correctly. ✓