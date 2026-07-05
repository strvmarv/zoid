# EventLog: cheap-clone + clearable event log (#6a + #6b) — Design

**Date:** 2026-07-04
**Status:** Approved (brainstorm), ready for implementation plan
**Targets:** backlog #6a (per-turn `app.events` deep clone) + #6b (free compacted `ToolResult.output` bodies from RAM)
**Line refs synced against:** `f50191b` (post-ACM-follow-ups main). All correctness-critical seams (`main.rs:2850/2802/916`, `eviction.rs:189`, `context.rs:254/265/315`, `projection.rs:135-137`) verified unchanged; supporting line numbers refreshed.

## Problem

Two related working-set costs survive the ACM demand-paged-context feature (which shipped Slices 1+2: turn-eviction skip-in-fold + FTS recall; Slice 3 "cold-paging" is unshipped):

1. **#6a — per-turn deep clone.** `spawn_turn` does `let seed = app.events.clone()` (`crates/zoid/src/main.rs:2850`; a second clone at the subagent spawn path `main.rs:2802`). `App.events: Vec<Event>` and `Event: Clone`, so this deep-copies every `String` body — including every `ToolResult.output` — once per turn. The returned `Vec<Event>` from the turn is discarded (`let _ = run_agent_turn_cancellable(...)`); new events flow back via the `session` actor + `AgentUpdate::Appended` UI channel. The clone exists solely to hand the spawned task an owned `'static` snapshot for request-building and in-turn projection recomputes. Cost grows O(total tool-output chars) per turn, unbounded.

2. **#6b — raw bodies resident forever.** `EventKind::ToolResult { output: String, .. }` (`crates/zoid-core/src/event.rs:60-65`) is never cleared. `ToolResultCompacted { id, summary, original_tokens }` (`event.rs:80-84`) and `TurnsEvicted` (`event.rs:112-116`) are append-only/reversible — consumers only skip evicted ids or substitute summaries at request-build time. So a compacted tool result keeps its full raw body live in RAM (and it is re-cloned every turn per #6a).

ACM does **not** address either: it changed neither the clone nor the resident raw bodies (confirmed by code survey).

## Goal

- Make the per-turn snapshot O(n) refcount bumps instead of O(total bytes) — no body copies.
- Free the raw `ToolResult.output` of **compacted** results from the in-memory log, safely.
- Stay pragmatic: do **not** implement ACM Slice 3 (dropping evicted events from the Vec, windowed resume load). That is a separate, deferred effort.

## Non-Goals (YAGNI / scope guard)

- No dropping of evicted events from the in-memory Vec (cold-paging).
- No windowed / lazy resume load.
- No clearing of **uncompacted** evicted bodies — a readmitted uncompacted turn is rendered from its raw body, so clearing it would break readmit-render.
- No change to the on-disk SQLite store contents (raw bodies stay persisted; recall depends on them).

## Architecture

### Core type: `EventLog` (in the `zoid` bin)

Introduce a newtype in the bin (where the mutable, effectful log lives) — **not** in zoid-core, so the core keeps its pure `&[Event]` projection contract:

```rust
/// The in-memory event log. Wraps `Vec<Arc<Event>>` so that:
/// (a) handing a turn its snapshot is O(n) refcount bumps, not O(total bytes)
///     of body copies (#6a); and
/// (b) an individual event's body can be swapped out in place — replace the
///     `Arc` slot — without disturbing snapshots already handed to in-flight
///     turns (they hold the old, immutable `Arc`) (#6b).
pub struct EventLog(Vec<Arc<Event>>);

impl EventLog {
    pub fn push(&mut self, e: Event);                 // self.0.push(Arc::new(e))
    pub fn iter(&self) -> impl Iterator<Item = &Event>; // self.0.iter().map(|a| &**a)
    pub fn len(&self) -> usize;
    pub fn snapshot(&self) -> EventLog;               // self.0.clone() — refcount bumps only
    /// #6b: replace the `ToolResult` with `id`'s slot with an Arc whose
    /// `output` is empty. No-op if `id` is absent or not a ToolResult.
    pub fn clear_tool_output(&mut self, id: Ulid);
}
```

`App.events: EventLog`. The `spawn_turn` seed (`main.rs:2850`) and the subagent seed (`main.rs:2802`) become `app.events.snapshot()` — the **#6a fix**.

### Feeding zoid-core projections (contained ripple)

zoid-core's projection entry points — `conversation`, `context_window` (`context.rs`), and `plan_evictions` / `evicted_ids` (`eviction.rs`) — currently take `&[Event]`. They shift to accept borrowed events without an `Arc` dependency, via one of:

- **Preferred:** change the signatures to `impl IntoIterator<Item = &Event>` (or a generic `E: AsRef<[…]>`-style bound is not possible over `Arc`, so an iterator bound is the clean choice). `EventLog::iter()` feeds them directly, no per-turn allocation.
- The turn's in-loop recomputes (`agent.rs` around the tool loop, e.g. the `events.push` at `agent.rs:1116`) operate on a local `EventLog` seeded from the snapshot; appends push `Arc::new(ev)`.

This is the only signature ripple: ~3 core functions plus their unit tests. The bin's callers change from passing `&events` to `events.iter()`.

### #6b: clearing compacted bodies

**Trigger.** In the App/UI update loop, when an appended event is `ToolResultCompacted { id, .. }`, call `app.events.clear_tool_output(id)`. The corresponding `ToolResult`'s slot is replaced by an `Arc<Event>` with `output: String::new()`. Any snapshot already handed to an in-flight turn holds the old `Arc` and is unaffected (immutable share).

**Reader redirects (correctness-critical).** After clearing, the only code that still reads the raw compacted body must not depend on it:

- `context.rs` — `context_window` computes each item's tokens via `estimate_tokens(output)` (`context.rs:254/265`) and then **overrides** compacted items to the summary's token count (`context.rs:315`). Because the result is overridden for compacted ids, a cleared body (→ `estimate_tokens("") ≈ 0`) does not change the final number. Correctness already holds; as a cleanup, skip the now-wasted pre-override estimate for ids known to be compacted.
- `eviction.rs` — the per-event size helper `event_tokens` (`eviction.rs:182`) estimates a `ToolResult`'s tokens from raw `output` (`eviction.rs:189`) with **no** override, and `plan_evictions` (`eviction.rs:250`) ranks turns by that helper. Critically, `ToolResultCompacted` is itself listed in `is_inert` (`eviction.rs:174`) → it contributes 0 tokens and forms no turn, so a compacted turn's **entire** eviction-ranking weight flows solely through its underlying `ToolResult` at `:189`. A cleared body would therefore make a compacted turn count as ~0 tokens and mis-rank eviction (it would look "free" to evict, or conversely never be prioritized). **Fix:** in `event_tokens` (or its caller), for an id that has a `ToolResultCompacted`, account its in-context size as `estimate_tokens(summary)` — matching `context.rs:315`, i.e. the number actually present in the request. **Not** the raw (now-empty) body, and **not** `original_tokens` (that field is the *pre-compaction raw* count, used for reclaimed-tokens reporting — using it here would over-count a compacted turn by its full raw size and skew eviction ranking).
- `projection.rs` — `conversation()` renders the **summary** for a compacted id (`projection.rs:136`) and only reads raw `output` in the non-compacted branch (`projection.rs:135-137`). No change needed; a cleared compacted body is never rendered.
- FTS index (`store.rs`) writes `{name}\n{output}` at **append** time (persisted copy) — not a per-turn in-memory read. Unaffected.
- Recall / readmit (recall tool around `agent.rs:675-725`, the session-scoped `session.recall(query, session_id, limit)` at `agent.rs:683`; `render_recalled` at `agent.rs:918`) read the raw body from **SQLite** via `SessionHandle::recall()`, not the in-memory Vec. The ACM follow-up `8f92263` further scopes `events_by_ids` to `session_id` (defense-in-depth), so recall never touches the hot `EventLog`. Unaffected.

**Resume path.** When a session is opened and its persisted log materializes into `App.events`, apply the same clear: for every `ToolResult` whose id has a matching `ToolResultCompacted` in the loaded log, load it with an empty body (or clear immediately after load). Without this, reopening a long session re-inflates RAM to the pre-#6b footprint. The exact load site is located during planning (the session-open/resume path that builds `App.events` from the store).

### Safety argument (why clearing is sound)

Clearing is scoped to **compacted** bodies only. For a compacted id:
- `conversation()` always renders the summary — the raw body is never a render source again (`projection.rs:136`).
- There is **no un-compact / restore path** in the codebase (verified) — a summary is never expanded back to raw in memory.
- Recall and readmit source the raw body from SQLite, not memory.
- The two token estimators are redirected (above).

Therefore, once the redirects land, the in-memory raw compacted body has **no surviving reader**, and clearing it cannot change rendered output, recall, readmit, or token accounting. Uncompacted evicted bodies are explicitly left intact (readmit renders them raw).

## Data flow (end to end)

1. Turn starts → `spawn_turn` hands the task `app.events.snapshot()` (refcount bumps).
2. Turn builds request via zoid-core projections over the snapshot; appends tool-call/tool-result events locally and via the session actor.
3. Session compacts a tool result → emits `ToolResultCompacted` → UI loop appends it to `App.events` and calls `clear_tool_output(id)`.
4. Subsequent turns snapshot a log whose compacted bodies are empty → smaller RAM, and (already) cheap snapshot.
5. `plan_evictions` accounts compacted turns by their in-context size — `estimate_tokens(summary)`, matching `context.rs:315`.
6. Resume load clears compacted bodies so RAM does not re-inflate.

## Testing

- **#6a:** `snapshot()` shares — `Arc::ptr_eq` on corresponding elements and `Arc::strong_count` increments after snapshot; no body is deep-copied. A representative event log with large bodies snapshotted N times shows no growth in body allocations (assert via ptr equality, not timing).
- **#6b core:** `clear_tool_output(id)` empties exactly the target `ToolResult.output` and leaves all other events (and non-matching ids) intact; no-op for absent id or non-`ToolResult` id.
- **#6b render regression:** a compacted + cleared event still renders its **summary** through `conversation()` (unchanged output).
- **#6b eviction accounting:** `plan_evictions` weighs a compacted-cleared turn by `estimate_tokens(summary)` (matching `context.rs:315`), not 0 and not the raw `original_tokens` (guards the redirect).
- **#6b resume:** loading a persisted log with a `ToolResult` + matching `ToolResultCompacted` yields an in-memory log whose that body is empty.
- **Whole workspace:** `cargo test` green; existing snapshot tests unchanged (rendered conversation output is unaffected).

## Global Constraints (for the plan)

- zoid-core stays pure — `EventLog` and the `Arc` sharing live in the `zoid` bin; the core keeps a borrowed-`&Event` (iterator) projection API.
- Do not alter on-disk store contents, recall, or readmit behavior.
- Clear **only** compacted bodies; never uncompacted evicted bodies.
- `plan_evictions` must account compacted turns by in-context size (`estimate_tokens(summary)`, matching `context.rs:315`), not raw, not zero, and not the pre-compaction `original_tokens`.
- Commit messages: NO `Co-Authored-By` / co-author trailer (user rule).
- Final gate: `cargo test` (workspace) green; introduce no new clippy warnings in touched code (repo is not clippy/fmt-clean at baseline — bar is "no new issues in feature-touched files").

## Open items deferred to planning

- Exact zoid-core signature form (`impl IntoIterator<Item=&Event>` vs a helper) — pick the one with least call-site churn while avoiding per-turn allocation.
- Exact resume/session-open load site that builds `App.events`.
- Whether the subagent seed (`main.rs:2802`) needs the same snapshot treatment (it does — same `.snapshot()` swap).
