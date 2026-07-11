# Live-Edge System-Prompt Re-Assertion ("Re-Floor")

> **Status:** design (ready for implementation planning). Adds an interval-gated
> re-injection of the system prompt at the *live edge* of the request to counter
> observed instruction-drift in long sessions. Revised after a technical review
> (2026-07-11) that caught a blocking trigger defect (B1) and a per-provider
> placement defect (S1); both are resolved below.
>
> Builds on the request-assembly path in `crates/zoid/src/agent.rs`
> (`build_request_with_thinking`, `run_turn_inner`, `preflight_gate`,
> `record_compactions`), the context projection in
> `crates/zoid-core/src/context.rs`, the eviction machinery in
> `crates/zoid-core/src/eviction.rs`, and the per-adapter request builders in
> `crates/zoid-provider/src/{anthropic,openai_compat,ollama}.rs`.

## Goal

In long sessions the agent measurably drifts from its initial system-prompt
instructions (observed, not theoretical — e.g. the "close with a short recap,
don't re-explain the whole effort" directive decays). Re-assert the operating
instructions near the generation point at a controlled cadence, so adherence is
restored without materially inflating token cost, portably across all providers
(Anthropic, zai / OpenAI-compat, Ollama-native), and — critically — **continuing
to fire in steady state**, not just during session warm-up.

## Background — why the front copy is not enough

The Chat system prompt (`SYSTEM_PROMPT`, wrapped into `TurnConfig.system`) is
**already re-sent verbatim on every request**, but always at the *front*:
Anthropic's top-level `system` block (1h cache breakpoint, `anthropic/cache.rs`);
`messages[0] = {"role":"system"}` on openai-compat (`openai_compat.rs:71`) and
ollama (`ollama.rs:21`). Re-injecting *there* is a no-op. Only the **tail (live
edge)** affects recency. On zai/GLM the drift is worse than on Anthropic: those
models weight a `role:"system"` message less than Anthropic weights its dedicated
`system` param, and open models decay instruction-following faster over context.

zoid has **no** live-edge re-assertion today. The eviction breadcrumb
(`eviction.rs:89`) is front-only and only advertises `recall()`; `recall` is
model-pull, not system-push.

## Design overview

An interval-gated **tail injection** ("re-floor"): every *N estimated tokens of
novel content processed*, append the full system prompt — verbatim, wrapped as a
"standing reminder" — at the live edge of the next request. Policy (whether/what)
is decided centrally; placement (where at the tail) is per-adapter.

| Aspect | Decision |
|---|---|
| Mechanism | Interval-gated tail injection; front/system copy unchanged |
| Trigger | **Cumulative estimated *appended* tokens** (monotonic, compaction-aware; see Component B) ≥ `interval` since last re-floor. NOT the eviction-bounded `context_window` total, and NOT a naive re-sum of live bodies (compaction empties those). |
| State | Persisted weightless `DirectiveReasserted { at_cumulative }` marker; reminder text itself is ephemeral (request-only) |
| Content | Full `config.system` (prompt + skill menu) verbatim, wrapped in a "standing reminder, not a completion signal" bookend |
| Placement | Neutral `CompletionRequest.reassert` field; **per-adapter** rendering (Anthropic vs openai-compat/ollama differ — see Component A) |
| Config | `[economy].reassert_interval_tokens`, default **100_000** (estimated-appended units), `0` disables, global, off for subagents |
| Cost/preflight | Reminder tokens folded into the turn's overhead so `preflight_gate` sizes the real request; marker emitted **after** a successful send |
| Observability | Re-floor fires surfaced in the transcript (acceptance is manual/empirical) |

## Component A — Policy/rendering boundary (per-adapter)

`CompletionRequest` gains one provider-neutral field carrying *intent*:

```rust
pub struct CompletionRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub reassert: Option<String>, // NEW: fully-wrapped reminder text, or None
    // ... existing fields ...
}
```

- **Policy (central, `agent.rs`)** decides *whether* to re-floor and *what* text;
  sets `req.reassert`. Single source of the trigger.
- **Rendering (per-adapter `build_body`)** decides *where* `reassert` lands.
  **This differs by provider** — the review (S1) established that "append to the
  last message content" is an Anthropic-specific constraint that would bury the
  reminder inside a `role:"tool"` payload (weakest salience) on exactly the
  providers this targets:
  - **Anthropic** (`anthropic/request.rs`): append the wrapped reminder as a
    trailing text block onto the **last message's content**. The tail at build
    time is always a `User` message (a `UserMessage` or tool-results ridden
    inside a `User` message, `request.rs:142-159`) — never an assistant-final —
    so this preserves strict alternation and adds no consecutive same-role turn.
  - **openai-compat (zai)** (`openai_compat.rs`) and **Ollama-native**
    (`ollama.rs`): push a **trailing `{"role":"system"}` message**. Neither
    enforces alternation and both accept a system message at any position, where
    it is weighted as instruction rather than as tool output.
  - `reassert = None` → each adapter early-returns to today's exact body
    (byte-identical; regression-tested).

Rationale: `req.reassert` keeps `CompletionRequest` a provider-neutral contract
carrying intent, while the placement mechanism that maximizes salience lives in
each adapter (and is unit-tested there).

## Component B — Trigger & marker (the B1 fix)

**Why not the context window.** The original design keyed on
`context_window(...).total_tokens`. Production Chat runs eviction **enabled**
(`main.rs:5300`: `enabled = compact_threshold_pct > 0`, default 80), and eviction
bounds that quantity in a band around `context_target`. So in steady state the
delta saturates below the interval and the trigger **goes dormant for the rest of
a long session** — the exact regime it exists to serve. The trigger must key on a
monotonic quantity eviction cannot claw back.

**Chosen quantity: cumulative estimated *appended* tokens (compaction-aware).**
Sum of `estimate_tokens` over every content-bearing event in the **raw** log —
`UserMessage`, assistant text (`ModelDelta` / `AssistantMessage`), and
`ToolResult` (incl. file-read output) — *without* the evicted-filter. This
quantity is:

- **Monotonic**, but this requires care against BOTH context levers, not just
  eviction:
  - *Eviction* marks events evicted (`evicted_ids`) but never deletes them — the
    log is append-only (`eventlog.rs`), so evicted events still count. ✅
  - *Compaction (#6b)* is the trap: `EventLog::clear_tool_output`
    (`eventlog.rs:50-58`) **empties a compacted `ToolResult`'s `output` in
    place**, fired live on every compaction (`main.rs:2643`) and on resume via
    `clear_compacted_bodies` (`main.rs:1810`, `:3639`). A naive re-sum of live
    bodies would therefore *drop* on compaction — resurrecting B1 via compaction.
  - **Fix:** the sum is *compaction-aware*. For any `ToolResult` that has a
    matching `ToolResultCompacted`, count its preserved `original_tokens`
    (`compaction.rs:206` = `it.tokens`, the exact pre-clear `estimate_tokens`
    value) instead of the live (possibly-emptied) body. Clearing is *always*
    paired with a `ToolResultCompacted`, so before clear a `ToolResult`
    contributes `estimate_tokens(body)` and after clear it contributes
    `original_tokens` — the same number. The sum never decreases, across live
    compaction *and* resume, with **no new persistence** (still a pure function
    of the append-only log).
- **Novel content only** (cached prefix re-reads are never re-counted) — the
  provider-independent realization of "cumulative non-cached tokens." Works
  identically on zai and GLM-via-Ollama (which reports 0 input on cached prompts
  — the `calibration_ratio` problem — making a provider-reported-uncached trigger
  unreliable there).
- **Estimated units** (`estimate_tokens` is `chars/3`, `economy.rs:43`). The
  interval is denominated in these estimated tokens; the default is chosen
  empirically (Component D).

New event (weightless):

```rust
// zoid-core::event::EventKind
DirectiveReasserted { at_cumulative: u64 },
```

It is **not** a message/tool/file event, so it is invisible to both
`context_window` (`_ => {}` arm, context.rs:295) *and* to the cumulative-appended
sum — self-consistent. Per review S4 it must **also** be added to
`eviction::is_inert()` (eviction.rs:220-231) so it does not join a turn's
evictable id-set in `group_turns`, and `projection::conversation` + the TUI
render must ignore it via their `_` arms.

Pure decision helper (`zoid-core`):

```rust
/// Cumulative estimated tokens of appended (novel) content in the raw log.
/// Compaction-aware: a ToolResult whose body was cleared by #6b is counted at
/// its preserved `original_tokens`, so the total never decreases.
pub fn cumulative_appended(events: impl IntoIterator<Item = &Event> + Clone) -> u64 {
    // First pass: id -> original_tokens for every compacted tool result.
    let orig: HashMap<&str, u64> = events.clone().into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, original_tokens, .. } => Some((id.as_str(), *original_tokens)),
            _ => None,
        }).collect();
    events.into_iter().map(|e| match &e.kind {
        EventKind::UserMessage { text } | EventKind::AssistantMessage { text }
            | EventKind::ModelDelta { text } => estimate_tokens(text),
        EventKind::ToolResult { id, output, .. } =>
            orig.get(id.as_str()).copied().unwrap_or_else(|| estimate_tokens(output)),
        _ => 0, // DirectiveReasserted, Usage, markers, etc. contribute nothing
    }).sum()
}

/// True when >= `interval` estimated-appended tokens have accrued since the last
/// re-assertion (or since session start if none). `interval == 0` disables.
pub fn reassertion_due(events: impl IntoIterator<Item = &Event> + Clone, interval: u64) -> bool {
    if interval == 0 { return false; }
    let last = events.clone().into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::DirectiveReasserted { at_cumulative } => Some(*at_cumulative),
            _ => None,
        })
        .last().unwrap_or(0);
    cumulative_appended(events).saturating_sub(last) >= interval
}
```

Loop wiring in `run_turn_inner`, ordered so cost is accounted and the marker is
honest under the retry path:

```
let will_reassert = config.reassert_interval > 0
    && reassertion_due(events.iter(), config.reassert_interval);
let reassert_text = will_reassert.then(|| wrap_reassertion(&config.system));

// S2: size the request honestly — fold the reminder into overhead for THIS turn.
// `overhead` is a ContextOverhead struct, so add into a field (not the struct).
let mut overhead_now = overhead.clone();
if will_reassert {
    overhead_now.system_tokens += estimate_tokens(reassert_text.as_ref().unwrap());
}
preflight_gate(..., &overhead_now).await?;

let req = build_request_with_thinking(&events, ..., reassert_text.clone());
// ... stream ...

// S2: emit the marker only AFTER a successful send (not on the context-length
// retry path, so a rejected re-floor re-fires next attempt instead of silently
// burning its interval). S3: skip the calibration_ratio update on this sub-turn.
if will_reassert && send_succeeded {
    emit(DirectiveReasserted { at_cumulative: cumulative_appended(events.iter()) });
    ui.send(AgentUpdate::DirectiveReasserted { .. }); // N1 observability
}
```

Properties:

- **Fires in steady state**, because the trigger clock is monotonic and
  independent of eviction. Self-paces on real throughput of novel content.
- **Retry-safe (S2):** the marker is emitted post-send, so a re-floored request
  that trips `is_context_length_error` → forced-eviction retry
  (agent.rs:605-628) does not consume its interval; it re-fires on the rebuild.
- **Preflight-honest (S2):** the ephemeral reminder's tokens are added to the
  turn's overhead estimate, so `preflight_gate` won't under-size a re-floor turn
  and push a "safe" request over the real ceiling.
- **Calibration-clean (S3):** the `calibration_ratio` update in
  `record_compactions` is skipped on re-floor sub-turns (numerator would include
  the extra copy, denominator would not — a transient ~5% over-scale otherwise).

## Component C — The wrapper

The raw prompt contains "when a task is done… close with a short recap." Injected
mid-tool-loop that can read as "the task is done *now*." The wrapper is fixed
framing (the only added text); the payload is `config.system` verbatim:

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

Wraps `config.system` (prompt **plus** the appended skill menu), so skill
availability is re-asserted too; a mode-swapped `AgentProfile` prompt swaps with
it for free. The early-termination mitigation is a genuine but unproven gamble —
hence the observability in N1/Component B and manual acceptance below.

## Component D — Config

Fold into `[economy]`, reusing the "0 disables" convention:

```rust
pub struct EconomyConfig {
    // ... existing ...
    /// Re-assert the system prompt at the live edge every N estimated-appended
    /// tokens of novel content. 0 disables. Default 100_000. Units are
    /// estimate_tokens (chars/3, economy.rs); tune empirically per model.
    pub reassert_interval_tokens: u64,
}
// Default → reassert_interval_tokens: 100_000
```

`TurnConfig` carries the resolved `reassert_interval: u64` (Chat from
`[economy]`; **subagents/tests pass `0`** — off, consistent with
`eviction: disabled()`).

Defaults & rationale:

- **100_000 estimated-appended tokens.** Chosen empirically: on a very long real
  session (user observed ~22M real uncached tokens) this yields on the order of
  tens of re-floors — frequent enough to counter drift, rare enough that the
  amortized cost of the full-prompt copy is negligible. The exact estimated↔real
  ratio is model-dependent (`estimate_tokens` is `chars/3`); treat the default as
  a starting point and tune via the transcript markers (Observability).
- **Enabled by default** (non-zero): drift is observed on the primary models.
- **Global, not per-provider.** Anthropic benefits least (privileged cached
  system block) but re-flooring fires rarely and is harmless there; per-provider
  defaults add cost for no real gain (YAGNI).

## Observability (N1)

Because acceptance is manual/empirical, each re-floor must be visible or you
can't tell whether it's firing. The transcript is projected from `ChatMsg`, and
`DirectiveReasserted` is deliberately inert in `conversation()` (never a
`ChatMsg`) — so observability follows the codebase's existing **bookkeeping-status
idiom** (how `CompactionStarted`/`CompactionComplete` surface), NOT a transcript
line:

- `agent.rs` emits `tracing::info!(kind = "reassert", at = at_cumulative, …)` at
  each fire — the primary greppable signal for manual acceptance/tuning.
- A new `AgentUpdate::DirectiveReasserted { at_cumulative }` is emitted and
  handled in the bin's `AgentUpdate` match (which is exhaustive — the handler
  MUST land in the same task as the variant), mirroring the compaction handlers:
  it bumps a lightweight session counter / transient indicator, not a bottom-bar
  `status_hint` (which the `SubagentStarted` handler warns overlaps the layout).
- The persisted `DirectiveReasserted` events are themselves the durable record of
  when/where fires happened (inspectable in the session log).

A richer economy-view indicator is possible later but is out of scope here.

## Testing

1. **Pure trigger (`zoid-core`):** `cumulative_appended` ignores
   `DirectiveReasserted`; `reassertion_due` false when `interval == 0`, false
   below threshold, true at `>= interval`; a marker resets the baseline (next
   fire only after another `interval` of appended tokens); uses the *last* marker.
2. **Monotonicity under BOTH context levers (the real B1 regression guard):**
   `cumulative_appended` must never decrease when
   (a) `TurnsEvicted` markers are folded in (evicted events still counted), AND
   (b) a `ToolResult` is compacted and its body cleared — i.e. run the log
   through `clear_tool_output` / `clear_compacted_bodies` and assert the total is
   unchanged (the cleared body is counted at its `ToolResultCompacted`'s
   `original_tokens`). This is the case that eviction-only tests miss and that
   reopened B1; it must exercise the *cleared* log, not just eviction markers.
   Then assert `reassertion_due` keeps firing at each interval as the log grows
   well past `context_target` — proving no window/compaction-driven dormancy.
3. **Weightless/inert marker:** `context_window` total unchanged by a
   `DirectiveReasserted` event; `eviction::is_inert()` returns true for it;
   `group_turns` does not attach it to a turn's id-set.
4. **Per-adapter rendering (`zoid-provider`, mirrors `body_has_*` tests):**
   - Anthropic: with `reassert = Some(..)` the reminder is a trailing text block
     on the last **user** message; alternation stays valid after a tool-result
     tail; no consecutive same-role turn.
   - openai-compat & ollama: the reminder is a trailing `{"role":"system"}`
     message (not merged into a `role:"tool"` payload).
   - all three: `reassert = None` → body byte-identical to today (explicit
     early-return).
5. **Preflight accounting (S2):** on a re-floor turn, the size handed to
   `preflight_gate` includes the reminder tokens.
6. **Loop integration (`zoid`):** drive `run_turn_inner` with a fake provider
   over a long log; assert a `DirectiveReasserted` is emitted ~once per interval
   of appended growth, that the fired request carried the reminder, and that a
   context-length error on a re-floor turn does **not** emit the marker (re-fires).
7. **Wrapper framing:** `wrap_reassertion` output contains the "not a completion
   signal / resume the task" framing around the verbatim system prompt.

**Acceptance is empirical.** Unit tests cannot prove drift is reduced or that GLM
won't wrap up early. Real acceptance: a long zai / glm-5.2 session with the
transcript re-floor markers (N1) visible, tuning `reassert_interval_tokens`.

## Non-goals

- Distilled/hand-maintained directive subsets (chose full-prompt verbatim).
- Persisting the reminder text into history (chose ephemeral to avoid pile-up).
- Provider-reported-uncached or context-window-growth triggers (chose
  provider-independent estimated-appended; see Component B).
- Turn/sub-turn-count triggers (decouples from token pressure).
- Per-provider interval defaults; clamping for small-window models.
- Re-asserting anything for subagents.
