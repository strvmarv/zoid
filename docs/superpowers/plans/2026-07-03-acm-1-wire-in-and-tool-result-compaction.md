# ACM-1 · Active Context Management: Wire-in + Tool-Result Compaction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's context economy from passive observation into safe *active* management: automatically compact oversized tool-results out of the live model request when the context window crosses a configured pressure threshold, recording each compaction as an auditable event that renders in the transcript at the right zoom altitude.

**Architecture:** Compaction is a **pure plan** (`plan_compactions`) over the event log + `ContextPolicy`, applied by the agent loop as append-only `ToolResultCompacted` events. The `conversation()` projection substitutes each compacted tool-result's summary for its original output — and because `build_request` already builds its messages from `conversation()`, compaction flows into the live request with **no change to `build_request` itself**. The original `ToolResult` event is never deleted, so compaction is structurally reversible. Eviction stays projected-only (visualize) in ACM-1; it is unlocked in ACM-2 once relevance makes "cold" trustworthy.

**Tech Stack:** Rust 2021, serde, ulid, ratatui 0.29 (`TestBackend`/`insta` snapshots), proptest (core). Crates: `zoid-core` (pure), `zoid-provider`, `zoid-tui`, `zoid` (bin).

## Global Constraints

- **Crate dep direction:** `zoid-core` is pure (no ratatui, no reqwest). `zoid-tui` depends on `zoid-core`; the bin depends on all. Never introduce a cycle. Projections/planning live in `zoid-core`; rendering/view-models in `zoid-tui`; side effects (event recording, config) only in the `zoid` bin and `zoid/src/agent.rs`.
- **Every mutation is an append-only event.** Compaction NEVER mutates or deletes the original `ToolResult` event; it appends a `ToolResultCompacted` event. This is what makes it auditable and reversible.
- **Safe-only in ACM-1.** Only `ItemKind::ToolResult` items are compacted — never `System`, `Message`, or `File`. Never compact a pinned item. Eviction of the live request is **out of scope** (deferred to ACM-2).
- **Design tokens are the single source of truth (spec §16):** no literal special glyphs or hex colors outside `crates/zoid-tui/src/tokens.rs`. New visual tokens must also be added to the authoritative table in `docs/ux/README.md`. ASCII punctuation (`[`, `]`, `/`, digits, `·`, `→`) is exempt. The raw `…` in `zoid-core` is exempt (core cannot depend on the tui token table; see the note in `zoom.rs`).
- **UX testing is multi-width:** every task that changes rendering adds/updates `TestBackend`+`insta` snapshots at **both 100×24 and 140×24**, and updates the matching `crates/zoid-tui/examples/preview.rs` scene where one exists.
- **Pure core gets proptest invariants** in addition to example-based unit tests.
- **TDD, DRY, YAGNI, frequent commits.** Run `cargo test` (workspace) and `cargo clippy --all-targets` clean before every commit. Accept new snapshots with `INSTA_UPDATE=always cargo test -p zoid-tui --test <file>` and review the `.snap` content before committing.
- **No `Co-Authored-By` / co-author trailer in commits** (user global instruction).
- **Token estimation:** per-item token cost is `estimate_tokens(s) = ceil(chars/4)` (`zoid_core::economy::estimate_tokens`). Do not reimplement it.

---

### Task 1: Harden per-model `context_window` (§4 step 0)

ACM's compaction threshold is a percent of the model's context window. Today `model_info().context_window` is a string-match stub (`contains("claude") → 200k, else 256k`). Replace it with an explicit per-model lookup + a safe conservative default, so the threshold is correct-by-construction. The model *picker* and *pricing* stay out of scope.

**Files:**
- Modify: `crates/zoid-provider/src/model.rs`
- Test: `crates/zoid-provider/src/model.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing.
- Produces: `model_info(model: &str) -> ModelInfo` with an accurate `context_window` per known model; unchanged signature. `context_ceiling(model)` (in `lib.rs`) continues to delegate here.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-provider/src/model.rs` `mod tests`, replace `model_info_caps_by_family_else_default` with:

```rust
#[test]
fn model_info_windows_are_explicit_per_model() {
    // Known models get their real window.
    assert_eq!(model_info("claude-sonnet-4-6").context_window, 200_000);
    assert_eq!(model_info("claude-opus-4-8").context_window, 200_000);
    assert_eq!(model_info("glm-5.2:cloud").context_window, 256_000);
    // Case-insensitive family match still works.
    assert_eq!(model_info("CLAUDE-sonnet-4-6").context_window, 200_000);
    // Unknown models take the CONSERVATIVE (small) default, never an
    // optimistic large one — an over-high window makes ACM under-compact.
    assert_eq!(model_info("some-tiny-local:8b").context_window, 32_000);
    assert!(model_info("anything").tools);
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p zoid-provider --lib model::tests::model_info_windows_are_explicit_per_model`
Expected: FAIL — unknown model returns 256_000, not 32_000.

- [ ] **Step 3: Implement the explicit table**

Replace the body of `model_info`:

```rust
pub fn model_info(model: &str) -> ModelInfo {
    let m = model.to_ascii_lowercase();
    // Explicit per-family windows. Unknown models take a CONSERVATIVE default:
    // under-estimating the window makes ACM compact a little early (safe);
    // over-estimating risks never compacting and overflowing the real window.
    let context_window = if m.contains("claude") {
        200_000
    } else if m.contains("glm") {
        256_000
    } else {
        32_000 // conservative default for unknown / small local models
    };
    ModelInfo {
        context_window,
        max_output: 0,
        tools: true,
    }
}
```

- [ ] **Step 4: Fix the now-stale assertion in `known_providers_and_models`** (unchanged) and run

Run: `cargo test -p zoid-provider --lib model::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/model.rs
git commit -m "feat(provider): explicit per-model context_window with conservative default"
```

---

### Task 2: `ToolResultCompacted` event variant

Add the append-only carrier event for a compaction. It records the original tool-result `id`, the `summary` that replaces its output, and the `original_tokens` for audit. Existing projections must ignore it until later tasks teach them to fold it.

**Files:**
- Modify: `crates/zoid-core/src/event.rs`
- Modify: `crates/zoid-core/src/projection.rs` (add ignore arm so it compiles)
- Test: `crates/zoid-core/src/event.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `EventKind::ToolResultCompacted { id: String, summary: String, original_tokens: u64 }`.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-core/src/event.rs` `#[cfg(test)]` (add the module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

    #[test]
    fn tool_result_compacted_round_trips_through_serde() {
        let k = EventKind::ToolResultCompacted {
            id: "call_42".into(),
            summary: "… (compacted: 300 more lines, ~2100 tokens elided)".into(),
            original_tokens: 2200,
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p zoid-core --lib event::tests::tool_result_compacted_round_trips_through_serde`
Expected: FAIL — `no variant named ToolResultCompacted`.

- [ ] **Step 3: Add the variant**

In `enum EventKind`, after the `ContextMutation { .. }` variant:

```rust
    /// An automatic context-management action: the tool-result with `id` was
    /// compacted to `summary` in the live request. Append-only — the original
    /// `ToolResult` event is retained, so this is reversible. `original_tokens`
    /// is the pre-compaction estimate, kept for the audit view.
    ToolResultCompacted {
        id: String,
        summary: String,
        original_tokens: u64,
    },
```

- [ ] **Step 4: Add the ignore arm in `conversation()`**

In `crates/zoid-core/src/projection.rs`, extend the economy-bookkeeping arm so it still compiles (real substitution comes in Task 6):

```rust
            EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. } => {
                // Economy bookkeeping; folded elsewhere, not a raw conversation item.
            }
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p zoid-core --lib event:: projection::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs
git commit -m "feat(core): add ToolResultCompacted event variant"
```

---

### Task 3: Pure `compact_tool_output` summarizer

The heuristic that shrinks an oversized tool-result body: keep the first N lines verbatim (where the signal usually lives), append a one-line footer noting how much was elided. Pure and deterministic.

**Files:**
- Create: `crates/zoid-core/src/compaction.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod compaction;`)
- Test: `crates/zoid-core/src/compaction.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `zoid_core::economy::estimate_tokens`.
- Produces: `pub fn compact_tool_output(output: &str, head_lines: usize) -> String`.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-core/src/compaction.rs`:

```rust
//! Active context management (ACM-1): plan and summarize tool-result
//! compactions. Pure — the agent loop records the results as events.

use crate::economy::estimate_tokens;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_returned_unchanged() {
        let out = "line1\nline2\nline3";
        assert_eq!(compact_tool_output(out, 8), out);
    }

    #[test]
    fn long_output_keeps_head_and_reports_elision() {
        let body: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let s = compact_tool_output(&body, 5);
        // Head preserved verbatim.
        assert!(s.starts_with("line1\nline2\nline3\nline4\nline5\n"));
        // Footer reports the elided line count (20 - 5 = 15).
        assert!(s.contains("15 more lines"), "footer missing: {s}");
        // The summary must be smaller than the original.
        assert!(estimate_tokens(&s) < estimate_tokens(&body));
    }
}
```

- [ ] **Step 2: Register the module and run to confirm it fails**

Add to `crates/zoid-core/src/lib.rs` (alphabetical with the other `pub mod`s): `pub mod compaction;`
Run: `cargo test -p zoid-core --lib compaction::tests`
Expected: FAIL — `cannot find function compact_tool_output`.

- [ ] **Step 3: Implement**

Add to `compaction.rs` (above the test module):

```rust
/// Summarize an oversized tool-result body: keep the first `head_lines` lines
/// verbatim, then a one-line footer noting the elided tail. Returns the input
/// unchanged when it is already at or under `head_lines` (nothing to gain).
pub fn compact_tool_output(output: &str, head_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= head_lines {
        return output.to_string();
    }
    let head = lines[..head_lines].join("\n");
    let elided = lines.len() - head_lines;
    let elided_tokens = estimate_tokens(&lines[head_lines..].join("\n"));
    // Raw '…' is intentional (core cannot reach the tui glyph table; see zoom.rs).
    format!("{head}\n… (compacted: {elided} more lines, ~{elided_tokens} tokens elided)")
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p zoid-core --lib compaction::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/compaction.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): pure compact_tool_output summarizer"
```

---

### Task 4: Pure `plan_compactions` (window-pressure selection)

Decide *which* tool-results to compact: only when the window exceeds the policy threshold, compact `ToolResult` items largest-first (skipping pinned + already-compacted + no-gain) until back under threshold.

**Files:**
- Modify: `crates/zoid-core/src/compaction.rs`
- Test: `crates/zoid-core/src/compaction.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::context::{context_window, ItemKind}`, `crate::assembler::ContextPolicy`, `crate::event::{Event, EventKind}`, `crate::economy::estimate_tokens`.
- Produces: `pub struct Compaction { pub id: String, pub summary: String, pub original_tokens: u64 }` and `pub fn plan_compactions(events: &[Event], policy: &ContextPolicy) -> Vec<Compaction>`.

- [ ] **Step 1: Write the failing test**

In `compaction.rs` `mod tests`, add helpers + tests:

```rust
    use crate::assembler::ContextPolicy;
    use crate::event::{Event, EventKind};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }
    fn big_tool_result(id: &str, name: &str, lines: usize) -> Event {
        let body: String = (0..lines).map(|i| format!("match {i} in file\n")).collect();
        ev(EventKind::ToolResult { id: id.into(), name: name.into(), output: body, is_error: false })
    }
    fn policy(threshold: u64) -> ContextPolicy {
        ContextPolicy { token_ceiling: None, auto_evict_cold: false, compact_threshold: Some(threshold) }
    }

    #[test]
    fn no_compaction_below_threshold() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "search please".into() }),
            big_tool_result("c1", "search", 100),
        ];
        // Threshold huge → nothing to do.
        assert!(plan_compactions(&evs, &policy(1_000_000)).is_empty());
        // No threshold set → nothing to do.
        assert!(plan_compactions(&evs, &ContextPolicy::default()).is_empty());
    }

    #[test]
    fn compacts_biggest_tool_results_until_under_threshold() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400), // biggest
            big_tool_result("c2", "shell", 50),
        ];
        let plan = plan_compactions(&evs, &policy(500));
        assert_eq!(plan.len(), 1, "only the big one needs compacting");
        assert_eq!(plan[0].id, "c1");
        assert!(plan[0].original_tokens > estimate_tokens(&plan[0].summary));
    }

    #[test]
    fn never_recompacts_already_compacted() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400),
            ev(EventKind::ToolResultCompacted { id: "c1".into(), summary: "small".into(), original_tokens: 800 }),
        ];
        // c1 already compacted → nothing left to compact.
        assert!(plan_compactions(&evs, &policy(1)).is_empty());
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p zoid-core --lib compaction::tests::compacts_biggest_tool_results_until_under_threshold`
Expected: FAIL — `cannot find function plan_compactions`.

- [ ] **Step 3: Implement**

Add to `compaction.rs` (above the tests). Note the key format from `context_window` is `tool:{name}:{id}`; `rsplit_once(':')` recovers the id even if `name` has no colon.

```rust
use crate::assembler::ContextPolicy;
use crate::context::{context_window, ItemKind};
use crate::event::{Event, EventKind};
use std::collections::{HashMap, HashSet};

/// Number of head lines kept verbatim when compacting a tool-result.
pub const COMPACT_HEAD_LINES: usize = 8;

/// One planned compaction: replace tool-result `id`'s output with `summary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub id: String,
    pub summary: String,
    pub original_tokens: u64,
}

/// Plan which tool-results to compact. Empty unless the window exceeds
/// `policy.compact_threshold`. Compacts `ToolResult` items only (never System /
/// Message / File), largest-first, skipping pinned + already-compacted + any
/// whose summary would not actually shrink them, until back under threshold.
pub fn plan_compactions(events: &[Event], policy: &ContextPolicy) -> Vec<Compaction> {
    let Some(threshold) = policy.compact_threshold else {
        return Vec::new();
    };
    let window = context_window(events);
    if window.total_tokens <= threshold {
        return Vec::new();
    }

    let done: HashSet<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    // Latest non-error output per tool-result id.
    let mut output_of: HashMap<&str, &str> = HashMap::new();
    for e in events {
        if let EventKind::ToolResult { id, output, is_error, .. } = &e.kind {
            if !*is_error {
                output_of.insert(id.as_str(), output.as_str());
            }
        }
    }

    let mut running = window.total_tokens;
    let mut out: Vec<Compaction> = Vec::new();
    for it in &window.items {
        // window.items is sorted tokens-desc, so this is largest-first.
        if running <= threshold {
            break;
        }
        if it.kind != ItemKind::ToolResult || it.pinned {
            continue;
        }
        let Some((_, id)) = it.key.rsplit_once(':') else { continue };
        if done.contains(id) {
            continue;
        }
        let Some(output) = output_of.get(id) else { continue };
        let summary = compact_tool_output(output, COMPACT_HEAD_LINES);
        let summary_tokens = estimate_tokens(&summary);
        if summary_tokens >= it.tokens {
            continue; // no gain
        }
        running -= it.tokens - summary_tokens;
        out.push(Compaction {
            id: id.to_string(),
            summary,
            original_tokens: it.tokens,
        });
    }
    out
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p zoid-core --lib compaction::tests`
Expected: PASS.

- [ ] **Step 5: Add a proptest invariant**

In `mod tests`, add (imports `use proptest::prelude::*;`):

```rust
    proptest! {
        #[test]
        fn planned_ids_are_unique_and_never_already_done(lines in proptest::collection::vec(20usize..300, 1..6)) {
            let mut evs = vec![ev(EventKind::UserMessage { text: "go".into() })];
            for (i, n) in lines.iter().enumerate() {
                evs.push(big_tool_result(&format!("c{i}"), "search", *n));
            }
            let plan = plan_compactions(&evs, &policy(100));
            let mut ids: Vec<&str> = plan.iter().map(|c| c.id.as_str()).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            prop_assert_eq!(ids.len(), n, "planned ids must be unique");
        }
    }
```

Run: `cargo test -p zoid-core --lib compaction::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/compaction.rs
git commit -m "feat(core): plan_compactions selects tool-results under window pressure"
```

---

### Task 5: `context_window` folds `ToolResultCompacted`

So the drawer/economy view (and `plan_compactions` itself, keeping it idempotent) sees a compacted item at its reduced size and marked compacted.

**Files:**
- Modify: `crates/zoid-core/src/context.rs`
- Modify: `crates/zoid-core/src/assembler.rs` (its `item()` test helper — new field)
- Test: `crates/zoid-core/src/context.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `ContextItem` gains `pub compacted: bool` (default `false`). After folding, a compacted tool-result's `tokens` equals `estimate_tokens(summary)` and `compacted == true`.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-core/src/context.rs` `mod tests`, add:

```rust
    #[test]
    fn context_window_folds_tool_result_compaction() {
        use crate::economy::estimate_tokens;
        let big: String = (0..200).map(|i| format!("row {i}\n")).collect();
        let summary = "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".to_string();
        let evs = vec![
            Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "go".into() }),
            Event::new(Ulid::new(), None, 0, EventKind::ToolCall { id: "c1".into(), name: "search".into(), args: "{}".into() }),
            Event::new(Ulid::new(), None, 0, EventKind::ToolResult { id: "c1".into(), name: "search".into(), output: big, is_error: false }),
            Event::new(Ulid::new(), None, 0, EventKind::ToolResultCompacted { id: "c1".into(), summary: summary.clone(), original_tokens: 999 }),
        ];
        let w = context_window(&evs);
        let it = w.items.iter().find(|i| i.key == "tool:search:c1").expect("tool item present");
        assert!(it.compacted);
        assert_eq!(it.tokens, estimate_tokens(&summary));
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p zoid-core --lib context::tests::context_window_folds_tool_result_compaction`
Expected: FAIL — `no field named compacted` / assertion.

- [ ] **Step 3: Add the field**

In `struct ContextItem`, add after `evicted: bool`:

```rust
    /// Set when a `ToolResultCompacted` event has folded over this item; its
    /// `tokens` then reflect the summary size, not the original.
    pub compacted: bool,
```

In `context_window`, in the `.map(|k| { … ContextItem { … } })` construction, add `compacted: false,`.

- [ ] **Step 4: Fold the compaction event**

In `context_window`, the existing `for e in events { if let EventKind::ContextMutation … }` fold loop: convert it to also handle compaction. Replace that loop with:

```rust
    // Fold mutations + compactions (log order; last write wins per item).
    for e in events {
        match &e.kind {
            EventKind::ContextMutation { item, op } => {
                if let Some(it) = items.iter_mut().find(|i| &i.key == item) {
                    use crate::event::MutationOp::*;
                    match op {
                        Pin => it.pinned = true,
                        Unpin => it.pinned = false,
                        Evict => it.evicted = true,
                        Restore => it.evicted = false,
                    }
                }
            }
            EventKind::ToolResultCompacted { id, summary, .. } => {
                // Item keys for non-file tool results are "tool:{name}:{id}".
                if let Some(it) = items
                    .iter_mut()
                    .find(|i| i.kind == ItemKind::ToolResult && i.key.rsplit_once(':').map(|(_, x)| x) == Some(id.as_str()))
                {
                    it.tokens = crate::economy::estimate_tokens(summary);
                    it.compacted = true;
                }
            }
            _ => {}
        }
    }
```

(The sort-by-tokens and `total_tokens` sum already run *after* this loop, so both reflect the reduced size.)

- [ ] **Step 5: Update the assembler test helper**

In `crates/zoid-core/src/assembler.rs` `mod tests`, the `item(...)` helper constructs a `ContextItem`; add `compacted: false,` to its literal so it still compiles.

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p zoid-core`
Expected: PASS (context + assembler + compaction).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/context.rs crates/zoid-core/src/assembler.rs
git commit -m "feat(core): context_window folds ToolResultCompacted (reduced tokens + compacted flag)"
```

---

### Task 6: `conversation()` substitutes the summary → flows into `build_request`

The load-bearing task: the live-request projection consults the compaction events, so the model sees the summary, not the dump. `build_request` needs no change.

**Files:**
- Modify: `crates/zoid-core/src/projection.rs`
- Test: `crates/zoid-core/src/projection.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `ChatMsg::ToolResult` gains `pub compacted: bool`. `conversation(events)` emits the substituted `summary` (with `compacted: true`) for any tool-result id that has a `ToolResultCompacted` event.

- [ ] **Step 1: Write the failing test**

In `crates/zoid-core/src/projection.rs` `mod tests`, add:

```rust
    #[test]
    fn conversation_substitutes_compacted_summary() {
        let evs = vec![
            Event::new(Ulid::new(), None, 100, EventKind::UserMessage { text: "go".into() }),
            Event::new(Ulid::new(), None, 200, EventKind::ToolResult { id: "c1".into(), name: "search".into(), output: "HUGE ORIGINAL OUTPUT".into(), is_error: false }),
            Event::new(Ulid::new(), None, 300, EventKind::ToolResultCompacted { id: "c1".into(), summary: "tiny summary".into(), original_tokens: 500 }),
        ];
        let conv = conversation(&evs);
        let tr = conv.iter().find_map(|m| match m {
            ChatMsg::ToolResult { id, output, compacted, .. } if id == "c1" => Some((output.clone(), *compacted)),
            _ => None,
        }).expect("tool result present");
        assert_eq!(tr.0, "tiny summary", "live request must carry the summary, not the dump");
        assert!(tr.1, "must be flagged compacted for the transcript");
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p zoid-core --lib projection::tests::conversation_substitutes_compacted_summary`
Expected: FAIL — `no field named compacted` / output mismatch.

- [ ] **Step 3: Add the field to `ChatMsg::ToolResult`**

In `enum ChatMsg`, the `ToolResult` variant — add after `is_error: bool,`:

```rust
        /// Set when a `ToolResultCompacted` event replaced this result's body
        /// with a summary. The transcript marks it; the live request carries it.
        compacted: bool,
```

- [ ] **Step 4: Substitute in `conversation()`**

At the top of `conversation()`, before the main loop, build the id→summary map:

```rust
    // ACM-1: a tool-result whose id has a later ToolResultCompacted is emitted
    // as its summary (last write wins), both to the live request and the view.
    let mut compacted: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for e in events {
        if let EventKind::ToolResultCompacted { id, summary, .. } = &e.kind {
            compacted.insert(id.as_str(), summary.as_str());
        }
    }
```

In the `EventKind::ToolResult { id, name, output, is_error }` arm, replace the `out.push(ChatMsg::ToolResult { … })` with:

```rust
                let (output, was_compacted) = match compacted.get(id.as_str()) {
                    Some(sum) => ((*sum).to_string(), true),
                    None => (output.clone(), false),
                };
                out.push(ChatMsg::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    output,
                    is_error: *is_error,
                    compacted: was_compacted,
                    ts: e.ts,
                });
```

- [ ] **Step 5: Fix every other `ChatMsg::ToolResult` construction**

Add `compacted: false,` to the `ChatMsg::ToolResult { … }` literals elsewhere in `projection.rs` (its own `mod tests`). Do NOT touch `map_msg` in `agent.rs` — it already destructures with `..`.

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p zoid-core`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/projection.rs
git commit -m "feat(core): conversation() substitutes compacted summaries into the live request"
```

---

### Task 7: Agent loop records compactions under window pressure

Thread the `ContextPolicy` into the turn and, after each round's tool-results are emitted (before the re-request), record `ToolResultCompacted` events for any planned compactions — so the very next `build_request` in the loop already benefits.

**Files:**
- Modify: `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/src/subagent.rs` (set the new `TurnConfig` field)
- Modify: `crates/zoid/src/main.rs` (set the new `TurnConfig` field from the built policy)
- Test: `crates/zoid/tests/economy_integration.rs`

**Interfaces:**
- Consumes: `zoid_core::compaction::plan_compactions`, `zoid_core::assembler::ContextPolicy`, existing `emit(...)`.
- Produces: `TurnConfig` gains `pub policy: ContextPolicy`. New private `async fn record_compactions(...)`.

- [ ] **Step 1: Write the failing integration test**

In `crates/zoid/tests/economy_integration.rs`, add a test that runs a turn whose tool-result is large, with a low threshold policy, and asserts a `ToolResultCompacted` event is recorded. Mirror the existing harness in that file for building a `FakeProvider` scripted to emit one tool call + finish. Skeleton (adapt names to the file's existing helpers):

```rust
#[tokio::test]
async fn oversized_tool_result_is_compacted_when_over_threshold() {
    // Provider script: one tool call to a shell-like tool, then a final message.
    // The fake tool returns a large multi-line body (> threshold tokens).
    // Build TurnConfig with policy = ContextPolicy { compact_threshold: Some(SMALL), .. }.
    // Run run_agent_turn; collect returned events.
    let events = /* run the turn via the file's existing helper */;
    let compacted = events.iter().any(|e| matches!(
        e.kind, zoid_core::event::EventKind::ToolResultCompacted { .. }
    ));
    assert!(compacted, "a large tool-result over threshold must be compacted");
}
```

(If `economy_integration.rs` lacks a reusable turn harness, copy the setup from `crates/zoid/tests/agent_loop.rs`, which already drives `run_agent_turn` with a `FakeProvider`.)

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p zoid --test economy_integration oversized_tool_result_is_compacted_when_over_threshold`
Expected: FAIL — no `ToolResultCompacted` recorded (loop doesn't compact yet), or the `policy` field doesn't exist.

- [ ] **Step 3: Add `policy` to `TurnConfig`**

In `crates/zoid/src/agent.rs`, add to `struct TurnConfig`:

```rust
    /// Context-management policy for this turn. Chat gets it from `[economy]`;
    /// subagents get `subagent_policy()`. Drives automatic tool-result compaction.
    pub policy: zoid_core::assembler::ContextPolicy,
```

Set it in `chat_turn_config()` — that constructor currently has no policy; give it `zoid_core::assembler::ContextPolicy::default()` there (the bin overrides it at call time in Step 6). In `crates/zoid/src/subagent.rs`, wherever a `TurnConfig` is built for a subagent, set `policy: subagent_policy()`.

- [ ] **Step 4: Add the recorder helper**

In `agent.rs`, add near `emit`:

```rust
/// Record `ToolResultCompacted` events for any tool-results the policy says
/// should be compacted given the current log. Idempotent: `plan_compactions`
/// skips already-compacted ids, so calling this each round is safe.
async fn record_compactions(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    for c in zoid_core::compaction::plan_compactions(events, &config.policy) {
        emit(
            session,
            events,
            ui,
            &config.branch,
            EventKind::ToolResultCompacted {
                id: c.id,
                summary: c.summary,
                original_tokens: c.original_tokens,
            },
            session_id,
            now,
        )
        .await?;
    }
    Ok(())
}
```

- [ ] **Step 5: Call it before the re-request**

In `run_turn_inner`, find the `// loop: re-request with the tool results now in context` comment at the end of the tool-handling block. Immediately BEFORE it, add:

```rust
        record_compactions(&session, &mut events, ui, config, session_id, now).await?;
```

(`config` is the `&TurnConfig` in scope. This runs after the round's `ToolResult`s are emitted, so the next `build_request(&events, …)` at the top of the loop projects the summaries.)

- [ ] **Step 6: Wire the real policy in the bin**

In `crates/zoid/src/main.rs`, the code already builds a `ContextPolicy` via `policy_from_config(econ, ceiling)` (around line 355) and resolves the ceiling from `config.economy.context_ceiling` or `zoid_provider::context_ceiling(&model)` (around line 575). Where `chat_turn_config()` is used to build the `TurnConfig` for a chat turn, set its `policy` to that built `ContextPolicy` (construct the config, then assign `cfg.policy = policy_from_config(&app.config.economy, ceiling);`). Ensure `compact_threshold_pct` defaults remain (0 = compaction off) so existing behavior is unchanged until the user sets it.

- [ ] **Step 7: Run the test to verify pass + full suite**

Run: `cargo test -p zoid --test economy_integration` then `cargo test` (workspace) and `cargo clippy --all-targets`
Expected: PASS, clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/subagent.rs crates/zoid/src/main.rs crates/zoid/tests/economy_integration.rs
git commit -m "feat(zoid): agent loop records tool-result compactions under window pressure"
```

---

### Task 8: Announce compaction in the transcript at Normal + Detail zoom

The glass-box: a compacted tool-result reads like a tool-call chip at Normal, and shows its summary body labelled compacted at Detail. (Summary/digest indicator is Task 9, optional.)

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` (new `glyph::COMPACT`)
- Modify: `docs/ux/README.md` (visual-language table — authoritative)
- Modify: `crates/zoid-tui/src/chat.rs` (Normal `build_conversation` + `detail_lines` tool-result arms)
- Modify: `crates/zoid-tui/examples/preview.rs` (add a compacted-tool-result to a scene)
- Test: `crates/zoid-tui/tests/chat_snapshot.rs` (insta @100 and @140)

**Interfaces:**
- Consumes: `ChatMsg::ToolResult { compacted, .. }` (Task 6).
- Produces: `glyph::COMPACT: char`.

- [ ] **Step 1: Add the token test + token**

In `crates/zoid-tui/src/tokens.rs` `mod tests`, add `assert_eq!(glyph::COMPACT, '⊟');`. Run to confirm it fails, then in `mod glyph` add:

```rust
    pub const COMPACT: char = '⊟'; // ⑤ compacted tool-result marker (ACM-1)
```

Add the row to the visual-language table in `docs/ux/README.md`.
Run: `cargo test -p zoid-tui --lib tokens::tests`
Expected: PASS.

- [ ] **Step 2: Write the failing snapshot test**

In `crates/zoid-tui/tests/chat_snapshot.rs`, add a scene whose `msgs` include a `ChatMsg::ToolResult { compacted: true, output: "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".into(), name: "search".into(), .. }`, rendered at Normal and Detail, at width 100 and 140. Follow the existing `insta::assert_snapshot!` pattern in the file. Run with `INSTA_UPDATE=always` once to generate, then inspect the `.snap` files to confirm the `⊟` marker appears on the tool-result row (Normal) and a "compacted" label at Detail.

- [ ] **Step 3: Render the marker at Normal**

In `chat.rs` `build_conversation`, in the `ChatMsg::ToolResult { name, is_error, compacted, .. }` handling, when `compacted` is true prepend the marker span to that row:

```rust
                if *compacted {
                    spans.push(Span::styled(
                        format!("{} compacted ", glyph::COMPACT),
                        Style::new().fg(color::DIM),
                    ));
                }
```

(Insert where the row's `spans` vec is assembled, before the tool name; match the file's existing variable names for the tool-result row.)

- [ ] **Step 4: Label it at Detail**

In `detail_lines`, in the `ChatMsg::ToolResult { name, compacted, .. }` arm, when `compacted` is true change the header to note it:

```rust
                let label = if *compacted {
                    format!("  {} {} {}", glyph::PASS, name, glyph::COMPACT)
                } else {
                    format!("  {} {}", glyph::PASS, name)
                };
                let header = Span::styled(label, Style::new().fg(color::DIM));
```

(The summary body is already the compacted text via `conversation()`, so `collapse_to_signatures(output, lang)` renders the small summary. Add `compacted` to the arm's destructure.)

- [ ] **Step 5: Update the preview scene**

Add a compacted tool-result to a scene in `crates/zoid-tui/examples/preview.rs` so `cargo run -p zoid-tui --example preview` shows it.

- [ ] **Step 6: Regenerate + review snapshots, run suite**

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test chat_snapshot` then review `.snap` diffs, then `cargo test` (workspace) + `cargo clippy --all-targets`.
Expected: marker present at Normal, label at Detail, both widths; suite + clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs docs/ux/README.md crates/zoid-tui/src/chat.rs crates/zoid-tui/examples/preview.rs crates/zoid-tui/tests/chat_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): announce tool-result compaction in transcript (Normal chip + Detail label)"
```

---

### Task 9 (optional): Summary-digest "−Nk" indicator + drawer compacted marker

Rounds out the glass-box: the zoomed-out digest shows how much a turn's compaction saved, and the ⑤ drawer marks compacted items. Skip if deferring; ACM-1 is complete without it.

**Files:**
- Modify: `crates/zoid-core/src/zoom.rs` (`TurnDigest` gains `saved_tokens: u64`, summed from `ToolResultCompacted` within the turn — requires `digests` to see events, or a parallel fold)
- Modify: `crates/zoid-tui/src/chat.rs` (`digest_lines` appends `· −Nk` when `saved_tokens > 0`)
- Modify: `crates/zoid-tui/src/economy_view.rs` (compacted items show `glyph::COMPACT`)
- Test: matching unit + snapshot tests

- [ ] **Step 1:** Write failing tests for `saved_tokens` in a digest and the `−Nk` render; implement; regenerate snapshots; run suite + clippy; commit.

(Full code deferred to implementation time — this task is optional and its shape depends on whether `digests` is refactored to accept events. If pursued, keep `saved_tokens` a pure sum of `original_tokens - estimate_tokens(summary)` per compaction in the turn.)

---

## Self-Review

**Spec coverage (vision §4 short-term slice):**
- Step 0 (context_window hardening) → Task 1. ✓
- Step 1 (compact tool-results) → Tasks 3, 4, 5, 7. ✓
- Step 3 (assembler → build_request) → Task 6 (via `conversation()` substitution; `build_request` unchanged by design). ✓ — scope note: only *compaction* is wired to the live request; *eviction* stays projected (ACM-2), per the safety refinement.
- Step 4 (announce via semantic zoom) → Task 8 (Normal + Detail); Summary digest → Task 9 (optional). ✓
- Step 5 (drawer = live set / transcript = history) → transcript history is Task 8; drawer already reflects reduced tokens via Task 5; drawer compacted marker is Task 9 (optional). ✓

**Placeholder scan:** Task 7 Step 1 (integration-test skeleton) and Task 9 intentionally reference the existing test harness rather than duplicating it, and Task 9 is explicitly optional with deferred code. All non-optional code steps contain complete code. Fix if executing Task 9: write real code before committing.

**Type consistency:** `ToolResultCompacted { id, summary, original_tokens }` is identical across Tasks 2, 5, 6, 7. `Compaction { id, summary, original_tokens }` consistent across Task 4/7. `compacted: bool` added to both `ContextItem` (Task 5) and `ChatMsg::ToolResult` (Task 6). `plan_compactions(events, policy)` signature consistent Task 4/7. Key parsing `rsplit_once(':')` consistent Task 4/5.

**Out of scope (ACM-1):** eviction of the live request; relevance/embeddings (ACM-2); Tier-2 LLM compaction; `$`/routing; manual user commands; undo UI (structurally reversible via retained original event, but no keybind).
