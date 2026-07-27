# TODO — deferred work

## Empty-state guidance for new vs. returning users (DONE)

Implemented in `crates/zoid-tui/src/onboarding.rs` + `crates/zoid/src/main.rs`.
See `docs/superpowers/specs/2026-07-06-empty-state-guidance-design.md`.

## Tool-call approvals (DONE)

Implemented across `crates/zoid-tools/src/approval.rs` (BlacklistGate +
shlex matcher), `crates/zoid/src/agent.rs` (Gate::Prompt arm), and config/CLI
wiring. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

## Delete old sessions from the session picker (DONE)

Implemented — session picker supports deleting stale sessions.

## Reduce "thinking" output shown in normal zoom (DONE)

Thinking content is trimmed/collapsed at normal zoom so the main agent's
messages and tool calls stay visible without being drowned out by reasoning.

## Subagents not working correctly (DONE)

Fixed — concurrent subagent pool, per-result delegation wake, DelegationResult
delivery, cancellation paths, and animated drawer spinner all working.
See `docs/superpowers/specs/2026-07-25-concurrent-subagent-execution-design.md`.

## Session startup: parallelize projection passes and lazy-load body cache

On resume of a long session, the initial frame render takes 10-16s. The
cost is 5 independent O(n) projection passes over the full event log
(`conversation`, `context_window`, `churn_timeline`, `tasks`,
`token_ledger`) plus the body cache's `conversation_view` (text wrap +
syntax highlight for every message).

Two optimizations to investigate:

1. **Parallelize the 5 projection passes.** They're independent — a
   `rayon::scope` or `tokio::join!` would cut the wall-clock cost to
   ~max(individual pass) instead of sum(all passes). The passes live in
   `ProjectionCache::refresh` (`crates/zoid/src/main.rs:1434`).

2. **Lazy-load the body cache.** `conversation_view` wraps + syntax-
   highlights every message, but only the visible viewport is painted.
   Render only the visible window's messages on the first frame, and
   build the rest on demand when scrolling. The body cache already
   supports incremental rebuilds — extend it to build only a window of
   lines instead of the full transcript.

Windowing the event log itself is NOT needed — `Arc<Event>` means bodies
are shared, not copied, and the eviction policy already compacts old
events. The cost is the projection passes, not storage.

## Tool call rendering truncated too short in the UI (DONE)

Fixed — tool-call lines and result previews now use more of the available
column width before truncating, with peek for the full content.