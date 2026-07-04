# Demand-Paged Context (ACM ceiling) — Design

**Date:** 2026-07-04
**Status:** Design — iterating (brainstorm); implementation plan to follow (`writing-plans`).
**Supersedes/extends:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md` (§4 short-term arc). ACM-1 (tool-result compaction, shipped/merged) is a component of this design, not a replacement target.

## One-line goal

Hold the **live request** (tokens actually sent to the model each turn) near a user-set **context target** (default ~384k) within an operating band (~300–500k on 1M-capacity models), across **indefinite** sessions, auto-managed, surfaced, and undoable, with **nothing truly forgotten**: evicted history stays queryable — and do it on a foundation of **data-driven model metadata** and **pluggable ML seams** (embedding / re-ranking) so later phases upgrade retrieval quality without rearchitecture.

## Terminology (locked)

- **Capacity** — the model's physical maximum context (e.g. 1,000,000). The hard bound; never exceeded. Sourced from the **model-metadata catalog** (§3.0), never hardcoded in logic.
- **Context target** (`context_target`, the primary user knob — renamed from `context_ceiling`) — the token count the controller *manages toward*. A **soft setpoint** (~384k), always `≤ capacity`.
- **Band** — `high_water = target + headroom`, `low_water = target − headroom`, both clamped to `≤ capacity`. `headroom` is one advanced knob (default ~20%). Crossing `high_water` triggers an eviction wave down to `low_water`; steady state hovers in the band.
- **Hot working set** — events the projections replay and send. **Cold tier** — evicted events, retained in sqlite (FTS5-indexed), not replayed, queryable via `recall()`.

## Architecture in one paragraph

Split the event history into a **hot working set** (the events projections replay and send to the model) and a **cold tier** (evicted events that remain in sqlite, FTS5-indexed, and are *not* replayed). A hysteresis **eviction controller** keeps the hot set inside the band by first compacting tool results (ACM-1, shipped) and then evicting the oldest *whole turns*, leaving an in-context **breadcrumb marker** so the model knows history exists and how to reach it. A **`recall()` tool** searches the cold tier (BM25 via sqlite FTS5, no embeddings) and re-admits matching turns on demand. The one cold tier serves three consumers at once: it bounds the request, bounds working-set RAM/CPU, and backs recall.

---

## 1. Problem & current state

### 1.1 The regression (both failure modes are real and in the tree)

- **"Removed all history" (old bug).** The pre-diff `conversation()` sliced the event log at the last `TurnsDropped` **timestamp cutoff**. Because the token estimate reflected the full context the model received, `current > threshold` stayed true after each drop, so `TurnsDropped` re-fired and cascaded until a single turn survived — then re-thrashed on the next message.
- **"Under-evicts / can't hold the ceiling" (current state).** The parallel session's uncommitted diff correctly **deleted layer-4 turn-dropping** and sharpened the token estimate (chars/4 → chars/3, counts system + tool-spec overhead, learns a calibration ratio from real provider usage). That stops the thrash — but it leaves the live request with **only one** reduction lever: tool-result compaction. Compaction shrinks only `ToolResult`/`File` bodies to head+footer and skips already-compacted items; once every tool result is summarized, nothing further can shrink while `running` can still exceed the ceiling. Messages and whole turns are never touched.

### 1.2 The ceiling is enforced in the wrong place

`token_ceiling` and heat-based eviction exist in `assemble_context` (`crates/zoid-core/src/assembler.rs`), but its **only caller is the subagent path** (`crates/zoid/src/subagent.rs`). The live request (`build_request` → `conversation`) never calls it. So on the live path, `token_ceiling` is dead weight and there is no mechanism to bound an indefinite session.

### 1.3 Storage / replay findings (why "indefinite" breaks before sqlite does)

- Full log held in memory as `Vec<Event>` (`crates/zoid/src/main.rs:918`); projections replay the whole slice each turn. `context_window`/`estimate_tokens` **re-walk every character of every stored tool output** (`chars/3`) on every structural frame. Dominant cost term is **total tool-output bytes, not turn count**.
- Resume is one unbounded `SELECT * WHERE session_id` (`crates/zoid-core/src/store.rs:105-112`) that materializes and JSON-parses the entire lifetime log into RAM. No windowing.
- Append-only; **compaction never reclaims** — it adds a `ToolResultCompacted` event and keeps the original blob. No retention anywhere.
- **Conclusion:** sqlite disk is the *last* thing to hurt (handles GB of append-only blobs). The *first* bottlenecks are the in-memory per-turn byte-walk and the unbounded resume load — both fixed by bounding the **working set**, which is the same mechanism that bounds the request.

---

## 2. Key decisions (resolved forks)

1. **The target governs tokens sent per turn** (not accumulated log size). The controller manages toward `context_target`; the band hovers around it; `capacity` is the hard bound.
2. **Model metadata is data, not code (Slice 0).** `crates/zoid-provider/src/model.rs` hardcodes `context_window` (200k/256k — both wrong; claude and glm-5.2 are actually **1,000,000**). Instead of patching the constants, replace hardcoded-in-logic metadata with a **model-metadata catalog** (§3.0): a sqlite table, seeded with known values and refreshed dynamically from providers where they expose it (Ollama `/api/show` already has a context-window parser at `ollama.rs:156`), with config override always winning and a conservative floor for the unknown-and-unfetchable. The 1M values become *seed rows*, not literals in a match arm.
3. **Eviction unit = whole turn**, never loose items — preserves `tool_use`/`tool_result` pairing in the message list. (Item-level heat eviction stays confined to the subagent path where it already lives.)
4. **Hysteresis, not edge-trimming.** Cross a **high-water** mark → evict down to a **low-water** mark in one wave. This *is* the operating band.
5. **Explicit evicted-id set, never a timestamp cutoff.** The cutoff was the thrash bug. Eviction records the exact ids removed; the projection honors *those*, making it idempotent.
6. **Protection is structural, not heuristic.** System/`Immutable`, `pinned`, and the most-recent-*N* turns are type-level un-selectable by the controller — not gated on a heat threshold that can be misconfigured.
7. **Graduated levers:** (1) compact tool results [shipped] → (2) evict oldest turns. Compaction runs first each wave; eviction only if still over low-water.
8. **Demand-paged, not lossy:** evicted turns leave a **breadcrumb marker** in-context and are retrievable via `recall()`. The marker is load-bearing — without it, demand-paging silently degrades to amnesia.
9. **Recall needs no embeddings for v1** — sqlite **FTS5 (BM25)**. Embeddings (ACM-2) are a later quality upgrade, not a blocker.
10. **Pure-core / effectful-bin seam preserved.** `zoid-core` gains only pure additions (`ItemKind::Retrieved`, eviction/marker/controller logic). The FTS index and the `recall` tool live in the **bin**, which already owns sqlite and the `Tool` trait.
11. **Auto + surfaced + undoable.** The controller runs every turn without prompting; each eviction renders in the transcript (semantic zoom) and is undoable by re-admitting the ids (append-only ⇒ reversible).
12. **Append-only reversibility retained.** Eviction and recall are new events; original events are never mutated or deleted from the log. (Physical reclamation of cold blobs from the hot `Vec`/resume path is Slice 3, and still never deletes from sqlite.)
13. **ML is a set of pure seams, wired from day one behind `None` (§3.7).** `Embedder`, `Reranker`, and `EvictionScorer` are pure `zoid-core` traits. Slices 1–3 pass `None` (recency-ordered eviction, FTS5-only recall). Slice 4 passes `Some(...)` in-process implementations — **no rearchitecture**, only lit-up seams. The same model-metadata catalog (Decision 2) carries embedding/reranker model metadata (dim, etc.).
14. **Recall is a staged retrieval pipeline, not a single query.** `CandidateSource → (optional) Reranker → budgeted selection`. Slice 2 = `[Fts5Source]`, no reranker. Slice 4 adds a `VectorSource` (hybrid retrieval) and a cross-encoder reranker. The pipeline is invokable **both** by the model (the `recall` tool) **and** by the controller (proactive auto-recall, §11) — one code path.
15. **Eviction victim-selection is pluggable.** `plan_evictions` scores turns through an `EvictionScorer`; the default is recency (oldest-first, safe, deterministic). This is the seam where Slice 4 swaps in relevance-to-current-goal so the agent keeps what matters *now* and pages out what doesn't — the north-star differentiator (§11).
16. **Two-speed execution (§3.8).** The correctness-gating decisions (evict to stay ≤ capacity/target; cheap tool-result truncation) stay a **synchronous, microsecond hot-path gate** — you can't send an over-capacity request, and a fast pure computation keeps the steady-state property testable. All **expensive, non-gating** work (embeddings, index build, cold-paging, catalog refresh, any LLM-summarization) runs on an **async maintenance lane** (idle/debounced, timer fallback), landing as append-only events with eventual consistency. Split by whether the work gates correctness, not by cost.

---

## 3. Components

### 3.0 Model-metadata catalog (Slice 0 — data-driven, replaces hardcoding)

**Pure type** (`zoid-provider` or `zoid-core`):

```
struct ModelMetadata {
    provider: String, model_id: String,
    kind: ModelKind,            // Chat | Embedding | Reranker
    capacity: u64,              // context window (chat); 0 for embed/rerank
    max_output: Option<u64>,
    supports_tools: bool, prompt_cache: bool,
    embedding_dim: Option<u32>, // for Embedding models
    source: MetaSource,         // Config | Live | Seed | Default
    fetched_at: Option<i64>,
}
```

**Resolver seam** (effectful impl in the bin): `trait ModelCatalog { fn metadata(&self, model: &str) -> ModelMetadata; fn refresh(&self, provider: &str) -> Result<usize>; }`. `zoid-core` consumes resolved `ModelMetadata` only — it never fetches. Resolution priority (highest wins):

1. **Config override** — explicit user value in `config.toml` (e.g. force a capacity).
2. **Fresh DB cache** — a `model_metadata` sqlite table row within TTL.
3. **Live provider fetch** — Ollama `/api/show` → `context_length` (parser already exists, `ollama.rs:156`); writes back to the DB cache. Anthropic `/v1/models` lists ids only (no window), so it contributes existence, not capacity.
4. **Seed data** — a migration seeds curated known models (claude & glm-5.2 = **1,000,000**, deepseek, etc.). Known values live as *rows*, so they are refreshable and overridable, never buried in a match arm.
5. **Conservative floor** — unknown-and-unfetchable → 32k (today's default), flagged `MetaSource::Default` so the UI can warn.

**Refresh** is effectful and bin-owned: on-demand (a `models refresh` action / on provider or model selection), TTL-based staleness, or manual. **ACM consumes `catalog.metadata(active_model).capacity`** for the hard bound and derives the band from `context_target`. This is the single dependency that makes the ceiling correct on any model, including local Ollama tags the registry has never heard of. The *same* catalog later supplies embedding/reranker model metadata (dim) to §3.7, so there is one metadata system, not two.

### 3.1 Eviction controller (pure, `zoid-core`)

A pure planner analogous to `plan_compactions`:

```
plan_evictions(events, policy, current_tokens, scorer: &dyn EvictionScorer) -> EvictionPlan
```

- Operates on the projected hot working set (post-compaction).
- **Trigger:** only when `current_tokens > high_water` (= `target + headroom`).
- **Selection:** rank evictable **turns** by `scorer` (default `RecencyScorer` = oldest-first), evict lowest-ranked first, accumulating reclaimed tokens until `current_tokens - reclaimed <= low_water` (= `target − headroom`). The scorer seam (§3.7) is where Slice 4 swaps recency for relevance-to-current-goal without touching the controller.
- **Evictable turn** = a contiguous message group whose items are all `Normal` protection, not `pinned`, not System/`Immutable`, and **older than the most-recent-*N*-turns window**.
- **Idempotent:** turns already evicted (their ids present in a prior `TurnsEvicted`) are skipped; re-running with no new pressure yields an empty plan.
- **Never empties the window:** the most-recent-*N* turns are structurally excluded, so the plan can leave `current_tokens` above `low_water` (or even above `high_water`) rather than evict protected content. This is correct behavior, surfaced as a warning (see §6).
- Emits `EvictionPlan { turns: Vec<EvictedTurn { ids, token_estimate, topic_hint }> }`.

`topic_hint` is a cheap extractive label (e.g. first user-message line of the turn, truncated) — **no LLM call** — used in the breadcrumb marker.

### 3.2 Events (pure, `zoid-core`)

- `EventKind::TurnsEvicted { ids: Vec<EventId>, reclaimed_tokens: u64, marker: EvictionMarker }` — append-only; original events retained.
- `EvictionMarker { spans: Vec<{ id_range_label, token_estimate, topic_hint }> }` — the data the transcript renders and the model reads.
- `EventKind::TurnsReadmitted { ids: Vec<EventId> }` — the undo / recall re-admission event; projections stop skipping these ids.
- The inert `TurnsDropped` variant is left as-is (backward-compatible deserialization); nothing new emits it.

### 3.3 Projections (pure, `zoid-core`)

- `conversation()` and `context_window()` maintain an **evicted-id set** (folded from `TurnsEvicted` minus `TurnsReadmitted`) and **early-`continue`** on any event whose id is evicted — the same shape as the existing subagent-branch skip. This bounds the request **and** the per-turn byte-walk together.
- `conversation()` injects the **breadcrumb marker** as a synthetic message at the position of each evicted span, so the model sees `[N turns evicted here · ~Xk tokens · topics: … · recall("…") to retrieve]`.
- Retrieved turns (via `recall`) are re-admitted through `TurnsReadmitted` and flow back into the projection normally, tagged `ItemKind::Retrieved` for scoring/UX.

### 3.4 Recall tool (effectful, bin)

- New `recall` tool (`ToolKind::Local`) in the bin: `recall(query: string, limit?: int)`.
- Backed by a sqlite **FTS5** virtual table indexing event content **at append time** (maintained in the bin's store alongside the `events` table) — *all* events are indexed, not just evicted ones, so recall works regardless of eviction state and the index needs no rebuild when a turn is evicted. Results whose ids are already in the hot set are de-duplicated (re-admitting a still-hot turn is a no-op).
- Returns coherent **rendered turns** (not raw `tool_use`/`tool_result` JSON), each with its original event ids.
- Re-admission: recall appends `TurnsReadmitted { ids }` so the turns re-enter the hot set as `Retrieved` items, subject to the controller (a recall can itself age back out).
- Miss → a normal empty/"no matches" tool result (not an error).

### 3.5 Cold-paging (Slice 3, deferrable)

- Stop materializing evicted blobs in the hot `Vec<Event>`; keep evicted ids + marker metadata hot, page full bodies from sqlite only on recall.
- **Windowed resume load:** load the live-window tail into the hot `Vec`; leave older events in sqlite (reachable via recall). Fixes the RAM/resume curve.
- Never deletes from sqlite — cold storage is the recall corpus and the undo backstop.

### 3.6 Config (bin + `zoid-core` `EconomyConfig`)

- **`capacity`** is not configured — it is resolved from the catalog (§3.0), config override available.
- **`context_target`** (renamed from `context_ceiling`) — the primary user knob: the soft setpoint the controller manages toward. Absolute tokens, or percent-of-capacity. If unset, default = `min(capacity, DEFAULT_TARGET)` where `DEFAULT_TARGET ≈ 400k` (so a fresh 1M-capacity model doesn't silently balloon to fill the whole window — cost/latency guard). **[OPEN: default policy — see §12.]**
- **`band_headroom`** (advanced, default ~20%) — derives `high_water = min(target + headroom, capacity)` and `low_water = max(target − headroom, 0)`. One number to widen/narrow the operating band. Invariant enforced at load: `0 < low_water < high_water ≤ capacity`.
- **`recent_n`** — protected recent-turn count (never evictable).
- **Master enable** — back-compat: `compact_threshold_pct = 0` still disables all ACM (compaction + eviction). The old `token_ceiling` field is retired/folded (capacity is the hard bound; target is the soft knob).
- Wire the resolved target/capacity into the **live** turn config (today the ceiling only reaches the subagent path).

### 3.7 Retrieval & relevance seams (defined now, implemented in Slice 4)

Pure `zoid-core` traits, threaded as `Option<Arc<dyn …>>` (None in Slices 1–3). Defining them now costs a few trait declarations; retrofitting them later would touch the controller, the recall pipeline, and the store — so they go in from the start.

- **`trait Embedder { fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>; fn dim(&self) -> usize; fn model_id(&self) -> &str; }`** — in-process (candidate impls: `fastembed`/ONNX bge-small, or `candle`). Model discovered via the catalog (§3.0, `ModelKind::Embedding`, `embedding_dim`).
- **`trait Reranker { fn rerank(&self, query: &str, candidates: &[RecallCandidate]) -> Vec<Scored>; }`** — in-process cross-encoder; refines candidate order for precision.
- **`trait EvictionScorer { fn score(&self, turn: &TurnView, ctx: &GoalContext) -> f32; }`** — victim selection (§3.1). `RecencyScorer` default now; `RelevanceScorer` (embedding cosine to current goal) in Slice 4.
- **Retrieval pipeline** — `trait CandidateSource { fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate>; }`. Slice 2 pipeline = `[Fts5Source]`. Slice 4 = `[Fts5Source, VectorSource]` (hybrid: lexical + semantic) → `Reranker` → budgeted selection. Same pipeline serves the `recall` tool and proactive auto-recall (§11).
- **Embedding storage (reserved schema, populated in Slice 4):** `event_embeddings(event_id, model_id, dim, vector BLOB, PRIMARY KEY(event_id, model_id))`, filled lazily when an `Embedder` is present. Vector search via the `sqlite-vec` extension **or** brute-force cosine in-process over the bounded cold set — **[OPEN: pick in Slice 4; both fit the pure-core/effectful-bin split since vectors live in the bin's store.]**

All of the above stay out of the Slice 0–2 build; their *signatures* and the reserved table are the deliverable, so Slice 4 is additive.

### 3.8 Execution model — hot-path safety gate vs async maintenance lane

Not all context work has the same urgency, so it should not all run on the turn hot-path. Split by **whether the work gates correctness**:

- **Synchronous safety gate (hot path, microseconds — stays inline).** You *cannot* send a request that exceeds the model's physical `capacity`, and the target should be honored before sending. But the gating decisions are **cheap**: eviction is "pick turn ids + append a `TurnsEvicted` event" (no LLM call, no byte rewriting), and ACM-1 tool-result compaction is head+footer truncation. These run in the loop before re-request, exactly as today. The gate is a fast, deterministic pure computation — keeping it synchronous is what makes the steady-state property testable.
- **Asynchronous maintenance lane (off hot path — idle/debounced, timer as fallback heartbeat).** The **expensive, non-gating** work: computing embeddings for new events, building/refreshing FTS + vector indexes, any future LLM-based summarization, reranker precompute, cold-paging/archival (Slice 3), and catalog refresh (Slice 0). None of these change what *must* be sent this turn; they improve future retrieval/efficiency. They run on a background task (via the existing single-writer session actor + tokio), **kicked event-driven** (after N new events or T seconds idle, whichever first) rather than on a naive fixed timer — the timer is only a fallback so maintenance can't stall indefinitely. Results land as append-only events, so a lagging maintainer never corrupts a turn — it just means embeddings/indexes are eventually-consistent, which is fine because recall degrades gracefully to FTS/lexical when a vector isn't ready.

**Why this matters now:** Slice 4's embedding-every-event is the first genuinely expensive maintenance task, and it *must* live in this lane. Defining the async maintainer as a seam in the architecture now (even though Slices 0–2 put almost nothing on it — only the cheap gate runs) means the expensive lane has a home and we never have to retrofit async into the hot path. **Direct answer to "timer/async compaction": yes for the expensive lane, no for the cheap safety gate — separate them by whether they gate correctness, not by cost alone.**

---

## 4. Data flow (one turn)

1. Assemble request from `conversation(events)` (skips evicted ids, injects markers).
2. Estimate `current_tokens` (real provider usage if available, else calibrated estimate).
3. If `current_tokens > high_water`: run **compaction** (ACM-1), re-estimate; if still over, run `plan_evictions` down to `low_water`; append `TurnsEvicted` before re-request.
4. Model may call `recall(query)` → FTS5 → append `TurnsReadmitted` → matching turns re-enter next assembly.
5. Transcript renders compaction (`⧟`) and eviction markers with semantic zoom; user can undo an eviction (re-admit) from the UI.

---

## 5. UX (surfaced + undoable)

- Eviction marker in transcript, consistent with the existing compaction glyph treatment: **Summary** = one-line chip (`⋯ 12 turns paged out · 14k`), **Detail** = per-span breakdown with topic hints and a recall/undo affordance.
- Context/economy drawer shows current live tokens vs the band (low/high water, with the setpoint midpoint marked) and the count of paged-out turns.
- Undo = re-admit the span's ids (`TurnsReadmitted`); the controller may re-evict on the next wave if still over — surfaced, not silent.

---

## 6. Error handling & edge cases

- **Can't reach low-water without touching protected content** → stop at the protected boundary, keep well-formedness, and surface a warning (the request may exceed the band when recent-*N* alone is large). Never evict protected turns to hit a number.
- **Recall miss** → empty result, not an error.
- **FTS5 unavailable / index build failure** → recall degrades to "unavailable"; eviction still functions (the marker still tells the model history existed). Log loudly; do not crash the turn.
- **Well-formedness** → eviction always removes complete turns; a partial turn (streaming in progress) is never evicted.
- **Calibration** → keep the parallel session's real-usage calibration; eviction decisions use the same `current_tokens` source as compaction.

## 7. Testing strategy

- **Steady-state property test (the missing coverage that let this regress):** simulate a long multi-turn session (hundreds of turns, large tool outputs); assert the live request stays `<= ceiling` **for every turn** and never drops below the recent-*N* protected floor. This "holds the band over time" property has **no test today** — its absence is the root cause the regression shipped.
- Eviction **idempotence / no-thrash:** re-running `plan_evictions` with no new pressure yields an empty plan; a single wave reaches low-water in one pass.
- **Protection invariants:** System/`Immutable`, `pinned`, and recent-*N* turns are never in any `EvictionPlan`.
- **Explicit-id (no cutoff):** an evicted id set removes exactly those turns, and later turns are unaffected.
- **Recall round-trip:** evict a turn, `recall(query)` finds it, `TurnsReadmitted` re-admits it, projection includes it again.
- **Undo restores** the exact evicted content.
- **Marker present** whenever anything is evicted (guards against silent amnesia).
- Reuse ACM-1's discipline: cross-crate field adds to shared types must be built with `--workspace` (a prior slice broke zoid-tui literals when tests were scoped to `-p zoid-core`).

## 8. Slicing & sequencing

- **Slice 0 — model-metadata catalog.** DB table + `ModelCatalog` resolver seam + seed (correct 1M values) + config override + Ollama `/api/show` live-fetch (parser exists) + conservative floor. Removes hardcoding; gives ACM a correct `capacity` on any model. Prerequisite for a correct target/band.
- **Slice 1 — bounded turn-eviction + breadcrumb, honored by skip-in-fold.** Holds the target and bounds per-turn CPU. Includes the `context_target`/band config and the `EvictionScorer` seam (recency default). Must-have; fixes the reported bug.
- **Slice 2 — recall pipeline over cold sqlite (FTS5).** Makes eviction non-lossy. Defines the `Embedder`/`Reranker`/`CandidateSource` seams (as `None`/`[Fts5Source]`) and the reserved `event_embeddings` table. **Built with Slice 1 as one coherent unit** (eviction without recall is lossy; recall without eviction is pointless).
- **Slice 3 — cold-paging + windowed resume.** Fixes the RAM/resume curve; reuses Slice 2's cold tier. Most runway; deferrable.
- **Slice 4 — ML upgrade (ACM-2+).** In-process `Embedder` + `Reranker` impls, populate `event_embeddings`, hybrid (lexical+vector) retrieval, cross-encoder rerank, relevance-driven eviction, and proactive auto-recall (§11). Quality only; deferrable — but every seam it needs ships in Slices 0–2.

**Implementation plan scope: Slices 0+1+2.** Slice 0 is a prerequisite for a correct ceiling and is cohesive with the ACM work. Slices 3+4 are documented here but out of the first plan.

## 9. Relationship to existing code & in-flight work

- Builds directly on the (now-committed) baseline that removed layer-4 turn-dropping and added token calibration — the correct foundation. The new eviction is the **bounded, explicit-id** replacement for that removed layer.
- Reuses ACM-1 wholesale: compaction stays lever 1; the projection-substitution wire-in point (`conversation()`) is exactly where eviction also intervenes.
- Key anchors: `projection.rs` (conversation fold + skip), `context.rs:184-317` (context_window fold, per-event estimate), `compaction.rs:52` (`plan_compactions`, sibling to `plan_evictions`), `assembler.rs` (existing hysteresis/heat logic to mirror now, converge later — §11 unified assembler), `store.rs:16-40` (events schema; FTS5 + `model_metadata` + `event_embeddings` tables added alongside), `main.rs:918`/`1061`/`2093` (hot `Vec`, boot/resume load — Slice 3 windowing target), `agent.rs:163-176` (request build), `agent.rs:374`/`677` (`record_compactions` — the synchronous gate, §3.8).
- Catalog (Slice 0) anchors: `crates/zoid-provider/src/model.rs` (hardcoded `MODEL_CAPS` → seed rows), `ollama.rs:156` (`/api/show` context-window parser — already exists), `lib.rs:123` + `ollama.rs:312`/`anthropic.rs:115` (`list_models`), `crates/zoid/src/session.rs` (single-writer sqlite actor — home for the async maintenance lane, §3.8).

## 10. Non-goals / out of scope (for the Slice 0–2 plan)

- Semantic (embedding) relevance scoring and re-ranking — **seams defined (§3.7), implemented in Slice 4.**
- The async maintenance lane carrying real load — **seam defined (§3.8); Slices 0–2 run only the cheap synchronous gate.**
- Physical deletion / vacuuming of cold sqlite rows.
- Changing compaction's tool-result *summarization* behavior (kept as-is; only its scheduling is discussed, §3.8).
- Item-level live eviction (stays turn-level on the live path; item/heat eviction remains subagent-only).
- Multi-model routing / pricing (tokens-only by design). Note: the catalog (§3.0) makes routing *possible* later, but selection/pricing stay out.

## 11. North Star (revolutionary directions — shape the seams, out of the 0–2 build)

The seams above exist so these become additive, not rewrites. This is where "ahead of the industry" lives — most agents do a sliding window or summarize-the-oldest; demand-paged context aims higher:

- **Proactive auto-recall.** Each turn, embed the current goal/working context, retrieve the top-k relevant *cold* turns, and inject (or offer) them — the model doesn't have to *know* it has a gap. The context assembles itself around what you're doing now. (Reuses the §3.7 pipeline, invoked by the controller instead of the model.)
- **Relevance-driven eviction.** Victim selection by relevance-to-current-goal (the `EvictionScorer` seam), not recency. Keep what matters to the task at hand; page out what doesn't; recall on demand. Recency is the safe default; relevance is the differentiator.
- **Unified assembler.** Today `assemble_context` (per-kind policy, heat, ceiling) serves only subagents; `conversation()` serves live. Converge them into **one** pluggable assembler with per-kind policy for both paths — the live working set becomes an actively *composed* brief (system + pinned + recent + relevant-recalled + digest), not a truncated log.
- **Per-kind token-budget allocation.** Sub-budgets (system / recent conversation / recalled / tool-results) so a large recall can't starve the conversation spine and vice-versa — the ceiling becomes an allocator, not a single cut line.
- **Explainable context.** Because every compaction/eviction/recall is an append-only event, zoid has a complete, replayable audit of *why* each item left or returned. "Explainable context management," surfaced through the existing semantic-zoom UI, is a genuine differentiator for trust.

## 12. Open questions (decide before/inside the plan)

- **Default `context_target` policy** when unset: `min(capacity, ~400k)` absolute, or a fraction of capacity (e.g. 40%)? (Leaning absolute-cap so behavior is predictable across models.)
- **Vector search backend** (Slice 4): `sqlite-vec` extension vs brute-force in-process cosine over the bounded cold set. (Defer; both respect the seam.)
- **Async maintainer trigger constants** (§3.8): the N-events / T-idle thresholds and whether Slice 0 catalog-refresh piggybacks the same lane or is purely on-demand.
- **`headroom` default** and whether the band is symmetric (`target ± headroom`) or asymmetric (`low = target − headroom`, `high = target`). Symmetric is the current spec assumption.
