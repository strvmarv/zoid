# Multi-Instance Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make zoid safe to run many process instances on the same host sharing one global `zoid.db`: tune SQLite for concurrent access (WAL + `busy_timeout`), add a stateful "active interface" liveness flag to sessions, and change boot/resume behavior so the second instance in a folder gets its own fresh session instead of hijacking the first's.

**Architecture:** Three layers bottom-up. (1) `EventStore::open` runs `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000` so readers+writers overlap and contended writers retry for 5s. (2) The `sessions` table gains three columns (`active`, `active_pid`, `active_heartbeat`); a pure `is_live` helper decides liveness; a 5s heartbeat task refreshes the timestamp and detects takeover (zero-row UPDATE); on takeover the old process cancels its in-flight turn and stops appending. (3) Boot creates a fresh session when the most-recent one is live; the resume picker shows an "in use" marker and raises a confirm card on manual takeover.

**Tech Stack:** Rust 2021, rusqlite, tokio, ratatui. Workspace tested via `cargo test --workspace`, linted via `cargo clippy --workspace --all-targets`, formatted via `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-07-06-multi-instance-safety-design.md`

---

## File Structure

**Modified files (in task order):**

- `crates/zoid-core/src/store.rs` — add WAL + `busy_timeout` PRAGMAs in `EventStore::open`; add the three-column idempotent migration; add `set_active`, `heartbeat`, and an `is_live`-input reader; extend `list_session_rows` to select the new columns. Unit tests in the same file.
- `crates/zoid-core/src/sessions.rs` — extend `SessionRow` + `SessionInfo` with `active`, `active_pid`, `active_heartbeat`; fold them through `session_list`.
- `crates/zoid-core/src/session.rs` — add `SetActive` + `Heartbeat` `Cmd` variants and actor arms; add `SessionHandle::set_active` + `heartbeat` methods; extend `list_sessions` plumbing to carry the new columns.
- `crates/zoid/src/main.rs` — boot auto-resume branch (fresh on live); claim-on-adopt; spawn the heartbeat task; clean-exit clear; the `yielded` flag + submit guard; resume-picker "in use" data + takeover confirm card; a `pid_alive` impl.
- `crates/zoid-tui/src/render.rs` — `render_sessions_overlay` shows an "in use" marker on live rows.
- `crates/zoid-tui/src/route.rs` — `route_sessions_key` raises the takeover confirm card on a live-row Enter.

**Untouched:** `secret.rs`, `config.rs`, `companion/`, `agent.rs`, `subagent.rs`, `worktree.rs`.

---

## Global Constants (from spec §2.1)

```rust
const HEARTBEAT_INTERVAL_MS: i64 = 5_000;
const LIVE_WINDOW_MS: i64 = 15_000;
```

These live in `crates/zoid-core/src/store.rs` (beside `is_live`) so the pure helper and the bin share one source of truth.

---

## Task 1: SQLite foundation — WAL + busy_timeout in `EventStore::open`

**Files:**
- Modify: `crates/zoid-core/src/store.rs:29-66` (`EventStore::open`)
- Test: `crates/zoid-core/src/store.rs` (unit tests, same file)

**Interfaces:**
- Produces: an `EventStore::open` that sets WAL + `busy_timeout`. No new public API (behavioral change only).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-core/src/store.rs`:

```rust
    #[test]
    fn open_sets_wal_journal_mode_and_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        let s = EventStore::open(p).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let timeout: i64 = s
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    #[test]
    fn wal_mode_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        {
            let _s = EventStore::open(p).unwrap();
        }
        let s2 = EventStore::open(p).unwrap();
        let mode: String = s2
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib store::tests::open_sets_wal_journal_mode_and_busy_timeout store::tests::wal_mode_persists_across_reopen`
Expected: FAIL — `journal_mode` returns `delete` (the default), `busy_timeout` returns 0.

- [ ] **Step 3: Add the PRAGMAs to `EventStore::open`**

In `crates/zoid-core/src/store.rs`, replace the start of `EventStore::open` (lines 29-32):

```rust
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // WAL allows concurrent readers + one writer without blocking (spec §1).
        // `busy_timeout` makes a contended writer retry for 5s before returning
        // SQLITE_BUSY — turns "two zoids → random turn failures" into brief stalls.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(
```

(Keep the rest of `execute_batch` unchanged. `pragma_update` is a rusqlite helper that runs `PRAGMA name = value`; for `journal_mode` it returns a row, which `pragma_update` ignores by design — WAL is persistent on the file afterward.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib store::tests::open_sets_wal_journal_mode_and_busy_timeout store::tests::wal_mode_persists_across_reopen`
Expected: PASS.

- [ ] **Step 5: Run the full crate to catch any regression**

Run: `cargo test -p zoid-core`
Expected: PASS — the new PRAGMAs don't affect any existing test (all use temp/`:memory:` DBs).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(store): WAL + busy_timeout in EventStore::open for concurrent access"
```

---

## Task 2: Stateful sessions — schema migration for `active`, `active_pid`, `active_heartbeat`

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (`EventStore::open` migration block, `list_session_rows` SELECT)
- Test: `crates/zoid-core/src/store.rs` (unit tests)

**Interfaces:**
- Consumes: `EventStore::open` from Task 1.
- Produces: three new columns on the `sessions` table, added idempotently; `list_session_rows` selects them (so `SessionRow` consumers see them after Task 3 extends the struct).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid-core/src/store.rs`:

```rust
    #[test]
    fn open_migrates_active_liveness_columns_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        // First open adds the columns.
        {
            let s = EventStore::open(p).unwrap();
            let id = Ulid::new();
            s.insert_session(id, "s", "/r", 1, 1).unwrap();
            s.set_active(id, true, 12345, 1000).unwrap();
        }
        // Re-open: the columns already exist; re-migration must NOT error.
        let s2 = EventStore::open(p).unwrap();
        let rows = s2.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].active, true);
        assert_eq!(rows[0].active_pid, Some(12345));
        assert_eq!(rows[0].active_heartbeat, Some(1000));
    }

    #[test]
    fn migrates_an_old_shape_db_without_active_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let p = path.to_str().unwrap();
        // Simulate a pre-liveness DB: sessions WITHOUT the three columns.
        {
            let conn = rusqlite::Connection::open(p).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT NOT NULL, \
                 root_path TEXT NOT NULL, created_ts INTEGER NOT NULL, last_touched_ts INTEGER NOT NULL, \
                 active_mode TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id,name,root_path,created_ts,last_touched_ts) VALUES (?1,'s','/r',1,1)",
                rusqlite::params![Ulid::new().to_string()],
            )
            .unwrap();
        }
        // Opening must add the columns (not throw) and they default to inactive.
        let s = EventStore::open(p).unwrap();
        let rows = s.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].active, false);
        assert_eq!(rows[0].active_pid, None);
        assert_eq!(rows[0].active_heartbeat, None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib store::tests::open_migrates_active_liveness_columns_idempotently store::tests::migrates_an_old_shape_db_without_active_columns`
Expected: FAIL — `set_active` doesn't exist; `SessionRow` has no `active` field; the old-shape DB open panics on the missing columns when `list_session_rows` SELECTs them.

- [ ] **Step 3: Add the three-column migration to `EventStore::open`**

In `crates/zoid-core/src/store.rs`, inside `EventStore::open`, right after the existing `active_mode` migration block (the `has_active_mode` probe + `ALTER TABLE`), add:

```rust
        // Liveness columns (multi-instance safety spec §2.2). Idempotent —
        // probe-then-ALTER, mirroring `active_mode` above (SQLite has no ADD
        // COLUMN IF NOT EXISTS).
        let has_active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'active'",
            [],
            |r| r.get(0),
        )?;
        if has_active == 0 {
            conn.execute("ALTER TABLE sessions ADD COLUMN active INTEGER NOT NULL DEFAULT 0", [])?;
            conn.execute("ALTER TABLE sessions ADD COLUMN active_pid INTEGER", [])?;
            conn.execute("ALTER TABLE sessions ADD COLUMN active_heartbeat INTEGER", [])?;
        }
```

- [ ] **Step 4: Add `set_active` to `EventStore`**

In `crates/zoid-core/src/store.rs`, add this method to `impl EventStore` (after `set_active_mode`):

```rust
    /// Claim or clear a session's "active interface" liveness flag (spec §2.2).
    /// `active=true` with `active_pid`/`active_heartbeat` claims the row for
    /// this process; `active=false` (with NULL pid/heartbeat) releases it.
    /// Overwrites any prior claim — this is also the reclaim path (a stale
    /// flag from a crashed process) and the takeover path (another process
    /// overwrites the row, which the old process detects via `heartbeat`).
    pub fn set_active(
        &self,
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<()> {
        if active {
            self.conn.execute(
                "UPDATE sessions SET active = 1, active_pid = ?2, active_heartbeat = ?3 WHERE id = ?1",
                params![id.to_string(), active_pid, active_heartbeat],
            )?;
        } else {
            self.conn.execute(
                "UPDATE sessions SET active = 0, active_pid = NULL, active_heartbeat = NULL WHERE id = ?1",
                params![id.to_string()],
            )?;
        }
        Ok(())
    }
```

- [ ] **Step 5: Extend `SessionRow` with the liveness columns**

In `crates/zoid-core/src/sessions.rs`, add three fields to `SessionRow` (after `last_touched_ts`):

```rust
    pub active: bool,
    pub active_pid: Option<i64>,
    pub active_heartbeat: Option<i64>,
```

- [ ] **Step 6: Extend `list_session_rows` to SELECT the new columns**

In `crates/zoid-core/src/store.rs`, replace `list_session_rows` (the SELECT + the `query_map` closure):

```rust
    pub fn list_session_rows(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, root_path, created_ts, last_touched_ts, active, active_pid, active_heartbeat FROM sessions ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, root_path, created_ts, last_touched_ts, active, active_pid, active_heartbeat) = r?;
            out.push(SessionRow {
                id: id.parse()?,
                name,
                root_path,
                created_ts,
                last_touched_ts,
                active,
                active_pid,
                active_heartbeat,
            });
        }
        Ok(out)
    }
```

- [ ] **Step 7: Fix the existing `list_session_rows`/`SessionRow` construction sites**

The existing `sessions_crud_round_trips` and `migration_is_idempotent_across_reopen` tests construct `SessionRow` literals without the new fields. Update them to add `active: false, active_pid: None, active_heartbeat: None` (the default for any row freshly read from the DB). In `sessions.rs`'s `session_list` test helper `row(...)`, add the same defaults. In any other `SessionRow { ... }` literal in the crate, add the three fields. Search:

Run: `grep -rn "SessionRow {" crates/ | grep -v "test\|plan\|spec"`
Expected: a handful of literal construction sites (the `sessions.rs` tests, the `session.rs` test); add the fields to each.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib store::tests::open_migrates_active_liveness_columns_idempotently store::tests::migrates_an_old_shape_db_without_active_columns`
Expected: PASS.

Run: `cargo test -p zoid-core`
Expected: PASS — every `SessionRow` literal now carries the new fields.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/store.rs crates/zoid-core/src/sessions.rs
git commit -m "feat(store): active/active_pid/active_heartbeat columns + set_active"
```

---

## Task 3: The pure `is_live` helper

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add `is_live` + constants)
- Test: `crates/zoid-core/src/store.rs` (unit tests)

**Interfaces:**
- Produces: `pub fn is_live(...)` and the two constants, in `store.rs`. Consumed by the bin (boot, picker) and the renderer's marker data.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-core/src/store.rs`:

```rust
    #[test]
    fn is_live_requires_flag_pid_and_fresh_heartbeat() {
        let alive = |_: i64| true; // pretend every PID is alive
        // flag set, live PID, fresh heartbeat ⇒ live
        assert!(is_live(true, Some(99), Some(1000), 2000, alive));
    }

    #[test]
    fn is_live_false_for_stale_heartbeat() {
        let alive = |_: i64| true;
        // heartbeat 20s ago, window is 15s ⇒ stale ⇒ not live
        assert!(!is_live(true, Some(99), Some(1000), 21000, alive));
    }

    #[test]
    fn is_live_false_for_dead_pid() {
        let alive = |pid: i64| pid != 99; // 99 is dead
        assert!(!is_live(true, Some(99), Some(1000), 2000, alive));
    }

    #[test]
    fn is_live_false_when_flag_cleared() {
        let alive = |_: i64| true;
        assert!(!is_live(false, Some(99), Some(1000), 2000, alive));
    }

    #[test]
    fn is_live_false_for_null_pid_or_heartbeat() {
        let alive = |_: i64| true;
        assert!(!is_live(true, None, Some(1000), 2000, alive));
        assert!(!is_live(true, Some(99), None, 2000, alive));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib store::tests::is_live`
Expected: FAIL — `is_live` not defined.

- [ ] **Step 3: Add the constants and `is_live`**

Add near the top of `crates/zoid-core/src/store.rs` (after the `use` lines, before `EventStore`):

```rust
/// How often a live process refreshes its `active_heartbeat` (ms). Spec §2.1.
pub const HEARTBEAT_INTERVAL_MS: i64 = 5_000;
/// A session is "live" only if its heartbeat is within this window (ms). 3× the
/// interval: a single missed heartbeat (GC pause, system suspend) does NOT mark
/// a live session stale. Spec §2.1.
pub const LIVE_WINDOW_MS: i64 = 15_000;

/// Is the session described by these columns currently held by a live
/// interface? Pure except for the injected `pid_alive` OS check (kept injectable
/// for testing). A session is live iff its flag is set, its PID is still alive,
/// and its heartbeat is within `LIVE_WINDOW_MS` of `now_ms`. Spec §2.2.
pub fn is_live(
    active: bool,
    active_pid: Option<i64>,
    active_heartbeat: Option<i64>,
    now_ms: i64,
    pid_alive: impl Fn(i64) -> bool,
) -> bool {
    active
        && match (active_pid, active_heartbeat) {
            (Some(pid), Some(hb)) => pid_alive(pid) && now_ms - hb < LIVE_WINDOW_MS,
            _ => false,
        }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib store::tests::is_live`
Expected: PASS — all five `is_live_*` tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(store): pure is_live liveness helper + constants"
```

---

## Task 4: `heartbeat` store method + `SessionHandle` plumbing

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add `heartbeat`)
- Modify: `crates/zoid-core/src/session.rs` (add `Heartbeat` + `SetActive` `Cmd` variants, actor arms, methods)
- Test: `crates/zoid-core/src/store.rs`, `crates/zoid-core/src/session.rs`

**Interfaces:**
- Consumes: `set_active` (Task 2).
- Produces: `EventStore::heartbeat(id, pid, now) -> bool` (returns `false` when zero rows matched = takeover detected); `SessionHandle::set_active(...)` and `SessionHandle::heartbeat(...)` async methods.

- [ ] **Step 1: Write the failing store test**

Add to `crates/zoid-core/src/store.rs` tests:

```rust
    #[test]
    fn heartbeat_refreshes_when_still_owner_and_returns_true() {
        let s = EventStore::open(":memory:").unwrap();
        let id = Ulid::new();
        s.insert_session(id, "s", "/r", 1, 1).unwrap();
        s.set_active(id, true, 1234, 1000).unwrap();
        // Same PID refreshes → true (still the owner).
        assert!(s.heartbeat(id, 1234, 2000).unwrap());
        let row = s.list_session_rows().unwrap().pop().unwrap();
        assert_eq!(row.active_heartbeat, Some(2000));
    }

    #[test]
    fn heartbeat_returns_false_when_taken_over() {
        let s = EventStore::open(":memory:").unwrap();
        let id = Ulid::new();
        s.insert_session(id, "s", "/r", 1, 1).unwrap();
        s.set_active(id, true, 1234, 1000).unwrap();
        // Another process takes over (overwrites active_pid).
        s.set_active(id, true, 5678, 1500).unwrap();
        // The old process's heartbeat (pid 1234) matches zero rows → false.
        assert!(!s.heartbeat(id, 1234, 2000).unwrap());
    }

    #[test]
    fn heartbeat_returns_false_for_unknown_session() {
        let s = EventStore::open(":memory:").unwrap();
        assert!(!s.heartbeat(Ulid::new(), 1234, 1000).unwrap());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib store::tests::heartbeat`
Expected: FAIL — `heartbeat` not defined.

- [ ] **Step 3: Add `heartbeat` to `EventStore`**

In `crates/zoid-core/src/store.rs`, add to `impl EventStore` (after `set_active`):

```rust
    /// Refresh `active_heartbeat` for `id` ONLY if `active_pid` still matches
    /// (i.e. this process is still the owner). Returns `true` when the row was
    /// updated (still owner), `false` when zero rows matched (another process
    /// took over, or the session row is gone). The `false` return is the
    /// takeover-detection signal the bin uses to yield. Spec §2.3.
    pub fn heartbeat(&self, id: Ulid, active_pid: i64, active_heartbeat: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE sessions SET active_heartbeat = ?3 WHERE id = ?1 AND active_pid = ?2",
            params![id.to_string(), active_pid, active_heartbeat],
        )?;
        Ok(n == 1)
    }
```

- [ ] **Step 4: Run the store tests to verify they pass**

Run: `cargo test -p zoid-core --lib store::tests::heartbeat`
Expected: PASS.

- [ ] **Step 5: Write the failing session-actor test**

Add to `crates/zoid-core/src/session.rs` tests:

```rust
    #[tokio::test]
    async fn set_active_and_heartbeat_round_trip_via_actor() {
        let h = SessionHandle::spawn(":memory:").unwrap();
        let id = Ulid::from(7u128);
        h.new_session(id, "s".into(), "/r".into(), 0).await.unwrap();
        h.set_active(id, true, 1234, 1000).await.unwrap();
        // Heartbeat as the owner refreshes the timestamp.
        assert!(h.heartbeat(id, 1234, 2000).await.unwrap());
        // A takeover (overwrite pid) makes the old owner's heartbeat return false.
        h.set_active(id, true, 5678, 1500).await.unwrap();
        assert!(!h.heartbeat(id, 1234, 2500).await.unwrap());
    }
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `cargo test -p zoid-core --lib session::tests::set_active_and_heartbeat_round_trip_via_actor`
Expected: FAIL — `set_active`/`heartbeat` methods absent on `SessionHandle`.

- [ ] **Step 7: Add the `Cmd` variants, actor arms, and `SessionHandle` methods**

In `crates/zoid-core/src/session.rs`:

Add to the `Cmd` enum (after `SetActiveMode`):

```rust
    SetActive {
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    Heartbeat {
        id: Ulid,
        active_pid: i64,
        active_heartbeat: i64,
        reply: oneshot::Sender<Result<bool>>,
    },
```

Add to the actor `match` in `SessionHandle::spawn` (after the `SetActiveMode` arm):

```rust
                    Cmd::SetActive { id, active, active_pid, active_heartbeat, reply } => {
                        let _ = reply.send(store.set_active(id, active, active_pid, active_heartbeat));
                    }
                    Cmd::Heartbeat { id, active_pid, active_heartbeat, reply } => {
                        let _ = reply.send(store.heartbeat(id, active_pid, active_heartbeat));
                    }
```

Add the public methods to `impl SessionHandle` (after `set_active_mode`):

```rust
    /// Claim or release the active-interface flag for a session (spec §2.2).
    pub async fn set_active(
        &self,
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::SetActive {
                id,
                active,
                active_pid,
                active_heartbeat,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Refresh the heartbeat for the current session. Returns `false` when
    /// another process has taken over (the yield signal). Spec §2.3.
    pub async fn heartbeat(
        &self,
        id: Ulid,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Heartbeat {
                id,
                active_pid,
                active_heartbeat,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
```

- [ ] **Step 8: Run the session test to verify it passes**

Run: `cargo test -p zoid-core --lib session::tests::set_active_and_heartbeat_round_trip_via_actor`
Expected: PASS.

- [ ] **Step 9: Run the full crate**

Run: `cargo test -p zoid-core`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-core/src/store.rs crates/zoid-core/src/session.rs
git commit -m "feat(session): heartbeat + set_active actor methods"
```

---

## Task 5: Carry liveness columns through `SessionInfo` and `list_sessions`

**Files:**
- Modify: `crates/zoid-core/src/sessions.rs` (`SessionInfo`, `session_list`)
- Modify: `crates/zoid-core/src/session.rs` (no change — `list_sessions` already folds via `session_list`)
- Test: `crates/zoid-core/src/sessions.rs`

**Interfaces:**
- Produces: `SessionInfo { active, active_pid, active_heartbeat }` so the bin and the renderer can call `is_live` per row.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-core/src/sessions.rs` tests:

```rust
    #[test]
    fn session_list_carries_liveness_columns() {
        let mut r = row(1, "a", "/repo", 100);
        r.active = true;
        r.active_pid = Some(42);
        r.active_heartbeat = Some(1000);
        let rows = vec![r];
        let list = session_list(&rows, &[], None);
        assert!(list[0].active);
        assert_eq!(list[0].active_pid, Some(42));
        assert_eq!(list[0].active_heartbeat, Some(1000));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-core --lib sessions::tests::session_list_carries_liveness_columns`
Expected: FAIL — `SessionInfo` has no `active` field.

- [ ] **Step 3: Extend `SessionInfo` and `session_list`**

In `crates/zoid-core/src/sessions.rs`, add the three fields to `SessionInfo` (after `last_touched_ts`):

```rust
    pub active: bool,
    pub active_pid: Option<i64>,
    pub active_heartbeat: Option<i64>,
```

In `session_list`, the `map(|r| SessionInfo { ... })` closure must pass them through. Replace the `SessionInfo { ... }` construction:

```rust
        .map(|r| SessionInfo {
            id: r.id,
            name: r.name.clone(),
            root_path: r.root_path.clone(),
            created_ts: r.created_ts,
            last_touched_ts: r.last_touched_ts,
            token_total: totals.get(&r.id).copied().unwrap_or(0),
            active: r.active,
            active_pid: r.active_pid,
            active_heartbeat: r.active_heartbeat,
        })
```

- [ ] **Step 4: Fix any other `SessionInfo { ... }` literal construction sites**

Run: `grep -rn "SessionInfo {" crates/ | grep -v "test\|plan\|spec"`
Expected: test helpers in `sessions.rs` and possibly `session.rs`; add `active: false, active_pid: None, active_heartbeat: None` to each literal.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib sessions`
Expected: PASS.

Run: `cargo test -p zoid-core`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/sessions.rs
git commit -m "feat(sessions): carry liveness columns through SessionInfo"
```

---

## Task 6: Boot auto-resume — fresh session when the most-recent is live

**Files:**
- Modify: `crates/zoid/src/main.rs:1218-1248` (the boot auto-resume block)
- Modify: `crates/zoid/src/main.rs` (add `pid_alive` helper, `spawn_heartbeat` helper, a `yielded` field on `App`, clean-exit clear)
- Test: `crates/zoid/src/main.rs` (integration tests using a test-double `pid_alive`)

**Interfaces:**
- Consumes: `SessionHandle::set_active`, `heartbeat`, `is_live` (Tasks 3-4).
- Produces: a boot path that creates a fresh session when the most-recent is live; a heartbeat task; a `yielded` flag guarding `Submit`.

This is the largest task. It is split into steps that each leave the workspace compiling.

- [ ] **Step 1: Add the `pid_alive` helper**

In `crates/zoid/src/main.rs`, add (near `now_ms`). Use `nix` (a thin, idiomatic wrapper over `kill(2)`) rather than raw `libc`, because `libc` does **not** export `__errno_location` on the primary `linux/gnu` target — `nix` returns a typed `Errno` instead, which is portable and testable.

Add `nix` to the unix target deps in `crates/zoid/Cargo.toml`:

```toml
[target.'cfg(unix)'.dependencies]
flate2 = "1"
tar = "0.4"
nix = { version = "0.29", default-features = false, features = ["process"] }
```

Then in `crates/zoid/src/main.rs`:

```rust
/// Whether the given OS PID is currently alive. `kill(pid, 0)` succeeds when the
/// process exists, returns `ESRCH` when it's dead, and `EPERM` when it exists but
/// isn't ours (treated as alive — we can't prove it's dead, and a stale-but-alive
/// row is reclaimable via the heartbeat window anyway). Injected into `is_live`
/// so callers can substitute a test double. Spec §2.2.
#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    match nix::unistd::Pid::from_raw(pid as i32).kill(None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => true, // unknown failure → lean on the heartbeat window
    }
}

#[cfg(not(unix))]
fn pid_alive(_pid: i64) -> bool {
    true // non-Unix: no portable check; lean on the heartbeat window.
}
```

Run: `cargo build -p zoid`
Expected: PASS — `nix` builds and `pid_alive` compiles on Unix; the non-Unix stub compiles elsewhere.

- [ ] **Step 2: Add `yielded` to `App` and a `pid_alive` test double hook**

In the `App` struct, add a field:

```rust
    /// Set when this process's session was taken over by another instance; the
    /// in-flight turn was cancelled and no further turns may start against it.
    /// The user can `:new` or `:resume` elsewhere, or quit. Spec §2.4.
    yielded: bool,
```

Initialize it `yielded: false,` in the `App { ... }` literal in `main`.

- [ ] **Step 3: Add `spawn_heartbeat`**

In `crates/zoid/src/main.rs`, add a helper (near `spawn_turn`):

```rust
/// Spawn the 5s heartbeat task for the active session. Each tick refreshes
/// `active_heartbeat`; if the UPDATE matches zero rows (another process took
/// over the row), fire the turn cancellation token, set `yielded`, stop the
/// task, and surface a hint. Spec §2.3/§2.4.
fn spawn_heartbeat(app: &App) {
    let session = app.session.clone();
    let session_id = app.session_id;
    let pid = std::process::id() as i64;
    let ui_tx = app.ui_tx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(
            zoid_core::store::HEARTBEAT_INTERVAL_MS as u64,
        ));
        tick.tick().await; // consume the immediate first tick
        loop {
            tick.tick().await;
            let now = now_ms();
            match session.heartbeat(session_id, pid, now).await {
                Ok(true) => { /* still owner */ }
                Ok(false) | Err(_) => {
                    // Taken over (or the actor stopped). Signal yield.
                    let _ = ui_tx
                        .send(AgentUpdate::SessionTakenOver)
                        .await;
                    break;
                }
            }
        }
    });
}
```

Add a new `AgentUpdate` variant in `crates/zoid/src/agent.rs`'s `AgentUpdate` enum:

```rust
    /// The current session was taken over by another instance (the heartbeat
    /// detected another process claimed the row). The bin cancels the in-flight
    /// turn, sets `yielded`, and surfaces a hint. Spec §2.4.
    SessionTakenOver,
```

- [ ] **Step 4: Handle `SessionTakenOver` in the run loop**

In `crates/zoid/src/main.rs`'s `run`, in the `match update { ... }` block, add an arm (after `AgentUpdate::TurnComplete`):

```rust
                    AgentUpdate::SessionTakenOver => {
                        // Fire the turn cancel if a turn is in flight (reuses the
                        // Esc/Ctrl-C path). Stop streaming, mark yielded.
                        if let Some(cancel) = &app.turn_cancel {
                            cancel.cancel();
                        }
                        app.streaming = false;
                        app.delegating = false;
                        app.yielded = true;
                        app.shell.status_hint =
                            Some("session taken over by another instance".into());
                    }
```

- [ ] **Step 5: Guard `Submit` against `yielded`**

In `handle_action`'s `Action::Submit` arm, change the opening guard:

```rust
        Action::Submit => {
            if app.streaming || app.delegating || app.yielded {
                if app.yielded {
                    app.shell.status_hint =
                        Some("session taken over — :new or :resume".into());
                }
                return Ok(false);
            }
```

- [ ] **Step 6: Rewrite the boot auto-resume block**

In `crates/zoid/src/main.rs` (the `// Auto-resume the most-recently-touched session` block, currently lines ~1218-1248), replace it with:

```rust
    // Auto-resume the most-recently-touched session for this repo, else create
    // one. If the most-recent session is live (another interface holds it),
    // create a FRESH session instead of colliding. Spec §3.1.
    let sessions = session
        .list_sessions(Some(root.clone()))
        .await
        .unwrap_or_default();
    let first_time_user = sessions.is_empty();
    let self_pid = std::process::id() as i64;
    let boot_ts = now_ms();
    let (session_id, session_name, session_started_ms) = if first_time_user {
        // No sessions for this repo yet → create one.
        let id = Ulid::new();
        let name = derive_session_name(None, boot_ts, tz_offset_secs);
        session
            .new_session(id, name.clone(), root.clone(), boot_ts)
            .await?;
        (id, name, boot_ts)
    } else {
        let s = &sessions[0];
        let live = zoid_core::store::is_live(
            s.active,
            s.active_pid,
            s.active_heartbeat,
            boot_ts,
            pid_alive,
        );
        if live {
            // Another instance is on it → create a fresh session, leave it alone.
            let id = Ulid::new();
            let name = derive_session_name(None, boot_ts, tz_offset_secs);
            session
                .new_session(id, name.clone(), root.clone(), boot_ts)
                .await?;
            (id, name, boot_ts)
        } else {
            // Reclaim it: load + touch + claim.
            session.touch_session(s.id, boot_ts).await.ok();
            (s.id, s.name.clone(), s.created_ts)
        }
    };
    // Claim the session (whether fresh or reclaimed) and start the heartbeat.
    session
        .set_active(session_id, true, self_pid, boot_ts)
        .await
        .ok();
```

**Important:** the original code had `let first_time_user = sessions.is_empty();` (line 1223) and `let boot_ts = now_ms();` (line 1203) declared **earlier** in `main`, before the auto-resume block. The new block reuses the existing `boot_ts` (do NOT re-declare it) and replaces the existing `let first_time_user = sessions.is_empty();` line. Concretely: delete the old lines 1218–1248 (the `let sessions = ...` block through the `(id, name, boot_ts)` tuple), and delete the standalone `let first_time_user = sessions.is_empty();` at 1223 — the new block below declares `first_time_user` itself. Keep `let boot_ts = now_ms();` at 1203 (it's already in scope). `tz_offset_secs` is already in scope. `pid_alive` is the helper from Step 1.

- [ ] **Step 7: Start the heartbeat after `app` is constructed**

In `main`, right after `restore_mode_for_session(&mut app).await;` (before `if companion_at_boot { ... }`), add:

```rust
    spawn_heartbeat(&app);
```

- [ ] **Step 8: Clear the active flag on clean exit**

In `main`, the `result` is computed by `run(...)`. After the terminal-restore block, before `result` is returned, add a best-effort clear:

```rust
    // Release the session's active flag on clean exit (best-effort). If the
    // process is force-killed the flag stays stale and the next evaluator
    // reclaims it via is_live == false. Spec §2.3.
    let _ = app.session.set_active(app.session_id, false, 0, 0).await;
    result
```

(Replace the final `result` in `main` with this — the `let result = run(...).await;` stays; this clear sits right before the `result` is the function's return value.)

- [ ] **Step 9: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS. (If `libc` was added, it builds. If a `SessionInfo` literal in `main.rs`'s tests needs the new fields, fix it per Task 5 Step 4's grep.)

- [ ] **Step 10: Add an integration test for the boot branch**

Add to `crates/zoid/src/main.rs` tests (this test exercises the pure decision via `is_live` directly, since the full `main` is not testable headlessly):

```rust
    #[tokio::test]
    async fn boot_reclaims_stale_session_and_uses_fresh_when_live() {
        // The boot decision is `is_live(...)`. A stale-heartbeat row is reclaimable
        // (is_live == false); a fresh-heartbeat row is not (is_live == true).
        use zoid_core::store::is_live;
        let alive = |_: i64| true;
        // Stale: heartbeat 20s ago, window 15s → not live → reclaim.
        assert!(!is_live(true, Some(99), Some(1000), 21000, alive));
        // Live: heartbeat now → live → create a fresh session instead.
        assert!(is_live(true, Some(99), Some(1000), 1000, alive));
    }
```

- [ ] **Step 11: Run the tests**

Run: `cargo test -p zoid --bin zoid boot_reclaims_stale_session`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid/src/agent.rs crates/zoid/Cargo.toml
git commit -m "feat(zoid): boot fresh-on-live + heartbeat + yield protocol"
```

---

## Task 7: Resume picker — "in use" marker + takeover confirm card

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:869-878` (`render_sessions_overlay`)
- Modify: `crates/zoid-tui/src/route.rs:258-267` (`route_sessions_key`)
- Modify: `crates/zoid/src/main.rs` (`Command::ResumeSessionPicker` picker population, `Action::SessionPick` takeover path)
- Test: `crates/zoid-tui/src/render.rs`, `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `is_live`, `SessionInfo` liveness columns (Tasks 3, 5), the `QuestionKind::Ask` confirm-card path (existing).
- Produces: an "in use" marker on live picker rows; a confirm card on selecting a live row; takeover (claim) on confirm.

- [ ] **Step 1: Add live-row data to the picker**

The picker rows are `Vec<String>` in `shell.sessions`, index-aligned with `session_ids`. The renderer needs a per-row "live" boolean. Add a parallel vec to `ShellState`:

In `crates/zoid-tui/src/state.rs`, add a field to `ShellState` (near `sessions`):

```rust
    /// Per-row "in use" flags for the resume-session picker, index-aligned with
    /// `sessions`/`session_ids`. `true` when `is_live` for that row at the time
    /// the picker was populated. Spec §3.2.
    pub sessions_live: Vec<bool>,
```

Initialize `sessions_live: Vec::new(),` in `ShellState::new` and clear it in `close_overlay` (add `self.sessions_live.clear();`).

- [ ] **Step 2: Populate `sessions_live` when the picker opens**

In `crates/zoid/src/main.rs`, `Command::ResumeSessionPicker` arm, after building `app.session_ids` and `app.shell.sessions`, add:

```rust
            let self_pid = std::process::id() as i64;
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
```

- [ ] **Step 3: Render the "in use" marker**

In `crates/zoid-tui/src/render.rs`, `render_sessions_overlay`, replace the row construction:

```rust
fn render_sessions_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    let rows = if state.sessions.is_empty() {
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

- [ ] **Step 4: Write the failing render test**

Add to `crates/zoid-tui/src/render.rs` tests:

```rust
    #[test]
    fn sessions_overlay_marks_live_rows() {
        use crate::state::{Overlay, ShellState};
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a  ·  1m ago  ·  10".into(), "b  ·  2m ago  ·  20".into()];
        s.sessions_live = vec![false, true];
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_sessions_overlay(f, &s, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            content.contains("in use"),
            "live row must carry the 'in use' marker: {content:?}"
        );
    }
```

- [ ] **Step 5: Run the render test to verify it passes**

Run: `cargo test -p zoid-tui --lib render::tests::sessions_overlay_marks_live_rows`
Expected: PASS.

- [ ] **Step 6: Raise the confirm card on a live-row Enter**

In `crates/zoid-tui/src/route.rs`, `route_sessions_key`, intercept Enter on a live row:

```rust
fn route_sessions_key(state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => {
            // If the highlighted row is live, raise the takeover confirm card
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
        KeyCode::Up | KeyCode::Char('k') => Action::SessionMove(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::SessionMove(1),
        _ => Action::Noop,
    }
}
```

Add the new `Action::SessionTakeoverConfirm` variant to the `Action` enum in `route.rs` (after `SessionPick`).

- [ ] **Step 7: Handle `SessionTakeoverConfirm` in the bin**

In `crates/zoid/src/main.rs`'s `handle_action`, add an arm (after `Action::SessionPick`):

```rust
        Action::SessionTakeoverConfirm => {
            // The user picked a live row. Raise a confirm card; on "Take over",
            // overwrite the row's active_pid/heartbeat to claim it, then resume.
            let sid = match app.session_ids.get(app.shell.session_selected) {
                Some(&sid) => sid,
                None => {
                    app.shell.close_overlay();
                    return Ok(false);
                }
            };
            app.shell.question = Some(zoid_tui::question::QuestionState::new(
                "This session is active in another instance. Take it over? \
                 The other instance will detect this and yield."
                    .into(),
                vec!["Take over".into(), "Cancel".into()],
            ));
            // Stash the takeover target so QuestionSelect can act on it.
            app.pending_takeover = Some(sid);
        }
```

Add a field to `App`:

```rust
    /// A takeover confirmation in flight: the session id the user is about to
    /// forcibly claim from another instance. Set by `SessionTakeoverConfirm`,
    /// consumed by the "Take over" answer in `QuestionSelect`. Spec §3.2.
    pending_takeover: Option<Ulid>,
```

Initialize `pending_takeover: None,` in `main`.

- [ ] **Step 8: Consume the takeover answer in `QuestionSelect`**

In `handle_action`'s `Action::QuestionSelect` arm, before the existing match, intercept a pending takeover. Note: `QuestionState::resolved()` in `Pick` mode resolves the *last* choice row to `LetYouDecide` (not `Choice`), so a two-choice `["Take over", "Cancel"]` card resolves `Cancel` to `LetYouDecide`. The check below treats anything that isn't `Choice("Take over")` as a cancel — that's the intended contract; document it inline rather than relying on row indices:

```rust
        Action::QuestionSelect => {
            use zoid_tui::question::{QuestionMode, QuestionOutcome};
            // A takeover confirm card's answer. NB: QuestionState resolves the
            // last choice row to LetYouDecide, so `Cancel` arrives as
            // LetYouDecide, not Choice("Cancel"). Anything that isn't
            // Choice("Take over") is treated as a cancel — the intended contract.
            if let Some(sid) = app.pending_takeover.take() {
                let outcome = app.shell.question.as_ref().map(|q| q.resolved());
                let take = matches!(outcome, Some(QuestionOutcome::Choice(s)) if s == "Take over");
                app.shell.question = None;
                app.shell.overlay = zoid_tui::state::Overlay::None;
                if !take {
                    // Cancel: return to the picker.
                    app.shell.overlay = zoid_tui::Overlay::Sessions;
                    return Ok(false);
                }
                // Take over: claim the row, then load it (reuse the SessionPick
                // load path). The old process detects the overwrite on its next
                // heartbeat and yields.
                let self_pid = std::process::id() as i64;
                app.session.set_active(sid, true, self_pid, now_ms()).await.ok();
                // Fall through to the normal session-load by setting up as if
                // SessionPick had fired for `sid`.
                app.shell.session_selected = app
                    .session_ids
                    .iter()
                    .position(|&x| x == sid)
                    .unwrap_or(app.shell.session_selected);
                // Delegate to the existing SessionPick load path.
                return handle_action(app, zoid_tui::route::Action::SessionPick).await;
            }
            // ... existing QuestionSelect logic unchanged ...
```

(The existing `QuestionSelect` body follows after this insertion; the takeover branch `return`s, so it doesn't fall into the wizard/ask logic.)

- [ ] **Step 9: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS. (Add `sessions_live` to any `ShellState` literal in tests that assert equality — `ShellState::new()` is fine; only explicit struct literals need the field, and tests use `ShellState::new()`.)

- [ ] **Step 10: Write a route test for the live-row intercept**

Add to `crates/zoid-tui/src/route.rs` tests:

```rust
    #[test]
    fn sessions_live_row_enter_raises_confirm_not_pick() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into(), "b".into()];
        s.sessions_live = vec![false, true];
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Non-live row → direct pick.
        s.session_selected = 0;
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::SessionPick);
        // Live row → confirm card.
        s.session_selected = 1;
        assert_eq!(
            route_key(&s, k(KeyCode::Enter)),
            Action::SessionTakeoverConfirm
        );
    }
```

- [ ] **Step 11: Run the tests**

Run: `cargo test -p zoid-tui --lib route::tests::sessions_live_row_enter_raises_confirm_not_pick`
Run: `cargo test -p zoid-tui --lib render::tests::sessions_overlay_marks_live_rows`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/state.rs crates/zoid/src/main.rs
git commit -m "feat(sessions): 'in use' marker + takeover confirm card"
```

---

## Task 8: Snapshot regeneration + clippy/fmt

**Files:** none new (verify-only)

- [ ] **Step 1: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. Fix any unused imports (e.g. `libc` on non-Unix) or dead-code warnings (`pid_alive` on non-Unix is `#[allow(dead_code)]` if needed).

- [ ] **Step 2: Run fmt**

Run: `cargo fmt --all`
Expected: no changes (or apply them).

- [ ] **Step 3: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS — all unit, integration, and snapshot tests green. (No snapshot should drift: `render_sessions_overlay` is not snapshot-tested; the `state.rs` changes don't affect any existing snapshot's `ShellState` construction since tests use `ShellState::new()`.)

- [ ] **Step 4: Commit any fmt/clippy fixes**

```bash
git add -A
git commit -m "chore: clippy + fmt for multi-instance safety"
```

---

## Self-Review

**1. Spec coverage:**
- §1 SQLite foundation (WAL + busy_timeout) → Task 1.
- §2.1 constants → Task 3.
- §2.2 schema migration + `is_live` → Tasks 2, 3.
- §2.3 heartbeat (claim, heartbeat, zero-rows signal) → Tasks 2, 4, 6.
- §2.4 yield protocol (cancel turn, stop appending, hint, `yielded` guard) → Task 6 (Steps 4, 5).
- §3.1 boot auto-resume (fresh on live) → Task 6 (Step 6).
- §3.2 resume picker (in-use marker, confirm card, takeover, never greyed) → Task 7.
- §Testing (unit `is_live`, migration idempotence, concurrency, lifecycle, boot, picker) → Tasks 1-7 tests.

**2. Placeholder scan:** No TBD/TODO. Every step shows the exact code or command. The `pid_alive` non-Unix branch is a real (deliberate) fallback, not a placeholder.

**3. Type consistency:**
- `is_live(active: bool, active_pid: Option<i64>, active_heartbeat: Option<i64>, now_ms: i64, pid_alive: impl Fn(i64)->bool)` — consistent across Task 3 (definition), Task 6 (boot), Task 7 (picker).
- `set_active(id, active, active_pid, active_heartbeat)` — consistent across Task 2 (store), Task 4 (session actor), Task 6 (boot claim), Task 7 (takeover).
- `heartbeat(id, active_pid, active_heartbeat) -> bool` — consistent across Task 4 (store + session).
- `SessionTakeoverConfirm` — consistent across Task 7 (route, bin arm).
- `sessions_live: Vec<bool>` — consistent across Task 7 (state, render, route, bin).

**4. Task ordering / compile-ability:** Task 1 (PRAGMAs) compiles standalone. Task 2 adds columns + `SessionRow` fields (every literal must be fixed in Step 7 or the crate won't compile — called out). Task 3 (`is_live`) compiles standalone. Task 4 adds the actor methods. Task 5 extends `SessionInfo` (literals fixed in Step 4). Task 6 touches the bin (depends on Tasks 3-4). Task 7 depends on Task 5's `SessionInfo` columns. Task 8 is verify-only. No mid-task compile dead-ends.