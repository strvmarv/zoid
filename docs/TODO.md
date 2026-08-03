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

## Session startup: parallelize projection passes (DONE)

The 5 independent O(n) projection passes (`conversation`, `context_window`,
`churn_timeline`, `tasks`, `token_ledger`) are now parallelized via
`std::thread::scope` in `ProjectionCache::refresh`
(`crates/zoid/src/main.rs:1458`). Wall-clock cost dropped from
sum(passes) to max(passes).

See `docs/superpowers/specs/2026-07-26-parallelize-projections-design.md`.
Commit `eaa311c`.

## Session startup: lazy-load the body cache (DEFERRED)

`conversation_view` wraps + syntax-highlights every message, but only the
visible viewport is painted. The remaining optimization is to render only
the visible window's messages on the first frame and build the rest on
demand when scrolling. The body cache already supports incremental
rebuilds — extend it to build only a window of lines instead of the full
transcript.

Windowing the event log itself is NOT needed — `Arc<Event>` means bodies
are shared, not copied, and the eviction policy already compacts old
events. The cost is the body cache build, not storage.

## Investigate aggressive context eviction (25-50k floor instead of 200-300k)

Context is being evicted down to 25-50k instead of the normal 200-300k
floor. This causes the model to lose large parts of its conversation history
mid-session — likely the root cause of the state confusion, duplicate
dispatches, and fragmented responses observed during long sessions.

**Root cause identified:** When `ModelInfoFetched` arrives (main.rs:3386),
`app.shell.ctx_ceiling` is set to the live-fetched `info.context_window`.
If the ollama-cloud API reports a smaller context window than the static
`MODEL_CAPS` table (e.g. 32K instead of 1M), the eviction band collapses:

- `EvictionPolicy.capacity` = 32K (from `ctx_ceiling`)
- `derive_band`: `effective_target` = min(300K, 32K - 8K) = 24K
- `low_water` = 24K - 4.8K = 19K
- Eviction fires at 24K, evicts down to 19K — only `recent_n: 4` turns
  protected.

The `context_target` config (300K) stays unchanged because it's `Some`
(line 3393 `unwrap_or_else` is skipped), but `capacity` overrides it in
`derive_band`.

Introduced by `e410be2` (Jul 25): changed the hard-ceiling pass to use
`config.context_window` (live-fetched) instead of the static table. The
eviction policy's `capacity` was always `ctx_ceiling`, but the live fetch
now overrides the 1M static value with whatever the API reports.

**Fix options:**
1. Use `max(ctx_ceiling, model_info(&model).context_window)` as the
   eviction `capacity` — the static table is the floor, the live fetch can
   only raise it.
2. When `ModelInfoFetched` sets `ctx_ceiling` below the static table's
   value, keep the static table's value instead (don't downgrade).
3. Clamp `context_target` down to `ctx_ceiling` when it arrives, so the
   band matches the actual capacity.

## Tool call rendering truncated too short in the UI (DONE)

Fixed — tool-call lines and result previews now use more of the available
column width before truncating, with peek for the full content.