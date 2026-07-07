# Startup Session Picker (CWD-scoped)

## Problem

Sessions are already CWD-linked (`sessions.root_path`) and auto-resume is already
scoped to the current directory. But the startup flow silently auto-resumes the
single most-recent session (or silently creates a fresh one if that session is
live), and never surfaces a picker at startup. When a user has 2+ sessions for
the same CWD, there's no way to choose between them at launch — they get
whichever happens to be most recent, or a surprise fresh session if that one is
live.

## Design

A startup picker that fires only when there are 2+ sessions for the current
CWD. Zero-friction in the common case (0 or 1 session), deliberate when there's
ambiguity (2+).

### §1 Startup decision flow

```
zoid [no flags]   →  list sessions for cwd
                      ├─ 0 sessions  → create one, proceed to run()
                      ├─ 1 session   → auto-resume, proceed to run()
                      └─ 2+ sessions → startup picker, collect choice, proceed
zoid --new        →  create fresh session, proceed (no picker, no list)
zoid --resume <id> → match ULID (full or last-4 short form)
                      ├─ found & not live → resume
                      ├─ found & live     → resume (immediate takeover, no confirm)
                      └─ not found        → exit with error message, no TUI
```

The startup picker is a dedicated pre-TUI screen with its own tiny render+input
loop. It opens after crossterm raw mode / alt screen is entered but before
`run()`. On any selection (resume or create-new), the chosen session is claimed
and heartbeat-started, then control hands off to `run()`.

### §2 The startup picker screen

A self-contained render+input loop (~60-80 lines) in `main.rs` as a
`pick_session()` function. No `App` struct, no event log, no heartbeat — it runs
before any of that exists.

**State:**
- `sessions: Vec<SessionInfo>` — from `list_sessions(Some(root))`
- `selected: usize` — cursor, initialized to 0 (most-recent)
- `live: Vec<bool>` — per-row liveness (same `is_live` call the existing picker uses)

**Rendering:** A minimal full-screen layout:
- A title line: "Resume a session for `<repo name>`"
- One row per session: `name · age · tokens · [live marker]` (live marker = `●`, dim)
- A trailing row: "Create new session"
- The selected row is highlighted (reverse video)

**Input:** Arrow keys (up/down) move the cursor. Enter selects. Esc aborts to a
clean exit (no session created, terminal restored). No mouse, no scroll, no
palette — just arrows + Enter + Esc.

**Selection handling:**
- Session row selected: claim via `set_active(id, true, pid, now)`, start the
  heartbeat, proceed to `run()` with that session loaded.
- "Create new session" selected: mint a new ULID, `new_session(...)`, claim,
  heartbeat, proceed with an empty event log.
- Live session selected: `set_active` overwrites the row (immediate takeover, no
  confirm card); the other instance detects the takeover via its next heartbeat
  and yields.

### §3 CLI flags

Add two flags to the existing `cli::Cli` enum (the `Run` variant already carries
`companion`):

- `--new` — skip the list and picker entirely; always create a fresh session
  for the current CWD, claim it, proceed to `run()`.
- `--resume <id>` — skip the picker; resolve `<id>` as a ULID, accepting either
  the full form or the last-4 short form (matching the dashboard's `last4(...)`
  display). Resolution: list all sessions for the current CWD, find the one
  whose ULID string ends with `<id>` (when `<id>` is 4 chars) or equals `<id>`
  (full length). If not found (or ambiguous — multiple sessions share the same
  last-4), print an error to stderr and exit without entering the TUI.

Both flags are mutually exclusive (using them together is a usage error).
Neither changes the existing no-flag behavior — the decision flow from §1
applies only when no flag is present.

The flags are parsed before the TUI starts, so `--resume` can fail and exit
cleanly without any terminal setup. `--new` short-circuits straight to session
creation.

### §4 Integration with existing startup

The current `main()` startup sequence becomes:

1. Resolve DB path, spawn `SessionHandle` — unchanged.
2. Parse CLI flags (new — `--new` / `--resume <id>`).
3. **`--resume <id>`**: resolve against sessions for this CWD. Not found →
   stderr + exit. Found → set up that session, skip to step 6.
4. **`--new`**: create a fresh session, skip to step 6.
5. **No flag**: `list_sessions(Some(root))` → apply the decision flow:
   - 0 or 1 session → existing auto-create/auto-resume path (unchanged).
   - 2+ sessions → enter raw mode + alt screen, call `pick_session()`, get the
     choice, exit the picker loop with either a session id to resume or a
     "create new" signal.
6. **Common path** (all routes converge here): claim the chosen session via
   `set_active`, start the heartbeat, load the event log, build `App`, enter
   `run()`.

The picker sits between raw-mode setup and `run()`, so terminal state is
already correct. On `pick_session()` returning "abort" (Esc), restore the
terminal and exit — no session claimed, no heartbeat started.

The existing in-session `:resume` overlay keeps its confirm card for live
takeovers — only the startup picker skips confirmation. The existing `:new`
command is unchanged.

### §5 What is not touched

- Session store schema (`sessions` table, `SessionRow`, `SessionInfo`)
- `SessionHandle` and its actor
- `is_live`, heartbeat mechanism, takeover detection
- The in-session `:resume`/`:new` overlays (confirm card preserved)
- Event log, projections, compaction

### §6 Testing

Pure functions to extract and unit-test:
- `resolve_resume_id(sessions: &[SessionInfo], query: &str) -> Option<Ulid>` —
  full or last-4 ULID match; returns `None` on no match or ambiguity.
- `pick_choice(sessions: &[SessionInfo], live: &[bool], selected: usize,
  key: Key) -> PickOutcome` — the input handler logic, decoupled from rendering.
  Returns one of `Resume(Ulid)`, `CreateNew`, `Abort`, or `Pending(selected)`.

The render path is thin (straightforward line layout, no wrapping logic) and
covered by the integration of a successful boot.

### §7 Edge cases

- **All sessions live (2+):** the picker still shows them all; selecting one
  takes it over immediately. The user could also pick "Create new" to avoid
  disrupting any instance.
- **Session deleted/changed between list and pick:** the picker holds a
  snapshot from `list_sessions`; the `set_active` call on selection is
  best-effort and idempotent (same as today).
- **Esc from the picker:** clean exit, no session claimed, terminal restored.
- **`--resume` with no sessions for the CWD:** error, no TUI.
- **`--resume` ambiguity (two sessions share last-4):** error listing both
  full ULIDs, no TUI.
- **Non-UTF-8 / invalid ULID for `--resume`:** error, no TUI.