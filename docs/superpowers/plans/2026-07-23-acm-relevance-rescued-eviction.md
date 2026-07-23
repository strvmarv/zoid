# ACM Slice-4b — Relevance-Rescued Eviction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the already-live recency eviction a semantic *rescue* term so a turn that is stale-by-recency but still on-goal is protected from the eviction wave.

**Architecture:** A soft-additive, rescue-only relevance bump folded into `plan_evictions`' sort key: `keep_score = turn.index + weight · rank_normalized_relevance`. The goal vector is embedded once per eviction off the synchronous gate (only when `est ≥ high_water`); candidate-turn vectors come from the DB `event_embeddings` cache (batch read), never re-embedded. Relevance is a layered pass *inside* `plan_evictions` (the only place with full candidate visibility for ranking); `RecencyScorer` stays the base — no new scorer impl. Empty `GoalContext` ⇒ byte-identical to today's recency eviction (the degradation path when the embedder is off/absent).

**Tech Stack:** Rust workspace (`zoid-core` pure/candle-free; `zoid-embed` candle bge-small, feature `local-embed`); `rusqlite`; `tokio` (session actor + `spawn_blocking`); `ulid`. Vectors are 384-d L2-normalized `f32` ⇒ cosine == dot product.

**Spec:** `docs/superpowers/specs/2026-07-23-acm-relevance-rescued-eviction-design.md`

## Global Constraints

- **Gate stays embedding-free.** The synchronous pre-flight performs no inference. The single goal embed runs on `tokio::task::spawn_blocking`, and only when eviction actually fires (`est ≥ band.high_water`). Candidate vectors are read from cache, never embedded on the gate.
- **Rescue-only, per-item provable.** The added term is always `≥ 0`, so `keep_score ≥ turn.index` for every turn — relevance can only move a turn *up* the keep order. It may still shift a *drop* onto a newer off-goal turn to hit the same reclaim target; that trade is intended. Do **not** assert "victim set ⊆ recency victim set" — it is false under budgeted eviction.
- **Rank-normalize, never calibrate raw cosine.** bge cosines are non-zero-centered (~0.37 unrelated, ~0.81 related). Normalize relevance *window-relative* (rank among candidates) so no absolute threshold is ever tuned.
- **Graceful degradation, no new failure mode.** Feature off / `embed.enabled=false` / weights missing / embed error / empty goal / no cached vectors ⇒ empty `GoalContext` ⇒ today's recency eviction. No panic, no gate, no error surfaced.
- **`RESCUE_WEIGHT` is in turn-index units.** "Maximal relevance is worth `weight` turns of newness." Provisional `12.0`; fixed pre-merge by the §7.1 replay eval (Task 7). A **const** this slice (no `[eviction]` config).
- **Cross-crate discipline.** The `GoalContext` field additions and the `plan_evictions` signature change are cross-crate — **every task builds `cargo build --workspace` and `cargo test --workspace`**, not just `-p zoid-core`.
- **No co-author trailer** in commits (repo `CLAUDE.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-core/src/store.rs` | `vectors_by_ids(model_id, ids)` — chunked batch read of cached vectors | Modify |
| `crates/zoid-core/src/session.rs` | `Cmd::VectorsByIds` + `SessionHandle::vectors_by_ids` | Modify |
| `crates/zoid-core/src/eviction.rs` | `goal_text`; `GoalContext` fields; pure relevance helpers (`cosine`, `rank_normalize`, `turn_relevance`); relevance layer + `ctx` param in `plan_evictions`; `DEFAULT_RESCUE_WEIGHT`, `GOAL_WINDOW_MSGS`, `MIN_GOAL_MSG_CHARS` consts | Modify |
| `crates/zoid/src/agent.rs` | build `GoalContext` in `preflight_gate` under pressure; `embeddable_event_ids` helper; pass `ctx` to all 3 `plan_evictions` call sites; tracing | Modify |
| `spikes/eviction-weight-eval/` | `#[ignore]` replay eval that fixes `DEFAULT_RESCUE_WEIGHT` from real logs | Create |

**Task order & dependency:** T1 → T2 (store then actor) are independent of T3/T4; T5 depends on T4 (uses `GoalContext` + layer); T6 depends on T1, T2, T3, T5 (wires everything); T7 depends on T5 (replays `plan_evictions`). Recommended linear order T1…T7.

---

### Task 1: `vectors_by_ids` store method (chunked batch read)

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add method after `load_recent_embeddings`, ~line 327; reuse private `blob_to_f32s` at line 48)
- Test: `crates/zoid-core/src/store.rs` (`#[cfg(test)]` module — colocated, matches existing `embeddings_write_load_and_unembedded`)

**Interfaces:**
- Consumes: existing `event_embeddings(event_id TEXT, model_id TEXT, dim INT, vector BLOB)`; private `blob_to_f32s(&[u8]) -> Vec<f32>`.
- Produces: `EventStore::vectors_by_ids(&self, model_id: &str, ids: &[Ulid]) -> Result<HashMap<Ulid, Vec<f32>>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn vectors_by_ids_reads_model_filtered_subset() {
    let store = EventStore::open_in_memory().unwrap();
    let sid = Ulid::from(1u128);
    // seed two models' vectors for the same ids
    store.write_embedding(Ulid::from(10u128), "bge", &[1.0, 0.0, 0.0]).unwrap();
    store.write_embedding(Ulid::from(11u128), "bge", &[0.0, 1.0, 0.0]).unwrap();
    store.write_embedding(Ulid::from(10u128), "other", &[9.0, 9.0, 9.0]).unwrap();
    let _ = sid;

    let got = store
        .vectors_by_ids("bge", &[Ulid::from(10u128), Ulid::from(11u128), Ulid::from(99u128)])
        .unwrap();

    assert_eq!(got.len(), 2, "only bge rows for existing ids");
    assert_eq!(got.get(&Ulid::from(10u128)).unwrap(), &vec![1.0, 0.0, 0.0]);
    assert_eq!(got.get(&Ulid::from(11u128)).unwrap(), &vec![0.0, 1.0, 0.0]);
    assert!(!got.contains_key(&Ulid::from(99u128)), "missing id absent, not error");
    assert!(!got.values().any(|v| v == &vec![9.0, 9.0, 9.0]), "other-model row excluded");
}

#[test]
fn vectors_by_ids_empty_ids_is_empty_no_query() {
    let store = EventStore::open_in_memory().unwrap();
    assert!(store.vectors_by_ids("bge", &[]).unwrap().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core vectors_by_ids -- --nocapture`
Expected: FAIL — `no method named vectors_by_ids`.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Batch-read cached vectors for `ids` under `model_id`. Chunked to stay under
/// SQLite's bound-variable limit (a large eviction candidate set can exceed 999).
/// Missing ids are simply absent from the map; a corrupt row is skipped, never
/// propagated (same degrade posture as `load_recent_embeddings`).
pub fn vectors_by_ids(
    &self,
    model_id: &str,
    ids: &[Ulid],
) -> Result<std::collections::HashMap<Ulid, Vec<f32>>> {
    use std::collections::HashMap;
    let mut out: HashMap<Ulid, Vec<f32>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    // 500 keeps us well under SQLITE_MAX_VARIABLE_NUMBER (default 999) with the
    // model_id param to spare.
    for chunk in ids.chunks(500) {
        let placeholders = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT event_id, vector FROM event_embeddings
             WHERE model_id = ?1 AND event_id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        // param 1 = model_id; params 2.. = the chunk's ulid strings
        let mut params: Vec<String> = Vec::with_capacity(chunk.len() + 1);
        params.push(model_id.to_string());
        params.extend(chunk.iter().map(|id| id.to_string()));
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |r| {
            let id: String = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            Ok((id, blob))
        })?;
        for row in rows {
            let (id, blob) = row?;
            if let Ok(u) = Ulid::from_string(&id) {
                out.insert(u, blob_to_f32s(&blob));
            }
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core vectors_by_ids`
Expected: PASS (both tests).

- [ ] **Step 5: Build the workspace (cross-crate guard)**

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(zoid-core): vectors_by_ids — chunked batch read of cached embeddings"
```

---

### Task 2: session actor `VectorsByIds` command + handle

**Files:**
- Modify: `crates/zoid-core/src/session.rs` (add `Cmd` variant near `WriteEmbedding` ~line 70; match arm ~line 195; `SessionHandle` method near `write_embedding` ~line 420)
- Test: `crates/zoid-core/src/session.rs` (`#[cfg(test)]` — matches existing handle round-trip tests ~line 650)

**Interfaces:**
- Consumes: `EventStore::vectors_by_ids` (Task 1).
- Produces: `SessionHandle::vectors_by_ids(&self, model_id: String, ids: Vec<Ulid>) -> Result<HashMap<Ulid, Vec<f32>>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn handle_vectors_by_ids_round_trips() {
    let (h, _dir) = test_handle().await; // existing helper used by embedding tests
    h.write_embedding(Ulid::from(10u128), "bge".into(), vec![1.0, 0.0]).await.unwrap();
    let got = h
        .vectors_by_ids("bge".into(), vec![Ulid::from(10u128), Ulid::from(20u128)])
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got.get(&Ulid::from(10u128)).unwrap(), &vec![1.0, 0.0]);
}
```

> If the existing embedding tests use a differently-named constructor than `test_handle()`, reuse that exact one (see the test that calls `h.write_embedding(...)` around session.rs:650).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core handle_vectors_by_ids`
Expected: FAIL — `no method named vectors_by_ids` on the handle.

- [ ] **Step 3: Add the `Cmd` variant**

In the `enum Cmd { … }`, beside `WriteEmbedding`:

```rust
VectorsByIds {
    model_id: String,
    ids: Vec<Ulid>,
    reply: oneshot::Sender<Result<std::collections::HashMap<Ulid, Vec<f32>>>>,
},
```

- [ ] **Step 4: Add the match arm**

In the actor loop, beside the `Cmd::WriteEmbedding` arm:

```rust
Cmd::VectorsByIds { model_id, ids, reply } => {
    let _ = reply.send(store.vectors_by_ids(&model_id, &ids));
}
```

- [ ] **Step 5: Add the `SessionHandle` method**

Beside `write_embedding`:

```rust
/// Batch-read cached vectors for `ids` under `model_id` (relevance rescue).
pub async fn vectors_by_ids(
    &self,
    model_id: String,
    ids: Vec<Ulid>,
) -> Result<std::collections::HashMap<Ulid, Vec<f32>>> {
    let (reply, rx) = oneshot::channel();
    self.tx
        .send(Cmd::VectorsByIds { model_id, ids, reply })
        .await
        .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
    rx.await
        .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p zoid-core handle_vectors_by_ids`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/session.rs
git commit -m "feat(zoid-core): session VectorsByIds command + handle"
```

---

### Task 3: `goal_text` — the relevance query

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (add near the top-level consts / helpers)
- Test: `crates/zoid-core/src/eviction.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `Event`, `EventKind::UserMessage { text }` (already imported in the file's tests).
- Produces:
  - `pub const GOAL_WINDOW_MSGS: usize = 3;`
  - `pub const MIN_GOAL_MSG_CHARS: usize = 8;`
  - `pub fn goal_text(events: &[&Event], n: usize) -> String;`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn goal_text_takes_recent_nontrivial_user_msgs_newest_first() {
    let evs = vec![
        user(1, "implement the relevance rescue scorer"),
        asst(2, "ok"),
        user(3, "yes"),               // trivial: dropped
        user(4, "wire it into preflight_gate under pressure"),
    ];
    let refs: Vec<&Event> = evs.iter().collect();
    let g = goal_text(&refs, GOAL_WINDOW_MSGS);
    // newest-first, trivial "yes" filtered, only user messages
    let pos_wire = g.find("wire it into").unwrap();
    let pos_impl = g.find("implement the relevance").unwrap();
    assert!(pos_wire < pos_impl, "newest-first");
    assert!(!g.contains("yes"), "trivial confirmation filtered");
    assert!(!g.contains("ok"), "assistant text excluded");
}

#[test]
fn goal_text_empty_when_no_nontrivial_user_msgs() {
    let evs = vec![user(1, "y"), user(2, "3")];
    let refs: Vec<&Event> = evs.iter().collect();
    assert!(goal_text(&refs, GOAL_WINDOW_MSGS).is_empty());
}
```

> Reuse/extend the existing `user(id, text)` test constructor in this file (see `plan_tests`). Add a sibling `asst(id, text)` returning `EventKind::AssistantMessage { text }` if not present.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core goal_text`
Expected: FAIL — `cannot find function goal_text`.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Newest-first concatenation of up to `n` non-trivial user messages, the
/// relevance query. "Non-trivial" filters empties and short confirmations
/// ("yes", "3", "ok") so terse turns don't poison the goal.
pub const GOAL_WINDOW_MSGS: usize = 3;
pub const MIN_GOAL_MSG_CHARS: usize = 8;

pub fn goal_text(events: &[&Event], n: usize) -> String {
    let mut picked: Vec<&str> = Vec::with_capacity(n);
    for e in events.iter().rev() {
        if let EventKind::UserMessage { text } = &e.kind {
            let t = text.trim();
            if t.chars().count() >= MIN_GOAL_MSG_CHARS {
                picked.push(t);
                if picked.len() == n {
                    break;
                }
            }
        }
    }
    picked.join("\n")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core goal_text`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(zoid-core): goal_text — recent non-trivial user turns as relevance query"
```

---

### Task 4: `GoalContext` fields + pure relevance helpers

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (replace the empty `GoalContext` at ~line 191; add helpers + `DEFAULT_RESCUE_WEIGHT`)
- Test: `crates/zoid-core/src/eviction.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `Ulid`, `TurnView { ids, index, token_estimate, topic_hint, protected }`.
- Produces:
  - `pub const DEFAULT_RESCUE_WEIGHT: f32 = 12.0;`
  - `pub struct GoalContext { pub goal: Vec<f32>, pub vecs: HashMap<Ulid, Vec<f32>>, pub weight: f32 }` (derives `Default`).
  - `fn cosine(a: &[f32], b: &[f32]) -> f32` (dot product; guards length mismatch → 0.0).
  - `fn turn_relevance(turn: &TurnView, ctx: &GoalContext) -> f32` (max cosine over the turn's cached event vectors; 0.0 if none).
  - `fn rank_normalize(raws: &[f32]) -> Vec<f32>` (rank/(n−1) ∈ [0,1]; all-equal or len≤1 → all 0.0).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn cosine_is_dot_for_unit_vectors_and_guards_mismatch() {
    assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0, "length mismatch → 0");
}

#[test]
fn turn_relevance_is_max_over_cached_event_vecs() {
    let mut vecs = std::collections::HashMap::new();
    vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]);   // cos 1.0 vs goal
    vecs.insert(Ulid::from(2u128), vec![0.0, 1.0]);   // cos 0.0 vs goal
    let ctx = GoalContext { goal: vec![1.0, 0.0], vecs, weight: DEFAULT_RESCUE_WEIGHT };
    let turn = TurnView { ids: vec![Ulid::from(1u128), Ulid::from(2u128)], index: 0,
        token_estimate: 0, topic_hint: String::new(), protected: false };
    assert!((turn_relevance(&turn, &ctx) - 1.0).abs() < 1e-6, "max, not mean");

    let none = TurnView { ids: vec![Ulid::from(9u128)], ..turn.clone() };
    assert_eq!(turn_relevance(&none, &ctx), 0.0, "no cached vec → 0");
}

#[test]
fn rank_normalize_maps_to_unit_interval_and_degenerates_to_zero() {
    let n = rank_normalize(&[0.37, 0.81, 0.55]);
    assert_eq!(n[1], 1.0, "highest raw → 1.0");
    assert_eq!(n[0], 0.0, "lowest raw → 0.0");
    assert!((n[2] - 0.5).abs() < 1e-6, "middle → 0.5");
    // degenerate: all-equal (incl. all-zero) → no spurious rescue
    assert_eq!(rank_normalize(&[0.5, 0.5, 0.5]), vec![0.0, 0.0, 0.0]);
    assert_eq!(rank_normalize(&[0.9]), vec![0.0]);
    assert_eq!(rank_normalize(&[]), Vec::<f32>::new());
    // TIE GUARD (B1): the common case — one on-goal turn, the rest raw==0. All the
    // zeros MUST map to 0.0, not be spread by array position. A position-based
    // rank returns [1.0, 0.0, 0.25, 0.5] here and hands off-goal turns a bump.
    assert_eq!(rank_normalize(&[0.9, 0.0, 0.0, 0.0]), vec![1.0, 0.0, 0.0, 0.0]);
    assert_eq!(rank_normalize(&[0.0, 0.5, 0.0, 0.5]), vec![0.0, 1.0, 0.0, 1.0]);
}
```

> `TurnView` needs `#[derive(Clone)]` for the `..turn.clone()` spread — it already derives `Clone` (eviction.rs:196). If not, add it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core -- cosine_is_dot turn_relevance rank_normalize`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation**

Replace the empty `GoalContext` (eviction.rs:191-193) and add helpers:

```rust
/// Rescue weight in "turns of recency" units (provisional; fixed by the replay
/// eval). Maximal relevance is worth ~this many turns of newness.
pub const DEFAULT_RESCUE_WEIGHT: f32 = 12.0;

/// Relevance context for a rescue-aware eviction pass. Empty `goal` ⇒ no rescue
/// ⇒ byte-identical to pure recency (the degradation path).
#[derive(Debug, Default, Clone)]
pub struct GoalContext {
    /// Goal (query) unit vector; empty ⇒ relevance term disabled.
    pub goal: Vec<f32>,
    /// event_id → cached unit vector, for candidate-turn events.
    pub vecs: std::collections::HashMap<Ulid, Vec<f32>>,
    /// Rescue weight in turn-index units.
    pub weight: f32,
}

/// Cosine == dot product for L2-normalized vectors; 0.0 on length mismatch.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Max cosine(goal, cached vector) over the turn's events; 0.0 if none cached.
fn turn_relevance(turn: &TurnView, ctx: &GoalContext) -> f32 {
    turn.ids
        .iter()
        .filter_map(|id| ctx.vecs.get(id))
        .map(|v| cosine(&ctx.goal, v))
        .fold(0.0f32, f32::max)
}

/// Map raws to [0,1] by DISTINCT-VALUE rank: ties share a rank, and the lowest
/// distinct value pins to 0.0. All-equal (incl. all-zero) or len ≤ 1 ⇒ all 0.0.
/// CRITICAL: this must be value-based, not array-position-based. In production the
/// candidate set is mostly `raw == 0.0` (off-goal / no cached vector); those MUST
/// all map to 0.0 (zero bump). A position-based rank would spread equal zeros
/// across [0,1] and hand off-goal turns a spurious rescue — silently corrupting
/// the rescue-only guarantee.
fn rank_normalize(raws: &[f32]) -> Vec<f32> {
    let n = raws.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut distinct: Vec<f32> = raws.to_vec();
    distinct.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    distinct.dedup_by(|a, b| (*a - *b).abs() < f32::EPSILON);
    let d = distinct.len();
    if d <= 1 {
        return vec![0.0; n]; // all-equal ⇒ no rescue
    }
    raws.iter()
        .map(|r| {
            let rank = distinct
                .iter()
                .position(|v| (v - r).abs() < f32::EPSILON)
                .unwrap_or(0);
            rank as f32 / (d as f32 - 1.0)
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core -- cosine_is_dot turn_relevance rank_normalize`
Expected: PASS.

- [ ] **Step 5: Build the workspace (GoalContext is public, cross-crate)**

Run: `cargo build --workspace`
Expected: success (no consumer sets `GoalContext` fields yet; `Default` still works).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(zoid-core): GoalContext fields + cosine/turn_relevance/rank_normalize helpers"
```

---

### Task 5: relevance layer + `ctx` param in `plan_evictions` (+ property tests)

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (`plan_evictions` at ~line 340; it currently builds `GoalContext::default()` internally at ~line 356)
- Modify: `crates/zoid/src/agent.rs` — update the **3** `plan_evictions` call sites (833, 2679, 2695) to pass `&GoalContext::default()` for now (real ctx wired in Task 6)
- Test: `crates/zoid-core/src/eviction.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `GoalContext`, `turn_relevance`, `rank_normalize` (Task 4); `RecencyScorer`, `TurnView`, `group_turns`.
- Produces: new signature
  `pub fn plan_evictions<'a>(events, policy, current_tokens, scorer: &dyn EvictionScorer, ctx: &GoalContext) -> EvictionPlan`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod plan_tests` (reuses its `user`, `asst`, `policy`
fixtures). All use this 8-turn builder — 6 candidates after `recent_n = 2`, each
~1000 tokens; user ids `1,3,5,7,9,11,13,15`:

```rust
fn turns8() -> Vec<Event> {
    let big = "x".repeat(3000); // ~1000 tokens (chars/3)
    let mut events = Vec::new();
    for i in 0..8u128 {
        events.push(user(i * 2 + 1, &big));
        events.push(asst(i * 2 + 2, "ok"));
    }
    events
}
// policy(5_000, 2): high_water=5_000, low_water=4_000; current 8_000 ⇒ reclaim 4 turns.
fn evicted_ids(p: &EvictionPlan) -> Vec<Ulid> {
    p.turns.iter().flat_map(|t| t.ids.clone()).collect()
}

#[test]
fn empty_goalcontext_is_byte_identical_to_recency() {
    let events = turns8();
    let a = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default());
    let b = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default());
    assert_eq!(a, b, "deterministic");
    assert!(evicted_ids(&a).contains(&Ulid::from(1u128)), "oldest evicted (recency)");
    assert!(!evicted_ids(&a).contains(&Ulid::from(15u128)), "newest protected");
}

#[test]
fn relevant_old_turn_survives_while_newer_offgoal_is_evicted() {
    let events = turns8();
    // default: oldest (user id 1) is evicted
    let base = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default());
    assert!(evicted_ids(&base).contains(&Ulid::from(1u128)));

    // rescue user id 1: goal matches only its vector; all others orthogonal
    let mut vecs = std::collections::HashMap::new();
    vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]);
    for id in [3u128, 5, 7, 9, 11] { vecs.insert(Ulid::from(id), vec![0.0, 1.0]); }
    let ctx = GoalContext { goal: vec![1.0, 0.0], vecs, weight: DEFAULT_RESCUE_WEIGHT };
    let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
    assert!(!evicted_ids(&rescued).contains(&Ulid::from(1u128)), "on-goal old turn rescued");
    assert!(evicted_ids(&rescued).contains(&Ulid::from(3u128)), "a newer off-goal turn dropped instead");
}

#[test]
fn band_preservation_rescue_never_shrinks_quota() {
    // GENUINELY distinct relevances (distinct unit-vector cosines) so bumps spread
    // 0..weight and the rescue path is actually exercised — NOT all-equal (which
    // the degenerate guard would zero out, making the test vacuous).
    let events = turns8();
    let base = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default());
    let mut vecs = std::collections::HashMap::new();
    //                      cos vs [1,0]:  1.0   0.8        0.6        0.0       0.6      0.8
    let angled = [(1u128, vec![1.0, 0.0]), (3, vec![0.8, 0.6]), (5, vec![0.6, 0.8]),
                  (7, vec![0.0, 1.0]), (9, vec![0.6, 0.8]), (11, vec![0.8, 0.6])];
    for (id, v) in angled { vecs.insert(Ulid::from(id), v); }
    let ctx = GoalContext { goal: vec![1.0, 0.0], vecs, weight: DEFAULT_RESCUE_WEIGHT };
    let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
    // no-starve: rescue reorders WHICH turns go, never how MANY (same reclaim quota).
    assert_eq!(rescued.turns.len(), base.turns.len());
    assert!(!rescued.turns.is_empty(), "wave still fired");
}

#[test]
fn bounded_reach_weight_zero_is_pure_recency() {
    // A maximally-relevant OLD turn is STILL evicted at weight 0 — proving the
    // bump = weight·norm is finite and scales with weight (reach 0 at weight 0).
    let events = turns8();
    let mut vecs = std::collections::HashMap::new();
    vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]); // maximally relevant, but weight 0
    let ctx = GoalContext { goal: vec![1.0, 0.0], vecs, weight: 0.0 };
    let plan = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
    let base = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default());
    assert_eq!(plan, base, "weight 0 ⇒ reach 0 ⇒ pure recency");
    assert!(evicted_ids(&plan).contains(&Ulid::from(1u128)), "no rescue at weight 0");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core -- empty_goalcontext relevant_old_turn band_preservation bounded_reach`
Expected: FAIL — `plan_evictions` takes 4 args, not 5. (Confirm the filter matches 4 tests, not 0 — a zero-match filter exits green and would mask a missing test.)

- [ ] **Step 3: Add the `ctx` param + relevance layer**

Change the signature and the candidate-sort block. Replace the internal
`let ctx = GoalContext::default();` (line ~356) — `ctx` now comes from the caller:

```rust
pub fn plan_evictions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
    ctx: &GoalContext,
) -> EvictionPlan {
    if !policy.enabled { return EvictionPlan::default(); }
    let band = policy.band();
    if current_tokens < band.high_water { return EvictionPlan::default(); }
    let events: Vec<&Event> = events.into_iter().collect();
    let evicted = evicted_ids(events.iter().copied());
    let turns = group_turns(&events, &evicted, policy.recent_n);

    let mut candidates: Vec<&TurnView> = turns
        .iter()
        .filter(|t| !t.protected && !t.ids.is_empty())
        .collect();

    // Relevance layer: rank-normalized max-cosine, folded soft-additive into the
    // recency sort key. Empty goal ⇒ bump 0 ⇒ identical to pure recency.
    let bump: Vec<f32> = if ctx.goal.is_empty() {
        vec![0.0; candidates.len()]
    } else {
        let raws: Vec<f32> = candidates.iter().map(|t| turn_relevance(t, ctx)).collect();
        let norm = rank_normalize(&raws);
        norm.iter().map(|n| ctx.weight * n).collect()
    };

    let key = |i: usize, t: &TurnView| scorer.score(t, ctx) + bump[i];
    let mut idx: Vec<usize> = (0..candidates.len()).collect();
    idx.sort_by(|&a, &b| {
        key(a, candidates[a])
            .partial_cmp(&key(b, candidates[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut reclaimed = 0u64;
    let mut plan = EvictionPlan::default();
    for &i in &idx {
        if current_tokens.saturating_sub(reclaimed) <= band.low_water { break; }
        let t = candidates[i];
        reclaimed += t.token_estimate;
        plan.turns.push(EvictedTurn {
            ids: t.ids.clone(),
            token_estimate: t.token_estimate,
            topic_hint: t.topic_hint.clone(),
        });
    }
    plan
}
```

> `EvictionScorer::score(turn, ctx)` still receives `ctx`; `RecencyScorer` ignores it (returns `turn.index`). The relevance term is applied here, not in the scorer, because ranking needs all candidates at once.

- [ ] **Step 4: Update EVERY existing `plan_evictions` call site (compile-break sweep)**

The new 5th param breaks all existing callers — production **and** tests — so the
crate won't compile until every one is updated in this same commit. Sweep them:

**The grep is authoritative — update every line it prints, do not trust a hand list:**

Run: `grep -rn "plan_evictions(" crates/ | grep -v "fn plan_evictions"`

At time of writing this returns **10** call sites (3 production + 7 tests). Append
`, &GoalContext::default()` (or `&zoid_core::eviction::GoalContext::default()` in
`zoid`) as the final argument to each:
- `crates/zoid/src/agent.rs` — 3 sites (`:833` context-length retry, `:2679` band
  pass, `:2695` hard-floor pass). All three use `default()` in this task; Task 6
  replaces the two `preflight_gate` ones (`:2679`, `:2695`) with the real
  `goal_ctx`. `:833` stays `default()` permanently.
- `crates/zoid-core/src/eviction.rs` — **7** test sites across TWO modules:
  `mod plan_tests` (`no_plan_below_high_water`, `evicts_oldest_first_down_to_low_water`,
  `idempotent_skips_already_evicted`, `never_evicts_protected_even_if_over`,
  `readmitted_turn_is_protected_from_re_eviction`,
  `readmitted_turn_evictable_after_cooldown_lapses`) **and** `mod steady_state_tests`
  (`holds_band_over_hundreds_of_turns`). Miss the second module and `-p zoid-core`
  won't compile — hence: trust the grep, not this list.

- [ ] **Step 5: Run tests + workspace build**

Run: `cargo test -p zoid-core -- empty_goalcontext relevant_old_turn band_preservation bounded_reach`
Expected: PASS (all 4).
Run: `cargo build --workspace && cargo test --workspace --no-fail-fast`
Expected: success; no existing eviction test regressed (empty-ctx path is identical).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/eviction.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid-core): rescue-only relevance layer in plan_evictions (ctx param)"
```

---

### Task 6: wire `GoalContext` into `preflight_gate` under pressure

**Files:**
- Modify: `crates/zoid/src/agent.rs` — `preflight_gate` (~line 2604); build `ctx` once above passes (2) and (3); add `embeddable_event_ids` helper; tracing
- Test: `crates/zoid/src/agent.rs` (`#[cfg(test)]` integration, mirroring the existing recall/eviction gate tests)

**Interfaces:**
- Consumes: `SessionHandle::vectors_by_ids` (T2), `goal_text`/`GoalContext`/`DEFAULT_RESCUE_WEIGHT`/`GOAL_WINDOW_MSGS` (T3/T4), `plan_evictions` 5-arg (T5), `TurnConfig { embed, embedder }` (agent.rs:157-159).
- Produces: populated `GoalContext` passed to the two `preflight_gate` `plan_evictions` calls; `agent.rs:833` stays `GoalContext::default()`.

- [ ] **Step 1: Write the failing integration test**

Mirror the existing `preflight_gate_evicts_before_send` harness (`agent.rs:3054`).
Key technique: **fatness lives on the assistant messages** (drives the token
estimate) while **user messages carry the discriminating tokens** (`goal_text`
reads only `UserMessage`, and `FakeEmbedder` hashes whitespace tokens). Seed
`event_embeddings` manually via `session.write_embedding` — the async lane is not
running in the test, so nothing is cached unless we write it.

```rust
#[tokio::test]
async fn preflight_rescues_relevant_old_turn_over_newer_offgoal() {
    use ulid::Ulid;
    use zoid_core::event::{Event, EventKind};
    use zoid_core::retrieval::{Embedder, FakeEmbedder};

    let fat = "x".repeat(3000); // ONE token; ~1000 est tokens, on the assistant side
    // user ids 1,3,5,7,9,11,13,15. recent_n=2 → 13,15 protected; goal (3 recent user
    // msgs) = ids 15,13,11. On-goal set {1,11,13,15}; off-goal {3,5,7,9}.
    let goalish = "alpha beta gamma delta";
    let offgoal = "zulu yankee xray whiskey";
    let utext = |uid: u128| -> String {
        if matches!(uid, 1 | 11 | 13 | 15) { format!("{goalish} n{uid}") } else { format!("{offgoal} n{uid}") }
    };
    let mut seed = Vec::new();
    for i in 0..8u128 {
        let uid = i * 2 + 1;
        seed.push(Event::new(Ulid::from(uid), None, uid as i64,
            EventKind::UserMessage { text: utext(uid) }));
        seed.push(Event::new(Ulid::from(i * 2 + 2), None, (i * 2 + 2) as i64,
            EventKind::AssistantMessage { text: fat.clone() }));
    }
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    for e in &seed { session.append(e.clone()).await.unwrap(); }

    // Seed cached vectors for the candidate user events (model "fake").
    let emb = FakeEmbedder::new(16);
    for uid in [1u128, 3, 5, 7, 9, 11] {
        let v = emb.embed(&[utext(uid).as_str()]).unwrap().remove(0);
        session.write_embedding(Ulid::from(uid), "fake".into(), v).await.unwrap();
    }

    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: true, capacity: 1_000_000, context_target: 5_000,
        band_headroom_pct: 20, recent_n: 2, max_output: None,
    };
    cfg.embedder = Some(std::sync::Arc::new(FakeEmbedder::new(16)));

    let out = run_gate_only(cfg, session, seed).await; // helper: drives run_agent_turn, returns events
    let evicted: Vec<Ulid> = out.iter().filter_map(|e| match &e.kind {
        EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()), _ => None,
    }).flatten().collect();

    assert!(!evicted.is_empty(), "a wave fired");
    assert!(!evicted.contains(&Ulid::from(1u128)), "on-goal old turn rescued");
    assert!(evicted.contains(&Ulid::from(3u128)), "a newer off-goal turn dropped instead");
}

#[tokio::test]
async fn preflight_without_embedder_evicts_the_old_turn() {
    use ulid::Ulid;
    use zoid_core::event::{Event, EventKind};
    // Same seed shape, but cfg.embedder = None → recency-only → oldest (id 1) evicted.
    let fat = "x".repeat(3000);
    let mut seed = Vec::new();
    for i in 0..8u128 {
        let uid = i * 2 + 1;
        seed.push(Event::new(Ulid::from(uid), None, uid as i64,
            EventKind::UserMessage { text: format!("msg n{uid}") }));
        seed.push(Event::new(Ulid::from(i * 2 + 2), None, (i * 2 + 2) as i64,
            EventKind::AssistantMessage { text: fat.clone() }));
    }
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    for e in &seed { session.append(e.clone()).await.unwrap(); }
    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: true, capacity: 1_000_000, context_target: 5_000,
        band_headroom_pct: 20, recent_n: 2, max_output: None,
    };
    cfg.embedder = None;
    let out = run_gate_only(cfg, session, seed).await;
    let evicted: Vec<Ulid> = out.iter().filter_map(|e| match &e.kind {
        EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()), _ => None,
    }).flatten().collect();
    assert!(evicted.contains(&Ulid::from(1u128)), "no embedder ⇒ recency evicts oldest");
}
```

Add a small `run_gate_only(cfg, session, seed_vec) -> Vec<Event>` test helper that
wraps `run_agent_turn` exactly as `preflight_gate_evicts_before_send` does
(`FakeProvider` yielding `TextDelta("done") + Done`, a drained mpsc channel,
`EventLog::from_vec(seed)`, `|| 0`), returning the `out` events. Reuse it for both
tests to keep them focused on the assertion.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid --features local-embed -- preflight_rescues preflight_without_embedder`
Expected: the rescue test FAILS its `!evicted.contains(id 1)` assertion (real ctx
not built yet — the gate is recency-only, so id 1 is still evicted). The
no-embedder test already passes. (Confirm the filter matches 2 tests, not 0.)

- [ ] **Step 3: Add the `embeddable_event_ids` helper**

```rust
/// Candidate ids for the relevance read. Over-approximates the real candidate set
/// (avoids replicating `group_turns`) BUT excludes already-evicted ids: those
/// turns are `protected`, so `plan_evictions` never looks up their vectors —
/// reading them would be pure waste. The survivors are ~the in-context working
/// set (bounded by the band), not O(history), which keeps this hot-path read
/// bounded on long sessions.
fn embeddable_event_ids(events: &crate::eventlog::EventLog) -> Vec<Ulid> {
    let evicted = zoid_core::eviction::evicted_ids(events.iter()); // HashSet<Ulid>, pub (see agent.rs:1369)
    events.iter().map(|e| e.id).filter(|id| !evicted.contains(id)).collect()
}
```

- [ ] **Step 4: Build `ctx` in `preflight_gate` — AFTER compaction, before passes (2)/(3)**

Placement matters: pass (1) compaction (agent.rs:2637–2675) can drop `est` below
`high_water`, so building the context before it would pay the ~30 ms embed + vector
read for nothing. Insert this **after the compaction pass re-estimates `est`** (the
`if compacted { est = estimate(events); }` at ~2673) and **before** pass (2). It is
gated on the post-compaction `est`, so if compaction alone relieved pressure, no
embed happens:

```rust
    // Relevance rescue context — built only when a wave will fire and the
    // embedder is present. Otherwise default (recency-only, unchanged).
    let goal_ctx: zoid_core::eviction::GoalContext = if est >= band.high_water {
        if let Some(emb) = &config.embedder {
            let text = zoid_core::eviction::goal_text(
                &events.iter().collect::<Vec<_>>(),
                zoid_core::eviction::GOAL_WINDOW_MSGS,
            );
            if text.is_empty() {
                Default::default()
            } else {
                let model = emb.model_id().to_string();
                let goal = {
                    let emb = emb.clone();
                    tokio::task::spawn_blocking(move || {
                        emb.embed(&[text.as_str()]).ok().and_then(|mut v| v.pop()).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default()
                };
                if goal.is_empty() {
                    Default::default()
                } else {
                    let ids = embeddable_event_ids(events);
                    let vecs = session.vectors_by_ids(model, ids).await.unwrap_or_default();
                    zoid_core::eviction::GoalContext {
                        goal,
                        vecs,
                        weight: zoid_core::eviction::DEFAULT_RESCUE_WEIGHT,
                    }
                }
            }
        } else {
            Default::default()
        }
    } else {
        Default::default()
    };
    if !goal_ctx.goal.is_empty() {
        tracing::info!(
            candidates = goal_ctx.vecs.len(),
            weight = zoid_core::eviction::DEFAULT_RESCUE_WEIGHT,
            "eviction relevance rescue active"
        );
    }
```

Then pass `&goal_ctx` to the pass-(2) and pass-(3) `plan_evictions` calls (leave `agent.rs:833`, the context-length emergency retry, on `&GoalContext::default()`).

- [ ] **Step 5: Run tests + release-gate build**

Run: `cargo test -p zoid --features local-embed -- preflight_rescues preflight_without_embedder`
Expected: PASS.
Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success (the release gate from AGENTS.md §4).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): wire relevance rescue into preflight_gate under pressure"
```

---

### Task 7: offline replay eval — fix `DEFAULT_RESCUE_WEIGHT`

**Files:**
- Create: `spikes/eviction-weight-eval/Cargo.toml`, `spikes/eviction-weight-eval/src/main.rs` (a binary, run manually; mirrors `spikes/embed-rerank-eval/*`)
- Modify (after running): `crates/zoid-core/src/eviction.rs` — set `DEFAULT_RESCUE_WEIGHT` to the eval-chosen value

**Interfaces:**
- Consumes: `EventStore` (open a real session DB read-only), `plan_evictions` (T5), `goal_text`, `vectors_by_ids`.
- Produces: a printed table `weight → {regret_rate, band_health, churn}` and a recommended weight.

- [ ] **Step 1: Scaffold the spike crate (not in the workspace release path)**

Add `spikes/eviction-weight-eval` as its own crate (the `spikes/` dir is already excluded from the shipped build — follow `spikes/embed-rerank-eval/Cargo.toml`).

- [ ] **Step 2: Implement the replay + metrics**

```rust
// Pseudocode-level structure (fill with real EventStore reads):
// 1. Open a real session DB (path via argv). Load its full event log.
// 2. Find each point where the live gate WOULD fire (est >= high_water), using
//    the same context_window estimate.
// 3. Ground truth: collect ids later recall'd / TurnsReadmitted after eviction.
// 4. For weight in [0,4,8,12,16,24,32]:
//      - build GoalContext { goal, vecs, weight } from goal_text(at that point)
//        + vectors_by_ids(all ids)   (NOTE: the 5-arg plan_evictions signature)
//      - plan = plan_evictions(events, &policy, est, &RecencyScorer, &ctx)
//      - regret += evicted ∩ later_recalled;  measure realized window vs low_water;
//        churn += symmetric-diff(plan_ids, weight0_plan_ids)
// 5. Print the table; recommend the knee (min regret with band health green).
```

- [ ] **Step 3: Run it against a real dogfood session (manual)**

Run: `cargo run -p eviction-weight-eval -- /path/to/session.sqlite`
Expected: a weight→metrics table; a recommended value.

- [ ] **Step 4: Set the const to the chosen value**

Update `DEFAULT_RESCUE_WEIGHT` in `eviction.rs` if the eval's knee ≠ 12.0; re-run Task 5's property tests to confirm the value stays in the safe range.

Run: `cargo test -p zoid-core -- band_preservation relevant_old_turn`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add spikes/eviction-weight-eval crates/zoid-core/src/eviction.rs
git commit -m "eval(eviction): replay harness fixes DEFAULT_RESCUE_WEIGHT from real logs"
```

---

## Self-Review

Run after all tasks: `cargo test --workspace --features zoid/local-embed --no-fail-fast` (AGENTS.md release gate) and confirm the two degradation regression tests (empty-ctx byte-identical, no-embedder identical) pass. Every task ends with a green workspace build — the `GoalContext`/`plan_evictions` signature changes are cross-crate and must never leave the tree un-compilable between commits.
