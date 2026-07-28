# Parallelize ProjectionCache::refresh — Design

> **Status:** DESIGN (brainstormed 2026-07-26). Ready for `writing-plans`.

---

## 1. Goal

Parallelize the 5 sequential O(n) projection passes in `ProjectionCache::refresh`
using `std::thread::scope` (stable since Rust 1.63, zero new dependencies). Cut
session-resume wall-clock from ~sum(5 passes) to ~max(1 slowest pass) — full
5-way parallelism.

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

**Why this compiles:** `std::thread::scope` requires spawned closures to be
`Send`. The chain: `Event: Sync` (all fields are `Sync` — `Ulid`, `String`,
`i64`, `Option<TokenStat>`, and every `EventKind` variant; no `Rc`/`Cell`/
`RefCell`/`thread_local` anywhere in the projection modules) ⟹ `&Event: Send`
⟹ the cloned iterator (yielding `&Event`) is `Send` ⟹ each spawned closure is
`Send`. The return types (`Vec<ChatMsg>`, `ContextWindow`, `ChurnTimeline`,
`Vec<TaskItem>`, `TokenLedger`) are all `Send` (owned values of `Sync` types).
The outer `scope` closure captures `&mut self` but `thread::scope` places no
`Send` bound on it — only `Scope::spawn` does. So `self` is touched only on
the calling thread after `join().unwrap()`, never inside a spawned closure.

⚠ A future field addition to `Event` that breaks `Sync` (e.g., an `Rc<...>`)
would turn this sound parallelization into a compile error. That's the right
failure mode — loud at compile time, not a silent data race.

Replace with `std::thread::scope`, spawning all 5 passes as scoped threads:

```rust
let iter = events.iter();
std::thread::scope(|s| {
    let a = s.spawn(|| conversation(iter.clone()));
    let b = s.spawn(|| zoid_core::context::context_window(iter.clone()));
    let c = s.spawn(|| zoid_core::economy::churn_timeline(iter.clone()));
    let d = s.spawn(|| zoid_core::tasks::tasks(iter.clone()));
    let e = s.spawn(|| zoid_core::economy::token_ledger(iter.clone()));
    self.msgs = a.join().unwrap();
    self.window = b.join().unwrap();
    self.churn = c.join().unwrap();
    self.tasks = d.join().unwrap();
    let ledger = e.join().unwrap();
    self.ledger_total = ledger.total;
    self.cached_total = ledger.cached;
});
```

The 2 reverse scans (`last_input_tokens`, `last_output_tokens`) stay
sequential — they early-exit on the first match and are cheap.

## 3. Dependency

**None.** `std::thread::scope` is in the standard library (stable since Rust
1.63; the workspace is edition 2021). No new crate dependencies, no `Cargo.toml`
changes. This is the decisive advantage over `rayon`: the release binary is
size-optimized (`opt-level = "z"`, `lto = true`, `codegen-units = 1`, `strip =
true`), and the workspace deliberately keeps the default build lean — the
`local-embed` feature is default-off specifically to avoid pulling
candle/rayon into the default binary (see `crates/zoid/Cargo.toml` comment).
`std::thread::scope` adds zero binary-size cost and zero new transitive
dependencies.

## 4. Expected impact

All 5 passes run concurrently as scoped threads:
```
scope( A ‖ B ‖ C ‖ D ‖ E )   // full 5-way parallelism
```

Wall-clock: `max(A, B, C, D, E)` — a single max, not the two sequential
maxes that a 3+2 `rayon::join` grouping would produce (`max(A,max(B,C)) +
max(D,E)`). This is strictly better than the 3+2 grouping: if `conversation`
and `tasks` are both expensive `Vec`-building passes, the 3+2 split's additive
second max could approach `2 × conversation`; the 5-way `thread::scope` avoids
that entirely.

**Tradeoff:** `thread::scope` spawns 5 OS threads per `refresh` call (no thread
pool reuse). For session-resume — which fires a handful of times per session,
not in a hot loop — the ~µs-per-thread spawn overhead is noise relative to the
folds themselves. Rayon's work-stealing pool would amortize spawn cost across
calls, but the pool-init cost (~ms, one-time) and the permanent default-build
dependency footprint are not worth it for this one-shot fork-join.

## 5. panic = "abort" interaction

The release profile uses `panic = "abort"` (`Cargo.toml` `[profile.release]`).
`std::thread::scope` has no pool-level panic-isolation machinery of its own —
it relies on the standard per-thread `catch_unwind` wrapper that `thread::spawn`
already applies, with panics propagating via `JoinHandle::join()` returning
`Err` (re-raised by `.unwrap()`) or via the scope's auto-join panic. There is no
rayon-style work-stealing pool with its own `catch_unwind` job executor. The
behavior is straightforward:

- **Release (`panic = "abort"`):** a panic in any spawned thread aborts the
  process at the panic site. Observably identical to the sequential code,
  which also aborts on panic. No regression.
- **Dev/test (`panic = "unwind"`, the Rust default):** `JoinHandle::join()`
  returns `Err(Box<dyn Any>)` if the thread panicked; `.unwrap()` then
  re-raises the panic on the joining (calling) thread. So a panic in a
  projection during a test propagates to the test thread and fails the test
  normally — no masking, no reordering.

Either way, the observable behavior matches the sequential code. No action
needed.

## 6. Testing

No new tests needed — the existing tests verify projection correctness.
The parallelization doesn't change results, only computation order.

## 7. Out of scope

- Lazy-loading the body cache (separate spec)
- Peeking removal/rework (separate discussion)
- Parallelizing the 2 reverse scans (not worth it — early-exit)
- `rayon` as a direct dependency (the `std::thread::scope` approach adds zero
  new deps; rayon's work-stealing pool is overkill for a one-shot 5-way
  fork-join on session-resume, and would add a permanent default-build
  footprint the workspace deliberately avoids)
- A pooled `thread::scope` approach (e.g., a `OnceLock`-backed worker pool) —
  would reinvent rayon; the per-call spawn cost is negligible at
  session-resume frequency