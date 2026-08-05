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
  level       TEXT NOT NULL        -- "warn" | "error" | "info"
  scope       TEXT NOT NULL        -- "turn" | "system"
  session_id  TEXT                 -- NULL for system logs
  event_id    TEXT                 -- links to events.id (NULL for system)
  message     TEXT NOT NULL        -- the log message text
  fields      TEXT                 -- JSON object of structured fields (OIDs, error codes)
```

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
before the actor starts, they're lost — but that's a narrow window, and the
panic hook captures panics through `tracing` into the ring buffer.

### 3. The `ObsState` ring buffer

`ObsState` gains a `logs: VecDeque<LogEntry>` (bounded, e.g. 500 entries). The
`ObsLayer`'s `Visit` implementation captures `warn`/`error` events into this
buffer. The buffer serves two purposes:

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
let entries = obs.state.lock().unwrap().take_logs();
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
- **The panic hook** — unchanged. It already routes through `tracing`, so
  panics land in the ring buffer → db.
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

The purge runs once at boot (one `DELETE` with a `ts` index), not during turns.
Turn-scoped log writes go through the actor (same path as `emit()`, which
already writes larger `ToolResult` events on every tool call). One small
`INSERT` per warn is negligible relative to existing write volume.

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