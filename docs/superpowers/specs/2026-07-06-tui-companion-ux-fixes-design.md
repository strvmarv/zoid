# TUI + Companion UX Fixes — Design

Date: 2026-07-06
Status: Draft (pending review)
Scope: five itemized TUI/companion changes — input wrapping, companion dashboard trim, message queueing, worktree detection, subagents drawer.

## 1. Message input wraps instead of horizontal-scrolling

### Problem
The message input box (`TextArea` from `ratatui_textarea`) defaults to `WrapMode::None`, so a long line horizontally scrolls instead of wrapping. The user wants it to wrap at the window edge.

### Approach
`ratatui_textarea` already exposes a `WrapMode` enum (`None`/`Word`/`Glyph`/`WordOrGlyph`) for soft-wrapping at render time. The fix is a one-liner in `make_input` (the bin's textarea factory in `crates/zoid/src/main.rs`):

```rust
fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
    let mut textarea = textarea;
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea.set_wrap_mode(ratatui_textarea::WrapMode::WordOrGlyph);
    textarea
}
```

`WordOrGlyph` wraps at word boundaries and falls back to grapheme wrapping for a word wider than the viewport — the best general behavior.

### Layout interaction
`app.shell.input_rows` is sampled each frame as `app.textarea.lines().len().max(1)` — the **logical** line count. The wrap mode is display-only; the buffer stays single-line, so `input_rows` is unaffected and `layout::input_height` (which grows the box up to `MAX_INPUT_ROWS = 8` then scrolls internally) still tracks logical lines. The textarea internally handles vertical scrolling of the wrapped display view. No layout changes needed.

### Files touched
- `crates/zoid/src/main.rs` — `make_input`: add `set_wrap_mode`.
- `crates/zoid/src/main.rs` — the `make_input_disables_cursor_line_underline` test: extend to assert `WrapMode::WordOrGlyph` is set (or add a companion test).

### Risk
None. Pure render-mode toggle; the existing cursor-motion/editing keymap is unchanged.

---

## 2. Companion becomes a card host (remove economy dashboard)

### Problem
The companion dashboard (`shell.html` + `app.js`) currently renders economy content — provider/model/session header, context-usage bar, tiers list, churn sparkline, and a `tasks: N · busy` line — above the agent's `show`-tool cards. The economy content grows with session length (tiers, churn) and forces the user to scroll past it to reach the actual artifact (the card). The user wants the economy dashboard **removed** and the card surface **kept** so the agent can push and manage custom cards.

### Current state
The companion hub (`crates/zoid-companion/src/hub.rs`) publishes two independent SSE event streams:
- `dashboard` — a `DashboardSnapshot` (economy projection: provider/model/session, ctx bar, tiers, churn, tasks_len, busy). Published per-frame by `main.rs` via `dashboard_snapshot()` + `companion_hub.publish_snapshot(snap)`.
- `card` — agent-authored HTML (the `show` tool). Published by `agent.rs::companion_show` via `hub.publish_card(html)`.

`shell.html` has two sections: `<section id="dashboard">` and `<section id="card">`. `app.js` has two EventSource listeners (`dashboard` renders the economy DOM; `card` renders the sandboxed iframe).

### Approach
Remove the economy dashboard entirely; keep the card surface. The companion becomes a card host, not a metrics dashboard.

**`crates/zoid-companion/src/shell.html`** — drop `<section id="dashboard">`; keep `<section id="card">`.

**`crates/zoid-companion/src/app.js`** — remove the `dashboard` EventSource listener, the `esc()`/tiers/bar rendering, and the `dash` element reference. Keep the `card` listener + the iframe sandbox machinery (RESIZE_REPORTER, cardDoc, frame sizing). The `esc()` helper is only used by the dashboard listener, so it can be removed too.

**`crates/zoid-companion/src/snapshot.rs`** — remove `DashboardSnapshot`, `TierRow`, and the custom `PartialEq` impl. The crate no longer needs a snapshot type at all.

**`crates/zoid-companion/src/hub.rs`** — remove `publish_snapshot` + the `snapshot` field from `Latest`/`Frame`. Keep `publish_card` + the `card` field. Simplify `Frame` to just `version` + `card`. The `current()`/`wait_after()` methods return only `version` + `card`. Update tests (`snap()` helper, `publish_bumps_version`, `identical_snapshot_does_not_bump`, `wait_after_returns_on_publish`).

**`crates/zoid-companion/src/server.rs`** — `SseReader::absorb` stops emitting `dashboard` frames; keep `card` frames. Remove `last_snapshot`/`last_snapshot` field. Update tests (`events_stream_flushes_first_frame_over_http`, `sse_reader_emits_dashboard_then_card_frames` → rename/trim to card-only, `DashboardSnapshot` construction in test helpers).

**`crates/zoid-companion/src/lib.rs`** — re-export only what remains (`CompanionHub`, `CompanionServer`, `start`); drop the `DashboardSnapshot`/`TierRow` re-exports.

**`crates/zoid/src/main.rs`** — remove `dashboard_snapshot()` and `heat_rank()`; remove the per-frame `app.companion_hub.publish_snapshot(snap)` block in `run()`. The `companion_hub` field stays (the `show` tool publishes cards through it). Remove `DashboardSnapshot` from any test helpers.

**`crates/zoid/src/agent.rs`** — unchanged. `companion_show` already publishes via `hub.publish_card(html)`.

**`crates/zoid-companion/examples/serve.rs`** — remove `publish_snapshot`; keep `publish_card`.

### Data flow after the change
- `main.rs` no longer publishes anything to the hub per-frame.
- `agent.rs::companion_show` (the `show` tool) is the only publisher, via `publish_card`.
- `SseReader::absorb` emits only `card` frames (+ heartbeats).
- The browser renders only the card iframe.

### Files touched
- `crates/zoid-companion/src/shell.html`
- `crates/zoid-companion/src/app.js`
- `crates/zoid-companion/src/snapshot.rs` (likely becomes empty or is deleted; `lib.rs` drops the `mod snapshot`)
- `crates/zoid-companion/src/hub.rs`
- `crates/zoid-companion/src/server.rs`
- `crates/zoid-companion/src/lib.rs`
- `crates/zoid-companion/examples/serve.rs`
- `crates/zoid/src/main.rs` — remove `dashboard_snapshot`, `heat_rank`, the publish block
- `crates/zoid/src/agent.rs` — no changes (verify `companion_show` still compiles against the trimmed hub)

### Risk
The `DashboardSnapshot` `PartialEq` dedupe logic goes away — card-only dedupe is simpler (just `Option<String>` equality on the `card` field, which the hub already does). The `churn`/`tiers`/`window` data still exists in-core for the TUI economy drawer and Overview; only the companion projection is removed.

---

## 3. Queue a message while the agent is working

### Problem
While a turn is in flight (`streaming || !in_flight_subagents.is_empty()`), `Action::Submit` is a hard no-op that surfaces a "finish the current turn first" hint and discards the typed text. The user wants to queue a message that auto-submits when the current turn ends, as an alternative to ESC-steering the current turn.

### Approach
Add a pending-message slot to `App`:
```rust
pending_message: Option<String>,
```
(defaults to `None` in the `App` constructor and `test_app()`.)

#### Routing change in `Action::Submit`
The current guard:
```rust
if app.streaming || !app.in_flight_subagents.is_empty() || app.yielded {
    if app.yielded { /* takeover hint */ }
    return Ok(false);
}
```
becomes:
```rust
// Yielded always blocks (even when not busy) — a taken-over session can't
// accept new turns until the user :new or :resume.
if app.yielded {
    app.shell.status_hint = Some("session taken over — :new or :resume".into());
    return Ok(false);
}
// Busy (streaming or delegating) but not yielded: stash the message for after.
if app.streaming || !app.in_flight_subagents.is_empty() {
    app.pending_message = Some(text.clone());
    app.textarea = make_input(TextArea::default());
    app.shell.status_hint = Some(format!("queued: {}", truncate_for_hint(&text)));
    return Ok(false);
}
```
Splitting `yielded` out of the busy guard is important: a yielded session with `streaming=false` and `in_flight_subagents` empty (e.g. after the turn was cancelled by takeover) must still block normal submit. The original combined guard handled this; the rewrite keeps `yielded` as its own early-return **before** the busy branch.
`truncate_for_hint` trims to ~40 chars with an ellipsis (mirrors `derive_session_name`'s truncation). This is a small helper in `main.rs`.

The `:delegate`/command interception (a single `:`-prefixed line) runs **before** the busy guard and is unaffected — commands are never queued.

#### Consumption on turn end
In the `AgentUpdate::TurnComplete` handler (which clears `streaming`, `pending_answer`, `turn_cancel`), after the existing cleanup, check the queue:
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
                app.record(EventKind::UserMessage { text: text.clone() }).await?;
                app.streaming = true;
                spawn_turn(app);
            } else {
                app.shell.status_hint = None; // empty/blank queued msg: drop silently
            }
        }
    }
}
```
The `in_flight_subagents.is_empty()` guard ensures the queued message waits for **both** streaming and any delegation to finish. `take()` on `Option` is naturally idempotent — a double `TurnComplete` can't double-fire.

#### ESC behavior
`Action::CancelTurn` fires the cancellation token (steers the current turn) and does **not** touch `pending_message` — the queued message stays to run after the current turn ends. The user can clear the queue by submitting an empty replacement (the busy-guard stashes an empty string, which the consumption block drops) or we can add an explicit clear later. Per the user's decision: ESC leaves the queue.

#### Edge cases
- **Queued message + new delegation spawned** (`:delegate` / verb pick while a message is queued): the delegation consumes the busy slot; `pending_message` stays. The `TurnComplete` consumption guard (`in_flight_subagents.is_empty()`) holds the message until the delegation also finishes.
  - But `start_delegation` itself checks `app.streaming || !app.in_flight_subagents.is_empty()` and blocks with "busy · one subagent at a time". If a message is queued (busy just cleared) and the user triggers a delegation, the delegation starts, `pending_message` waits. Correct.
- **Session switch / `:new` / `:resume` while a message is queued**: clear `pending_message` (the queue belongs to the prior session's flow). Add `app.pending_message = None` to `Command::NewSession` and `Action::SessionPick`.
- **`yielded`**: never queue. The takeover hint stands.
- **Empty/blank queued text**: the busy-guard still stashes it (the textarea was non-empty when the user hit submit, but defensive), and the consumption block drops it silently.

#### Visibility
The status hint `queued: {truncated text}` is visible in the status bar. It is cleared on consumption (the consumption block spawns a turn, and `spawn_turn` / the next `TurnComplete` resets `status_hint`). If the user wants to see the full queued text, they can look at the status bar; a future enhancement could echo it muted in the input box, but the status hint suffices for v1.

### Files touched
- `crates/zoid/src/main.rs` — `App` struct (add `pending_message`), constructor + `test_app()`, `Action::Submit` handler, `AgentUpdate::TurnComplete` handler, `Command::NewSession` + `Action::SessionPick` (clear queue), `truncate_for_hint` helper.
- Tests: `submit_is_noop_while_delegating` → update to assert the message is queued, not discarded; add a test for `TurnComplete` consuming the queue; add a test for session-switch clearing the queue.

### Risk
The `TurnComplete` → `spawn_turn` re-entry path runs without a keystroke. Must ensure it doesn't double-fire if `TurnComplete` arrives twice — `Option::take()` handles this. The `yielded` guard on consumption is belt-and-suspenders (a yielded session shouldn't reach consumption because `TurnComplete` from the cancelled turn clears `streaming` but `yielded` is true).

---

## 4. Fix the worktree portion of the repo widget

### Problem
`ShellState::worktree` is initialized to `"(none)"` in `ShellState::new()` and **never assigned anywhere** in the bin. So the repo drawer always shows "worktree (none)" regardless of whether zoid is running inside a git worktree.

### Root cause
At startup (`main.rs`, the `repo_present` block), the bin sets `shell.branch`, `shell.repo_name`, and `shell.changes_*`, but never sets `shell.worktree`. The field is dead state.

### Approach
Populate `shell.worktree` at startup, in the `repo_present` branch of the startup block, via a `worktree_label` helper using `git2` (already a workspace dependency, already used by `worktree.rs`):

```rust
/// The worktree label for the repo drawer: the linked-worktree name when the
/// process cwd is a linked worktree (not the main working copy), else "(none)".
fn worktree_label(repo: &git2::Repository) -> String {
    // A linked worktree's gitdir is a `.git` file pointing elsewhere; the main
    // working copy has `path() == commondir()`. git stores linked worktrees
    // under <common>/worktrees/<name>, so the worktree's gitdir basename IS
    // the worktree name.
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

The startup block opens the repo once (it already calls `in_git_repo()` which shells out to `git rev-parse`; for the worktree label we open via `git2::Repository::open_from_env()` or `Repository::open(".")` which is already what `worktree.rs` does). On any error, fall back to `"(none)"`:

```rust
if repo_present {
    shell.branch = current_branch();
    shell.repo_name = ...;
    shell.worktree = git2::Repository::open(".")
        .ok()
        .map(|r| worktree_label(&r))
        .unwrap_or_else(|| "(none)".into());
    let (boot_added, boot_removed, boot_files) = git_status();
    ...
}
```

This runs once at startup (cheap — no per-second polling; worktree membership doesn't change mid-session).

### Alternative considered
Shell out to `git rev-parse --git-common-dir` / `--git-dir` and compare the two paths. Rejected: `git2` is already a dependency, `Repository::open` is already used in the same crate, and the git2 path avoids a subprocess at startup.

### Files touched
- `crates/zoid/src/main.rs` — add `worktree_label`, set `shell.worktree` in the `repo_present` block.
- Tests: add a unit test for `worktree_label` against a temp repo with and without a linked worktree (using `worktree::create_worktree` from the test helpers).

### Risk
Low. `git2::Repository::open(".")` is already used in `worktree.rs` and handles the "not a repo" case via `ok()`. The `path() == commondir()` comparison is the documented git2 idiom for detecting linked worktrees.

---

## 5. Subagents drawer in the right rail

### Problem
There is no visible indicator for in-flight subagents in the TUI beyond a transient status hint ("1 subagent running…"). The user wants a persistent drawer below Tasks showing active subagents, removed when they complete.

### Current state — important subtlety
There are **two** subagent dispatch paths, and only one is tracked in `main.rs`:
1. **`start_delegation`** (`:delegate` command / verb pick, `main.rs`) — inserts `sub_id` into `app.in_flight_subagents: HashSet<String>`. The `DelegationResult` handler removes it.
2. **`dispatch_subagent` tool** (`agent.rs`, the model calling the tool) — spawns via `spawn_subagent::spawn_subagent` but does **not** insert into `in_flight_subagents`. The `DelegationResult` handler's `remove(subagent_id)` is a no-op for these (the id was never inserted).

So `app.shell.busy` (`streaming || !in_flight_subagents.is_empty()`) only reflects the `:delegate` path, not the `dispatch_subagent` tool path. The widget must account for this.

### Approach

#### 5a. Unify subagent tracking
Introduce a new `AgentUpdate` variant so the `dispatch_subagent` tool path can notify `main.rs` of a spawned subagent:

```rust
// agent.rs
pub enum AgentUpdate {
    ...
    /// A subagent was dispatched (via the dispatch_subagent tool). The UI tracks
    /// it as in-flight until its DelegationResult arrives.
    SubagentStarted { id: String, task: String },
    ...
}
```

In `agent.rs`, the `dispatch_subagent` tool handler emits `AgentUpdate::SubagentStarted { id: sub_id.clone(), task: task.clone() }` **before** calling `spawn_subagent::spawn_subagent`.

`main.rs` handles it:
```rust
AgentUpdate::SubagentStarted { id, task } => {
    app.in_flight_subagents.push(SubagentInfo { id, task });
    app.shell.status_hint = Some(format!("{} {} subagent running…", glyph::RUNNING, app.in_flight_subagents.len()));
}
```

#### 5b. Enrich the in-flight data structure
Change `in_flight_subagents: HashSet<String>` → `Vec<SubagentInfo>`:
```rust
struct SubagentInfo {
    id: String,
    task: String,
}
```
A `Vec` preserves insertion order for stable rendering. Update all existing sites:
- `is_empty()` → `Vec::is_empty()` (unchanged semantics).
- `insert(sub_id)` (in `start_delegation`) → `push(SubagentInfo { id: sub_id, task: task.clone() })`.
- `remove(subagent_id)` (in the `DelegationResult` handler) → `retain(|s| s.id != *subagent_id)`.
- `len()` (status hint) → `len()` (unchanged).
- `clear()` (in `SessionTakenOver`) → `clear()` (unchanged).

The test helpers that do `in_flight_subagents.insert("sub-test".into())` → `push(SubagentInfo { id: "sub-test".into(), task: "test".into() })`.

#### 5c. Add the drawer
**`DrawerId::Subagents`** — a new variant in `crates/zoid-tui/src/state.rs`, placed last in the enum (after `Tasks`).

**`ShellState::new()`** — push a new `Drawer { id: DrawerId::Subagents, title: "subagents".into(), open: true }` after the Tasks drawer. Open by default — it's live activity the user wants to see, mirroring Tasks.

**`drawer_body_rows`** (`layout.rs`) — add `DrawerId::Subagents => SUBAGENTS_BODY_ROWS` (a new constant, default 5, mirroring `TASKS_BODY_ROWS`).

**`drawer_fit_priority`** (`layout.rs`) — add `DrawerId::Subagents => 4` (lowest priority — yields first when the rail is short, after Repo/Context/Session/Tasks). Subagents are transient; the persistent drawers keep their rows.

**`allocate_drawer_bodies`** — extend the content-driven grow logic. Currently `task_count` drives the Tasks drawer's pass-2 surplus growth. Add a `subagent_count` parameter (parallel to `task_count`) and a pass-3 that grows Subagents toward `subagent_count`. Alternatively, generalize: pass a small map of `DrawerId → content_count` and loop. The simplest change: add a `subagent_count: u16` parameter and a pass-3 block mirroring the Tasks pass-2.

**`ShellState::subagents_len`** — a new `u16` field (default 0), sampled by the bin each frame from `app.in_flight_subagents.len()`, so `allocate_drawer_bodies` can grow the drawer. Mirrors `tasks_len`.

**`render_subagents_body`** (`render.rs`) — a new fn mirroring `render_tasks_body`:
- Empty (`in_flight_subagents` empty) → dim "no subagents" line.
- Non-empty → one row per in-flight subagent: a running glyph (`glyph::RUNNING`, `color::WARN`) + truncated id + truncated task label.
- Row format: `{RUNNING} {id}  {task}` truncated to the drawer width.
- Capped to the body rows the allocator gave the drawer.

The row data (`Vec<SubagentInfo>`) needs to reach the pure renderer. Two options:
1. Pass `&[SubagentInfo]` to `render_shell` (adds a parameter — `render_shell` already takes `tasks: &[TaskItem]`).
2. Store a rendered `Vec<(String, String)>` (id, task) on `ShellState` each frame (mirrors how `tasks_len` is a hint, but the full list is passed separately).

Option 1 is cleaner and mirrors the `tasks` parameter. `render_shell` gains a `subagents: &[SubagentInfo]` parameter. But `SubagentInfo` lives in `main.rs` (the bin), and `render_shell` is in `zoid-tui`. To avoid a cross-crate type dependency, define a small `SubagentRow { id: String, task: String }` in `zoid-tui::state` (or `zoid_core::tasks`), and the bin maps `in_flight_subagents` → `Vec<SubagentRow>` each frame before calling `render_shell`.

**`render_rail`** — add `DrawerId::Subagents => render_subagents_body(frame, body_rect, subagents)` to the match.

#### 5d. Layout computation
`compute()` in `layout.rs` calls `allocate_drawer_bodies(&state.drawers, inner.height, state.tasks_len)`. Extend to `allocate_drawer_bodies(&state.drawers, inner.height, state.tasks_len, state.subagents_len)`. The pass-3 grows Subagents toward `subagents_len` using leftover rows after Tasks' pass-2.

### Files touched
- `crates/zoid-tui/src/state.rs` — `DrawerId::Subagents`, `subagents_len` field, `SubagentRow` type, `ShellState::new()` drawer push.
- `crates/zoid-tui/src/layout.rs` — `SUBAGENTS_BODY_ROWS`, `drawer_body_rows` match arm, `drawer_fit_priority` match arm, `allocate_drawer_bodies` signature + pass-3, `compute()` call site.
- `crates/zoid-tui/src/render.rs` — `render_subagents_body`, `render_rail` match arm, `render_shell` signature (add `subagents` param).
- `crates/zoid/src/main.rs` — `SubagentInfo` struct, `in_flight_subagents` type change, `SubagentStarted` handler, `start_delegation` push, `DelegationResult` retain, `subagents_len` sampling, `render_shell` call site, test helpers.
- `crates/zoid/src/agent.rs` — `AgentUpdate::SubagentStarted` variant, `dispatch_subagent` handler emits it.
- `crates/zoid/src/spawn_subagent.rs` — no change (it already sends `Appended(DelegationResult)` on completion).
- Tests: `zoid-tui` layout/render tests for the new drawer; `main.rs` tests for the `Vec` migration; `agent.rs` test that `dispatch_subagent` emits `SubagentStarted`.

### Risk
The `in_flight_subagents` type change (`HashSet` → `Vec`) touches ~10 sites. The `SubagentStarted` variant adds a new `AgentUpdate` arm that must be handled in every `match AgentUpdate` exhaustively (only `main.rs::run()`). The `render_shell` signature change touches every caller (the bin + snapshot tests). The `SubagentRow` cross-crate type is small and avoids coupling the TUI to the bin's `SubagentInfo`.

---

## Cross-cutting notes

### Test strategy
Each item has unit-testable seams:
1. Input wrap: `make_input` test asserts `WrapMode::WordOrGlyph`.
2. Companion: hub/server tests assert no `dashboard` frame, only `card`.
3. Queue: `Submit`-while-busy stashes; `TurnComplete` consumes; session-switch clears.
4. Worktree: `worktree_label` against a temp repo + linked worktree.
5. Subagents: layout allocation with `subagents_len`; render body empty/non-empty; `SubagentStarted` handler.

### Order
Items are independent and can be implemented in any order. Suggested: 1 (trivial, build-confidence) → 4 (isolated) → 2 (companion-only) → 5 (most cross-crate) → 3 (touches the busy guard that 5 also touches, so doing 5 first stabilizes the `in_flight_subagents` shape).

### Out of scope
- A full custom input widget (item 1 is just a wrap-mode toggle).
- Removing the `show` tool or the companion server (item 2 keeps the card host).
- An explicit "clear queue" action (ESC leaves the queue per the user's decision; a future enhancement).
- Worktree membership polling (item 4 is startup-only; worktree membership doesn't change mid-session).
- Subagent progress/status within the drawer (id + task label only; a future enhancement could show elapsed time or completion status).