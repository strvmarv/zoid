# Test-Suite Performance — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce the wall time of `cargo test --workspace --features
zoid/local-embed` — the agent-facing release gate — without reducing
coverage or touching the release/dist profiles.

**Architecture:** Five phases, strictly sequential. Phase 0 installs
nextest for per-test timing. Phase 1 re-baselines against the correct
gate. Phase 2 applies the opt-level profile override + cargo clean.
Phase 3 shrinks oversized fixtures. Phase 4 adopts nextest as the
documented gate. Each phase ends with a measurement; no phase begins
before the previous one's numbers are recorded.

**Tech Stack:** Rust workspace, `cargo`, `cargo-nextest`. No new crate
dependencies. One `Cargo.toml` edit, one `AGENTS.md` edit, one test-file
edit.

**Spec:** `docs/superpowers/specs/2026-07-25-test-suite-performance-design.md`

## Global Constraints

- **No coverage reduction.** Every test function before and after. No
  test deleted, `#[ignore]`-ed, or weakened.
- **No release/dist profile changes.** `[profile.release]` and
  `[profile.release.package.zoid-embed]` are load-bearing for the
  static-musl target and must not be disturbed.
- **`cargo test --workspace` must still pass** after every phase (§4.2
  compatibility requirement).
- **Sequential measurements only.** Never run cargo invocations in
  parallel — on 6 cores, concurrent runs skew every number (§6).
- **Revert on regression.** Any phase whose measurement shows a net
  regression is reverted, not kept (§10).
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `Cargo.toml` | `[profile.test.package.zoid-core] opt-level = 1` | Modify (Phase 2) |
| `AGENTS.md` | Gate command updated to `cargo nextest run` | Modify (Phase 4) |
| `crates/zoid/tests/economy_integration.rs` | `seq 1 2000` → `seq 1 100` | Modify (Phase 3) |

---

## Phase 0 — Install nextest, change nothing else

**Goal:** Install `cargo-nextest` for per-test timing. No repo change.

- [ ] **Step 1: Install nextest**

```bash
cargo install cargo-nextest
cargo nextest --version
```

Expected: a version string. If the install fails, STOP and report —
Phase 0 is a prerequisite for all subsequent measurement.

- [ ] **Step 2: Verify nextest runs the suite**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

Expected: all tests pass (same as `cargo test`). This is a verification
run — nextest runs each test in its own process, so a failure here
indicates a latent test-isolation bug (§4.2 risk), not a nextest defect.
**Investigate any failure as a test bug, not a nextest bug.**

- [ ] **Step 3: Capture per-test timing**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

Record the output. The slowest tests (§2.2: `economy_integration`,
`inline_question_card`, `tasks_tool`, `ask_user`) should have per-test
timings that close §2.4's attribution gap — which crates hold the hot
code for each.

---

## Phase 1 — Re-baseline

**Goal:** Record the true baseline against the correct gate
(`--features zoid/local-embed`). The §2 numbers were measured without
the feature and are wrong.

- [ ] **Step 1: Warm freshness check**

```bash
cargo test --workspace --features zoid/local-embed --no-run
```

Record: wall time.

- [ ] **Step 2: Incremental rebuild**

```bash
touch crates/zoid-core/src/lib.rs
/usr/bin/time -f "REBUILD %e s" \
  cargo test --workspace --features zoid/local-embed --no-run
```

Record: rebuild seconds. This is the number to watch for the §7
trigger (90s guard).

- [ ] **Step 3a: Execution — cargo test**

```bash
/usr/bin/time -f "EXEC_cargo %e s" \
  cargo test --workspace --features zoid/local-embed --no-fail-fast
```

Record: execution seconds. This is the like-for-like comparison to
the §2 baseline (which was 132.9s without `--features`).

- [ ] **Step 3b: Execution — nextest**

```bash
/usr/bin/time -f "EXEC_nextest %e s" \
  cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

Record: execution seconds. The gap between 3a and 3b is nextest's
parallelism contribution.

- [ ] **Step 4: Record all four numbers**

No commit. Write the four numbers down — they are the baseline every
subsequent phase is measured against.

---

## Phase 2 — Profile override + clean

**Goal:** Apply `[profile.test.package.zoid-core] opt-level = 1` and
`cargo clean` at the moment it's free (the profile change invalidates
all test artifacts anyway).

- [ ] **Step 1: Add the profile override**

In `Cargo.toml`, after the `[profile.release.package.zoid-embed]`
section (around line 59), add:

```toml
[profile.test.package.zoid-core]
opt-level = 1
```

This is the established house idiom — the root manifest already carries
`[profile.release.package.zoid-embed] opt-level = 3` for the same
surgical pattern. `zoid-core` is the leaf crate containing the hot
compaction code (§2.3).

- [ ] **Step 2: Clean (free — artifacts are invalidated by the profile change)**

```bash
cargo clean
```

- [ ] **Step 3: Measure — warm freshness, rebuild, execution (cargo + nextest)**

Run the full measurement protocol (§6):

```bash
# 1. warm freshness
cargo test --workspace --features zoid/local-embed --no-run

# 2. incremental rebuild
touch crates/zoid-core/src/lib.rs
/usr/bin/time -f "REBUILD %e s" \
  cargo test --workspace --features zoid/local-embed --no-run

# 3a. execution — cargo test
/usr/bin/time -f "EXEC_cargo %e s" \
  cargo test --workspace --features zoid/local-embed --no-fail-fast

# 3b. execution — nextest
/usr/bin/time -f "EXEC_nextest %e s" \
  cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

Record all four numbers.

- [ ] **Step 4: Check the 90s guard**

If incremental rebuild (Step 3, number 2) exceeds **90s**, STOP — do not
widen the package list. Trigger the §7 contingency (binary consolidation)
instead. If rebuild is ≤ 90s, continue to Step 5.

- [ ] **Step 5: Escalate — widen the package list or try opt-level = 2**

If execution speedup is insufficient (a remaining hot binary is still
slow, per nextest per-test timing), there are two escalation axes:

**Axis A — widen packages.** Add one more package override — likely
`zoid-tui` or `zoid`. Add ONE, re-measure (Step 3), check the 90s guard
(Step 4), repeat.

```toml
# Example escalation — only if measurement shows a remaining hot binary:
[profile.test.package.zoid-tui]
opt-level = 1
```

**Axis B — try opt-level = 2 on the existing package.** If widening
packages pushes rebuild past the 90s guard, try increasing the opt-level
on `zoid-core` from 1 to 2 instead. This may improve execution further
without adding rebuild cost for other crates.

```toml
[profile.test.package.zoid-core]
opt-level = 2  # try if opt-level = 1 underdelivers
```

Re-measure after each change. The guard (Step 4) gates every escalation.

- [ ] **Step 6: Spot-check debug-info fidelity**

Run a deliberately failing test and verify the panic backtrace reports
a usable line number (§4.1 risk):

```bash
# Temporarily break a test, run it, check the backtrace, then revert.
# Or: find an existing test that panics on a specific line and verify
# the location is still correct.
```

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml
git commit -m "perf(test): opt-level=1 for zoid-core in test profile

Surgical override mirroring the release profile's zoid-embed pattern.
The hot compaction code in zoid-core runs 2.5x faster under opt-level=1;
other crates stay at the default (no rebuild cost for them). Baseline
and post-change measurements recorded."
```

---

## Phase 3 — Shrink fixtures

**Goal:** Change `seq 1 2000` to `seq 1 100` in
`economy_integration.rs`. 100 lines is provably sufficient (§4.3 —
1333 tokens still ~27x the 50-token threshold, and the summary is
unambiguously smaller than the original).

**Note:** The spec says "three `seq 1 2000` fixtures" but there are
only **two** occurrences in the source (lines 70 and 129). The third
slow test (`compaction_does_not_emit_updates_when_nothing_compacted`,
22.61s) uses `echo hi` — its slowness comes from the compaction code
processing the event log, not from a large fixture. Shrinking the
two `seq 1 2000` fixtures affects the two tests that have them; the
third test's speedup comes from Phase 2's opt-level override.

- [ ] **Step 1: Change the fixtures**

In `crates/zoid/tests/economy_integration.rs`, change both occurrences of:

```
"for i in $(seq 1 2000); do echo \"line $i: filler text to pad out tokens\"; done"
```

to:

```
"for i in $(seq 1 100); do echo \"line $i: filler text to pad out tokens\"; done"
```

There are exactly two occurrences: lines 70 and 129.

- [ ] **Step 2: Run the affected tests**

```bash
cargo test -p zoid --test economy_integration -- --nocapture
```

Expected: all tests pass, including:
- `oversized_tool_result_is_compacted_when_over_threshold` — `assert!(compacted, ...)` still holds
- `compaction_emits_started_and_complete_updates` — all assertions still hold
- `compaction_does_not_emit_updates_when_nothing_compacted` — unaffected (uses `echo hi`)

If any assertion fails, raise N from 100 until it passes. **Do not weaken
assertions.** Record the final N.

- [ ] **Step 3: Measure — full suite**

Run the full measurement protocol (§6, all four numbers).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/tests/economy_integration.rs
git commit -m "perf(test): shrink economy_integration fixtures 2000→100 lines

The 50-token compaction threshold is crossed at ~100 lines (1333
tokens, ~27x the threshold). 2000 lines was a ~530x overshoot. Same
assertions, same coverage, ~20x less compute in the two affected
tests."
```

---

## Phase 4 — Adopt nextest as the gate

**Goal:** Update `AGENTS.md:46` to use `cargo nextest run` as the
documented release gate. This retires the piped-exit-code hazard (§4.2)
and gains nextest's per-test parallelism.

- [ ] **Step 1: Update AGENTS.md**

At `AGENTS.md:46`, change:

```
4. Verify the release gate: `cargo test --workspace --features zoid/local-embed`
   (dist bakes in `local-embed`). Never trust a piped exit code; use
   `--no-fail-fast` so one failing test binary can't hide others.
```

to:

```
4. Verify the release gate: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast`
   (dist bakes in `local-embed`). Nextest emits a single reliable exit code
   (no piped-exit-code hazard) and parallelizes at the test level across all
   binaries. Use `--no-fail-fast` so one failing test doesn't hide others.
   `cargo test --workspace --no-fail-fast` still works as a fallback.
```

- [ ] **Step 2: Verify both runners pass**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
cargo test --workspace --features zoid/local-embed --no-fail-fast
```

Expected: both pass, same test count (1577), zero failures, same ignored count (2).

- [ ] **Step 3: Verify release builds undisturbed**

```bash
cargo build --release
```

Expected: success. The `test` profile change does not affect `release`.

If the static-musl release target is configured (check `dist-workspace.toml`),
also verify it builds — the `test` profile override must not bleed into
the `release` profile. This is the §10 acceptance criterion.

- [ ] **Step 4: Measure — final numbers**

Run the full measurement protocol (§6, all four numbers). These are the
final post-change numbers.

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md
git commit -m "perf(test): adopt nextest as the release gate

Nextest emits a single reliable exit code (retiring the piped-exit-code
hazard) and parallelizes at the test level across all binaries.
--no-fail-fast still required so one failing test doesn't hide others.
cargo test --no-fail-fast still works as a fallback."
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Phase |
|---|---|
| §4.1 — targeted test-profile optimization | Phase 2 |
| §4.2 — nextest becomes the canonical gate | Phase 4 |
| §4.3 — shrink oversized fixtures | Phase 3 |
| §4.4 — reclaim target/ disk | Phase 2 (clean) |
| §5 — sequencing (0-4) | Phase order |
| §6 — measurement protocol (4 numbers) | Every phase |
| §7 — contingency (binary consolidation) | Trigger-gated by 90s guard in Phase 2 |
| §10 — acceptance criteria | Phase 4 Step 2-3 |

**Discrepancy noted:** spec §4.3 says "three `seq 1 2000` fixtures" but
source has only two. The third slow test uses `echo hi`, not a large
fixture. Plan reflects the actual count (two) and explains why the
third test's speedup comes from Phase 2, not Phase 3.