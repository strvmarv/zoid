# zoid Sessions & Application Database — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's single implicit in-repo event log into a user-global, multi-session application database — every event tagged with a `session_id`, a `sessions` table + `SessionList()` projection, XDG DB relocation with a one-time legacy import, session lifecycle (auto-resume-last-for-repo / new / rename) in the bin, palette + command-line session verbs, and a rail restructured to **repo / session / context ⑤** (§2.1).

**Architecture:** The event-sourced core (`zoid-core`) stays a pure fold over `Vec<Event>` plus a single-writer SQLite actor (`SessionHandle`); this plan adds a `session_id` column + a `sessions` table + pure `session_list()` projection, then threads an *active session* through the bin so append/replay partition by `session_id`. The `zoid-tui` crate stays terminal-free and pure — the bin computes view-model strings (repo diff stats, session name/model/duration/cwd) and hands them to the renderer, exactly as it already does for `branch`/`files`.

**Tech Stack:** Rust 2021; `rusqlite` (bundled SQLite), `ulid`, `serde`/`serde_json`, `ratatui` 0.29, `tokio`, `chrono` (bin-only, for wall-clock timestamps + local offset), `insta` snapshots, `proptest`. No new heavy dependencies — XDG path resolution uses `std::env` + `PathBuf`; git diff stats shell out to the `git` binary.

**Prerequisites:** Plan 1 (Chat Polish) is assumed merged — it already removed drawer keybind labels and consolidated chrome (branch → rail only). This is Plan 2 of 3; Plan 3 (P5 delegation) consumes the `session_id` this plan adds.

> **Note on Plan-1 drift.** The *current* tree still shows branch in the top bar (`render.rs:88`) and status bar (`render.rs:119`), and still renders a `Drawer.keybind` field (`render.rs:150`, `state.rs:113–115`). If Plan 1 is genuinely merged before you start, those are already gone and the relevant sub-steps are no-ops — verify with `grep -n "keybind" crates/zoid-tui/src` before editing. This plan is written to reach the correct end state either way: Task 11 drops the `keybind` field and Task 14 removes any remaining branch-in-chrome.

## Global Constraints
- Rust edition 2021; workspace crates `zoid-core`, `zoid-provider`, `zoid-tui`, `zoid-tools`, `zoid-syntax`, `zoid` bin. Size-optimized release profile — keep new deps minimal (prefer `std` for XDG path resolution; do NOT add a `dirs`/`directories` crate).
- §16 Design tokens: NO literal glyphs/hex outside `crates/zoid-tui/src/tokens.rs`. The repo changes line (green `+added` / red `-removed`) and any new rail glyphs use tokens (add them there).
- TDD default: failing test first, minimal code, green, commit.
- Every new/changed TUI screen ships an `insta` snapshot (`terminal.backend().to_string()` via `TestBackend`, matching the existing `shell_snapshot.rs` style) in `crates/zoid-tui/tests/snapshots/`, built to match `docs/ux/chat-mode.html` (repo & session rail widgets) and `docs/ux/palette.html` (session group). The first `cargo test` run writes the `.snap.new`; accept it with `cargo insta accept` (or rename `.snap.new` → `.snap`) after eyeballing.
- Core is CLOCK-FREE: pure core takes time as data (`ts: i64` injected); `chrono` is bin-only. Session `created_ts`/`last_touched_ts` are injected from the bin, never read from an ambient clock in core.
- Commit messages END with `Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY`. NEVER add a co-author trailer.
- `Ulid::from(0u128)` is the canonical "no session / default" sentinel (proven to compile across the codebase — see `store.rs` tests). Use it wherever a placeholder `session_id` is needed.

---

## File Structure

**Create:**
- `crates/zoid-core/src/sessions.rs` — `SessionRow` (raw table row), `SessionInfo` (folded), pure `session_list(rows, events, root_filter)` projection.
- `crates/zoid-core/tests/sessions_db.rs` — integration test: append events under two `session_id`s, list/rename/touch through `EventStore`, assert isolation + ordering.
- `crates/zoid-tui/tests/session_snapshot.rs` — snapshots for the repo/session rail widgets and the resume-session overlay.

**Modify:**
- `crates/zoid-core/src/event.rs` — add `session_id: Ulid` to `Event`; `with_session` builder; serde/round-trip test.
- `crates/zoid-core/src/store.rs` — `events.session_id` column + index; `sessions` table + CRUD; `load_session`; `list_session_rows`.
- `crates/zoid-core/src/session.rs` — actor `Cmd` variants + `SessionHandle` methods: `snapshot_session`, `new_session`, `rename_session`, `touch_session`, `list_sessions`.
- `crates/zoid-core/src/lib.rs` — `pub mod sessions;`.
- `crates/zoid-core/tests/round_trip.rs` — event literals gain `session_id`.
- `crates/zoid/src/main.rs` — XDG `db_path`, legacy import, session lifecycle, repo/session view-model, `git_status`, name/duration helpers; execute `:new`/`:rename`/resume.
- `crates/zoid/src/agent.rs` — thread `session_id` into `run_agent_turn`/`emit`.
- `crates/zoid-tui/src/state.rs` — `DrawerId` → `Repo`/`Session`/`Context`; drop `Drawer.keybind`; `Overlay::Sessions`; repo/session view-model fields; session-picker state.
- `crates/zoid-tui/src/layout.rs` — per-drawer body rows for the new ids; overlay rect for `Sessions`.
- `crates/zoid-tui/src/command.rs` — `Command::{NewSession, RenameSession(String), ResumeSessionPicker}`; parse `:new`/`:rename`; retarget `:repo`/`:session`/`:context`.
- `crates/zoid-tui/src/palette.rs` — session group; navigate group retargeted to the new drawers.
- `crates/zoid-tui/src/render.rs` — repo/session drawer bodies; context body (was economy); resume overlay; remove branch-in-chrome + keybind rendering.
- `crates/zoid-tui/src/route.rs` — route keys for `Overlay::Sessions`; test call-sites retargeted off `DrawerId::Files`.
- `crates/zoid-tui/src/tokens.rs` — `color::ADDED`/`color::REMOVED` aliases for the changes line.
- `crates/zoid-tui/examples/preview.rs`, `crates/zoid-tui/tests/shell_snapshot.rs` — retarget `DrawerId::Files` references; drop `s.files`.

---

## Task 1: `session_id` on the `Event` model
**Files:** Modify `crates/zoid-core/src/event.rs` (struct ~47–61; tests ~67–136) / Test in-file.
**Interfaces:** Consumes `Ulid`. Produces `Event { …, session_id: Ulid }` and `Event::with_session(self, Ulid) -> Event`. `Event::new` signature is **unchanged** (defaults `session_id` to `Ulid::from(0u128)`), so the ~30 existing `Event::new(...)` call sites keep compiling.

- [ ] **Step 1: Write the failing test.** Add to `event.rs` `mod tests`:
```rust
#[test]
fn session_id_round_trips_and_defaults() {
    let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    // new() defaults to the zero sentinel …
    let ev = Event::new(id, None, 1, EventKind::UserMessage { text: "hi".into() });
    assert_eq!(ev.session_id, Ulid::from(0u128));
    // … and the builder sets it, surviving a JSON round-trip.
    let sid = Ulid::from(7u128);
    let ev = ev.with_session(sid);
    let back: Event = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
    assert_eq!(back.session_id, sid);
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-core session_id_round_trips_and_defaults` → fails to compile (`no field session_id`, `no method with_session`).
- [ ] **Step 3: Write minimal implementation.** In `event.rs`:
  - Add the field to `Event` (after `branch`):
```rust
pub struct Event {
    pub id: Ulid,
    pub parent: Option<Ulid>,
    pub branch: BranchId,
    pub session_id: Ulid,
    pub ts: i64,
    pub kind: EventKind,
    pub tokens: Option<TokenStat>,
}
```
  - In `impl Event`, keep `new` defaulting the field and add the builder:
```rust
pub fn new(id: Ulid, parent: Option<Ulid>, ts: i64, kind: EventKind) -> Self {
    Event { id, parent, branch: BranchId::default(), session_id: Ulid::from(0u128), ts, kind, tokens: None }
}

/// Tag this event with its owning session (bin wiring; core stays session-agnostic).
pub fn with_session(mut self, session_id: Ulid) -> Self {
    self.session_id = session_id;
    self
}
```
  - Update the three struct-literal `Event { … }` constructions in this file's tests to add `session_id: Ulid::from(0u128),` after `branch: …`: `event_json_round_trips` (~71), `usage_and_mutation_round_trip` (~124). (The `Event::new(...)` calls need no change.)
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-core --lib event::` → all `event` tests green (existing + new).
- [ ] **Step 5: Commit.** `git add crates/zoid-core/src/event.rs && git commit -m "feat(core): add session_id to Event + with_session builder

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 2: `events.session_id` column, index, and session-scoped load
**Files:** Modify `crates/zoid-core/src/store.rs` (open ~12–25, append ~27–41, load_all ~43–73; tests ~76–94); Modify `crates/zoid-core/tests/round_trip.rs`.
**Interfaces:** Consumes `Event.session_id`. Produces `EventStore::load_session(&self, session_id: Ulid) -> Result<Vec<Event>>`; `append`/`load_all` now persist/read `session_id`.

- [ ] **Step 1: Write the failing test.** Add to `store.rs` `mod tests`:
```rust
#[test]
fn append_persists_session_id_and_load_session_filters() {
    let store = EventStore::open(":memory:").unwrap();
    let sa = Ulid::from(10u128);
    let sb = Ulid::from(20u128);
    let a = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "a".into() }).with_session(sa);
    let b = Event::new(Ulid::from(2u128), None, 2, EventKind::UserMessage { text: "b".into() }).with_session(sb);
    store.append(&a).unwrap();
    store.append(&b).unwrap();
    // load_all keeps every event, with session_id intact …
    assert_eq!(store.load_all().unwrap(), vec![a.clone(), b.clone()]);
    // … load_session partitions the log.
    assert_eq!(store.load_session(sa).unwrap(), vec![a]);
    assert_eq!(store.load_session(sb).unwrap(), vec![b]);
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-core append_persists_session_id_and_load_session_filters` → compile error (`no method load_session`) / column-count mismatch.
- [ ] **Step 3: Write minimal implementation.** In `store.rs`:
  - `open` — extend the `CREATE TABLE events` batch and add an index (fresh DB; the new global DB starts clean, so no `ALTER` is needed — legacy data is imported in Task 7):
```rust
conn.execute_batch(
    "CREATE TABLE IF NOT EXISTS events (
        id         TEXT PRIMARY KEY,
        parent     TEXT,
        branch     TEXT NOT NULL,
        session_id TEXT NOT NULL,
        ts         INTEGER NOT NULL,
        kind       TEXT NOT NULL,
        tokens     TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);",
)?;
```
  - `append` — add the column:
```rust
self.conn.execute(
    "INSERT INTO events (id, parent, branch, session_id, ts, kind, tokens)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    params![
        event.id.to_string(),
        event.parent.map(|p| p.to_string()),
        event.branch.0,
        event.session_id.to_string(),
        event.ts,
        serde_json::to_string(&event.kind)?,
        event.tokens.map(|t| serde_json::to_string(&t)).transpose()?,
    ],
)?;
```
  - Add a private row-decoder to keep `load_all`/`load_session` DRY, and both public methods:
```rust
const SELECT_COLS: &str =
    "SELECT id, parent, branch, session_id, ts, kind, tokens FROM events";

fn decode_rows(stmt: &mut rusqlite::Statement, params: impl rusqlite::Params) -> Result<Vec<Event>> {
    let raw = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in raw {
        let (id, parent, branch, session_id, ts, kind, tokens) = r?;
        out.push(Event {
            id: id.parse()?,
            parent: parent.map(|p| p.parse()).transpose()?,
            branch: BranchId(branch),
            session_id: session_id.parse()?,
            ts,
            kind: serde_json::from_str(&kind)?,
            tokens: tokens.map(|t| serde_json::from_str(&t)).transpose()?,
        });
    }
    Ok(out)
}

pub fn load_all(&self) -> Result<Vec<Event>> {
    let mut stmt = self.conn.prepare(&format!("{SELECT_COLS} ORDER BY id ASC"))?;
    Self::decode_rows(&mut stmt, [])
}

pub fn load_session(&self, session_id: Ulid) -> Result<Vec<Event>> {
    let mut stmt = self.conn.prepare(&format!("{SELECT_COLS} WHERE session_id = ?1 ORDER BY id ASC"))?;
    Self::decode_rows(&mut stmt, params![session_id.to_string()])
}
```
  - Add `use ulid::Ulid;` at the top of `store.rs` (currently only imported in the test module).
  - The existing `append_then_load_round_trips_in_order` test uses `Event::new` (session_id defaults to `0`) and stays valid.
  - In `crates/zoid-core/tests/round_trip.rs`, the two `Event::new(...)` calls need no change (they default `session_id` to `0`, which now round-trips through the column). Run it to confirm.
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-core store::` and `cargo test -p zoid-core --test round_trip` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid-core/src/store.rs crates/zoid-core/tests/round_trip.rs && git commit -m "feat(core): persist session_id column + load_session filter

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 3: `sessions` table + store CRUD
**Files:** Modify `crates/zoid-core/src/store.rs` (open batch; new methods); Modify `crates/zoid-core/src/sessions.rs` (create in Task 4 — for now define `SessionRow` here at the top of Task 3 in `sessions.rs`). **Do Task 4's `sessions.rs` scaffolding first if you prefer**, or define `SessionRow` inline. This task defines `SessionRow` in `sessions.rs`, then store methods that produce it.
**Interfaces:** Produces `EventStore::{insert_session, rename_session, touch_session, list_session_rows}`; consumes `SessionRow`.

- [ ] **Step 1: Write the failing test.** Add to `store.rs` `mod tests`:
```rust
#[test]
fn sessions_crud_round_trips() {
    use crate::sessions::SessionRow;
    let store = EventStore::open(":memory:").unwrap();
    let id = Ulid::from(1u128);
    store.insert_session(id, "first", "/repo/a", 100, 100).unwrap();
    store.touch_session(id, 200).unwrap();
    store.rename_session(id, "renamed").unwrap();
    let rows = store.list_session_rows().unwrap();
    assert_eq!(rows, vec![SessionRow {
        id, name: "renamed".into(), root_path: "/repo/a".into(),
        created_ts: 100, last_touched_ts: 200,
    }]);
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-core sessions_crud_round_trips` → compile error (unresolved `crate::sessions`, missing methods).
- [ ] **Step 3: Write minimal implementation.**
  - Create `crates/zoid-core/src/sessions.rs` with just the raw row for now, and register the module:
```rust
//! Sessions: raw `sessions`-table rows (`SessionRow`) and the folded
//! `SessionInfo` projection (see `session_list`). Pure; the store owns SQL.

use crate::event::Event;
use ulid::Ulid;

/// One row of the `sessions` table, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
}
```
  - In `crates/zoid-core/src/lib.rs`, add `pub mod sessions;` (alphabetical, after `projection`).
  - In `store.rs` `open`, append the `sessions` table to the `execute_batch` string:
```sql
CREATE TABLE IF NOT EXISTS sessions (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    root_path       TEXT NOT NULL,
    created_ts      INTEGER NOT NULL,
    last_touched_ts INTEGER NOT NULL
);
```
  - Add the CRUD methods to `impl EventStore` (add `use crate::sessions::SessionRow;` at top):
```rust
pub fn insert_session(&self, id: Ulid, name: &str, root_path: &str, created_ts: i64, last_touched_ts: i64) -> Result<()> {
    self.conn.execute(
        "INSERT INTO sessions (id, name, root_path, created_ts, last_touched_ts)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.to_string(), name, root_path, created_ts, last_touched_ts],
    )?;
    Ok(())
}

pub fn rename_session(&self, id: Ulid, name: &str) -> Result<()> {
    self.conn.execute("UPDATE sessions SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
    Ok(())
}

pub fn touch_session(&self, id: Ulid, last_touched_ts: i64) -> Result<()> {
    self.conn.execute("UPDATE sessions SET last_touched_ts = ?2 WHERE id = ?1",
        params![id.to_string(), last_touched_ts])?;
    Ok(())
}

pub fn list_session_rows(&self) -> Result<Vec<SessionRow>> {
    let mut stmt = self.conn.prepare(
        "SELECT id, name, root_path, created_ts, last_touched_ts FROM sessions ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id, name, root_path, created_ts, last_touched_ts) = r?;
        out.push(SessionRow { id: id.parse()?, name, root_path, created_ts, last_touched_ts });
    }
    Ok(out)
}
```
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-core sessions_crud_round_trips` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid-core/src/store.rs crates/zoid-core/src/sessions.rs crates/zoid-core/src/lib.rs && git commit -m "feat(core): sessions table + insert/rename/touch/list CRUD

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 4: `SessionInfo` + pure `session_list()` projection
**Files:** Modify `crates/zoid-core/src/sessions.rs` (add `SessionInfo` + `session_list` + tests).
**Interfaces:** Consumes `&[SessionRow]`, `&[Event]`, `Option<&str>` (root filter). Produces `Vec<SessionInfo { id: Ulid, name: String, root_path: String, created_ts: i64, last_touched_ts: i64, token_total: u64 }>` — most-recent-first by `last_touched_ts`.

- [ ] **Step 1: Write the failing test.** Add to `sessions.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, TokenStat};

    fn row(id: u128, name: &str, root: &str, touched: i64) -> SessionRow {
        SessionRow { id: Ulid::from(id), name: name.into(), root_path: root.into(),
            created_ts: 0, last_touched_ts: touched }
    }
    fn usage(session: u128, input: u64, output: u64) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::Usage)
            .with_session(Ulid::from(session))
            .with_tokens(TokenStat { input, output, cached: 0 })
    }

    #[test]
    fn orders_recent_first_sums_tokens_and_filters_repo() {
        let rows = vec![
            row(1, "old", "/repo/a", 100),
            row(2, "new", "/repo/a", 300),
            row(3, "other", "/repo/b", 200),
        ];
        let events = vec![usage(1, 10, 5), usage(2, 100, 0), usage(2, 0, 40)];
        // No filter: most-recent-first across all repos, token totals folded.
        let all = session_list(&rows, &events, None);
        assert_eq!(all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["new", "other", "old"]);
        assert_eq!(all[0].token_total, 140); // session 2: 100 + 40
        assert_eq!(all[2].token_total, 15);  // session 1: 10 + 5
        // Filtered to /repo/a: drops "other".
        let a = session_list(&rows, &events, Some("/repo/a"));
        assert_eq!(a.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
    }
}
```
  > This test also calls `Event::with_tokens` — add that trivially in Task 4 Step 3 (it keeps the `usage` helper readable and mirrors `with_session`).
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-core session_list` → compile error (`session_list`, `SessionInfo`, `with_tokens` missing).
- [ ] **Step 3: Write minimal implementation.**
  - In `event.rs` `impl Event`, add:
```rust
pub fn with_tokens(mut self, tokens: TokenStat) -> Self {
    self.tokens = Some(tokens);
    self
}
```
  - In `sessions.rs`:
```rust
use std::collections::HashMap;

/// A session folded for the resume picker / rail widget: the row plus a
/// token total summed from that session's events (`input + output`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
    pub token_total: u64,
}

/// Fold session rows into `SessionInfo`, most-recent-first by `last_touched_ts`
/// (ties broken by `id` desc for determinism). `token_total` sums each session's
/// events' `input + output`. When `root_filter` is `Some`, only sessions whose
/// `root_path` matches are returned. Pure.
pub fn session_list(rows: &[SessionRow], events: &[Event], root_filter: Option<&str>) -> Vec<SessionInfo> {
    let mut totals: HashMap<Ulid, u64> = HashMap::new();
    for e in events {
        if let Some(t) = e.tokens {
            *totals.entry(e.session_id).or_default() += t.input + t.output;
        }
    }
    let mut out: Vec<SessionInfo> = rows
        .iter()
        .filter(|r| root_filter.is_none_or(|f| r.root_path == f))
        .map(|r| SessionInfo {
            id: r.id,
            name: r.name.clone(),
            root_path: r.root_path.clone(),
            created_ts: r.created_ts,
            last_touched_ts: r.last_touched_ts,
            token_total: totals.get(&r.id).copied().unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| b.last_touched_ts.cmp(&a.last_touched_ts).then(b.id.cmp(&a.id)));
    out
}
```
  > `Option::is_none_or` is stable since Rust 1.82; if your toolchain is older, use `root_filter.map_or(true, |f| r.root_path == f)`.
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-core session_list` → green; `cargo test -p zoid-core --lib` still all green.
- [ ] **Step 5: Commit.** `git add crates/zoid-core/src/sessions.rs crates/zoid-core/src/event.rs && git commit -m "feat(core): session_list projection (recent-first, token totals, repo filter)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 5: `SessionHandle` actor — session-scoped snapshot + session CRUD
**Files:** Modify `crates/zoid-core/src/session.rs` (`Cmd` ~6–10, `spawn` loop ~30–43, methods ~47–65; tests ~68–95).
**Interfaces:** Produces `SessionHandle::{snapshot_session(Ulid), new_session(Ulid,String,String,i64), rename_session(Ulid,String), touch_session(Ulid,i64), list_sessions(Option<String>) -> Result<Vec<SessionInfo>>}`. Consumes `EventStore` methods from Tasks 2–4.

- [ ] **Step 1: Write the failing test.** Add to `session.rs` `mod tests`:
```rust
#[tokio::test]
async fn actor_partitions_sessions_and_lists_them() {
    let handle = SessionHandle::spawn(":memory:").unwrap();
    let sa = Ulid::from(1u128);
    handle.new_session(sa, "alpha".into(), "/repo".into(), 100).await.unwrap();
    handle.append(
        Event::new(Ulid::from(9u128), None, 5, EventKind::UserMessage { text: "hi".into() })
            .with_session(sa),
    ).await.unwrap();
    // session-scoped snapshot returns only sa's events
    let snap = handle.snapshot_session(sa).await.unwrap();
    assert_eq!(snap.len(), 1);
    // list surfaces the row
    let list = handle.list_sessions(Some("/repo".into())).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "alpha");
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-core actor_partitions_sessions_and_lists_them` → compile error (missing methods/Cmd variants).
- [ ] **Step 3: Write minimal implementation.** In `session.rs`:
  - Extend `Cmd` (add imports `use crate::sessions::SessionInfo; use ulid::Ulid;` at top — `Ulid` is currently only in tests):
```rust
enum Cmd {
    Append { event: Box<Event>, reply: oneshot::Sender<Result<()>> },
    Snapshot { reply: oneshot::Sender<Vec<Event>> },
    SnapshotSession { session_id: Ulid, reply: oneshot::Sender<Vec<Event>> },
    NewSession { id: Ulid, name: String, root_path: String, ts: i64, reply: oneshot::Sender<Result<()>> },
    RenameSession { id: Ulid, name: String, reply: oneshot::Sender<Result<()>> },
    TouchSession { id: Ulid, ts: i64, reply: oneshot::Sender<Result<()>> },
    ListSessions { root_filter: Option<String>, reply: oneshot::Sender<Result<Vec<SessionInfo>>> },
}
```
  - Handle them in the `spawn` match loop:
```rust
Cmd::SnapshotSession { session_id, reply } => {
    let _ = reply.send(store.load_session(session_id).unwrap_or_default());
}
Cmd::NewSession { id, name, root_path, ts, reply } => {
    let _ = reply.send(store.insert_session(id, &name, &root_path, ts, ts));
}
Cmd::RenameSession { id, name, reply } => {
    let _ = reply.send(store.rename_session(id, &name));
}
Cmd::TouchSession { id, ts, reply } => {
    let _ = reply.send(store.touch_session(id, ts));
}
Cmd::ListSessions { root_filter, reply } => {
    let out = (|| {
        let rows = store.list_session_rows()?;
        let events = store.load_all()?;
        anyhow::Ok(crate::sessions::session_list(&rows, &events, root_filter.as_deref()))
    })();
    let _ = reply.send(out);
}
```
  - Add matching `SessionHandle` methods (same `send + await` pattern as `append`/`snapshot`). Example for two of them; mirror for the rest:
```rust
pub async fn snapshot_session(&self, session_id: Ulid) -> Result<Vec<Event>> {
    let (reply, rx) = oneshot::channel();
    self.tx.send(Cmd::SnapshotSession { session_id, reply }).await
        .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
    rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))
}

pub async fn new_session(&self, id: Ulid, name: String, root_path: String, ts: i64) -> Result<()> {
    let (reply, rx) = oneshot::channel();
    self.tx.send(Cmd::NewSession { id, name, root_path, ts, reply }).await
        .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
    rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
}
```
  (Add `rename_session`, `touch_session`, `list_sessions` analogously; `list_sessions` returns `Result<Vec<SessionInfo>>`, so its final line is `rx.await.map_err(...)?`.)
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-core session::` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid-core/src/session.rs && git commit -m "feat(core): SessionHandle session CRUD + snapshot_session

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 6: XDG DB path resolution
**Files:** Modify `crates/zoid/src/main.rs` (`db_path` ~32–40; add `#[cfg(test)] mod tests`).
**Interfaces:** Produces `resolve_db_path(env: impl Fn(&str) -> Option<String>) -> PathBuf` (pure, injectable env) + `db_path() -> Result<PathBuf>` (creates parent dirs, calls resolver with `std::env::var`).

- [ ] **Step 1: Write the failing test.** Add to `main.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn zoid_db_overrides_everything() {
        let p = resolve_db_path(env_of(&[("ZOID_DB", "/tmp/x.db"), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/tmp/x.db"));
    }

    #[test]
    fn xdg_data_home_wins_over_home() {
        let p = resolve_db_path(env_of(&[("XDG_DATA_HOME", "/xdg"), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/xdg/zoid/zoid.db"));
    }

    #[test]
    fn falls_back_to_home_local_share() {
        let p = resolve_db_path(env_of(&[("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/home/u/.local/share/zoid/zoid.db"));
    }
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid resolve_db_path` (or `cargo test -p zoid xdg_data_home_wins_over_home`) → compile error (`resolve_db_path` missing).
- [ ] **Step 3: Write minimal implementation.** Replace `db_path` in `main.rs`:
```rust
/// Pure DB-path resolver (env injected for testing). Precedence:
/// `$ZOID_DB` > `$XDG_DATA_HOME/zoid/zoid.db` > `$HOME/.local/share/zoid/zoid.db`.
fn resolve_db_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("ZOID_DB") {
        return PathBuf::from(p);
    }
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".local/share"));
    base.join("zoid").join("zoid.db")
}

/// Resolve the DB path from the real environment and ensure its parent exists.
fn db_path() -> Result<PathBuf> {
    let path = resolve_db_path(|k| std::env::var(k).ok());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(path)
}
```
  (`use std::path::{Path, PathBuf};` already present; `Path` may become unused after Task 7 removes the legacy `.zoid` `Path::new` — keep until then.)
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid resolve_db_path` → 3 green.
- [ ] **Step 5: Commit.** `git add crates/zoid/src/main.rs && git commit -m "feat(bin): XDG-based DB path (~/.local/share/zoid/zoid.db), ZOID_DB override

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 7: One-time legacy `./.zoid/session.db` import
**Files:** Modify `crates/zoid/src/main.rs` (add `import_legacy_if_present`); `crates/zoid-core/src/store.rs` (already exposes `load_all`/`append`/`insert_session` — no change).
**Interfaces:** Produces `import_legacy_if_present(new_db: &Path, legacy: &Path, session_id: Ulid, name: &str, root_path: &str, ts: i64) -> Result<bool>` — returns `true` if an import ran. Consumes `EventStore`.

- [ ] **Step 1: Write the failing test.** Add to `main.rs` `mod tests`:
```rust
#[test]
fn imports_legacy_events_under_one_session_once() {
    use zoid_core::event::{Event, EventKind};
    use zoid_core::store::EventStore;
    use ulid::Ulid;
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.db");
    let newdb = dir.path().join("new.db");
    // Seed a legacy DB with two events (no meaningful session_id).
    {
        let s = EventStore::open(legacy.to_str().unwrap()).unwrap();
        s.append(&Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "old q".into() })).unwrap();
        s.append(&Event::new(Ulid::from(2u128), None, 2, EventKind::AssistantMessage { text: "old a".into() })).unwrap();
    }
    let sid = Ulid::from(42u128);
    // First run imports.
    assert!(import_legacy_if_present(&newdb, &legacy, sid, "imported", "/repo", 500).unwrap());
    let s = EventStore::open(newdb.to_str().unwrap()).unwrap();
    assert_eq!(s.load_session(sid).unwrap().len(), 2);
    assert_eq!(s.list_session_rows().unwrap().len(), 1);
    // Second run is a no-op (new DB already exists → nothing re-imported).
    assert!(!import_legacy_if_present(&newdb, &legacy, sid, "imported", "/repo", 500).unwrap());
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid imports_legacy_events_under_one_session_once` → compile error (`import_legacy_if_present` missing). (Add `tempfile` + `ulid` to `[dev-dependencies]`? `tempfile` is already a dev-dep; `ulid` is a normal dep — both available in tests.)
- [ ] **Step 3: Write minimal implementation.** In `main.rs`:
```rust
/// One-time pre-release migration: if a legacy in-repo `./.zoid/session.db`
/// exists and the new global DB does NOT yet exist, import the legacy events
/// under a single generated `session_id` (with a `sessions` row). Idempotent:
/// once the new DB exists we never import again. Returns whether an import ran.
fn import_legacy_if_present(
    new_db: &Path,
    legacy: &Path,
    session_id: Ulid,
    name: &str,
    root_path: &str,
    ts: i64,
) -> Result<bool> {
    if new_db.exists() || !legacy.exists() {
        return Ok(false);
    }
    let old = zoid_core::store::EventStore::open(legacy.to_str().context("legacy path not UTF-8")?)?;
    let events = old.load_all()?;
    if let Some(dir) = new_db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let store = zoid_core::store::EventStore::open(new_db.to_str().context("new db path not UTF-8")?)?;
    store.insert_session(session_id, name, root_path, ts, ts)?;
    for e in events {
        store.append(&e.with_session(session_id))?;
    }
    Ok(true)
}
```
  (`use zoid_core::store::` — reference fully-qualified as above to avoid a new top-level import; `Event::with_session` from Task 1.)
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid imports_legacy_events_under_one_session_once` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid/src/main.rs && git commit -m "feat(bin): one-time legacy .zoid/session.db import under a single session

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 8: Session lifecycle in the bin (auto-resume / create, active `session_id`)
**Files:** Modify `crates/zoid/src/main.rs` (`App` struct ~72–88, `record` ~93–98, `main` ~101–129, `spawn_turn` ~375–385, helpers); Modify `crates/zoid/src/agent.rs` (`run_agent_turn` ~80–92, `run_turn_inner` ~101–110, `emit` call sites).
**Interfaces:** Produces `repo_root() -> String`, `derive_session_name(first_user_msg: Option<&str>, ts_ms: i64, tz_offset_secs: i32) -> String`. Threads `App.session_id: Ulid` into `record` and `run_agent_turn(..., session_id: Ulid, ...)`.

- [ ] **Step 1: Write the failing test.** Add to `main.rs` `mod tests`:
```rust
#[test]
fn derives_name_from_first_message_else_timestamp() {
    // Truncates a long first message to <= 40 display chars with an ellipsis.
    let long = "fix the 500 error on GET /users/:id when the row is missing entirely";
    let n = derive_session_name(Some(long), 0, 0);
    assert!(n.chars().count() <= 40);
    assert!(n.starts_with("fix the 500"));
    // Empty / no message → timestamp fallback (HH:MM, deterministic at offset 0).
    assert_eq!(derive_session_name(None, 49_500_000, 0), "session 13:45");
    assert_eq!(derive_session_name(Some("   "), 0, 0), "session 00:00");
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid derives_name_from_first_message` → compile error.
- [ ] **Step 3: Write minimal implementation.**
  - Add helpers to `main.rs` (reuse `zoid_tui`'s HH:MM logic conceptually; `hhmm` is `pub(crate)` in zoid-tui, so recompute here with the same integer math to avoid exposing it):
```rust
/// Canonical repo/cwd root as a string (best-effort absolute path).
fn repo_root() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

/// Auto-derive a session name: the first user message truncated to 40 chars,
/// else `session HH:MM` from the injected timestamp.
fn derive_session_name(first_user_msg: Option<&str>, ts_ms: i64, tz_offset_secs: i32) -> String {
    let trimmed = first_user_msg.map(str::trim).filter(|s| !s.is_empty());
    match trimmed {
        Some(msg) => {
            let one_line = msg.lines().next().unwrap_or(msg);
            if one_line.chars().count() > 40 {
                let head: String = one_line.chars().take(39).collect();
                format!("{head}\u{2026}")
            } else {
                one_line.to_string()
            }
        }
        None => {
            let secs = ts_ms.div_euclid(1000) + tz_offset_secs as i64;
            let sod = secs.rem_euclid(86_400);
            format!("session {:02}:{:02}", sod / 3600, (sod % 3600) / 60)
        }
    }
}
```
  - Add `session_id: Ulid` to `struct App`. In `record`, tag the event: `let ev = Event::new(Ulid::new(), None, now_ms(), kind).with_session(self.session_id);`.
  - In `main`, replace the boot sequence (~103–129) with lifecycle wiring:
```rust
let path = db_path()?;
let root = repo_root();
// One-time legacy import (pre-release): ./.zoid/session.db → new global DB.
let legacy = Path::new(".zoid").join("session.db");
let tz_offset_secs = chrono::Local::now().offset().local_minus_utc();
let boot_ts = now_ms();
let _ = import_legacy_if_present(&path, &legacy, Ulid::new(),
    &derive_session_name(None, boot_ts, tz_offset_secs), &root, boot_ts);

let session = SessionHandle::spawn(path.to_str().context("session DB path is not valid UTF-8")?)?;

// Auto-resume the most-recently-touched session for this repo, else create one.
let sessions = session.list_sessions(Some(root.clone())).await.unwrap_or_default();
let session_id = if let Some(s) = sessions.first() {
    session.touch_session(s.id, boot_ts).await.ok();
    s.id
} else {
    let id = Ulid::new();
    session.new_session(id, derive_session_name(None, boot_ts, tz_offset_secs), root.clone(), boot_ts).await?;
    id
};
let events = session.snapshot_session(session_id).await?;
```
  - Set `session_id` in the `App { … }` literal. Keep `shell.branch = current_branch();` for now (repo view-model lands in Task 12).
  - In `Action::Submit` (~305–314), after `app.record(EventKind::UserMessage { text })`, if this is the session's first user message, rename it: capture whether `conversation(&app.events)` had zero `User` messages before the append, and if so call a rename through the session actor. Minimal inline:
```rust
Action::Submit => {
    if app.streaming { return Ok(false); }
    let text = app.textarea.lines().join("\n");
    if text.trim().is_empty() { return Ok(false); }
    let first = !app.events.iter().any(|e| matches!(e.kind, EventKind::UserMessage { .. }));
    app.textarea = TextArea::default();
    app.shell.status_hint = None;
    app.record(EventKind::UserMessage { text: text.clone() }).await?;
    if first {
        let name = derive_session_name(Some(&text), now_ms(), app.tz_offset_secs);
        app.session.rename_session(app.session_id, name.clone()).await.ok();
        app.shell.session_name = name; // field added in Task 13
    }
    app.streaming = true;
    spawn_turn(app);
}
```
  > If Task 13 (which adds `shell.session_name`) is not yet done, drop the `app.shell.session_name = name;` line and re-add it in Task 13.
  - Thread `session_id` into the agent: change `run_agent_turn(provider, tools, session, seed, model, ui, now_ms)` in `spawn_turn` to also pass `app.session_id`; change `run_agent_turn`/`run_turn_inner` signatures in `agent.rs` to accept `session_id: Ulid` and have `emit` build `Event::new(...).with_session(session_id)`. Update `emit`'s signature to take `session_id: Ulid` and every `emit(&session, &mut events, ui, kind, now)` call to `emit(&session, &mut events, ui, kind, session_id, now)`.
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid derives_name_from_first_message` → green; `cargo build` → the whole workspace compiles; `cargo test -p zoid` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid/src/main.rs crates/zoid/src/agent.rs && git commit -m "feat(bin): session lifecycle — auto-resume-for-repo, create, first-msg rename, tagged events

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 9: Command line `:new` / `:rename` (+ resume command)
**Files:** Modify `crates/zoid-tui/src/command.rs` (`Command` ~6–12, `parse_command` ~16–26; tests ~28–45).
**Interfaces:** Produces `Command::{NewSession, RenameSession(String), ResumeSessionPicker}` (in addition to existing variants). `parse_command` maps `:new`, `:rename`, `:rename <name>`.

- [ ] **Step 1: Write the failing test.** Add to `command.rs` `mod tests`:
```rust
#[test]
fn parses_session_commands() {
    assert_eq!(parse_command(":new"), Command::NewSession);
    assert_eq!(parse_command("new"), Command::NewSession);
    assert_eq!(parse_command(":rename"), Command::RenameSession(String::new()));
    assert_eq!(parse_command(":rename fix login"), Command::RenameSession("fix login".into()));
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui parses_session_commands` → compile error (variants missing).
- [ ] **Step 3: Write minimal implementation.** In `command.rs`:
  - Extend the enum:
```rust
pub enum Command {
    SwitchMode(Mode),
    Quit,
    OpenDrawer(DrawerId),
    NewSession,
    /// Rename the active session. Empty string = "prompt me" (the bin opens the
    /// command line seeded with `rename `); non-empty = apply directly.
    RenameSession(String),
    /// Open the resume-session picker overlay (palette-only; no `:` form).
    ResumeSessionPicker,
    Unknown(String),
}
```
  - Extend `parse_command` (before the `other =>` arm):
```rust
"new" => Command::NewSession,
"rename" => Command::RenameSession(String::new()),
s if s.starts_with("rename ") => Command::RenameSession(s["rename ".len()..].trim().to_string()),
```
  > `DrawerId::Files`/`Branch` in the `"files"`/`"branch"` arms are retargeted in Task 14; leave them for now so the crate still compiles.
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui parses_session_commands` and `cargo test -p zoid-tui command::` → green.
- [ ] **Step 5: Commit.** `git add crates/zoid-tui/src/command.rs && git commit -m "feat(tui): :new / :rename command parsing + session Command variants

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 10: Palette session group + resume-session overlay (+ palette snapshot)
**Files:** Modify `crates/zoid-tui/src/palette.rs` (`all_items` ~22–44); `crates/zoid-tui/src/state.rs` (`Overlay` ~39–46, `ShellState` sessions fields + `close_overlay`); `crates/zoid-tui/src/route.rs` (overlay key routing + `Action`); `crates/zoid-tui/src/render.rs` (`render_shell` overlay dispatch + `render_sessions_overlay`); `crates/zoid-tui/src/layout.rs` (overlay rect); `crates/zoid/src/main.rs` (execute the new commands); Test `crates/zoid-tui/tests/session_snapshot.rs` (new) + existing `palette_overlay_frame` snapshot update.
**Interfaces:** Produces the **session** palette group (New / Resume / Rename), `Overlay::Sessions`, `Action::{SessionMove(i32), SessionPick}`, `render_sessions_overlay`. Consumes `Command::{NewSession, RenameSession, ResumeSessionPicker}`.

- [ ] **Step 1: Write the failing test.** Add to `palette.rs` `mod tests`:
```rust
#[test]
fn session_group_is_first_and_selectable() {
    let items = all_items(Mode::Chat);
    // The session group leads the palette (matches palette.html).
    assert_eq!(items[0].group, "session");
    let labels: Vec<&str> = items.iter().filter(|i| i.group == "session").map(|i| i.label).collect();
    assert_eq!(labels, vec!["New session", "Resume session…", "Rename session…"]);
    // All three are selectable (have commands).
    for l in ["New session", "Resume session…", "Rename session…"] {
        assert!(selectable_matches(&items, l).iter().any(|&i| items[i].label == l));
    }
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui session_group_is_first_and_selectable` → fails (group absent / order wrong).
- [ ] **Step 3: Write minimal implementation.**
  - `palette.rs` — prepend the session group at the top of the `vec![…]` in `all_items` (before the `mode` row). Icons via tokens (reuse existing glyphs; `palette.html` shows `＋`/`↺`/`✎` — map to available tokens to avoid adding three glyphs: New→`glyph::USER_TURN` is wrong semantically; instead add three tokens in Task 12's tokens work or reuse `glyph::EDIT`/`glyph::UNDO`/`glyph::EDIT`). **To keep §16 clean without inventing glyphs here, reuse:** New→`glyph::MODE_SWITCH` is misleading; **preferred:** add `glyph::NEW='＋'`, `glyph::RESUME='↺'`, `glyph::RENAME='✎'` to `tokens.rs` (with a token test) — do that as Step 3a below, then use them.
    - Step 3a — in `tokens.rs` `mod glyph`, add:
```rust
pub const NEW: char = '＋';     // palette: new session
pub const RESUME: char = '↺';   // palette: resume session
pub const RENAME: char = '✎';   // palette: rename session
```
      and a token assertion in `tokens.rs` tests: `assert_eq!(glyph::NEW, '＋');` etc.
    - Step 3b — session group rows:
```rust
PaletteItem { group: "session".to_string(), icon: glyph::NEW, label: "New session", hint: "fresh thread + clean context budget", keybind: ":new", command: Some(Command::NewSession) },
PaletteItem { group: "session".to_string(), icon: glyph::RESUME, label: "Resume session…", hint: "this repo, most-recent first", keybind: "⏎", command: Some(Command::ResumeSessionPicker) },
PaletteItem { group: "session".to_string(), icon: glyph::RENAME, label: "Rename session…", hint: "rename the current thread", keybind: ":rename", command: Some(Command::RenameSession(String::new())) },
```
  - `state.rs` — add `Sessions` to `Overlay`; add view-model + picker state to `ShellState`:
```rust
// in Overlay enum:
Sessions,
// in ShellState struct:
pub sessions: Vec<String>,      // display rows for the resume picker (bin-formatted)
pub session_selected: usize,
```
    Initialize both in `ShellState::new` (`sessions: Vec::new(), session_selected: 0,`) and reset in `close_overlay` (`self.sessions.clear(); self.session_selected = 0;`).
  - `route.rs` — add `Action::SessionMove(i32)` and `Action::SessionPick`; add `Overlay::Sessions => return route_sessions_key(key),` to the overlay match; implement `route_sessions_key` mirroring `route_objects_key` (Up/`k`→SessionMove(-1), Down/`j`→SessionMove(1), Enter→SessionPick, Esc→CloseOverlay).
  - `layout.rs` — include `Overlay::Sessions` in the `palette` rect `matches!(…)` so the picker gets a centered rect.
  - `render.rs` — in `render_shell`, add `else if state.overlay == Overlay::Sessions { if let Some(p) = layout.palette { render_sessions_overlay(frame, state, p); } }`; implement:
```rust
fn render_sessions_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    let rows = if state.sessions.is_empty() { vec!["(no sessions for this repo)".to_string()] } else { state.sessions.clone() };
    let sel = nav(state.session_selected, 0, rows.len());
    list_overlay(frame, area, format!(" {} resume session ", glyph::RESUME), &rows, sel);
}
```
  - `main.rs` — handle the new actions and commands:
    - In `handle_action`, add `Action::SessionMove(d) => { app.shell.session_selected = zoid_tui::palette::nav(app.shell.session_selected, d, app.shell.sessions.len()); }` and `Action::SessionPick => { … resume … }`. Resume: keep `app.session_ids: Vec<Ulid>` on `App` (populated when opening the picker) and map the selected index to a `session_id`, then reload:
```rust
Action::SessionPick => {
    if let Some(&sid) = app.session_ids.get(app.shell.session_selected) {
        app.session.touch_session(sid, now_ms()).await.ok();
        app.session_id = sid;
        app.events = app.session.snapshot_session(sid).await.unwrap_or_default();
        app.shell.conversation_scroll = 0;
    }
    app.shell.close_overlay();
}
```
    - In `exec_command`, add:
```rust
Command::NewSession => {
    let id = Ulid::new();
    let ts = now_ms();
    let name = derive_session_name(None, ts, app.tz_offset_secs);
    app.session.new_session(id, name, repo_root(), ts).await.ok();
    app.session_id = id;
    app.events.clear();
    app.shell.conversation_scroll = 0;
    Ok(false)
}
Command::RenameSession(name) => {
    if name.is_empty() {
        // Seed the command line so the user types the name.
        app.shell.overlay = zoid_tui::Overlay::CommandLine;
        app.shell.cmdline.buffer = "rename ".into();
    } else {
        app.session.rename_session(app.session_id, &name).await.ok();
    }
    Ok(false)
}
Command::ResumeSessionPicker => {
    let list = app.session.list_sessions(Some(repo_root())).await.unwrap_or_default();
    app.session_ids = list.iter().map(|s| s.id).collect();
    app.shell.sessions = list.iter()
        .map(|s| format!("{}  ·  {}  ·  {}",
            s.name,
            fmt_since(s.last_touched_ts, now_ms()),                 // helper from Task 12
            zoid_tui::economy_view::human_tokens(s.token_total)))
        .collect();
    app.shell.session_selected = 0;
    app.shell.overlay = zoid_tui::Overlay::Sessions;
    Ok(false)
}
```
    - Add `session_ids: Vec<Ulid>` to `struct App` (init `Vec::new()`). `exec_command` runs *after* `close_overlay` in the palette path, so setting `overlay = Sessions` there correctly re-opens the picker. `human_tokens` is `pub` in `economy_view`.
    - `fmt_since` is introduced in Task 12; if you reach here first, inline `format!("{}k", …)` or move `fmt_since`/`fmt_duration` earlier.
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui session_group_is_first_and_selectable` green; `cargo build` (whole workspace) compiles. Add the snapshot test to `session_snapshot.rs`:
```rust
#[test]
fn resume_session_overlay_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Sessions;
    s.sessions = vec![
        "fix 500 on GET /users  ·  12m ago  ·  58k".into(),
        "rail restructure       ·  3h ago   ·  120k".into(),
    ];
    insta::assert_snapshot!(draw(&s, &[], 100, 24));
}
```
  (Copy the `draw`/`empty_economy`/`normal_view` helpers from `shell_snapshot.rs` into `session_snapshot.rs`, or factor them — keep it simple by duplicating for now.) Run `cargo test -p zoid-tui --test session_snapshot`; first run writes `.snap.new`. Also re-run `cargo test -p zoid-tui --test shell_snapshot` — `palette_overlay_frame` now includes the session group, so its snapshot changes.
- [ ] **Step 5: Commit.** Accept snapshots then commit: `cargo insta accept && git add crates/zoid-tui crates/zoid/src/main.rs && git commit -m "feat: palette session group + resume-session overlay; snapshots to palette.html

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 11: Rail restructure — `DrawerId::{Repo, Session, Context}`
**Files:** Modify `crates/zoid-tui/src/state.rs` (`DrawerId` ~48–53, `Drawer` ~55–61, `ShellState::new` ~111–116, tests); `crates/zoid-tui/src/layout.rs` (body-row consts + `drawer_body_rows` + tests); `crates/zoid-tui/src/render.rs` (`render_rail` chevron/keybind + `DrawerId::Economy` match + `drawer_body`); `crates/zoid-tui/src/route.rs` (test call-sites); `crates/zoid-tui/examples/preview.rs`; `crates/zoid-tui/tests/shell_snapshot.rs`.
**Interfaces:** `DrawerId` becomes `{ Repo, Session, Context }`; `Drawer` loses `keybind`. Rail default set (top→bottom): Repo (open), Session (open), Context (open) — matching `docs/ux/chat-mode.html`.

- [ ] **Step 1: Write the failing test.** Replace the `new_is_calm_chat_with_chat_rail` assertions in `state.rs` tests:
```rust
#[test]
fn new_is_calm_chat_with_repo_session_context_rail() {
    let s = ShellState::new();
    assert_eq!(s.mode, Mode::Chat);
    assert!(s.rail_visible);
    let ids: Vec<DrawerId> = s.drawers.iter().map(|d| d.id).collect();
    assert_eq!(ids, vec![DrawerId::Repo, DrawerId::Session, DrawerId::Context]);
    // All three expanded (mockup shows repo/session/context all `on`).
    assert!(s.drawer(DrawerId::Repo).unwrap().open);
    assert!(s.drawer(DrawerId::Session).unwrap().open);
    assert!(s.drawer(DrawerId::Context).unwrap().open);
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui new_is_calm_chat_with_repo_session_context_rail` → compile error (variants don't exist).
- [ ] **Step 3: Write minimal implementation.**
  - `state.rs`:
    - `DrawerId` → `pub enum DrawerId { Repo, Session, Context }`.
    - `Drawer` → drop `keybind`: `pub struct Drawer { pub id: DrawerId, pub title: String, pub open: bool }`.
    - `ShellState::new` drawers (glyph for context title via `crate::tokens::glyph`):
```rust
use crate::tokens::glyph;
let drawers = vec![
    Drawer { id: DrawerId::Repo,    title: "repo".into(),    open: true },
    Drawer { id: DrawerId::Session, title: "session".into(), open: true },
    Drawer { id: DrawerId::Context, title: format!("{} context · tokens", glyph::CONTEXT), open: true },
];
```
    - Update the other `state.rs` tests that reference `DrawerId::Files`/`Branch`/`Economy`: `drawer_lookup_returns_none_for_absent` (use `DrawerId::Session`), `toggle_drawer_flips_open_and_opens_rail` (use `DrawerId::Session`/`DrawerId::Repo`). Remove the `assert_eq!(s.branch, "main")` line only if `branch` field is being removed — it is **not** (branch stays as repo view-model), so keep it.
  - `layout.rs`:
    - Replace `DRAWER_BODY_ROWS`/`ECONOMY_BODY_ROWS` with per-drawer consts:
```rust
pub const REPO_BODY_ROWS: u16 = 3;    // name+branch · worktree · changes
pub const SESSION_BODY_ROWS: u16 = 5; // name · model·provider · dur·tok · ctx · cwd
pub const CONTEXT_BODY_ROWS: u16 = 6; // items + churn + ledger/toggle (unchanged economy body)
```
    - `drawer_body_rows`:
```rust
pub fn drawer_body_rows(id: DrawerId) -> u16 {
    match id {
        DrawerId::Repo => REPO_BODY_ROWS,
        DrawerId::Session => SESSION_BODY_ROWS,
        DrawerId::Context => CONTEXT_BODY_ROWS,
    }
}
```
    - Update layout tests: `wide_shows_rail_and_drawer_headers` (comment/count stays 3), `open_drawer_gets_a_body_rect_sized_by_kind` (toggle `DrawerId::Session`; compare `DrawerId::Context`==`CONTEXT_BODY_ROWS`, `DrawerId::Session`==`SESSION_BODY_ROWS`; closed check use `DrawerId::Repo` after toggling it closed), `headers_stack_below_taller_economy_body` (use `CONTEXT_BODY_ROWS`; note default now has all three open, so recompute expected `y` = header(1) + CONTEXT? No — Context is **last**; the *first* drawer is Repo. Rewrite the assertion to check the second header sits below Repo's body: `assert_eq!(headers[1].1.y, headers[0].1.y + 1 + REPO_BODY_ROWS + 1);`).
  - `render.rs`:
    - `render_rail`: remove the `keybind` span (the `format!("  {}", d.keybind)` line) — header is now just chevron + title. Change `if d.id == DrawerId::Economy` → `if d.id == DrawerId::Context` (still routes to `render_economy_body` — the context body is unchanged in content).
    - `drawer_body`: the `Files`/`Branch`/`Economy` arms are replaced in Tasks 12–14; for **this** task, temporarily stub the non-context arms so the crate compiles:
```rust
fn drawer_body(id: DrawerId, state: &ShellState) -> Vec<Line<'static>> {
    match id {
        DrawerId::Repo => vec![Line::styled("repo · Task 12", Style::new().fg(color::DIM))],
        DrawerId::Session => vec![Line::styled("session · Task 13", Style::new().fg(color::DIM))],
        DrawerId::Context => vec![Line::styled("context economy", Style::new().fg(color::DIM))],
    }
}
```
  - `route.rs` tests `hit_test_drawer_header_and_panes` + `mouse_click_toggles_drawer_and_focuses`: change `DrawerId::Files` → `DrawerId::Session`.
  - `examples/preview.rs`: remove `s.files = …;` and change `s.toggle_drawer(DrawerId::Files)` → `DrawerId::Session` (in the `"files"` scene — rename the scene key to `"session"` if you like, or leave the string key and just retarget the drawer).
  - `tests/shell_snapshot.rs`: line 119 `s.toggle_drawer(DrawerId::Files)` → `DrawerId::Session`; remove any `s.files = …` usage; the `files_drawer_open_frame` test — rename to `session_drawer_open_frame` (or delete; the real session snapshot lands in Task 13). The `economy_drawer_frame` snapshots still pass since Context reuses the economy body.
- [ ] **Step 4: Run test to verify it passes.** `cargo build` (workspace) compiles; `cargo test -p zoid-tui state:: layout::` green. Snapshot tests will have changed frames (rail titles, no keybinds) — regenerate: `cargo test -p zoid-tui`, eyeball `.snap.new` (headers show `▾ repo` / `▾ session` / `▾ ⑤ context · tokens`, no `^5`/`^F`), then `cargo insta accept`.
- [ ] **Step 5: Commit.** `git add crates/zoid-tui crates/zoid/src/main.rs 2>/dev/null; git add -A crates/zoid-tui && git commit -m "refactor(tui): rail drawers → repo/session/context; drop keybind field; snapshots

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 12: Repo drawer body — branch + worktree + changes line
**Files:** Modify `crates/zoid-tui/src/tokens.rs` (`color::ADDED`/`REMOVED`); `crates/zoid-tui/src/state.rs` (repo view-model fields); `crates/zoid-tui/src/render.rs` (`render_repo_body`); `crates/zoid/src/main.rs` (`git_status` + `parse_numstat` + `fmt_since`/`fmt_duration`, set repo fields); Test `crates/zoid/src/main.rs` (parse_numstat), `crates/zoid-tui/tests/session_snapshot.rs` (repo snapshot).
**Interfaces:** Produces `parse_numstat(&str) -> (usize, usize, usize)` (added, removed, files) and `git_status() -> (usize, usize, usize)`; `render_repo_body`. Consumes `ShellState.{repo_name, branch, worktree, changes_added, changes_removed, changes_files}`.

- [ ] **Step 1: Write the failing test.** Add to `main.rs` `mod tests`:
```rust
#[test]
fn parses_numstat_sums_and_counts_files() {
    let out = "12\t3\tsrc/a.rs\n0\t5\tsrc/b.rs\n7\t0\tCargo.toml\n";
    assert_eq!(parse_numstat(out), (19, 8, 3)); // added=12+0+7, removed=3+5+0, files=3
    // Binary files show `-\t-\tpath`; count the file, add zero lines.
    assert_eq!(parse_numstat("-\t-\tlogo.png\n"), (0, 0, 1));
    assert_eq!(parse_numstat(""), (0, 0, 0));
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid parses_numstat_sums_and_counts_files` → compile error.
- [ ] **Step 3: Write minimal implementation.**
  - `tokens.rs` `mod color`: add aliases (DRY — the changes line reuses the status palette; §16 keeps them here):
```rust
pub const ADDED: Color = OK;    // repo changes line: +added lines
pub const REMOVED: Color = ERROR; // repo changes line: -removed lines
```
    and a token test: `assert_eq!(color::ADDED, color::OK); assert_eq!(color::REMOVED, color::ERROR);`.
  - `state.rs` `ShellState`: add fields (keep the existing `branch`):
```rust
pub repo_name: String,
pub worktree: String,
pub changes_added: usize,
pub changes_removed: usize,
pub changes_files: usize,
```
    Initialize in `ShellState::new` (`repo_name: String::new(), worktree: "(none)".into(), changes_added: 0, changes_removed: 0, changes_files: 0,`).
  - `render.rs` — replace the `DrawerId::Repo` stub in `drawer_body` with a dedicated renderer routed from `render_rail` (mirror how Context routes to `render_economy_body`). In `render_rail`, extend the `d.open` block:
```rust
if d.id == DrawerId::Context {
    render_economy_body(frame, economy, rect, state.focus == Focus::Rail);
} else if d.id == DrawerId::Repo {
    render_repo_body(frame, state, rect);
} else if d.id == DrawerId::Session {
    render_session_body(frame, state, rect); // Task 13
}
```
    and add:
```rust
fn render_repo_body(frame: &mut Frame, state: &ShellState, area: Rect) {
    let name = if state.repo_name.is_empty() { "repo" } else { &state.repo_name };
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{name}   "), Style::new().fg(color::TXT)),
            Span::styled(format!("{} {}", glyph::BRANCH, state.branch), Style::new().fg(color::BRANCH)),
        ]),
        Line::from(vec![
            Span::styled("worktree ", Style::new().fg(color::DIM)),
            Span::styled(state.worktree.clone(), Style::new().fg(color::DIM)),
        ]),
        Line::from(vec![
            Span::styled("changes ", Style::new().fg(color::DIM)),
            Span::styled(format!("+{}", state.changes_added), Style::new().fg(color::ADDED)),
            Span::styled(" ", Style::new()),
            Span::styled(format!("-{}", state.changes_removed), Style::new().fg(color::REMOVED)),
            Span::styled(format!(" · {} files", state.changes_files), Style::new().fg(color::DIM)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}
```
    (Remove the `DrawerId::Repo` arm from `drawer_body`; `drawer_body` now only ever handles cases routed to it — since Repo/Session/Context are all routed above, `drawer_body` becomes dead. Delete `drawer_body` entirely and the `else { frame.render_widget(Paragraph::new(drawer_body(d.id, state)), rect); }` fallback in `render_rail`.)
  - `main.rs` — add helpers:
```rust
/// Parse `git diff --numstat` output → (added, removed, files). Binary files
/// show `-` for both counts (counted as a file, zero lines). Pure.
fn parse_numstat(out: &str) -> (usize, usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut files = 0usize;
    for line in out.lines().filter(|l| !l.trim().is_empty()) {
        let mut cols = line.split('\t');
        let a = cols.next().unwrap_or("-");
        let r = cols.next().unwrap_or("-");
        if cols.next().is_none() { continue; } // no path → malformed, skip
        added += a.parse::<usize>().unwrap_or(0);
        removed += r.parse::<usize>().unwrap_or(0);
        files += 1;
    }
    (added, removed, files)
}

/// Working-tree change stats via `git diff --numstat` (unstaged) + `--cached`
/// (staged). Best-effort — any failure yields zeros.
fn git_status() -> (usize, usize, usize) {
    let run = |args: &[&str]| -> String {
        std::process::Command::new("git").args(args).output().ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let (a1, r1, f1) = parse_numstat(&run(&["diff", "--numstat"]));
    let (a2, r2, f2) = parse_numstat(&run(&["diff", "--numstat", "--cached"]));
    (a1 + a2, r1 + r2, f1 + f2)
}

/// Compact "N ago" from two epoch-millis stamps (e.g. "12m ago", "3h ago").
fn fmt_since(then_ms: i64, now_ms: i64) -> String {
    let mins = (now_ms - then_ms).max(0) / 60_000;
    if mins < 60 { format!("{mins}m ago") }
    else if mins < 1440 { format!("{}h ago", mins / 60) }
    else { format!("{}d ago", mins / 1440) }
}

/// Compact duration since `start_ms` (e.g. "12m", "1h3m").
fn fmt_duration(start_ms: i64, now_ms: i64) -> String {
    let mins = (now_ms - start_ms).max(0) / 60_000;
    if mins < 60 { format!("{mins}m") } else { format!("{}h{}m", mins / 60, mins % 60) }
}
```
  - `main.rs` — set repo fields at boot and refresh `git_status` each loop iteration (before `terminal.draw`, not inside the closure, to avoid a borrow conflict). At boot, after building `shell`: `shell.repo_name = Path::new(&root).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.clone());`. In the `run` loop, at the top of `loop {` before `terminal.draw`, add a cadence guard (refresh every ~1s or on turn completion) — simplest correct version: refresh each iteration:
```rust
let (a, r, f) = git_status();
app.shell.changes_added = a;
app.shell.changes_removed = r;
app.shell.changes_files = f;
```
    (If per-tick `git` shell-outs prove heavy, gate behind an `Instant` cadence — a follow-up; correctness first.)
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid parses_numstat_sums_and_counts_files` green; `cargo build` compiles. Add repo snapshot to `session_snapshot.rs`:
```rust
#[test]
fn repo_drawer_frame() {
    let mut s = ShellState::new();
    s.repo_name = "zoid".into();
    s.branch = "main".into();
    s.changes_added = 128; s.changes_removed = 34; s.changes_files = 7;
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}
```
    Run `cargo test -p zoid-tui --test session_snapshot`; eyeball `.snap.new` against `chat-mode.html` (`zoid ⎇ main`, `worktree (none)`, `changes +128 -34 · 7 files`); `cargo insta accept`.
- [ ] **Step 5: Commit.** `git add -A crates/zoid-tui crates/zoid/src/main.rs && git commit -m "feat: repo rail drawer — branch + worktree + git changes line; snapshot to chat-mode.html

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 13: Session drawer body — name · model/provider · duration · tokens · ctx · cwd (truncated)
**Files:** Modify `crates/zoid-tui/src/state.rs` (session view-model fields); `crates/zoid-tui/src/render.rs` (`render_session_body`, using `text::truncate` for cwd); `crates/zoid/src/main.rs` (populate fields, provider label); Test `crates/zoid-tui/tests/session_snapshot.rs` (session snapshot, incl. a long-cwd truncation case).
**Interfaces:** Consumes `ShellState.{session_name, model, provider, duration, session_tokens, ctx_used, ctx_ceiling, cwd}`. Produces `render_session_body`.

- [ ] **Step 1: Write the failing test.** Add to `session_snapshot.rs`:
```rust
#[test]
fn session_drawer_truncates_long_cwd() {
    let mut s = ShellState::new();
    s.session_name = "fix 500 on GET /users".into();
    s.model = "glm-5.2".into();
    s.provider = "ollama".into();
    s.duration = "12m".into();
    s.session_tokens = 58_000;
    s.ctx_used = 58_000; s.ctx_ceiling = 200_000;
    s.cwd = "~/develop/projects/zoid/crates/zoid-tui/src/very/deep/nested/path".into();
    let out = draw(&s, &seeded(), 100, 24);
    // The cwd never wraps — it is truncated with the §16 ellipsis.
    assert!(out.contains('\u{2026}'), "long cwd should be truncated with an ellipsis");
    insta::assert_snapshot!(out);
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui session_drawer_truncates_long_cwd` → compile error (fields missing).
- [ ] **Step 3: Write minimal implementation.**
  - `state.rs` `ShellState`: add fields + init in `new`:
```rust
pub session_name: String, // default String::new()
pub model: String,        // default String::new()
pub provider: String,     // default String::new()
pub duration: String,     // default "0m".into()
pub session_tokens: u64,  // default 0
pub ctx_used: u64,        // default 0
pub ctx_ceiling: u64,     // default 0
pub cwd: String,          // default String::new()
```
  - `render.rs` — add (uses `crate::text::truncate` + `economy_view::human_tokens`):
```rust
fn render_session_body(frame: &mut Frame, state: &ShellState, area: Rect) {
    use crate::economy_view::human_tokens;
    use crate::text::truncate;
    let name = if state.session_name.is_empty() { "(unnamed)" } else { &state.session_name };
    let ctx = if state.ctx_ceiling > 0 {
        format!("{}/{}", human_tokens(state.ctx_used), human_tokens(state.ctx_ceiling))
    } else {
        human_tokens(state.ctx_used)
    };
    let mut lines = vec![
        Line::from(Span::styled(truncate(name, area.width as usize), Style::new().fg(color::TXT))),
        Line::from(vec![
            Span::styled(state.model.clone(), Style::new().fg(color::CHAT_ACCENT)),
            Span::styled(format!(" · {}", state.provider), Style::new().fg(color::DIM)),
        ]),
        Line::from(vec![
            Span::styled("dur ", Style::new().fg(color::DIM)),
            Span::styled(format!("{}   ", state.duration), Style::new().fg(color::TXT)),
            Span::styled("tok ", Style::new().fg(color::DIM)),
            Span::styled(human_tokens(state.session_tokens), Style::new().fg(color::TXT)),
        ]),
        Line::from(vec![
            Span::styled("ctx ", Style::new().fg(color::DIM)),
            Span::styled(ctx, Style::new().fg(color::TXT)),
        ]),
    ];
    // cwd: truncate to the drawer width, never wrap (paths get long).
    lines.push(Line::from(vec![
        Span::styled("cwd ", Style::new().fg(color::DIM)),
        Span::styled(truncate(&state.cwd, (area.width as usize).saturating_sub(4)), Style::new().fg(color::DIM)),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}
```
  - `main.rs` — populate at boot + refresh volatile fields before `terminal.draw` (alongside the Task-12 `git_status` block):
    - Boot: `shell.model = model.clone(); shell.provider = provider_label(); shell.cwd = root.clone(); shell.session_name = <resumed-or-derived name>;` where:
```rust
/// Human provider label from the same env used by `default_model` selection.
fn provider_label() -> String {
    if std::env::var("OLLAMA_API_KEY").map(|k| !k.is_empty()).unwrap_or(false) {
        "ollama".into()
    } else {
        "anthropic".into()
    }
}
```
      Capture the resumed session's name (from the `list_sessions` result in Task 8) or the derived name; store it in a local and set `shell.session_name`.
    - Per-loop (before draw): compute from the same projections the draw closure uses:
```rust
let ledger = zoid_core::economy::token_ledger(&app.events);
let window = zoid_core::context::context_window(&app.events);
app.shell.session_tokens = ledger.total;
app.shell.ctx_used = window.total_tokens;
app.shell.ctx_ceiling = 200_000; // matches the P3 default ceiling shown in the mock
app.shell.duration = fmt_duration(app.session_started_ms, now_ms());
```
      Add `session_started_ms: i64` to `App` (set to the resumed session's `created_ts`, or `boot_ts` for a fresh session).
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui session_drawer_truncates_long_cwd` green; `cargo build` compiles. Eyeball `.snap.new` against `chat-mode.html` session widget (`fix 500 on GET /users`, `glm-5.2 · ollama`, `dur 12m   tok 58k`, `ctx 58k/200k`, `cwd …`); `cargo insta accept`.
- [ ] **Step 5: Commit.** `git add -A crates/zoid-tui crates/zoid/src/main.rs && git commit -m "feat: session rail drawer — name/model/provider/duration/tokens/ctx/cwd (truncated); snapshot

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Task 14: Context ⑤ drawer wiring + chrome cleanup + navigate group
**Files:** Modify `crates/zoid-tui/src/command.rs` (retarget `:repo`/`:session`/`:context`); `crates/zoid-tui/src/palette.rs` (navigate group → new drawers); `crates/zoid-tui/src/render.rs` (`render_title`/`render_status`: remove branch); `crates/zoid/src/main.rs` (drop `cwd_files`, `current_branch` now feeds `shell.branch` only); Test updates + snapshots (`shell_snapshot.rs` chrome).
**Interfaces:** `Command::OpenDrawer(DrawerId::{Repo,Session,Context})`; palette *navigate* toggles the three drawers; title/status bars no longer render branch (branch is rail-only, §2.2).

- [ ] **Step 1: Write the failing test.** In `command.rs` tests, replace the `:files`/`:branch` assertions:
```rust
#[test]
fn parses_drawer_toggle_commands() {
    assert_eq!(parse_command(":repo"), Command::OpenDrawer(DrawerId::Repo));
    assert_eq!(parse_command(":session"), Command::OpenDrawer(DrawerId::Session));
    assert_eq!(parse_command(":context"), Command::OpenDrawer(DrawerId::Context));
}
```
  And in `palette.rs` tests add:
```rust
#[test]
fn navigate_group_targets_new_drawers() {
    let items = all_items(Mode::Chat);
    let nav: Vec<&Command> = items.iter()
        .filter(|i| i.group == "navigate")
        .filter_map(|i| i.command.as_ref())
        .collect();
    assert!(nav.contains(&&Command::OpenDrawer(DrawerId::Repo)));
    assert!(nav.contains(&&Command::OpenDrawer(DrawerId::Session)));
    assert!(nav.contains(&&Command::OpenDrawer(DrawerId::Context)));
}
```
- [ ] **Step 2: Run test to verify it fails.** `cargo test -p zoid-tui parses_drawer_toggle_commands navigate_group_targets_new_drawers` → fails.
- [ ] **Step 3: Write minimal implementation.**
  - `command.rs` `parse_command`: replace the `"files"`/`"branch"` arms with:
```rust
"repo" => Command::OpenDrawer(DrawerId::Repo),
"session" => Command::OpenDrawer(DrawerId::Session),
"context" => Command::OpenDrawer(DrawerId::Context),
```
  - `palette.rs` `all_items` navigate group — replace the two "Open files/branch drawer" rows with three toggles (no keybind labels, per §2.1):
```rust
PaletteItem { group: "navigate".to_string(), icon: glyph::COLLAPSED, label: "Toggle repo drawer", hint: "repo · branch · changes", keybind: "", command: Some(Command::OpenDrawer(DrawerId::Repo)) },
PaletteItem { group: "navigate".to_string(), icon: glyph::COLLAPSED, label: "Toggle session drawer", hint: "name · model · cost · cwd", keybind: "", command: Some(Command::OpenDrawer(DrawerId::Session)) },
PaletteItem { group: "navigate".to_string(), icon: glyph::CONTEXT, label: "Toggle context ⑤ drawer", hint: "tokens · heat · churn", keybind: "", command: Some(Command::OpenDrawer(DrawerId::Context)) },
```
  - `render.rs`:
    - `render_title` (~80–91): drop the branch span — title shows only ` zoid ` + the mode chip (§2.2: branch is rail-only). Keep the `{label}` mode chip.
    - `render_status` (~113–129): remove `glyph::BRANCH, state.branch` from the Chat status format string (leave zoom/palette/quit hints).
  - `main.rs`: delete `cwd_files` (no files drawer) and the `shell.files = cwd_files(64);` line; keep `current_branch()` and `shell.branch = current_branch();` (feeds the repo drawer). Remove the now-unused `files` field usage — the `files` field was removed from `ShellState` in Task 11 if you followed the note; if it still exists, remove it now (and any remaining `s.files` in tests/examples).
- [ ] **Step 4: Run test to verify it passes.** `cargo test -p zoid-tui command:: palette::` green; `cargo build` (workspace) compiles; `cargo test` (workspace) green. Chrome snapshots (`chat_with_rail_frame`, `build_placeholder_frame`, etc.) changed (no branch in title/status) — `cargo test -p zoid-tui`, eyeball `.snap.new` (branch appears **only** in the repo drawer now — asserts the §2.2 consolidation), `cargo insta accept`.
- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat: context ⑤ drawer nav + chrome consolidation (branch rail-only); :repo/:session/:context

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"`

---

## Final verification
- [ ] `cargo test` (whole workspace) — all unit, integration, snapshot, and proptest cases green.
- [ ] `cargo build --release` — size-optimized profile still links (no new heavy deps).
- [ ] Manual smoke: `cargo run -p zoid` in a git repo → boots, resumes/creates a session for this repo, rail shows repo/session/context; `^P` → session group (New/Resume/Rename); `:new` starts a clean thread; type a first message → session auto-renames; quit and relaunch → the same session auto-resumes (`~/.local/share/zoid/zoid.db`).
- [ ] `git grep -n "0x" crates/zoid-tui/src | grep -v tokens.rs` returns nothing (no stray hex); rail glyphs (`＋`/`↺`/`✎`, changes colors) all live in `tokens.rs`.
- [ ] Spec coverage check: chat §7 (session mgmt), §2.1 (repo/session/context rail), §10 (session tests + fidelity snapshots), core §5 (Event.session_id, sessions table, `SessionList()`), §7.1 (`~/.local/share/zoid/zoid.db`, `$XDG_DATA_HOME`, `ZOID_DB`) — all satisfied.
