# ACM-2 — Relevance-Scored Heat & Guarded Live Eviction (Design)

> **Status:** Approved design / spec. Terminal step after the spec-review gate is
> `writing-plans`. This is the second buildable slice of the Active Context
> Management vision — it implements vision steps **2** (relevance-score files)
> and **3** (wire the assembler into the live request), which the vision
> deliberately couples: relevance on files only *acts* by driving eviction, and
> eviction is only *safe* once heat carries a relevance term.
>
> **Date:** 2026-07-03 · **Builds on:** ACM-1 (tool-result compaction, merged
> `14deafc`) · **Parent vision:** `2026-07-03-active-context-management-vision.md`

---

## 1. Goal

Give zoid a **semantic relevance signal** for file context and use it to drive
**guarded, reversible, announced eviction** of stale/irrelevant items from the
live request — keeping the working set near a **user-preferred target size**
(default 384k tokens) rather than letting it grow unbounded.

This is the slice that turns `heat_of(refs, recency)` into
`heat_of(refs, recency, relevance)` and makes `assemble_context` — today a fully
tested pure function that *nothing in production calls* — the live decider for
what gets sent.

## 2. Why these two steps together

The vision's anti-decoration test (§1.2): every piece must be *acted on* or
*decided from*. Relevance-on-files fails that test in isolation:

- Relevance matters most for **files** (per-kind table); tool-results are
  compacted on age/size (ACM-1), not semantics.
- Files are never compacted (you edit them, not summarize them).
- Therefore the *only* live action a file-relevance score can drive is
  **eviction / ceiling-drop ordering**.

And eviction is the *risky* operation (it drops content; compaction only
shrinks), which is exactly why the vision said eviction must wait until heat has
a relevance term. The two steps are mutually dependent, so they ship as one
coherent slice.

## Global Constraints

Copied binding requirements — every task's requirements implicitly include this
section.

- **Event-sourced, never in-place.** Every context mutation is a
  `ContextMutation` event (vision §3). Eviction emits `ContextMutation{Evict}`;
  the original `ToolResult`/`File` events are never removed. Undo = the existing
  `MutationOp::Restore`.
- **Projections + pure assembler stay the single chokepoint.**
  `context_window(events)` stays **pure and embedder-free**. Relevance is a
  separate layered pass. `assemble_context(window, policy)` is the sole decider
  of "what gets sent."
- **`heat_of` is one pure scoring function** — the relevance term is added to
  its signature, not bolted on elsewhere.
- **Guardrails are provably safe.** `Immutable` items (`System` guardrails) are a
  **structural skip** in `assemble_context`: never counted against any limit,
  never a compaction/eviction candidate. Enforced by type, covered by a test.
- **Rescue-only relevance.** Relevance may only *promote* heat (protect an item);
  it must **never** demote an item or newly make one eviction-eligible.
- **Default-on, but a safety valve.** `DEFAULT_CONTEXT_TARGET = 384_000`.
  Eviction is on by default and fires only under genuine pressure
  (`total > high_water`); on a normal session under target it is a no-op.
- **Clamp, never error.** `high_water = min(target·(1+band), token_ceiling)`.
  If target ≥ model window, eviction degenerates gracefully to pure
  ceiling-overflow protection.
- **Static-musl friendly.** The default `Embedder` backend adds **no** heavy /
  native dependencies. The real `bge-small` backend is a future feature gate.
- **Cross-crate discipline.** Public-field additions to `ContextItem`
  (`Protection`) and any `ChatMsg`/policy change build against `--workspace`
  in every task (the ACM-1 lesson: `-p zoid-core`-scoped tests miss `zoid-tui`
  literal breaks).

---

## 3. File structure

| File | Responsibility | Change |
| ---- | -------------- | ------ |
| `crates/zoid-core/src/relevance.rs` | `Embedder` trait, `Embedding` enum, `LexicalEmbedder`, `FakeEmbedder`, `goal_text`, `relevance_scores`, `apply_relevance` | **Create** |
| `crates/zoid-core/src/context.rs` | `Protection` enum + field on `ContextItem`; `heat_of` gains relevance term | Modify |
| `crates/zoid-core/src/assembler.rs` | `ContextPolicy` target-band fields; `assemble_context` becomes Protection- + band-aware decider | Modify |
| `crates/zoid-core/src/lib.rs` | register `relevance` module | Modify |
| `crates/zoid-provider/src/model.rs` | recognize 1M-context model variants in `context_window` | Modify |
| `crates/zoid/src/agent.rs` | `record_evictions()` in `run_turn_inner`; `TurnConfig` carries embedder + target policy | Modify |
| `crates/zoid/src/main.rs` | `policy_from_config` sets `target_tokens` from `config.economy.context_target` (default 384k) | Modify |
| `crates/zoid/src/subagent.rs` | subagent policy: eviction posture for subagent branches | Modify |
| `crates/zoid-tui/src/tokens.rs` | `EVICT` glyph const | Modify |
| `crates/zoid-tui/src/chat.rs` | render eviction chip (Normal) + detail breakdown | Modify |
| `crates/zoid-tui/src/economy_view.rs` (+ drawer) | show evicted items as restorable in the live working set | Modify |

---

## 4. The relevance unit (`relevance.rs`)

### 4.1 The seam

```rust
/// Turns text into a comparable vector. Default backend is lexical (no deps);
/// a future `embeddings-bge` feature adds a dense bge-small backend.
pub trait Embedder {
    fn embed(&self, text: &str) -> Embedding;
}

/// One representation both backends can produce; cosine compares same-variant.
pub enum Embedding {
    /// (hashed-token, weight), sorted by token asc. The lexical default.
    Sparse(Vec<(u32, f32)>),
    /// Dense vector — bge-small later, behind a feature.
    Dense(Vec<f32>),
}

impl Embedding {
    /// Cosine similarity in [0.0, 1.0] for same-variant embeddings; 0.0 for
    /// a variant mismatch (never happens in one run — one embedder per session).
    pub fn cosine(&self, other: &Embedding) -> f32 { /* ... */ }
}
```

### 4.2 `LexicalEmbedder` (default backend)

- Lowercase; split on non-alphanumeric; additionally split `snake_case` and
  `camelCase`/`PascalCase` boundaries so code identifiers tokenize into their
  parts (`assemble_context` → `assemble`, `context`; `ContextWindow` →
  `context`, `window`).
- Hash each token to `u32`; weight by term frequency, L2-normalized so cosine is
  a pure dot product.
- Deterministic, allocation-bounded, zero new dependencies.

### 4.3 `FakeEmbedder` (test double)

- Deterministic, controllable similarity (e.g. constructed from an explicit
  token-set or a fixed score map) so relevance/eviction tests never depend on
  the lexical heuristic's exact numbers.

### 4.4 Goal vector

```rust
/// Concatenate the last `n` non-trivial user messages (most-recent first) as the
/// relevance query. "Non-trivial" filters empties and sub-threshold
/// confirmations ("yes", "3", "confirmed") so terse turns don't poison the goal.
pub fn goal_text(events: &[Event], n: usize) -> String;
```

- `MIN_GOAL_MSG_CHARS` (const) sets the triviality threshold.
- `GOAL_WINDOW_MSGS` (const) sets `n` (e.g. 3).

### 4.5 Scoring + application

```rust
/// key -> relevance in [0,1]. Embeds `goal_text` once, then scores each FILE
/// item's content against it. Non-file kinds get no relevance term.
pub fn relevance_scores(window: &ContextWindow, events: &[Event], e: &dyn Embedder)
    -> HashMap<String, f32>;

/// Layer relevance onto a window: recompute each item's heat via
/// `heat_of(refs, recency, relevance)` (rescue-only). Pure; the projection
/// itself stays embedder-free.
pub fn apply_relevance(window: &ContextWindow, scores: &HashMap<String, f32>)
    -> ContextWindow;
```

File content is resolved with the existing `file_contents(events)` mapping
(`file:{path}` → latest non-error output).

---

## 5. Heat + Protection (`context.rs`)

### 5.1 Relevance folds into heat

```rust
fn heat_of(refs: u32, last_turn: usize, current_turn: usize, relevance: f32) -> Heat
```

**Rescue-only rule:** an item that would be `Cold` by recency/refs but whose
`relevance >= RESCUE_THRESHOLD` is promoted to `Warm` (never eviction-eligible).
Low relevance **never** demotes and **never** forces a drop — recency/refs still
gate everything else. Tier-0 callers pass `relevance = 0.0` and get identical
behavior to today (backward-compatible).

### 5.2 Protection axis (orthogonal to `ItemKind`)

```rust
pub enum Protection { Normal, Protected, Immutable }
```

Added as a field on `ContextItem` (default `Normal`). Provenance (`ItemKind`) and
criticality (`Protection`) are separate axes.

- **`Immutable`** — never counted, never evicted, never compacted. **Structural
  skip** in `assemble_context`. **Guardrails = `System` + `Immutable`.**
- **`Protected`** — tool-schemas, user-pinned items: dropped only if the hard
  `token_ceiling` would otherwise break; never in a routine target-band pass.
- **`Normal`** (default) — fully managed per kind + heat.

System items are assigned `Immutable` at projection time; pinned items map to
`Protected` (pin already overrides eviction, so this is belt-and-suspenders and
makes the intent explicit).

---

## 6. Target band + assembler (`assembler.rs`)

### 6.1 Policy

```rust
pub struct ContextPolicy {
    pub token_ceiling: Option<u64>,   // hard API-overflow guard (ACM-1)
    pub auto_evict_cold: bool,
    pub compact_threshold: Option<u64>,
    pub target_tokens: Option<u64>,   // user preference; None disables band eviction
    pub target_band_pct: f32,         // e.g. 0.15
}
```

`DEFAULT_CONTEXT_TARGET: u64 = 384_000`. `main.rs` sets `target_tokens =
Some(config.economy.context_target.unwrap_or(DEFAULT_CONTEXT_TARGET))` **and
`auto_evict_cold = false`** (the band pass is the only, pressure-gated
cold-dropper); an explicit `context_target` of `0` disables band eviction
entirely.

### 6.2 The band invariant

```
floor ≤ low_water ≤ target ≤ high_water ≤ token_ceiling ≤ model_window
low_water  = target · (1 − band)
high_water = min(target · (1 + band), token_ceiling)
```

(`token_ceiling` itself defaults from the model window in ACM-1, so clamping to
it is also clamping to the window.)

### 6.3 `assemble_context` as decider

1. **Structural skip:** `Immutable` items are set aside first — always included,
   never counted toward `running`.
2. **Existing passes** (pin/manual-evict, then ceiling) run over the rest. The
   live policy sets `auto_evict_cold = false` (see below) so there is **no**
   routine every-turn cold drop; the band pass is the only cold-dropper.
3. **Target-band pass (new):** if `total > high_water`, drop **`Cold` `Normal`
   items only, largest-tokens-first** (fast reclaim), until `total ≤ low_water`
   or no `Cold` `Normal` items remain. It **never touches `Warm`/`Hot`** — since
   rescue promotes relevant items to `Warm`, rescued items are structurally
   outside the drop set (this is how rescue-only is enforced at the assembler).
   If all `Cold` `Normal` items are dropped and `total` is still `> high_water`,
   eviction **stops**: the window is legitimately full of recent/relevant
   content, and `target` is a *preference*, not a hard cap.
4. `Protected` items are dropped only if the hard `token_ceiling` (the real cap)
   would otherwise break — never by the band pass.

**Pressure-gated, not routine.** The live policy (`main.rs`) sets
`auto_evict_cold = false`; the target-band pass is the sole eviction path and
fires only above `high_water`. Hysteresis (evict down to `low_water`, act only
above `high_water`) prevents per-turn thrashing.

---

## 7. Live wire-in (`agent.rs`) — approach C

`record_evictions()`, called in `run_turn_inner` immediately before the
re-request (mirrors ACM-1's `record_compactions`):

```rust
let win = context_window(events);
let scores = relevance_scores(&win, events, embedder);
let win = apply_relevance(&win, &scores);
let sel = assemble_context(&win, &policy);
for it in &sel.excluded {
    if it.protection == Protection::Normal && !already_evicted(it, events) {
        emit(ContextMutation { item: it.key.clone(), op: Evict });
    }
}
```

**The wire-in.** `context_window` already folds `Evict` events onto items
(`MutationOp::Evict` arm exists → `item.evicted = true`, feeds the drawer). But
`conversation()` — which `build_request` maps over — currently **ignores**
`ContextMutation` (it's in the skip arm; see `conversation_ignores_usage_and_mutation`).
So the load-bearing wire-in is teaching `conversation()` to omit evicted items
from the **live request**, mirroring how ACM-1 taught it to substitute compacted
summaries:

- Pre-scan events into a `HashSet<String>` of currently-evicted item keys,
  folding `Evict`/`Restore` last-write-wins.
- Rebuild the same `tool-id → path` `call_path` map `context_window` uses, so
  each `ToolResult` event resolves to its context-window key (`file:{path}` when
  the paired `ToolCall` had a `tool_path`, else `tool:{name}:{id}`).
- Skip emitting a `ChatMsg::ToolResult` whose key is in the evicted set.

To avoid the key-derivation logic drifting between `context_window` and
`conversation()`, factor a shared `item_key_of(name, id, &call_path) → String`
helper in `context.rs` and use it in both. `build_request` itself needs **no**
change. Idempotent via an `already-evicted` set (same pattern as
`plan_compactions`'s `done` set).

Reversibility: `Restore` emits the inverse event; the projection re-includes the
item on the next turn.

---

## 8. Model-window recognition (`model.rs`)

Extend `model_info().context_window` so 1M-context variants report their true
window (e.g. `claude-opus-4-8[1m]` / names containing `1m` → `1_000_000`), while
the conservative defaults for other models stand. Without this, a default 384k
target is silently clamped to a 200k ceiling and never fires. The existing
`config.economy.context_ceiling` override is preserved. The model **picker** and
**pricing** remain out of scope.

---

## 9. Announce surface (semantic zoom)

Eviction is a `ContextMutation{Evict}` event and renders like ACM-1's compaction:

- **Summary** — turn digest: `context trimmed −38k (6 items)`.
- **Normal** — one-line chip like a tool call: `⑤ evicted 6 items → 384k (cold + low-relevance)`.
  New glyph `EVICT` const in `tokens.rs` beside `COMPACT`.
- **Detail** — per-item: which item, why (heat / relevance score / age), token
  delta, and that it is reversible.

Drawer = live working set (evicted items shown excluded + restorable);
transcript = the `Evict` events that got there. Together = the glass box.

---

## 10. Testing strategy

- **`relevance.rs`:** tokenization incl. snake/camel splitting; cosine bounds
  [0,1]; goal triviality filter drops "yes"/"3" but keeps a real request;
  `FakeEmbedder` determinism.
- **`context.rs`:** `heat_of` with `relevance = 0.0` == today (backward compat);
  rescue promotes cold+relevant to Warm; low relevance never demotes.
- **`assembler.rs`:** `Immutable` never counted/evicted (guardrail-safety test —
  vision success criterion); `Protected` drops only at hard ceiling; band pass
  evicts `Cold` `Normal` to low-water in one batch and does **not** re-fire next
  turn (hysteresis); **a rescued (`Warm`) item survives the band pass even when
  over high-water** (rescue-only enforcement); band pass stops rather than
  touching `Warm`/`Hot` when Cold is exhausted; clamp-to-ceiling when
  `target ≥ ceiling`; floor guard keeps current-turn items; with
  `auto_evict_cold = false` and `total ≤ high_water`, `excluded` is empty
  (default no-op).
- **Wire-in (`zoid` integration):** `build_request_omits_evicted_items` (twin of
  ACM-1's `build_request_carries_compacted_summary_into_live_messages`) proving
  eviction reaches live messages; `Restore` brings the item back.
- **`model.rs`:** 1M variant → 1M window; others unchanged; ceiling override
  still wins.
- **Every task builds `--workspace`** (the `Protection` field is a cross-crate
  public add).

---

## 11. Explicitly out of scope (seams honored, not built)

- Real `bge-small` backend — the `Embedder` trait + `Embedding::Dense` + the
  `embeddings-bge` feature gate are the seam; the backend ships later.
- Tier-2 generative (LLM) compaction — long-term; the one place
  propose-and-confirm will apply.
- RAG / additive retrieval — `retrieve()` remains a future additive seam.
- The `$` / budget-governor / model-routing layer.
- System-blob decomposition — `System` stays one `Immutable` item.
- Relevance for **messages** and **tool-results** — files only this slice; other
  kinds keep age-based heat.
- The model **picker** and **pricing** fields.

---

## 12. Success criteria

- File items carry a relevance score derived from the goal window; a central
  file read once is no longer classified `Cold` (rescue works).
- With a session pushed over `high_water`, eviction reclaims tokens down to
  `low_water`, is announced at each zoom level, and is undone by `Restore`.
- `Immutable` guardrails are provably never counted or evicted (test-enforced).
- Default-on eviction is a no-op on a normal session under 384k.
- On a 1M-context model, the 384k default actually engages (window recognized).
- `assemble_context` is called in production — the vision's "nothing calls it"
  critique is resolved.
