# Unified Logging with TTL Purge

## Problem

zoid has no always-on logging. The `tracing` subscriber writes JSON to a file
only when `ZOID_LOG` is set (an env var most users never configure). The
in-memory `ObsState` captures rolling stats and a bounded error ring, but those
are lost on exit. When `exit_worktree` silently deleted a branch with unmerged
commits, the diagnostic OIDs were emitted via `tracing::warn!` — which went
nowhere because `ZOID_LOG` wasn't set.

The fix (commit `af6d6d2`) included the OIDs in the `exit_worktree` tool return
message, but that only helps when the agent is the caller. For non-agent paths
(boot, UI, background tasks), the diagnostic is silent without `ZOID_LOG`.

## Vision

Always-on logging to a single `logs` table in the zoid db. Turn-scoped logs
carry their `session_id` and `event_id` for contextual debugging; system logs
(boot, UI, background) have those columns NULL. A 72h TTL purge at boot keeps
storage bounded. The `ZOID_LOG` env var becomes an override (custom path/level
for the JSON file layer) rather than the on/off gate.

## Architecture

### 1. The `logs` table

A new table in the zoid SQLite db, written by both turn-scoped and system log
paths:

```
logs table:
  ts          INTEGER NOT NULL     -- epoch milliseconds
  level       TEXT NOT NULL        -- "warn" | "error" (info deferred — see below)
  scope       TEXT NOT NULL        -- "turn" | "system"
  session_id  TEXT                 -- NULL for system logs
  event_id    TEXT                 -- links to events.id (NULL for system)
  message     TEXT NOT NULL        -- the log message text
  fields      TEXT                 -- JSON object of structured fields (OIDs, error codes)
```

**Constraints:**

```sql
CHECK (
  (scope = 'system' AND session_id IS NULL AND event_id IS NULL)
  OR
  (scope = 'turn' AND session_id IS NOT NULL AND event_id IS NOT NULL)
)
```

This makes the turn-vs-system invariant a db-level guarantee, not just caller
discipline.

**`level` domain:** the schema accepts `"warn"` and `"error"`. `info` is
deferred — the ring buffer only captures `warn`/`error` (the existing
`on_event` behavior). `info`-level logs (turn timing, provider stats) stay in
`ObsState`'s rolling stats and are not written to the `logs` table. If
`info`-level db logging is needed in the future, add it to the ring buffer
capture and the `level` `CHECK` constraint then.

**The `Cmd::WriteLog` payload.** The ring buffer stores `LogEntry` (reduced —
no `scope`/`session_id`/`event_id`). The `Cmd::WriteLog` command carries a
fuller `LogRow` struct:

```rust
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

Turn-scoped writes build a `LogRow` directly (the agent loop has `session_id`
and `event_id` in scope). The flush maps each `LogEntry` → `LogRow` with
`scope = "system"`, nulls for the ids.

**The `fields` column and schema drift.** Unlike `vram_curve` in the
`local_models` table (which is parsed structurally by `recommend_model` and
persists indefinitely), `fields` is opaque JSON read only by humans running
`sqlite3`. No code parses it structurally. The 72h TTL purge is itself the
schema-drift mitigation: old rows with old `fields` shapes age out in 72h.
This is the key asymmetry — `fields` is drift-tolerant *because* it's
TTL-bounded; `vram_curve` is drift-fragile *because* it isn't.

- Index on `ts` for the purge query: `CREATE INDEX IF NOT EXISTS logs_ts ON logs(ts)`.
- Purge: `DELETE FROM logs WHERE ts < ?` where `?` is 72h ago. One query, one
  table, fast with the index.

### 2. The write path

**Turn-scoped logs** (agent loop, provider stream): written via the
`SessionHandle` actor (same single-writer path as `emit()`). A new
`Cmd::WriteLog { entry, reply }` command routes through the actor to
`EventStore::write_log()`. The `session_id` and `event_id` are populated by the
caller (the agent loop has both in scope). These are `scope = "turn"`.

**System logs** (boot, UI, background): written via the same actor when it's
available. The `tracing` subscriber's `ObsLayer` captures `warn`/`error` into a
bounded ring buffer in `ObsState` (always on, no db needed at install time).
After `SessionHandle::spawn` creates the db, a one-time flush writes the buffer
to the `logs` table via `Cmd::WriteLog`. Subsequent system logs go directly
through the actor. These are `scope = "system"`, `session_id = NULL`.

**Logs before the actor starts** (config warnings, build-expiry,
pre-`obs::init`): these are rare and mostly `info`-level. They live only in the
`ObsState` ring buffer and are flushed when the actor is up. If zoid crashes
before the actor starts, the ring buffer is lost (it's in-memory). The panic
hook routes panics through `tracing` into the ring buffer — but the ring buffer
is not durable. A pre-actor panic is only durable if `ZOID_LOG` is set (the
JSON file layer). This is acceptable: pre-actor failures are deterministic and
reproducible (bad db path, permissions), and the window is narrow (~70 lines of
boot code). For crash diagnosis specifically, `ZOID_LOG` remains the
recommendation; the `logs` table is for runtime debugging, not crash
post-mortem.

### 3. The `ObsState` ring buffer

`ObsState` gains a `logs: VecDeque<LogEntry>` (bounded, 500 entries). The
`ObsLayer`'s `on_event` already captures `WARN`/`ERROR` level events into the
existing `errors: VecDeque<ErrEntry>` (obs.rs:305-317) — extracting `message`
via `FieldGrab`'s `Visit` and `ctx` from a known field name. Extending to the
`logs` ring buffer is a near-mechanical change: the same extraction path, a
larger ring (500 vs 20), and an added `fields` capture.

**The `fields` JSON column requires a new `Visit` fallback.** The existing
`FieldGrab` `Visit` impl (obs.rs:239-274) knows specific field names (`kind`,
`name`, `ctx`, `message`, `ms`, etc.) and discards everything else (`_ => {}`
in every `record_*` method). The `fields` column is the entire point of the
feature — it captures structured data like the `branch_oid`/`head_oid` from
the `exit_worktree` diagnostic. To populate it, `FieldGrab` is extended with a
`serde_json::Map` collector: each `record_*` method's `_ => {}` fallback
appends the unknown field to the map. After `Visit` completes, the map is
serialized to the `fields` JSON string (or `None` if empty).

**The existing `errors` ring stays.** `errors: VecDeque<ErrEntry>` (capped at
20, obs.rs:49) powers the Overview page's error widget and is read on every
frame. The new `logs` ring (500 entries) feeds the db flush and the future
debug view. They serve different readers (Overview widget vs. db/debug); the
cost of double-capturing `warn`/`error` in `on_event` is trivial.

1. **Pre-actor buffering** — holds logs until the db is available, then flushes.
2. **Overview/debug view** — the ring buffer powers a live log view in the TUI
   (if/when we add one), without querying the db on every frame.

After the flush, the ring buffer stays live (it's the in-memory view); the db
is the persistent copy. When the buffer is full, the oldest entry is dropped
(FIFO).

```rust
pub struct LogEntry {
    pub ts: i64,
    pub level: String,       // "warn" | "error" | "info"
    pub message: String,
    pub fields: Option<String>, // JSON
}
```

The ring buffer stores `LogEntry` without `scope`/`session_id`/`event_id`
(those are added at flush time — the flush knows whether each entry is
turn-scoped or system; pre-actor entries are always `scope = "system"`).

### 4. The purge

At boot, after `seed_local_models` (same non-fatal pattern):

```rust
if let Err(e) = session.purge_logs(72 * 60 * 60 * 1000).await {
    tracing::warn!(error = %e, "failed to purge old logs");
}
```

`SessionHandle::purge_logs(ttl_ms)` routes through the actor to
`EventStore::purge_logs()`, which runs `DELETE FROM logs WHERE ts < ?` with
`now - ttl_ms`. Non-fatal — a purge failure warns but doesn't block startup.
Follows the exact same `if let Err(e) ... tracing::warn!` pattern as
`seed_local_models`.

### 5. The flush

After `SessionHandle::spawn` (and after `seed_local_models` + `purge_logs`),
the bin flushes the `ObsState` ring buffer to the `logs` table:

```rust
let entries = match obs.state.lock() {
    Ok(mut s) => s.take_logs(),
    Err(_) => return, // poisoned mutex — don't crash the boot path
};
for entry in &entries {
    if let Err(e) = session.write_log(entry, "system", None, None).await {
        tracing::warn!(error = %e, "failed to flush system log to db");
    }
}
```

Non-fatal — a flush failure warns and the entry stays in the ring buffer (it's
still visible in-memory). After the flush, subsequent system logs go directly
through the actor.

### Data flow

```
boot:
  obs::init() → ObsLayer installed → ring buffer captures warn/error
  SessionHandle::spawn → db connection opened
  session.seed_local_models()   (non-fatal)
  session.purge_logs(72h)       (non-fatal, deletes old logs)
  flush ObsState ring buffer → session.write_log("system", None, None)
  → subsequent system logs go directly through the actor

turn:
  agent loop hits a warn/error
  → session.write_log("turn", Some(session_id), Some(event_id))
  → Cmd::WriteLog → EventStore::write_log → INSERT INTO logs

debugging:
  sqlite3 ~/.local/share/zoid/zoid.db
    "SELECT datetime(ts/1000,'unixepoch'), level, scope, message
     FROM logs ORDER BY ts DESC LIMIT 50;"
```

### What does not change

- **`ObsState`'s existing rolling stats** (turn ms, TPS, tool timings, frame
  times) — unchanged. The ring buffer is additive.
- **The `ZOID_LOG` JSON file layer** — still works as an override. If set, it
  writes structured JSON to a file in addition to the db. If not set, the db is
  the only sink.
- **`emit_ephemeral` for `ModelThinking`** — unchanged. Thinking stays
  in-memory only (not a log).
- **The panic hook** — unchanged. It routes through `tracing`, so panics land
  in the ring buffer (and the db once the actor is up). Pre-actor panics are
  only durable via `ZOID_LOG` (the file layer), not the db.
- **The `events` table** — unchanged. Logs are in `logs`, not `events`. No
  new `EventKind` variant.

### Error handling

- **Actor write fails** — the `Cmd::WriteLog` reply returns `Err`, the caller
  logs to the ring buffer (already there) and continues. No crash.
- **Purge fails** — `tracing::warn!`, continue. Old logs accumulate until the
  next successful purge. Bounded by the 72h window (at most 72h of logs).
- **Db doesn't exist yet** (pre-actor logs) — ring buffer holds them. If the
  actor never starts (crash), they're lost. Acceptable: the crash itself is
  captured by the panic hook.
- **Ring buffer full** — oldest entry dropped (FIFO). The db has the complete
  history within the 72h window; the ring buffer is just the live view.

### Storage and performance

The current db is 567MB with 1.08M events. Logs are negligible: even at a
pessimistic 10 warn/error entries per turn, 20 turns per session, 200 bytes
each = 40KB per session. Across 73 sessions that's ~3MB — less than 0.5% of the
db. The 72h purge keeps the `logs` table bounded regardless of session count.

The purge runs once at boot. If zoid runs continuously for days without
restart, the `logs` table grows past 72h until the next boot's purge. This is
acceptable for a TUI app (users close it regularly), but the spec acknowledges
the bound is a boot-time bound, not a runtime bound. If long-running instances
become a concern, a periodic purge (e.g. hourly via a timer command through the
actor) is a follow-up.

**Chatty warn-site flooding.** A retry loop or stuck provider stream can emit
hundreds of `tracing::warn!` calls in seconds. The 72h window means a chatty
site accumulates unboundedly until the next boot. The ring buffer (500 entries,
FIFO) bounds the in-memory view, but the db can grow. Admission control at the
write site — deduplication of identical `(message, target)` within a 1s window
in `ObsLayer::on_event` — is deferred to a follow-up if a chatty site is
observed in practice. The spec flags this as a known risk, not an unaddressed
gap.

### Testing

- **`write_log`** — insert a turn-scoped log with `session_id` + `event_id`,
  insert a system log without. Verify both are queryable from the `logs` table.
- **`purge_logs`** — insert logs at various timestamps, purge with a 72h TTL,
  verify only recent logs survive. Verify the `ts` index is created.
- **Ring buffer** — `ObsState` captures `warn`/`error` into the bounded
  `VecDeque`, drops oldest when full. `info`-level events are not captured.
- **Flush** — after `SessionHandle::spawn`, the ring buffer flushes to the
  `logs` table via the actor. Verify all buffered entries appear in the db
  with `scope = "system"`.
- **Non-fatal** — `write_log` and `purge_logs` failures produce `tracing::warn!`
  but don't crash or block startup.

### Scope

1. **`logs` table + `write_log` + `purge_logs`** — the db layer
   (`zoid-core/store.rs` + `session.rs`).
2. **`ObsState` ring buffer** — the in-memory capture layer (`obs.rs`).
3. **Flush on boot** — wire the ring buffer flush to the actor after
   `SessionHandle::spawn`.
4. **Boot purge** — the non-fatal `purge_logs` call, same pattern as
   `seed_local_models`.

Phase 1 (db layer) ships inert — table created, nothing writes to it yet. Phase
2 (ring buffer) captures logs in-memory. Phase 3 (flush) persists them. Phase
4 (purge) bounds storage. Each phase is independently shippable.