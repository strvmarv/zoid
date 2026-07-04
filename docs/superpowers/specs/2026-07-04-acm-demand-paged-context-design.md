# Demand-Paged Context (ACM ceiling) — Design

**Date:** 2026-07-04
**Status:** Design — iterating (brainstorm); pressure-tested by gilfoyle (findings C1/I2–I5/M6–M10 folded in); `writing-plans` GATED on parallel debt-list completion, then re-anchor §9 line numbers.
**Supersedes/extends:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md` (§4 short-term arc). ACM-1 (tool-result compaction, shipped/merged) is a component of this design, not a replacement target.

## One-line goal

Hold the **live request** (tokens actually sent to the model each turn) at or below a user-set **context target** (default `min(capacity, 384k)`), with an eviction wave dropping to `target − headroom` (~307k at a 384k target / 20% headroom) so steady state hovers in `[low_water, target]` across **indefinite** sessions — auto-managed, surfaced, and undoable, with **nothing truly forgotten** (evicted history stays queryable). Built on **data-driven model metadata** and **pluggable ML seams** (embedding / re-ranking) so later phases upgrade retrieval quality without rearchitecture.

## Terminology (locked)

- **Capacity** — the model's physical maximum context = **input + output** (e.g. 1,000,000). The hard bound; never exceeded. Resolved from `MODEL_CAPS` seed (fixed) + the already-wired live `fetch_model_info` override (§3.0), never a match-arm literal you have to trust.
- **Context target** (`context_target`, the primary user knob — renamed from `context_ceiling`) — the token count the controller *manages toward*. A **soft setpoint** (~384k).
- **Effective target** (§3.6a) — `min(context_target, capacity − output_reserve)`, recomputed per active model. This is what the band is actually built from, so `target` can never exceed what the model can carry (handles the `capacity ≤ target` / small-model case).
- **Band (asymmetric)** — `high_water = effective_target`, `low_water = effective_target − headroom` (headroom default ~20%). A **pre-flight** gate (§3.8) evicts when the estimate reaches `high_water`, down to `low_water`, so the request **never routinely sits above the target** — steady state hovers in `[low_water, effective_target]`, and a capacity-error retry guarantees the hard bound even when the estimate under-reads.
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
2. **Model-metadata: minimal seed-fix for ACM; full DB catalog decoupled (§3.0).** The wrong constants (`model.rs` claude/glm = 200k/256k; both are actually **1,000,000**) are fixed in place, and the **already-wired** async `fetch_model_info` override (`main.rs:548,1550`) supplies live capacity where a provider exposes it (Ollama `/api/show`). That is all ACM's ceiling needs — the resolution seam is already at the bin boundary, so no pure signature changes. The user's "stop hardcoding → cache to a DB table" ambition is real but **decoupled** into its own slice (needed by Slice 4 for embedding-model metadata), not bundled into the ACM plan.
3. **Eviction unit = whole turn**, never loose items — preserves `tool_use`/`tool_result` pairing in the message list. (Item-level heat eviction stays confined to the subagent path where it already lives.)
4. **Hysteresis, not edge-trimming.** Cross a **high-water** mark → evict down to a **low-water** mark in one wave. This *is* the operating band.
5. **Explicit evicted-id set, never a timestamp cutoff.** The cutoff was the thrash bug. Eviction records the exact ids removed; the projection honors *those*, making it idempotent.
6. **Protection is structural, not heuristic.** System/`Immutable`, `pinned`, and the most-recent-*N* turns are type-level un-selectable by the controller — not gated on a heat threshold that can be misconfigured.
7. **Graduated levers:** (1) compact tool results [shipped] → (2) evict oldest turns. Compaction runs first each wave; eviction only if still over low-water.
8. **Demand-paged, not lossy:** evicted turns leave a **breadcrumb marker** in-context and are retrievable via `recall()`. The marker is load-bearing — without it, demand-paging silently degrades to amnesia.
9. **Recall needs no embeddings for v1** — sqlite **FTS5 (BM25)**. Embeddings (ACM-2) are a later quality upgrade, not a blocker.
10. **Purity boundary corrected: the *planners* are pure; sqlite already lives in `zoid-core`.** The `rusqlite::Connection` (`store.rs` `EventStore`) and the single-writer actor (`session.rs` `SessionHandle`/`Cmd`) are **in `zoid-core`**, so the crate is already effectful for persistence — "bin owns sqlite" was wrong. What stays pure is the **planner/projection surface**: `plan_compactions`, `plan_evictions`, the assembler, and `conversation`/`context_window` remain pure functions over `&[Event]`. New storage (FTS table, `model_metadata`, `event_embeddings`) is added as **`EventStore` methods reached through new actor `Cmd` variants** (e.g. `RecallQuery`, `WriteEmbedding`, `RefreshCatalog`); the `recall` **tool** lives in the bin and calls the actor like every other sqlite access (there is no `Connection` in the bin to query directly). The plan must budget this actor+store surface — it is not free.
11. **Auto + surfaced + undoable.** The controller runs every turn without prompting; each eviction renders in the transcript (semantic zoom) and is undoable by re-admitting the ids (append-only ⇒ reversible).
12. **Append-only reversibility retained.** Eviction and recall are new events; original events are never mutated or deleted from the log. (Physical reclamation of cold blobs from the hot `Vec`/resume path is Slice 3, and still never deletes from sqlite.)
13. **ML is a set of pure seams, wired from day one behind `None` (§3.7).** `Embedder`, `Reranker`, and `EvictionScorer` are pure `zoid-core` traits. Slices 1–3 pass `None` (recency-ordered eviction, FTS5-only recall). Slice 4 passes `Some(...)` in-process implementations — **no rearchitecture**, only lit-up seams. The same model-metadata catalog (Decision 2) carries embedding/reranker model metadata (dim, etc.).
14. **Recall is a staged retrieval pipeline, not a single query.** `CandidateSource → (optional) Reranker → budgeted selection`. Slice 2 = `[Fts5Source]`, no reranker. Slice 4 adds a `VectorSource` (hybrid retrieval) and a cross-encoder reranker. The pipeline is invokable **both** by the model (the `recall` tool) **and** by the controller (proactive auto-recall, §11) — one code path.
15. **Eviction victim-selection is pluggable.** `plan_evictions` scores turns through an `EvictionScorer`; the default is recency (oldest-first, safe, deterministic). This is the seam where Slice 4 swaps in relevance-to-current-goal so the agent keeps what matters *now* and pages out what doesn't — the north-star differentiator (§11).
16. **Two-speed execution (§3.8), with a real PRE-FLIGHT gate.** The correctness-gating levers (evict/compact to fit) run **synchronously *before* `build_request`, on every sub-turn** — not after the response as today's `record_compactions` does. Today's loop is reactive (it shrinks request N+1 using usage measured from request N); this design moves the gate ahead of the send. Because the pre-flight estimate can undercount (the `chars/3 × calibration_ratio` estimate is known to under-read code/tool output), the gate (a) applies a **safety margin below capacity** and **biases the estimate to over-count**, and (b) has a **fallback: on a provider context-length error, force an eviction wave and retry, bounded**. Overshooting a *soft target* is fine; overshooting *hard capacity* crashes the turn, so the failure is asymmetric and the retry path is mandatory, not optional. All **expensive, non-gating** work (embeddings, vector-index build, cold-paging, catalog refresh, any LLM-summarization) runs on an **async maintenance lane** writing **side-table rows** (§3.7 — *not* `EventKind` rows). Split by whether the work gates correctness, not by cost.

---

## 3. Components

### 3.0 Model metadata — minimal correctness fix now, full catalog decoupled

Gilfoyle review (I5) established that ACM's *ceiling correctness* does **not** need a new DB catalog, because the resolution seam already sits at the bin boundary and is already async:

- `context_ceiling()` (`zoid-provider/src/lib.rs:198-206`) is pure/sync and **called only from the bin** (`main.rs:1103,1888`); `zoid-core` consumes a resolved `u64`, never `model_info()`. So making capacity DB/async-backed threatens **no** pure projection signature.
- The async override is **already wired**: `Provider::fetch_model_info` (async, `lib.rs:130`) → `spawn_model_info_fetch` (`main.rs:548`) folds a live result into `ctx_ceiling` (`main.rs:1550-1559`); Ollama `/api/show` parser exists (`ollama.rs:156`).

**So Slice 0 (in the ACM plan) is minimal:** (a) fix the three wrong seed constants in `MODEL_CAPS` (`model.rs:106/124/133` → claude & glm-5.2 = **1,000,000**), and (b) ensure the already-wired `fetch_model_info` override is applied so any model (incl. unknown local Ollama tags) gets a correct capacity. No new table. This delivers correct capacity on any model for Slices 1–2.

**The DB-backed catalog below is the user's "stop hardcoding → cache in DB" end-state — but it is DECOUPLED from ACM** (it buys nothing for the Slice 1–2 ceiling and is genuinely needed only when Slice 4 wants embedding-model metadata). It ships as its own slice, on its own schedule, not inside the ACM plan.

---

**Full catalog (decoupled slice; Slice 4 depends on it). Pure type** (`zoid-provider`):

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
- **Trigger:** only when `current_tokens >= high_water` (= `target`).
- **Selection:** rank evictable **turns** by `scorer` (default `RecencyScorer` = oldest-first), evict lowest-ranked first, accumulating reclaimed tokens until `current_tokens - reclaimed <= low_water` (= `target − headroom`). The scorer seam (§3.7) is where Slice 4 swaps recency for relevance-to-current-goal without touching the controller.
- **Evictable turn** = a contiguous message group whose items are all `Normal` protection, not `pinned`, not System/`Immutable`, not freshly-`Retrieved` (M10 below), and **older than the most-recent-*N*-turns window**.
- **Turn grouping skips inert events (M6).** "Turn" is not a first-class id in the log; `conversation()` derives it positionally, and the async lane can interleave inert/maintenance events between a `ToolCall` and its `ToolResult` (actor serializes by `rowid`). `plan_evictions` must group turns by **skipping inert kinds**, or a turn fragments and a `tool_use`/`tool_result` pair could split. Grouping is defined over the *non-inert* projection, same as `conversation`.
- **Retrieved-turn protection (M10).** A turn just re-admitted via `recall` is tagged `Retrieved` and is **protected from eviction for a cooldown** (e.g. treated as recent, or a scorer bonus), so recency scoring doesn't immediately re-evict the oldest-timestamped turn the model just paid to retrieve — otherwise recall→evict→recall oscillates.
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
- **Breadcrumb marker — NOT a standalone message** (this would break Anthropic). Recency-first eviction removes the *oldest* turns, so the span sits at the **front** of history; a standalone marker message there means either two consecutive user messages or a conversation starting with assistant — both are Anthropic 400s (`anthropic.rs` requires first-message-user + strict user/assistant alternation). Instead the marker is carried **out-of-band**: appended to the **system prompt** ("Earlier context (N turns, ~Xk tokens, topics: …) has been paged out; call `recall(query)` to retrieve it."), or prepended into the **text of the next surviving user message**. Never emitted as a `ChatMsg`. (Ollama's `/api/chat` is lenient and would tolerate an extra message, but the portable, provider-neutral choice is out-of-band.)
- **Tool-pairing across eviction:** on Anthropic this is a non-issue — `anthropic.rs` currently flattens all messages to text and does not emit `tool_use`/`tool_result` blocks, so no pairing exists to break. On Ollama, pairing is lenient (a dangling `tool` message survives). Eviction still removes **whole turns** so it does not orphan a pair regardless; this is belt-and-suspenders.
- Retrieved turns (via `recall`) are re-admitted through `TurnsReadmitted` and flow back into the projection normally, tagged `ItemKind::Retrieved` for scoring/UX, with **re-admission protection** (M10, §5) so a recalled old turn is not the immediate next eviction victim.

### 3.4 Recall tool (bin tool → actor query)

- New `recall` tool (`ToolKind::Local`) in the bin: `recall(query: string, limit?: int)`. It has no `Connection`; it issues a new actor `Cmd::RecallQuery` (§ Decision 10) that runs the FTS query inside `EventStore`.
- **FTS5 confirmed available** — `rusqlite` is `features=["bundled"]`, and `libsqlite3-sys` compiles the amalgamation with `-DSQLITE_ENABLE_FTS5` unconditionally (verified: `build.rs`), so `CREATE VIRTUAL TABLE … USING fts5` cannot fail at runtime, on musl/macos/windows alike. No extra feature flag needed.
- Backed by a sqlite **FTS5** virtual table indexing event content, maintained **synchronously in the actor's `Append` handler** (one transaction: `events` insert + FTS insert — cheap; this is the exception to §3.8's async lane). *All* events are indexed, not just evicted ones, so recall works regardless of eviction state and needs no rebuild when a turn is evicted. Results whose ids are already in the hot set are de-duplicated.
- Returns coherent **rendered turns** (not raw `tool_use`/`tool_result` JSON), each with its original event ids.
- Re-admission: recall appends `TurnsReadmitted { ids }` so the turns re-enter the hot set as `Retrieved` items — protected from immediate re-eviction (M10, §5) so a recalled old turn does not instantly churn back out.
- Miss → a normal empty/"no matches" tool result (not an error). DB read error → surfaced, not swallowed (rides debt-fix #9, `session.rs` error handling).

### 3.5 Cold-paging (Slice 3, deferrable)

- Stop materializing evicted blobs in the hot `Vec<Event>`; keep evicted ids + marker metadata hot, page full bodies from sqlite only on recall.
- **Windowed resume load:** load the live-window tail into the hot `Vec`; leave older events in sqlite (reachable via recall). Fixes the RAM/resume curve.
- Never deletes from sqlite — cold storage is the recall corpus and the undo backstop.

### 3.6 Config (bin + `zoid-core` `EconomyConfig`)

- **`capacity`** is not configured — it is resolved from the catalog (§3.0), config override available.
- **`context_target`** (renamed from `context_ceiling`) — the primary user knob: the soft setpoint the controller manages toward. Absolute tokens, or percent-of-capacity. If unset, default = **`min(capacity, 384_000)`** (so a fresh 1M-capacity model doesn't silently balloon to fill the whole window — cost/latency guard — while small-capacity models just use their capacity).
- **`band_headroom`** (advanced, default ~20% of target) — the band is **asymmetric**: `high_water = min(target, capacity)`, `low_water = max(target − headroom, 0)`. One number sets how far below target an eviction wave drops (the re-trigger hysteresis). Invariant enforced at load: `0 < low_water < high_water ≤ capacity`.
- **`recent_n`** — protected recent-turn count (never evictable).
- **Master enable** — back-compat: `compact_threshold_pct = 0` still disables all ACM (compaction + eviction). The old `token_ceiling` field is retired/folded (capacity is the hard bound; target is the soft knob).
- Wire the resolved target/capacity into the **live** turn config (today the ceiling only reaches the subagent path).

### 3.6a Effective target, output reserve, and the `capacity ≤ target` case

`capacity` is **total context = input + output**, so the gate must reserve room for the model's *response*; you can never target the full window. And a small model can have `capacity ≤ context_target`. Both are handled by deriving an **effective target** from the *active model's* capacity, recomputed on every model change:

```
output_reserve = model.max_output OR max(SAFETY_MARGIN, 0.1 * capacity)   // room to generate
usable         = capacity - output_reserve
effective_target = min(context_target, usable)                            // clamp: never > usable
high_water      = effective_target
low_water       = max(effective_target - headroom, recent_n_floor)
```

- **Normal (1M model, 384k target):** `effective_target = 384k`, band as designed.
- **`capacity ≤ target` (e.g. 32k model, 384k target):** `effective_target` collapses to `usable` (~28k) — the controller manages toward the biggest budget that still leaves room to respond. The band recomputes; the user's 384k is silently honored as "as much as this model allows."
- **Model switch mid-session** (e.g. 1M→32k with 300k live): recomputing the band leaves the working set far over the new `high_water`, so the **pre-flight gate performs a large eviction wave on the very next turn** before sending — exactly the mechanism that prevents an instant over-capacity 400.
- **Degenerate (tiny model, protected content alone > usable):** if System + `recent_n` turns already exceed `usable`, eviction cannot reach `low_water` without touching protected content (§6). Behavior: keep well-formedness, shrink the *effective* `recent_n` toward a floor of 1 if needed, and if even that overflows, surface a hard "model too small for this working set" warning rather than silently 400. This is the honest failure — a 32k model genuinely cannot hold a large in-flight task.

The band is thus always a function of the **current** model; there is no configuration in which `target` can exceed what the model can actually carry.

### 3.7 Retrieval & relevance seams (defined now, implemented in Slice 4)

Pure `zoid-core` traits, threaded as `Option<Arc<dyn …>>` (None in Slices 1–3). Defining them now costs a few trait declarations; retrofitting them later would touch the controller, the recall pipeline, and the store — so they go in from the start.

- **`trait Embedder { fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>; fn dim(&self) -> usize; fn model_id(&self) -> &str; }`** — in-process (candidate impls: `fastembed`/ONNX bge-small, or `candle`). Model discovered via the catalog (§3.0, `ModelKind::Embedding`, `embedding_dim`).
- **`trait Reranker { fn rerank(&self, query: &str, candidates: &[RecallCandidate]) -> Vec<Scored>; }`** — in-process cross-encoder; refines candidate order for precision.
- **`trait EvictionScorer { fn score(&self, turn: &TurnView, ctx: &GoalContext) -> f32; }`** — victim selection (§3.1). `RecencyScorer` default now; `RelevanceScorer` (embedding cosine to current goal) in Slice 4.
- **Retrieval pipeline** — `trait CandidateSource { fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate>; }`. Slice 2 pipeline = `[Fts5Source]`. Slice 4 = `[Fts5Source, VectorSource]` (hybrid: lexical + semantic) → `Reranker` → budgeted selection. Same pipeline serves the `recall` tool and proactive auto-recall (§11).
- **Embedding storage (reserved schema, populated in Slice 4):** `event_embeddings(event_id, model_id, dim, vector BLOB, PRIMARY KEY(event_id, model_id))`, filled lazily when an `Embedder` is present. Vector search via the `sqlite-vec` extension **or** brute-force cosine in-process over the bounded cold set — **[OPEN: pick in Slice 4; both fit the pure-core/effectful-bin split since vectors live in the bin's store.]**

All of the above stay out of the Slice 0–2 build; their *signatures* and the reserved table are the deliverable, so Slice 4 is additive.

### 3.8 Execution model — pre-flight gate vs async maintenance lane

Split by **whether the work gates correctness**.

**Synchronous PRE-FLIGHT gate (runs before `build_request`, every sub-turn).** *This is a change to `run_turn_inner`, not a drop-in.* Today (`agent.rs:273-377`) the request is built and streamed at the top of the loop and `record_compactions` runs *after* the response — so compaction/eviction only affect the *next* request. The new gate runs the cheap levers **ahead of the send**:

```
loop {
    // PRE-FLIGHT GATE (new position):
    est = estimate_request_tokens(events, calibration_ratio) * OVERCOUNT_BIAS
    if est >= high_water: compact_tool_results(); est = re-estimate
    if est >= high_water: plan_evictions() down to low_water   // append TurnsEvicted
    if est >= capacity - SAFETY_MARGIN: evict harder toward a hard floor
    req = build_request(events)                                 // now within budget
    match provider.stream(&req) {
        Err(ContextLengthExceeded) if retries_left =>          // estimate under-read reality
            { force_eviction_wave(); continue }                // bounded retry
        ...
    }
    record_usage()  // still learns calibration_ratio from real usage for the NEXT gate
}
```

The levers are cheap (eviction = pick turn ids + append one `TurnsEvicted` event; compaction = head/footer truncation), so running them synchronously costs microseconds. The **estimate is fallible** (undercounts code/tool output 5–7×), so the gate is *belt-and-suspenders*: over-count bias + safety margin below capacity make an over-send unlikely, and the **context-length-error retry makes "never exceed capacity" actually true** rather than aspirational. `calibration_ratio` is still learned from real usage post-response, feeding the *next* turn's pre-flight estimate — so accuracy improves over a session (and rides on debt-fix #5, correct `Usage` accumulation).

**Asynchronous maintenance lane (off hot path — event-driven/debounced, timer fallback).** The **expensive, non-gating** work: computing embeddings for new events, building/refreshing the vector index, any future LLM-based summarization, reranker precompute, cold-paging/archival (Slice 3). These write **side-table rows** (`event_embeddings`, vector index — §3.7), **never `EventKind` rows** (vectors in the event log would be replayed every frame, defeating §1.3). A lagging maintainer therefore can't corrupt a turn: the turn loop iterates its own working `Vec` (`agent.rs:261`, mutated only by its own emits — the async lane cannot touch it), and recall degrades gracefully to FTS/lexical when a vector isn't ready yet. **FTS5 indexing is the exception — it is cheap and runs synchronously in the actor's `Append` handler (one transaction: `events` insert + FTS insert), NOT on the async lane.** So "async" ⊃ {embeddings, vector index, summarization, cold-paging}; "sync-at-append" ⊃ {FTS row}; "pre-flight" ⊃ {compaction, eviction}.

**Concurrency precision (not hand-waved):** the async lane reaches `app.events` only via `AgentUpdate::Appended`, which bumps the `events.len()`-keyed `ProjectionCache` (`main.rs:756`) and forces a full `refresh` — a *perf* cost during streaming (it bypasses the O(1) `apply_streaming` fast path), **not** a correctness cost. The steady-state property test must drive this lane synchronously (M9) to stay deterministic.

**Why define the async lane now** even though Slices 0–2 put almost nothing on it: Slice 4's embed-every-event is the first genuinely expensive tenant, and reserving the lane as a seam avoids retrofitting async into the hot path later.

---

## 4. Data flow (one sub-turn)

1. **Pre-flight estimate:** `est = estimate(conversation(events)) × OVERCOUNT_BIAS` (calibrated from prior real usage; biased to over-count).
2. **Pre-flight gate:** if `est ≥ high_water` → compact tool results, re-estimate; if still over → `plan_evictions` down to `low_water` (append `TurnsEvicted`); if near `capacity − SAFETY_MARGIN` → evict harder toward a hard floor.
3. **Build & send:** `build_request(conversation(events))` — now within budget, skipping evicted ids and carrying the breadcrumb (as system-prompt text / prepended to the next user message, §3.3 — *not* a standalone message).
4. **Capacity-error catch:** if the provider returns context-length-exceeded, force an eviction wave and retry (bounded). This is what makes "never exceed capacity" true despite a fallible estimate.
5. **Post-response:** record `Usage`, updating `calibration_ratio` for the next sub-turn's estimate.
6. **Recall:** the model may call `recall(query)` → FTS5 → append `TurnsReadmitted` → matching turns re-enter the next assembly (with Retrieved-protection so they don't instantly re-evict, M10).
7. **UX:** transcript renders compaction (`⧟`) and eviction markers with semantic zoom; user can undo an eviction (re-admit) from the UI.

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

- **Steady-state property test (the missing coverage that let this regress):** simulate a long multi-turn session (hundreds of turns, large tool outputs); assert (M8) the pre-flight request stays **`≤ capacity` always** (hard), and **`≤ effective_target`** whenever evictable content exists (soft — it may exceed when only protected content remains, §6). Never drops below the recent-*N* floor. This "holds the band over time" property has **no test today** — its absence is the root cause the regression shipped.
- **Pre-flight ordering (C1):** the gate runs before the send — assert a turn that starts over `high_water` is reduced *in the same turn's* request, not the next.
- **Capacity-error retry (C1):** a mocked provider returning context-length-exceeded triggers a forced eviction wave and a bounded retry that then succeeds; unbounded loop is impossible.
- **Model-switch (§3.6a):** switching to a smaller-capacity model recomputes the band and forces the next pre-flight to evict down to the new `usable`; the degenerate tiny-model case surfaces a warning instead of a 400.
- **Async determinism (M9):** the maintenance lane is injectable/synchronous in tests (driven manually) so the steady-state test never races an index/embedding build.
- Eviction **idempotence / no-thrash:** re-running `plan_evictions` with no new pressure yields an empty plan; a single wave reaches low-water in one pass.
- **Protection invariants:** System/`Immutable`, `pinned`, and recent-*N* turns are never in any `EvictionPlan`.
- **Explicit-id (no cutoff):** an evicted id set removes exactly those turns, and later turns are unaffected.
- **Recall round-trip:** evict a turn, `recall(query)` finds it, `TurnsReadmitted` re-admits it, projection includes it again.
- **Undo restores** the exact evicted content.
- **Marker present** whenever anything is evicted (guards against silent amnesia).
- Reuse ACM-1's discipline: cross-crate field adds to shared types must be built with `--workspace` (a prior slice broke zoid-tui literals when tests were scoped to `-p zoid-core`).

## 8. Slicing & sequencing

- **Slice 0 — capacity correctness (minimal, per I5).** Fix the wrong `MODEL_CAPS` seed constants (→ 1M) and apply the already-wired async `fetch_model_info` override so any model gets a correct `capacity`. **No new DB table.** Plus the pre-flight target/band derivation incl. capacity clamp + output reserve (§3.6a). Small.
- **Slice 1 — pre-flight eviction gate + breadcrumb + capacity-error retry.** Restructures `run_turn_inner` so compaction+eviction run *before* the send (C1); breadcrumb out-of-band (I2); `EvictionScorer` seam (recency default). Holds the target, bounds per-turn CPU, guarantees ≤ capacity via the retry catch. Must-have; fixes the reported bug. **Coordinates with debt-fix #2 (turn-cancel) — same loop.**
- **Slice 2 — recall pipeline over cold sqlite (FTS5).** New actor `Cmd`s + `EventStore` FTS methods (I3); FTS synchronous-at-append (I4); `recall` tool. Defines the `Embedder`/`Reranker`/`CandidateSource` seams (`None`/`[Fts5Source]`) and reserves `event_embeddings`. **Built with Slice 1 as one coherent unit.**
- **Slice 3 — cold-paging + windowed resume.** Fixes the RAM/resume curve; reuses Slice 2's cold tier. **= debt-item #6** (unbounded + deep-cloned `Vec<Event>`) — one effort, not two.
- **Model-metadata catalog (decoupled slice).** The DB `model_metadata` table + resolver + TTL + provenance + embedding-model metadata (§3.0 "full catalog"). The user's "stop hardcoding → DB cache" end-state; **Slice 4 depends on it**. Runs on its own schedule, not inside the ACM plan. **Overlaps debt-item #8** (`zoid-tui`→`zoid-provider` catalog leak) — coordinate ownership.
- **Slice 4 — ML upgrade (ACM-2+).** In-process `Embedder` + `Reranker`, populate `event_embeddings`, hybrid retrieval, cross-encoder rerank, relevance-driven eviction, proactive auto-recall (§11). Depends on the catalog slice + Slices 0–2 seams. Deferrable.

**Implementation plan scope: Slices 0+1+2** (0 shrunk to a seed-fix per I5). Slice 3, the catalog slice, and Slice 4 are documented but out of the first plan.

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

## 12. Open questions

**Resolved (2026-07-04):**
- **Default `context_target`** = `min(capacity, 384_000)`. ✓
- **Band shape** = **asymmetric**: `high_water = target`, `low_water = target − headroom`. Never routinely exceed the dialed target. ✓

**Still open (decide inside the plan / Slice 4):**
- **Vector search backend** (Slice 4): `sqlite-vec` extension vs brute-force in-process cosine over the bounded cold set. (Both respect the seam.)
- **Async maintainer trigger constants** (§3.8): the N-events / T-idle thresholds, and whether Slice 0 catalog-refresh piggybacks the same lane or stays purely on-demand.
- **`headroom` default magnitude** (~20% of target assumed) — tune against real sessions.
