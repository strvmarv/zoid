# Live-Edge System-Prompt Re-Assertion ("Re-Floor")

> **Status:** design (ready for implementation planning). Adds an interval-gated
> re-injection of the system prompt at the *live edge* of the request to counter
> observed instruction-drift in long sessions. Builds on the request-assembly
> path in `crates/zoid/src/agent.rs` (`build_request_with_thinking`,
> `run_turn_inner`), the context projection in
> `crates/zoid-core/src/context.rs`, the eviction-breadcrumb pattern in
> `crates/zoid-core/src/eviction.rs`, and the per-adapter request builders in
> `crates/zoid-provider/src/{anthropic,openai_compat,ollama}.rs`.

## Goal

In long sessions the agent measurably drifts from its initial system-prompt
instructions (observed behavior, not just theoretical — e.g. the "close with a
short recap, don't re-explain the whole effort" directive decays). Re-assert the
operating instructions near the generation point at a controlled cadence, so
adherence is restored without materially inflating token cost, and portably
across all providers (Anthropic, zai / OpenAI-compat, Ollama-native).

## Background — why the front copy is not enough

The Chat system prompt is a compile-time constant (`SYSTEM_PROMPT`, `agent.rs`)
wrapped into `TurnConfig.system` once per turn. It is **already re-sent verbatim
on every provider request**, but always at the *front*:

- **Anthropic** — top-level `system` param (`Vec<SystemBlock>`), with a 1h
  ephemeral cache breakpoint (`anthropic/cache.rs`).
- **zai (OpenAI-compat)** and **glm-5.2 (Ollama-native)** — a leading
  `{"role":"system"}` message at `messages[0]` (`openai_compat.rs:71`,
  `ollama.rs:21`).

Because it is always present at the front, re-injecting it *there* is a no-op.
The only position that affects recency/salience is the **tail (live edge)**.
Drift is driven by how many tokens accumulate *between* the front instruction and
the generation point; on zai/GLM the effect is worse than on Anthropic because
those models weight the `role:"system"` message less than Anthropic weights its
dedicated `system` param, chat templates may collapse system into the first user
turn, and open models decay instruction-following faster over context.

zoid currently has **no** live-edge re-assertion path. The eviction breadcrumb
(`eviction.rs:89`) is appended to the *system field* (front) and only advertises
`recall()`; the `recall` tool is model-pull, not system-push. Neither restates
behavioral directives near the live edge.

## Design overview

An interval-gated **tail injection** ("re-floor"): every *N tokens of context
growth*, append the full system prompt — verbatim, wrapped as a "standing
reminder" — onto the live edge of the next request. Policy (whether/what) is
decided centrally; placement (where at the tail) is per-adapter.

| Aspect | Decision |
|---|---|
| Mechanism | Interval-gated tail injection; front/system copy unchanged |
| Trigger | Token-distance ≥ `interval` since last re-floor, measured on post-preflight `context_window(...).total_tokens` |
| State | Persisted weightless `DirectiveReasserted { at_tokens }` marker; reminder text itself is ephemeral (request-only) |
| Content | Full `config.system` (prompt + skill menu) verbatim, wrapped in a "standing reminder, not a completion signal" bookend |
| Placement | Neutral `CompletionRequest.reassert` field; each adapter renders onto the tail message's content (alternation-safe) |
| Config | `[economy].reassert_interval_tokens`, default `50_000`, `0` disables, global, off for subagents |

## Component A — Policy/rendering boundary

`CompletionRequest` gains one provider-neutral field carrying *intent*, not
mechanism:

```rust
pub struct CompletionRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub reassert: Option<String>, // NEW: fully-wrapped reminder text, or None
    // ... existing fields ...
}
```

- **Policy (central, `agent.rs`)** decides *whether* to re-floor and *what* the
  text is; sets `req.reassert`. Single source of the trigger — mirrors how the
  eviction breadcrumb is computed once in `build_request_with_thinking`.
- **Rendering (per-adapter `build_body`)** decides *where* `reassert` lands,
  respecting that provider's message-role rules. All three currently render the
  same way — append onto the **last message's content** — but the field lets a
  future adapter with a genuinely distinct slot diverge without touching policy.

Rationale: keeps `CompletionRequest` honest as the provider-neutral contract and
avoids duplicating the token-distance math into each adapter (which would drift).

## Component B — Trigger & marker

New event (metadata, weightless):

```rust
// zoid-core::event::EventKind
DirectiveReasserted { at_tokens: u64 },
```

`context_window` counts only `Message`/`ToolResult`/`File` kinds, so this marker
falls into the existing `_ => {}` arm and **never inflates the window it
measures** — the invariant that keeps the trigger from feeding back on itself.

Pure decision helper (`zoid-core`):

```rust
/// True when context has grown >= `interval` tokens since the last
/// re-assertion (or since session start if none). `interval == 0` disables.
pub fn reassertion_due(
    events: impl IntoIterator<Item = &Event>,
    current_total: u64,
    interval: u64,
) -> bool {
    let last_floor = events.into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::DirectiveReasserted { at_tokens } => Some(*at_tokens),
            _ => None,
        })
        .last()                // most recent marker; 0 if none
        .unwrap_or(0);
    interval > 0 && current_total.saturating_sub(last_floor) >= interval
}
```

Loop wiring in `run_turn_inner`, ordered against existing gates:

```
preflight_gate(...)                       // may evict → changes total
  ↓
let total = context_window_with(events, overhead).total_tokens;
let reassert = if config.reassert_interval > 0
    && reassertion_due(events.iter(), total, config.reassert_interval) {
        emit(DirectiveReasserted { at_tokens: total })   // advance the floor
        Some(wrap_reassertion(&config.system))           // Component C
    } else { None };
  ↓
build_request_with_thinking(&events, ..., reassert)      // → req.reassert
```

Properties:

- **Runs after `preflight_gate`**, so `total` is post-eviction; if eviction just
  reclaimed space, the delta shrinks and re-flooring is naturally deferred. The
  two context mechanisms compose rather than fight.
- **"Every N tokens of growth," not "once per turn."** A tool-heavy turn adding
  40k tokens with a 50k interval may fire mid-turn; each fire emits a marker that
  advances the floor, so the cadence self-paces and spans the whole session.
- **Marker emitted before the request**, so a mid-turn crash/abort leaves an
  honest floor — on resume we wait for the next interval of growth rather than
  re-firing immediately.

## Component C — The wrapper

The raw prompt contains "when a task is done… close with a short recap." Injected
mid-tool-loop that can read as "the task is done *now*," nudging the model to
wrap up early. The wrapper is fixed framing (the only added text); the payload is
`config.system` verbatim (zero drift):

```rust
fn wrap_reassertion(system: &str) -> String {
    format!(
        "[Standing reminder — your operating instructions below are still in \
         effect. This is a periodic re-statement, NOT a change of task and NOT \
         a signal that anything is complete. Do not alter what you are doing in \
         response to seeing this; continue the current work and keep following \
         these instructions:]\n\n\
         {system}\n\n\
         [End of reminder — resume the task in progress.]"
    )
}
```

Wraps `config.system` (prompt **plus** the appended skill menu from
`chat_turn_config_with`), so skill availability — a real forgetting surface — is
re-asserted too. If a mode swaps the system prompt via `AgentProfile`, the
reminder swaps with it for free.

## Component D — Config

Fold into `[economy]` (sibling of eviction/compaction), reusing the existing
"0 disables" convention (`compact_threshold_pct`):

```rust
pub struct EconomyConfig {
    // ... existing ...
    /// Re-assert the system prompt at the live edge every N tokens of context
    /// growth. 0 disables. Default 50_000.
    pub reassert_interval_tokens: u64,
}
// Default → reassert_interval_tokens: 50_000
```

`TurnConfig` carries the resolved `reassert_interval: u64` (Chat from
`[economy]`; **subagents/tests pass `0`** — off, consistent with
`eviction: disabled()`).

Defaults & rationale:

- **50_000 tokens.** Against the default `context_target` of 300k, ~6 re-floors
  across a full window — frequent enough to counter drift, rare enough that the
  amortized cost of the full-prompt copy is negligible.
- **Enabled by default** (non-zero): drift is observed on the primary models, so
  opt-out rather than opt-in.
- **Global, not per-provider.** Anthropic benefits least (privileged cached
  system block) but re-flooring fires rarely and is harmless there; per-provider
  branching adds cost for no real gain (YAGNI). Tunable/disable-able via config.

Known limitation (documented, not solved): on a small-window model (e.g. 32k) a
50k absolute interval never fires. Acceptable — short-window sessions can't drift
as far — but noted as the one case the absolute default is "wrong."

## Testing

1. **Pure trigger (`zoid-core`):** `reassertion_due` false when `interval == 0`,
   false below threshold, true at `>= interval`; a `DirectiveReasserted` marker
   resets the baseline (next fire only at `2×interval`); uses the *last* marker
   with multiple present.
2. **Weightless-marker invariant:** `context_window` over a log with a
   `DirectiveReasserted` event yields identical `total_tokens` to without it.
   Guards the self-reference (trigger must not inflate its own measurement).
3. **Per-adapter rendering (`zoid-provider`, mirrors `body_has_*` tests):** one
   per adapter (Anthropic, openai_compat, ollama) asserting that with
   `req.reassert = Some(..)` the reminder appears at the **tail** and alternation
   stays valid (no second consecutive user turn after a tool-result). Symmetric
   test: `reassert = None` → body byte-identical to today.
4. **Loop integration (`zoid`):** drive `run_turn_inner` with a fake provider
   over a long log; assert `DirectiveReasserted` emitted ~once per interval of
   growth and that the fired request carried the reminder.
5. **Wrapper framing:** unit-assert `wrap_reassertion` output contains the "not a
   completion signal / resume the task" framing around the verbatim system
   prompt.

**Acceptance is empirical.** Unit tests cannot prove drift is reduced or that
GLM won't terminate early — the real acceptance test is manual validation on a
long zai / glm-5.2 session, with `reassert_interval_tokens` as the tuning knob.

## Non-goals

- Distilled/hand-maintained directive subsets (chose full-prompt verbatim, zero
  maintenance).
- Persisting the reminder text into history (chose ephemeral to avoid pile-up /
  duplicate-block contradiction).
- Fraction-of-window or turn-count triggers (chose absolute token-distance).
- Per-provider interval defaults or clamping for small-window models.
- Re-asserting anything for subagents.
