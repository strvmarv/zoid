# Build Expiration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every zoid build a 30-day shelf life so a leaked hidden pre-release build refuses to launch once it is more than 30 days past its build date.

**Architecture:** `build.rs` stamps the build's Unix epoch into a compile-time env var (`ZOID_BUILD_EPOCH`). A new pure module `expiry.rs` computes a `Verdict` from `(now, build, window)` and an `enforce()` wrapper reads the real clock + the stamp and exits the process before terminal setup on a bad verdict. `main.rs` calls `enforce()` only in the TUI-launch arm, so `--version`/`--help`/`update` remain usable as escape hatches.

**Tech Stack:** Rust 2021, Cargo build script (`cargo:rustc-env`), stdlib `std::time` only — no new dependencies.

## Global Constraints

- **No new dependencies.** Uses only `std::time` and existing cargo build-script mechanics. (Spec: "zero runtime deps".)
- **Window is exactly 30 days**, expressed as `pub const WINDOW_SECS: u64 = 30 * 24 * 60 * 60;` — the single source of truth. (Spec § Window.)
- **Boundary is inclusive of day 30:** valid up to and including `build + WINDOW_SECS`; refuse at `build + WINDOW_SECS + 1`. (Spec § B boundary note.)
- **Gate only the `Cli::Run` arm.** `Version`, `Help`, `Update` stay ungated. (Spec § C, escape hatch.)
- **Enforce before terminal setup**, printing to **stderr** and exiting status **1** — no panic/backtrace. (Spec § B.)
- **Git commits must NOT include any `Co-Authored-By` / co-author trailer.** (User global CLAUDE.md.)
- Module exposure follows the existing pattern in `crates/zoid/src/lib.rs`: `pub mod <name>;`. (Verified: `pub mod cli;`, `pub mod update;` present.)
- **Escape-hatch caveat (leaned on, stated out loud):** leaving `zoid update` ungated means an expired build can be reset by self-updating. This is safe **only while release assets are non-public** — a casual leak-holder cannot fetch a fresh asset. If zoid's release assets ever become publicly downloadable, `zoid update` becomes a one-command bypass of the whole tripwire and this decision must be revisited.
- **Shell portability:** the repo's default shell is `fish`, which lacks `$?`, `$((…))`, and inline `VAR=val cmd`. Every smoke-test block below is therefore wrapped in `bash -c '…'` so it runs identically regardless of the invoking shell. Do not strip the wrapper.

---

### Task 1: Stamp the build epoch in `build.rs`

Must land first: Task 2's `enforce()` uses `env!("ZOID_BUILD_EPOCH")`, which fails compilation of the whole crate if the build script has not emitted that variable.

**Files:**
- Modify: `crates/zoid/build.rs` (currently 8 lines; adds a second `rustc-env` stamp)

**Interfaces:**
- Consumes: nothing.
- Produces: compile-time env var `ZOID_BUILD_EPOCH` = decimal Unix-epoch seconds (a `u64` rendered as a string), available to the `zoid` crate's lib and bin targets via `env!("ZOID_BUILD_EPOCH")`.

- [ ] **Step 1: Replace `build.rs` with the epoch-stamping version**

Full new contents of `crates/zoid/build.rs`:

```rust
//! Embeds two compile-time facts for the `zoid` binary:
//!   * `ZOID_TARGET`      — the build target triple, so `zoid update` can pick
//!     the matching release asset at runtime (spec §2 component A).
//!   * `ZOID_BUILD_EPOCH` — the build's Unix-epoch seconds, so the build can
//!     refuse to launch once it is >30 days old (build-expiration spec §A).

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=ZOID_TARGET={target}");

    // Prefer a caller-provided SOURCE_DATE_EPOCH (reproducible / CI-pinnable);
    // otherwise stamp the current wall clock at build time.
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before UNIX_EPOCH")
                .as_secs()
        });
    println!("cargo:rustc-env=ZOID_BUILD_EPOCH={epoch}");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}
```

- [ ] **Step 2: Verify the crate still builds and the var is emitted**

Run:
```bash
cargo build -p zoid 2>&1 | tail -5
```
Expected: builds without error (warnings unrelated to this change are fine).

Run (confirms the stamp is a parseable epoch baked into the binary):
```bash
strings target/debug/zoid | grep -E 'ZOID_BUILD_EPOCH|^1[0-9]{9}$' | head
```
Expected: at least one 10-digit epoch-like number present (the stamp; the literal `ZOID_BUILD_EPOCH` name is NOT in the binary — only its value is substituted by `env!`, so matching the epoch value is the real check). If unsure, defer verification to Task 2's tests, which exercise the value directly.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/build.rs
git commit -m "feat(expiry): stamp ZOID_BUILD_EPOCH at build time"
```

---

### Task 2: Pure `expiry` module with `evaluate`, `enforce`, and boundary tests

**Files:**
- Create: `crates/zoid/src/expiry.rs`
- Test: inline `#[cfg(test)] mod tests` in the same file (matches the codebase's in-file test convention, e.g. `crates/zoid/src/cli.rs`).

**Interfaces:**
- Consumes: `env!("ZOID_BUILD_EPOCH")` from Task 1.
- Produces:
  - `pub const WINDOW_SECS: u64` (= 2_592_000).
  - `pub enum Verdict { Ok, Expired, ClockBeforeBuild }` (derives `Debug, PartialEq, Eq`).
  - `pub fn evaluate(now_secs: u64, build_secs: u64, window: u64) -> Verdict` — pure.
  - `pub fn enforce()` — reads real clock + stamp, `std::process::exit(1)` on non-`Ok`.

- [ ] **Step 1: Write the module with the pure evaluator and failing tests first**

Create `crates/zoid/src/expiry.rs` with the pure logic and tests, but leave `enforce()` out for now so the tests compile and run against `evaluate` alone:

```rust
//! Build expiration: a best-effort tripwire so a leaked hidden pre-release
//! build stops launching once it is >30 days past its build date. Not DRM —
//! the check reads the local clock inside the binary (build-expiration spec).

/// 30-day shelf life. Single source of truth for the window.
pub const WINDOW_SECS: u64 = 30 * 24 * 60 * 60;

/// Outcome of an age check. `ClockBeforeBuild` means the wall clock reads
/// earlier than the build stamp — impossible for a legit run, so treated as a
/// wrong/tampered clock.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Expired,
    ClockBeforeBuild,
}

/// Pure age check. All inputs injected — no clock read, no env read.
/// Valid up to and including `build_secs + window`; expired one second later.
pub fn evaluate(now_secs: u64, build_secs: u64, window: u64) -> Verdict {
    if now_secs < build_secs {
        Verdict::ClockBeforeBuild
    } else if now_secs > build_secs.saturating_add(window) {
        Verdict::Expired
    } else {
        Verdict::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::{evaluate, Verdict, WINDOW_SECS};

    const BUILD: u64 = 1_700_000_000; // arbitrary fixed build epoch

    #[test]
    fn fresh_build_is_ok() {
        assert_eq!(evaluate(BUILD, BUILD, WINDOW_SECS), Verdict::Ok);
    }

    #[test]
    fn last_valid_second_is_ok() {
        assert_eq!(evaluate(BUILD + WINDOW_SECS, BUILD, WINDOW_SECS), Verdict::Ok);
    }

    #[test]
    fn one_second_past_window_is_expired() {
        assert_eq!(
            evaluate(BUILD + WINDOW_SECS + 1, BUILD, WINDOW_SECS),
            Verdict::Expired
        );
    }

    #[test]
    fn clock_before_build_is_flagged() {
        assert_eq!(evaluate(BUILD - 1, BUILD, WINDOW_SECS), Verdict::ClockBeforeBuild);
    }

    #[test]
    fn window_is_exactly_thirty_days() {
        // Pins the constant so a fat-fingered edit to the window can't pass silently.
        assert_eq!(WINDOW_SECS, 2_592_000);
    }
}
```

- [ ] **Step 2: Register the module so the tests compile**

Add to `crates/zoid/src/lib.rs`, in alphabetical position between `pub mod eventlog;` and `pub mod github_fetch;` (the file is alphabetized):

```rust
pub mod expiry;
```

Resulting region of `lib.rs`:
```rust
pub mod cli;
pub mod eventlog;
pub mod expiry;
pub mod github_fetch;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run:
```bash
cargo test -p zoid --lib expiry 2>&1 | tail -20
```
Expected: `test result: ok. 5 passed` (four boundary tests + the `WINDOW_SECS` constant pin).

(These are written to pass immediately because `evaluate` is included in Step 1 — the "failing" phase here is conceptual: if you comment out the body of `evaluate` and return `Verdict::Ok` unconditionally, `one_second_past_window_is_expired` and `clock_before_build_is_flagged` fail. Optional: do that, run, watch them fail, then restore — to confirm the tests have teeth.)

- [ ] **Step 4: Add the `enforce()` wrapper**

Append to `crates/zoid/src/expiry.rs`, after `evaluate` and before the `#[cfg(test)]` module:

```rust
/// Read the compile-time build stamp and the real clock, evaluate, and on a
/// non-`Ok` verdict print to stderr and exit(1) — call this BEFORE any terminal
/// setup so the message renders cleanly with no panic backtrace.
pub fn enforce() {
    // `env!` makes the stamp a hard compile-time requirement (build.rs emits
    // it). `unwrap_or(0)` only guards a malformed value, which cannot occur
    // from the numeric build.rs stamp.
    let build_secs: u64 = env!("ZOID_BUILD_EPOCH").parse().unwrap_or(0);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match evaluate(now_secs, build_secs, WINDOW_SECS) {
        Verdict::Ok => {}
        Verdict::Expired => {
            eprintln!("This zoid build has expired. Grab a newer build (`zoid update`).");
            std::process::exit(1);
        }
        Verdict::ClockBeforeBuild => {
            eprintln!("zoid can't verify this build's age — check your system clock.");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 5: Verify the whole crate still compiles with `enforce()` present**

Run:
```bash
cargo build -p zoid 2>&1 | tail -5 && cargo test -p zoid --lib expiry 2>&1 | tail -5
```
Expected: build succeeds; the 5 `expiry` tests still pass. (`enforce()` has no unit test — it calls `process::exit`; it is covered by Task 3's manual smoke test.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/expiry.rs crates/zoid/src/lib.rs
git commit -m "feat(expiry): pure evaluate() + enforce() with 30-day window"
```

---

### Task 3: Wire `enforce()` into the TUI-launch path and smoke-test

**Files:**
- Modify: `crates/zoid/src/main.rs` — the `Cli::Run { .. }` arm (currently `main.rs:1470-1474`).

**Interfaces:**
- Consumes: `zoid::expiry::enforce()` from Task 2.
- Produces: no new public surface — an enforcement side effect on the TUI-launch path.

- [ ] **Step 1: Add the `enforce()` call to the `Run` arm**

In `crates/zoid/src/main.rs`, find this arm (around line 1470):

```rust
        zoid::cli::Cli::Run { companion, new, resume } => {
            cli_new = new;
            cli_resume = resume;
            companion
        }
```

Replace it with:

```rust
        zoid::cli::Cli::Run { companion, new, resume } => {
            // Build expiration: refuse to launch a >30-day-old build (or one on
            // a clock that predates the build). Runs before any DB/terminal
            // setup so the message prints cleanly. --version/--help/update are
            // deliberately NOT gated (escape hatches). See src/expiry.rs.
            zoid::expiry::enforce();
            cli_new = new;
            cli_resume = resume;
            companion
        }
```

- [ ] **Step 2: Build**

Run:
```bash
cargo build -p zoid 2>&1 | tail -5
```
Expected: builds cleanly.

- [ ] **Step 3: Smoke-test an EXPIRED build refuses the TUI but honors escape hatches**

Build with a stamp 40 days in the past, then exercise all three paths (wrapped in `bash -c` per the shell-portability constraint):

```bash
bash -c '
SOURCE_DATE_EPOCH=$(( $(date +%s) - 40*24*60*60 )) cargo build -p zoid 2>&1 | tail -3
echo "--- TUI (should refuse, exit 1) ---"
./target/debug/zoid < /dev/null; echo "exit=$?"
echo "--- --version (escape hatch, should print + exit 0) ---"
./target/debug/zoid --version; echo "exit=$?"
echo "--- --help (escape hatch, should print + exit 0) ---"
./target/debug/zoid --help >/dev/null; echo "exit=$?"
'
```
Expected:
- TUI path prints `This zoid build has expired. Grab a newer build (\`zoid update\`).` and `exit=1`.
- `--version` prints `zoid <version>` and `exit=0`.
- `--help` prints usage and `exit=0`.

(`zoid update` is also an escape hatch but performs a network fetch; the `--version`/`--help` checks prove the ungated-path wiring without hitting the network.)

- [ ] **Step 4: Smoke-test a FRESH build launches normally**

```bash
bash -c '
cargo build -p zoid 2>&1 | tail -3   # fresh stamp = now
./target/debug/zoid --version; echo "exit=$?"
'
```
Expected: `zoid <version>`, `exit=0`. (A full TUI launch needs a terminal; `--version` confirms the fresh build is not falsely blocked. To eyeball the interactive path, run `./target/debug/zoid` in a real terminal and confirm it starts.)

- [ ] **Step 5: Optional — smoke-test the reverse-clock tripwire**

Build with a stamp dated in the FUTURE, so the current clock reads "before build":

```bash
bash -c '
SOURCE_DATE_EPOCH=$(( $(date +%s) + 10*24*60*60 )) cargo build -p zoid 2>&1 | tail -3
./target/debug/zoid < /dev/null; echo "exit=$?"
cargo build -p zoid 2>&1 | tail -1   # rebuild fresh to leave the tree in a good state
'
```
Expected: prints `zoid can't verify this build's age — check your system clock.` and `exit=1`. The final rebuild restores a normal stamp.

- [ ] **Step 6: Run the full crate test suite to confirm no regressions**

Run:
```bash
cargo test -p zoid 2>&1 | tail -15
```
Expected: all tests pass (including the 4 new `expiry` tests).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(expiry): enforce 30-day expiration on TUI launch"
```

---

## Self-Review

**1. Spec coverage:**

| Spec element | Task |
|---|---|
| §A build.rs stamps `ZOID_BUILD_EPOCH` (SOURCE_DATE_EPOCH-or-now) | Task 1 |
| §A `rerun-if-env-changed=SOURCE_DATE_EPOCH` | Task 1 Step 1 |
| §B `WINDOW_SECS`, `Verdict`, pure `evaluate` | Task 2 Steps 1 |
| §B `enforce()` stderr + exit(1), before terminal setup | Task 2 Step 4 + Task 3 Step 1 |
| §B inclusive day-30 boundary | Task 2 tests (`last_valid_second_is_ok`, `one_second_past_window_is_expired`) |
| §C gate only `Cli::Run`; leave version/help/update | Task 3 Step 1 + Step 3 verification |
| §C `pub mod expiry;` in lib.rs | Task 2 Step 2 |
| §E four boundary unit tests (+ `WINDOW_SECS` const pin) | Task 2 Step 1 |
| §E manual smoke test (old SOURCE_DATE_EPOCH) | Task 3 Steps 3–5 |
| §D future hidden-only gate | Out of scope by design (YAGNI); no task — correct. |
| Escape-hatch bypass caveat named | Global Constraints |
| Shell portability (fish default) | Global Constraints + `bash -c` on all smoke blocks |

All in-scope spec elements map to a task. §D is explicitly not built.

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to Task N". All code shown in full. ✓

**3. Type consistency:** `WINDOW_SECS: u64`, `Verdict::{Ok,Expired,ClockBeforeBuild}`, `evaluate(u64,u64,u64) -> Verdict`, `enforce()` used identically across Tasks 2 and 3. Module path `zoid::expiry::enforce` matches `pub mod expiry;` in `lib.rs`. ✓
