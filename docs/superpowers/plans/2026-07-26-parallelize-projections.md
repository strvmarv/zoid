# Parallelize ProjectionCache::refresh — Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Parallelize the 5 sequential O(n) projection passes in `ProjectionCache::refresh` using `std::thread::scope` (full 5-way parallelism, zero new dependencies).

**Spec:** `docs/superpowers/specs/2026-07-26-parallelize-projections-design.md`

**Tech Stack:** Rust standard library (`std::thread::scope`, stable since 1.63). No new crate dependencies.

## Global Constraints

- No coverage reduction. All existing tests must pass.
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- No co-author trailer in commits (repo `AGENTS.md`).

---

## File Structure

| File | Change |
|---|---|
| `crates/zoid/src/main.rs` | Parallelize `ProjectionCache::refresh` with `std::thread::scope` |

No `Cargo.toml` changes — `std::thread::scope` is in the standard library.

---

### Task 1: Parallelize refresh with std::thread::scope

- [ ] **Step 1: Parallelize ProjectionCache::refresh**

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

With `std::thread::scope` (full 5-way parallelism):
```rust
let iter = events.iter();
std::thread::scope(|s| {
    let a = s.spawn(|| conversation(iter.clone()));
    let b = s.spawn(|| zoid_core::context::context_window(iter.clone()));
    let c = s.spawn(|| zoid_core::economy::churn_timeline(iter.clone()));
    let d = s.spawn(|| zoid_core::tasks::tasks(iter.clone()));
    let e = s.spawn(|| zoid_core::economy::token_ledger(iter));
    self.msgs = a.join().unwrap();
    self.window = b.join().unwrap();
    self.churn = c.join().unwrap();
    self.tasks = d.join().unwrap();
    let ledger = e.join().unwrap();
    self.ledger_total = ledger.total;
    self.cached_total = ledger.cached;
});
```

Keep the 2 reverse scans (`last_input_tokens`, `last_output_tokens`) sequential — they early-exit and are cheap.

- [ ] **Step 2: Smoke-test the default build**

```bash
cargo build
```

Must exit 0. If it fails, stop — do not proceed to Step 3. A default-features
build failure means the parallelization broke the lean build, which is the
exact regression this step exists to catch.

Confirms the code compiles with default features (no `local-embed`). Since this
plan adds no new dependencies, this also verifies the default binary's
dependency footprint is unchanged.

- [ ] **Step 3: Run the gate**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

- [ ] **Step 4: Commit**

```bash
git commit -m "perf(proj): parallelize ProjectionCache::refresh with std::thread::scope

5 independent O(n) passes (conversation, context_window, churn_timeline,
tasks, token_ledger) now run concurrently as scoped threads. Wall-clock
from sum(passes) to max(passes) — full 5-way parallelism with zero new
dependencies (std::thread::scope, stable since Rust 1.63)."
```