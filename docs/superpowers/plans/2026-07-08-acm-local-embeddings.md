# ACM Local Embeddings + Hybrid Recall Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give zoid a local, bundled embedding model (bge-small via candle) so the existing `recall` tool becomes hybrid (lexical FTS5 + semantic vector), retrieving paged-out history by meaning — additively, changing nothing the model already sees each turn.

**Architecture:** A pure `EmbeddingIndex` (fixed-capacity FIFO ring of L2-normalized vectors + brute-force cosine scan) and a `VectorSource` live in `zoid-core` (candle-free, testable with a `FakeEmbedder`). The real `CandleEmbedder` + weight fetcher live in a new feature-gated leaf crate `zoid-embed` (the only candle user). An async maintenance lane embeds events off the hot path into the already-reserved `event_embeddings` table; recall merges FTS + vector candidates with Reciprocal Rank Fusion (RRF). All wiring mirrors the proven `zoid-mcp` groove (pure trait in core, heavy impl in optional leaf crate, `Option<Arc<…>>` on `TurnConfig`, assembled in the bin).

**Tech Stack:** Rust 2021 workspace; `candle-core`/`candle-nn`/`candle-transformers` 0.8 + `tokenizers` 0.20 (in `zoid-embed` only); `rusqlite` (bundled, FTS5); in-tree `reqwest` (rustls-tls) for weight download; `ulid::Ulid` ids; `tokio` mpsc/oneshot actor.

## Global Constraints

Every task's requirements implicitly include these (verbatim from the spec):

- **Runtime is candle only — never ONNX/`ort`.** Phase-0 spike proved `ort` cannot link static-musl (`ort-sys/build.rs:441`: no musl binaries); candle PASSED.
- **Must build for all three release targets** (`dist-workspace.toml`): `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. candle CPU path is pure-Rust on all three.
- **No new heavy dependencies.** Reuse in-tree `reqwest` + `rustls-tls` for weight download — do NOT add `hf-hub` (its default `native-tls` drags OpenSSL, which fails musl — Phase-0 finding).
- **Binary stays `opt-level = "z"`;** add a per-crate override `[profile.release.package.zoid-embed] opt-level = 3` so the cosine scan vectorizes.
- **Compile feature `local-embed` defaults OFF** at the workspace (keeps `cargo test` candle-free); releases enable it via `dist-workspace.toml`.
- **Graceful degradation is mandatory:** feature off, `enabled=false`, weights missing, or offline → recall is pure FTS5, no error, no panic.
- **`event_embeddings` writes are side-table only** — never an `EventKind` row (they must not be replayed by projections).
- TDD throughout: failing test → run-fail → minimal impl → run-pass → commit.

Evidence base for all sizing/latency claims: `spikes/embed-rerank-eval/{PHASE0,PHASE1,PHASE1b,VECSTORE}-RESULTS.md`.

---

### Task 1: `[embed]` config section

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (add `EmbedConfig`, wire into `Config`, `PartialEmbed`, merge)
- Test: `crates/zoid-core/src/config.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `EmbedConfig { enabled: bool, max_vectors: usize, auto_download: bool }`, reachable as `Config::default().embed`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `config.rs`:

```rust
#[test]
fn embed_defaults_and_parse() {
    let c = Config::default();
    assert!(c.embed.enabled);
    assert_eq!(c.embed.max_vectors, 50_000);
    assert!(c.embed.auto_download);

    let (p, _warn) = parse_toml("[embed]\nenabled = false\nmax_vectors = 1000").unwrap();
    assert_eq!(p.embed.enabled, Some(false));
    assert_eq!(p.embed.max_vectors, Some(1000));
    let (cfg, _prov) = merge(&[(Source::UserGlobal, p)]);
    assert!(!cfg.embed.enabled);
    assert_eq!(cfg.embed.max_vectors, 1000);
    assert!(cfg.embed.auto_download); // default preserved when absent
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core embed_defaults_and_parse`
Expected: FAIL — `no field embed on type Config`.

- [ ] **Step 3: Write minimal implementation**

Add the struct + default near `EconomyConfig`:

```rust
// Mirror EconomyConfig (config.rs:74) — Config derives Eq (config.rs:27), so
// every nested config struct must too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct EmbedConfig {
    /// Master switch (default true when compiled in with feature `local-embed`).
    pub enabled: bool,
    /// Ring-buffer capacity = the RAM knob (≈73 MB @ 50k, ≈220 MB @ 150k).
    pub max_vectors: usize,
    /// Fetch model weights on first use; false = use only if already cached.
    pub auto_download: bool,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self { enabled: true, max_vectors: 50_000, auto_download: true }
    }
}
```

Add `pub embed: EmbedConfig,` to `struct Config` and `embed: EmbedConfig::default(),` to `impl Default for Config`. Add the partial mirroring `PartialEconomy`:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEmbed {
    pub enabled: Option<bool>,
    pub max_vectors: Option<usize>,
    pub auto_download: Option<bool>,
}
```

Add `pub embed: PartialEmbed,` to the top-level partial config struct (the one holding `economy: PartialEconomy`), and in `merge()` apply each field: `if let Some(v) = p.embed.enabled { cfg.embed.enabled = v; }` (repeat for `max_vectors`, `auto_download`), following exactly how `economy` fields are merged.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core embed_defaults_and_parse`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [embed] section (enabled/max_vectors/auto_download)"
```

---

### Task 2: `EmbeddingIndex` ring buffer + cosine scan

**Files:**
- Create: `crates/zoid-core/src/embed_index.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod embed_index;`)

**Interfaces:**
- Produces: `EmbeddingIndex::new(dim: usize, cap: usize)`, `append(&mut self, id: Ulid, vec: &[f32])`, `scan_topk(&self, query: &[f32], k: usize) -> Vec<(Ulid, f32)>`, `len(&self) -> usize`, `dim(&self) -> usize`. Rows are stored as given (callers pass L2-normalized unit vectors); cosine == dot.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-core/src/embed_index.rs`:

```rust
//! In-memory FIFO ring of L2-normalized embedding vectors + brute-force cosine
//! (= dot, since unit vectors) top-K scan. Pure; no candle. Bounded by `cap`:
//! resume-load, RAM, and scan cost are all O(cap), never O(history)
//! (spikes/embed-rerank-eval/VECSTORE-RESULTS.md).

use ulid::Ulid;

pub struct EmbeddingIndex {
    // Intentionally NO model_id field: staleness is handled upstream by only
    // loading/writing the active model's rows (store methods filter by model_id).
    dim: usize,
    cap: usize,
    vectors: Vec<f32>, // len == len_rows*dim, row r at [r*dim..(r+1)*dim]
    ids: Vec<Ulid>,
    write: usize, // next slot to overwrite once full
    len: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: f32, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|i| (seed + i as f32).sin()).collect();
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v { *x /= n; }
        v
    }

    #[test]
    fn ring_overwrites_oldest_at_cap() {
        let mut idx = EmbeddingIndex::new(4, 2);
        idx.append(Ulid::from(1u128), &unit(1.0, 4));
        idx.append(Ulid::from(2u128), &unit(2.0, 4));
        idx.append(Ulid::from(3u128), &unit(3.0, 4)); // evicts id 1
        assert_eq!(idx.len(), 2);
        let hits = idx.scan_topk(&unit(3.0, 4), 2);
        let ids: Vec<u128> = hits.iter().map(|(id, _)| u128::from(*id)).collect();
        assert!(ids.contains(&3) && ids.contains(&2));
        assert!(!ids.contains(&1), "oldest id 1 was overwritten");
    }

    #[test]
    fn scan_ranks_nearest_first() {
        let mut idx = EmbeddingIndex::new(4, 8);
        let q = unit(5.0, 4);
        idx.append(Ulid::from(10u128), &q); // identical → cosine ~1
        idx.append(Ulid::from(11u128), &unit(99.0, 4));
        let hits = idx.scan_topk(&q, 1);
        assert_eq!(u128::from(hits[0].0), 10);
        assert!(hits[0].1 > 0.99);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core embed_index`
Expected: FAIL — `EmbeddingIndex::new` not found.

- [ ] **Step 3: Write minimal implementation**

Above the tests in `embed_index.rs`:

```rust
impl EmbeddingIndex {
    pub fn new(dim: usize, cap: usize) -> Self {
        let cap = cap.max(1);
        Self { dim, cap, vectors: Vec::with_capacity(cap * dim), ids: Vec::with_capacity(cap), write: 0, len: 0 }
    }

    pub fn dim(&self) -> usize { self.dim }
    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Append (overwriting the oldest slot once full). `vec.len()` must == dim;
    /// mismatched vectors are ignored (defensive).
    pub fn append(&mut self, id: Ulid, vec: &[f32]) {
        if vec.len() != self.dim { return; }
        if self.len < self.cap {
            self.vectors.extend_from_slice(vec);
            self.ids.push(id);
            self.len += 1;
        } else {
            let off = self.write * self.dim;
            self.vectors[off..off + self.dim].copy_from_slice(vec);
            self.ids[self.write] = id;
        }
        self.write = (self.write + 1) % self.cap;
    }

    /// Top-K by dot product (== cosine for unit vectors). Query need not be
    /// pre-truncated; only the first `dim` values are used.
    pub fn scan_topk(&self, query: &[f32], k: usize) -> Vec<(Ulid, f32)> {
        if query.len() < self.dim || k == 0 { return Vec::new(); }
        let q = &query[..self.dim];
        let mut top: Vec<(Ulid, f32)> = Vec::with_capacity(k + 1);
        for r in 0..self.len {
            let row = &self.vectors[r * self.dim..(r + 1) * self.dim];
            let mut s = 0.0f32;
            for i in 0..self.dim { s += q[i] * row[i]; }
            if top.len() < k {
                top.push((self.ids[r], s));
                top.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            } else if s > top[0].1 {
                top[0] = (self.ids[r], s);
                let mut j = 0;
                while j + 1 < top.len() && top[j].1 > top[j + 1].1 { top.swap(j, j + 1); j += 1; }
            }
        }
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap()); // best first
        top
    }
}
```

Add `pub mod embed_index;` to `crates/zoid-core/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core embed_index`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/embed_index.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): EmbeddingIndex ring buffer + brute-force cosine scan"
```

---

### Task 3: `FakeEmbedder` + `VectorSource`

**Files:**
- Modify: `crates/zoid-core/src/retrieval.rs` (add `FakeEmbedder`, `VectorSource`)

**Interfaces:**
- Consumes: `Embedder` trait, `EmbeddingIndex`, `CandidateSource`, `RecallCandidate` (all existing).
- Produces: `FakeEmbedder::new(dim: usize)` (deterministic unit vectors), `VectorSource::new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>)` implementing `CandidateSource`. `VectorSource::embed_unit(&self, text: &str) -> Option<Vec<f32>>` (public helper the lane reuses).

- [ ] **Step 1: Write the failing test**

Add to `retrieval.rs` (extend imports: `use crate::embed_index::EmbeddingIndex; use std::sync::{Arc, RwLock};`):

```rust
#[cfg(test)]
mod vector_tests {
    use super::*;
    use crate::embed_index::EmbeddingIndex;
    use std::sync::{Arc, RwLock};

    #[test]
    fn vectorsource_finds_seeded_event() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        // seed the index with the embedding of a known text
        let v = emb.embed(&["alpha beta"]).unwrap().remove(0);
        idx.write().unwrap().append(Ulid::from(7u128), &v);
        idx.write().unwrap().append(
            Ulid::from(8u128),
            &emb.embed(&["totally different"]).unwrap().remove(0),
        );
        let vs = VectorSource::new(emb, idx);
        let cands = vs.candidates("alpha beta", 1);
        assert_eq!(cands.len(), 1);
        assert_eq!(u128::from(cands[0].event_id), 7);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core vectorsource_finds_seeded_event`
Expected: FAIL — `FakeEmbedder`/`VectorSource` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `retrieval.rs` (non-test scope):

```rust
use crate::embed_index::EmbeddingIndex;
use std::sync::{Arc, RwLock};

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 { for x in &mut v { *x /= n; } }
    v
}

/// Deterministic test/dev embedder: hashes tokens into a fixed-dim unit vector.
/// Not semantic — only for tests and the candle-free build path.
pub struct FakeEmbedder { dim: usize }
impl FakeEmbedder { pub fn new(dim: usize) -> Self { Self { dim } } }
impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| {
            let mut v = vec![0f32; self.dim];
            for tok in t.split_whitespace() {
                let mut h: u64 = 1469598103934665603;
                for b in tok.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
                v[(h as usize) % self.dim] += 1.0;
            }
            l2_normalize(v)
        }).collect())
    }
    fn dim(&self) -> usize { self.dim }
    fn model_id(&self) -> &str { "fake" }
}

/// Semantic recall stage: embeds the query and scans the in-memory index.
pub struct VectorSource {
    embedder: Arc<dyn Embedder>,
    index: Arc<RwLock<EmbeddingIndex>>,
}
impl VectorSource {
    pub fn new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>) -> Self {
        Self { embedder, index }
    }
    /// Embed one text to a unit vector (reused by the lane to fill the index).
    pub fn embed_unit(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder.embed(&[text]).ok().and_then(|mut v| v.pop()).map(l2_normalize)
    }
}
impl CandidateSource for VectorSource {
    fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate> {
        let Some(q) = self.embed_unit(query) else { return Vec::new() };
        let idx = self.index.read().unwrap();
        idx.scan_topk(&q, k).into_iter().map(|(event_id, score)| RecallCandidate {
            event_id, content: String::new(), lexical_score: score,
        }).collect()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core vectorsource_finds_seeded_event`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/retrieval.rs
git commit -m "feat(core): FakeEmbedder + VectorSource (CandidateSource over the index)"
```

---

### Task 4: RRF hybrid merge

**Files:**
- Create: `crates/zoid-core/src/hybrid.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod hybrid;`)

**Interfaces:**
- Produces: `hybrid_recall(fts_ids: &[Ulid], vector_ids: &[Ulid], limit: usize) -> Vec<Ulid>` — Reciprocal Rank Fusion (`k=60`) of two rank-ordered id lists, deduplicated, best-first, truncated to `limit`.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-core/src/hybrid.rs`:

```rust
//! Reciprocal Rank Fusion for hybrid recall. Merges two rank-ordered id lists
//! (FTS/BM25 and vector/cosine) by RANK, not raw score — so BM25's unbounded
//! scale and cosine's [-1,1] never need calibration. score = Σ 1/(k + rank_i).

use ulid::Ulid;

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u128) -> Ulid { Ulid::from(n) }

    #[test]
    fn identical_lists_preserve_order() {
        let a = [id(1), id(2), id(3)];
        assert_eq!(hybrid_recall(&a, &a, 10), a.to_vec());
    }

    #[test]
    fn empty_vector_equals_fts_order() {
        let fts = [id(5), id(6), id(7)];
        assert_eq!(hybrid_recall(&fts, &[], 10), fts.to_vec());
    }

    #[test]
    fn disjoint_lists_interleave_by_rank() {
        let fts = [id(1), id(2)];
        let vec = [id(3), id(4)];
        // rank-0 of each source tie (id1, id3), then rank-1 (id2, id4)
        let out = hybrid_recall(&fts, &vec, 10);
        assert_eq!(out.len(), 4);
        assert!(out[..2].contains(&id(1)) && out[..2].contains(&id(3)));
        assert!(out[2..].contains(&id(2)) && out[2..].contains(&id(4)));
    }

    #[test]
    fn shared_id_ranks_higher_than_singletons() {
        let fts = [id(1), id(9)];
        let vec = [id(2), id(9)];
        let out = hybrid_recall(&fts, &vec, 10);
        assert_eq!(out[0], id(9), "id present in both sources wins");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core hybrid`
Expected: FAIL — `hybrid_recall` not found.

- [ ] **Step 3: Write minimal implementation**

Above the tests:

```rust
const RRF_K: f32 = 60.0;

pub fn hybrid_recall(fts_ids: &[Ulid], vector_ids: &[Ulid], limit: usize) -> Vec<Ulid> {
    use std::collections::HashMap;
    let mut score: HashMap<Ulid, f32> = HashMap::new();
    let mut order: Vec<Ulid> = Vec::new(); // first-seen order, for stable ties
    for list in [fts_ids, vector_ids] {
        for (rank, id) in list.iter().enumerate() {
            let e = score.entry(*id).or_insert_with(|| { order.push(*id); 0.0 });
            *e += 1.0 / (RRF_K + rank as f32);
        }
    }
    order.sort_by(|a, b| {
        score[b].partial_cmp(&score[a]).unwrap()
            .then_with(|| a.cmp(b)) // deterministic tie-break by id
    });
    order.truncate(limit);
    order
}
```

Add `pub mod hybrid;` to `lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core hybrid`
Expected: PASS (all four).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/hybrid.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): RRF hybrid_recall merge (rank fusion, no score calibration)"
```

---

### Task 5: EventStore embedding methods

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add three methods + BLOB helpers)

**Interfaces:**
- Produces on `EventStore`:
  - `write_embedding(&self, event_id: Ulid, model_id: &str, vector: &[f32]) -> Result<()>`
  - `load_recent_embeddings(&self, model_id: &str, cap: usize) -> Result<Vec<(Ulid, Vec<f32>)>>` (newest-first)
  - `unembedded_events(&self, model_id: &str, session_id: Ulid, limit: usize) -> Result<Vec<(Ulid, String)>>` (disk left-join FTS↔embeddings)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `store.rs` (reuse the existing temp-DB + append helpers; model after `append_indexes_searchable_content`):

```rust
#[test]
fn embeddings_write_load_and_unembedded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.db");
    let store = EventStore::open(path.to_str().unwrap()).unwrap();
    let sid = Ulid::from(1u128);
    // insert_session takes 5 args (store.rs:257): id, name, root_path, created_ts, last_touched_ts
    store.insert_session(sid, "s", "/tmp", 0, 0).unwrap();

    // two searchable events (go into events_fts at append). NOTE: Event::new's
    // 2nd arg is `parent`, NOT session (event.rs:187) — set the session with
    // `.with_session(sid)` (event.rs:200) so unembedded_events' session filter matches.
    let e1 = Event::new(Ulid::from(10u128), None, 1, EventKind::UserMessage { text: "hello world".into() }).with_session(sid);
    let e2 = Event::new(Ulid::from(11u128), None, 1, EventKind::UserMessage { text: "second body".into() }).with_session(sid);
    store.append(&e1).unwrap();
    store.append(&e2).unwrap();

    // both unembedded initially
    let todo = store.unembedded_events("bge", sid, 10).unwrap();
    assert_eq!(todo.len(), 2);
    assert!(todo.iter().any(|(id, c)| *id == Ulid::from(10u128) && c.contains("hello")));

    // embed one; it drops out of the unembedded set
    store.write_embedding(Ulid::from(10u128), "bge", &[0.1, 0.2, 0.3]).unwrap();
    let todo2 = store.unembedded_events("bge", sid, 10).unwrap();
    assert_eq!(todo2.len(), 1);
    assert_eq!(todo2[0].0, Ulid::from(11u128));

    // load round-trips the vector
    let loaded = store.load_recent_embeddings("bge", 10).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].0, Ulid::from(10u128));
    assert_eq!(loaded[0].1, vec![0.1, 0.2, 0.3]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core embeddings_write_load_and_unembedded`
Expected: FAIL — `write_embedding` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `impl EventStore` (and two free helpers near the top of the file):

```rust
fn f32s_to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v { b.extend_from_slice(&f.to_le_bytes()); }
    b
}
fn blob_to_f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}
```

```rust
    /// Persist one embedding (side-table row; never replayed). Idempotent per
    /// (event_id, model_id).
    pub fn write_embedding(&self, event_id: Ulid, model_id: &str, vector: &[f32]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO event_embeddings (event_id, model_id, dim, vector)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event_id.to_string(), model_id, vector.len() as i64, f32s_to_blob(vector)],
        )?;
        Ok(())
    }

    /// Newest-first, capped — the resume-fill query. O(cap), not O(history).
    pub fn load_recent_embeddings(&self, model_id: &str, cap: usize) -> Result<Vec<(Ulid, Vec<f32>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, vector FROM event_embeddings
             WHERE model_id = ?1 ORDER BY rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![model_id, cap as i64], |r| {
            let id: String = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            Ok((id, blob))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            if let Ok(u) = Ulid::from_string(&id) { out.push((u, blob_to_f32s(&blob))); }
        }
        Ok(out)
    }

    /// Searchable events lacking an embedding for `model_id`, in this session.
    /// Content comes from `events_fts` (same set `recall` searches).
    pub fn unembedded_events(&self, model_id: &str, session_id: Ulid, limit: usize) -> Result<Vec<(Ulid, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.event_id, f.content FROM events_fts f
             LEFT JOIN event_embeddings e ON e.event_id = f.event_id AND e.model_id = ?1
             WHERE e.event_id IS NULL AND f.session_id = ?2
             ORDER BY f.rowid LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![model_id, session_id.to_string(), limit as i64], |r| {
            let id: String = r.get(0)?;
            let content: String = r.get(1)?;
            Ok((id, content))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, content) = row?;
            if let Ok(u) = Ulid::from_string(&id) { out.push((u, content)); }
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core embeddings_write_load_and_unembedded`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(core): EventStore embedding write/load/unembedded methods"
```

---

### Task 6: Actor Cmds + SessionHandle methods

**Files:**
- Modify: `crates/zoid-core/src/session.rs` (new `Cmd` variants + handler arms + async handle methods)

**Interfaces:**
- Produces on `SessionHandle`:
  - `write_embedding(&self, event_id: Ulid, model_id: String, vector: Vec<f32>) -> Result<()>`
  - `load_recent_embeddings(&self, model_id: String, cap: usize) -> Result<Vec<(Ulid, Vec<f32>)>>`
  - `unembedded_events(&self, model_id: String, session_id: Ulid, limit: usize) -> Result<Vec<(Ulid, String)>>`
  - `events_by_ids(&self, ids: Vec<Ulid>, session_id: Ulid) -> Result<Vec<Event>>`

- [ ] **Step 1: Write the failing test**

Add a `#[tokio::test]` to `session.rs` tests:

```rust
#[tokio::test]
async fn handle_embedding_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let h = SessionHandle::spawn(dir.path().join("s.db").to_str().unwrap()).unwrap();
    let sid = Ulid::from(1u128);
    h.new_session(sid, "s".into(), "/tmp".into(), 0).await.unwrap();
    // 2nd arg is parent, not session — set session via .with_session (event.rs:200).
    let ev = Event::new(Ulid::from(10u128), None, 1, crate::event::EventKind::UserMessage { text: "hi there".into() }).with_session(sid);
    h.append(ev).await.unwrap();

    assert_eq!(h.unembedded_events("bge".into(), sid, 10).await.unwrap().len(), 1);
    h.write_embedding(Ulid::from(10u128), "bge".into(), vec![0.5, 0.5]).await.unwrap();
    assert_eq!(h.unembedded_events("bge".into(), sid, 10).await.unwrap().len(), 0);
    let loaded = h.load_recent_embeddings("bge".into(), 10).await.unwrap();
    assert_eq!(loaded, vec![(Ulid::from(10u128), vec![0.5, 0.5])]);
    let evs = h.events_by_ids(vec![Ulid::from(10u128)], sid).await.unwrap();
    assert_eq!(evs.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core handle_embedding_roundtrip`
Expected: FAIL — `write_embedding` not found on `SessionHandle`.

- [ ] **Step 3: Write minimal implementation**

Add `Cmd` variants (mirroring `Recall`):

```rust
    WriteEmbedding { event_id: Ulid, model_id: String, vector: Vec<f32>, reply: oneshot::Sender<Result<()>> },
    LoadRecentEmbeddings { model_id: String, cap: usize, reply: oneshot::Sender<Result<Vec<(Ulid, Vec<f32>)>>> },
    UnembeddedEvents { model_id: String, session_id: Ulid, limit: usize, reply: oneshot::Sender<Result<Vec<(Ulid, String)>>> },
    EventsByIds { ids: Vec<Ulid>, session_id: Ulid, reply: oneshot::Sender<Result<Vec<Event>>> },
```

Add handler arms in the actor loop (next to `Cmd::Recall`):

```rust
    Cmd::WriteEmbedding { event_id, model_id, vector, reply } => {
        let _ = reply.send(store.write_embedding(event_id, &model_id, &vector));
    }
    Cmd::LoadRecentEmbeddings { model_id, cap, reply } => {
        let _ = reply.send(store.load_recent_embeddings(&model_id, cap));
    }
    Cmd::UnembeddedEvents { model_id, session_id, limit, reply } => {
        let _ = reply.send(store.unembedded_events(&model_id, session_id, limit));
    }
    Cmd::EventsByIds { ids, session_id, reply } => {
        let _ = reply.send(store.events_by_ids(&ids, session_id));
    }
```

Add the async handle methods (mirror `recall`, which builds a oneshot, sends the Cmd, awaits):

```rust
    pub async fn write_embedding(&self, event_id: Ulid, model_id: String, vector: Vec<f32>) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::WriteEmbedding { event_id, model_id, vector, reply }).await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
    pub async fn load_recent_embeddings(&self, model_id: String, cap: usize) -> Result<Vec<(Ulid, Vec<f32>)>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::LoadRecentEmbeddings { model_id, cap, reply }).await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
    pub async fn unembedded_events(&self, model_id: String, session_id: Ulid, limit: usize) -> Result<Vec<(Ulid, String)>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::UnembeddedEvents { model_id, session_id, limit, reply }).await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
    pub async fn events_by_ids(&self, ids: Vec<Ulid>, session_id: Ulid) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Cmd::EventsByIds { ids, session_id, reply }).await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core handle_embedding_roundtrip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/session.rs
git commit -m "feat(core): actor Cmds for embeddings (write/load/unembedded/events_by_ids)"
```

---

### Task 7: Embed lane (`tick`)

**Files:**
- Create: `crates/zoid-core/src/embed_lane.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod embed_lane;`)

**Interfaces:**
- Consumes: `Embedder`, `EmbeddingIndex`, `VectorSource::embed_unit` (via a shared embedder).
- Produces: `EmbedLane::new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>)`, `tick(&self, batch: &[(Ulid, String)]) -> Vec<(Ulid, Vec<f32>)>` — embeds each text to a unit vector, appends it to the index, and returns the `(id, vector)` rows for the caller to persist. Pure w.r.t. the DB (caller owns persistence) → deterministic in tests (M9).

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-core/src/embed_lane.rs`:

```rust
//! The async maintenance lane's pure core: embed a batch of (id, text), append
//! to the in-memory index, and return rows to persist. Synchronous + injectable
//! so tests drive it deterministically with a FakeEmbedder (spec §5, M9). The
//! bin wraps this with store fetch (unembedded_events) + store write.

use crate::embed_index::EmbeddingIndex;
use crate::retrieval::Embedder;
use std::sync::{Arc, RwLock};
use ulid::Ulid;

pub struct EmbedLane {
    embedder: Arc<dyn Embedder>,
    index: Arc<RwLock<EmbeddingIndex>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::FakeEmbedder;

    #[test]
    fn tick_embeds_appends_and_returns_rows() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        let lane = EmbedLane::new(emb, idx.clone());
        let batch = vec![(Ulid::from(1u128), "alpha".to_string()), (Ulid::from(2u128), "beta".to_string())];
        let rows = lane.tick(&batch);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, Ulid::from(1u128));
        assert_eq!(rows[0].1.len(), 16);
        assert_eq!(idx.read().unwrap().len(), 2);
    }

    #[test]
    fn tick_is_deterministic() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        let lane = EmbedLane::new(emb, idx);
        let batch = vec![(Ulid::from(1u128), "same text".to_string())];
        assert_eq!(lane.tick(&batch)[0].1, lane.tick(&batch)[0].1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core embed_lane`
Expected: FAIL — `EmbedLane::new` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
impl EmbedLane {
    pub fn new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>) -> Self {
        Self { embedder, index }
    }

    /// Embed each (id, text) → unit vector, append to the index, return rows to
    /// persist. Embedding happens outside the index lock; only the append takes
    /// the write lock (briefly).
    pub fn tick(&self, batch: &[(Ulid, String)]) -> Vec<(Ulid, Vec<f32>)> {
        if batch.is_empty() { return Vec::new(); }
        let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
        let embs = match self.embedder.embed(&texts) {
            Ok(e) => e,
            Err(_) => return Vec::new(), // degrade: skip this batch, no panic
        };
        let mut out = Vec::with_capacity(batch.len());
        let mut idx = self.index.write().unwrap();
        for ((id, _), raw) in batch.iter().zip(embs.into_iter()) {
            let v = normalize(raw);
            idx.append(*id, &v);
            out.push((*id, v));
        }
        out
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 { for x in &mut v { *x /= n; } }
    v
}
```

Add `pub mod embed_lane;` to `lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core embed_lane`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/embed_lane.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): EmbedLane.tick — deterministic embed+append (M9)"
```

---

### Task 8: `zoid-embed` crate — CandleEmbedder + weight fetcher

**Files:**
- Create: `crates/zoid-embed/Cargo.toml`
- Create: `crates/zoid-embed/src/lib.rs`
- Create: `crates/zoid-embed/src/fetch.rs`
- Modify: root `Cargo.toml` (add `"crates/zoid-embed"` to `members`)

**Interfaces:**
- Consumes: `zoid_core::retrieval::Embedder`.
- Produces: `zoid_embed::CandleEmbedder::load(cache_dir: &Path, auto_download: bool) -> anyhow::Result<CandleEmbedder>` implementing `Embedder` (dim 384, model_id `"bge-small-en-v1.5"`); `zoid_embed::fetch::ensure_weights(cache_dir: &Path, auto_download: bool) -> anyhow::Result<WeightPaths>` (sha256-verified).

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-embed/Cargo.toml`:

```toml
[package]
name = "zoid-embed"
version = "0.1.2"
edition = "2021"

[dependencies]
zoid-core = { path = "../zoid-core" }
candle-core = "0.8"
candle-nn = "0.8"
candle-transformers = "0.8"
tokenizers = { version = "0.20", default-features = false, features = ["onig"] }
anyhow = { workspace = true }
serde_json = { workspace = true }
# workspace reqwest lacks `blocking`; fetch.rs uses reqwest::blocking::get
reqwest = { workspace = true, features = ["blocking"] }
sha2 = "0.10"

[dev-dependencies]
tempfile = { workspace = true }
```

Create `crates/zoid-embed/src/fetch.rs` with a sha256 test first:

```rust
//! Weight fetcher: download model files from pinned URLs via in-tree reqwest
//! (rustls), sha256-verify against pinned hashes, cache under `cache_dir`. NO
//! hf-hub dep (its native-tls drags OpenSSL, which fails musl — Phase-0).

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub struct WeightPaths { pub config: PathBuf, pub tokenizer: PathBuf, pub weights: PathBuf }

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
    #[test]
    fn verify_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, b"corrupt").unwrap();
        assert!(verify_file(&p, "0000000000000000000000000000000000000000000000000000000000000000").is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-embed sha256`
Expected: FAIL — `verify_file` not found (and crate may need `members` entry; add it now if the workspace errors).

- [ ] **Step 3: Write minimal implementation**

Add to `fetch.rs`:

```rust
pub(crate) fn verify_file(path: &Path, want_sha256: &str) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let got = sha256_hex(&bytes);
    if got != want_sha256 { bail!("sha256 mismatch for {}: got {got}, want {want_sha256}", path.display()); }
    Ok(())
}

// Pinned artifacts for bge-small-en-v1.5. URLs resolve to the HF CDN; hashes
// pin exact bytes. (Fill exact sha256 values from the downloaded files during
// implementation — see spikes/embed-rerank-eval/candle-bench, which downloads
// the same files.)
struct Artifact { file: &'static str, url: &'static str, sha256: &'static str }
const ARTIFACTS: &[Artifact] = &[
    Artifact { file: "config.json",       url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/config.json",       sha256: "REPLACE_WITH_REAL_SHA256" },
    Artifact { file: "tokenizer.json",    url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json",    sha256: "REPLACE_WITH_REAL_SHA256" },
    Artifact { file: "model.safetensors", url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/model.safetensors", sha256: "REPLACE_WITH_REAL_SHA256" },
];

pub fn ensure_weights(cache_dir: &Path, auto_download: bool) -> Result<WeightPaths> {
    std::fs::create_dir_all(cache_dir)?;
    let mut paths = Vec::new();
    for a in ARTIFACTS {
        let dest = cache_dir.join(a.file);
        if !dest.exists() {
            if !auto_download { bail!("weight {} missing and auto_download=false", a.file); }
            let bytes = reqwest::blocking::get(a.url)?.error_for_status()?.bytes()?;
            std::fs::write(&dest, &bytes)?;
        }
        verify_file(&dest, a.sha256)?;
        paths.push(dest);
    }
    Ok(WeightPaths { config: paths[0].clone(), tokenizer: paths[1].clone(), weights: paths[2].clone() })
}
```

> **Implementation note:** replace `REPLACE_WITH_REAL_SHA256` with the actual sha256 of each downloaded file (`sha256sum` the files the `candle-bench` spike already fetched). The `verify_rejects_mismatch` test does not depend on these; the `#[ignore]` smoke test in Step 3b does. `reqwest` needs the `blocking` feature — add `features = ["blocking"]` to the workspace `reqwest` dep or use the async client with a small runtime; prefer the workspace dep already having `blocking` (verify and add if absent).

Create `crates/zoid-embed/src/lib.rs` (CandleEmbedder — lift the loader + forward from `spikes/embed-rerank-eval/candle-bench/src/main.rs`, which is proven):

```rust
//! candle bge-small embedder. CLS pooling + L2 normalize → 384-d unit vectors.
//! Lifted from the validated spike (spikes/embed-rerank-eval/candle-bench).

pub mod fetch;

use anyhow::{Context, Result};
use candle_core::{Device, Tensor, IndexOp};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use std::path::Path;
use tokenizers::Tokenizer;
use zoid_core::retrieval::Embedder;

pub struct CandleEmbedder { model: BertModel, tokenizer: Tokenizer, device: Device }

impl CandleEmbedder {
    pub fn load(cache_dir: &Path, auto_download: bool) -> Result<Self> {
        let w = fetch::ensure_weights(cache_dir, auto_download)?;
        let cfg: BertConfig = serde_json::from_str(&std::fs::read_to_string(&w.config)?)?;
        let tokenizer = Tokenizer::from_file(&w.tokenizer).map_err(anyhow::Error::msg)?;
        let device = Device::Cpu;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[w.weights], DTYPE, &device)? };
        let model = BertModel::load(vb, &cfg).context("load bert")?;
        Ok(Self { model, tokenizer, device })
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self.tokenizer.encode(text, true).map_err(anyhow::Error::msg)?;
        // Copy to owned Vec<u32> before Tensor::new (matches the validated spike,
        // candle-bench/src/main.rs:63-66).
        let ids_v: Vec<u32> = enc.get_ids().to_vec();
        let types_v: Vec<u32> = enc.get_type_ids().to_vec();
        let mask_v: Vec<u32> = enc.get_attention_mask().to_vec();
        let n = ids_v.len();
        let ids = Tensor::new(ids_v, &self.device)?.reshape((1, n))?;
        let types = Tensor::new(types_v, &self.device)?.reshape((1, n))?;
        let mask = Tensor::new(mask_v, &self.device)?.reshape((1, n))?;
        let hidden = self.model.forward(&ids, &types, Some(&mask))?;
        let cls: Vec<f32> = hidden.i((0, 0))?.to_vec1()?;
        let norm: f32 = cls.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok(if norm > 0.0 { cls.iter().map(|x| x / norm).collect() } else { cls })
    }
}

impl Embedder for CandleEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }
    fn dim(&self) -> usize { 384 }
    fn model_id(&self) -> &str { "bge-small-en-v1.5" }
}
```

- [ ] **Step 3b: Gated real-model smoke test**

Add to `lib.rs` (ignored by default so CI never downloads 130 MB — mirrors the `zoid-mcp` heavy-e2e gating):

```rust
#[cfg(test)]
mod smoke {
    use super::*;
    #[test]
    #[ignore = "downloads ~130MB bge-small; run manually"]
    fn embeds_384_and_paraphrase_beats_unrelated() {
        let dir = tempfile::tempdir().unwrap();
        let e = CandleEmbedder::load(dir.path(), true).unwrap();
        let v = e.embed(&["the cat sat on the mat"]).unwrap();
        assert_eq!(v[0].len(), 384);
        let a = &e.embed(&["the cat sat on the mat"]).unwrap()[0];
        let a2 = &e.embed(&["a cat is sitting on a rug"]).unwrap()[0];
        let b = &e.embed(&["quarterly revenue beat expectations"]).unwrap()[0];
        let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        assert!(cos(a, a2) > cos(a, b));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid-embed` (fast tests only; the smoke test is ignored)
Expected: PASS (`sha256_matches_known_vector`, `verify_rejects_mismatch`).
Then verify the crate compiles: `cargo build -p zoid-embed`.
(Optional manual: `cargo test -p zoid-embed -- --ignored` to run the real-model smoke.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-embed/ Cargo.toml
git commit -m "feat(embed): zoid-embed crate — CandleEmbedder (bge-small) + sha256 weight fetcher"
```

---

### Task 9a: Bin skeleton — feature, config fields, App plumbing (tree stays green)

Compile-safe scaffold: the feature, the two `TurnConfig` fields, and `App`
fields — all defaulting to `None`. No candle, no behavior change. Every crate
compiles for the whole workspace after this task.

**Files:**
- Modify: `crates/zoid/Cargo.toml` (optional `zoid-embed` dep + `local-embed` feature)
- Modify: root `Cargo.toml` (`[profile.release.package.zoid-embed] opt-level = 3`)
- Modify: `dist-workspace.toml` (`features = ["local-embed"]`)
- Modify: `crates/zoid/src/agent.rs` (two `TurnConfig` fields; the literal at `agent.rs:88` in `chat_turn_config_with`; `App` fields; `spawn_turn`)
- Modify: `crates/zoid/src/subagent.rs` (the second `TurnConfig` literal at `subagent.rs:151`)

**Interfaces:**
- Produces: `TurnConfig.embed: Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>` and `TurnConfig.embedder: Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>`; `App.embed_index` + `App.embedder` (both `None` in 9a). recall is dispatched in `agent.rs::run_turn_inner` (agent.rs:434), which reads these via the `config` FIELD — exactly like `config.mcp` (agent.rs:1477). There is NO turn-loop parameter to thread.

- [ ] **Step 1: Add the feature + optional dep + profile + dist**

In `crates/zoid/Cargo.toml`:

```toml
[dependencies]
zoid-embed = { path = "../zoid-embed", optional = true }

[features]
default = []
local-embed = ["dep:zoid-embed"]
```

In root `Cargo.toml` add:

```toml
[profile.release.package.zoid-embed]
opt-level = 3
```

In `dist-workspace.toml`, add to the `[dist]` table: `features = ["local-embed"]`.

- [ ] **Step 2: Add the two `TurnConfig` fields**

In `crates/zoid/src/agent.rs`, add to `struct TurnConfig` (near `pub mcp`, agent.rs:62):

```rust
    /// In-memory embedding index for hybrid recall (None = FTS-only). Present
    /// only when built with `local-embed` and `[embed] enabled = true`.
    pub embed: Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>,
    /// The embedder used to embed the recall query. Paired with `embed`.
    pub embedder: Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>,
```

Set both fields in the **only two** real `TurnConfig` literals (tests go through
`chat_turn_config()` which delegates, so they inherit the defaults):
- `crates/zoid/src/agent.rs:88` (in `chat_turn_config_with`): add `embed: None,` and `embedder: None,`.
- `crates/zoid/src/subagent.rs:151`: add `embed: None,` and `embedder: None,`.

- [ ] **Step 3: Add `App` fields + set them on `TurnConfig` in `spawn_turn`**

Add to the `App` struct: `embed_index: Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>` and `embedder: Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>`, both initialized to `None` at `App` construction in 9a. In `spawn_turn`, when building the turn's `TurnConfig`, set:

```rust
        embed: app.embed_index.clone(),
        embedder: app.embedder.clone(),
```

- [ ] **Step 4: Graceful-degradation test (FTS path unchanged)**

Add a `#[tokio::test]` in `agent.rs` tests that builds a turn config via the
existing `chat_turn_config()` helper (its `embed`/`embedder` are `None`) and
drives the existing recall harness, asserting the FTS recall result is unchanged.
Reuse the body of `recall_tool_readmits_and_returns_content`.

Run: `cargo test -p zoid recall_tool_readmits_and_returns_content`
Expected: PASS (behavior identical to before this feature).

- [ ] **Step 5: Verify the whole tree is green + commit**

Run: `cargo build --workspace` and `cargo build -p zoid --features local-embed`
Expected: both PASS; `cargo tree -p zoid | grep -c candle` → `0` on the default build.

```bash
git add crates/zoid/Cargo.toml Cargo.toml dist-workspace.toml crates/zoid/src/agent.rs crates/zoid/src/subagent.rs
git commit -m "feat(zoid): local-embed feature scaffold (default-off) + TurnConfig/App fields"
```

---

### Task 9b: Runtime wiring — build embedder/index/lane + hybrid recall dispatch

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `resolve_cache_dir`; build `CandleEmbedder` + `EmbeddingIndex`, boot-fill, spawn lane; populate `App.embed_index`/`App.embedder`)
- Modify: `crates/zoid/src/agent.rs` (rewrite the recall dispatch at `agent.rs:984`)

**Interfaces:**
- Consumes: `zoid_embed::CandleEmbedder`, `EmbeddingIndex`, `EmbedLane`, `VectorSource`, `hybrid_recall`, `SessionHandle` embedding methods, the `TurnConfig.embed`/`embedder` fields from 9a.

- [ ] **Step 1: Add an XDG cache-dir resolver (mirror `resolve_config_dir`)**

`dirs_cache_dir()` does NOT exist. Add near `resolve_config_dir` (main.rs:65), mirroring its precedence but for the cache base:

```rust
/// `$XDG_CACHE_HOME/zoid` > `$HOME/.cache/zoid` (mirrors resolve_config_dir).
fn resolve_cache_dir(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CACHE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("zoid")
}
```

- [ ] **Step 2: Build embedder/index/lane in `#[tokio::main]`**

Where `App`/`SessionHandle` are built (mirror the `McpManager` construction), add (guarded so default builds compile). Note the C5 fix: `tokio::runtime::Handle::current()` is captured **before** `std::thread::spawn` — calling it inside the spawned OS thread panics (no runtime registered there).

```rust
#[cfg(feature = "local-embed")]
let (embed_index, embedder): (
    Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>,
    Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>,
) = if config.embed.enabled {
    let cache = resolve_cache_dir(|k| std::env::var(k).ok())
        .join("models").join("bge-small-en-v1.5");
    match zoid_embed::CandleEmbedder::load(&cache, config.embed.auto_download) {
        Ok(e) => {
            let e: std::sync::Arc<dyn zoid_core::retrieval::Embedder> = std::sync::Arc::new(e);
            let idx = std::sync::Arc::new(std::sync::RwLock::new(
                zoid_core::embed_index::EmbeddingIndex::new(e.dim(), config.embed.max_vectors),
            ));
            // boot-fill the ring from disk (newest-first rows appended oldest-first)
            if let Ok(rows) = session.load_recent_embeddings(e.model_id().to_string(), config.embed.max_vectors).await {
                let mut g = idx.write().unwrap();
                for (id, v) in rows.into_iter().rev() { g.append(id, &v); }
            }
            // spawn the maintenance lane on a blocking worker.
            // C5: capture the runtime handle BEFORE spawning the OS thread.
            let rt = tokio::runtime::Handle::current();
            {
                let (sess, idx2, emb2, model) = (session.clone(), idx.clone(), e.clone(), e.model_id().to_string());
                let sid = session_id;
                std::thread::spawn(move || {
                    let lane = zoid_core::embed_lane::EmbedLane::new(emb2, idx2);
                    loop {
                        let todo = match rt.block_on(sess.unembedded_events(model.clone(), sid, 64)) {
                            Ok(t) if !t.is_empty() => t,
                            _ => { std::thread::sleep(std::time::Duration::from_secs(2)); continue; }
                        };
                        for (id, v) in lane.tick(&todo) {
                            let _ = rt.block_on(sess.write_embedding(id, model.clone(), v));
                        }
                    }
                });
            }
            (Some(idx), Some(e))
        }
        Err(err) => { tracing::warn!(%err, "local-embed disabled: model load failed"); (None, None) }
    }
} else { (None, None) };
#[cfg(not(feature = "local-embed"))]
let (embed_index, embedder): (Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>, Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>) = (None, None);
```

Assign `embed_index` → `App.embed_index` and `embedder` → `App.embedder` at construction (replacing the `None`s from 9a Step 3).

- [ ] **Step 3: Hybrid recall dispatch (agent.rs:984, in `run_turn_inner`)**

Rewrite the recall dispatch body. It reads the new fields off `config` (the same
`config` that already carries `mcp` — agent.rs:1477), NOT a turn-loop parameter:

```rust
Some(zoid_tools::ToolKind::Emitting) if tc.name == "recall" => {
    let query = tc.args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let limit = tc.args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

    // FTS candidates (existing path) → ids
    let fts_events = session.recall(query.clone(), session_id, limit).await.unwrap_or_default();
    let fts_ids: Vec<Ulid> = fts_events.iter().map(|e| e.id).collect();

    // Vector candidates — only when BOTH the index and embedder are present.
    let vec_ids: Vec<Ulid> = match (&config.embed, &config.embedder) {
        (Some(index), Some(emb)) => {
            let vs = zoid_core::retrieval::VectorSource::new(emb.clone(), index.clone());
            let q = query.clone();
            tokio::task::spawn_blocking(move || vs.candidates(&q, limit))
                .await.unwrap_or_default().into_iter().map(|c| c.event_id).collect()
        }
        _ => Vec::new(),
    };

    let hits: Vec<Event> = if vec_ids.is_empty() {
        fts_events // pure-FTS fast path, byte-identical to pre-feature behavior
    } else {
        let merged = zoid_core::hybrid::hybrid_recall(&fts_ids, &vec_ids, limit);
        let mut evs = session.events_by_ids(merged.clone(), session_id).await.unwrap_or_default();
        evs.sort_by_key(|e| merged.iter().position(|id| *id == e.id).unwrap_or(usize::MAX));
        evs
    };
    // …unchanged from here: re-admit live-evicted ids, render_recalled, emit ToolResult…
}
```

- [ ] **Step 4: Verify + commit**

Run: `cargo test --workspace` (default — candle-free tests green; FTS recall unchanged)
Run: `cargo build -p zoid --features local-embed` (feature build compiles)
Expected: both PASS.

```bash
git add crates/zoid/src/main.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid): runtime wire local-embed — CandleEmbedder, lane, hybrid recall dispatch"
```

---

## Notes for the implementer

- **Order matters:** Tasks 1–7 are all in `zoid-core` and candle-free; they can be built and tested with the fast loop. Task 8 introduces candle (slow compile). Tasks 9a/9b are the bin changes — 9a is a compile-safe scaffold that keeps the whole tree green; 9b adds the candle runtime + the hybrid dispatch.
- **The recall fast path must stay byte-identical when `embed`/`embedder` are None** — the graceful-degradation guarantee (Global Constraints). Task 9a Step 4 locks it with a test *before* 9b rewrites the dispatch.
- **Recall is dispatched in `agent.rs::run_turn_inner` (agent.rs:434/984)** and reads `config.embed`/`config.embedder` as FIELDS — exactly like `config.mcp` (agent.rs:1477). There is no turn-loop parameter to thread and no dispatch in `main.rs`.
- **`spawn_blocking` for query embedding** keeps the ~30 ms candle forward off the async reactor (spike: PHASE1-RESULTS.md).
- **Only two real `TurnConfig` literals exist** — `agent.rs:88` (`chat_turn_config_with`) and `subagent.rs:151`. Tests build configs via `chat_turn_config()`, which delegates, so they inherit `embed: None`/`embedder: None` automatically.
