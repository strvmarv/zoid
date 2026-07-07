# Compaction status animation — design

**Source:** user request — a new animated indicator in the status bar, next to
"idle"/"working", that displays only while automated compaction is running.
Purple (`color::BRANCH`), animated, placed right of the activity indicator
without re-centering.

## Problem

Compaction runs synchronously inside the agent loop, emitting a burst of
`ToolResultCompacted` events via `AgentUpdate::Appended`. There's no visible
signal to the user that compaction is happening — it looks like the turn is
just "working" with no distinction between streaming and the compaction phase.

## Design

### 1. New `AgentUpdate` variants

Add `CompactionStarted` and `CompactionComplete` to `AgentUpdate` in
`crates/zoid/src/agent.rs`:

```rust
/// Automated compaction is running (before a burst of ToolResultCompacted events).
CompactionStarted,
/// Automated compaction finished (after the burst).
CompactionComplete,
```

The agent loop emits them inside `record_compactions` and `preflight_gate`,
gated on `!plan.compactions.is_empty()` — they only fire when compaction
actually runs. If the plan is empty (no compactions needed), the pair is
not emitted, and the UI no-ops.

### 2. New glyph token

In `crates/zoid-tui/src/tokens.rs`, add to `mod glyph`:

```rust
/// Compaction status spinner — a 6-frame box-shuffle ramp, animated at ~120ms
/// (slower than the working spinner, signaling a different kind of work).
/// Purple (color::BRANCH). Only shown while automated compaction is running.
pub const COMPACT_SPINNER: [char; 6] = ['⊟', '⊞', '⊟', '⊕', '⊞', '⊕'];
```

### 3. New `ShellState` fields

Two new fields on `ShellState` in `crates/zoid-tui/src/state.rs`:

- `pub compacting: bool` — `true` while compaction is running. Default `false`.
- `pub compact_spinner: char` — current animation frame. Default
  `glyph::COMPACT_SPINNER[0]`.

### 4. Bin wiring (`main.rs`)

**Minimum display duration (3s debounce).** Compaction can finish in
milliseconds — a flash too fast to perceive. The indicator stays visible for
at least 3 seconds from `CompactionStarted`, even if `CompactionComplete`
arrives sooner. Implementation: the bin stores a
`compaction_started_at: Option<std::time::Instant>` on `App` (not `ShellState`
— this is bin-level timing). On `CompactionStarted`, it sets
`app.shell.compacting = true` and records `Instant::now()`. On
`CompactionComplete`, it does NOT immediately clear `compacting` — instead it
sets a `compaction_complete: bool` flag on `App`. In the per-frame loop, when
`compaction_complete` is true and 3 seconds have elapsed since
`compaction_started_at`, the bin clears `app.shell.compacting = false` and
resets both fields. The motion tick guard (below) already wakes while
`compacting` is true, so the debounce timer drains without an extra wake
source.

**Per-frame compact spinner** — in `run()`, alongside the existing
`app.shell.spinner` assignment:

```rust
app.shell.compact_spinner = zoid_tui::tokens::glyph::COMPACT_SPINNER
    [zoid_tui::motion::spinner_frame(elapsed, 120, 6, app.shell.reduced_motion)];
```

**Per-frame debounce check** — in `run()`, after the spinner assignment, check
the minimum display duration:

```rust
if app.compaction_complete {
    if let Some(start) = app.compaction_started_at {
        if start.elapsed() >= std::time::Duration::from_secs(3) {
            app.shell.compacting = false;
            app.compaction_complete = false;
            app.compaction_started_at = None;
        }
    }
}
```

**`AgentUpdate` handler** — two new arms in the `ui_rx.recv()` match:

```rust
AgentUpdate::CompactionStarted => {
    app.shell.compacting = true;
    app.compaction_started_at = Some(std::time::Instant::now());
    app.compaction_complete = false;
}
AgentUpdate::CompactionComplete => {
    app.compaction_complete = true;
    // Don't clear app.shell.compacting here — the per-frame debounce
    // check clears it once the 3s minimum has elapsed.
}
```

**New `App` fields:**

```rust
/// When compaction started (for the 3s minimum-display debounce). None when
/// no compaction is in flight or the debounce has cleared.
compaction_started_at: Option<std::time::Instant>,
/// CompactionComplete arrived; the indicator stays visible until the 3s
/// minimum display duration elapses (checked per-frame).
compaction_complete: bool,
```

**Motion tick guard** — expand to wake while compaction is running:

```rust
_ = motion_tick.tick(), if app.streaming || app.delegating || app.shell.compacting || app.zoom_changed_at.is_some() => {
```

### 5. `render_status` change

After the center segment is pushed, when `state.compacting` is true, append
the compaction segment directly — no re-centering math changes. It adds spans
after the center, before the right padding:

```rust
if state.compacting {
    spans.push(Span::styled(
        format!("  {} compacting", state.compact_spinner),
        Style::new().fg(color::BRANCH),
    ));
}
```

The `pad2` calculation (centering the right zoom hint) already uses
`right_start.saturating_sub(left_w + pad1 + center_w)`. The compaction segment
adds width after `center_w`, which means `pad2` shrinks. The zoom hint stays
pinned to the right edge. On a narrow terminal, the compaction segment eats
into the padding — it clips gracefully (saturating math).

**Layout** (when compacting during a turn):
```
CHAT · ...    ⠋ working  ⊞ compacting    ... zoom normal
```

When not compacting, the segment is absent — the status bar looks exactly as
it does today.

### 6. Agent loop emission sites

In `agent.rs`, wrap the compaction sections with `CompactionStarted` /
`CompactionComplete`:

**`record_compactions`** (~line 1186): emit `CompactionStarted` before the
compaction loop and `CompactionComplete` after it, **gated on
`!plan.compactions.is_empty()`** — no emission when the plan is empty. The
emission lives inside `record_compactions` itself (not at the call sites),
so both call sites (~line 568 and ~line 1077) need no changes.

**`preflight_gate`** (~line 1255): emit `CompactionStarted` before the
compaction loop and `CompactionComplete` after it, **gated on the existing
`compacted` variable** (`!plan.compactions.is_empty()`). If the compaction
section is skipped (estimate below threshold) or the plan is empty, the
pair is not emitted.

### 7. Testing

- **Token test** (`tokens.rs`): `assert_eq!(glyph::COMPACT_SPINNER, ['⊟', '⊞', '⊟', '⊕', '⊞', '⊕']);`
- **State test** (`state.rs`): `compacting_defaults_false` — `ShellState::new().compacting` is `false`.
- **Render test**: a pure test verifying the compaction segment appears in the status bar spans when `state.compacting` is true, and doesn't when false.
- **Agent loop test**: `economy_integration.rs` already runs a turn with compaction — add assertions that `CompactionStarted` and `CompactionComplete` are received in the update stream.

### 8. Scope

| Change | File | Size |
|--------|------|------|
| New `AgentUpdate` variants | `crates/zoid/src/agent.rs` | ~6 lines |
| Emit start/complete around compaction | `crates/zoid/src/agent.rs` (2 sites) | ~8 lines |
| New `COMPACT_SPINNER` token | `crates/zoid-tui/src/tokens.rs` | ~3 lines + test |
| New `ShellState` fields | `crates/zoid-tui/src/state.rs` | ~8 lines + test |
| New `App` fields (debounce) | `crates/zoid/src/main.rs` | ~6 lines |
| Per-frame compact spinner + debounce check | `crates/zoid/src/main.rs` | ~12 lines |
| `AgentUpdate` handler arms | `crates/zoid/src/main.rs` | ~10 lines |
| Motion tick guard | `crates/zoid/src/main.rs` | ~1 line |
| `render_status` compaction segment | `crates/zoid-tui/src/render.rs` | ~5 lines |
| Tests | various | ~25 lines |