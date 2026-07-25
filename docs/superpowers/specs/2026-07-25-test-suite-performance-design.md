# Test-Suite Performance — Design

> **Status:** DESIGN (brainstormed + partially measured, 2026-07-25). Ready for `writing-plans`.
>
> **Parent:** Developer-loop ergonomics — wall time of the agent-facing test gate.
>
> **Revision:** Supersedes the initial 6-item draft of the same date. Scope was
> cut from six items to four (plus one contingency, one exclusion) during
> brainstorming. §8 records what changed and why, so the dropped items are not
> naively re-proposed.

---

## 1. Goal & metric

Reduce the wall time of the **agent-facing test gate**, which `AGENTS.md:46`
defines as:

```sh
cargo test --workspace --features zoid/local-embed   # today
```

Agents run the full suite far more often than the maintainer does, so
unattended full-run wall time is the metric that matters. Single-test
iteration speed is explicitly *not* the optimization target.

**In scope:** four changes (§4.1–§4.4), sequenced in §5.

**Out of scope:**
- Reducing coverage. 1577 test functions before and after. No test deleted,
  `#[ignore]`-ed, or weakened.
- The `release` / `dist` profiles. `opt-level = "z"`, `lto = true`, and the
  `[profile.release.package.zoid-embed] opt-level = 3` override are
  load-bearing for the static-musl release target and must not be disturbed.
- Adding a CI test workflow. `.github/workflows/` contains only
  release/publish jobs — no `cargo test` invocation exists. Every item here is
  a local/agent-loop change.
- Replacing `insta`, `proptest`, or the tokio test runtime.

---

## 2. Baseline — PROVISIONAL, must be re-measured

> **⚠ The numbers in this section were measured with the WRONG COMMAND.**
> They came from `cargo test --workspace`, omitting
> `--features zoid/local-embed`. That feature pulls `zoid-embed` — and
> therefore **candle** — into the build, so the true gate is heavier than
> anything recorded below. **Re-baselining against the documented gate is the
> first implementation step (§5, Phase 1).** Treat every number here as a
> lower bound and a relative signal, not an absolute.

Host: 6 cores, `x86_64-unknown-linux-gnu`, rustc 1.97.1, `target/` at 31GB.

### 2.1 Phase split

| Phase | Measured (no `local-embed`) |
|---|---|
| Warm freshness check (`--no-run`, nothing changed) | 0.37s |
| Rebuild after a source change (`--no-run`) | 42.6s |
| Test **execution** | 132.9s |

The 0.37s warm figure establishes that **cargo's incremental cache is
healthy**. This is not a stale-cache problem; the cost is execution-dominated.

### 2.2 Execution ranked by binary

```
96.81s  economy_integration      ← 73% of the entire run
13.99s  inline_question_card
 7.85s  tasks_tool
 7.13s  ask_user
 3.86s  zoid_provider (unit)
 1.72s  zoid (unit)
 1.03s  zoid (bin unit)
<4.0s   all 34 remaining binaries combined
```

Four binaries account for **125.8s of 132.9s (95%)**.

### 2.3 Root cause

`economy_integration` holds four `#[tokio::test]` functions. Isolated:

| Test | Isolated |
|---|---|
| `turn_usage_lands_in_ledger` | 0.01s |
| `compaction_does_not_emit_updates_when_nothing_compacted` | 22.61s |
| `oversized_tool_result_is_compacted_when_over_threshold` | ~22s |
| `compaction_emits_started_and_complete_updates` | ~22s |

At `--test-threads=1` the binary totals 210.41s.

The three slow tests each shell out to:

```sh
for i in $(seq 1 2000); do echo "line $i: filler text to pad out tokens"; done
```

That subprocess was timed directly: **0.00s**. The ~22s is therefore **pure
Rust compute** — `zoid-core::compaction` walking ~80KB through
`estimate_tokens` / `chars().count()` / `output.lines()` — running at
`opt-level = 0`, because the `test` profile inherits `dev`.

### 2.4 Measured effect of optimizing the test profile

Rebuilt into a scratch `CARGO_TARGET_DIR` with `CARGO_PROFILE_DEV_OPT_LEVEL=1`:

| Binary | `opt-level = 0` | `opt-level = 1` | Speedup |
|---|---|---|---|
| `economy_integration` | 96.81s | **39.25s** | 2.47x |
| `inline_question_card` | 13.99s | **3.13s** | 4.47x |
| `tasks_tool` | 7.85s | **1.75s** | 4.49x |
| `ask_user` | 7.13s | **1.62s** | 4.40x |

All four are measured, not projected. Note the effect is **not** the 10–50x
that debug-vs-release comparisons often show — the hot paths are allocation-
and iteration-bound rather than arithmetic-bound.

**Attribution gap:** `economy_integration`'s hot code is *proven* to be
`zoid-core::compaction`. The other three tests' 4.4–4.5x is **unattributed** —
it could be `zoid-core`, `zoid-tui`, or `zoid`. §4.1 depends on closing this
gap; §5 Phase 0 exists partly to close it.

### 2.5 Environment facts relevant to the plan

- **27 integration test binaries**: 19 in `crates/zoid`, 5 in `zoid-tui`, 1
  each in `zoid-core`, `zoid-mcp`, `zoid-plugin-import`, plus ~14 unit-test
  binaries. Each integration binary is a separate crate linking the full
  dependency graph.
- **`crates/zoid/tests/fixtures/` holds only data** (`catalog/index.json`,
  `catalog/ok-skills.toml`) — no `.rs` helpers, and **no `mod` declarations
  exist in any test file**. Relevant to the §7 contingency.
- **No shared process state in tests**: zero occurrences of `static`,
  `lazy_static`, `OnceLock`, `env::set_var`, or `set_current_dir` across all
  test files; `serial_test` is not a dependency. This is what makes §4.2's
  process-per-test model low-risk.
- **`lld` and `clang` installed; `mold`, `sccache`, `cargo-nextest` are not.**
- **No `.cargo/config.toml`** at repo or user level.

---

## 3. Design principle: two phases, opposing pressures

The work touches two distinct costs that move in **opposite** directions:

- **Execution** (132.9s) — reduced by optimizing the test profile.
- **Rebuild** (42.6s) — *increased* by the same change, since higher
  `opt-level` means slower compiles.

The original draft offset this with two rebuild-targeting items (binary
consolidation, `lld`). Both were cut in brainstorming (§8), so **nothing in
scope offsets the rebuild regression.** Two consequences, both load-bearing:

1. §4.1 is deliberately **surgical rather than blanket** — it optimizes only
   the crates proven to contain hot code.
2. §6's measurement protocol records **three numbers, not one**. A single
   "total time" figure can hide a rebuild regression behind an execution win.

---

## 4. In-scope changes

### 4.1 Targeted test-profile optimization

**Change** — root `Cargo.toml`:

```toml
[profile.test.package.zoid-core]
opt-level = 1
```

**Rationale.** Cargo profile settings apply **per compiled crate**, not per
test binary. `[profile.test.package.zoid-core]` optimizes `zoid-core`'s code
wherever it executes — including from `crates/zoid`'s integration tests — so
the hot loop gets optimized without paying to optimize the other thirteen
crates. `zoid-core` is a small leaf crate, so the rebuild cost is minimal.

This is the **established house idiom**, not a novel technique: the root
manifest already carries `[profile.release.package.zoid-embed] opt-level = 3`
for exactly this reason (candle's matmuls under `opt-level = "z"`). §4.1
applies the same surgical pattern to the `test` profile.

**Escalation rule.** Start with `zoid-core` alone. Measure. Add packages
**only where measurement shows a remaining hot binary**, one at a time,
re-measuring after each. Candidates, in likely order: `zoid-tui`, `zoid`.
Do not add packages speculatively.

**Guard.** If incremental rebuild (§6, number 2) exceeds **90s**, stop
widening the package list and trigger the §7 contingency.

**Rejected alternative — blanket `[profile.test] opt-level = 1`.** Maximum
execution win, but every crate pays compile cost with nothing in scope to
offset it (§3). Retained as the **fallback** if targeted overrides
underdeliver on execution.

**Rejected alternative — `[profile.test.package."*"] opt-level = 2`.** The
common internet recipe: optimize dependencies, leave first-party crates
unoptimized. It optimizes precisely the wrong half here — §2.3 establishes the
hot code is ours. Documented only so it is not re-proposed.

**Open:** `opt-level = 1` vs `2`. Resolve empirically (§9.1).

**Risk.** `opt-level > 0` degrades debug-info fidelity; panic backtraces in
failing tests may report less precise line numbers. Spot-check once that a
deliberately failing test still reports a usable location.

---

### 4.2 Nextest becomes the canonical gate

**Change** — install `cargo-nextest`; update `AGENTS.md:46` to:

```sh
cargo nextest run --workspace --features zoid/local-embed
```

**Rationale — correctness first, speed second.**

`AGENTS.md:46` today carries this warning:

> "Never trust a piped exit code; use `--no-fail-fast` so one failing test
> binary can't hide others."

That warning exists because `cargo test` fails fast at *binary* granularity
and pipes mask exit codes — a documented footgun in the release process.
Nextest runs every test regardless of failures and emits a single
authoritative summary with a reliable exit code. **§4.2 retires that hazard**,
which is a stronger justification than the speed win.

The speed win is real but secondary: `cargo test` runs test **binaries**
sequentially, parallelizing only *within* a binary. With ~41 binaries on 6
cores, most of the run is one core working while five idle — §2.2 shows 34
binaries contributing <4s of real work while each pays serial process startup.
Nextest schedules at the **test** level across all binaries.

Nextest also reports per-test timings natively — the diagnostic this entire
spec had to be reconstructed by hand, and the tool that closes §2.4's
attribution gap.

**Risk — the only item with real behavioral exposure.** Nextest runs each test
in **its own process**. Tests implicitly sharing process state can fail under
it. §2.5 records that a scan found none of the usual culprits, so risk is
assessed **low** — but low is not none. The first nextest run is a
**verification step**: any failure must be investigated as a **latent
test-isolation bug in our tests**, not as a nextest defect, and must not be
worked around by forcing serial execution without understanding the cause.

**Compatibility requirement.** `cargo test --workspace` must continue to pass
unchanged. Nextest becomes the *documented* gate, not the *only* way to run
tests.

---

### 4.3 Shrink the oversized fixtures

**Change** — in `crates/zoid/tests/economy_integration.rs`, change the three
`seq 1 2000` fixtures to `seq 1 100`.

**Rationale.** The tests set `compact_threshold = Some(50)` — *fifty tokens*.
The fixture generates ~2000 lines × ~40 chars ≈ 80,000 chars. With
`estimate_tokens` = `ceil(chars / 3)`, that is **~26,600 tokens to cross a
50-token threshold** — an overshoot of roughly three orders of magnitude.

**Why 100 lines is provably sufficient.** Two conditions must hold:

1. *Threshold crossed:* 100 × ~40 chars ≈ 4000 chars ≈ **1333 tokens**, still
   ~27x the 50-token threshold.
2. *Compaction yields a gain:* `compact_tool_output` keeps
   `COMPACT_HEAD_LINES = 8` lines plus an elision marker, and returns the
   original unless `estimate_tokens(candidate) < estimate_tokens(output)`
   (`compaction.rs:243`). At 100 lines the summary is 8 lines + marker vs. 100
   lines — an unambiguous reduction, comfortably satisfying the
   `summary_tokens >= it.tokens` "no gain" guard at `compaction.rs:193`.

**Acceptance.** Existing assertions stay **unchanged and passing** — in
particular `oversized_tool_result_is_compacted_when_over_threshold`'s
`assert!(compacted, ...)`. If 100 proves insufficient, raise N until it passes;
**do not weaken the assertion.** Record the final N.

This changes fixture *volume* only, never what is verified.

---

### 4.4 Reclaim `target/` disk

**Change** — `cargo clean`. Optionally evaluate `cargo-sweep` for ongoing
pruning.

**Rationale.** `target/` is **31GB**. This is housekeeping, not a speed fix,
and must not be reported as a performance result. Indirect benefit only:
cheaper filesystem stat-walks on the freshness check.

**Sequencing (load-bearing).** §4.1 changes the `test` profile, which
**invalidates every existing test artifact anyway**. Running the clean at that
moment costs nothing, because the rebuild is already mandatory. Run it at any
other time and it wastes a full rebuild.

---

## 5. Sequencing

**Phase 0 — install nextest, change nothing else.**
Installing a tool is not a repo change and carries no rollback risk, but it
yields per-test timing that improves measurement quality for every phase
after it, and it is the most direct way to close §2.4's attribution gap —
which §4.1's package list depends on. This is why nextest moved from third
(original draft) to zeroth.

**Phase 1 — re-baseline.** Record §6's three numbers against the *correct*
gate (`--features zoid/local-embed`). Replace §2 wholesale. No code change.

**Phase 2 — §4.4 clean + §4.1 profile, together.** Measure. Apply the
escalation rule and the 90s guard.

**Phase 3 — §4.3 fixtures.** Measure.

**Phase 4 — §4.2 adopt nextest as the gate.** Update `AGENTS.md:46`. Verify
`cargo test --workspace` still passes.

Phases 2–4 each end with a measurement; no phase begins before the previous
one's numbers are recorded.

---

## 6. Measurement protocol

Required after **every** phase. Record all three numbers — §3 explains why one
number is insufficient.

```sh
# 1. warm freshness (nothing changed)
cargo test --workspace --features zoid/local-embed --no-run

# 2. incremental rebuild (touch the most-depended-on crate)
touch crates/zoid-core/src/lib.rs
/usr/bin/time -f "REBUILD %e s" \
  cargo test --workspace --features zoid/local-embed --no-run

# 3a. execution — cargo test (the comparable-to-baseline runner)
/usr/bin/time -f "EXEC_cargo %e s" \
  cargo test --workspace --features zoid/local-embed --no-fail-fast

# 3b. execution — nextest (the future gate)
/usr/bin/time -f "EXEC_nextest %e s" \
  cargo nextest run --workspace --features zoid/local-embed
```

**Both execution numbers are required, every phase.** The §2 baseline was
measured with `cargo test`; the target gate is `cargo nextest run`. Recording
only the latter would conflate nextest's parallelism win with the `opt-level`
win and make every per-phase delta uninterpretable. 3a gives a clean
like-for-like series against the baseline; 3b tracks the number that will
actually matter once §4.2 lands. The gap between them *is* nextest's
contribution, measured for free.

Provisional baseline to beat (§2, wrong command — supersede in Phase 1):
**0.37s / 42.6s / 132.9s (cargo test)**.

**Measurement hygiene:** run phases **sequentially, never in parallel** — on 6
cores, concurrent cargo invocations contend and skew every number. (This is
not hypothetical; it was the reason the measurement harness for §2.4 was
written serially.)

---

## 7. Contingency — binary consolidation

**Status: NOT in scope. Trigger-gated.**

**Trigger:** incremental rebuild (§6, number 2) exceeds **90s** after §4.1.

**Change if triggered:** move the 19 files in `crates/zoid/tests/*.rs` into
`crates/zoid/tests/suite/`, and add one `crates/zoid/tests/suite.rs` declaring
`mod agent_loop; mod ask_user; …`. Cargo auto-discovers only *top-level*
`tests/*.rs`, so the files become modules of a single crate. Nineteen full
links of the dependency graph (tokio, reqwest+rustls, rusqlite-bundled, git2,
ratatui) collapse to one.

**Why it is viable here:** §2.5 establishes there are no shared `mod` helpers
to untangle — the usual blocker for merging N test binaries. `tests/fixtures/`
is already a subdirectory and is unaffected; fixture paths resolve from
`CARGO_MANIFEST_DIR`, not the source file, so no path edits are needed
(verify).

**Why it is not in scope now:** it changes test invocation paths
(`--test economy_integration` → `--test suite economy_integration::`),
requiring `AGENTS.md` and agent-habit updates, and it coarsens rebuild
granularity for single-test iteration.

**Interaction with §4.2:** consolidation does **not** reduce nextest's
parallelism — nextest parallelizes per-test, not per-binary. The two are
complementary.

---

## 8. Excluded, with rationale

Recorded so these are not naively re-proposed.

**`lld` linker via committed `.cargo/config.toml` — excluded.**
Would have targeted the rebuild phase. Excluded on **release-path risk**: the
root manifest's `git2 = { default-features = false }` comment documents a
prior openssl-sys/libssh2-sys failure to cross-compile for static-musl, and a
linker change is precisely the class of change that disturbs that. A committed
`.cargo/config.toml` also imposes a toolchain requirement on every contributor
and every agent environment. The cost/benefit did not justify touching the
`dist` pipeline for a rebuild-phase win.

**Reducing test count / `#[ignore]`-ing slow tests — never considered.**
Coverage is fixed at 1577 tests. Every item here makes the same tests run
faster; none makes fewer tests run.

---

## 9. Open questions

1. **`opt-level = 1` vs `2`** for the §4.1 overrides. A measurement job was
   run but **its output was lost to a shell failure** — this is genuinely
   unresolved, not merely undocumented. Resolve in Phase 2.
2. **Crate attribution** for `inline_question_card` / `tasks_tool` /
   `ask_user`'s 4.4–4.5x (§2.4). Determines whether §4.1's package list needs
   `zoid-tui` and/or `zoid`. Resolve in Phase 0/1 using nextest per-test
   timing.
3. **True baseline magnitude** under `--features zoid/local-embed`. Candle is
   a heavy dependency; the rebuild number in particular may be far worse than
   §2.1 suggests, which would raise the stakes on the §7 trigger.

---

## 10. Acceptance criteria

- `cargo nextest run --workspace --features zoid/local-embed` passes with
  **1577 tests**, zero failures, and the same ignored count (2) as today.
- `cargo test --workspace` still passes unchanged (§4.2 compatibility
  requirement).
- `cargo build --release` succeeds and the static-musl release target builds —
  proving the `dist` pipeline is undisturbed.
- Total wall time (rebuild + execution) is materially below the Phase 1
  re-baseline, with per-phase before/after numbers recorded.
- Incremental rebuild is ≤ 90s, or the §7 contingency was triggered and
  applied.
- Any change whose measurement shows a net regression is **reverted rather
  than kept**.
