# ACM Slice-4b — Relevance-Rescued Eviction — Design

> **Status:** DESIGN APPROVED (brainstorming, 2026-07-23). Ready for
> `writing-plans`.
>
> **Parent vision:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md`
> (§4 step 2 "relevance-score files", §6 Tier-1). **Supersedes** the relevance
> half of `docs/superpowers/specs/2026-07-03-acm-2-relevance-and-live-eviction-design.md`
> (see §8). **Builds on the shipped v1 embeddings slice:**
> `docs/superpowers/specs/2026-07-08-acm-local-embeddings-design.md`.

---

## 1. Goal & scope

Give the **already-live, recency-based eviction** a **semantic rescue term** so a
turn that is stale by recency but still on-goal is *protected* from the eviction
wave — the vision's `heat_of(refs, recency) → heat_of(refs, recency, relevance)`
step, realized against the real embedder v1 shipped.

**In scope (v1 of this slice):**
- Populate the reserved `GoalContext` (`eviction.rs`) with a goal vector + cached
  per-event vectors + a rescue weight.
- A **relevance layer inside `plan_evictions`**: rank-normalized max-cosine per
  candidate turn, folded **soft-additive, rescue-only** into the sort key.
- `goal_text(events, n)` — the relevance query from recent non-trivial user turns.
- Store `vectors_by_ids(model_id, ids)` — one batch read of cached embeddings.
- Wire-in at the `preflight_gate` eviction call sites, gated on real pressure
  (`est ≥ high_water`) and embedder presence.

**Deferred (seams honored, not built) — see §8.**

### 1.1 Why this is a small slice now

The v1 embeddings slice and the earlier recency-eviction work already shipped
**~80%** of what the 3-week-old ACM-2 design assumed it would build from scratch:
`band.rs`, `assemble_context`, live pressure-gated eviction (`preflight_gate`,
`agent.rs`), `TurnsEvicted` events, tool-result compaction, the drawer, **and** a
real dense `Embedder` (`CandleEmbedder`, bge-small) with a persisted
`event_embeddings` cache and an async maintenance lane. What remains is a single
clean seam-fill, not the large ACM-2 build.

---

## 2. Evidence base (inherited, already measured)

From the v1 spike (`spikes/embed-rerank-eval/`, and the v1 design §2):

| Fact | Consequence for this slice |
|---|---|
| bge-small cosine: related ≈ 0.81, **unrelated ≈ 0.37** (not zero-centered) | An **absolute** cosine threshold/weight is a calibration trap → we normalize **window-relative (rank-based)**, mirroring v1 recall's deliberate RRF "rank, don't calibrate" choice. |
| One embed ≈ 30 ms; embedding must stay **off the synchronous gate** | We embed the **goal once** per eviction on `spawn_blocking`; candidate-turn vectors come from the **DB cache**, never re-embedded on the gate. |
| Vectors are cached in `event_embeddings` by the async lane (message/tool-result/file kinds) | Turn relevance reads cached vectors; the ring is *not* used here (eviction targets old turns already aged out of the capped FIFO). |

---

## 3. Architecture

### 3.1 The combine rule (decided)

Eviction (`plan_evictions`) drops `protected` turns, sorts the rest **ascending by
score**, and evicts lowest-first until tokens fall below `low_water`. Relevance
folds in as a **soft, additive, rescue-only** term on the sort key:

```
keep_score(turn) = base_score(turn) + weight · normalized_relevance(turn)
base_score       = RecencyScorer::score = turn.index      // higher = keep newer
normalized_relevance ∈ [0, 1]                             // rank-normalized, see §3.3
weight            = DEFAULT_RESCUE_WEIGHT                  // "turns of newness a
                                                          //  perfectly on-goal turn is worth"
```

**Rescue-only is mechanical, not aspirational:** the added term is always `≥ 0`,
so `keep_score(turn) ≥ turn.index` for every turn. No turn is ever ranked *below*
its pure-recency position — relevance can only move a turn **up** the keep order
(protect it), never down (never newly evict it). Relevance leapfrogging one turn
over another (an older on-goal turn kept while a newer off-goal turn is dropped)
is the intended trade, and only ever happens among turns already past `recent_n`.

**No deadlock / no under-fire:** the eviction loop breaks on *tokens*, not score,
so even a rescued (bumped) turn is still evictable under extreme pressure. The
band always reaches `low_water`; `target` stays a preference, not a hard cap.

**Why soft-additive over hard-rescue (threshold → `protected`):** the
`EvictionScorer` seam expresses *ordering* (an `f32`), not *exemption* (a `bool`);
a hard exemption would need `protected` threaded up into `group_turns`. Soft
fits the existing seam with zero new machinery, is provably rescue-only per-item,
can't under-fire the band, and its knob (`weight`, in turn-of-recency units) is
more intuitive than a raw-cosine threshold. The one thing given up — an absolute
"this turn is *never* evicted" promise — is already the job of pinning and
`recent_n`, and is deliberately *not* what relevance should provide.

### 3.2 `GoalContext` becomes the carrier

The reserved-empty `GoalContext {}` (`eviction.rs:191`, commented "Slice-4
relevance context (empty now; keeps the scorer signature stable)") is populated:

```rust
#[derive(Debug, Default)]
pub struct GoalContext {
    /// Goal (query) unit vector; empty ⇒ no relevance term (default = today).
    pub goal: Vec<f32>,
    /// event_id → cached unit vector for candidate-turn events.
    pub vecs: HashMap<Ulid, Vec<f32>>,
    /// Rescue weight in "turns of recency" units.
    pub weight: f32,
}
```

`GoalContext::default()` (empty `goal`) yields **zero bump** ⇒ eviction is
**byte-identical to today's recency behavior**. This is the graceful-degradation
path (feature off / disabled / no weights / no embedder / no cached vectors).

`RecencyScorer` stays the base scorer. **No `RelevanceScorer` impl is added** —
relevance is a layered pass, keeping the projection/scorer embedder-free (cosine
over already-cached vectors is pure math).

### 3.3 Relevance layer inside `plan_evictions`

`plan_evictions` gains a `ctx: &GoalContext` parameter (replacing its internal
`GoalContext::default()` construction). After `group_turns`, **when `ctx.goal` is
non-empty**:

1. For each **candidate** turn (already filtered: `!protected && !ids.is_empty()`),
   compute `raw = max over turn.ids of cosine(ctx.goal, ctx.vecs[id])`. Ids with no
   cached vector contribute nothing; a turn with **no** cached vector → `raw = 0.0`.
   *Max* (not mean): protect a turn if **any** load-bearing part is on-goal.
2. **Rank-normalize** the candidates' `raw` values into `[0, 1]`
   (`normalized = rank / (n − 1)`, best-relevance = 1.0). Fully scale-free — the
   bge non-zero-centered range never needs calibrating. **Degenerate guard:** ≤ 1
   candidate, or all-equal `raw` (incl. all-zero) → `normalized = 0.0` for all ⇒
   pure recency, no spurious rescue.
3. Sort key `= base_score(turn) + ctx.weight · normalized`. Empty `ctx.goal`
   skips 1–3 entirely (bump = 0).

Vectors are unit vectors (v1 normalizes on write), so `cosine` is a dot product.

### 3.4 `goal_text(events, n)` (new helper)

```rust
/// Concatenate the last `n` non-trivial user messages (newest-first) as the
/// relevance query. "Non-trivial" filters empties and sub-threshold confirmations
/// ("yes", "3", "confirmed") so terse turns don't poison the goal.
pub fn goal_text(events: &[&Event], n: usize) -> String;
```

- `MIN_GOAL_MSG_CHARS` (const) — triviality threshold.
- `GOAL_WINDOW_MSGS` (const, e.g. 3) — `n`.

### 3.5 Store: `vectors_by_ids`

```rust
// EventStore (store.rs), mirroring load_recent_embeddings / write_embedding:
pub fn vectors_by_ids(&self, model_id: &str, ids: &[Ulid])
    -> Result<HashMap<Ulid, Vec<f32>>>;
```

One batch `SELECT event_id, vector FROM event_embeddings WHERE model_id = ?1 AND
event_id IN (…)`. **Model-filtered** (staleness-safe, like every other embed
query). A bad/short row is skipped, never propagated (matches
`unembedded_events`' degrade posture). Threaded through the session actor as a new
`Cmd` + `SessionHandle::vectors_by_ids`, mirroring `write_embedding`
(`session.rs:420`) / `load_recent_embeddings` (`session.rs:441`).

---

## 4. Live wire-in (`preflight_gate`, agent.rs)

At the two eviction call sites (band pass §2 "(2)", hard-floor pass "(3)"), build
the context **only when eviction is actually going to run** (`est ≥ high_water`,
already the guard) **and** the embedder + index are present:

```rust
let ctx = if let (Some(_index), Some(emb)) = (&config.embed, &config.embedder) {
    let text = zoid_core::eviction::goal_text(&events_slice, GOAL_WINDOW_MSGS);
    let model = emb.model_id().to_string();
    let goal = tokio::task::spawn_blocking({
        let emb = emb.clone();
        move || emb.embed(&[&text]).ok().and_then(|mut v| v.pop()).unwrap_or_default()
    }).await.unwrap_or_default();               // normalized on the embed path
    let ids = embeddable_event_ids(&events);     // over-approx: all embeddable ids
    let vecs = session.vectors_by_ids(model, ids).await.unwrap_or_default();
    GoalContext { goal, vecs, weight: DEFAULT_RESCUE_WEIGHT }
} else {
    GoalContext::default()                        // recency-only, unchanged
};
let plan = zoid_core::eviction::plan_evictions(events.iter(), policy, est, &RecencyScorer, &ctx);
```

- **Cost:** one ~30 ms goal embed (off-thread) + one batch DB read, **only under
  genuine pressure**. Normal turns (`est < high_water`) never touch the embedder.
- **Degradation:** any of {feature off, `embed.enabled=false`, weights missing,
  embed error, empty goal, no cached vectors} → empty/at-worst-zero-bump ctx →
  today's recency eviction. No new failure mode, no gate, no panic.
- **Subagents/tests:** `policy.enabled == false` short-circuits `preflight_gate`
  before any of this (`agent.rs:2615`) — untouched.
- If both call sites recompute `ctx`, hoist it once above the passes (goal +
  vectors are stable within one gate invocation).
- **Candidate ids without replicating `group_turns`:** `embeddable_event_ids`
  gathers the ids of all embeddable-kind events (optionally minus the most-recent
  window) — an *over-approximation* of the real candidate set. Extra entries in
  `ctx.vecs` are simply never looked up by `plan_evictions`, so the wire-in needs
  no dependency on eviction's private turn-grouping. Bounded by session size, run
  only under pressure.

---

## 5. Control plane

- `DEFAULT_RESCUE_WEIGHT: f32` — a **const**, **starting value `12.0`**: a
  perfectly on-goal old turn is treated as ~12 turns newer. Rationale: large
  enough to lift a relevant turn clear of a handful of intervening off-goal turns,
  small enough that it can't leapfrog the whole (typically 20–40 turn) live
  window and starve eviction. Confirmed/adjusted during implementation against a
  realistic over-pressure session. Exposing it via `[eviction]` config is a
  deferred follow-up (§8), not this slice.
- No new config keys, no new compile feature: this rides entirely on v1's
  existing `local-embed` feature and `[embed]` runtime switch. When `local-embed`
  is compiled out, `config.embedder` is `None` ⇒ recency-only, and none of this
  code path allocates.

---

## 6. Observability

- `tracing` the rescued/relevance summary when the term is active (e.g.
  `evicted=N reclaimed=…k relevance=on goal_len=…` and a debug count of turns
  whose rank-normalized relevance exceeded a log threshold).
- The eviction **detail view** already renders *what was evicted*; surfacing
  per-turn "kept because relevant" reasons needs extra plumbing (the plan records
  victims, not survivors) and is a deliberate follow-up (§8). This slice keeps the
  announce surface unchanged beyond tracing.

---

## 7. Testing strategy

All candle-free via `FakeEmbedder`; no network.

- **`goal_text`:** drops "yes"/"3"/empty, keeps a real request; newest-first;
  respects `GOAL_WINDOW_MSGS`.
- **Relevance layer (`plan_evictions` with a populated `GoalContext`):**
  - A relevant-but-old turn **survives** while a newer-irrelevant turn is evicted
    under pressure (rescue works, and the leapfrog is the intended trade).
  - **Empty `GoalContext` ⇒ eviction byte-identical** to the current
    `RecencyScorer`-only result (regression guard on the degradation path).
  - **All-equal / all-zero raw cosines ⇒ pure recency** (degenerate guard).
  - A turn with **no cached vector** contributes `raw = 0` ⇒ not rescued.
  - **Rescue-only invariant (per-item, provable):** for every turn,
    `keep_score ≥ turn.index` (the bump is always `≥ 0`) — relevance can only move
    a turn *up* the keep order, never below its recency baseline.
    *Note:* this does **not** mean the victim set is a subset of the recency
    victim set. Under a fixed reclaim target, rescuing an old on-goal turn forces
    a *different* (newer, off-goal) turn to be dropped to still reach `low_water`
    — the intended trade (§3.1). Assert the *scenario* (below), not a subset.
  - **Same reclaim target:** relevance changes *which* turns fill the quota, not
    the quota — an over-pressure session still reclaims down past `low_water`.
  - **No deadlock:** with every candidate maximally rescued and pressure above
    the band, eviction still reaches `low_water`.
- **`vectors_by_ids`:** round-trip; **model filter** (rows of another `model_id`
  excluded); missing ids absent from the map; short/bad row skipped, not errored.
- **Wire-in (`zoid` integration):** with a `FakeEmbedder` + seeded
  `event_embeddings`, a session pushed over `high_water` evicts a newer-off-goal
  turn and **keeps** an older-on-goal turn; with the embedder absent, the same
  session evicts identically to recency-only.
- **Every task builds `--workspace`** (the `GoalContext` field additions and the
  `plan_evictions` signature change are cross-crate).

---

## 8. Out of scope / non-goals (seams honored)

- **Cross-encoder reranker** — the recall-precision slice; `Reranker` stays
  `Noop`. Fully spike-de-risked (`ms-marco-MiniLM-L-6-v2`), built later.
- **Per-file `Protection` axis** and the full ACM-2 `assemble_context`-as-decider
  rewrite — superseded: the shipped `EvictionScorer`/`plan_evictions`/`band`
  machinery already provides the injection point at turn granularity.
- **Relevance for messages / tool-results as a distinct signal** — a turn simply
  aggregates over whatever of its events are cached; no per-kind relevance policy.
- **`[eviction]` config exposure of `RESCUE_WEIGHT`** — const now.
- **Eviction-detail "rescued because…" UI** — tracing only this slice (§6).
- **Tier-2 generative compaction, RAG/additive retrieval, budget/model-routing** —
  unchanged long-term seams.

---

## 9. Success criteria

- Under genuine pressure (`est ≥ high_water`) on a session with cached embeddings,
  a stale-but-on-goal turn is **kept** while a newer off-goal turn is evicted, and
  the reclaim still reaches `low_water`.
- With the embedder absent/disabled/uncompiled, eviction is **byte-identical** to
  today's recency behavior (regression-tested).
- The rescue-only invariant holds: relevance **never** evicts a turn recency would
  have kept.
- The synchronous gate performs **no** embedding; the single goal embed runs
  off-thread and only when eviction fires.
- `GoalContext` — reserved empty since Slice-4 — is populated and load-bearing.
