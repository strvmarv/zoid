# Unified Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add always-on logging to a single `logs` table in the zoid db, capturing both turn-scoped and system `warn`/`error` events, with a 72h TTL purge at boot.

**Architecture:** A `logs` table in `EventStore` (zoid-core) written via the `SessionHandle` actor (single-writer). The `ObsLayer` in `obs.rs` (zoid bin) captures `warn`/`error` events into a bounded ring buffer on `ObsState` (always on, no db needed at subscriber-install time). After the actor starts, the ring buffer flushes to the `logs` table; subsequent system logs go directly through the actor. A 72h TTL purge runs at boot (non-fatal, same pattern as `seed_local_models`).

**Tech Stack:** Rust, `rusqlite` (zoid-core dep), `tracing`/`tracing-subscriber` (zoid bin dep), `serde_json` for the `fields` column.

---

## Handoff Context

**Companion spec:** `docs/superpowers/specs/2026-08-04-unified-logging-design.md` (commit `6fcf859`). Read it for the full schema, the `FieldGrab` extension rationale, the pre-actor gap analysis, and the chatty-warn flooding risk.

**Status:** planned, not started. No code written.

### What already exists — no new plumbing needed

- `crates/zoid-core/src/session.rs:108` — `Cmd::SeedLocalModels` variant. `WriteLog` and `PurgeLogs` follow the exact same pattern: add a `Cmd` variant, handle it in the actor's `blocking_recv` loop, expose an async method on `SessionHandle`.
- `crates/zoid-core/src/store.rs:154` — `seed_local_models` method on `EventStore`. `write_log` and `purge_logs` follow the same shape (execute SQL on `self.conn`).
- `crates/zoid/src/obs.rs:280-318` — `ObsLayer::on_event` already captures `WARN`/`ERROR` into the existing `errors: VecDeque<ErrEntry>` (capped at 20). The `logs` ring buffer (500 entries) uses the same capture path with one addition: the `FieldGrab` `Visit` collects unknown fields into a `serde_json::Map` for the `fields` column.
- `crates/zoid/src/obs.rs:239-274` — `FieldGrab`'s `Visit` impl. Each `record_*` method has a `_ => {}` fallback that discards unknown fields. The new `fields` capture replaces these with `map.insert(field.name(), value)`.
- `crates/zoid/src/main.rs:2333` — the `seed_local_models` boot call. `purge_logs` and the ring-buffer flush go right after it, same `if let Err(e) ... tracing::warn!` pattern.

### Two design decisions that are not obvious from the tasks

**1. The `logs` ring buffer and the `errors` ring coexist.** `errors` (capped at 20) powers the Overview page's error widget and is read on every TUI frame. `logs` (capped at 500) feeds the db flush and the future debug view. They serve different readers; the cost of double-capturing `warn`/`error` in `on_event` is trivial. Don't replace `errors` — add `logs` alongside it.

**2. The `Cmd::WriteLog` carries a `LogRow`, not a `LogEntry`.** The ring buffer stores `LogEntry` (reduced — no `scope`/`session_id`/`event_id`). The `Cmd::WriteLog` command carries a `LogRow` (full row — all columns). Turn-scoped writes build a `LogRow` directly. The flush maps each `LogEntry` → `LogRow` with `scope = "system"`, nulls for the ids. This separation exists because the ring buffer doesn't know the scope (it captures pre-actor, when everything is system-scoped), but turn-scoped writes (which go directly to the actor) do. Note: this deliberately diverges from the spec §5 flush pseudocode's 4-arg `write_log(entry, "system", None, None)` signature in favor of the spec's authoritative "Cmd::WriteLog payload" subsection (§1) which specifies `LogRow` as the payload.

---

## Global Constraints

- **`zoid-model` must stay dependency-free.** No logging code goes in `zoid-model`. All logging lives in `zoid-core` (db layer) and the `zoid` bin (subscriber/`ObsLayer`).
- **The single-writer invariant is the actor's `blocking_recv` loop.** Both `write_log` and `purge_logs` go through `Cmd` variants. No direct `Connection` access from outside the actor thread.
- **`info`-level events are not captured.** Only `warn`/`error` go to the `logs` table. `info`-level turn stats stay in `ObsState`'s rolling stats.
- **The purge is non-fatal.** Same `if let Err(e) ... tracing::warn!` pattern as `seed_local_models`. A purge failure warns but doesn't block startup.
- **The `ObsLayer` must never panic.** Mutex locks use `let Ok(mut s) = self.state.lock() else { return }` (obs.rs:288), never `.unwrap()`. The flush code follows the same guard.
- **Commit messages: no `Co-Authored-By` or any co-author trailer.**

---

## File Structure

**Modified:**
- `crates/zoid-core/src/store.rs` — `logs` table creation, `write_log` method, `purge_logs` method, `LogRow` struct.
- `crates/zoid-core/src/session.rs` — `Cmd::WriteLog` + `Cmd::PurgeLogs` variants, actor handlers, `SessionHandle::write_log` + `SessionHandle::purge_logs` async methods.
- `crates/zoid/src/obs.rs` — `LogEntry` struct, `ObsState::logs` ring buffer, `ObsState::record_log` + `ObsState::take_logs`, `FieldGrab` `fields` collector, `ObsLayer::on_event` ring-buffer capture.
- `crates/zoid/src/main.rs` — boot: `purge_logs` call + ring-buffer flush after `seed_local_models`.

**No new files. No new crates. No config changes. No UI changes (the debug view is future work).**

---

### Task 1: `logs` table + `write_log` + `purge_logs` in `EventStore`

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add `LogRow` struct, table creation in `EventStore::open`, `write_log` method, `purge_logs` method)

**Interfaces:**
- Produces: `pub struct LogRow`, `pub fn write_log(&self, row: &LogRow) -> Result<()>`, `pub fn purge_logs(&self, ttl_ms: i64) -> Result<()>` on `EventStore`. Task 2 wires these through the actor.

- [ ] **Step 1: Write the failing tests**

Add to `crates/zoid-core/src/store.rs` test module (after the `seed_local_models` tests):

```rust
#[test]
fn write_log_inserts_turn_scoped_entry() {
    let dir = std::env::temp_dir().join(format!("zoid-wlog-turn-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();

    let row = LogRow {
        ts: 1000,
        level: "warn".into(),
        scope: "turn".into(),
        session_id: Some("01KZ7TEST".into()),
        event_id: Some("01KZ7EV1".into()),
        message: "provider error".into(),
        fields: Some(r#"{"status":429}"#.into()),
    };
    store.write_log(&row).unwrap();

    let (level, scope, sid, eid, msg, fields): (String, String, Option<String>, Option<String>, String, Option<String>) = store.conn.query_row(
        "SELECT level, scope, session_id, event_id, message, fields FROM logs WHERE ts = 1000",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    ).unwrap();
    assert_eq!(level, "warn");
    assert_eq!(scope, "turn");
    assert_eq!(sid.as_deref(), Some("01KZ7TEST"));
    assert_eq!(eid.as_deref(), Some("01KZ7EV1"));
    assert_eq!(msg, "provider error");
    assert_eq!(fields.as_deref(), Some(r#"{"status":429}"#));
}

#[test]
fn write_log_inserts_system_entry_with_null_ids() {
    let dir = std::env::temp_dir().join(format!("zoid-wlog-sys-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();

    let row = LogRow {
        ts: 2000,
        level: "error".into(),
        scope: "system".into(),
        session_id: None,
        event_id: None,
        message: "seed failed".into(),
        fields: None,
    };
    store.write_log(&row).unwrap();

    let (sid, eid): (Option<String>, Option<String>) = store.conn.query_row(
        "SELECT session_id, event_id FROM logs WHERE ts = 2000",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert!(sid.is_none(), "system log must have NULL session_id");
    assert!(eid.is_none(), "system log must have NULL event_id");
}

#[test]
fn purge_logs_deletes_old_entries() {
    let dir = std::env::temp_dir().join(format!("zoid-purge-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();

    // Insert entries at ts 1000 (old) and ts 5000 (recent).
    let old = LogRow { ts: 1000, level: "warn".into(), scope: "system".into(), session_id: None, event_id: None, message: "old".into(), fields: None };
    let recent = LogRow { ts: 5000, level: "warn".into(), scope: "system".into(), session_id: None, event_id: None, message: "recent".into(), fields: None };
    store.write_log(&old).unwrap();
    store.write_log(&recent).unwrap();

    // Purge entries older than ts 3000 (ttl_ms relative to now).
    // purge_logs computes cutoff = now - ttl_ms. To test, we pass a
    // ttl_ms that makes the cutoff land between 1000 and 5000.
    // Now is ~current epoch ms; we want cutoff = 3000. So ttl_ms = now - 3000.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    store.purge_logs(now - 3000).unwrap();

    let count: i64 = store.conn.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1, "only the recent entry (ts=5000) should survive");
    let msg: String = store.conn.query_row("SELECT message FROM logs", [], |r| r.get(0)).unwrap();
    assert_eq!(msg, "recent");
}

#[test]
fn purge_logs_with_no_entries_is_noop() {
    let dir = std::env::temp_dir().join(format!("zoid-purge-empty-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();
    store.purge_logs(72 * 60 * 60 * 1000).unwrap();
    let count: i64 = store.conn.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
}
```

Note: the `purge_logs_deletes_old_entries` test needs a `now_ms()` function. If `store.rs` doesn't have one, use `std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64` inline in the test. The `purge_logs` method itself takes `ttl_ms` and computes `cutoff = <current time> - ttl_ms` internally.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib write_log -- --nocapture` and `cargo test -p zoid-core --lib purge_logs -- --nocapture`
Expected: FAIL to compile — `cannot find type 'LogRow' in this scope` and `no method named 'write_log'/'purge_logs' found`.

- [ ] **Step 3: Write the implementation**

Add the `LogRow` struct near the top of `crates/zoid-core/src/store.rs` (after the `use` statements, before `EventStore`):

```rust
/// One row in the `logs` table. Turn-scoped logs populate `session_id` and
/// `event_id`; system logs leave them `None`.
pub struct LogRow {
    pub ts: i64,
    pub level: String,
    pub scope: String,
    pub session_id: Option<String>,
    pub event_id: Option<String>,
    pub message: String,
    pub fields: Option<String>,
}
```

Add the `logs` table creation to `EventStore::open`'s `execute_batch` (append after the `local_models` table — but note `local_models` is created in `seed_local_models`, not in `open`. Add `logs` to the `open` batch since it doesn't need seeding):

```rust
            CREATE TABLE IF NOT EXISTS logs (
                ts          INTEGER NOT NULL,
                level       TEXT NOT NULL,
                scope       TEXT NOT NULL,
                session_id  TEXT,
                event_id    TEXT,
                message     TEXT NOT NULL,
                fields      TEXT,
                CHECK (
                    (scope = 'system' AND session_id IS NULL AND event_id IS NULL)
                    OR
                    (scope = 'turn' AND session_id IS NOT NULL AND event_id IS NOT NULL)
                )
            );
            CREATE INDEX IF NOT EXISTS logs_ts ON logs(ts);
```

Add the `write_log` and `purge_logs` methods to `impl EventStore` (after `seed_local_models`):

```rust
    /// Insert one log entry. Called through the actor (single-writer).
    pub fn write_log(&self, row: &LogRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO logs (ts, level, scope, session_id, event_id, message, fields)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.ts,
                row.level,
                row.scope,
                row.session_id,
                row.event_id,
                row.message,
                row.fields,
            ],
        )?;
        Ok(())
    }

    /// Delete log entries older than `ttl_ms` ago. Called at boot (non-fatal).
    /// One DELETE with the logs_ts index — fast even on a large table.
    pub fn purge_logs(&self, ttl_ms: i64) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let cutoff = now - ttl_ms;
        self.conn.execute("DELETE FROM logs WHERE ts < ?1", params![cutoff])?;
        Ok(())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib write_log -- --nocapture` and `cargo test -p zoid-core --lib purge_logs -- --nocapture`
Expected: PASS — all four tests.

- [ ] **Step 5: Run the full zoid-core suite for regressions**

Run: `cargo test -p zoid-core --lib -- --nocapture`
Expected: PASS — all existing tests still pass (the new table doesn't affect existing tables).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(core): logs table + write_log + purge_logs in EventStore

The logs table captures both turn-scoped (with session_id/event_id) and
system (NULL ids) warn/error entries. CHECK constraint enforces the
scope/id invariant. purge_logs deletes entries older than a TTL (72h
default). The logs_ts index makes the purge fast."
```

---

### Task 2: `Cmd::WriteLog` + `Cmd::PurgeLogs` + `SessionHandle` methods

**Files:**
- Modify: `crates/zoid-core/src/session.rs` (add `Cmd` variants, actor handlers, `SessionHandle` async methods)

**Interfaces:**
- Consumes: `LogRow`, `EventStore::write_log`, `EventStore::purge_logs` from Task 1.
- Produces: `SessionHandle::write_log` + `SessionHandle::purge_logs` async methods. Task 4 calls these at boot.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-core/src/session.rs` test module (after the `seed_local_models_via_handle` test):

```rust
#[tokio::test]
async fn write_log_via_handle_inserts_system_entry() {
    let dir = std::env::temp_dir().join(format!("zoid-wlog-handle-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let h = SessionHandle::spawn(dir.to_str().unwrap().into()).await.unwrap();

    let row = crate::store::LogRow {
        ts: 42_000,
        level: "warn".into(),
        scope: "system".into(),
        session_id: None,
        event_id: None,
        message: "test via handle".into(),
        fields: None,
    };
    h.write_log(row).await.unwrap();
    drop(h); // flush the actor writer

    // Re-open and verify.
    let store = crate::store::EventStore::open(dir.to_str().unwrap()).unwrap();
    let msg: String = store.conn.query_row(
        "SELECT message FROM logs WHERE ts = 42000",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(msg, "test via handle");
}

#[tokio::test]
async fn purge_logs_via_handle_is_noop_on_empty() {
    let dir = std::env::temp_dir().join(format!("zoid-purge-handle-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let h = SessionHandle::spawn(dir.to_str().unwrap().into()).await.unwrap();
    h.purge_logs(72 * 60 * 60 * 1000).await.unwrap();
    drop(h);

    let store = crate::store::EventStore::open(dir.to_str().unwrap()).unwrap();
    let count: i64 = store.conn.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib write_log_via_handle -- --nocapture`
Expected: FAIL to compile — `no method named 'write_log' found for struct 'SessionHandle'`.

- [ ] **Step 3: Write the implementation**

Add two `Cmd` variants to the `Cmd` enum in `crates/zoid-core/src/session.rs` (after `SeedLocalModels`):

```rust
    /// Write one log entry to the `logs` table.
    WriteLog {
        row: crate::store::LogRow,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Purge log entries older than `ttl_ms` ago.
    PurgeLogs {
        ttl_ms: i64,
        reply: oneshot::Sender<Result<()>>,
    },
```

Add actor handlers in the `blocking_recv` loop (after the `SeedLocalModels` arm):

```rust
                    Cmd::WriteLog { row, reply } => {
                        let _ = reply.send(store.write_log(&row));
                    }
                    Cmd::PurgeLogs { ttl_ms, reply } => {
                        let _ = reply.send(store.purge_logs(ttl_ms));
                    }
```

Add async methods to `impl SessionHandle` (after `seed_local_models`):

```rust
    /// Write one log entry to the `logs` table via the actor.
    pub async fn write_log(&self, row: crate::store::LogRow) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::WriteLog { row, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Purge log entries older than `ttl_ms` ago via the actor.
    pub async fn purge_logs(&self, ttl_ms: i64) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::PurgeLogs { ttl_ms, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib write_log_via_handle purge_logs_via_handle -- --nocapture`
Expected: PASS — both tests.

- [ ] **Step 5: Run the full zoid-core suite for regressions**

Run: `cargo test -p zoid-core --lib -- --nocapture`
Expected: PASS — all tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/session.rs
git commit -m "feat(core): Cmd::WriteLog + Cmd::PurgeLogs + SessionHandle methods

Routes write_log and purge_logs through the actor (single-writer
invariant). Same oneshot-reply pattern as seed_local_models."
```

---

### Task 3: `ObsState` ring buffer + `FieldGrab` fields collector + `ObsLayer` capture

**Files:**
- Modify: `crates/zoid/src/obs.rs` (add `LogEntry` struct, `MAX_LOG_RING` const, `ObsState::logs` field, `record_log` + `take_logs` methods, `FieldGrab` fields collector, `ObsLayer::on_event` ring capture)

**Interfaces:**
- Consumes: nothing from prior tasks (this is the in-memory capture layer — independent of the db).
- Produces: `ObsState::logs` ring buffer, `ObsState::take_logs()` method. Task 4 calls `take_logs` to flush to the db.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid/src/obs.rs` test module:

```rust
    #[test]
    fn ring_buffer_captures_warn_and_error() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        let layer = ObsLayer { state: state.clone() };
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::warn!("test warning");
            tracing::error!("test error");
            tracing::info!("test info — should NOT be captured");
        });
        let s = state.lock().unwrap();
        assert_eq!(s.logs.len(), 2, "warn and error captured, info not");
        assert_eq!(s.logs[0].level, "warn");
        assert!(s.logs[0].message.contains("test warning"));
        assert_eq!(s.logs[1].level, "error");
        assert!(s.logs[1].message.contains("test error"));
    }

    #[test]
    fn ring_buffer_captures_unknown_fields() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        let layer = ObsLayer { state: state.clone() };
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::warn!(branch = "test-branch", branch_oid = "abc123", head_oid = "def456", "worktree diagnostic");
        });
        let s = state.lock().unwrap();
        assert_eq!(s.logs.len(), 1);
        let fields = s.logs[0].fields.as_ref().expect("fields must be captured");
        assert!(fields.contains("branch"), "fields must include branch: {fields}");
        assert!(fields.contains("abc123"), "fields must include branch_oid: {fields}");
        assert!(fields.contains("def456"), "fields must include head_oid: {fields}");
    }

    #[test]
    fn ring_buffer_drops_oldest_when_full() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        let layer = ObsLayer { state: state.clone() };
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            for i in 0..(MAX_LOG_RING + 5) {
                tracing::warn!(i, "fill entry {}", i);
            }
        });
        let s = state.lock().unwrap();
        assert_eq!(s.logs.len(), MAX_LOG_RING, "ring must be at capacity");
        // The oldest 5 entries are dropped; the first remaining is index 5.
        assert_eq!(s.logs[0].message, "fill entry 5", "oldest surviving entry should be entry 5");
    }

    #[test]
    fn take_logs_drains_and_returns_entries() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        {
            let mut s = state.lock().unwrap();
            s.record_log(now_ms(), "warn", "test message", None);
            s.record_log(now_ms(), "error", "another", None);
        }
        let entries = {
            let mut s = state.lock().unwrap();
            s.take_logs()
        };
        assert_eq!(entries.len(), 2);
        let s = state.lock().unwrap();
        assert!(s.logs.is_empty(), "take_logs must drain the ring");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid --lib obs::tests::ring_buffer -- --nocapture`
Expected: FAIL to compile — `no field 'logs' on type 'ObsState'`, `cannot find 'MAX_LOG_RING'`, `no method 'record_log'` / `'take_logs'`.

- [ ] **Step 3: Write the implementation**

Add to `crates/zoid/src/obs.rs`:

After `MAX_ERR_RING` (line 13):

```rust
pub const MAX_LOG_RING: usize = 500;
```

After `ErrEntry` (line 32):

```rust
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: i64,
    pub level: String,
    pub message: String,
    pub fields: Option<String>,
}
```

Add `logs` field to `ObsState` (after `errors`):

```rust
    pub logs: std::collections::VecDeque<LogEntry>,
```

Add `record_log` and `take_logs` to `impl ObsState` (after `record_error`):

```rust
    pub fn record_log(&mut self, ts: i64, level: &str, message: &str, fields: Option<String>) {
        if self.logs.len() == MAX_LOG_RING {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            ts,
            level: level.to_string(),
            message: message.to_string(),
            fields,
        });
    }

    /// Drain and return all entries from the logs ring buffer.
    pub fn take_logs(&mut self) -> Vec<LogEntry> {
        self.logs.drain(..).collect()
    }
```

Add `extra_fields: serde_json::Map<String, serde_json::Value>` to `FieldGrab` (after the existing fields):

```rust
    extra_fields: serde_json::Map<String, serde_json::Value>,
```

Change each `record_*` method's `_ => {}` fallback to collect into `extra_fields`:

In `record_u64`:
```rust
            _ => { self.extra_fields.insert(field.name().to_string(), value.into()); }
```

In `record_bool`:
```rust
            _ => { self.extra_fields.insert(field.name().to_string(), value.into()); }
```

In `record_str`:
```rust
            _ => { self.extra_fields.insert(field.name().to_string(), value.into()); }
```

In `record_debug` (add an else branch):
```rust
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.message.is_none() {
            self.message = Some(format!("{value:?}"));
        } else if field.name() != "message" {
            self.extra_fields.insert(field.name().to_string(), format!("{value:?}").into());
        }
    }
```

Extend `ObsLayer::on_event` to capture into the `logs` ring (after the existing `record_error` block, inside the `if level == WARN || level == ERROR` block):

```rust
            let fields = if g.extra_fields.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&g.extra_fields).unwrap_or_default())
            };
            s.record_log(
                now_ms(),
                lvl,
                &g.message.clone().unwrap_or_default(),
                fields,
            );
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid --lib obs::tests -- --nocapture`
Expected: PASS — all four new tests plus existing obs tests.

- [ ] **Step 5: Run the full bin test suite for regressions**

Run: `cargo test -p zoid -- -- --nocapture`
Expected: PASS — all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/obs.rs
git commit -m "feat(obs): logs ring buffer + FieldGrab fields collector

ObsState gains a 500-entry VecDeque<LogEntry> ring buffer (FIFO) that
captures warn/error events alongside the existing 20-entry errors ring.
FieldGrab's Visit is extended to collect unknown fields into a
serde_json::Map for the logs.fields column. info-level events are not
captured."
```

---

### Task 4: Boot flush + `purge_logs` call

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `purge_logs` + ring-buffer flush after `seed_local_models`)

**Interfaces:**
- Consumes: `SessionHandle::purge_logs` + `SessionHandle::write_log` (Task 2), `ObsState::take_logs` (Task 3).
- Produces: the boot path purges old logs and flushes the ring buffer to the db. After this, the system is fully operational.

- [ ] **Step 1: Find the boot site**

The `seed_local_models` call is at main.rs:2333 (after `SessionHandle::spawn`). The `purge_logs` call and ring-buffer flush go immediately after it. The `ObsHandle` is created at main.rs:2255 as `obs` (verify the variable name).

- [ ] **Step 2: Add the boot calls**

After the `seed_local_models` call (main.rs:2333-2335), add:

```rust
    // Purge log entries older than 72h (non-fatal — same pattern as
    // seed_local_models). Bounds the logs table across restarts.
    if let Err(e) = session.purge_logs(72 * 60 * 60 * 1000).await {
        tracing::warn!(error = %e, "failed to purge old logs");
    }

    // Flush the ObsState ring buffer to the logs table. Pre-actor system
    // logs (config warnings, boot diagnostics) were captured in-memory;
    // persist them now that the actor is available. Non-fatal.
    {
        let entries = match obs.state.lock() {
            Ok(mut s) => s.take_logs(),
            Err(_) => Vec::new(), // poisoned mutex — skip flush, don't crash
        };
        for entry in &entries {
            let row = zoid_core::store::LogRow {
                ts: entry.ts,
                level: entry.level.clone(),
                scope: "system".into(),
                session_id: None,
                event_id: None,
                message: entry.message.clone(),
                fields: entry.fields.clone(),
            };
            if let Err(e) = session.write_log(row).await {
                tracing::warn!(error = %e, "failed to flush system log to db");
            }
        }
    }
```

Note: verify that `obs` is the variable name for the `ObsHandle` created earlier. If it's `obs_state` or similar, adjust. Also verify that `obs.state` is accessible (it's `pub state: Arc<Mutex<ObsState>>` on `ObsHandle` at obs.rs:98).

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p zoid`
Expected: compiles cleanly.

- [ ] **Step 4: Run zoid and verify logs are persisted**

Run: `cargo run --release -- -p zoid -- --yolo` (then quit immediately)

Then verify:
```bash
sqlite3 ~/.local/share/zoid/zoid.db "SELECT count(*) FROM logs;"
```
Expected: some entries (any warn/error that fired during boot, like the worktree diagnostic or config warnings).

Also verify the purge ran (no entries older than 72h):
```bash
sqlite3 ~/.local/share/zoid/zoid.db "SELECT min(ts), datetime(min(ts)/1000,'unixepoch') FROM logs;"
```
Expected: the oldest entry is within the last 72h.

- [ ] **Step 5: Run the full test suite for regressions**

Run: `cargo test -p zoid -- -- --nocapture`
Expected: PASS — all tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): boot purge + ring-buffer flush to logs table

Purges logs older than 72h (non-fatal) and flushes the ObsState ring
buffer to the logs table after the actor starts. Same if-let-Err-warn
pattern as seed_local_models. After this, system warn/error events are
persisted to the db automatically."
```

---

## Post-implementation verification (not a task — manual, after all tasks land)

1. `cargo build --release` and run zoid once.
2. `sqlite3 ~/.local/share/zoid/zoid.db ".schema logs"` — verify the table matches the spec schema (7 columns + CHECK constraint + `logs_ts` index).
3. `sqlite3 ~/.local/share/zoid/zoid.db "SELECT datetime(ts/1000,'unixepoch'), level, scope, message, fields FROM logs ORDER BY ts DESC LIMIT 20;"` — verify recent warn/error events are captured with the right scope and fields.
4. Trigger a warn (e.g. a provider error) and verify it appears in the `logs` table with `scope = "turn"` and the correct `session_id`.
5. Restart zoid and verify the 72h purge ran (no entries older than 72h).
6. Verify the existing Overview page error widget still works (the `errors` ring is unchanged).