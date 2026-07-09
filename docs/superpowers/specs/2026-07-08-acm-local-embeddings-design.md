# ACM Slice 4 (v1): Local Embeddings + Hybrid Recall — Design

> **Status:** DESIGN APPROVED (brainstorming, 2026-07-08). Ready for writing-plans.
> Scope is the **smallest coherent slice** of the ACM Slice-4 ML upgrade: a local
> embedding model that makes `recall` semantic. Re-ranking and relevance-driven
> eviction are explicitly **deferred** (their seams stay `None`).

Parent design: `docs/superpowers/specs/2026-07-04-acm-demand-paged-context-design.md`
(§3.7 retrieval/relevance seams). Vision: `docs/superpowers/specs/2026-07-03-active-context-management-vision.md`
(Tier-1 local embeddings). This slice lights up the reserved `Embedder` /
`CandidateSource` seams and the reserved `event_embeddings` table; it is
**purely additive** — nothing the model already sees each turn changes.

---

## 1. Goal & scope

Give zoid a **local, bundled embedding model** so the existing `recall` tool
becomes **hybrid** (lexical FTS5 + semantic vector), retrieving paged-out history
by meaning, not just keyword. Everything runs **off the synchronous per-turn
pre-flight gate**.

**In scope (v1):**
- `CandleEmbedder` (bge-small-en-v1.5) behind an `Embedder` trait.
- `EmbeddingIndex` — an in-memory FIFO ring of vectors + brute-force cosine scan.
- `VectorSource` (a `CandidateSource`) added to the recall pipeline; hybrid merge
  via Reciprocal Rank Fusion (RRF).
- Async maintenance lane that embeds events and populates `event_embeddings`.
- Weights **downloaded on first use**, sha256-verified, cached under XDG.
- Compile feature `local-embed` (default-off) + runtime `[embed]` config.

**Deferred (later slices, seams stay `None`):**
- Cross-encoder `Reranker` (spike-validated: `ms-marco-MiniLM-L-6-v2`, 91 MB,
  ~21 ms/pair — see spike results; stays `NoopReranker`).
- `RelevanceScorer` + `heat_of` relevance term (eviction stays recency-based).
- Model-metadata DB catalog, model picker, quantization, proactive auto-recall.

---

## 2. Evidence base (spike results, 2026-07-08)

All risky assumptions were measured before this design was accepted. Full detail
in `spikes/embed-rerank-eval/PHASE0-RESULTS.md`, `PHASE1-RESULTS.md`,
`PHASE1b-RESULTS.md`, `VECSTORE-RESULTS.md`.

| Question | Result |
|---|---|
| Runtime that links for static-musl | **candle** PASS (2.25 MB); fastembed/`ort` FAIL — no musl ONNX Runtime (`ort-sys/build.rs:441`) |
| bge-small embed correctness/latency | PASS — cos 0.81 vs 0.37; ~30 ms/chunk; 130 MB; load 64 ms |
| Small reranker (deferred, de-risked) | `ms-marco-MiniLM-L-6-v2` PASS — 91 MB, ~21 ms/pair (hand-wired BERT seq-class head; candle-transformers 0.8 has no `BertForSequenceClassification`) |
| Resume-load + scan (ring, cap=150k) | LOAD 224 ms, SCAN 38 ms, RAM 220 MB at cap; bounded by `cap` not history (300k rows → same) |

Decision from the evidence: **candle** runtime; **brute-force in-memory** vector
search (no `sqlite-vec` — it is a C extension that would reintroduce the musl
linking pain the codebase deliberately avoids, and its `vec0` is itself a linear
scan, so it buys only a constant factor). Pure-Rust HNSW (`hnsw_rs`) is the clean
future escape hatch if sublinear search is ever needed.

---

## 3. Architecture

### 3.1 Crate boundaries
- **`zoid-core`** (pure, always compiled, candle-free):
  - Traits (already exist in `retrieval.rs`): `Embedder`, `CandidateSource`,
    `Reranker`, `EvictionScorer`, `RecallCandidate`, `Scored`.
  - **`EmbeddingIndex`** — the ring + cosine scan (pure math, no candle).
  - **`VectorSource : CandidateSource`** — embeds the query via an injected
    `&dyn Embedder`, scans the ring.
  - **RRF merge** — pure function fusing ranked candidate lists.
  - **`FakeEmbedder`** — deterministic test double.
- **`zoid-embed`** (optional leaf crate, feature-gated, the *only* candle user):
  - **`CandleEmbedder : Embedder`** — bge-small BERT forward, CLS pool + L2 norm.
  - **weight fetcher** — reqwest + rustls, sha256-verify, XDG cache.
- **`zoid` (bin)** — builds `EmbeddingIndex` (`Arc<RwLock<…>>`), constructs
  `CandleEmbedder` when the feature+config allow, spawns the embed lane, injects
  `VectorSource` into the recall pipeline, threads the index via `TurnConfig`
  (mirrors `mcp` / `kill` fields — the proven `zoid-mcp` wiring groove).

> Rationale: keeping the ring/scan/RRF/pipeline in `zoid-core` makes ~90% of the
> logic testable with `FakeEmbedder` in the fast, candle-free `cargo test` loop.
> `zoid-embed` is a thin candle wrapper.

### 3.2 EmbeddingIndex (in-memory vector cache)
Flat contiguous matrix + parallel id array, as a **fixed-capacity ring buffer**:

```rust
struct EmbeddingIndex {
    dim: usize,          // 384
    cap: usize,          // config: embed.max_vectors
    model_id: String,    // staleness guard
    vectors: Vec<f32>,   // cap*dim, preallocated; row r = [r*dim..(r+1)*dim]
    ids: Vec<EventId>,   // cap
    write: usize,        // next slot, wraps at cap
    len: usize,          // 0..cap
}
```
- **Rows are L2-normalized unit vectors** ⇒ cosine == dot product; the query is
  normalized once and the scan is a single fused multiply-add sweep + bounded
  top-K heap.
- **Flat `Vec<f32>`, not `Vec<Vec<f32>>`** — contiguous memory the CPU prefetches
  and vectorizes; this is what gives brute-force sqlite-vec-class speed.
- **FIFO ring:** at `cap`, insert overwrites the oldest slot (`O(dim)`, no
  realloc). Consequence: semantic recall covers a **recent-M window**; older
  events remain **lexically** recall-able (FTS still indexes all) and their
  vectors persist on disk — forward-compatible to a later full-scan/HNSW upgrade
  with no migration.
- **Concurrency:** `Arc<RwLock<EmbeddingIndex>>`. Writer = async lane (brief write
  lock to append; candle inference happens *outside* the lock). Readers = recall
  scans (shared read lock). Append-mostly + rare reads = ideal RwLock workload.
- **Write-once, never stale:** events are immutable in the log, so embeddings
  never invalidate. Only a **model change** invalidates — handled by `model_id`
  (rows from other models aren't loaded; the lane re-embeds, a one-time reindex).

### 3.3 Data flow

**Write path — async embed lane** (the ACM async maintenance lane, §3.8):
1. Boot: `LoadEmbeddings { model_id, cap }` → fill ring (`ORDER BY rowid DESC LIMIT cap`).
2. Debounced trigger on new events → `UnembeddedEvents { kinds, limit }` (a **disk**
   left-join, *not* a ring check — so aged-out events aren't re-embedded forever).
3. Batch texts → embed on a **dedicated blocking worker thread** (candle is
   CPU-bound; never on tokio async threads) → `WriteEmbedding { event_id, vector }`
   (side-table row only, never an `EventKind` row) + append to ring.
- Embeddable kinds: message + tool-result + file (the retrievable content set,
  matching what `recall` searches). System/internal events skipped.

**Read path — hybrid recall:**
1. `recall(query)` → embed query on the blocking worker (~30 ms).
2. `[Fts5Source (BM25), VectorSource (ring cosine scan)]` produce ranked candidates.
3. **RRF merge** (`score = Σ 1/(k + rank_i)`) — rank-based fusion, so BM25's
   unbounded scale and cosine's [−1,1] never need calibration.
4. Budgeted selection → `TurnsReadmitted` (existing re-admit path, M10 cooldown).
- Same pipeline shape as the parent design (`CandidateSource → (Reranker) →
  budget`); the `Reranker` slot stays `Noop` in v1.

### 3.4 Graceful degradation
Feature off, `enabled=false`, weights missing, or offline → `VectorSource` absent
or yields nothing → recall is **pure FTS5**, exactly today's behavior. No error,
no gate, no panic.

---

## 4. Control plane

Two orthogonal switches:

| Axis | Mechanism | Default |
|---|---|---|
| Compile | cargo feature `local-embed` on `zoid` bin (optional `dep:zoid-embed`) | **off** at workspace (fast candle-free test loop); **enabled for releases** via `dist-workspace.toml` `features = ["local-embed"]` + a CI capability assertion |
| Runtime | `[embed]` config (TOML, `zoid-core`) | active when compiled in |

```toml
[embed]
enabled       = true    # master switch (default true when compiled in)
max_vectors   = 50000   # ring cap → RAM knob (≈73 MB @ 50k; ≈220 MB @ 150k)
auto_download = true    # fetch weights on first use; false = use only if present
```
Model id is fixed to **bge-small-en-v1.5** in v1 (no picker; deferred to registry
work). `zoid-model` gains `ModelKind` + `embedding_dim` only if the plan finds it
needed; v1 can hardcode dim 384.

**Build profile:** `[profile.release.package.zoid-embed] opt-level = 3` so the
cosine scan vectorizes while the binary stays `opt-level = "z"`.

**Weights delivery:** download-on-first-use from pinned URLs, sha256-verify
against pinned hashes, cache under XDG (mirrors total-recall's `fetch-bge-small`
bootstrap). Release pipeline and binary size **unchanged**. No new `hf-hub`
dependency — reuse in-tree `reqwest` + `rustls-tls` (proven musl-clean in Phase 0).

---

## 5. Testing strategy

Most logic is candle-free and runs in the normal loop via `FakeEmbedder`:

- **RRF** unit tests: identical lists→same order; disjoint→rank-interleave;
  empty vector source→**equals FTS order** (degradation guarantee).
- **Ring/FIFO:** fill past `cap` → oldest overwritten, `len==cap`, correct ids,
  top-K scan correct.
- **Resume load:** temp sqlite → `LoadEmbeddings` → ring == recent-`cap`, newest-first.
- **Async lane determinism (M9):** lane exposes synchronous `tick()` so tests
  drive it manually (no thread race): insert events → tick with `FakeEmbedder` →
  `event_embeddings` filled + ring appended + **disk-dedup** verified.
- **Graceful degradation:** pipeline with only `Fts5Source` → recall works, no panic.
- **Weight fetcher (`zoid-embed`):** sha256 mismatch → rejected; local fixture
  blob, **no network in tests**.
- **Real-model smoke (`zoid-embed`, gated):** `#[cfg(feature="local-embed")]` +
  `#[ignore]`-by-default — load real bge-small, assert dim 384 + cosine sanity.
  Gated like the existing `zoid-mcp` heavy e2e tests so CI never downloads 130 MB.

---

## 6. Out of scope / non-goals
- Cross-encoder re-ranking (deferred; model + latency already de-risked).
- Relevance-driven eviction / `heat_of` relevance term (unchanged: recency).
- Model-metadata DB catalog, model picker, quantization (f16/int8), pure-Rust
  HNSW, proactive auto-recall, GPU. All are clean later-slice additions over the
  seams this slice lights up.
