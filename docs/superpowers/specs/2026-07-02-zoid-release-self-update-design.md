# zoid Release & Self-Update — Design

**Date:** 2026-07-02
**Status:** Approved (design), pending implementation plan
**Author:** strvmarv (with Claude)

## Goal

Establish a repeatable release + self-update pipeline for zoid: tag a version →
CI cross-compiles the shipping binary for three targets → publishes a GitHub
Release to a **public** distribution repo → trusted users run `zoid update` to
self-upgrade anonymously with checksum verification. The source repository
(`strvmarv/zoid`) stays private.

## Scoping decisions (locked)

| Decision | Choice | Rationale |
|---|---|---|
| Audience | Small trusted group (N machines) | Not just the author; not public-facing. |
| Targets | musl-static Linux x86_64, macOS arm64, Windows x86_64 | 3 targets; musl-static supersedes a separate glibc build. |
| Update model | Built-in `zoid update` (user-invoked; zoid is not a service) | No daemon/timer — zoid is a TUI users launch, not a hosted process. |
| Distribution | **Private code, public releases repo** (`strvmarv/zoid-releases`) | Anonymous downloads → zero tokens on user machines; code stays private. |
| Self-updater | Hand-rolled, anonymous (reuses `reqwest`+rustls) | Public assets remove all auth complexity; simpler than Billy's updater. |
| Build tooling | cargo-dist for the matrix + a custom cross-repo publish job | cargo-dist absorbs the cross-compile toolchains; custom job lands the release in the public repo. |
| First install | cargo-dist shell + PowerShell installer scripts | Bootstraps a new machine; `zoid update` handles every upgrade after. |

### Why a public releases repo over per-machine PAT

zoid has N user machines, unlike Billy's single container. A per-machine PAT
means N secrets to plant and rotate. A **public** releases repo makes downloads
**anonymous**, which deletes the entire authenticated-download layer (Bearer
headers, asset-API URLs, keyring plumbing, PAT rotation) from the client side.
The one secret that remains lives in CI (write access to the public repo), in a
single place.

Costs accepted:
1. One CI secret — a least-privilege token (`contents:write` on
   `zoid-releases` only), stored as an Actions secret in the private repo.
2. **Curated release notes** — version numbers, binary sizes, and changelog are
   world-readable. Release notes are written deliberately; they are **never**
   auto-generated from the private commit log (which could leak internal detail).
3. The **existence** of a project named "zoid" becomes public (a discoverable
   public repo). Source stays private; the name does not.

## Architecture

```
git tag vX.Y.Z ─► cargo-dist workflow (build matrix) ─► archives + SHA256SUMS
                                                            │
                        custom publish job (RELEASES_REPO_TOKEN)
                                                            ▼
                              gh release create --repo strvmarv/zoid-releases
                                                            │
   user: zoid update ─► GET latest release (anon) ─► pick asset ─► verify sha256 ─► atomic swap (+.bak)
```

## Components

The work decomposes into four cohesive chunks (A–D), one implementation plan.

### A. Version surface (new — zoid currently has no CLI arg layer)

zoid today reads only environment variables in `crates/zoid/src/main.rs`; there
is no argument parsing and no way to ask "what version am I running?" — the first
thing needed when supporting a distributed binary.

- **Workspace-shared version.** Add `[workspace.package] version = "0.1.0"` to
  the root `Cargo.toml` and set each shipping crate to `version.workspace = true`.
  Bumping the release version becomes a one-line edit. `zoid-testkit` may keep
  its own `0.1.0` if desired (dev-only, not shipped), but sharing is simpler.
- **Minimal hand-rolled arg parser** in a new `crates/zoid/src/cli.rs` — **not
  `clap`**. Three flags and one subcommand do not justify the dependency, and a
  small parser fits the repo's minimalist ethos. Shape:
  ```
  enum Cli { Run, Version, Help, Update }
  fn parse_args(args: impl Iterator<Item = String>) -> Cli
  ```
  - `zoid --version` / `-V` → prints `zoid X.Y.Z` (from `CARGO_PKG_VERSION`), exits 0.
  - `zoid --help` / `-h` → short usage text, exits 0.
  - `zoid update` → runs the self-updater (component B); does **not** launch the TUI.
  - no args → launches the TUI exactly as today.
  - unknown args → usage to stderr, exit 2.
- **Target-triple embed.** A new `crates/zoid/build.rs` reads the build script's
  `TARGET` env var and emits `cargo:rustc-env=ZOID_TARGET=<triple>` so the
  updater can read `env!("ZOID_TARGET")` at runtime to pick the right asset.

`parse_args` is a pure function and is unit-tested independently of `main`.

### B. `zoid update` self-updater (new — `crates/zoid/src/update.rs`)

Anonymous, checksum-verified self-replace. Structured as a **pure core + thin
imperative shell** so the logic is deterministic and testable.

Pure functions (unit-tested):
- `is_newer(current: &str, latest: &str) -> bool` — semver-ish tag compare.
- `asset_name(target: &str) -> String` — maps `ZOID_TARGET` to the published
  asset filename (e.g. `zoid-x86_64-unknown-linux-musl.tar.gz`,
  `zoid-x86_64-pc-windows-msvc.zip`).
- `verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<()>` — recompute and
  compare; error on mismatch.
- `parse_sha256sums(text: &str) -> HashMap<String, String>` — filename → hex.

Imperative shell (integration-tested where portable):
1. Read current version (`CARGO_PKG_VERSION`) and running-exe path
   (`std::env::current_exe()`).
2. `GET https://api.github.com/repos/strvmarv/zoid-releases/releases/latest`
   (anonymous; sets a `User-Agent` header, which the GitHub API requires). Reuses
   the existing `reqwest` client (rustls-tls).
3. If the release tag is not newer than the current version → print "already up
   to date (vX.Y.Z)" and exit 0.
4. Select the asset for `ZOID_TARGET`; download the archive and `SHA256SUMS`
   (anonymous asset URLs).
5. Verify the archive's sha256 against `SHA256SUMS`. Abort on mismatch (no swap).
6. Extract the `zoid` binary from the archive (`tar.gz` on unix, `zip` on
   windows).
7. **Atomic self-replace**, keeping a `.bak`:
   - **Unix:** write the new binary to a temp file beside the current exe, `chmod
     0755`, `rename` current → `<exe>.bak`, `rename` temp → current exe.
   - **Windows:** a running exe cannot be overwritten in place, but it **can** be
     renamed. Rename current → `<exe>.bak`, then move the new binary into the
     original path.
8. Print success + new version. (No TUI restart concern — `update` is its own
   subcommand and never had the TUI running.)

New dependencies (the real cost of this component):
- `sha2` — checksum verification.
- `flate2` + `tar` — extract `.tar.gz` on unix.
- `zip` — extract `.zip` on windows, behind `#[cfg(windows)]`.

Error handling — every failure yields a clear message, a non-zero exit, and
**no partial swap**; the `.bak` is retained for manual rollback:
- network/HTTP failure → "could not reach releases repo: …".
- no asset matching `ZOID_TARGET` → "no release asset for <triple>".
- checksum mismatch → "checksum verification failed; aborting" (never swap).
- permission denied on swap (binary in a root-owned location) → "cannot replace
  <path>: permission denied; re-run with appropriate privileges".

### C. cargo-dist + custom cross-repo publish (`.github/workflows/release.yml`)

- **`dist init`** writes the dist config (`[workspace.metadata.dist]` or
  `dist-workspace.toml`, per cargo-dist version) and the release workflow.
- **Targets:** `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`.
  - **musl/libgit2 risk & fallback:** `git2`/libgit2 is the classic static-musl
    pain point. Musl is primary; **if** the musl leg proves intractable in CI,
    the documented fallback is to ship `x86_64-unknown-linux-gnu` built on an
    old-glibc runner (e.g. `ubuntu-20.04`) so the glibc floor stays low.
- **Installers:** enable cargo-dist's shell (`curl … | sh`) and PowerShell
  installer scripts for **first-install bootstrap** (they work anonymously
  against the public repo). `zoid update` handles all subsequent upgrades.
- **Cross-repo publish:** cargo-dist builds and uploads the archives +
  `SHA256SUMS` as CI artifacts. A downstream **publish job** downloads them and
  runs `gh release create --repo strvmarv/zoid-releases vX.Y.Z ./dist/* \
  --notes-file NOTES.md`, authenticated with the `RELEASES_REPO_TOKEN` secret.
  The publish job is gated on **all build legs succeeding**, so a failed target
  never produces a partial release. (If a given cargo-dist version supports
  native cross-repo hosting to a different repo, that may replace the custom job;
  the custom job is the robust default and the source of truth for this design.)
- **Tag/version guard:** the workflow fails fast if the pushed tag `vX.Y.Z` does
  not match the workspace crate version.

### D. Public releases repo, CI token, and runbook

- **`strvmarv/zoid-releases`** — created once (`gh repo create strvmarv/zoid-releases
  --public`). Releases-only: a `README.md` explaining it is the distribution
  point, an appropriate `LICENSE` for the binaries, and the GitHub Releases. No
  source.
- **CI token** — a fine-grained PAT (or GitHub App installation token) with
  **`contents:write` on `zoid-releases` only**, stored as the Actions secret
  `RELEASES_REPO_TOKEN` in the private `zoid` repo. Least privilege; single
  location.
- **Release runbook** (`docs/RELEASING.md`): bump `[workspace.package] version`,
  write `NOTES.md`, commit, `git tag vX.Y.Z`, `git push --tags` → CI runs → verify
  the public release. Includes PAT rotation notes (token expiry fails the publish
  job loudly).

## Data flow (end to end)

1. Maintainer bumps `[workspace.package] version`, writes release notes, commits,
   pushes tag `vX.Y.Z`.
2. cargo-dist workflow cross-compiles the three targets, producing per-target
   archives and a combined `SHA256SUMS`.
3. Publish job (gated on all builds green) creates the GitHub Release in
   `strvmarv/zoid-releases` using `RELEASES_REPO_TOKEN`.
4. A user runs `zoid update`: anonymous GET of the latest release → asset
   selected by `ZOID_TARGET` → download + sha256 verify → extract → atomic swap
   (+`.bak`) → success message.

## Error handling summary

- **CI:** tag≠version fails fast; any failed build leg blocks the publish job (no
  partial releases).
- **Update:** network / missing-asset / checksum-mismatch / permission-denied →
  clear message, non-zero exit, no partial swap, `.bak` retained.

## Testing strategy

- **Pure unit tests:** `parse_args` (each flag/subcommand/unknown); `is_newer`
  (older/equal/newer, and a pre-release edge); `asset_name` per target;
  `parse_sha256sums`; `verify_sha256` (valid **and** tampered-bytes cases).
- **Archive extraction:** a fixture `.tar.gz` (and `.zip` under `#[cfg(windows)]`)
  containing a stub `zoid` binary, extracted to a temp dir and asserted.
- **Swap (Linux-only integration test):** in a temp dir, place a fake "current"
  exe and a fake "new" binary, run the swap routine, assert the swap and the
  `.bak`.
- **`zoid --version`:** assert the printed string matches `CARGO_PKG_VERSION`
  (test the pure `version_string()` helper; optionally spawn the built binary).
- **Workflow:** `dist plan` validates the matrix locally; a throwaway
  `v0.0.1-test` prerelease published to `zoid-releases` (then deleted) is the
  end-to-end smoke test before the first real tag.

## Out of scope (YAGNI)

- crates.io publishing, Homebrew tap, AUR packaging (not needed for a trusted
  group; can be added later).
- Automatic/background update checks or a timer (zoid is user-invoked, not a
  service).
- Rollback automation beyond the retained `.bak` (manual restore is sufficient
  for this audience).
- Code signing / notarization (macOS Gatekeeper friction acceptable for a
  trusted group; revisit if the audience widens).

## Implementation chunks → plan tasks

1. **Version surface** — workspace version bump, `build.rs` target embed,
   `cli.rs` arg layer + dispatch, tests.
2. **`zoid update`** — `update.rs` pure core + imperative shell, new deps, tests.
3. **Release CI** — `dist init`, `release.yml`, custom cross-repo publish job,
   tag/version guard.
4. **Public repo & runbook** — create `zoid-releases`, `RELEASES_REPO_TOKEN`,
   `docs/RELEASING.md`, first `v0.1.0` release.
