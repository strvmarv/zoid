# zoid Observability & the Overview Page — Design

**Status:** approved (brainstorm 2026-07-03), ready for `writing-plans`
**Audience:** implementers with zero prior context on this feature

## Goal

Give zoid one structured instrumentation layer, built on `tracing`, that feeds
**two consumers**: (1) an env-gated JSON diagnostic file sink for debugging zoid
itself, and (2) an always-on in-memory aggregator that powers a new **Overview**
zoom page — a whole-session metrics/usage dashboard rendered in the conversation
pane at the topmost zoom altitude.

This replaces the ad-hoc `dbglog`/`zlog!` seed with a real observability
substrate, and lights up the economy/cache telemetry that already exists but has
never had a home.

## Non-Goals (YAGNI)

- **No network export** (OTLP / Prometheus / remote collectors). The `tracing`
  choice keeps this a trivial later add; it is out of scope now.
- **No `Sessions` zoom altitude.** The user has a separate future idea for a
  `Sessions` altitude (a session switcher) above `Summary`; this spec adds
  **only** `Overview`. `Sessions` is its own later spec.
- **No persistence of telemetry** beyond the optional JSON file. Aggregates are
  in-memory and reset per process; they are not written to the SQLite event log
  (that log stays domain-only; 60 fps frame timing would flood it).
- **No new telemetry for token/economy data** — that already flows through the
  event log + economy projection and is read directly.

## Global Constraints

- **Baseline window size is 160×40** (cols×rows). Design and snapshot-test the
  Overview page at 160×40; 100×24 is the degrade floor, not the baseline.
- **Static-musl release must keep building.** `tracing` and `tracing-subscriber`
  are pure Rust (no C deps) — safe. Do not add features that pull C libraries.
- **The TUI owns the terminal** (alternate screen + raw mode). Nothing may write
  to stdout/stderr during a session; diagnostics go to a file only.
- **Never panic from the observability layer.** File-sink failures are silent;
  lock access is `.ok()`-guarded; aggregates are bounded.
- Commit messages carry no `Co-Authored-By` / co-author trailer.

## Architecture Overview

```
  tracing::{info_span!, info!, warn!, error!, trace!}   ← decentralized emission
        (zoid-core, zoid-provider, zoid-tui, bin)          (any crate, no wiring)
                          │
                 tracing-subscriber registry               ← set up once in main()
                 ┌────────┴─────────┐
                 ▼                  ▼
        JSON file layer        ObsLayer (custom, always on)
        (env ZOID_LOG,          ├ rolling aggregates (turn/tool/provider/frame)
         RUST_LOG level,        └ bounded error ring
         default off)                   │  Arc<Mutex<ObsState>>
                                        ▼
                          bin snapshots ObsState each frame
                          + economy projection (zoid-core)
                                        ▼
                       OverviewData → overview_lines(data, width)   (pure, zoid-tui)
                                        ▼
                        conversation pane @ Zoom::Overview altitude
```

Emission is decentralized (call `tracing` macros anywhere); collection/policy is
centralized in one subscriber built at startup. Instrumenting a call site is
additive and local — nothing threads a logger handle through functions.

## Component A — Instrumentation Layer

Lives in a new bin module `crates/zoid/src/obs.rs`, initialized once at the top
of `main()` before the TUI takes the terminal.

### A.1 Emission points

Add `tracing` as a dependency to the crates that emit. Instrument:

| Point | Location | Span/event | Fields |
|---|---|---|---|
| Agent turn | `zoid/src/agent.rs::run_turn_inner` (`'turn` loop) | `info_span!("turn")` | `model`, `branch`, `iterations`, `outcome` (completed / cap / error), duration |
| Tool call | `agent.rs` around each tool execution | `info_span!("tool")` | `name`, `ok` (bool), duration |
| Provider stream | `zoid-provider` `AnthropicProvider::stream` / `OllamaProvider::stream` | `info_span!("provider")` | `provider`, `model`, time-to-first-event, total stream time |
| Errors | existing error sites (provider `Error`, tool failures, session/DB `Err`) | `warn!` / `error!` event | context (HTTP status, tool name, op) + message |
| Panic | panic hook installed in `main()` | `error!` event | payload + location, logged **before** terminal restore |
| Frame | `zoid/src/main.rs` render loop, once per drawn frame | `trace!` event | `frame_ms`, `cache_hit` (bool), `proj_rebuilt` (bool) |

Levels: turn/tool/provider = INFO; errors = WARN/ERROR; frame = **TRACE**.

### A.2 Subscriber

Built in `obs::init() -> ObsHandle`. A `tracing_subscriber::registry()` with two
layers:

- **File layer** — `tracing_subscriber::fmt` with `.json()`, writing to the path
  in `ZOID_LOG` (unset → layer absent, zero overhead, matches today's `dbglog`).
  Level filter from `RUST_LOG` via `EnvFilter`, **default `info`**. So the 60 fps
  TRACE `frame` events are filtered out of the file unless the operator sets
  `RUST_LOG=trace` while chasing a perf regression.
- **`ObsLayer`** (custom `tracing_subscriber::Layer`, always on) — see A.3.

`obs::init()` returns a handle holding `Arc<Mutex<ObsState>>` so the render loop
can snapshot it. It also installs the panic hook (chaining the previous hook).

### A.3 `ObsState` + `ObsLayer`

`ObsState` (in `obs.rs`) holds bounded aggregates — O(1) memory regardless of
session length:

- `turn: RollingStats` — count, last, avg, p90 of turn durations.
- `provider: { ttft: RollingStats, total: RollingStats }`.
- `tools: BTreeMap<String, ToolStat>` where `ToolStat { count, avg_ms }`.
- `frame: RollingStats` (ms) + `cache_hits`, `cache_total`, `proj_rebuilds`.
- `errors: VecDeque<ErrEntry>` capped at `MAX_ERR_RING` (e.g. 20), each
  `{ ts_ms, level, context, message }`.

`RollingStats` is a small struct with a fixed-capacity ring (e.g. last 64
samples) computing avg/p90 on read. Pure, unit-testable.

`ObsLayer::on_event` / `on_close` maps `tracing` spans/events to `ObsState`
folds under a brief mutex lock:
- `turn`/`tool`/`provider` span close → fold duration into the matching stat.
- `frame` event → fold `frame_ms`, bump cache/proj counters (aggregate, never
  stored per-frame).
- WARN/ERROR event → push onto the error ring (capped).

A poisoned mutex degrades to stale data (`lock().ok()`), never a panic.

### A.4 Retiring `dbglog`

`crates/zoid/src/dbglog.rs` and the `zlog!` macro are removed; their ~7 call
sites (agent.rs tool/ask_user traces, main.rs scroll traces) are either dropped
or converted to `tracing::{debug,trace}!`. `ZOID_LOG` is preserved as the env var
that enables the file layer, so muscle memory and docs still work.

## Component B — Overview Zoom Page

### B.1 Zoom altitude

`crates/zoid-tui/src/state.rs`:
- Add `Zoom::Overview` as a variant. `Zoom::label` → `"overview"`.
- Altitude order (least → most detail): **Overview < Summary < Normal < Detail**.
- `zoom_out`: `Summary → Overview`, `Overview → Overview` (saturates).
- `zoom_in`: `Overview → Summary` (then Summary→Normal→Detail as today).
- Entering or leaving Overview resets `conversation_scroll = 0` (same
  re-anchor rule the other altitude changes already use).

### B.2 Direct entry via palette

Add a palette entry `"overview"` that sets `shell.zoom = Zoom::Overview`
directly (jump from any altitude). Implemented as a palette command in the
existing palette/command plumbing (`zoid-tui` command list + the bin's palette
run handler), mirroring how existing palette commands dispatch. No new hotkey in
this spec (palette entry only, per decision).

### B.3 Rendering

- At `Zoom::Overview` the conversation pane renders the **dashboard**, not
  transcript lines. `msg_starts` / cross-zoom message anchoring is **skipped**
  for this altitude (there is no per-message mapping); the body is top-anchored.
- A new module `crates/zoid-tui/src/overview.rs` holds both `OverviewData`
  (B.4) and the pure fn `overview_lines(data: &OverviewData, width: u16) ->
  Vec<Line>`, keeping the dashboard self-contained and independently testable.
  Layout is **C**: a session header line, a heavy-ruled KPI strip
  (tokens · cache-hit · avg turn · frame · errors), then a two-column body
  (ECONOMY + TOOLS left, TIMING + RUNTIME right), then an ERRORS band. At the
  115-col conversation width (160×40 baseline) the two columns fit with room to
  spare; content degrades gracefully at the 100×24 floor (single readable column
  region — no panic, no overlap).
- The dashboard is a `Vec<Line>`, so the **existing scrollbar + scroll offset
  machinery applies unchanged** when the dashboard is taller than the pane.
- The rail (repo/session/context/tasks) **stays visible** at Overview (decision:
  leave as-is for now).

### B.4 `OverviewData` seam

`OverviewData` (plain data, defined in `zoid-tui`): session header fields
(session id, model, provider, uptime, turn count), economy figures (input/output/
total tokens, cache read + hit %, per-turn sparkline series), timing (turn
last/avg/p90, provider ttft/stream, avg iterations), tools (name, count, avg ms),
runtime (frame avg/p90/max, cache-hit %, projection rebuilds, event count), and
recent errors (ts, level, context, message).

The bin (`main.rs`) assembles `OverviewData` each frame from an `ObsState`
snapshot + the existing economy projection (`zoid-core`), and caches it like the
conversation body cache (rebuild only when inputs change). `zoid-tui` stays pure
and never depends on `ObsState`.

## Data Flow (end to end)

1. Code emits `tracing` spans/events at the A.1 points.
2. The subscriber fans out: JSON file (if `ZOID_LOG` set, level `RUST_LOG`) +
   `ObsLayer` folds into `Arc<Mutex<ObsState>>`.
3. Render loop snapshots `ObsState` + reads the economy projection → builds/caches
   `OverviewData`.
4. At `Zoom::Overview`, the render calls `overview_lines(&data, width)` for the
   conversation body; scrollbar/scroll reuse the existing path.

## Error Handling

- File sink open failure → file layer silently absent (as `dbglog` is today).
- Mutex poisoning → `lock().ok()`; stale aggregates, never a crash.
- Aggregates bounded (rolling rings + capped error ring) → no unbounded growth.
- Panic hook logs to `tracing` (→ file) *before* the terminal-restore guard, so a
  crash leaves a forensic trail instead of a corrupted terminal.

## Testing

- **Pure unit tests:** `RollingStats` (avg/p90 correctness, ring wrap), `ObsState`
  fold operations (turn/tool/frame → stats; error-ring capping at `MAX_ERR_RING`).
- **Snapshot tests (insta):** `overview_lines` at **160×40** (matching the
  existing `*_wide_frame` convention) and at the **100×24** degrade floor.
- **File sink smoke test:** with `ZOID_LOG` set to a temp path, emit a span/event
  and assert a well-formed JSON line is written.
- Spans/events themselves are emission points, not separately unit-tested.

## Phasing (two independently-shippable plans)

- **Phase A — Instrumentation layer.** Add `tracing`; `obs::init` with the two
  layers; the A.1 spans/events + panic hook; retire `dbglog`/`zlog!`. Ships
  immediate value (a proper JSON diagnostic file) with no UI change.
- **Phase B — Overview page.** `Zoom::Overview` + palette entry; `OverviewData`
  + `overview_lines` (layout C); render + cache wiring in the bin; snapshots.
  Depends on Phase A (consumes `ObsState`).

Each phase is a separate implementation plan with its own tasks.
