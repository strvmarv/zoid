# Context Overflow Protection — Implementation Plan (Revised)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent local models with small context windows (32K) from hitting
context-overflow errors when a single turn accumulates too much tool output.
Two independent fixes: (1) pre-request context trimming as a safety net,
and (2) a lower default read limit to reduce per-call output size.

**Architecture:** Two tasks, independently shippable.
- Task 1: Add a hard-ceiling compaction pass inside `preflight_gate` — after
  the existing soft-threshold compaction/eviction, if the estimated tokens
  still exceed the model's context window, force-compact the largest
  uncompacted tool results until it fits. Uses `compact_tool_output` for
  real summary computation (not a heuristic).
- Task 2: Lower the `read` tool's default limit from 2000 to 500 lines.

**Tech Stack:** Rust (`zoid-core`, `zoid`, `zoid-tools` crates). No new deps.

## Global Constraints

- **No coverage reduction.** All existing tests must pass.
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid/src/agent.rs` | Pass model context window to `preflight_gate`; add hard-ceiling pass | Modify (Task 1) |
| `crates/zoid-core/src/compaction.rs` | `plan_compactions_for_overflow` — force-compact largest results | Add (Task 1) |
| `crates/zoid-tools/src/read.rs` | `DEFAULT_LIMIT` 2000 → 500 | Modify (Task 2) |

---

### Task 1: Hard-ceiling compaction pass in `preflight_gate`

**Goal:** `preflight_gate` already runs compaction against a soft threshold
(`band.high_water`). Add a final hard-ceiling check: if the estimate still
exceeds the model's actual context window after the soft pass, force-compact
the largest uncompacted tool results (using real `compact_tool_output` summaries)
until it fits or no candidates remain (best effort).

**Files:**
- Modify: `crates/zoid-core/src/compaction.rs` (new `plan_compactions_for_overflow`)
- Modify: `crates/zoid/src/agent.rs` (`preflight_gate` signature + call site)

- [ ] **Step 1: Write tests for `plan_compactions_for_overflow`**

Add to `crates/zoid-core/src/compaction.rs` test module. Use the existing
test helpers (`ev`, `big_tool_result`, `policy`) that are already defined:

```rust
#[test]
fn plan_compactions_for_overflow_compacts_largest_first() {
    // Three tool results: small, medium, large. Context window ceiling
    // is set so that only the two largest must be compacted.
    let mut log = Vec::new();
    log.extend(ev("tc1", "shell", big_tool_result(50)));   // ~17 tokens
    log.extend(ev("tc2", "shell", big_tool_result(300)));  // ~100 tokens
    log.extend(ev("tc3", "read", big_tool_result(900)));   // ~300 tokens
    // Total ~417 + overhead ~100 = ~517. Ceiling = 350.
    // Must compact tc3 (largest) and tc2 (second). tc1 stays.
    let plan = plan_compactions_for_overflow(
        log.iter(),
        350,   // hard ceiling (tokens)
        100,   // overhead tokens
        &zoid_core::context::ContextOverhead::default(),
        None,  // no calibration
    );
    let ids: Vec<&str> = plan.compactions.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"tc3"), "largest must be compacted: {ids:?}");
    assert!(ids.contains(&"tc2"), "second largest must be compacted: {ids:?}");
    assert!(!ids.contains(&"tc1"), "smallest should not be compacted: {ids:?}");
    // Each compaction must have a real summary (not empty).
    for c in &plan.compactions {
        assert!(!c.summary.is_empty(), "summary must be computed");
        assert!(c.original_tokens > 0, "original_tokens must be set");
    }
}

#[test]
fn plan_compactions_for_overflow_noop_when_under_ceiling() {
    let mut log = Vec::new();
    log.extend(ev("tc1", "shell", "hello world"));
    let plan = plan_compactions_for_overflow(
        log.iter(),
        50000,  // ceiling way above
        100,
        &zoid_core::context::ContextOverhead::default(),
        None,
    );
    assert!(plan.compactions.is_empty());
}

#[test]
fn plan_compactions_for_overflow_skips_already_compacted() {
    let mut log = Vec::new();
    log.extend(ev("tc1", "shell", big_tool_result(900)));
    // Mark tc1 as already compacted by adding a ToolResultCompacted event.
    log.push(zoid_core::event::Event::new(
        ulid::Ulid::new(), None, 200,
        zoid_core::event::EventKind::ToolResultCompacted {
            id: "tc1".into(),
            summary: "already compacted".into(),
            original_tokens: 300,
        },
    ));
    let plan = plan_compactions_for_overflow(
        log.iter(),
        100,   // very low ceiling — would normally compact
        50,
        &zoid_core::context::ContextOverhead::default(),
        None,
    );
    // tc1 is already compacted — no candidates remain.
    assert!(plan.compactions.is_empty(), "already-compacted should be skipped");
}
```

Run: `cargo test -p zoid-core --lib compaction::tests::plan_compactions_for_overflow`
Expected: FAIL (function doesn't exist).

- [ ] **Step 2: Implement `plan_compactions_for_overflow`**

Add to `crates/zoid-core/src/compaction.rs`. This function mirrors the
existing `plan_compactions` loop (lines 154-210) but uses a hard ceiling
instead of a soft threshold, and always uses `compact_tool_output` for
real summary computation:

```rust
/// Like `plan_compactions`, but driven by a hard ceiling (the model's
/// actual context window) rather than a soft threshold. Compacts the
/// LARGEST uncompacted tool results first, using real `compact_tool_output`
/// summaries, until the estimated total fits under `ceiling + overhead`.
/// Best effort: if even compacting everything doesn't fit, returns all
/// available compactions. Does NOT skip items based on the no-gain guard —
/// when the context is over the hard ceiling, even a small reduction helps
/// (the alternative is a provider 400 error).
pub fn plan_compactions_for_overflow<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    ceiling: u64,
    overhead_tokens: u64,
    overhead: &crate::context::ContextOverhead,
    calibration_ratio: Option<f64>,
) -> CompactionPlan {
    let window = crate::context::context_window_with(events, overhead.clone());
    let total = if let Some(r) = calibration_ratio {
        if r > 0.0 {
            (window.total_tokens as f64 * r) as u64
        } else {
            window.total_tokens
        }
    } else {
        window.total_tokens
    } + overhead_tokens;
    if total <= ceiling {
        return CompactionPlan::default();
    }

    // Build the same lookup tables as plan_compactions: tool-call-id → output,
    // already-compacted set, path → output for File items.
    let visible: Vec<&Event> = events.into_iter().collect();
    let mut output_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &visible {
        match &e.kind {
            EventKind::ToolResult { id, output, .. } => {
                output_of.insert(id.as_str(), output.as_str());
            }
            EventKind::ToolResultCompacted { id, .. } => {
                done.insert(id.clone());
            }
            _ => {}
        }
    }

    // window.items is sorted tokens-desc, so iterating is largest-first.
    let mut running = total;
    let mut out: Vec<Compaction> = Vec::new();
    for it in &window.items {
        if running <= ceiling {
            break;
        }
        if (it.kind != ItemKind::ToolResult && it.kind != ItemKind::File) || it.pinned {
            continue;
        }
        let Some(id) = tool_id_of(&it.key) else {
            continue;
        };
        if done.contains(id) {
            continue; // already compacted
        }
        let Some(output) = output_of.get(id) else {
            continue;
        };
        let summary = compact_tool_output(output, COMPACT_HEAD_LINES);
        let summary_tokens = estimate_tokens(&summary);
        // Even when summary_tokens >= it.tokens (no gain), compact when over
        // the hard ceiling — the summary replaces the output in the projection,
        // and a small reduction is better than a provider 400.
        running = running.saturating_sub(it.tokens.saturating_sub(summary_tokens));
        out.push(Compaction {
            id: id.to_string(),
            summary,
            original_tokens: it.tokens,
        });
    }
    CompactionPlan { compactions: out }
}
```

Run: `cargo test -p zoid-core --lib compaction::tests::plan_compactions_for_overflow`
Expected: PASS.

- [ ] **Step 3: Wire into `preflight_gate`**

In `crates/zoid/src/agent.rs`:

1. Add `model_context_window: u64` parameter to `preflight_gate` (line 2766).
2. At the call site (line 789), pass the model's context window from
   `zoid_provider::model::model_info(&model).context_window`.
3. At the END of `preflight_gate` (after the existing compaction + eviction
   passes), add the hard-ceiling check:

```rust
    // Hard-ceiling safety net: if the estimate still exceeds the model's
    // actual context window, force-compact the largest uncompacted tool
    // results. This catches the case where a single tool result (e.g.
    // reading a 10K-line file) pushes context past the limit in one sub-turn,
    // bypassing the soft-threshold compaction above.
    est = estimate(events);
    if est > model_context_window && model_context_window > 0 {
        let plan = zoid_core::compaction::plan_compactions_for_overflow(
            events.iter(),
            model_context_window,
            overhead.system_tokens,
            overhead,
            *calibration_ratio,
        );
        let compacted = !plan.compactions.is_empty();
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionStarted).await;
        }
        for c in &plan.compactions {
            emit(
                session,
                events,
                ui,
                &config.branch,
                EventKind::ToolResultCompacted {
                    id: c.id.clone(),
                    summary: c.summary.clone(),
                    original_tokens: c.original_tokens,
                },
                session_id,
                now,
            )
            .await?;
        }
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionComplete).await;
        }
    }
```

This reuses the same emit pattern as the existing compaction pass (lines 2816-2833).
The `estimate` closure (line 2782) already accounts for calibration + overhead.

- [ ] **Step 4: Run the gate**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid-core/src/compaction.rs
git commit -m "fix(agent): hard-ceiling compaction pass in preflight_gate

When a single turn accumulates more tool output than the model's
context window (e.g. reading several large files), the provider rejects
the request with a 400. The existing soft-threshold compaction may not
fire in time if a single tool result jumps the context past both the
threshold and the ceiling. A new plan_compactions_for_overflow force-
compacts the largest uncompacted tool results (using real
compact_tool_output summaries) after the soft pass, so the request
fits. Best effort — if even compacting everything doesn't fit, the
provider will still reject, but at least we tried."
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

In `crates/zoid-tools/src/read.rs`:

Line 29: `const DEFAULT_LIMIT: usize = 500;` (was 2000)

Line 22 (spec description):
```rust
"limit":  { "type": "integer", "description": "Max lines to return (default 500)." }
```

- [ ] **Step 2: Fix the test that asserts the old default**

The `over_cap_appends_truncation_notice` test (line 184) uses 2100 lines
expecting 2000 to be returned. With `DEFAULT_LIMIT = 500`, it needs 600
lines and a different offset assertion:

```rust
let body: String = (1..=600).map(|n| format!("line{n}\n")).collect();
```

And change the offset assertion from `offset=2001` to `offset=501`:
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

**Gilfoyle review issues resolved:**
- C1 (Compaction struct fields): Fixed — uses `id`, `summary`, `original_tokens` matching the actual struct.
- C2 (ItemKind::ToolResult is unit variant): Fixed — uses `tool_id_of(&it.key)` like the existing `plan_compactions`.
- C3 (test helpers don't exist): Fixed — reuses the existing `ev` and `big_tool_result` helpers already in the test module.
- C4 (token_ceiling is None for main chat): Fixed — the ceiling comes from `model_info(&model).context_window` passed as a parameter, not from `policy.token_ceiling`.
- H1 (Step 3 is prose-only): Fixed — Step 3 now has a concrete code block for the hard-ceiling pass.
- H2 (should fold into preflight_gate): Fixed — the hard-ceiling check goes at the end of `preflight_gate`, reusing the same emit pattern.
- M1 (calibration_ratio is dead): Fixed — wired into the total estimate using the same pattern as the `estimate` closure.
- M2 (overhead model inconsistent): Fixed — uses `ContextOverhead` type and `context_window_with`, matching the existing code.
- L1 (90% savings heuristic): Fixed — uses real `compact_tool_output` for summary computation, like the existing `plan_compactions`.

**Task 2 is unchanged** (gilfoyle approved it as-is).