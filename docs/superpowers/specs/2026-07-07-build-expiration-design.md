# zoid Build Expiration — Design

**Date:** 2026-07-07
**Status:** Approved (design), pending implementation plan
**Author:** strvmarv (with Claude)

## Goal

Give every zoid build a 30-day shelf life so that a **hidden (pre-public,
unannounced) release that leaks eventually stops working**. A build stamps its
own build time at compile; at launch it refuses to start once it is more than 30
days past that stamp.

This is **best-effort deterrence, not DRM**. The check reads the local system
clock inside a compiled binary, so a determined holder can set their clock back
or patch the binary. The source repository is private, so for a leaked binary
handed to a casual recipient the expiration is real friction. We are not trying
to defeat a motivated attacker with debugger access — only to ensure a leaked
build does not remain quietly useful for months.

## Scoping decisions (locked)

| Decision | Choice | Rationale |
|---|---|---|
| Which builds expire | **Every build, always (for now)** | zoid is pre-public; today every release is effectively a hidden pre-release. A single window constant makes a later "hidden-only" gate a one-line change. |
| Behavior on expiry | **Hard-block only, no warning** | Runs normally through day 30, then flatly refuses to launch. No countdown nag. |
| Clock-tamper hardening | **Reverse-clock tripwire included** | Also refuse if `now < build_date` — a legit build can never run before it was built, so this catches naive clock-setback for free. |
| Escape hatch | **Gate only the TUI launch path** | `--version`, `--help`, and `zoid update` still work on an expired build, so a legit tester can self-heal instead of re-downloading by hand. |
| Build-date source | **Compile-time stamp** (`SOURCE_DATE_EPOCH` if set, else build wall-clock) | "Build expiration" literally means the build's own age. CI clean builds stamp fresh per release. |
| Window | **30 days** (`WINDOW_SECS` constant) | Single source of truth; trivially adjustable. |

### On the reverse-clock tripwire's false positives

Refusing when `now < build_date` will also block a machine whose clock is
genuinely wrong and behind (dead CMOS battery, a fresh VM, a misconfigured
container). This is an accepted cost: the tripwire's message names the clock as
the suspected cause so the user can correct it, and the alternative (no
hardening) lets the lazy setback bypass succeed. It does **not** stop a *precise*
setback (clock nudged to `build_date + 1 day` stays in-window forever); only a
network time source would, which adds an online dependency we are deliberately
not taking.

## Architecture

```
build.rs ──stamp ZOID_BUILD_EPOCH (rustc-env)──► binary
                                                   │
                                          main() ── parse_args
                                                   │
                        ┌── Version / Help / Update ──► run (NOT gated)
                        │
                        └── Run { .. } ──► expiry::enforce()
                                             │
                                    evaluate(now, build, window)
                                       │        │            │
                                      Ok    ClockBeforeBuild  Expired
                                       │        └──── eprintln + exit(1) ────┘
                                       ▼
                                 launch TUI
```

## Components

### A. Build stamp — `crates/zoid/build.rs`

The build script already emits `ZOID_TARGET` via `cargo:rustc-env`. Add a build
epoch next to it, preferring a caller-provided `SOURCE_DATE_EPOCH` (so CI or a
reproducible build can pin the date) and falling back to the current wall clock:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

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
println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
```

**Known caveat (accepted):** Cargo caches build-script output, so on *incremental
local* rebuilds the stamp reflects when `target/` was last cleaned, not each
`cargo build`. This is cosmetic. **Release builds run in a clean CI checkout, so
every published build stamps fresh** — the case that matters. A stale local build
simply expires relative to its last clean build, consistent with "every build,
always."

### B. Expiry logic — new `crates/zoid/src/expiry.rs`

A pure evaluator with no I/O, plus a thin enforcement wrapper that supplies the
real clock and the compile-time stamp.

```rust
/// 30-day shelf life. Single source of truth for the window.
pub const WINDOW_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Expired,
    ClockBeforeBuild,
}

/// Pure: all inputs injected, no clock read, no env read. Unit-testable.
pub fn evaluate(now_secs: u64, build_secs: u64, window: u64) -> Verdict {
    if now_secs < build_secs {
        Verdict::ClockBeforeBuild
    } else if now_secs > build_secs.saturating_add(window) {
        Verdict::Expired
    } else {
        Verdict::Ok
    }
}
```

The wrapper reads the compile-time stamp and the real clock, evaluates, and on a
non-`Ok` verdict prints a plain message to **stderr** and exits with status `1`
**before** any terminal setup (so the message renders cleanly, no panic
backtrace):

```rust
pub fn enforce() {
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

Boundary note: `WINDOW_SECS` is inclusive of day 30 — the binary is valid up to
and including `build + 30 days` exactly, and refuses at `build + 30 days + 1s`.

### C. Enforcement point — `crates/zoid/src/main.rs`

`main()` matches the parsed `Cli` enum. `Version`, `Help`, and `Update` already
`return` early before terminal setup and are **left ungated** (the escape hatch).
`expiry::enforce()` is called at the top of the `Cli::Run` arm — the only path
that launches the TUI — before the DB, repo, and terminal are touched:

```rust
zoid::cli::Cli::Run { companion, new, resume } => {
    zoid::expiry::enforce(); // exits(1) if expired / clock-before-build
    cli_new = new;
    cli_resume = resume;
    companion
}
```

Add `mod expiry;` (and a `pub` re-export if `main.rs` calls it as
`zoid::expiry::enforce`, matching how `zoid::cli` / `zoid::update` are already
exposed via `lib.rs`).

### D. Future "hidden-only" gate (designed for, not built)

When zoid goes public and a stable release should **not** self-expire, the change
is confined to `enforce()`: read an opt-in build flag (e.g. a
`ZOID_EXPIRES` env consumed in `build.rs` into a second `rustc-env`) and return
early from `enforce()` when it is unset. No other component changes. Not built
now (YAGNI); the seam is the single `enforce()` function.

## Testing

**Unit (pure `evaluate`), covering the four boundaries:**

| Case | `now` relative to build | Expected |
|---|---|---|
| Fresh | `build` (day 0) | `Ok` |
| Last valid second | `build + WINDOW_SECS` exactly | `Ok` |
| First expired second | `build + WINDOW_SECS + 1` | `Expired` |
| Clock before build | `build - 1` | `ClockBeforeBuild` |

These need no clock mocking or process spawning — all inputs are injected.

**Manual smoke test (wiring):** build with an old stamp and confirm refusal:

```sh
SOURCE_DATE_EPOCH=$(( $(date +%s) - 40*24*60*60 )) cargo build -p zoid
./target/debug/zoid            # → prints "expired" message, exits 1
./target/debug/zoid --version  # → still prints version (escape hatch)
./target/debug/zoid update     # → still runs the updater (escape hatch)
```

## Non-goals

- Defeating a motivated attacker (debugger, binary patching, precise clock
  setback). Out of scope by design.
- Any network / online time source. The check is fully offline.
- Warning or grace period before expiry. Explicitly rejected — hard-block only.
- Expiring by *release/tag* date rather than *build* date. We expire by build age.
