# Context Overflow Protection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent local models with small context windows (32K) from hitting
context-overflow errors when a single turn accumulates too much tool output.
Two independent fixes: (1) pre-request context trimming as a safety net,
and (2) a lower default read limit to reduce per-call output size.

**Architecture:** Two tasks, independently shippable.
- Task 1: Pre-request emergency compaction in the agent loop — if estimated
  tokens exceed the model's context window, force-compact the largest
  uncompacted tool results until it fits. Safety net for any tool.
- Task 2: Lower the `read` tool's default limit from 2000 to 500 lines.
  Prevents a single `read` of a large file from producing 5K+ tokens.

**Tech Stack:** Rust (`zoid-core`, `zoid` crates). No new dependencies.

**Spec:** None — this is a gap found during local model evaluation
(`docs/superpowers/specs/2026-07-25-local-model-evaluation-design.md`).

## Global Constraints

- **No coverage reduction.** All existing tests must pass.
- **No release/dist profile changes.**
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid/src/agent.rs` | Pre-request context trimming before provider call | Modify (Task 1) |
| `crates/zoid-core/src/compaction.rs` | `plan_compactions_for_overflow` — force-compact largest results | Add (Task 1) |
| `crates/zoid-tools/src/read.rs` | `DEFAULT_LIMIT` 2000 → 500 | Modify (Task 2) |

---

### Task 1: Pre-request context trimming

**Goal:** Before each provider call, estimate the request's token count. If it
exceeds the model's context window, force-compact uncompacted tool results
(largest first) until the estimate fits. This is a safety net — normal
compaction between sub-turns handles the common case; this catches the case
where a single tool result pushes context past the limit in one sub-turn.

**Files:**
- Modify: `crates/zoid/src/agent.rs` (the `build_request_with_thinking` caller)
- Modify: `crates/zoid-core/src/compaction.rs` (new `plan_compactions_for_overflow`)

- [ ] **Step 1: Write tests for `plan_compactions_for_overflow`**

Add to `crates/zoid-core/src/compaction.rs` test module:

```rust
#[test]
fn plan_compactions_for_overflow_compacts_largest_first() {
    // Three tool results: 100, 200, 300 tokens. Context window is 500,
    // overhead is 100. Request is 700 tokens — must compact the 300
    // and the 200 to fit under 500.
    let events = vec![
        tool_call("tc1", "shell"),
        tool_result("tc1", &"x".repeat(300)), // ~100 tokens
        tool_call("tc2", "shell"),
        tool_result("tc2", &"x".repeat(600)), // ~200 tokens
        tool_call("tc3", "read"),
        tool_result("tc3", &"x".repeat(900)), // ~300 tokens
    ];
    let policy = ContextPolicy {
        token_ceiling: Some(500),
        auto_evict_cold: false,
        compact_threshold: None,
    };
    let plan = plan_compactions_for_overflow(
        events.iter(),
        &policy,
        100, // overhead tokens
        None, // no calibration
    );
    // Must compact tc3 (largest) and tc2 (second largest).
    assert!(plan.compactions.iter().any(|c| c.id == "tc3"));
    assert!(plan.compactions.iter().any(|c| c.id == "tc2"));
    // tc1 should NOT be compacted (smallest, and compacting it would
    // over-shoot).
    assert!(!plan.compactions.iter().any(|c| c.id == "tc1"));
}

#[test]
fn plan_compactions_for_overflow_noop_when_under_ceiling() {
    let events = vec![
        tool_call("tc1", "shell"),
        tool_result("tc1", "hello"),
    ];
    let policy = ContextPolicy {
        token_ceiling: Some(50000),
        auto_evict_cold: false,
        compact_threshold: None,
    };
    let plan = plan_compactions_for_overflow(
        events.iter(),
        &policy,
        100,
        None,
    );
    assert!(plan.compactions.is_empty());
}

#[test]
fn plan_compactions_for_overflow_compacts_all_still_over() {
    // Even compacting everything, context is still over the ceiling.
    // The function should compact ALL uncompacted tool results (best effort).
    let events = vec![
        tool_call("tc1", "shell"),
        tool_result("tc1", &"x".repeat(60000)), // ~20000 tokens
    ];
    let policy = ContextPolicy {
        token_ceiling: Some(100),
        auto_evict_cold: false,
        compact_threshold: None,
    };
    let plan = plan_compactions_for_overflow(
        events.iter(),
        &policy,
        100,
        None,
    );
    // Must compact tc1 even though it alone won't fit — best effort.
    assert!(plan.compactions.iter().any(|c| c.id == "tc1"));
}
```

Run: `cargo test -p zoid-core --lib compaction::tests::plan_compactions_for_overflow`
Expected: FAIL (function doesn't exist).

- [ ] **Step 2: Implement `plan_compactions_for_overflow`**

Add to `crates/zoid-core/src/compaction.rs`:

```rust
/// Like `plan_compactions`, but driven by a hard ceiling (the model's
/// context window) rather than a soft threshold. Compacts the LARGEST
/// uncompacted tool results first until the estimated total fits under
/// `token_ceiling + overhead`. Best effort: if even compacting everything
/// doesn't fit, returns all available compactions (the provider will still
/// reject, but at least we tried).
pub fn plan_compactions_for_overflow<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &ContextPolicy,
    overhead_tokens: u64,
    calibration_ratio: Option<f64>,
) -> CompactionPlan {
    let window = crate::context::context_window(events);
    let total = window.total_tokens + overhead_tokens;
    let ceiling = policy.token_ceiling.unwrap_or(u64::MAX);
    if total <= ceiling {
        return CompactionPlan::default();
    }
    // Collect uncompacted tool results, sorted by token size (largest first).
    let mut candidates: Vec<(String, u64)> = Vec::new();
    for item in &window.items {
        if let crate::context::ItemKind::ToolResult { id, .. } = &item.kind {
            if !item.compacted {
                candidates.push((id.clone(), item.tokens));
            }
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    // Compact largest-first until we fit (or run out).
    let mut savings = 0u64;
    let mut compactions = Vec::new();
    let deficit = total.saturating_sub(ceiling);
    for (id, tokens) in &candidates {
        if savings >= deficit {
            break;
        }
        // Compacting replaces the full output with 8 head lines + marker.
        // Estimate the savings as ~90% of the original tokens.
        let saved = tokens.saturating_mul(90) / 100;
        savings += saved;
        compactions.push(Compaction {
            id: id.clone(),
            original_tokens: *tokens,
            summary_tokens: tokens - saved,
        });
    }
    CompactionPlan { compactions }
}
```

Run: `cargo test -p zoid-core --lib compaction::tests::plan_compactions_for_overflow`
Expected: PASS.

- [ ] **Step 3: Wire into the agent loop**

In `crates/zoid/src/agent.rs`, before the `provider.stream()` call, add a
check: estimate the request's tokens, and if it exceeds the model's context
window, force-compact via `plan_compactions_for_overflow`.

The key location is after `build_request_with_thinking` returns and before
`provider.stream(req)` is called. The agent loop should:
1. Compute `context_window_with(events, overhead)` to get `total_tokens`.
2. If `total_tokens > model_context_window`, call `plan_compactions_for_overflow`.
3. If the plan has compactions, emit `ToolResultCompacted` events.
4. Rebuild the request (the compacted events are now in the log).

This reuses the existing `record_compactions` machinery — the only difference
is the trigger (hard ceiling vs. soft threshold).

- [ ] **Step 4: Run the gate**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid-core/src/compaction.rs
git commit -m "fix(agent): pre-request context trimming for small-context models

When a single turn accumulates more tool output than the model's context
window (e.g. reading several large files), the provider rejects the
request with a 400/404. A new plan_compactions_for_overflow force-compacts
the largest uncompacted tool results (best effort) before the provider
call, so the request fits. Normal compaction between sub-turns still
handles the common case; this is the safety net."
```

---

### Task 2: Lower default read limit

**Goal:** Reduce the `read` tool's default line limit from 2000 to 500.
Most files are under 500 lines. For large files, the model reads 500 lines
at a time and uses `offset` to page — 2-3 reads to understand structure,
then targeted reads for specific sections.

**Files:**
- Modify: `crates/zoid-tools/src/read.rs` (line 29: `DEFAULT_LIMIT`)

- [ ] **Step 1: Change the default**

```rust
const DEFAULT_LIMIT: usize = 500; // was 2000 — caps per-read context cost
```

Update the spec description at line 22:
```rust
"limit":  { "type": "integer", "description": "Max lines to return (default 500)." }
```

- [ ] **Step 2: Fix the test that asserts the old default**

The `over_cap_appends_truncation_notice` test uses 2100 lines expecting
2000 to be returned. With `DEFAULT_LIMIT = 500`, it needs 600 lines:

```rust
let body: String = (1..=600).map(|n| format!("line{n}\n")).collect();
```

And the assertion changes from `offset=2001` to `offset=501`:
```rust
assert!(out.text.contains("offset=501"));
```

- [ ] **Step 3: Run the gate**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/src/read.rs
git commit -m "perf(tools): lower read default limit 2000→500 lines

A 2000-line read of a large file produces ~10K tokens — a third of a
32K context window in a single tool call. 500 lines (~2.5K tokens) is
enough to understand a file's structure; the model pages with offset
for the rest. The 256KB byte cap remains as the hard ceiling."
```

---

## Self-Review

**Two independent fixes:**
1. Pre-request trimming (safety net for any tool) — Task 1
2. Lower read limit (reduces per-call output) — Task 2

**Productivity validation (option 2 concern):**
- 500 lines × ~50 chars/line = ~25KB ≈ ~2.5K tokens per read
- A 10K-line file takes 20 reads to fully page through — but in practice
  the model reads the first 500 (structure), then `grep`/`offset` for
  specifics. Capable models already work this way.
- The 256KB byte cap remains, so reads of files with very long lines are
  still capped at the byte level.

**Risk:**
- Task 1 is additive — a new safety net. If `plan_compactions_for_overflow`
  has a bug, it could over-compact, but the existing compaction tests guard
  the core. The new function is only called when context is already over
  the ceiling (a state that would fail anyway).
- Task 2 is a constant change. The only risk is productivity regression for
  models that relied on 2000-line reads. Watch for increased tool-call
  counts in real sessions.