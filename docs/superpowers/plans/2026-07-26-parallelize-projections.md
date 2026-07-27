# Parallelize ProjectionCache::refresh — Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Parallelize the 5 sequential O(n) projection passes in `ProjectionCache::refresh` using `rayon::join`.

**Spec:** `docs/superpowers/specs/2026-07-26-parallelize-projections-design.md`

**Tech Stack:** Rust, `rayon` crate (new dependency).

## Global Constraints

- No coverage reduction. All existing tests must pass.
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- No co-author trailer in commits (repo `AGENTS.md`).

---

## File Structure

| File | Change |
|---|---|
| `crates/zoid/Cargo.toml` | Add `rayon = "1"` dependency |
| `crates/zoid/src/main.rs` | Parallelize `ProjectionCache::refresh` |

---

### Task 1: Add rayon dependency + parallelize refresh

- [ ] **Step 1: Add rayon to Cargo.toml**

In `crates/zoid/Cargo.toml`, add to `[dependencies]`:
```toml
rayon = "1"
```

- [ ] **Step 2: Parallelize ProjectionCache::refresh**

In `crates/zoid/src/main.rs`, find `fn refresh` in `impl ProjectionCache` (line ~1434).

Replace the 5 sequential passes:
```rust
self.msgs = conversation(events.iter());
self.window = zoid_core::context::context_window(events.iter());
self.churn = zoid_core::economy::churn_timeline(events.iter());
self.tasks = zoid_core::tasks::tasks(events.iter());
let ledger = zoid_core::economy::token_ledger(events.iter());
self.ledger_total = ledger.total;
self.cached_total = ledger.cached;
```

With nested `rayon::join`:
```rust
let iter = events.iter();
let (msgs, (window, churn)) = rayon::join(
    || conversation(iter.clone()),
    || rayon::join(
        || zoid_core::context::context_window(iter.clone()),
        || zoid_core::economy::churn_timeline(iter.clone()),
    ),
);
let (tasks, ledger) = rayon::join(
    || zoid_core::tasks::tasks(iter.clone()),
    || zoid_core::economy::token_ledger(iter),
);
self.msgs = msgs;
self.window = window;
self.churn = churn;
self.tasks = tasks;
self.ledger_total = ledger.total;
self.cached_total = ledger.cached;
```

Keep the 2 reverse scans (`last_input_tokens`, `last_output_tokens`) sequential — they early-exit and are cheap.

- [ ] **Step 3: Run the gate**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

- [ ] **Step 4: Commit**

```bash
git commit -m "perf(proj): parallelize ProjectionCache::refresh with rayon::join

5 independent O(n) passes (conversation, context_window, churn_timeline,
tasks, token_ledger) now run in parallel via nested rayon::join (3+2
grouping). Wall-clock from sum(passes) to max(A,max(B,C)) + max(D,E)."
```