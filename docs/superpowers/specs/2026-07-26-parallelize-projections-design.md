# Parallelize ProjectionCache::refresh — Design

> **Status:** DESIGN (brainstormed 2026-07-26). Ready for `writing-plans`.

---

## 1. Goal

Parallelize the 5 sequential O(n) projection passes in `ProjectionCache::refresh`
using `rayon::join`. Cut session-resume wall-clock from ~sum(5 passes) to
~max(2 slowest passes).

## 2. What changes

`ProjectionCache::refresh` (`crates/zoid/src/main.rs:1434`) currently runs 5
independent O(n) passes sequentially over the full event log:

1. `conversation` → `Vec<ChatMsg>`
2. `context_window` → `ContextWindow`
3. `churn_timeline` → `ChurnTimeline`
4. `tasks` → `Vec<TaskItem>`
5. `token_ledger` → `(total, cached)`

All 5 take `events.iter()` (a shared borrow — the iterator is `Clone`).
None depend on each other's output. Perfect for fork-join parallelism.

Replace with nested `rayon::join` (2+2 grouping for 4-way parallelism):

```rust
let (msgs, (window, churn)) = rayon::join(
    || conversation(iter1),
    || rayon::join(
        || context_window(iter2),
        || churn_timeline(iter3),
    ),
);
let (tasks, ledger) = rayon::join(
    || tasks(iter4),
    || token_ledger(iter5),
);
```

The 2 reverse scans (`last_input_tokens`, `last_output_tokens`) stay
sequential — they early-exit on the first match and are cheap.

## 3. Dependency

Add `rayon = "1"` to `crates/zoid/Cargo.toml`.

## 4. Expected impact

Wall-clock: ~sum(5 passes) → ~max(2 slowest passes). The `conversation` pass
is likely the most expensive (builds `Vec<ChatMsg>` from all events), so the
speedup is roughly 2-3x, not 5x (Amdahl's law).

## 5. Testing

No new tests needed — the existing tests verify projection correctness.
The parallelization doesn't change results, only computation order.

## 6. Out of scope

- Lazy-loading the body cache (separate spec)
- Peeking removal/rework (separate discussion)
- Parallelizing the 2 reverse scans (not worth it — early-exit)