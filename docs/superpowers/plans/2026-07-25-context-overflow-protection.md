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
test helpers — `ev(kind: EventKind) -> Event` and `big_tool_result(id: &str,
name: &str, lines: usize) -> Event` — both return a single `Event` (push, not
extend). `big_tool_result(n)` generates `n` lines of "match {i} in file\n"
(~16 chars each → ~5.3 tokens/line via chars/3 estimate):

```rust
#[test]
fn plan_compactions_for_overflow_compacts_largest_first() {
    // Three tool results: tc1 small, tc2 medium, tc3 large.
    // big_tool_result(n) ≈ n * 5.3 tokens.
    // tc1=10→53t, tc2=60→318t, tc3=200→1060t. Total ≈1431t.
    // Ceiling = 800. Must compact tc3 (largest) and tc2 (second). tc1 stays.
    let mut log = vec![
        ev(EventKind::ToolCall { id: "tc1".into(), name: "shell".into(), args: "{}".into() }),
        big_tool_result("tc1", "shell", 10),
        ev(EventKind::ToolCall { id: "tc2".into(), name: "shell".into(), args: "{}".into() }),
        big_tool_result("tc2", "shell", 60),
        ev(EventKind::ToolCall { id: "tc3".into(), name: "read".into(), args: "{}".into() }),
        big_tool_result("tc3", "read", 200),
    ];
    let plan = plan_compactions_for_overflow(
        log.iter(),
        800,   // hard ceiling (tokens)
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
    let log = vec![
        ev(EventKind::ToolCall { id: "tc1".into(), name: "shell".into(), args: "{}".into() }),
        big_tool_result("tc1", "shell", 5),
    ];
    let plan = plan_compactions_for_overflow(
        log.iter(),
        50000,  // ceiling way above
        &zoid_core::context::ContextOverhead::default(),
        None,
    );
    assert!(plan.compactions.is_empty());
}

#[test]
fn plan_compactions_for_overflow_skips_already_compacted() {
    let mut log = vec![
        ev(EventKind::ToolCall { id: "tc1".into(), name: "read".into(), args: "{}".into() }),
        big_tool_result("tc1", "read", 200),
        ev(EventKind::ToolResultCompacted {
            id: "tc1".into(),
            summary: "already compacted".into(),
            original_tokens: 1060,
        }),
    ];
    let plan = plan_compactions_for_overflow(
        log.iter(),
        100,   // very low ceiling — would normally compact
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
existing `plan_compactions` loop but uses a hard ceiling instead of a soft
threshold. It collects events into a `Vec` first (same as `plan_compactions`
line 62), uses `context_window_with` (which already folds overhead into
`total_tokens` — no separate overhead param), ports the File-item handling
from `plan_compactions` (lines 106-150), and uses real `compact_tool_output`:

```rust
/// Like `plan_compactions`, but driven by a hard ceiling (the model's
/// actual context window) rather than a soft threshold. Compacts the
/// LARGEST uncompacted tool results first, using real `compact_tool_output`
/// summaries, until the estimated total fits under `ceiling`. Best effort:
/// if even compacting everything doesn't fit, returns all available
/// compactions. Does NOT skip items based on the no-gain guard — when the
/// context is over the hard ceiling, even a small reduction helps (the
/// alternative is a provider 400 error).
pub fn plan_compactions_for_overflow<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    ceiling: u64,
    overhead: &crate::context::ContextOverhead,
    calibration_ratio: Option<f64>,
) -> CompactionPlan {
    // Collect once — we need multiple passes (window + lookup tables).
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let window = context_window_with(events.iter().copied(), overhead.clone());

    let current = match calibration_ratio {
        Some(ratio) if ratio > 0.0 => (window.total_tokens as f64 * ratio) as u64,
        _ => window.total_tokens,
    };
    if current <= ceiling {
        return CompactionPlan::default();
    }

    // Already-compacted tool-result ids.
    let done: HashSet<&str> = visible
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    // Latest non-error output per tool-result id.
    let mut output_of: HashMap<&str, &str> = HashMap::new();
    for e in visible {
        if let EventKind::ToolResult { id, output, is_error, .. } = &e.kind {
            if !*is_error {
                output_of.insert(id.as_str(), output.as_str());
            }
        }
    }

    // Map file paths → tool-result id and file paths → output (for File items
    // whose key is "file:{path}", not "tool:{name}:{id}"). Ported from
    // plan_compactions (lines 106-150).
    let mut path_id_of: HashMap<String, String> = HashMap::new();
    let mut path_output_of: HashMap<String, &str> = HashMap::new();
    {
        let mut call_path: HashMap<String, String> = HashMap::new();
        for e in visible {
            match &e.kind {
                EventKind::ToolCall { id, args, .. } => {
                    if let Some(p) = tool_path(args) {
                        call_path.insert(id.clone(), p);
                    }
                }
                EventKind::ToolResult { id, output, .. } => {
                    if let Some(p) = call_path.get(id) {
                        path_id_of.insert(p.clone(), id.clone());
                        path_output_of.insert(p.clone(), output.as_str());
                    }
                }
                _ => {}
            }
        }
    }

    // window.items is sorted tokens-desc, so iterating is largest-first.
    let mut running = current;
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
        // For File items, look up the tool call id via path_id_of.
        let (tool_call_id, output) = if it.kind == ItemKind::File {
            match path_id_of.get(id).map(|s| s.as_str()) {
                Some(tid) => (tid, path_output_of.get(id).copied()),
                None => continue,
            }
        } else {
            (id, output_of.get(id).copied())
        };
        if done.contains(tool_call_id) {
            continue; // already compacted
        }
        let Some(output) = output else {
            continue;
        };
        let summary = compact_tool_output(output, COMPACT_HEAD_LINES);
        let summary_tokens = estimate_tokens(&summary);
        // Even when summary_tokens >= it.tokens (no gain), compact when over
        // the hard ceiling — a small reduction is better than a provider 400.
        running = running.saturating_sub(it.tokens.saturating_sub(summary_tokens));
        out.push(Compaction {
            id: tool_call_id.to_string(),
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
2. At the call site (line 789), pass the model's context window:
```rust
        let model_ctx = zoid_provider::model::model_info(&model).context_window;
        preflight_gate(
            &session,
            &mut events,
            ui,
            config,
            session_id,
            now,
            &*calibration_ratio,
            &overhead_now,
            model_ctx,
        )
        .await?;
```
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
The `plan_compactions_for_overflow` call uses the same `overhead` reference that
`context_window_with` uses internally — no double-counting, since `context_window_with`
folds overhead into `total_tokens` as a System item.

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

**Gilfoyle review (2nd pass) issues resolved:**
- C1 (test helper signatures): Fixed — `ev(kind: EventKind)` and `big_tool_result(id, name, lines)` called correctly. Tests use `vec![...]` with `push` semantics, not `extend`.
- C2 (`log.extend` on single Event): Fixed — tests use `vec![ev(...), big_tool_result(...), ...]` inline.
- C3 (iterator consumed twice): Fixed — `events` collected into `Vec<&Event>` first (line 1 of the function body), then `.iter().copied()` passed to `context_window_with` (same pattern as `plan_compactions` line 62-64).
- H1 (double-counting overhead): Fixed — removed the `overhead_tokens: u64` parameter. `context_window_with` already folds overhead into `total_tokens`. The call site passes `overhead` directly, no extra addition.
- H2 (File items silently skipped): Fixed — ported the full File-item handling (`path_id_of` and `path_output_of` maps) from `plan_compactions` (lines 106-150). File items now resolve their tool-call id via the path → id map.
- M1 (error results not filtered): Fixed — `output_of` map uses `if !*is_error` guard, matching the existing `plan_compactions` (line 99).
- M2 (test token estimates wrong): Fixed — test comments now use correct `big_tool_result(n) ≈ n * 5.3` math, and the ceiling/overhead values are calibrated accordingly.

**Task 2 is unchanged** (gilfoyle approved it as-is).