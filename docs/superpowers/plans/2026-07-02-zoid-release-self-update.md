# zoid Release & Self-Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a repeatable release + self-update pipeline for the `zoid` binary: tag a version → cargo-dist cross-compiles three targets → a custom job publishes a GitHub Release to the public `strvmarv/zoid-releases` repo → users run `zoid update` to self-upgrade anonymously with checksum verification.

**Architecture:** Source stays in private `strvmarv/zoid`; binaries are distributed via a separate **public** `strvmarv/zoid-releases` repo so downloads are anonymous (zero tokens on user machines). A new hand-rolled CLI layer adds `--version`/`--help`/`update`. The `zoid update` command is a pure core (version compare, asset selection, sha256 verify, sums parsing) wrapped in a thin network/filesystem shell, reusing the workspace `reqwest` (rustls) client.

**Tech Stack:** Rust 2021 workspace; `reqwest` (rustls-tls, no OpenSSL); `sha2`; `flate2`+`tar` (unix) / `zip` (windows); cargo-dist for the CI build matrix; GitHub Actions.

## Global Constraints

- Version single-source: `[workspace.package] version`, bumped `0.0.0` → `0.1.0`; shipping crates use `version.workspace = true`.
- CLI arg parsing is **hand-rolled** (no `clap` dependency).
- HTTP uses the workspace `reqwest` (`default-features = false`, `rustls-tls`) — never add OpenSSL.
- Release targets (exactly three): `x86_64-unknown-linux-musl` (primary), `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. musl-static supersedes a separate glibc build.
- Archive formats are pinned so the updater's asset names stay in lockstep with CI: **unix = `.tar.gz`, windows = `.zip`**.
- Published asset names: `zoid-<target-triple>.tar.gz` (unix) / `zoid-<target-triple>.zip` (windows), plus `SHA256SUMS`.
- Distribution repo: `strvmarv/zoid-releases` (public, releases-only). `zoid update` fetches anonymously; **no token lives on any user machine**.
- CI publish auth: a single Actions secret `RELEASES_REPO_TOKEN` with `contents:write` on `zoid-releases` only.
- Release notes are curated by hand; **never** auto-generated from the private commit log.
- Self-update MUST verify sha256 before swapping; on any failure: clear message, non-zero exit, **no partial swap**, retain `<exe>.bak`.
- `cargo fmt --all` and `cargo clippy --all-targets` stay clean; follow existing repo style.

**Suggested branch:** `feat/release-self-update` (no worktree needed).

---

### Task 1: Version single-source + build-target embed

Establishes the one-line version bump mechanism and embeds the build target triple so the updater (Task 2) can pick the right asset.

**Files:**
- Modify: `Cargo.toml` (root — add `version` to `[workspace.package]`)
- Modify: `crates/zoid-core/Cargo.toml`, `crates/zoid-provider/Cargo.toml`, `crates/zoid-tui/Cargo.toml`, `crates/zoid-tools/Cargo.toml`, `crates/zoid-syntax/Cargo.toml`, `crates/zoid/Cargo.toml` (each: `version = "0.0.0"` → `version.workspace = true`)
- Create: `crates/zoid/build.rs`
- Test: `crates/zoid/tests/version_embed.rs`

**Interfaces:**
- Produces: `env!("CARGO_PKG_VERSION")` == `"0.1.0"` for the `zoid` bin; the build script sets `ZOID_TARGET` (the target triple) available via `env!("ZOID_TARGET")` in the `zoid` crate and its test/bin targets.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid/tests/version_embed.rs`:

```rust
//! Verifies the workspace version bump and the build.rs target embed.

#[test]
fn workspace_version_is_0_1_0() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
}

#[test]
fn build_target_is_embedded() {
    // Set by crates/zoid/build.rs via `cargo:rustc-env=ZOID_TARGET`.
    assert!(!env!("ZOID_TARGET").is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --test version_embed`
Expected: FAIL — `version` is still `0.0.0` (assert_eq mismatch) and `ZOID_TARGET` is undefined (compile error `environment variable 'ZOID_TARGET' not defined`).

- [ ] **Step 3: Add the workspace version**

In root `Cargo.toml`, under `[workspace.package]`:

```toml
[workspace.package]
edition = "2021"
version = "0.1.0"
```

- [ ] **Step 4: Point shipping crates at the workspace version**

In each of the six shipping crates' `Cargo.toml`, replace the `version = "0.0.0"` line under `[package]` with:

```toml
version.workspace = true
```

(Files: `crates/zoid-core`, `crates/zoid-provider`, `crates/zoid-tui`, `crates/zoid-tools`, `crates/zoid-syntax`, `crates/zoid`. Leave `crates/zoid-testkit` as `0.1.0` — dev-only, not shipped.)

- [ ] **Step 5: Create the build script**

Create `crates/zoid/build.rs`:

```rust
//! Embeds the build target triple so `zoid update` can select the matching
//! release asset at runtime (spec §2 component A).

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=ZOID_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p zoid --test version_embed`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/*/Cargo.toml crates/zoid/build.rs crates/zoid/tests/version_embed.rs
git commit -m "feat(release): workspace version 0.1.0 + embed build target triple"
```

---

### Task 2: `zoid update` self-updater module

The anonymous, checksum-verified self-replace logic. Pure functions are unit-tested; the network `run()` orchestrator is exercised end-to-end by the Task 5 smoke release.

**Files:**
- Modify: `crates/zoid/Cargo.toml` (add deps)
- Modify: `crates/zoid/src/lib.rs` (add `pub mod update;`)
- Create: `crates/zoid/src/update.rs`
- Test: `crates/zoid/tests/update_test.rs`

**Interfaces:**
- Consumes: `env!("ZOID_TARGET")` and `env!("CARGO_PKG_VERSION")` from Task 1.
- Produces:
  - `pub fn is_newer(current: &str, latest: &str) -> bool`
  - `pub fn asset_name(target: &str) -> String`
  - `pub fn parse_sha256sums(text: &str) -> std::collections::HashMap<String, String>`
  - `pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> anyhow::Result<()>`
  - `pub fn extract_binary(archive: &[u8]) -> anyhow::Result<Vec<u8>>`
  - `pub fn install_binary(target: &std::path::Path, new_bin: &[u8]) -> anyhow::Result<()>`
  - `pub fn build_target() -> &'static str`
  - `pub async fn run() -> anyhow::Result<()>` (called by Task 3's `update` dispatch)

- [ ] **Step 1: Add dependencies**

In `crates/zoid/Cargo.toml`, add to `[dependencies]`:

```toml
reqwest = { workspace = true }
sha2 = "0.10"
```

Add these target-specific sections (place after `[dependencies]`):

```toml
[target.'cfg(unix)'.dependencies]
flate2 = "1"
tar = "0.4"

[target.'cfg(windows)'.dependencies]
zip = "2"
```

Add to `[dev-dependencies]` (so the unix extraction test can build archives):

```toml
flate2 = "1"
tar = "0.4"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid/tests/update_test.rs`:

```rust
use zoid::update::{
    asset_name, install_binary, is_newer, parse_sha256sums, verify_sha256,
};

#[test]
fn newer_when_core_version_increases() {
    assert!(is_newer("0.1.0", "0.1.1"));
    assert!(is_newer("0.1.0", "v0.2.0"));
    assert!(is_newer("v0.1.0", "1.0.0"));
}

#[test]
fn not_newer_when_equal_or_older_or_prerelease() {
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.2.0", "0.1.9"));
    // Same core version with a pre-release suffix is NOT an upgrade.
    assert!(!is_newer("0.1.0", "0.1.0-test"));
}

#[test]
fn asset_name_matches_pinned_archive_formats() {
    assert_eq!(
        asset_name("x86_64-unknown-linux-musl"),
        "zoid-x86_64-unknown-linux-musl.tar.gz"
    );
    assert_eq!(
        asset_name("aarch64-apple-darwin"),
        "zoid-aarch64-apple-darwin.tar.gz"
    );
    assert_eq!(
        asset_name("x86_64-pc-windows-msvc"),
        "zoid-x86_64-pc-windows-msvc.zip"
    );
}

#[test]
fn parse_sums_maps_filename_to_hex() {
    let text = "abc123  zoid-x86_64-unknown-linux-musl.tar.gz\n\
                def456 *zoid-x86_64-pc-windows-msvc.zip\n";
    let map = parse_sha256sums(text);
    assert_eq!(map.get("zoid-x86_64-unknown-linux-musl.tar.gz").unwrap(), "abc123");
    // Leading '*' (binary mode) on the filename is stripped.
    assert_eq!(map.get("zoid-x86_64-pc-windows-msvc.zip").unwrap(), "def456");
}

#[test]
fn verify_sha256_accepts_correct_and_rejects_tampered() {
    // echo -n "hello" | sha256sum
    let digest = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    assert!(verify_sha256(b"hello", digest).is_ok());
    assert!(verify_sha256(b"hello!", digest).is_err()); // tampered payload
}

#[cfg(unix)]
#[test]
fn extract_finds_zoid_binary_in_targz() {
    // Build a .tar.gz containing `<subdir>/zoid` in memory, then extract it.
    let mut gz = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let data: &[u8] = b"fake-zoid-binary";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "zoid-x86_64-unknown-linux-musl/zoid", data)
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    let bin = zoid::update::extract_binary(&gz).unwrap();
    assert_eq!(bin, b"fake-zoid-binary");
}

#[cfg(unix)]
#[test]
fn install_swaps_binary_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("zoid");
    std::fs::write(&target, b"old-binary").unwrap();
    install_binary(&target, b"new-binary").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new-binary");
    assert_eq!(std::fs::read(target.with_extension("bak")).unwrap(), b"old-binary");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p zoid --test update_test`
Expected: FAIL — `unresolved import zoid::update` (module does not exist yet).

- [ ] **Step 4: Create the module**

Create `crates/zoid/src/update.rs`:

```rust
//! `zoid update`: anonymous, checksum-verified self-replace against the public
//! releases repo (spec §2 component B). Pure core (version compare, asset
//! selection, checksum, sums parsing) + a thin network/filesystem shell.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Public distribution repo that holds the GitHub Releases. Source stays private.
const RELEASES_REPO: &str = "strvmarv/zoid-releases";

/// The build target triple, embedded by `build.rs`.
pub fn build_target() -> &'static str {
    env!("ZOID_TARGET")
}

/// Parse "v0.1.0" / "0.1.0" / "0.1.0-test" into a (major, minor, patch) core
/// triple, ignoring a leading 'v' and any `-prerelease` suffix.
fn parse_core(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let core = v.split('-').next().unwrap_or(v);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is a strictly newer release than `current`. A pre-release
/// sharing the same core version is not considered newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_core(current), parse_core(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// The published asset filename for a build target triple. Archive formats are
/// pinned in cargo-dist config (unix `.tar.gz`, windows `.zip`).
pub fn asset_name(target: &str) -> String {
    if target.contains("windows") {
        format!("zoid-{target}.zip")
    } else {
        format!("zoid-{target}.tar.gz")
    }
}

/// Parse a `SHA256SUMS` file (coreutils format: "<hex>  <filename>") into a map
/// of filename → lowercase hex digest. A leading '*' on the filename is stripped.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(hex), Some(name)) = (parts.next(), parts.next()) {
            map.insert(name.trim_start_matches('*').to_string(), hex.to_lowercase());
        }
    }
    map
}

/// Verify `bytes` hashes to `expected_hex` (SHA-256). Error on mismatch.
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let got = Sha256::digest(bytes);
    let got_hex = got.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if got_hex == expected_hex.to_lowercase() {
        Ok(())
    } else {
        bail!("checksum verification failed (expected {expected_hex}, got {got_hex})")
    }
}

/// Extract the `zoid` binary bytes from a downloaded `.tar.gz` archive (unix).
#[cfg(unix)]
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.file_name().and_then(|s| s.to_str()) == Some("zoid") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("no `zoid` binary found in release archive")
}

/// Extract the `zoid.exe` binary bytes from a downloaded `.zip` archive (windows).
#[cfg(windows)]
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let base = file.name().rsplit(['/', '\\']).next().unwrap_or("");
        if base == "zoid.exe" {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("no `zoid.exe` binary found in release archive")
}

/// Atomically replace the binary at `target` with `new_bin`, keeping `<target>.bak`.
#[cfg(unix)]
pub fn install_binary(target: &Path, new_bin: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target.parent().ok_or_else(|| anyhow!("target has no parent dir"))?;
    let tmp = dir.join(".zoid-update.tmp");
    std::fs::write(&tmp, new_bin).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    let bak = target.with_extension("bak");
    if target.exists() {
        std::fs::rename(target, &bak)
            .with_context(|| format!("backing up {}", target.display()))?;
    }
    std::fs::rename(&tmp, target).with_context(|| format!("installing {}", target.display()))?;
    Ok(())
}

/// Windows variant: a running `.exe` cannot be overwritten in place, but it can
/// be renamed out of the way first.
#[cfg(windows)]
pub fn install_binary(target: &Path, new_bin: &[u8]) -> Result<()> {
    let dir = target.parent().ok_or_else(|| anyhow!("target has no parent dir"))?;
    let tmp = dir.join("zoid-update.tmp.exe");
    std::fs::write(&tmp, new_bin).with_context(|| format!("writing {}", tmp.display()))?;
    let bak = target.with_extension("bak");
    if bak.exists() {
        let _ = std::fs::remove_file(&bak);
    }
    if target.exists() {
        std::fs::rename(target, &bak)
            .with_context(|| format!("backing up {}", target.display()))?;
    }
    std::fs::rename(&tmp, target).with_context(|| format!("installing {}", target.display()))?;
    Ok(())
}

/// Entry point for `zoid update`.
pub async fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = build_target();
    let exe = std::env::current_exe().context("resolving current executable path")?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("zoid-updater/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest");
    let rel: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .context("could not reach releases repo")?
        .error_for_status()
        .context("releases API returned an error")?
        .json()
        .await?;

    let latest = rel["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow!("release has no tag_name"))?;
    if !is_newer(current, latest) {
        println!("zoid is already up to date (v{current})");
        return Ok(());
    }

    let want = asset_name(target);
    let assets = rel["assets"]
        .as_array()
        .ok_or_else(|| anyhow!("release has no assets"))?;
    let find = |name: &str| -> Option<String> {
        assets.iter().find_map(|a| {
            if a["name"].as_str() == Some(name) {
                a["browser_download_url"].as_str().map(String::from)
            } else {
                None
            }
        })
    };
    let asset_url = find(&want).ok_or_else(|| anyhow!("no release asset for {target}"))?;
    let sums_url = find("SHA256SUMS").ok_or_else(|| anyhow!("release has no SHA256SUMS"))?;

    println!("updating zoid {current} -> {latest} ({want})...");
    let archive = client.get(&asset_url).send().await?.error_for_status()?.bytes().await?;
    let sums = client.get(&sums_url).send().await?.error_for_status()?.text().await?;

    let expected = parse_sha256sums(&sums)
        .get(&want)
        .cloned()
        .ok_or_else(|| anyhow!("{want} missing from SHA256SUMS"))?;
    verify_sha256(&archive, &expected)
        .context("aborting: refusing to install an unverified binary")?;

    let bin = extract_binary(&archive)?;
    install_binary(&exe, &bin).with_context(|| format!("cannot replace {}", exe.display()))?;

    println!("zoid updated to {latest} (previous binary kept as {}.bak)", exe.display());
    Ok(())
}
```

- [ ] **Step 5: Register the module**

In `crates/zoid/src/lib.rs`, add after the existing `pub mod` lines:

```rust
pub mod update;
```

(Task 3 adds `pub mod cli;` when `cli.rs` exists. Do **not** add `pub mod cli;` here — the module file does not exist yet and the crate would fail to compile at Step 6.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid --test update_test`
Expected: PASS (7 tests on unix; the two `#[cfg(unix)]` tests are skipped on windows).

- [ ] **Step 7: Verify clippy and fmt are clean**

Run: `cargo clippy -p zoid --all-targets && cargo fmt --all -- --check`
Expected: no warnings, no diffs.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/Cargo.toml crates/zoid/src/update.rs crates/zoid/src/lib.rs crates/zoid/tests/update_test.rs Cargo.lock
git commit -m "feat(update): anonymous checksum-verified zoid self-updater"
```

---

### Task 3: CLI arg layer + wire into `main`

Adds the hand-rolled parser and dispatches `--version`/`--help`/`update`, leaving the no-arg path launching the TUI exactly as before.

**Files:**
- Create: `crates/zoid/src/cli.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod cli;`)
- Modify: `crates/zoid/src/main.rs:484` (dispatch at the top of `main`)
- Test: `crates/zoid/tests/cli_test.rs`

**Interfaces:**
- Consumes: `zoid::update::run()` from Task 2.
- Produces:
  - `pub enum Cli { Run, Version, Help, Update, Unknown(String) }`
  - `pub fn parse_args(args: impl IntoIterator<Item = String>) -> Cli`
  - `pub fn version_string() -> String`
  - `pub fn help_text() -> String`

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid/tests/cli_test.rs`:

```rust
use zoid::cli::{parse_args, version_string, Cli};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_args_launches_tui() {
    assert_eq!(parse_args(args(&[])), Cli::Run);
}

#[test]
fn version_flags() {
    assert_eq!(parse_args(args(&["--version"])), Cli::Version);
    assert_eq!(parse_args(args(&["-V"])), Cli::Version);
}

#[test]
fn help_flags() {
    assert_eq!(parse_args(args(&["--help"])), Cli::Help);
    assert_eq!(parse_args(args(&["-h"])), Cli::Help);
}

#[test]
fn update_subcommand() {
    assert_eq!(parse_args(args(&["update"])), Cli::Update);
}

#[test]
fn unknown_arg_is_reported() {
    assert_eq!(parse_args(args(&["--bogus"])), Cli::Unknown("--bogus".to_string()));
}

#[test]
fn version_string_tracks_pkg_version() {
    assert_eq!(version_string(), format!("zoid {}", env!("CARGO_PKG_VERSION")));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --test cli_test`
Expected: FAIL — `unresolved import zoid::cli`.

- [ ] **Step 3: Create the parser**

Create `crates/zoid/src/cli.rs`:

```rust
//! Minimal hand-rolled CLI parsing for the `zoid` binary (spec §2 component A).
//! Three flags and one subcommand do not justify a `clap` dependency.

/// The parsed intent of a process invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    /// Launch the TUI (default; no recognised args).
    Run,
    /// Print version and exit.
    Version,
    /// Print help and exit.
    Help,
    /// Run the self-updater and exit.
    Update,
    /// Unrecognised argument; carries the offending token.
    Unknown(String),
}

/// Parse process arguments (excluding argv[0]) into a [`Cli`] intent. Only the
/// first token is significant; the subcommands/flags take no operands.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Cli {
    match args.into_iter().next().as_deref() {
        None => Cli::Run,
        Some("--version" | "-V") => Cli::Version,
        Some("--help" | "-h") => Cli::Help,
        Some("update") => Cli::Update,
        Some(other) => Cli::Unknown(other.to_string()),
    }
}

/// The line printed by `--version`.
pub fn version_string() -> String {
    format!("zoid {}", env!("CARGO_PKG_VERSION"))
}

/// The text printed by `--help`.
pub fn help_text() -> String {
    "\
zoid - event-sourced terminal agent

USAGE:
    zoid            Launch the TUI
    zoid update     Download and install the latest release
    zoid --version  Print version
    zoid --help     Print this help"
        .to_string()
}
```

- [ ] **Step 4: Register the module**

In `crates/zoid/src/lib.rs`, add alongside the existing `pub mod update;` line:

```rust
pub mod cli;
```

- [ ] **Step 5: Dispatch at the top of `main`**

In `crates/zoid/src/main.rs`, insert as the **first statements** inside `async fn main() -> Result<()> {` (line 484), before `let path = db_path()?;`:

```rust
    match zoid::cli::parse_args(std::env::args().skip(1)) {
        zoid::cli::Cli::Version => {
            println!("{}", zoid::cli::version_string());
            return Ok(());
        }
        zoid::cli::Cli::Help => {
            println!("{}", zoid::cli::help_text());
            return Ok(());
        }
        zoid::cli::Cli::Update => {
            return zoid::update::run().await;
        }
        zoid::cli::Cli::Unknown(arg) => {
            eprintln!("zoid: unrecognized argument '{arg}'\n\n{}", zoid::cli::help_text());
            std::process::exit(2);
        }
        zoid::cli::Cli::Run => {}
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid --test cli_test`
Expected: PASS (6 tests).

- [ ] **Step 7: Manually verify the wired binary**

Run:
```bash
cargo run -p zoid -- --version   # prints: zoid 0.1.0
cargo run -p zoid -- --help      # prints usage
cargo run -p zoid -- --bogus; echo "exit=$?"   # prints error + usage to stderr, exit=2
```
Expected: as annotated. (No-arg `cargo run -p zoid` still launches the TUI — exit with `q`.)

- [ ] **Step 8: Verify clippy and fmt, then commit**

```bash
cargo clippy -p zoid --all-targets && cargo fmt --all -- --check
git add crates/zoid/src/cli.rs crates/zoid/src/lib.rs crates/zoid/src/main.rs crates/zoid/tests/cli_test.rs
git commit -m "feat(cli): --version/--help/update dispatch (hand-rolled, no clap)"
```

---

### Task 4: Adopt cargo-dist and configure the build matrix

Generates the release workflow that cross-compiles the three targets and produces archives + `SHA256SUMS`. Publishing to the public repo is wired in Task 5; this task pins targets, archive formats, and installers, and defers release creation.

**Files:**
- Modify: `Cargo.toml` (root — `[workspace.metadata.dist]` written by `dist init`, then edited)
- Create: `.github/workflows/release.yml` (generated by `dist generate`)
- Create/Modify: `.gitignore` (dist may add `target/distrib/`)

**Interfaces:**
- Produces: CI that, on a `v*` tag, builds `zoid-<triple>.tar.gz` (unix), `zoid-<triple>.zip` (windows), and `SHA256SUMS`; it does NOT create a release in the private repo (`create-release = false`), leaving publication to Task 5's custom job.

- [ ] **Step 1: Install cargo-dist**

Run: `cargo install cargo-dist --locked`
Then record the version: `dist --version`
Expected: `dist` on PATH (note the exact version in the commit message; config key names below are stable across recent 0.x releases but confirm against `dist --help` if the installed version differs).

- [ ] **Step 2: Initialize dist config**

Run: `dist init --yes`
Expected: writes `[workspace.metadata.dist]` into root `Cargo.toml` and creates `.github/workflows/release.yml`.

- [ ] **Step 3: Edit the dist config to pin our decisions**

In root `Cargo.toml`, set the `[workspace.metadata.dist]` block to include exactly these keys (merge with what `dist init` wrote; overwrite conflicting values):

```toml
[workspace.metadata.dist]
cargo-dist-version = "0.28.0"        # match the installed `dist --version`
ci = "github"
installers = ["shell", "powershell"]
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
]
unix-archive = ".tar.gz"
windows-archive = ".zip"
# Do NOT create a Release in this private source repo; Task 5's custom job
# publishes to the public strvmarv/zoid-releases instead.
create-release = false
publish-jobs = ["./publish-public"]
install-path = "CARGO_HOME"
```

> musl/libgit2 note: if the `x86_64-unknown-linux-musl` leg fails to link libgit2 in CI, the documented fallback is to replace it with `x86_64-unknown-linux-gnu` and add `github-build-setup` to run on `ubuntu-20.04` (old glibc floor). Update `asset_name`/targets consistently if you switch.

- [ ] **Step 4: Regenerate the workflow**

Run: `dist generate`
Expected: `.github/workflows/release.yml` updated to reflect the three targets and the `publish-public` custom job reference.

- [ ] **Step 5: Validate the plan locally**

Run: `dist plan`
Expected: prints a build plan listing exactly the three archives (`.tar.gz` ×2, `.zip` ×1) and `SHA256SUMS`, with no target other than the three configured. If `dist plan` errors that `publish-public` is missing, that is expected until Task 5 creates the reusable workflow — proceed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml .github/workflows/release.yml .gitignore
git commit -m "ci(release): cargo-dist matrix (musl/macos-arm64/windows), archives pinned, release deferred"
```

---

### Task 5: Cross-repo publish, public repo, and release runbook

Wires the custom publish job that lands the Release in the public repo, creates the public repo + CI secret, documents the process, and cuts a smoke release to prove the end-to-end path.

**Files:**
- Create: `.github/workflows/publish-public.yml`
- Create: `docs/RELEASING.md`
- Create: `NOTES.md` (curated release notes for v0.1.0)

**Interfaces:**
- Consumes: dist artifacts from Task 4; the `RELEASES_REPO_TOKEN` Actions secret.
- Produces: a GitHub Release at `strvmarv/zoid-releases` carrying the three archives + `SHA256SUMS`, which `zoid update` (Task 2) consumes.

- [ ] **Step 1: Create the public releases repo (maintainer action)**

Run (requires the maintainer's `gh` auth; a CI subagent cannot do this):
```bash
gh repo create strvmarv/zoid-releases --public \
  --description "Prebuilt zoid binaries (source lives in the private strvmarv/zoid)."
```
Expected: the empty public repo exists. Add a short `README.md` to it explaining it is the distribution point (no source).

- [ ] **Step 2: Create the least-privilege token and store it as a secret (maintainer action)**

Create a fine-grained PAT scoped to **`strvmarv/zoid-releases` only**, permission **Contents: Read and write**. Then:
```bash
gh secret set RELEASES_REPO_TOKEN --repo strvmarv/zoid --body "<the-fine-grained-PAT>"
```
Expected: `RELEASES_REPO_TOKEN` present in the private repo's Actions secrets. (Rotation: when it expires the publish job fails loudly; regenerate the PAT and re-run this command.)

- [ ] **Step 3: Author the custom publish workflow**

Create `.github/workflows/publish-public.yml` (a reusable workflow invoked by dist's `publish-jobs`):

```yaml
name: publish-public

# Invoked by cargo-dist's publish stage (publish-jobs = ["./publish-public"]).
on:
  workflow_call:
    inputs:
      plan:
        required: true
        type: string

jobs:
  publish-public:
    runs-on: ubuntu-latest
    steps:
      - name: Download built artifacts
        uses: actions/download-artifact@v4
        with:
          # cargo-dist uploads all built archives + SHA256SUMS under this name.
          name: artifacts
          path: dist-artifacts

      - name: Publish release to public repo
        env:
          GH_TOKEN: ${{ secrets.RELEASES_REPO_TOKEN }}
          TAG: ${{ github.ref_name }}
        run: |
          set -euo pipefail
          # Prefer a curated NOTES.md if present; else a minimal generated body.
          if [ -f NOTES.md ]; then NOTES_ARG=(--notes-file NOTES.md); else NOTES_ARG=(--notes "zoid ${TAG}"); fi
          gh release create "$TAG" \
            --repo strvmarv/zoid-releases \
            --title "$TAG" \
            "${NOTES_ARG[@]}" \
            dist-artifacts/*
```

> If the installed cargo-dist version does not pass `artifacts` under that name, run one real tag (Step 6) and read the upload step's artifact name from the failed `release.yml` run, then update `name:` accordingly. This is the one tool-coupling to confirm empirically; everything else is fixed.

- [ ] **Step 4: Write the release runbook**

Create `docs/RELEASING.md`:

```markdown
# Releasing zoid

Source is private (`strvmarv/zoid`); binaries are published to the public
`strvmarv/zoid-releases`. Users self-update with `zoid update` (anonymous,
checksum-verified). No tokens live on user machines.

## Cut a release

1. Bump the version: edit `[workspace.package] version` in the root `Cargo.toml`.
2. Write release notes in `NOTES.md` by hand. Do NOT paste the private commit
   log — this file becomes public.
3. Commit: `git commit -am "release: vX.Y.Z"`.
4. Tag and push: `git tag vX.Y.Z && git push origin main --tags`.
5. CI (`release.yml`) cross-compiles the three targets and the `publish-public`
   job creates the Release at `strvmarv/zoid-releases`.
6. Verify: the public repo's Releases page shows three archives + `SHA256SUMS`.
7. Smoke-test: on a machine with an older zoid, run `zoid update`.

## First-time install (new machine)

Use the installer from the latest release on `strvmarv/zoid-releases`
(shell on unix, PowerShell on Windows). Afterwards, `zoid update` handles upgrades.

## Token rotation

`RELEASES_REPO_TOKEN` (fine-grained PAT, Contents:write on `zoid-releases`
only) lives in the private repo's Actions secrets. On expiry the publish job
fails loudly; regenerate the PAT and:
`gh secret set RELEASES_REPO_TOKEN --repo strvmarv/zoid --body "<PAT>"`.
```

- [ ] **Step 5: Write the v0.1.0 release notes**

Create `NOTES.md`:

```markdown
# zoid v0.1.0

First distributed release.

- Prebuilt binaries for Linux (x86_64, static musl), macOS (Apple Silicon), and Windows (x86_64).
- `zoid update` — anonymous, checksum-verified self-update.
- `zoid --version` / `zoid --help`.
```

- [ ] **Step 6: Commit, then cut a smoke release (maintainer action)**

```bash
git add .github/workflows/publish-public.yml docs/RELEASING.md NOTES.md
git commit -m "ci(release): cross-repo publish to public zoid-releases + runbook"
```

Then prove the pipeline with a throwaway prerelease before the real tag:
```bash
git tag v0.0.1-test && git push origin v0.0.1-test
```
Expected: `release.yml` runs green; the `publish-public` job creates a `v0.0.1-test` release on `strvmarv/zoid-releases` with three archives + `SHA256SUMS`. Then, from a build of this branch, `ZOID_TARGET`-matching `zoid update` upgrades to it.

- [ ] **Step 7: Clean up the smoke release and cut v0.1.0**

```bash
gh release delete v0.0.1-test --repo strvmarv/zoid-releases --yes
git push --delete origin v0.0.1-test
git tag v0.1.0 && git push origin v0.1.0
```
Expected: the real `v0.1.0` release appears on the public repo; `zoid update` on an older binary upgrades to `0.1.0`.

---

## Notes for the executor

- Tasks 1–3 are pure Rust with fast unit tests — ideal for a cheap-model implementer.
- Tasks 4–5 involve GitHub Actions and one-time repo/secret setup that require the **maintainer's** credentials (marked "maintainer action"); a subagent should implement the files and stop at steps needing `gh` auth or PAT creation, handing those to the human.
- The only empirical tool-coupling to confirm is cargo-dist's uploaded-artifact name in `publish-public.yml` Step 3 (verified by the Step 6 smoke run). Everything else is fully specified.
