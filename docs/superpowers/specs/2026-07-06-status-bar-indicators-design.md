# Status Bar Indicator Refinement — Design

> **Status:** brainstormed 2026-07-06. **Scope:** refine the placement,
> spacing, and animation of the three activity indicators on the bottom
> status bar (tool, working/idle, compaction) so "working" stays dead-center
> regardless of the others, the tool indicator has room for detail, and
> continuous animation is reduced to a single spinner + two pulse-on-appear
> badges.

## Goal

The status bar shows three activity indicators:

1. **Tool** (`◐ shell …`, orange) — which tool is running. Appears when a
   Local tool call is in flight; disappears on its `ToolResult`.
2. **Working/idle** (`⠋ working` blue / `● idle` green) — overall agent state.
   Always present.
3. **Compaction** (`⊟ compacting`, purple) — automated context compaction in
   progress. Appears during a `CompactionStarted`→`CompactionComplete` burst
   (held for a 3s minimum display via debounce).

Today the tool indicator docks left-of-center and compaction docks
right-of-center, both displacing the "working" indicator whenever they
appear or disappear. This causes **jitter**: "working" shifts left when
compaction appears, right when the tool appears, and both when all three
are present. The three also have inconsistent animation — "working" spins
(braille), compaction spins (its own 6-frame cycle), and the tool is static.

This spec makes "working" **dead-center, always**, docks the other two at
fixed anchors (⅓ and ⅔ of the bar width), holds a constant 4-space gap, and
reduces animation to the single "working" spinner plus a brief
**pulse-on-appear** for tool and compaction.

## Non-Goals

- **Moving indicators out of the status bar** (a right-rail activity panel).
  Rejected during brainstorming — overkill for 3 transient indicators.
- **A centered cluster** (all three grouped, centered as a unit). Rejected —
  "working" still moves within the group; minimal improvement over today.
- **Three independent continuous animations.** Rejected — visually noisy.
- Any change to the mode chip, status hint, zoom hint, or the "working"
  spinner itself (braille, 10 frames at 80ms — unchanged).

## Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│  status bar (full width W)                                             │
│                                                                        │
│  [CHAT] · hint     ◐ shell — cargo build    ⠋ working    ⊟ compacting   zoom normal │
│  └── left ──┘     └── ⅓ anchor ──┘        └── ½ ──┘    └── ⅔ anchor ──┘  └── right ──┘ │
│                                                                        │
│  ⅓ = W/3 (saturating)    ½ = W/2 (dead center)    ⅔ = 2W/3 (saturating) │
│  gap = 4 spaces (fixed, between tool↔working and working↔compaction)    │
└────────────────────────────────────────────────────────────────────────┘
```

Three independent slots, each at a fixed horizontal position. An indicator
that is absent simply doesn't render; its slot stays empty. No indicator's
position depends on another's presence → **zero jitter**.

### 1. Layout — fixed anchors (A2, halfway)

**Working** is at dead-center (W/2). Always rendered. Its position is
computed as `center_start = W.saturating_sub(center_w) / 2` — identical to
today's formula when no other indicators are present.

**Tool** docks at the ⅓ anchor. Its left edge is placed at
`tool_start = (W / 3).saturating_sub(tool_w)` — centered *within* the left
third, so it sits roughly at the ⅓ mark with balanced padding on both
sides. When absent, no rendering, no space reserved.

**Compaction** docks at the ⅔ anchor. Its left edge is placed at
`compact_start = (2W / 3).saturating_sub(compact_w / 2)` — centered within
the right third. When absent, no rendering.

**Right** (zoom hint) stays pinned to the right edge, as today:
`right_start = W.saturating_sub(right_w)`.

**Left** (mode chip + status hint) stays left-aligned, as today.

**Narrow terminal fallback:** all anchors use saturating math. Below ~60
columns the ⅓ and ⅔ positions collapse toward center; the tool and compaction
indicators abut "working" rather than overflowing or wrapping. Below ~40
columns the tool detail truncates to `◐ {name}` (drops the ` — {args}`
suffix and the ellipsis). The `human_tokens`-style truncation is display-
only; the full tool name is always on `state.active_tool`.

### 2. Spacing — fixed 4-space gap (B, moderate)

A fixed 4-space gap separates the tool indicator from "working" and
"working" from compaction. The gap is a constant string literal (`"    "`),
not proportional to bar width. On wide terminals the extra space flows into
the padding between the chip/zoom and the ⅓/⅔ anchors (the indicators
spread out); on narrow terminals the fixed gap is preserved until the
indicators abut (saturating math clips it to 0 before overflowing).

The 4-space gap is rendered as a dim `Span::styled("    ", Style::new())`
(empty style) — invisible padding, not a visible separator.

### 3. Animation — working spins, tool/compaction pulse on appear (C)

**Working** (`⠋` / `●`): unchanged. The braille spinner cycles 10 frames at
80ms while `state.busy`; static `●` (green) when idle. The frame is supplied
by the bin each render from wall-clock elapsed (kept out of the pure
renderer for snapshot determinism).

**Tool** (`◐ shell …`): **static glyph while present**, but **pulses on
appear**. When the tool indicator first renders (tool just started), it
shows at **full `color::WARN` intensity** for ~300ms, then settles to a
**dimmer steady-state**. The pulse is a simple brightness ramp:

```
if tool_started_at.is_some() && elapsed < PULSE_MS:
    fg = color::WARN                    // full bright (pulse)
else:
    fg = color::WARN_DIM                 // steady-state (dimmer)
```

`PULSE_MS = 300`. The pulse window is driven by the motion tick (already
running while `streaming || delegating`). No new tick cadence.

**Compaction** (`⊟ compacting`): same pulse-on-appear pattern. Bright for
~300ms after compaction starts, then settles. The existing
`compact_spinner` (6-frame `⊟⊞⊟⊕⊞⊕` cycle) is **retired** — the static `⊟`
glyph + pulse replaces it. `ShellState.compact_spinner` and
`tokens::COMPACT_SPINNER` are deleted.

#### Pulse state

The pulse needs a "started at" timestamp for each indicator, mirrored onto
`ShellState` (the pure renderer can't reach `App`):

- `ShellState.tool_started_at: Option<std::time::Instant>` — set by
  `set_active_tool`, cleared by `clear_active_tool`. The renderer reads it
  to compute `elapsed` for the pulse window.
- `ShellState.compaction_started_at: Option<std::time::Instant>` — needs
  to be **added** to `ShellState` (it currently lives only on `App` for the
  3s debounce; the pure renderer can't reach `App`). The bin mirrors it
  each frame alongside `compacting`. The renderer reads it for the pulse
  window.

A new `color::WARN_DIM` token is the steady-state color for the tool
indicator (a dimmed variant of `WARN`'s orange — e.g.
`Color::Rgb(0x8a, 0x66, 0x1a)`). Compaction's steady-state uses a similarly
dimmed purple (`COMPACT_DIM`), and its pulse uses the existing bright
compaction color.

#### Motion tick guard

The motion tick's `select!` guard currently fires while
`app.streaming || !app.in_flight_subagents.is_empty() || app.shell.compacting || app.zoom_changed_at.is_some()`.
The `compacting` guard is already present (keeps the tick alive through
compaction's 3s debounce). Add `|| app.shell.active_tool.is_some()` so the
tool pulse animation completes even if the tool finishes during a brief
non-streaming window (defense-in-depth — `streaming` stays true across
sub-turns today, but the guard future-proofs against a flicker).

## Data Flow

```
set_active_tool(name):
  state.active_tool = Some(name)
  state.tool_started_at = Some(Instant::now())     ← NEW: pulse anchor

clear_active_tool():
  state.active_tool = None
  state.tool_started_at = None                      ← NEW: clear pulse anchor

CompactionStarted (AgentUpdate):
  app.compaction_started_at = Some(Instant::now())  (existing)
  app.compacting = true                             (existing)
  app.shell.compaction_started_at = ...              (existing mirror)

CompactionComplete + 3s debounce:
  app.compacting = false                            (existing)
  app.shell.compaction_started_at = None            (existing)

render_status():
  tool_pulse = tool_started_at.map(|t| t.elapsed() < 300ms).unwrap_or(false)
  tool_fg = if tool_pulse { color::WARN } else { color::WARN_DIM }
  compact_pulse = compaction_started_at.map(|t| t.elapsed() < 300ms).unwrap_or(false)
  compact_fg = if compact_pulse { color::COMPACT } else { color::COMPACT_DIM }
  // place tool at ⅓, working at ½, compaction at ⅔ (fixed anchors)
```

## Components Touched

- **`crates/zoid-tui/src/render.rs`** — `render_status`: rewrite the
  indicator layout to fixed ⅓/½/⅔ anchors with 4-space gaps; add the
  pulse-on-appear brightness ramp for tool and compaction; remove the
  `compact_spinner` frame read (static `⊟` instead).
- **`crates/zoid-tui/src/state.rs`** — add `tool_started_at:
  Option<std::time::Instant>` to `ShellState`; set/clear it in
  `set_active_tool`/`clear_active_tool`. Delete `compact_spinner: char`
  and its init/mirror.
- **`crates/zoid-tui/src/tokens.rs`** — add `WARN_DIM` and `COMPACT_DIM`
  color tokens; delete `COMPACT_SPINNER`.
- **`crates/zoid/src/main.rs`** — the motion tick `select!` guard: add
  `|| app.shell.active_tool.is_some() || app.shell.compacting`; remove
  the `compact_spinner` per-frame computation (no longer needed); the
  `compaction_started_at` mirror onto `shell` stays (already exists for
  the debounce; now also read by the renderer for the pulse).
- **`crates/zoid-tui/tests/snapshots/`** — update snapshot tests that
  assert the old indicator positions/animation (`active_tool_spinner_frame`
  already updated for the status-bar move; compaction snapshots will need
  updating for the retired spinner + new position).

## Error Handling

- **Pulse timestamp missing:** if `tool_started_at` is `None` but
  `active_tool` is `Some` (shouldn't happen, but defensive), the renderer
  treats `elapsed` as infinite → steady-state (no pulse). No panic.
- **Narrow terminal overflow:** all anchor math is saturating; indicators
  abut or clip rather than overflow. No panic, no wrapping.
- **Reduced motion:** when `state.reduced_motion` is true, the pulse is
  skipped (indicator renders at steady-state immediately on appear). The
  "working" spinner already degrades to a static glyph under reduced motion
  (existing behavior).

## Testing

- **Unit (render):** a `ShellState` with `active_tool` set and
  `tool_started_at = Some(Instant::now())` renders the tool indicator at
  the ⅓ anchor (within a few columns of W/3); with `tool_started_at` far in
  the past, renders at steady-state color. Same for compaction at ⅔.
- **Unit (layout):** "working" is at W/2 in all four states (idle, tool,
  compaction, all three) — assert its left edge is
  `(W - working_w) / 2` in each. This is the jitter regression test.
- **Unit (spacing):** the gap between the tool indicator's right edge and
  "working"'s left edge is 4 (and same on the compaction side) when both
  are present on a wide-enough terminal.
- **Unit (narrow):** at 40 columns, the tool indicator truncates to
  `◐ {name}` (no args suffix); "working" stays at ½; no overflow.
- **Snapshot:** update `active_tool_spinner_frame` (tool at ⅓, pulse
  steady-state in the snapshot since `Instant::now()` isn't deterministic —
  the snapshot tests `ShellState::new()` which has `tool_started_at = None`,
  so no pulse). Add a compaction snapshot with `compacting = true`.
- **Unit (reduced motion):** with `reduced_motion = true`, the pulse is
  skipped even with a fresh `tool_started_at`.

## Open Questions for Implementation

(Resolved during brainstorming; recorded here so the plan doesn't re-litigate.)

- **Layout principle:** "working" dead-center, others at fixed anchors
  (not a centered cluster, not a right-rail panel).
- **Anchor points:** ⅓ and ⅔ (A2, halfway — tighter than ¼/¾, closer to
  center).
- **Spacing:** fixed 4-space gap (B, moderate — not tight, not
  fill-available).
- **Animation:** only "working" spins; tool and compaction pulse on appear
  (C — bright for 300ms, then steady-state). The compaction 6-frame spinner
  is retired.
- **Tool detail room:** ~120px at the ⅓ anchor on a standard terminal;
  truncates to `◐ {name}` below ~40 columns.