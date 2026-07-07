# `:compact` Command — Design

> **Status:** brainstormed 2026-07-06. **Scope:** add an explicit `:compact`
> command (and palette entry) that triggers context compaction on demand,
> reusing the existing `plan_compactions` + `record_compactions` machinery.
> No new state, events, or render code — everything downstream already
> exists.

## Goal

Compaction is currently automatic: the preflight gate (`agent.rs`) fires
`plan_compactions` when the estimated context size crosses the configured
`compact_threshold_pct`. There is no way for the user to trigger it
manually — to see the `⊟→⊠→⊞→⊕` animation, to reclaim context proactively,
or to test compaction behavior without filling the context first.

This spec adds a `:compact` command (and a matching palette entry) that
explicitly triggers compaction on the current event log, reusing the
existing `plan_compactions` + `record_compactions` functions and the
existing `CompactionStarted`/`CompactionComplete` `AgentUpdate` variants.

## Non-Goals

- **Blocking chat during compaction.** The user can submit a new turn while
  compaction runs. `plan_compactions` is idempotent (skips already-compacted
  ids), and the events interleave cleanly with a new turn's events. The
  `Submit` guard is NOT extended.
- **Configuring the compaction threshold from the command.** That stays in
  the config overlay / `config.toml`.
- **Compaction of a specific tool result.** `:compact` compacts everything
  `plan_compactions` says should be compacted (largest-first, policy-driven).
  No per-id targeting.

## Architecture

```
User types :compact (or picks "compact" from the palette)
  → parse_command(":compact") → Command::CompactNow
  → exec_command(CompactNow)
    → if already compacting: status_hint "already compacting", return
    → spawn async task:
        → plan_compactions(events, policy, None, calibration, overhead)
        → if plan is empty: send AgentUpdate::CompactionStarted + CompactionComplete
          (brief indicator flash, confirms the command ran)
        → else: send CompactionStarted
          → for each compaction: session.append(ToolResultCompacted) + ui.send(Appended)
          → send CompactionComplete
  → existing run-loop handling:
    → CompactionStarted → app.compaction_started_at = Some(now), app.compacting = true,
      shell.compaction_started_at mirrored → box-rotation animates
    → CompactionComplete → app.compaction_complete = true, 3s debounce → compacting = false
```

The command is a thin wrapper around existing machinery. The only new code
is the `Command` variant, the parser arm, the palette entry, and the
`exec_command` arm that spawns the task.

## Components Touched

- **`crates/zoid-tui/src/command.rs`** — add `Command::CompactNow` to the
  `Command` enum; add `"compact"` to `parse_command` (maps to
  `Command::CompactNow`).
- **`crates/zoid-tui/src/palette.rs`** — add a `PaletteItem { label:
  "compact", command: Command::CompactNow }` to `all_items` (the Pick fuzzy
  list) so typing "compact" in the Ctrl+P palette surfaces it.
- **`crates/zoid/src/main.rs`** — add a `Command::CompactNow` arm to
  `exec_command`:
  - Guard: if `app.shell.compacting`, set `status_hint = "already
    compacting"` and return.
  - Spawn an async task (like `spawn_turn`) that:
    1. Computes `plan_compactions` on `app.events` (cloned snapshot) with
       the current `TurnConfig` policy + overhead + calibration ratio.
       Since there's no in-flight turn to read the calibration from, pass
       `None` for `real_input_tokens` and `None` for `calibration_ratio`
       (the plan uses the raw estimate — conservative, may compact less
       aggressively than the automatic gate, but correct).
    2. Sends `AgentUpdate::CompactionStarted` (unconditionally — even an
       empty plan flashes the indicator briefly so the user sees the
       command ran).
    3. For each compaction in the plan: `session.append(ToolResultCompacted
       { id, summary, original_tokens })` + `ui.send(AgentUpdate::Appended)`.
    4. Sends `AgentUpdate::CompactionComplete`.
- **No new state, events, or render code.** The `CompactionStarted`/
  `CompactionComplete` handling, the `compacting` flag, the 3s debounce, and
  the `⊟→⊠→⊞→⊕` box-rotation animation all already exist and work unchanged.

## Error Handling

- **Nothing to compact:** `plan_compactions` returns an empty plan. The
  command still emits `CompactionStarted` + `CompactionComplete` (the
  indicator flashes briefly via the 3s debounce), confirming the command
  ran. No "nothing to compact" hint — the brief flash IS the feedback.
  (Simpler than a special-cased hint, and the user sees the indicator
  animate, which is the point.)
- **Already compacting:** the `app.shell.compacting` guard short-circuits
  with a `status_hint = "already compacting"` and does not spawn a duplicate
  task.
- **session.append failure:** a failed append (DB error) is logged and
  skipped — the compaction continues to the next item. The
  `CompactionComplete` still fires. Non-fatal (the existing automatic
  compaction has the same behavior).

## Testing

- **Unit (command parsing):** `parse_command(":compact")` returns
  `Command::CompactNow`; `parse_command("compact")` (from the palette)
  returns `Command::CompactNow`.
- **Unit (palette):** `all_items(...)` contains a row with `label ==
  "compact"`; `selectable_matches(&items, "compact")` returns it.
- **Integration (exec_command):** with a seeded event log containing
  uncompacted tool results, `exec_command(CompactNow)` emits
  `CompactionStarted`, one or more `Appended` (carrying
  `ToolResultCompacted`), and `CompactionComplete`. With an empty log (no
  tool results), it emits `CompactionStarted` + `CompactionComplete` with no
  `Appended` events in between.
- **Integration (guard):** calling `exec_command(CompactNow)` while
  `app.shell.compacting == true` sets the "already compacting" hint and
  does not spawn a task (no duplicate `CompactionStarted`).

## Open Questions for Implementation

(Resolved during brainstorming; recorded here so the plan doesn't re-litigate.)

- **Blocking:** `:compact` does NOT block new chat turns (parallel is fine).
- **Discoverability:** both the direct command (`:compact`) and the
  palette entry (type "compact" in Ctrl+P) are added.
- **Empty plan:** still emits `CompactionStarted` + `CompactionComplete`
  (brief indicator flash as feedback, not a hint).
- **Calibration:** `None` (no in-flight turn to read from; the raw estimate
  is used — conservative but correct).