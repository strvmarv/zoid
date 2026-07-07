# Multi-Instance Safety — Design

> **Status:** brainstormed 2026-07-06. **Scope:** make zoid safe to run many
> process instances on the same host, sharing one global `zoid.db`. Three
> interlocking changes: (1) a tuned SQLite concurrency foundation (WAL +
> `busy_timeout`), (2) stateful sessions carrying an "active interface"
> liveness flag, and (3) boot / resume behavior that avoids blind session
> collisions. A **config edit lock is explicitly out of scope** (deferred).

## Goal

zoid's persistent state is **host-global**: one `~/.local/share/zoid/zoid.db`
holds every session for every repo, partitioned by `session_id`. A user
commonly runs several zoid processes at once — multiple terminals in the same
working folder, or across repos — all hitting that one DB file. Today that
causes two real problems:

1. **Auto-resume collides.** Both instances adopt the *same* most-recently
   touched session for the repo; their appends interleave on disk and each
   in-memory log diverges from the other's. The next resume merges two
   parallel chats by rowid.
2. **`SQLITE_BUSY` turn failures.** `EventStore::open` sets no journal mode
   (default `DELETE`) and no `busy_timeout`. A second writer that can't
   acquire the lock returns `SQLITE_BUSY` *immediately*, which surfaces as a
   failed `store.append` → aborted turn + dropped event. SQLite never
   corrupts under concurrency (it errors rather than corrupts), but
   concurrent writes today cause random turn failures.

This spec makes "many zoids, one host, one DB" a safe, graceful mode of
operation: the second instance in a folder gets its own fresh session
instead of hijacking the first's, and all instances write the shared DB
without dropping turns.

## Non-Goals

- A **config edit lock** (read-modify-write on `./.zoid/config.toml` and the
  user-global config). The last-write-wins behavior stays. Revisited later.
- A **writer daemon / IPC architecture** (one process owns the DB; others
  talk to it over a socket). Far too large a change for a local single-user
  app; the WAL foundation makes it unnecessary.
- **Per-session DB files** (`sessions/<id>.db`) plus a global index. Rejected
  during brainstorming: it eliminates cross-session write contention but
  keeps a still-contended index DB, complicates the secret store (keyed to
  the global file today), and adds open/close-per-session machinery — for a
  benefit WAL largely already delivers.
- Any change to the companion server, worktree isolation, or the secret-key
  race (these were reviewed and found low-risk; left as-is).

## Architecture

Three layers, bottom-up. The SQLite foundation is independent; the
stateful-session layer sits on it; boot/resume behavior reads the
liveness flag.

```
┌──────────────────────────────────────────────────────────────┐
│  Boot / auto-resume / resume picker                          │
│  reads is_live(session) to decide: reclaim / fresh / warn    │
└───────────────────────┬──────────────────────────────────────┘
                        │
┌───────────────────────▼──────────────────────────────────────┐
│  Stateful sessions: active flag + active_pid + heartbeat    │
│  (sessions table) — pure is_live helper, heartbeat task,    │
│  yield protocol (old process detects takeover, cancels turn)│
└───────────────────────┬──────────────────────────────────────┘
                        │
┌───────────────────────▼──────────────────────────────────────┐
│  SQLite foundation: PRAGMA journal_mode=WAL + busy_timeout   │
│  (EventStore::open) — readers+writers overlap; writers wait │
└──────────────────────────────────────────────────────────────┘
```

### 1. SQLite foundation — WAL + busy_timeout

**Change:** `EventStore::open` runs, immediately after `Connection::open`,
two PRAGMAs:

```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
```

- **WAL** (`journal_mode = WAL`) allows concurrent readers and one writer
  without blocking — readers see a consistent snapshot, the writer appends
  to the WAL. This is the SQLite-recommended mode for "many local processes,
  one shared file" and is crash-safe (the WAL is checkpointed into the main
  DB automatically). WAL mode is **persistent** on the DB file (set once,
  persists across opens), but setting it on every open is harmless and
  self-documenting, so we do.
- **`busy_timeout = 5000`** makes a writer that can't acquire the lock
  **retry for 5 seconds** before returning `SQLITE_BUSY`. This turns
  "two zoids → random turn failures" into "two zoids → occasional brief
  stalls, no dropped events." The timeout is per-statement; the existing
  writer-thread serialization (one `Connection`, one actor) means zoid
  never contends with itself, only with other processes.

**No schema change.** No migration. The WAL files (`zoid.db-wal`,
`zoid.db-shm`) appear next to `zoid.db` and are auto-managed by SQLite; they
are safe across the host and require no cleanup.

**Error handling:** if a write still errors after 5s (sustained contention
from many processes, or a wedged reader), the existing agent-loop error
path surfaces it — `emit` returns `Err`, the turn aborts with a `⚠`
assistant message, `TurnComplete` fires. No new crash paths; behavior is
"turn fails gracefully" rather than "turn silently drops events."

### 2. Stateful sessions — `active` liveness

**Schema change** to the `sessions` table, added by an idempotent
migration in `EventStore::open` (the same probe-then-ALTER pattern already
used for `active_mode`):

```sql
ALTER TABLE sessions ADD COLUMN active           INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN active_pid       INTEGER;
ALTER TABLE sessions ADD COLUMN active_heartbeat INTEGER;
```

`active` is the boolean flag ("an interface has this session open");
`active_pid` is the OS PID of that interface; `active_heartbeat` is the
epoch-millis timestamp the interface last refreshed. Defaults (`active=0`,
NULL pid/heartbeat) mean "no interface has this session open" — the state
of every existing session after migration.

#### Constants

```rust
/// How often a live process refreshes its `active_heartbeat` (ms).
const HEARTBEAT_INTERVAL_MS: i64 = 5_000;
/// A session is considered "live" only if its heartbeat is within this
/// window (ms). 3× the interval: a single missed heartbeat (GC pause,
/// system suspend) does NOT mark a live session stale.
const LIVE_WINDOW_MS: i64 = 15_000;
```

#### Liveness judgment — pure helper

A pure function, unit-tested, used by both boot/auto-resume and the resume
picker:

```rust
/// Is `session` currently held by a live interface? Pure except for the
/// `pid_alive` OS check. A session is live iff its flag is set, its PID is
/// still alive, and its heartbeat is within LIVE_WINDOW_MS.
pub fn is_live(
    active: i64,
    active_pid: Option<i64>,
    active_heartbeat: Option<i64>,
    now_ms: i64,
    pid_alive: impl Fn(i64) -> bool,
) -> bool {
    active == 1
        && active_heartbeat.is_some()
        && match active_pid {
            Some(pid) => pid_alive(pid) && now_ms - active_heartbeat.unwrap() < LIVE_WINDOW_MS,
            None => false,
        }
}
```

`pid_alive` is injected (not a global syscall) so it's testable. The bin
supplies a real impl (`kill(pid, 0) == 0` on Unix; a no-op-true or
process-list check on Windows — out of scope to fully spec, but the Unix
path is the only one exercised today). The `SessionRow` / `SessionInfo`
types gain the three columns so callers don't reach into raw rows.

#### Heartbeat task

When a process adopts a session (boot auto-resume, fresh-session creation,
or manual resume), it claims the row then starts a heartbeat:

```rust
// Claim (idempotent — overwrites any stale flag, which is the reclaim path):
store.set_active(session_id, active=1, active_pid=self_pid, active_heartbeat=now_ms);
// Then spawn the heartbeat on the existing writer thread / a background task:
//   every HEARTBEAT_INTERVAL_MS: store.heartbeat(session_id, self_pid, now_ms)
```

`store.heartbeat` is a new `SessionHandle` command → writer-thread arm
running:

```sql
UPDATE sessions SET active_heartbeat = ?1
WHERE id = ?2 AND active_pid = ?3;
```

The `WHERE active_pid = ?3` guard is the **yield-detection probe**: if
another process has taken over the row (overwritten `active_pid`), this
UPDATE matches **zero rows** — which is the signal the old process uses to
yield (see §2.4). If the UPDATE matches zero rows for any other reason
(session row deleted — not possible today, no `DELETE` on `sessions`),
that's still a correct yield.

On **clean exit**, the process clears its row before terminating:

```sql
UPDATE sessions SET active=0, active_pid=NULL, active_heartbeat=NULL WHERE id=?;
```

Best-effort; if the process is force-killed the row stays stale and the
next evaluator (boot/picker) reclaims it via `is_live == false`.

#### Yield protocol — the old process detects takeover

When the heartbeat UPDATE matches zero rows (its `active_pid` is no longer
on the row — another process took over), the old process yields:

1. **Fire the in-flight turn's cancellation token immediately.** The
   `turn_cancel: Option<CancellationToken>` already exists for Esc/Ctrl-C;
   yielding reuses it. The agent loop drains pending tool calls and ends
   the turn cleanly (the existing cancel path).
2. **Stop appending to that session.** The heartbeat task halts; no further
   `session.append` calls are made for that `session_id`. The in-flight
   turn's events (already in the local `EventLog`) are *not* persisted
   further — the cancel path already emits balanced `[skipped]` results,
   and those emits are the last writes.
3. **Surface a hint:** `status_hint = "session taken over by another
   instance"`. The UI stays live; the user can `:new`, `:resume`
   elsewhere, or quit. The event loop continues to render (no crash), but
   `spawn_turn` is guarded against starting a new turn on a yielded
   session (a `yielded: bool` flag on `App`, set here, checked in
   `Action::Submit` / the bin's submit path — symmetric with the existing
   `streaming || delegating` guard).

The divergence window is bounded to **one heartbeat interval** (~5s): the
old process may append for at most that long after takeover before its
next heartbeat detects the takeover and yields. Two processes on one
session only coexist if the user explicitly forced a takeover, and only
for ≤5s — far better than today's "forever."

### 3. Boot / auto-resume / resume-picker behavior

#### Boot auto-resume (in `main`, replacing today's auto-resume block)

```
sessions = session.list_sessions(Some(root))   // most-recent-first, as today
first_time_user = sessions.is_empty()
if sessions.is_empty():
    create a fresh session (as today)
else:
    s = sessions[0]                               // most-recently-touched
    if is_live(s):
        # another interface is on it → create a fresh session, leave s alone
        create a fresh session
    else:
        # reclaim it: load, touch, claim
        resume s; clear/overwrite its active_* to self
```

A fresh session created here is *not* "first time" (`first_time_user` stays
`false`), so it shows the returning-user empty state, not onboarding.

#### Resume picker (manual resume)

The picker rows already render `SessionInfo`; each gains an "in use" marker
when `is_live`. The exact marker glyph is a render detail (left to
implementation; e.g. a `●` or a `· in use` suffix), but the data path is:
`list_sessions` (already returns most-recent-first) enriched with the
three `active_*` columns, and `is_live` evaluated per row at render time
with `now_ms` and the real `pid_alive`.

Selecting a **non-live** row resumes it directly (as today, plus the
claim/heartbeat).

Selecting a **live** row raises a confirm card (the existing inline
question-card mechanism, `QuestionKind::Ask`):

> *"Session `<name>` is active in another instance. Take it over? The other
> instance will detect this and yield."*
> Choices: **Take over** / **Cancel**.

- **Cancel** → return to the picker (no state change).
- **Take over** → overwrite that row's `active_pid`/`active_heartbeat` to
  claim it (`set_active(..., active_pid=self, ...)`), then load + resume as
  today. The old process detects the overwrite on its next heartbeat and
  yields (§2.4).

A live row is **never greyed out / unselectable** — the user can always
override deliberately. This avoids the "a slow process locks me out
forever" failure mode (a hung-but-alive process would otherwise make a
session permanently un-resumable).

## Data Flow

```
Boot:
  list_sessions(repo) ──▶ most-recent session
  is_live? ──yes──▶ create fresh session ──▶ claim ──▶ heartbeat task
         │
         no──▶ resume + claim ──▶ heartbeat task

Heartbeat (every 5s, writer thread):
  UPDATE active_heartbeat WHERE id AND active_pid==self
  matched? ──yes──▶ refresh local heartbeat timestamp
         │
         no──▶ YIELD: cancel turn, stop appending, hint, set yielded flag

Manual resume (picker):
  row live? ──no──▶ resume + claim + heartbeat
         │
         yes──▶ confirm card
            Take over ──▶ claim (overwrite active_pid) + resume + heartbeat
            Cancel     ──▶ back to picker
```

## Components Touched

- **`crates/zoid-core/src/store.rs`** — `EventStore::open`: add WAL +
  `busy_timeout` PRAGMAs; add the three-column idempotent migration; add
  `set_active` (claim/clear) and `heartbeat` SQL. Add `is_live` (pure).
- **`crates/zoid-core/src/session.rs`** — new `SessionHandle` commands
  (`SetActive`, `Heartbeat`); `SessionHandle::spawn` heartbeat is driven
  by the bin, not the actor, so no long-running task here.
- **`crates/zoid-core/src/sessions.rs`** — extend `SessionRow` /
  `SessionInfo` with `active`, `active_pid`, `active_heartbeat`; the
  `session_list` fold passes them through.
- **`crates/zoid/src/main.rs`** — boot auto-resume branch (fresh on live);
  claim on adopt; spawn the heartbeat task (a tokio interval calling
  `session.heartbeat`); clean-exit clear; the `yielded: bool` flag and its
  submit guard; resume-picker "in use" marker data + the takeover confirm
  card; `pid_alive` impl.
- **`crates/zoid-tui` (picker rows / confirm card)** — render the "in
  use" marker; the takeover confirm card reuses the existing
  `QuestionKind::Ask` path (no new card type).

## Error Handling

- **DB write under contention:** WAL + `busy_timeout` absorbs it; a
  write that still errors after 5s surfaces through the existing agent
  error path (turn aborts, `⚠` message, `TurnComplete`). No new crash
  paths, no silent event drops.
- **Stale-flag cleanup is implicit.** A crashed process leaves a stale
  row, but the next process that evaluates that session (boot or picker)
  sees `is_live == false` (dead PID or stale heartbeat) and reclaims. No
  background reaper task is needed. This is the key property that makes
  the flag model safe under force-kills.
- **PID reuse** (the OS recycled a dead process's PID to a different
  process): the heartbeat staleness check (`now - heartbeat < 15s`) is
  the safety net — even if the recycled PID happens to be alive, a
  session whose heartbeat is 15s+ stale is reclaimable regardless. The
  window for a false "live" from PID reuse is ≤15s and requires a
  specific PID collision within that window — vanishingly unlikely on a
  local dev host.
- **Heartbeat write failure:** the heartbeat is best-effort; a failed
  `UPDATE` (DB temporarily unwritable) is logged and retried next
  interval. It never aborts the process or the turn.

## Testing

- **Unit (pure):** `is_live` against constructed rows — (a) flag set +
  live PID + fresh heartbeat ⇒ live; (b) stale heartbeat (>15s) ⇒ not
  live; (c) dead PID ⇒ not live; (d) `active=0` ⇒ not live; (e) NULL
  pid/heartbeat ⇒ not live.
- **Unit (migration):** `EventStore::open` is idempotent across re-open
  (the three columns are added once, not re-added); an old-shape DB
  (without the columns) migrates cleanly and the columns default to
  `active=0`/NULL.
- **Integration (concurrency):** two `SessionHandle`s on one temp DB;
  assert concurrent appends don't error (WAL); assert a contended write
  eventually lands within `busy_timeout`.
- **Integration (lifecycle):** claim → heartbeat updates the row; a
  second claim (takeover) makes the first's heartbeat UPDATE match zero
  rows (the yield signal); clearing the row (clean exit) leaves
  `active=0`.
- **Integration (boot):** with a test-double `pid_alive` (returns false
  for a given PID), assert the boot path creates a fresh session when
  the most-recent session is "live"; with `pid_alive` true, assert it
  reclaims a stale-heartbeat row.
- **Integration (resume picker):** a live row's confirm card is raised
  on select; "Take over" overwrites `active_pid`; "Cancel" returns to
  the picker with no state change.

## Open Questions for Implementation

(Resolved during brainstorming; recorded here so the plan doesn't re-litigate.)

- **Config edit lock:** deferred (out of scope).
- **Liveness model:** flag + PID + heartbeat (not PID-only, not OS flock).
- **Boot on a live most-recent session:** create a fresh session (not
  refuse, not confirm-card).
- **Manual resume of a live session:** warn + explicit confirm, then
  takeover (never blocked).
- **Yield on detected takeover:** cancel the in-flight turn immediately,
  stop appending, hint, set `yielded` (not "let it finish", not
  "auto-fork to a new session").
- **Cadence:** heartbeat every 5s, live window 15s (3×).