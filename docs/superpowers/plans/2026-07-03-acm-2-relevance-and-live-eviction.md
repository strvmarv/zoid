# ACM-2 — Relevance-Scored Heat & Guarded Live Eviction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give zoid a semantic relevance signal for file context and use it to drive guarded, reversible, announced eviction that keeps the live request near a user-preferred target size (default 384k tokens).

**Architecture:** A new pure `relevance` module (an `Embedder` seam with a dependency-free lexical default) scores File items against a rolling goal window; the score folds into `heat_of` (rescue-only — it can only *protect*). A `Protection` axis (`Immutable` guardrails) makes eviction safe. `assemble_context` becomes the live decider (target-band, hysteresis, structural `Immutable` skip); the agent loop journals its decision as `ContextMutation{Evict}` events, and `conversation()` learns to omit evicted items from the live request. Undo reuses the existing `Restore` op.

**Tech Stack:** Rust (workspace: `zoid-core`, `zoid-provider`, `zoid`, `zoid-tui`); event-sourced projections; `proptest` + `insta` for tests.

## Global Constraints

- **Event-sourced, never in-place.** Eviction emits `ContextMutation{Evict}`; original `ToolResult`/`File` events are never removed. Undo = `MutationOp::Restore`.
- **`context_window(events)` stays pure and embedder-free.** Relevance is a separate layered pass (`apply_relevance`). `assemble_context(window, policy)` is the sole "what gets sent" decider.
- **`heat_of` is one pure scoring function**; the relevance term is added to its signature, not bolted on elsewhere.
- **Rescue-only relevance.** Relevance may only *promote* heat; it must never demote or newly make an item eviction-eligible.
- **`Immutable` is a structural skip** in `assemble_context`: never counted, never a candidate. **Guardrails = `System` + `Immutable`.**
- **Default-on safety valve.** `DEFAULT_CONTEXT_TARGET = 384_000`; eviction fires only above `high_water`; a normal session under target is a no-op. Live policy sets `auto_evict_cold = false` (band pass is the only, pressure-gated cold-dropper).
- **Clamp, never error.** `high_water = min(target·(1+band), token_ceiling)`. If target ≥ ceiling, eviction degenerates to ceiling-overflow protection.
- **Static-musl friendly.** Default `Embedder` adds no heavy/native deps; `bge-small` is a future feature gate.
- **Cross-crate discipline.** Every task runs `cargo build --workspace` and `cargo test --workspace` — the `Protection` field on `ContextItem` is a cross-crate public add (the ACM-1 lesson: `-p zoid-core`-scoped tests miss `zoid-tui` literal breaks).
- **No co-author trailer** on commits.

---

## File Structure

| File | Responsibility | Tasks |
| ---- | -------------- | ----- |
| `crates/zoid-provider/src/model.rs` | recognize 1M-context windows | T1 |
| `crates/zoid-core/src/relevance.rs` | Embedder seam, lexical default, goal, scoring | T2, T4 (create) |
| `crates/zoid-core/src/lib.rs` | register `relevance` module | T2 |
| `crates/zoid-core/src/context.rs` | `heat_of` relevance term; `Protection`; `item_key_of` | T3, T5, T7 |
| `crates/zoid-core/src/assembler.rs` | target-band policy + decider | T6 |
| `crates/zoid-core/src/projection.rs` | `conversation()` omits evicted items | T7 |
| `crates/zoid/src/agent.rs` | `record_evictions()` in `run_turn_inner` | T8 |
| `crates/zoid/src/main.rs` | `policy_from_config` target + `auto_evict_cold=false` | T8 |
| `crates/zoid/src/subagent.rs` | subagent eviction posture | T8 |
| `crates/zoid-core/src/config.rs` | `EconomyConfig.context_target` | T8 |
| `crates/zoid-tui/src/tokens.rs`, `chat.rs` | eviction announce (glyph + chip) | T9 |

---

## Task 1: Recognize 1M-context model windows

**Files:**
- Modify: `crates/zoid-provider/src/model.rs:34-39` (the `context_window` if-chain) and its test module.

**Interfaces:**
- Consumes: nothing new.
- Produces: `model_info(model).context_window == 1_000_000` for 1M-context model ids.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/zoid-provider/src/model.rs`:

```rust
#[test]
fn one_million_context_models_report_full_window() {
    // 1M-context variants must report their true window so ACM-2's 384k
    // default target is not silently clamped to the 200k Claude default.
    assert_eq!(model_info("claude-opus-4-8[1m]").context_window, 1_000_000);
    assert_eq!(model_info("claude-opus-4-8-1m").context_window, 1_000_000);
    // Non-1M Claude still reports the conservative 200k.
    assert_eq!(model_info("claude-opus-4-8").context_window, 200_000);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider one_million_context_models_report_full_window`
Expected: FAIL — `claude-opus-4-8[1m]` currently matches `contains("claude")` → 200_000.

- [ ] **Step 3: Implement the minimal change**

In `crates/zoid-provider/src/model.rs`, change the `context_window` if-chain (currently lines 34-39) so the 1M check precedes the generic `claude` check:

```rust
    let context_window = if m.contains("1m") || m.contains("[1m]") {
        1_000_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("glm") {
        256_000
    } else {
        32_000 // conservative default for unknown / small local models
    };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (new test + existing `model_info_windows_are_explicit_per_model`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/model.rs
git commit -m "feat(provider): recognize 1M-context model windows"
```

---

## Task 2: The `Embedder` seam — trait, `Embedding`, `LexicalEmbedder`, `FakeEmbedder`

**Files:**
- Create: `crates/zoid-core/src/relevance.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod relevance;` in alphabetical order, after `pub mod projection;`)

**Interfaces:**
- Produces:
  - `pub trait Embedder { fn embed(&self, text: &str) -> Embedding; }`
  - `pub enum Embedding { Sparse(Vec<(u32, f32)>), Dense(Vec<f32>) }` with `pub fn cosine(&self, other: &Embedding) -> f32`
  - `pub struct LexicalEmbedder;` implementing `Embedder` (produces `Embedding::Sparse`)
  - `pub struct FakeEmbedder { … }` (test double; see below) — behind `#[cfg(test)]`? No: exported for use by `zoid` integration tests too, so keep it public and un-gated.
  - `pub fn tokenize(text: &str) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-core/src/relevance.rs` with a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_snake_and_camel_case() {
        let toks = tokenize("assemble_context ContextWindow readFile");
        // Lowercased, split on non-alphanumeric AND snake/camel boundaries.
        assert!(toks.contains(&"assemble".to_string()));
        assert!(toks.contains(&"context".to_string()));
        assert!(toks.contains(&"window".to_string()));
        assert!(toks.contains(&"read".to_string()));
        assert!(toks.contains(&"file".to_string()));
    }

    #[test]
    fn cosine_is_one_for_identical_and_zero_for_disjoint() {
        let e = LexicalEmbedder;
        let a = e.embed("token ceiling budget governor");
        let b = e.embed("token ceiling budget governor");
        assert!((a.cosine(&b) - 1.0).abs() < 1e-6);
        let c = e.embed("completely different words here");
        assert!(a.cosine(&c).abs() < 1e-6);
    }

    #[test]
    fn cosine_is_bounded_zero_to_one() {
        let e = LexicalEmbedder;
        let a = e.embed("relevance scoring for files");
        let b = e.embed("relevance for the current goal");
        let s = a.cosine(&b);
        assert!((0.0..=1.0).contains(&s), "cosine out of range: {s}");
        assert!(s > 0.0, "shared 'relevance'/'for' should give positive similarity");
    }

    #[test]
    fn empty_text_embeds_to_zero_similarity_without_panic() {
        let e = LexicalEmbedder;
        let empty = e.embed("");
        let some = e.embed("hello world");
        assert_eq!(empty.cosine(&some), 0.0);
        assert_eq!(empty.cosine(&empty), 0.0); // zero-norm → defined as 0
    }

    #[test]
    fn variant_mismatch_is_zero() {
        let sparse = Embedding::Sparse(vec![(1, 1.0)]);
        let dense = Embedding::Dense(vec![1.0]);
        assert_eq!(sparse.cosine(&dense), 0.0);
    }

    #[test]
    fn fake_embedder_is_deterministic_and_controllable() {
        let e = FakeEmbedder::new(&[("auth.rs", 0.9), ("readme", 0.1)]);
        // Same text → same score against the same probe every call.
        assert_eq!(e.embed("auth.rs").cosine(&e.probe()), 0.9);
        assert_eq!(e.embed("auth.rs").cosine(&e.probe()), 0.9);
        assert_eq!(e.embed("readme").cosine(&e.probe()), 0.1);
        assert_eq!(e.embed("unknown").cosine(&e.probe()), 0.0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core relevance::tests`
Expected: FAIL — module/types not defined.

- [ ] **Step 3: Implement the module**

Write the implementation above the test module in `crates/zoid-core/src/relevance.rs`:

```rust
//! Semantic relevance for context items (ACM-2). The `Embedder` seam produces a
//! comparable `Embedding`; the default `LexicalEmbedder` is dependency-free
//! (token-frequency cosine) so it cross-compiles for static-musl. A future
//! `embeddings-bge` feature adds a dense `bge-small` backend behind the same
//! trait. Pure — no I/O in the default backend.

use std::collections::HashMap;

/// Turns text into a comparable vector representation.
pub trait Embedder {
    fn embed(&self, text: &str) -> Embedding;
}

/// One representation both backends can produce. `cosine` compares same-variant
/// embeddings; a variant mismatch (never happens in one run) scores 0.0.
#[derive(Debug, Clone, PartialEq)]
pub enum Embedding {
    /// (hashed-token, weight), sorted by token asc — the lexical default.
    Sparse(Vec<(u32, f32)>),
    /// Dense vector — bge-small later, behind the `embeddings-bge` feature.
    Dense(Vec<f32>),
}

impl Embedding {
    /// Cosine similarity in [0.0, 1.0]. Inputs are L2-normalized at construction,
    /// so this is a dot product clamped to the valid range. Zero-norm → 0.0.
    pub fn cosine(&self, other: &Embedding) -> f32 {
        match (self, other) {
            (Embedding::Sparse(a), Embedding::Sparse(b)) => {
                // Both sorted by token asc → linear merge dot product.
                let (mut i, mut j, mut dot) = (0usize, 0usize, 0.0f32);
                while i < a.len() && j < b.len() {
                    match a[i].0.cmp(&b[j].0) {
                        std::cmp::Ordering::Equal => {
                            dot += a[i].1 * b[j].1;
                            i += 1;
                            j += 1;
                        }
                        std::cmp::Ordering::Less => i += 1,
                        std::cmp::Ordering::Greater => j += 1,
                    }
                }
                dot.clamp(0.0, 1.0)
            }
            (Embedding::Dense(a), Embedding::Dense(b)) if a.len() == b.len() => {
                a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

/// Split text into lowercase tokens on non-alphanumeric boundaries AND
/// snake_case / camelCase boundaries, so code identifiers tokenize into parts.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            // camelCase boundary: lower/digit followed by Upper starts a token.
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            for c in ch.to_lowercase() {
                cur.push(c);
            }
            prev_lower = ch.is_lowercase() || ch.is_numeric();
        } else {
            // snake_case / punctuation / whitespace boundary.
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Deterministic FNV-1a hash of a token into a `u32` bucket.
fn hash_token(tok: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in tok.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Dependency-free lexical relevance: L2-normalized term-frequency sparse vectors.
pub struct LexicalEmbedder;

impl Embedder for LexicalEmbedder {
    fn embed(&self, text: &str) -> Embedding {
        let mut tf: HashMap<u32, f32> = HashMap::new();
        for tok in tokenize(text) {
            *tf.entry(hash_token(&tok)).or_insert(0.0) += 1.0;
        }
        let norm: f32 = tf.values().map(|w| w * w).sum::<f32>().sqrt();
        let mut v: Vec<(u32, f32)> = if norm > 0.0 {
            tf.into_iter().map(|(k, w)| (k, w / norm)).collect()
        } else {
            Vec::new()
        };
        v.sort_unstable_by_key(|(k, _)| *k);
        Embedding::Sparse(v)
    }
}

/// Test double: `embed(text)` returns a sparse vector whose cosine against
/// `probe()` is the score registered for that exact text (0.0 if unregistered).
pub struct FakeEmbedder {
    scores: HashMap<String, f32>,
}

impl FakeEmbedder {
    pub fn new(scores: &[(&str, f32)]) -> Self {
        Self {
            scores: scores.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }
    /// A fixed unit probe (`[(0, 1.0)]`); `embed(text)` encodes the score as the
    /// weight on token bucket 0, so `embed(text).cosine(probe()) == score`.
    pub fn probe(&self) -> Embedding {
        Embedding::Sparse(vec![(0, 1.0)])
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, text: &str) -> Embedding {
        let score = self.scores.get(text).copied().unwrap_or(0.0);
        Embedding::Sparse(vec![(0, score)])
    }
}
```

Add to `crates/zoid-core/src/lib.rs` after `pub mod projection;`:

```rust
pub mod relevance;
```

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid-core relevance::tests` → Expected: PASS
Run: `cargo build --workspace` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/relevance.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): Embedder seam with dependency-free lexical backend"
```

---

## Task 3: `heat_of` gains a rescue-only relevance term

**Files:**
- Modify: `crates/zoid-core/src/context.rs` — `heat_of` (line 295) signature + body; its one call site (line 220); add const `RESCUE_THRESHOLD`.

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn heat_of(refs: u32, last_turn: usize, current_turn: usize, relevance: f32) -> Heat`. Callers with `relevance = 0.0` get today's behavior. Public const `pub const RESCUE_THRESHOLD: f32 = 0.25;`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/zoid-core/src/context.rs` test module:

```rust
#[test]
fn heat_of_relevance_zero_matches_legacy_behavior() {
    // A once-referenced, stale item is Cold when relevance is absent.
    assert_eq!(heat_of(1, 0, 10, 0.0), Heat::Cold);
}

#[test]
fn heat_of_high_relevance_rescues_cold_to_warm() {
    // Same stale item, but highly relevant → promoted to Warm (never evicted).
    assert_eq!(heat_of(1, 0, 10, 0.9), Heat::Warm);
}

#[test]
fn heat_of_low_relevance_never_promotes() {
    // Below the rescue threshold → no change from Cold.
    assert_eq!(heat_of(1, 0, 10, RESCUE_THRESHOLD - 0.01), Heat::Cold);
}

#[test]
fn heat_of_relevance_never_demotes_hot() {
    // Relevance can only promote; a hot item stays hot regardless.
    assert_eq!(heat_of(5, 0, 0, 0.0), Heat::Hot);
    assert_eq!(heat_of(5, 0, 0, 1.0), Heat::Hot);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core heat_of`
Expected: FAIL — `heat_of` takes 3 args, not 4.

- [ ] **Step 3: Implement**

Replace `heat_of` (currently lines 295+) in `crates/zoid-core/src/context.rs`:

```rust
/// Relevance at or above this promotes an otherwise-`Cold` item to `Warm`,
/// rescuing task-central files that recency/refs alone would drop.
pub const RESCUE_THRESHOLD: f32 = 0.25;

fn heat_of(refs: u32, last_turn: usize, current_turn: usize, relevance: f32) -> Heat {
    let recency = current_turn.saturating_sub(last_turn);
    let base = if refs >= HOT_REFS || recency == 0 {
        Heat::Hot
    } else if refs >= WARM_REFS || recency <= COLD_RECENCY_TURNS {
        Heat::Warm
    } else {
        Heat::Cold
    };
    // Rescue-only: relevance may promote Cold→Warm, never demote.
    if base == Heat::Cold && relevance >= RESCUE_THRESHOLD {
        Heat::Warm
    } else {
        base
    }
}
```

Update the call site (currently line 220) to pass `0.0` (relevance is layered in by `apply_relevance`, Task 4, not by the base projection):

```rust
                heat: heat_of(a.refs, a.last_turn, last_turn_global, 0.0),
```

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid-core` → Expected: PASS (new + existing context tests).
Run: `cargo build --workspace` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/context.rs
git commit -m "feat(core): heat_of gains rescue-only relevance term"
```

---

## Task 4: Goal window + relevance scoring + `apply_relevance`

**Files:**
- Modify: `crates/zoid-core/src/relevance.rs` (add functions + tests)
- Uses: `crate::context::{ContextWindow, ContextItem, ItemKind, Heat, RESCUE_THRESHOLD}`, `crate::context::file_contents`, `crate::event::{Event, EventKind}`.

**Interfaces:**
- Consumes: `Embedder` (T2), `RESCUE_THRESHOLD` + `ContextWindow`/`ContextItem` (T3/existing), `file_contents(events)` (existing, returns `HashMap<String,String>` keyed `file:{path}`).
- Produces:
  - `pub const GOAL_WINDOW_MSGS: usize = 3;` `pub const MIN_GOAL_MSG_CHARS: usize = 12;`
  - `pub fn goal_text(events: &[Event], n: usize) -> String`
  - `pub fn relevance_scores(window: &ContextWindow, events: &[Event], e: &dyn Embedder) -> HashMap<String, f32>`
  - `pub fn apply_relevance(window: &ContextWindow, scores: &HashMap<String, f32>) -> ContextWindow`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/zoid-core/src/relevance.rs`:

```rust
    use crate::context::{context_window, ContextItem, ContextWindow, Heat, ItemKind};
    use crate::event::{Event, EventKind};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }

    #[test]
    fn goal_text_keeps_substantive_user_turns_skips_trivial() {
        let events = vec![
            ev(EventKind::UserMessage { text: "add relevance scoring to the assembler".into() }),
            ev(EventKind::UserMessage { text: "yes".into() }),      // trivial
            ev(EventKind::UserMessage { text: "3".into() }),        // trivial
            ev(EventKind::UserMessage { text: "wire eviction into build_request".into() }),
        ];
        let g = goal_text(&events, GOAL_WINDOW_MSGS);
        assert!(g.contains("relevance scoring"));
        assert!(g.contains("wire eviction"));
        assert!(!g.contains("yes"));
    }

    #[test]
    fn relevance_scores_only_file_items() {
        // One File item ("auth"), one ToolResult item ("shell"). Only File scored.
        let window = ContextWindow {
            items: vec![
                ContextItem { key: "file:auth.rs".into(), label: "auth.rs".into(), kind: ItemKind::File, tokens: 100, heat: Heat::Cold, pinned: false, evicted: false, compacted: false, protection: crate::context::Protection::Normal },
                ContextItem { key: "tool:shell:x".into(), label: "shell".into(), kind: ItemKind::ToolResult, tokens: 50, heat: Heat::Cold, pinned: false, evicted: false, compacted: false, protection: crate::context::Protection::Normal },
            ],
            total_tokens: 150,
        };
        let fake = FakeEmbedder::new(&[("auth code here", 0.8)]);
        // file_contents resolves file:auth.rs → its output; emulate via events.
        let events = vec![
            ev(EventKind::ToolCall { id: "1".into(), name: "read_file".into(), args: "{\"path\":\"auth.rs\"}".into() }),
            ev(EventKind::ToolResult { id: "1".into(), name: "read_file".into(), output: "auth code here".into(), is_error: false }),
        ];
        let scores = relevance_scores(&window, &events, &fake);
        assert!(scores.contains_key("file:auth.rs"));
        assert!(!scores.contains_key("tool:shell:x"));
    }

    #[test]
    fn apply_relevance_rescues_cold_relevant_file_to_warm() {
        let window = ContextWindow {
            items: vec![ContextItem { key: "file:auth.rs".into(), label: "auth.rs".into(), kind: ItemKind::File, tokens: 100, heat: Heat::Cold, pinned: false, evicted: false, compacted: false, protection: crate::context::Protection::Normal }],
            total_tokens: 100,
        };
        let mut scores = HashMap::new();
        scores.insert("file:auth.rs".to_string(), 0.9);
        let out = apply_relevance(&window, &scores);
        assert_eq!(out.items[0].heat, Heat::Warm, "relevant cold file must be rescued");
    }

    #[test]
    fn apply_relevance_leaves_unscored_items_untouched() {
        let window = ContextWindow {
            items: vec![ContextItem { key: "file:x.rs".into(), label: "x".into(), kind: ItemKind::File, tokens: 10, heat: Heat::Cold, pinned: false, evicted: false, compacted: false, protection: crate::context::Protection::Normal }],
            total_tokens: 10,
        };
        let out = apply_relevance(&window, &HashMap::new());
        assert_eq!(out.items[0].heat, Heat::Cold);
    }
```

> Note: these tests reference `protection: Protection::Normal`, added in Task 5. If Task 4 is implemented before Task 5, temporarily omit that field; the implementer should sequence T5 before T4's tests compile, OR add the field now. **Recommended order: T3 → T5 → T4 → T6.** (This plan lists T4 before T5 for narrative flow; the executor should implement T5's `Protection` field first if building strictly in number order. Interfaces block above notes the dependency.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core relevance::tests::goal_text_keeps`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement**

Add to `crates/zoid-core/src/relevance.rs` (above the test module):

```rust
use crate::context::{ContextItem, ContextWindow, Heat, ItemKind, RESCUE_THRESHOLD};
use crate::context::file_contents;
use crate::event::{Event, EventKind};

/// How many recent non-trivial user turns form the relevance goal.
pub const GOAL_WINDOW_MSGS: usize = 3;
/// User messages shorter than this are treated as trivial (confirmations like
/// "yes"/"3"/"confirmed") and excluded from the goal — otherwise a terse turn
/// poisons the goal vector.
pub const MIN_GOAL_MSG_CHARS: usize = 12;

/// Concatenate the last `n` non-trivial user messages (most-recent first).
pub fn goal_text(events: &[Event], n: usize) -> String {
    let mut picked: Vec<&str> = Vec::new();
    for e in events.iter().rev() {
        if let EventKind::UserMessage { text } = &e.kind {
            if text.trim().chars().count() >= MIN_GOAL_MSG_CHARS {
                picked.push(text.as_str());
                if picked.len() >= n {
                    break;
                }
            }
        }
    }
    picked.join("\n")
}

/// key → relevance in [0,1] for FILE items only, scored against the goal window.
pub fn relevance_scores(
    window: &ContextWindow,
    events: &[Event],
    e: &dyn Embedder,
) -> HashMap<String, f32> {
    let goal = goal_text(events, GOAL_WINDOW_MSGS);
    let goal_emb = e.embed(&goal);
    let contents = file_contents(events); // file:{path} → latest non-error output
    let mut scores = HashMap::new();
    for it in &window.items {
        if it.kind != ItemKind::File {
            continue;
        }
        if let Some(body) = contents.get(&it.key) {
            let sim = e.embed(body).cosine(&goal_emb);
            scores.insert(it.key.clone(), sim);
        }
    }
    scores
}

/// Layer relevance onto a window: recompute each item's heat with its relevance
/// score (rescue-only). Non-file / unscored items keep their heat. Pure.
pub fn apply_relevance(window: &ContextWindow, scores: &HashMap<String, f32>) -> ContextWindow {
    let items = window
        .items
        .iter()
        .map(|it| {
            let mut it = it.clone();
            if it.heat == Heat::Cold {
                if let Some(&rel) = scores.get(&it.key) {
                    if rel >= RESCUE_THRESHOLD {
                        it.heat = Heat::Warm; // rescue-only promotion
                    }
                }
            }
            it
        })
        .collect();
    ContextWindow {
        items,
        total_tokens: window.total_tokens,
    }
}
```

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid-core relevance` → Expected: PASS
Run: `cargo build --workspace` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/relevance.rs
git commit -m "feat(core): goal window + file relevance scoring + apply_relevance"
```

---

## Task 5: `Protection` axis on `ContextItem`

> **Sequencing:** implement this BEFORE Task 4's tests compile (they set `protection: Protection::Normal`). See T4's note.

**Files:**
- Modify: `crates/zoid-core/src/context.rs` — add `Protection` enum, `protection` field on `ContextItem`, assign at projection time (System → `Immutable`).
- Modify (mechanical, cross-crate): every `ContextItem { … }` struct literal in `zoid-core` (assembler tests) and `zoid-tui` (chat.rs, economy_view.rs, objects.rs, preview.rs, zoom.rs tests/examples) to add `protection: Protection::Normal`.

**Interfaces:**
- Produces: `pub enum Protection { Normal, Protected, Immutable }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `ContextItem.protection: Protection`. System items are `Immutable`; all others default `Normal` (pinned→`Protected` handled in T6 via the pinned flag, not here).

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-core/src/context.rs` tests:

```rust
#[test]
fn system_items_are_immutable_others_normal() {
    let events = vec![
        EventKind::system_prompt_event_or_equivalent(), // see note
    ];
    // Use the real projection: build a window containing a System item and a File.
    // (If there is no System event constructor, assert via a projected window.)
    let w = context_window(&sample_events_with_system());
    let sys = w.items.iter().find(|i| i.kind == ItemKind::System).unwrap();
    assert_eq!(sys.protection, Protection::Immutable);
    let non_sys = w.items.iter().find(|i| i.kind != ItemKind::System);
    if let Some(it) = non_sys {
        assert_eq!(it.protection, Protection::Normal);
    }
}
```

> The implementer must locate how a `System` item enters the window (search `ItemKind::System` in `context.rs`; if System is injected by a helper or a specific event, use that). If `System` is not currently produced by `context_window` from any event in tests, assert the field default instead:

```rust
#[test]
fn context_item_protection_defaults_to_normal() {
    let w = context_window(&[
        Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hello there friend".into() }),
    ]);
    assert!(w.items.iter().all(|i| i.protection == Protection::Normal
        || i.protection == Protection::Immutable));
    // A Message item is Normal.
    assert_eq!(w.items[0].protection, Protection::Normal);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core context_item_protection_defaults_to_normal`
Expected: FAIL — no `protection` field / `Protection` type.

- [ ] **Step 3: Implement**

In `crates/zoid-core/src/context.rs`, add near `ItemKind`:

```rust
/// Criticality of a context item — orthogonal to `ItemKind` (provenance).
/// Guardrails are `System` + `Immutable`; pinned/tool-schemas map to `Protected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    Normal,
    Protected,
    Immutable,
}
```

Add the field to `ContextItem`:

```rust
    /// Criticality axis (orthogonal to `kind`). System guardrails are `Immutable`.
    pub protection: Protection,
```

In the `.map(|k| { … ContextItem { … } })` projection (around line 211-217), set:

```rust
                protection: if a.kind == ItemKind::System {
                    Protection::Immutable
                } else {
                    Protection::Normal
                },
```

- [ ] **Step 4: Fix all cross-crate `ContextItem` literals**

Run `cargo build --workspace` and add `protection: Protection::Normal,` (import `zoid_core::context::Protection` where needed) to every failing `ContextItem { … }` literal — in `zoid-core/src/assembler.rs` test `item(...)` helper and any `zoid-tui` test/example literals. The `assembler.rs` `item()` helper should gain a `protection` field defaulting to `Normal`.

Run: `cargo build --workspace`
Expected: clean after all literals updated.

- [ ] **Step 5: Run tests + commit**

Run: `cargo test --workspace`
Expected: PASS (snapshots byte-identical — `protection` doesn't render yet).

```bash
git add -A
git commit -m "feat(core): add Protection axis to ContextItem (System=Immutable)"
```

---

## Task 6: `assemble_context` — Immutable skip, Protection-aware, target-band eviction

**Files:**
- Modify: `crates/zoid-core/src/assembler.rs` — extend `ContextPolicy`, add `DEFAULT_CONTEXT_TARGET`, rewrite `assemble_context`.

**Interfaces:**
- Consumes: `ContextItem.protection` (T5), `Heat`.
- Produces:
  - `ContextPolicy` gains `pub target_tokens: Option<u64>` and `pub target_band_pct: f32`.
  - `pub const DEFAULT_CONTEXT_TARGET: u64 = 384_000;`
  - `assemble_context` behavior per Global Constraints. `ContextSelection` unchanged (`included`, `excluded`, `tokens`, `compacted`).

- [ ] **Step 1: Write the failing tests**

Add to `crates/zoid-core/src/assembler.rs` tests (extend the `item` helper first to take protection):

```rust
    fn item_p(key: &str, tokens: u64, heat: Heat, protection: Protection) -> ContextItem {
        ContextItem {
            key: key.into(), label: key.into(), kind: ItemKind::File, tokens,
            heat, pinned: false, evicted: false, compacted: false, protection,
        }
    }
    fn band_policy(target: u64) -> ContextPolicy {
        ContextPolicy {
            token_ceiling: Some(2_000_000), auto_evict_cold: false, compact_threshold: None,
            target_tokens: Some(target), target_band_pct: 0.15,
        }
    }

    #[test]
    fn immutable_never_counted_or_evicted() {
        // Immutable item is huge and Cold, but must always be included and must
        // NOT count toward the running total that the band pass measures.
        let w = window(vec![
            item_p("sys", 1_000, Heat::Cold, Protection::Immutable),
            item_p("cold-file", 900, Heat::Cold, Protection::Normal),
        ]);
        let s = assemble_context(&w, &band_policy(100)); // target tiny → pressure
        let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"sys"), "Immutable always included");
    }

    #[test]
    fn band_pass_drops_cold_normal_to_low_water() {
        // total 1000, target 400 (band 0.15 → high 460, low 340). Over high-water:
        // drop Cold Normal (largest first) until <= 340.
        let w = window(vec![
            item_p("cold-big", 500, Heat::Cold, Protection::Normal),
            item_p("cold-small", 200, Heat::Cold, Protection::Normal),
            item_p("hot", 300, Heat::Hot, Protection::Normal),
        ]);
        let s = assemble_context(&w, &band_policy(400));
        let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
        assert!(!keys.contains(&"cold-big"), "largest cold dropped first");
        assert!(keys.contains(&"hot"), "Hot never touched by band pass");
        assert!(s.tokens <= 340, "evicted down to low-water, got {}", s.tokens);
    }

    #[test]
    fn band_pass_never_drops_warm_even_when_over_high_water() {
        // Only Warm+Hot present, all over high-water: nothing to drop → all kept.
        let w = window(vec![
            item_p("warm1", 500, Heat::Warm, Protection::Normal),  // e.g. a rescued file
            item_p("warm2", 500, Heat::Warm, Protection::Normal),
        ]);
        let s = assemble_context(&w, &band_policy(400));
        assert_eq!(s.included.len(), 2, "band pass must not touch Warm/Hot");
    }

    #[test]
    fn no_target_is_no_op() {
        let w = window(vec![item_p("cold", 5000, Heat::Cold, Protection::Normal)]);
        let s = assemble_context(&w, &ContextPolicy::default());
        assert_eq!(s.included.len(), 1, "no target_tokens → no band eviction");
    }

    #[test]
    fn target_above_ceiling_clamps_to_ceiling_only() {
        // target 1M but ceiling 600 → high_water clamps to 600; over 600 drops cold.
        let w = window(vec![item_p("cold", 1000, Heat::Cold, Protection::Normal)]);
        let p = ContextPolicy {
            token_ceiling: Some(600), auto_evict_cold: false, compact_threshold: None,
            target_tokens: Some(1_000_000), target_band_pct: 0.15,
        };
        let s = assemble_context(&w, &p);
        assert!(s.tokens <= 600, "clamped to ceiling, got {}", s.tokens);
    }
```

Add `use crate::context::Protection;` to the test module imports.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core assembler`
Expected: FAIL — `ContextPolicy` has no `target_tokens`/`target_band_pct`.

- [ ] **Step 3: Implement**

Extend `ContextPolicy` and `Default`:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPolicy {
    pub token_ceiling: Option<u64>,
    pub auto_evict_cold: bool,
    pub compact_threshold: Option<u64>,
    /// User-preferred working-set size; None disables target-band eviction.
    pub target_tokens: Option<u64>,
    /// Hysteresis half-width: high_water = target*(1+pct), low_water = target*(1-pct).
    pub target_band_pct: f32,
}

/// Default preferred context size (tokens). A safety valve, not an aggressive
/// trimmer: eviction only fires when the window exceeds the high-water mark.
pub const DEFAULT_CONTEXT_TARGET: u64 = 384_000;

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            token_ceiling: None,
            auto_evict_cold: true,
            compact_threshold: None,
            target_tokens: None,
            target_band_pct: 0.15,
        }
    }
}
```

> Note: `ContextPolicy` derived `Eq` before; `f32` is not `Eq`. Drop `Eq` from the derive (keep `PartialEq`). Fix any `assert_eq!` on whole policies if the compiler flags it (there are none expected; tests compare fields).

Rewrite `assemble_context`:

```rust
pub fn assemble_context(window: &ContextWindow, policy: &ContextPolicy) -> ContextSelection {
    use crate::context::Protection;

    let compacted = policy
        .compact_threshold
        .is_some_and(|t| window.total_tokens > t);
    let drop_cold = policy.auto_evict_cold || compacted;

    let mut included: Vec<ContextItem> = Vec::new();
    let mut excluded: Vec<ContextItem> = Vec::new();

    // Pass 0: Immutable items are structural — always included, never counted.
    let mut managed: Vec<ContextItem> = Vec::new();
    for it in &window.items {
        if it.protection == Protection::Immutable {
            included.push(it.clone());
        } else {
            managed.push(it.clone());
        }
    }

    // Pass 1: pin / manual-evict / (legacy) auto-cold filtering.
    let mut survivors: Vec<ContextItem> = Vec::new();
    for it in managed {
        if it.pinned {
            survivors.push(it);
        } else if it.evicted || (drop_cold && it.heat == Heat::Cold) {
            excluded.push(it);
        } else {
            survivors.push(it);
        }
    }

    // Pass 2: target-band eviction (hysteresis). Drops Cold Normal items only,
    // largest-first, until <= low_water; never touches Warm/Hot (rescued items
    // are Warm), never touches Protected. Pinned survive regardless.
    if let Some(target) = policy.target_tokens {
        let managed_total: u64 = survivors.iter().map(|i| i.tokens).sum();
        let ceiling = policy.token_ceiling.unwrap_or(u64::MAX);
        let high_water =
            ((target as f64 * (1.0 + policy.target_band_pct as f64)) as u64).min(ceiling);
        let low_water = (target as f64 * (1.0 - policy.target_band_pct as f64)) as u64;
        if managed_total > high_water {
            // Candidates: Cold + Normal + not pinned, largest tokens first.
            let mut order: Vec<usize> = (0..survivors.len()).collect();
            order.sort_by(|&a, &b| survivors[b].tokens.cmp(&survivors[a].tokens));
            let mut running = managed_total;
            let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for &idx in &order {
                if running <= low_water {
                    break;
                }
                let it = &survivors[idx];
                if it.heat == Heat::Cold && it.protection == Protection::Normal && !it.pinned {
                    drop.insert(idx);
                    running -= it.tokens;
                }
            }
            let mut kept = Vec::new();
            for (i, it) in survivors.into_iter().enumerate() {
                if drop.contains(&i) {
                    excluded.push(it);
                } else {
                    kept.push(it);
                }
            }
            survivors = kept;
        }
    }

    // Pass 3: hard token ceiling (pinned always kept; non-pinned fit cumulatively).
    let mut running: u64 = 0;
    for it in survivors {
        if it.pinned {
            included.push(it);
            continue;
        }
        match policy.token_ceiling {
            Some(c) if running + it.tokens > c => excluded.push(it),
            _ => {
                running += it.tokens;
                included.push(it);
            }
        }
    }

    let tokens = included
        .iter()
        .filter(|i| i.protection != Protection::Immutable) // Immutable not counted
        .map(|i| i.tokens)
        .sum();
    ContextSelection { included, excluded, tokens, compacted }
}
```

> **Design note for the reviewer:** `tokens` excludes `Immutable` so the reported selection size matches the band-pass accounting (`immutable_never_counted_or_evicted` asserts this). Existing ACM-1 tests use `Protection::Normal` items, so their `tokens` sums are unchanged.

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid-core assembler` → Expected: PASS (new + existing).
Run: `cargo build --workspace && cargo test --workspace` → Expected: clean/PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/assembler.rs
git commit -m "feat(core): target-band eviction + Immutable skip in assemble_context"
```

---

## Task 7: `item_key_of` helper + `conversation()` omits evicted items

**Files:**
- Modify: `crates/zoid-core/src/context.rs` — add `pub fn item_key_of`, use it in `context_window`.
- Modify: `crates/zoid-core/src/projection.rs` — `conversation()` folds `Evict`/`Restore` and omits evicted `ToolResult`s.

**Interfaces:**
- Produces: `pub fn item_key_of(name: &str, id: &str, call_path: &HashMap<String, String>) -> String` — returns `file:{path}` if `call_path` has `id`, else `tool:{name}:{id}`.
- `conversation()` output no longer includes `ChatMsg::ToolResult` for items whose current-evicted key is set.

- [ ] **Step 1: Write the failing tests**

Add to `crates/zoid-core/src/projection.rs` tests (helpers `tres`/`tcall`/`user` exist; note they hardcode `id: ""` — add variants that set a real id/args):

```rust
    fn tcall_id(id: &str, name: &str, args: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
            id: id.into(), name: name.into(), args: args.into(),
        })
    }
    fn tres_id(id: &str, name: &str, output: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::ToolResult {
            id: id.into(), name: name.into(), output: output.into(), is_error: false,
        })
    }
    fn evict(key: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::ContextMutation {
            item: key.into(), op: crate::event::MutationOp::Evict,
        })
    }
    fn restore(key: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::ContextMutation {
            item: key.into(), op: crate::event::MutationOp::Restore,
        })
    }

    #[test]
    fn conversation_omits_evicted_tool_result() {
        let events = vec![
            tcall_id("t1", "shell", "{}"),
            tres_id("t1", "shell", "big output"),
            evict("tool:shell:t1"),
        ];
        let msgs = conversation(&events);
        assert!(!msgs.iter().any(|m| matches!(m, ChatMsg::ToolResult { output, .. } if output == "big output")),
            "evicted tool-result must not reach the live request");
    }

    #[test]
    fn conversation_omits_evicted_file_by_path_key() {
        let events = vec![
            tcall_id("r1", "read_file", "{\"path\":\"auth.rs\"}"),
            tres_id("r1", "read_file", "auth contents"),
            evict("file:auth.rs"),
        ];
        let msgs = conversation(&events);
        assert!(!msgs.iter().any(|m| matches!(m, ChatMsg::ToolResult { output, .. } if output == "auth contents")),
            "evicted file must be omitted by its file:{{path}} key");
    }

    #[test]
    fn conversation_restore_reincludes_item() {
        let events = vec![
            tcall_id("t1", "shell", "{}"),
            tres_id("t1", "shell", "big output"),
            evict("tool:shell:t1"),
            restore("tool:shell:t1"),
        ];
        let msgs = conversation(&events);
        assert!(msgs.iter().any(|m| matches!(m, ChatMsg::ToolResult { output, .. } if output == "big output")),
            "Restore (last write) must re-include the item");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core conversation_omits_evicted_tool_result`
Expected: FAIL — evicted item still present (conversation ignores ContextMutation).

- [ ] **Step 3a: Add `item_key_of` in `context.rs`**

Add near `tool_id_of` in `crates/zoid-core/src/context.rs`:

```rust
/// Compute the context-window item key for a tool-result. Files (calls whose
/// args carried a `tool_path`) coalesce under `file:{path}`; everything else is
/// `tool:{name}:{id}`. Shared by `context_window` and `conversation()` so the
/// keying can never drift between them.
pub fn item_key_of(name: &str, id: &str, call_path: &HashMap<String, String>) -> String {
    match call_path.get(id) {
        Some(path) => format!("file:{path}"),
        None => format!("tool:{name}:{id}"),
    }
}
```

Refactor `context_window`'s `ToolResult` arm (lines ~175-203) to use it:

```rust
            EventKind::ToolResult { id, name, output, .. } => {
                flush_delta(&mut delta_text, &mut order, &mut acc, &mut msg_seq, turn);
                let key = item_key_of(name, id, &call_path);
                let (label, kind) = if let Some(path) = call_path.get(id) {
                    (path.clone(), ItemKind::File)
                } else {
                    (name.clone(), ItemKind::ToolResult)
                };
                upsert(&mut order, &mut acc, key, label, kind, estimate_tokens(output), turn);
            }
```

- [ ] **Step 3b: Teach `conversation()` to omit evicted items**

In `crates/zoid-core/src/projection.rs`, add imports and a pre-scan at the top of `conversation()` (after the `compacted` map):

```rust
    use crate::economy::tool_path;
    use crate::event::MutationOp;

    // Currently-evicted item keys (Evict/Restore, last write wins).
    let mut evicted: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Rebuild the tool-id → path map so tool-results resolve to their window key.
    let mut call_path: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in events {
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            EventKind::ToolCall { id, args, .. } => {
                if let Some(p) = tool_path(args) {
                    call_path.insert(id.clone(), p);
                }
            }
            EventKind::ContextMutation { item, op } => match op {
                MutationOp::Evict => { evicted.insert(item.clone()); }
                MutationOp::Restore => { evicted.remove(item); }
                _ => {}
            },
            _ => {}
        }
    }
```

Then in the `ToolResult` fold arm (currently lines 116-136), skip evicted items after the `flush`:

```rust
            EventKind::ToolResult { id, name, output, is_error } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                let key = crate::context::item_key_of(name, id, &call_path);
                if evicted.contains(&key) {
                    continue; // omitted from the live request (ACM-2 eviction)
                }
                let (output, was_compacted) = match compacted.get(id.as_str()) {
                    Some(sum) => ((*sum).to_string(), true),
                    None => (output.clone(), false),
                };
                out.push(ChatMsg::ToolResult {
                    id: id.clone(), name: name.clone(), output,
                    is_error: *is_error, compacted: was_compacted, ts: e.ts,
                });
            }
```

> Keep the existing `ContextMutation { .. }` arm in the main match as a no-op (it's already folded in the pre-scan). Do not remove it.

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid-core projection` → Expected: PASS (new + existing, incl. `conversation_ignores_usage_and_mutation` which asserts a *lone* mutation with no matching tool-result still produces no item — verify it still holds; if it evicts a non-existent key it's a harmless no-op).
Run: `cargo build --workspace && cargo test --workspace` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/context.rs crates/zoid-core/src/projection.rs
git commit -m "feat(core): conversation() omits evicted items via shared item_key_of"
```

---

## Task 8: Wire-in — `record_evictions`, config target, policy defaults

**Files:**
- Modify: `crates/zoid-core/src/config.rs` — `EconomyConfig.context_target` + `PartialEconomy` + merge + provenance.
- Modify: `crates/zoid/src/main.rs` — `policy_from_config` sets `target_tokens` (default 384k) + `auto_evict_cold = false`.
- Modify: `crates/zoid/src/subagent.rs` — `subagent_policy` target posture.
- Modify: `crates/zoid/src/agent.rs` — `record_evictions()`, called before re-request.

**Interfaces:**
- Consumes: `relevance::{LexicalEmbedder, goal_text, relevance_scores, apply_relevance}`, `context_window`, `assemble_context`, `ContextPolicy` (T2/T4/T6), `Protection` (T5).
- Produces: `record_evictions(...)` emitting `ContextMutation{Evict}` for `Normal` excluded items; `EconomyConfig.context_target: Option<u64>`.

- [ ] **Step 1: Write the failing integration test**

Add to `crates/zoid/src/agent.rs` tests (mirror ACM-1's `build_request_carries_compacted_summary_into_live_messages`):

```rust
#[test]
fn build_request_omits_evicted_items_and_restore_brings_them_back() {
    use zoid_core::event::{Event, EventKind, MutationOp};
    use ulid::Ulid;
    let mk = |k: EventKind| Event::new(Ulid::new(), None, 0, k);
    let mut events = vec![
        mk(EventKind::ToolCall { id: "t1".into(), name: "shell".into(), args: "{}".into() }),
        mk(EventKind::ToolResult { id: "t1".into(), name: "shell".into(), output: "SECRET_DUMP".into(), is_error: false }),
        mk(EventKind::ContextMutation { item: "tool:shell:t1".into(), op: MutationOp::Evict }),
    ];
    // build_request derives messages from conversation(); evicted item must be gone.
    let req = build_request_for_test(&events); // use the crate's existing request builder/helper
    assert!(!format!("{req:?}").contains("SECRET_DUMP"), "evicted item must not be in the live request");
    events.push(mk(EventKind::ContextMutation { item: "tool:shell:t1".into(), op: MutationOp::Restore }));
    let req2 = build_request_for_test(&events);
    assert!(format!("{req2:?}").contains("SECRET_DUMP"), "Restore re-includes the item");
}
```

> The implementer must use the SAME request-construction path ACM-1's test used (search `build_request` / `map_msg` in `agent.rs`; reuse that helper or replicate its call). If ACM-1's test built messages via `conversation(events).into_iter().map(map_msg)`, do the same here.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid build_request_omits_evicted_items`
Expected: FAIL (before wiring — actually conversation() from T7 already omits, so this test may PASS once T7 landed; if so, it guards the wire-in path end-to-end — keep it). If it fails to compile, fix the helper reference.

- [ ] **Step 3a: Config `context_target`**

In `crates/zoid-core/src/config.rs`, add to `EconomyConfig` (after `token_ceiling`):

```rust
    /// User-preferred working-set size in tokens. None → DEFAULT_CONTEXT_TARGET.
    /// A value of Some(0) disables target-band eviction.
    pub context_target: Option<u64>,
```

Add matching `context_target: Option<u64>` to `PartialEconomy`; set `context_target: None` in the `Default` and in the merge base; add a `Source` field `context_target` to `Provenance` and merge it like `token_ceiling`. Follow the exact pattern of `token_ceiling` at every site (search `token_ceiling` in config.rs and replicate).

- [ ] **Step 3b: `policy_from_config` (main.rs)**

In `crates/zoid/src/main.rs`, update `policy_from_config` (lines ~353-366):

```rust
fn policy_from_config(econ: &EconomyConfig, ceiling: u64) -> ContextPolicy {
    let compact_threshold = if econ.compact_threshold_pct == 0 {
        None
    } else {
        Some(ceiling.saturating_mul(econ.compact_threshold_pct as u64) / 100)
    };
    let target_tokens = match econ.context_target {
        Some(0) => None, // explicit 0 disables band eviction
        Some(t) => Some(t),
        None => Some(zoid_core::assembler::DEFAULT_CONTEXT_TARGET),
    };
    ContextPolicy {
        token_ceiling: econ.token_ceiling,
        auto_evict_cold: false, // band pass is the only, pressure-gated cold-dropper
        compact_threshold,
        target_tokens,
        target_band_pct: 0.15,
    }
}
```

> `auto_evict_cold` is now forced `false` here regardless of `econ.auto_evict_cold`. Update the existing `policy_from_config_maps_pct_to_absolute` test's expectation (`assert!(!p.auto_evict_cold)` already true) and add `assert_eq!(p.target_tokens, Some(384_000))` for the default case, `assert_eq!(policy_from_config(&econ_zero_target, 200_000).target_tokens, None)` for the disable case. If `econ.auto_evict_cold` becomes unused, keep the config field (still surfaced in the settings UI) — it no longer feeds the live policy; note this in a comment.

- [ ] **Step 3c: `subagent_policy` (subagent.rs)**

Subagents run bounded, short-lived branches; keep their existing ceiling-based bounding and set no target (rely on `token_ceiling`):

```rust
pub fn subagent_policy() -> ContextPolicy {
    ContextPolicy {
        token_ceiling: Some(SUBAGENT_CONTEXT_CEILING),
        auto_evict_cold: true, // subagents keep aggressive cold-drop within their small ceiling
        compact_threshold: None,
        target_tokens: None,   // no target-band eviction on subagent branches
        target_band_pct: 0.15,
    }
}
```

- [ ] **Step 3d: `record_evictions` (agent.rs)**

Add after `record_compactions` in `crates/zoid/src/agent.rs`:

```rust
/// Record `ContextMutation{Evict}` events for Normal items the policy's
/// target-band pass excludes, given relevance-adjusted heat. Idempotent: skips
/// items already evicted in the log. Mirrors `record_compactions`.
async fn record_evictions(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    use zoid_core::context::{context_window, Protection};
    use zoid_core::relevance::{apply_relevance, relevance_scores, LexicalEmbedder};

    if config.policy.target_tokens.is_none() {
        return Ok(()); // eviction disabled
    }
    let window = context_window(events);
    let embedder = LexicalEmbedder;
    let scores = relevance_scores(&window, events, &embedder);
    let window = apply_relevance(&window, &scores);
    let sel = zoid_core::assembler::assemble_context(&window, &config.policy);

    // Already-evicted keys (idempotence): fold Evict/Restore last-write-wins.
    let mut already: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in events.iter() {
        if let EventKind::ContextMutation { item, op } = &e.kind {
            match op {
                zoid_core::event::MutationOp::Evict => { already.insert(item.clone()); }
                zoid_core::event::MutationOp::Restore => { already.remove(item); }
                _ => {}
            }
        }
    }

    for it in &sel.excluded {
        if it.protection == Protection::Normal && !already.contains(&it.key) {
            emit(
                session, events, ui, &config.branch,
                EventKind::ContextMutation { item: it.key.clone(), op: zoid_core::event::MutationOp::Evict },
                session_id, now,
            ).await?;
        }
    }
    Ok(())
}
```

Call it right after `record_compactions` (line 494):

```rust
        record_compactions(&session, &mut events, ui, config, session_id, now).await?;
        record_evictions(&session, &mut events, ui, config, session_id, now).await?;
        // loop: re-request with the tool results now in context
```

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo test -p zoid` → Expected: PASS (integration test + updated policy test).
Run: `cargo build --workspace && cargo test --workspace` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/config.rs crates/zoid/src/main.rs crates/zoid/src/subagent.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid): wire target-band eviction into the turn loop (default 384k)"
```

---

## Task 9: Announce eviction through semantic zoom

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` — add `EVICT` glyph const.
- Modify: `crates/zoid-tui/src/chat.rs` — render an eviction chip from `ContextMutation{Evict}` events at Normal zoom; detail at Detail zoom.
- Add: insta snapshots.

**Interfaces:**
- Consumes: `EventKind::ContextMutation{op: Evict}`, existing zoom-level rendering.
- Produces: `pub const EVICT: char = '⊘';` and a rendered chip line.

- [ ] **Step 1: Write the failing test/snapshot**

Determine how `chat.rs` renders `ToolResultCompacted` chips (search `COMPACT` in `chat.rs` — ACM-1 added the `⊟` marker). Mirror that for eviction. Add a snapshot test that builds a conversation view containing an `Evict` event and asserts the chip appears:

```rust
#[test]
fn eviction_chip_renders_at_normal_zoom() {
    // Build events with an evicted tool-result; render Normal zoom; snapshot.
    let events = /* user msg + tool call/result + ContextMutation Evict */;
    let lines = build_conversation(&events, Zoom::Normal /* match existing API */);
    insta::assert_snapshot!(render(lines));
}
```

> The implementer must match `chat.rs`'s actual rendering API (function names, `Zoom` enum). If eviction chips require a new `ChatMsg` variant to surface in the transcript, prefer instead reading `ContextMutation` events directly in the chat view-model (they are on the main branch), counting evictions per turn, and emitting one chip line `⊘ evicted N items`. Do NOT add a `ChatMsg` variant unless the existing view genuinely cannot access events — keep the change minimal.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui eviction_chip`
Expected: FAIL — glyph/rendering absent.

- [ ] **Step 3: Implement**

Add to `crates/zoid-tui/src/tokens.rs`:

```rust
/// Marker for an eviction announcement chip (ACM-2).
pub const EVICT: char = '⊘';
```

In `chat.rs`, render a one-line chip at Normal zoom summarizing evictions in a turn (`⊘ evicted N items`), and at Detail zoom list the evicted item labels. Follow the exact pattern the `COMPACT` (`⊟`) marker uses.

- [ ] **Step 4: Review + accept snapshots**

Run: `cargo test -p zoid-tui` → review new snapshots with `cargo insta review` (or inspect `.snap.new`), accept if correct.
Run: `cargo test --workspace` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/chat.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): announce eviction via ⊘ chip at Normal/Detail zoom"
```

---

## Task 10 (optional): Documentation + memory

**Files:**
- Modify: the ACM vision doc `§4` step markers, noting steps 2+3 are now shipped (ACM-2).

- [ ] **Step 1:** Update `docs/superpowers/specs/2026-07-03-active-context-management-vision.md` to mark short-term steps 2 (relevance) and 3 (wire assembler) as shipped in ACM-2, mirroring how ACM-1 marked step 1.
- [ ] **Step 2:** Commit: `git commit -m "docs(acm): mark vision steps 2-3 shipped in ACM-2"`

---

## Self-Review (author's checklist against the spec)

**Spec coverage:**
- §4 relevance unit → T2 (seam/lexical/fake) + T4 (goal/scoring/apply). ✅
- §5.1 heat relevance term (rescue-only) → T3. ✅
- §5.2 Protection axis (System=Immutable) → T5; Immutable structural skip → T6. ✅
- §6 target band + assembler decider (hysteresis, clamp, Cold-only) → T6. ✅
- §7 approach-C wire-in (record_evictions, journal Evict) + conversation omission + item_key_of → T7 + T8. ✅
- §8 1M-window recognition → T1. ✅
- §9 announce (EVICT glyph + chip) → T9. ✅
- §10 tests: relevance, assembler (Immutable/Protected/hysteresis/clamp/rescue-survives), wire-in omit+restore, model window → distributed across T1-T9. ✅
- §11 out-of-scope (bge backend, messages/tool-result relevance, RAG) → not implemented; seam only. ✅
- §12 success criteria → covered by T1/T6/T7/T8 tests + `assemble_context` now called in production (T8). ✅

**Type consistency:** `heat_of(refs, last_turn, current_turn, relevance)` (T3) matches the `apply_relevance` rescue logic (T4, which recomputes heat directly rather than re-calling `heat_of` — consistent, both use `RESCUE_THRESHOLD`). `Protection::{Normal,Protected,Immutable}` used identically in T5/T6/T8. `ContextPolicy` fields `target_tokens`/`target_band_pct` consistent T6→T8. `item_key_of(name, id, call_path)` signature identical in T7 producer and its `context_window`/`conversation` consumers. `DEFAULT_CONTEXT_TARGET = 384_000` referenced consistently.

**Known executor notes (not placeholders — explicit instructions):** T4 depends on T5's `protection` field (build T5 first — flagged in both tasks). T8 and T9 reference existing helpers (`build_request`/`map_msg` path, `chat.rs` render API, config `token_ceiling` pattern) the implementer must locate and mirror rather than guess — each such spot names exactly what to search for and what to replicate.

**`ContextPolicy` derive:** T6 drops `Eq` (f32 field). Any code relying on `ContextPolicy: Eq` must be adjusted — none known; the compiler will flag it. Noted in T6.
