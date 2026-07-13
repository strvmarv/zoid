# Community Plugin Catalog (Spec 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let zoid discover + install curated `mode`/`skills` plugins from the public `strvmarv/zoid-releases` catalog via a `:plugin` browse overlay with a provenance-confirm gate.

**Architecture:** A new `crates/zoid/src/catalog.rs` fetches/caches/parses `zoid-releases/plugins/index.json` (24h TTL, unauthenticated raw). The Spec 1 install pipeline is reused; the one structural change is that the resolved `PluginManifest` is **carried through** `AgentUpdate::PluginScan` instead of re-derived from `bundled_manifest`, so catalog manifests install. A new `Overlay::PluginCatalog` browses the cached list and confirms provenance before install; `:plugin list` is the text surface. The catalog files + a Python `gen_index.py` + CI live in `zoid-releases`.

**Tech Stack:** Rust (zoid workspace), `reqwest` (already a dep), `serde_json`, `chrono`; Python 3.11 stdlib `tomllib` (zoid-releases CI).

**Spec:** `docs/superpowers/specs/2026-07-13-plugin-catalog-design.md`

## Global Constraints

- **Catalog kinds are `mode` and `skills` only.** MCP catalog entries are deferred to Spec 2.5. Do not add an mcp install path here.
- **Unauthenticated raw fetch** from `https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins/...`. The catalog fetch never uses the GitHub API or a token. (The unchanged upstream-tree fetch still uses `github_fetch` with optional `$GITHUB_TOKEN`.)
- **24h TTL cache** at `resolve_cache_dir(env).join("catalog/")`. Clock + env + fetcher are **injected** for deterministic tests — no real network in unit tests.
- **Carry the resolved manifest** through `AgentUpdate::PluginScan`; `apply_plugin_scan` must stop calling `bundled_manifest(id)` and use the carried manifest. Bundled resolution stays as the fast path.
- **Provenance + confirm** before any install: show `source repo@sha`, kind, license; require `y`.
- **`index.json` is generated**, never hand-edited; `gen_index.py` output is deterministic (keys sorted by `id`, stable field order, trailing newline).
- **`zoid` is private; `zoid-releases` is public.** Files authored for `zoid-releases` must never reference private zoid internals.
- Never add a `Co-Authored-By`/co-author trailer to commits.
- Superpowers stays the sole compiled-in bundled manifest; the golden body stays byte-identical (no task here touches it).

---

## File Structure

- `crates/zoid/src/catalog.rs` — NEW. Pure `parse_index` + `CatalogEntry` + URL builders (Task 1); `fetch_catalog` TTL cache with injected clock/env/fetcher (Task 2).
- `crates/zoid/src/lib.rs` / `main.rs` — declare `mod catalog;`.
- `crates/zoid-plugin/src/resolve.rs` — add `ManifestSource::Catalog`; `Id` arm bundled-else-catalog (Task 3).
- `crates/zoid/src/agent.rs` — add `manifest` field to `AgentUpdate::PluginScan` (Task 3).
- `crates/zoid/src/main.rs` — `install_plugin` (bundled-else-catalog, carry manifest), `apply_plugin_scan` (use carried manifest), overlay open + row mapping (Task 3, 5), `:plugin list` handler (Task 4).
- `crates/zoid-tui/src/command.rs` — `Command::PluginList` + `Command::PluginCatalog`; remap bare `:plugin` (Task 4).
- `crates/zoid-tui/src/state.rs` — `Overlay::PluginCatalog`, `PluginCatalogRow`, `PluginCatalogState` (Task 5).
- `crates/zoid-tui/src/<render>` — render the overlay (Task 5).
- `crates/zoid/tests/fixtures/catalog/` — sample `index.json` + `<id>.toml` (Tasks 1–3).
- `zoid-releases/` deliverables authored under `contrib/zoid-releases-catalog/` in the zoid repo for transplant (Task 6): `plugins/README.md`, `scripts/gen_index.py`, `.github/workflows/catalog-index.yml`, a seed `plugins/<id>.toml` + generated `plugins/index.json`.

---

### Task 1: Catalog types + pure parse + URL builders

**Files:**
- Create: `crates/zoid/src/catalog.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod catalog;`) and `crates/zoid/src/main.rs` if it declares modules separately
- Test: inline `#[cfg(test)]` in `catalog.rs`

**Interfaces:**
- Produces:
  - `pub struct CatalogEntry { pub id: String, pub name: String, pub kind: Vec<String>, pub description: String, pub license: Option<String>, pub source_repo: String, pub source_ref: String }`
  - `pub fn parse_index(json: &str) -> anyhow::Result<Vec<CatalogEntry>>`
  - `pub fn catalog_index_url() -> String`
  - `pub fn catalog_manifest_url(id: &str) -> String`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "schema": 1,
      "plugins": [
        { "id": "ok-skills", "name": "OK Skills", "kind": ["skills"],
          "description": "Curated pack.", "license": "MIT",
          "source": { "repo": "mxyhi/ok-skills", "ref": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0" } },
        { "id": "superpowers", "name": "Superpowers", "kind": ["mode"],
          "description": "Workflows.",
          "source": { "repo": "obra/superpowers", "ref": "d884ae04edebef577e82ff7c4e143debd0bbec99" } }
      ]
    }"#;

    #[test]
    fn parse_index_reads_entries() {
        let v = parse_index(SAMPLE).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "ok-skills");
        assert_eq!(v[0].kind, vec!["skills".to_string()]);
        assert_eq!(v[0].license.as_deref(), Some("MIT"));
        assert_eq!(v[0].source_repo, "mxyhi/ok-skills");
        assert_eq!(v[1].license, None);
    }

    #[test]
    fn parse_index_rejects_wrong_schema() {
        let bad = r#"{ "schema": 2, "plugins": [] }"#;
        assert!(parse_index(bad).is_err());
    }

    #[test]
    fn parse_index_rejects_missing_source() {
        let bad = r#"{ "schema": 1, "plugins": [ { "id": "x", "name": "X", "kind": ["mode"], "description": "d" } ] }"#;
        assert!(parse_index(bad).is_err());
    }

    #[test]
    fn urls_are_raw_unauthenticated() {
        assert_eq!(catalog_index_url(),
            "https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins/index.json");
        assert_eq!(catalog_manifest_url("ok-skills"),
            "https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins/ok-skills.toml");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid catalog::tests`
Expected: FAIL — `catalog` module/functions undefined.

- [ ] **Step 3: Implement**

```rust
//! Fetch + cache + parse the public zoid-releases plugin catalog.
//! Unauthenticated raw.githubusercontent.com; see Spec 2 design.

use serde::Deserialize;

const CATALOG_BASE: &str =
    "https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub kind: Vec<String>,
    pub description: String,
    pub license: Option<String>,
    pub source_repo: String,
    pub source_ref: String,
}

#[derive(Deserialize)]
struct RawIndex {
    schema: u32,
    #[serde(default)]
    plugins: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Vec<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    license: Option<String>,
    source: RawSource,
}

#[derive(Deserialize)]
struct RawSource {
    repo: String,
    #[serde(rename = "ref")]
    ref_: String,
}

/// Parse an index.json. Rejects an unknown top-level schema; installability of
/// each kind is validated later when the manifest itself is fetched + validated.
pub fn parse_index(json: &str) -> anyhow::Result<Vec<CatalogEntry>> {
    let raw: RawIndex = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("catalog index parse error: {e}"))?;
    anyhow::ensure!(raw.schema == 1, "unsupported catalog index schema {}", raw.schema);
    Ok(raw.plugins.into_iter().map(|e| CatalogEntry {
        id: e.id,
        name: e.name,
        kind: e.kind,
        description: e.description,
        license: e.license,
        source_repo: e.source.repo,
        source_ref: e.source.ref_,
    }).collect())
}

pub fn catalog_index_url() -> String {
    format!("{CATALOG_BASE}/index.json")
}

pub fn catalog_manifest_url(id: &str) -> String {
    format!("{CATALOG_BASE}/{id}.toml")
}
```

Add `pub mod catalog;` to `crates/zoid/src/lib.rs` — `github_fetch` and `plugin_install` are already `pub mod` there and referenced as `zoid::…` from the bin; `catalog` follows the same convention (the bin uses `zoid::catalog::…`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid catalog::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/catalog.rs crates/zoid/src/lib.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): catalog index types + pure parse + raw URL builders"
```

---

### Task 2: Catalog fetch with 24h TTL cache (injected clock/env/fetcher)

**Files:**
- Modify: `crates/zoid/src/catalog.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `parse_index`, `catalog_index_url` (Task 1). The **bin** owns `resolve_cache_dir` (`main.rs:75`) and passes the resolved `catalog/` dir INTO the functions below — a lib module never imports a bin fn, so do not try to reach `resolve_cache_dir` from `catalog.rs`.
- Produces (a sync cache half + a test-only orchestrator; the REAL network call is async and done by the caller):
  - `pub fn cache_if_fresh(now: DateTime<Utc>, ttl: Duration, cache_dir: &Path) -> Option<Vec<CatalogEntry>>` — returns the cached, parseable index iff the stamp is younger than `ttl`; else `None`.
  - `pub fn store_and_parse(now: DateTime<Utc>, cache_dir: &Path, body: &str) -> anyhow::Result<Vec<CatalogEntry>>` — parse FIRST (so garbage never clobbers a good cache), then write `index.json` + stamp, return entries.
  - `pub fn cached_any(cache_dir: &Path) -> Option<Vec<CatalogEntry>>` — any parseable cache regardless of age (stale fallback).
  - `pub trait IndexFetcher { fn get(&self, url: &str) -> anyhow::Result<String>; }` — **test-only** seam.
  - `pub fn load_catalog(now, ttl, cache_dir, fetcher: &dyn IndexFetcher) -> anyhow::Result<Vec<CatalogEntry>>` — sync orchestrator over the trait (`cache_if_fresh` → `fetcher.get` → `store_and_parse`, stale fallback via `cached_any`). Used by the unit tests below AND documents the exact control flow the async caller replicates.

**Design note (CRITICAL — async, no blocking client):** workspace `reqwest` is `default-features = false, features = ["json","stream","rustls-tls"]` — there is **no `blocking` feature**, and calling `block_on` inside the tokio task that also runs the async `fetch_tree` would nest runtimes and panic. So the real fetch is **async in the caller**: `if let Some(v) = cache_if_fresh(now, ttl, dir) { use it } else { let body = client.get(url).send().await?.text().await?; store_and_parse(now, dir, &body) }`, with `cached_any(dir)` as the on-error fallback. The `IndexFetcher` trait exists ONLY so the five TTL tests are deterministic and network-free; production never constructs a blocking fetcher.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod cache_tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    struct FakeFetcher { body: std::cell::RefCell<Option<String>>, calls: std::cell::Cell<u32> }
    impl FakeFetcher { fn new(body: Option<&str>) -> Self {
        Self { body: std::cell::RefCell::new(body.map(str::to_string)), calls: std::cell::Cell::new(0) } } }
    impl IndexFetcher for FakeFetcher {
        fn get(&self, _url: &str) -> anyhow::Result<String> {
            self.calls.set(self.calls.get() + 1);
            self.body.borrow().clone().ok_or_else(|| anyhow::anyhow!("network down"))
        }
    }

    const IDX: &str = r#"{ "schema":1, "plugins":[ { "id":"a","name":"A","kind":["mode"],"description":"d","source":{"repo":"o/a","ref":"deadbeef"} } ] }"#;

    #[test]
    fn fetches_and_caches_when_no_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new(Some(IDX));
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let v = load_catalog(now, Duration::hours(24), tmp.path(), &f).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(f.calls.get(), 1);
        assert!(tmp.path().join("index.json").is_file());
    }

    #[test]
    fn reuses_fresh_cache_without_fetching() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        // Prime the cache via a first fetch.
        load_catalog(now, Duration::hours(24), tmp.path(), &FakeFetcher::new(Some(IDX))).unwrap();
        // A fetcher that would ERROR if called; fresh cache must avoid it.
        let f2 = FakeFetcher::new(None);
        let v = load_catalog(now + Duration::hours(1), Duration::hours(24), tmp.path(), &f2).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(f2.calls.get(), 0, "fresh cache must not fetch");
    }

    #[test]
    fn refetches_when_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        load_catalog(now, Duration::hours(24), tmp.path(), &FakeFetcher::new(Some(IDX))).unwrap();
        let f2 = FakeFetcher::new(Some(IDX));
        load_catalog(now + Duration::hours(25), Duration::hours(24), tmp.path(), &f2).unwrap();
        assert_eq!(f2.calls.get(), 1, "stale cache must refetch");
    }

    #[test]
    fn falls_back_to_stale_cache_on_network_error() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        load_catalog(now, Duration::hours(24), tmp.path(), &FakeFetcher::new(Some(IDX))).unwrap();
        let v = load_catalog(now + Duration::hours(25), Duration::hours(24), tmp.path(),
            &FakeFetcher::new(None)).unwrap();
        assert_eq!(v.len(), 1, "network down but stale cache serves");
    }

    #[test]
    fn errors_when_no_cache_and_network_down() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        assert!(load_catalog(now, Duration::hours(24), tmp.path(), &FakeFetcher::new(None)).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid catalog::cache_tests`
Expected: FAIL — `load_catalog`/`IndexFetcher` undefined.

- [ ] **Step 3: Implement**

```rust
use std::path::{Path, PathBuf};
use chrono::{DateTime, Duration, Utc};

pub trait IndexFetcher {
    fn get(&self, url: &str) -> anyhow::Result<String>;
}

fn cache_file(dir: &Path) -> PathBuf { dir.join("index.json") }
fn stamp_file(dir: &Path) -> PathBuf { dir.join("index.json.fetched") }

fn read_stamp(dir: &Path) -> Option<DateTime<Utc>> {
    let s = std::fs::read_to_string(stamp_file(dir)).ok()?;
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.with_timezone(&Utc))
}

/// Cached, parseable index iff the stamp is younger than `ttl`.
pub fn cache_if_fresh(now: DateTime<Utc>, ttl: Duration, cache_dir: &Path) -> Option<Vec<CatalogEntry>> {
    let fresh = read_stamp(cache_dir).map(|t| now - t < ttl).unwrap_or(false);
    if !fresh { return None; }
    let cached = std::fs::read_to_string(cache_file(cache_dir)).ok()?;
    parse_index(&cached).ok()
}

/// Parse a freshly-fetched body FIRST (so garbage never clobbers a good cache),
/// then write index + stamp, and return the entries.
pub fn store_and_parse(now: DateTime<Utc>, cache_dir: &Path, body: &str) -> anyhow::Result<Vec<CatalogEntry>> {
    let v = parse_index(body)?;
    std::fs::create_dir_all(cache_dir).ok();
    std::fs::write(cache_file(cache_dir), body).ok();
    std::fs::write(stamp_file(cache_dir), now.to_rfc3339()).ok();
    Ok(v)
}

/// Any parseable cache regardless of age (stale fallback when the network fails).
pub fn cached_any(cache_dir: &Path) -> Option<Vec<CatalogEntry>> {
    let cached = std::fs::read_to_string(cache_file(cache_dir)).ok()?;
    parse_index(&cached).ok()
}

/// Test-only sync orchestrator (over the `IndexFetcher` seam). The REAL caller
/// replicates this control flow with an async `reqwest` GET in place of
/// `fetcher.get` (see the CRITICAL design note above).
pub fn load_catalog(
    now: DateTime<Utc>,
    ttl: Duration,
    cache_dir: &Path,
    fetcher: &dyn IndexFetcher,
) -> anyhow::Result<Vec<CatalogEntry>> {
    if let Some(v) = cache_if_fresh(now, ttl, cache_dir) {
        return Ok(v);
    }
    match fetcher.get(&catalog_index_url()) {
        Ok(body) => store_and_parse(now, cache_dir, &body),
        Err(e) => cached_any(cache_dir).ok_or_else(|| anyhow::anyhow!("catalog unavailable: {e}")),
    }
}
```

Add `chrono` to `crates/zoid/Cargo.toml` deps if not present (it is used elsewhere in `zoid`, e.g. `plugin_install` uses `chrono::Utc` — confirm and reuse).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid catalog`
Expected: PASS (Task 1 + Task 2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/catalog.rs crates/zoid/Cargo.toml
git commit -m "feat(zoid): catalog TTL cache with injected clock/env/fetcher + stale fallback"
```

---

### Task 3: Resolution seam — carry the manifest through the install pipeline

**Files:**
- Modify: `crates/zoid-plugin/src/resolve.rs` (add `Catalog` variant)
- Modify: `crates/zoid/src/agent.rs` (`AgentUpdate::PluginScan` gains `manifest`)
- Modify: `crates/zoid/src/main.rs` (`install_plugin`, `apply_plugin_scan`)
- Test: `resolve.rs` inline; `main.rs` inline (`apply_plugin_scan` with a carried catalog manifest)

**Interfaces:**
- `ManifestSource` gains `Catalog`. `resolve_source` `Id` arm: `Bundled` if known, else `Catalog` (was `WizardFallback`).
- `AgentUpdate::PluginScan { id, origin, over, res: Result<(PluginManifest, UpstreamScan), String> }` — the resolved manifest is folded INTO the `Ok` alongside the scan (NOT a separate non-Option field). This is C1 from the plan review: the catalog manifest is fetched/parsed/validated inside the spawned task, so a manifest-stage failure must be representable as `Err(String)` and travel back through the SAME message that clears the `installing_plugin` guard.
- `apply_plugin_scan` keeps a `res: Result<(PluginManifest, UpstreamScan), String>` param, destructures `(manifest, scan)` in its existing `Ok` arm, and uses that manifest instead of `bundled_manifest(&id)`.

- [ ] **Step 1: Write the failing test (resolve)**

```rust
// in resolve.rs tests
#[test]
fn unknown_id_resolves_to_catalog() {
    let r = PluginRef::Id("ok-skills".into());
    assert_eq!(resolve_source(&r, &["superpowers"], false, false), ManifestSource::Catalog);
}
#[test]
fn known_id_still_bundled() {
    let r = PluginRef::Id("superpowers".into());
    assert_eq!(resolve_source(&r, &["superpowers"], false, false), ManifestSource::Bundled);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin resolve`
Expected: FAIL — `ManifestSource::Catalog` undefined.

- [ ] **Step 3: Implement resolve change**

In `resolve.rs`: add `Catalog` to `ManifestSource`; change the `Id` arm:

```rust
PluginRef::Id(id) => {
    if bundled_ids.contains(&id.as_str()) {
        ManifestSource::Bundled
    } else {
        ManifestSource::Catalog
    }
}
```

- [ ] **Step 4: Fold `(manifest, scan)` into `PluginScan` + async two-branch `install_plugin`**

In `agent.rs`, change `res` to carry the manifest (do NOT add a separate `manifest` field — C1):

```rust
PluginScan {
    id: String,
    origin: String,
    over: crate::plugin_install::KindOverride,
    // Manifest folded into Ok so a manifest-stage failure is representable AND
    // clears the installing_plugin guard through the same message.
    res: Result<(zoid_plugin::manifest::PluginManifest, zoid_core::wizard::UpstreamScan), String>,
},
```

In `main.rs::install_plugin`: resolve, then spawn. The **bundled** branch resolves the manifest synchronously on the main loop and spawns only the tree fetch. The **catalog** branch does the WHOLE thing async inside the spawn — manifest fetch → `parse_manifest` → `validate` → build URL → `parse_github_url` → `fetch_tree` — because (a) a blocking network call on the main loop stalls the UI and (b) `fetch_tree` is async and there is no blocking reqwest client (see Task 2 CRITICAL note). Both branches set `app.installing_plugin = true` before the spawn and send `PluginScan { .., res }`.

```rust
use zoid_plugin::resolve::{classify_ref, resolve_source, ManifestSource, PluginRef};
let r = classify_ref(&arg);
// Reject a bad id up front (M4): the Catalog branch interpolates id into a raw URL.
if let PluginRef::Id(id) = &r {
    if !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.'|'_'|'-')) {
        app.shell.status_hint = Some(format!("invalid plugin id '{id}'")); return;
    }
}
let ui_tx = app.ui_tx.clone();
match (&r, resolve_source(&r, zoid_plugin::bundled::bundled_ids(), false, false)) {
    (PluginRef::Id(id), ManifestSource::Bundled) => {
        let manifest = zoid_plugin::bundled::bundled_manifest(id).expect("bundled id resolves");
        if let Err(e) = manifest.validate() { app.shell.status_hint = Some(e); return; }
        let Some(src) = manifest.source.clone() else { app.shell.status_hint = Some(format!("plugin '{id}' has no [source]")); return; };
        let parsed = match zoid::github_fetch::parse_github_url(&format!("github.com/{}/tree/{}/{}", src.repo, src.ref_, src.subtree)) {
            Ok(p) => p, Err(e) => { app.shell.status_hint = Some(e); return; } };
        app.installing_plugin = true;
        app.shell.status_hint = Some(format!("installing plugin '{id}'…"));
        let (id, over) = (id.clone(), over);
        tokio::spawn(async move {
            let api = zoid::github_fetch::HttpGithubApi::new();
            let res = zoid::github_fetch::fetch_tree(&api, &parsed).await
                .map(|scan| (manifest, scan))
                .map_err(|e| format!("plugin fetch failed: {e}"));
            let _ = ui_tx.send(zoid::agent::AgentUpdate::PluginScan { id, origin: "bundled".into(), over, res }).await;
        });
    }
    (PluginRef::Id(id), ManifestSource::Catalog) => {
        app.installing_plugin = true;
        app.shell.status_hint = Some(format!("installing plugin '{id}'…"));
        let (id, over) = (id.clone(), over);
        tokio::spawn(async move {
            let res: Result<_, String> = async {
                // Async raw GET of <id>.toml (same async reqwest client style as fetch_tree).
                let body = zoid::catalog::fetch_text(&zoid::catalog::catalog_manifest_url(&id)).await
                    .map_err(|e| format!("catalog manifest fetch failed: {e}"))?;
                let manifest = zoid_plugin::manifest::parse_manifest(&body).map_err(|e| e)?;
                manifest.validate().map_err(|e| e)?;
                let src = manifest.source.clone().ok_or_else(|| format!("plugin '{id}' has no [source]"))?;
                let parsed = zoid::github_fetch::parse_github_url(&format!("github.com/{}/tree/{}/{}", src.repo, src.ref_, src.subtree))?;
                let api = zoid::github_fetch::HttpGithubApi::new();
                let scan = zoid::github_fetch::fetch_tree(&api, &parsed).await.map_err(|e| format!("plugin fetch failed: {e}"))?;
                Ok((manifest, scan))
            }.await;
            let _ = ui_tx.send(zoid::agent::AgentUpdate::PluginScan { id, origin: "catalog".into(), over, res }).await;
        });
    }
    (PluginRef::Url(_), _) => {
        app.shell.status_hint = Some("installing plugins from a URL is not supported yet; use a catalog id".into());
    }
    (PluginRef::Id(id), _) => { app.shell.status_hint = Some(format!("unknown plugin '{id}'")); }
}
```

Add a small async raw-GET helper to `catalog.rs` (reused by both the index fetch and the manifest fetch):

```rust
/// One-shot async raw GET of a public zoid-releases text file (unauthenticated).
pub async fn fetch_text(url: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder().user_agent("zoid").build()?;
    Ok(client.get(url).send().await?.error_for_status()?.text().await?)
}
```

In `apply_plugin_scan`, destructure `(manifest, scan)` from the `Ok` and delete the `bundled_manifest` lookup (the guard-clear + error rendering already live in its `res` match):

```rust
fn apply_plugin_scan(
    app: &mut App,
    id: String,
    origin: String,
    over: zoid::plugin_install::KindOverride,
    res: Result<(zoid_plugin::manifest::PluginManifest, zoid_core::wizard::UpstreamScan), String>,
) -> bool {
    app.installing_plugin = false;
    let (mut manifest, scan) = match res {
        Ok(pair) => pair,
        Err(e) => { app.shell.status_hint = Some(e); return false; }
    };
    // DELETE the old `bundled_manifest(&id)` lookup — `manifest` is carried now.
    // ... KindOverride application, build_plan, install, effects — unchanged ...
}
```

Update the dispatch call site (`main.rs:3114`) to `apply_plugin_scan(app, id, origin, over, res)` — `res` is now the tuple result.

- [ ] **Step 5: Fix the THREE existing call sites + add the carried-catalog test (M1, M2)**

Adding the tuple changes three existing positional calls that currently pass `Ok(scan)` — update each to `Ok((manifest, scan))` (build the appropriate manifest inline) so the crate compiles:
- `apply_plugin_scan_reports_honest_status_and_clears_guard` — TWO calls: `main.rs:7000` (success, `Ok((sp_manifest, scan))` with the bundled Superpowers manifest) and `main.rs:7028` (the `Err("fetch failed: boom")` call — unchanged, it's already `Err`).
- `apply_plugin_scan_skills_kind_reports_restart_hint_not_activation_error` — `main.rs:7078`: `Ok((sp_manifest, scan))`.

For the success calls, build the Superpowers manifest via `zoid_plugin::bundled::bundled_manifest("superpowers").unwrap()` (avoids restating the literal). Then add the NEW carried-catalog test proving the path no longer depends on `bundled_manifest`:

```rust
#[tokio::test]
async fn apply_plugin_scan_installs_a_carried_catalog_manifest() {
    use zoid_plugin::manifest::{PluginManifest, PluginSource, ModeRecipe, BodyStrategy};
    use zoid_plugin::effect::Effect;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = test_app().await;
    app.mode_dirs = vec![tmp.path().join("modes")];
    // scan MUST contain skills/using-demo/SKILL.md (the loader) so build_plan succeeds.
    let scan = /* UpstreamScan with skills/using-demo/SKILL.md + one more SKILL.md, mirror the neighbor test */;
    let manifest = PluginManifest {
        id: "demo".into(), schema: 1, kind: vec!["mode".into()],
        name: "Demo".into(), description: "d".into(),
        source: Some(PluginSource { repo: "o/demo".into(), ref_: "SHA".into(), subtree: "skills".into() }),
        mode: Some(ModeRecipe {
            loader: "using-demo/SKILL.md".into(), strip_prefix: "skills/".into(),
            body: BodyStrategy::FromSkillFrontmatter, description: "Demo mode".into(),
            body_intro: None, body_outro: None,        // M2: ModeRecipe has these two fields
        }),
        install: vec![Effect::Activate],
    };
    let activated = apply_plugin_scan(&mut app, "demo".into(), "catalog".into(),
        zoid::plugin_install::KindOverride::None, Ok((manifest, scan)));
    assert!(activated);
    assert!(app.modes.names().iter().any(|n| n == "Demo"));
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid-plugin resolve && cargo test -p zoid apply_plugin_scan && cargo build -p zoid`
Expected: PASS + clean build (all three prior call sites updated).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-plugin/src/resolve.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/src/catalog.rs
git commit -m "feat(zoid): resolve catalog ids + carry resolved manifest through PluginScan"
```

---

### Task 4: `:plugin list` command + bare `:plugin` opens the overlay

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (`Command::PluginList`, `Command::PluginCatalog`, remap bare `:plugin`)
- Modify: `crates/zoid/src/main.rs` (handlers)
- Test: `command.rs` inline

**Interfaces:**
- `Command::PluginList` and `Command::PluginCatalog` added. `:plugin` / `:plugin ` (empty) → `PluginCatalog`; `:plugin list` → `PluginList`; `:plugin install <arg>` → `PluginInstall` (unchanged).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parses_plugin_list_and_bare_plugin() {
    assert_eq!(parse_command(":plugin list"), Command::PluginList);
    assert_eq!(parse_command(":plugin"), Command::PluginCatalog);
    assert_eq!(parse_command(":plugin "), Command::PluginCatalog);
    assert_eq!(parse_command(":plugin install ok-skills"), Command::PluginInstall("ok-skills".into()));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-tui parses_plugin_list_and_bare_plugin`
Expected: FAIL — `Command::PluginList`/`PluginCatalog` undefined (today bare `:plugin` falls through to `Command::Unknown("plugin")` via the default arm, and `plugin list` likewise).

- [ ] **Step 3: Implement**

In `command.rs`, add variants to `enum Command` and adjust the parse arms (order matters — match `install ` and `list` before the bare fallthrough):

```rust
s if s.starts_with("plugin install ") =>
    Command::PluginInstall(s["plugin install ".len()..].trim().to_string()),
"plugin install" => Command::PluginInstall(String::new()),
"plugin list" => Command::PluginList,
"plugin" => Command::PluginCatalog,
```

In `main.rs` command dispatch, both variants kick off the SAME async catalog load (never block the main loop) whose result returns via an `AgentUpdate` message (fully specified in Task 5, which owns the load + `AgentUpdate::CatalogLoaded`): `Command::PluginCatalog` → set `plugin_catalog = Some(loading())`, `overlay = Overlay::PluginCatalog`, spawn the load; `Command::PluginList` → spawn the load and, on the message, print `id  [kind]  description` lines to the scrollback/status. Task 4 adds the command variants + parse arms + the dispatch stubs; Task 5 implements the async load, the `CatalogLoaded` handler, and the overlay. (This means Task 4's dispatch handlers are finished in Task 5 — acceptable since they share the load path; the command PARSE is fully tested here in Task 4.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-tui parses_plugin_list_and_bare_plugin`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): :plugin list + bare :plugin opens the catalog overlay"
```

---

### Task 5: `Overlay::PluginCatalog` — browse + provenance confirm

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`Overlay::PluginCatalog`, `PluginCatalogRow`, `PluginCatalogState`)
- Modify: the zoid-tui render module that renders overlays (follow `Overlay::Mcp`/`Feedback` rendering)
- Modify: `crates/zoid/src/main.rs` (map `catalog::CatalogEntry` → `PluginCatalogRow`; input handling: ↑↓, Enter→confirm, y→install, n/Esc→back; on `y` call the same path as `:plugin install <id>`)
- Test: `state.rs` inline (state machine + confirm gate)

**Interfaces:**
- Produces in `state.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalogRow {
    pub id: String,
    pub name: String,
    pub kind_label: String,     // "mode" | "skills"
    pub description: String,
    pub source_label: String,   // "mxyhi/ok-skills @ a1b2c3d"
    pub license: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogMode { List, Confirm }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStatus { Loading, Ready, Error(String) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalogState {
    pub rows: Vec<PluginCatalogRow>,
    pub cursor: usize,
    pub mode: CatalogMode,
    pub status: CatalogStatus,
}

impl PluginCatalogState {
    pub fn loading() -> Self { Self { rows: vec![], cursor: 0, mode: CatalogMode::List, status: CatalogStatus::Loading } }
    pub fn selected(&self) -> Option<&PluginCatalogRow> { self.rows.get(self.cursor) }
    pub fn move_up(&mut self) { if self.cursor > 0 { self.cursor -= 1; } }
    pub fn move_down(&mut self) { if self.cursor + 1 < self.rows.len() { self.cursor += 1; } }
    pub fn enter_confirm(&mut self) { if self.selected().is_some() { self.mode = CatalogMode::Confirm; } }
    pub fn back_to_list(&mut self) { self.mode = CatalogMode::List; }
}
```
- Store it on the shell: `pub plugin_catalog: Option<PluginCatalogState>` (mirrors how `FeedbackState`/`mcp_status` hang off the shell).

- [ ] **Step 1: Write the failing test (state machine + confirm gate)**

```rust
#[test]
fn catalog_state_transitions_and_confirm_gate() {
    let mut s = PluginCatalogState { rows: vec![
        PluginCatalogRow { id: "a".into(), name: "A".into(), kind_label: "mode".into(), description: "d".into(), source_label: "o/a @ dead".into(), license: None },
        PluginCatalogRow { id: "b".into(), name: "B".into(), kind_label: "skills".into(), description: "d".into(), source_label: "o/b @ beef".into(), license: Some("MIT".into()) },
    ], cursor: 0, mode: CatalogMode::List, status: CatalogStatus::Ready };
    s.move_down(); assert_eq!(s.cursor, 1);
    s.move_down(); assert_eq!(s.cursor, 1, "clamps at end");
    assert_eq!(s.selected().unwrap().id, "b");
    s.enter_confirm(); assert_eq!(s.mode, CatalogMode::Confirm);
    s.back_to_list(); assert_eq!(s.mode, CatalogMode::List);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-tui catalog_state_transitions`
Expected: FAIL — types undefined.

- [ ] **Step 3: Implement the state types** (as above) and add `Overlay::PluginCatalog` to the enum and `plugin_catalog: Option<PluginCatalogState>` to the shell struct (default `None`).

- [ ] **Step 4: Render + input** (bin side)

- Render: in the overlay render match, add a `Overlay::PluginCatalog` arm following `Overlay::Mcp`. List mode: title "zoid plugins", one row per entry (`name  [kind]  description`), highlight `cursor`, footer `↑↓ select · ↵ install · esc close`; `Loading`/`Error` render a centered line. Confirm mode: show the selected row's `name`, `source_label`, `kind_label`, `license`, and `Install this pack? [y/N]`.
- Input (bin key handler, gated on `overlay == PluginCatalog`): `Up/Down` → `move_up/move_down` (List only); `Enter` (List) → `enter_confirm`; in Confirm: `y`/`Y` → close overlay + run the install path for `selected().id` (same as `:plugin install <id>`); `n`/`N`/`Esc` → `back_to_list`; `Esc` (List) → close overlay (`Overlay::None`).
- On open (`Command::PluginCatalog`): set `plugin_catalog = Some(PluginCatalogState::loading())`, set `overlay = Overlay::PluginCatalog`, and `tokio::spawn` an **async** catalog load (the result must return via a message — a spawned task cannot touch `&mut App`). Add `AgentUpdate::CatalogLoaded(Result<Vec<zoid::catalog::CatalogEntry>, String>)` and in the spawn run the async index fetch (`cache_if_fresh` → else `fetch_text(catalog_index_url())` → `store_and_parse`; stale fallback via `cached_any`), sending `CatalogLoaded`. The bin resolves the cache dir (`resolve_cache_dir(env).join("catalog")`) on the main loop and moves it into the spawn. Handle `AgentUpdate::CatalogLoaded` in the dispatch (`main.rs:~3113`): map each `CatalogEntry` → `PluginCatalogRow` and set `status = Ready`, or `status = Error(..)`. Mapping: `kind_label = entry.kind.first().cloned().unwrap_or_default()`; **skip entries whose kind is not `mode` or `skills`** (L4 — keeps the MCP-deferred boundary honest); `source_label = format!("{} @ {}", entry.source_repo, entry.source_ref.chars().take(7).collect::<String>())` (char-safe, no byte slice).
- `:plugin list` (Task 4) uses the SAME async load + a message (or reuses `CatalogLoaded` with a flag / a sibling `AgentUpdate::CatalogListed`) to print rows once the load resolves — do not block the main loop on the fetch.

- [ ] **Step 5: Run tests + manual sanity**

Run: `cargo test -p zoid-tui catalog_state_transitions` → PASS. `cargo build -p zoid` → clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/*.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): Overlay::PluginCatalog browse + provenance confirm gate"
```

---

### Task 6: `zoid-releases` catalog kit (gen_index.py + CI + README + seed) and app fixtures

**Files:**
- Create (transplant to zoid-releases): `contrib/zoid-releases-catalog/scripts/gen_index.py`
- Create: `contrib/zoid-releases-catalog/.github/workflows/catalog-index.yml`
- Create: `contrib/zoid-releases-catalog/plugins/README.md`
- Create: `contrib/zoid-releases-catalog/plugins/superpowers.toml` (seed; produced by the Spec 1 converter) + generated `contrib/zoid-releases-catalog/plugins/index.json`
- Create: `crates/zoid/tests/fixtures/catalog/index.json` and `.../ok-skills.toml` (app test fixtures, used by earlier tasks' fixtures if referenced)
- Test: `gen_index.py` self-test (a `--check` mode or a pytest-free `if __name__ == "__main__"` assertion over a fixture dir)

**Interfaces:**
- `gen_index.py <plugins_dir>` reads every `*.toml` (skipping `index.json`), extracts `[plugin].{id,name,kind,description,license?}` + `[source].{repo,ref}`, writes `<plugins_dir>/index.json` deterministically (entries sorted by `id`, 2-space indent, trailing newline).

- [ ] **Step 1: Write `gen_index.py`**

```python
#!/usr/bin/env python3
"""Regenerate plugins/index.json from plugins/*.toml. Stdlib only (tomllib)."""
import json, sys, tomllib
from pathlib import Path

def build_index(plugins_dir: Path) -> dict:
    entries = []
    for toml_path in sorted(plugins_dir.glob("*.toml")):
        with toml_path.open("rb") as fh:
            data = tomllib.load(fh)
        p, s = data["plugin"], data["source"]
        entry = {
            "id": p["id"], "name": p.get("name", p["id"]),
            "kind": p["kind"], "description": p.get("description", ""),
            "source": {"repo": s["repo"], "ref": s["ref"]},
        }
        if "license" in p:
            entry["license"] = p["license"]
        entries.append(entry)
    entries.sort(key=lambda e: e["id"])
    return {"schema": 1, "plugins": entries}

def main(argv):
    plugins_dir = Path(argv[1]) if len(argv) > 1 else Path(__file__).resolve().parent.parent / "plugins"
    index = build_index(plugins_dir)
    out = json.dumps(index, indent=2, ensure_ascii=False) + "\n"
    (plugins_dir / "index.json").write_text(out, encoding="utf-8")
    print(f"wrote {plugins_dir/'index.json'} ({len(index['plugins'])} plugins)")

if __name__ == "__main__":
    main(sys.argv)
```

- [ ] **Step 2: CI workflow `catalog-index.yml`**

```yaml
name: catalog-index
on:
  push:
    paths: [ "plugins/*.toml" ]
permissions:
  contents: write
jobs:
  regen:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: "3.11" }
      - run: python scripts/gen_index.py plugins
      - name: Commit index if changed
        run: |
          if ! git diff --quiet plugins/index.json; then
            git config user.name "catalog-bot"
            git config user.email "catalog-bot@users.noreply.github.com"
            git add plugins/index.json
            git commit -m "chore(catalog): regenerate index.json"
            git push
          fi
```

- [ ] **Step 3: README + seed manifest + generated index**

Write `plugins/README.md` (contributor guide: run the Spec 1 converter, add `<id>.toml`, open a PR; index.json is generated by CI — do not hand-edit). Add a seed `superpowers.toml` (the Spec 1 manifest format) and run `python scripts/gen_index.py plugins` to produce `index.json`. Copy `index.json` + a second sample `<id>.toml` into `crates/zoid/tests/fixtures/catalog/` for the app tests.

- [ ] **Step 4: gen_index self-check**

Run: `python contrib/zoid-releases-catalog/scripts/gen_index.py contrib/zoid-releases-catalog/plugins` and assert the emitted `index.json` matches the committed one byte-for-byte (deterministic). Document the transplant step: these files move to the root of the `strvmarv/zoid-releases` repo (`scripts/`, `plugins/`, `.github/workflows/`).

- [ ] **Step 5: Commit**

```bash
git add contrib/zoid-releases-catalog crates/zoid/tests/fixtures/catalog
git commit -m "feat(catalog): zoid-releases publishing kit (gen_index.py + CI + seed) and app fixtures"
```

---

### Task 7: Workspace verification

**Files:** none (verification only)

- [ ] **Step 1:** `cargo build --workspace` — clean, no new warnings (especially no dead_code from `catalog.rs`).
- [ ] **Step 2:** `cargo test --workspace --no-fail-fast` — all green.
- [ ] **Step 3:** Confirm the Superpowers golden body is byte-unchanged: `git diff <branch-base> HEAD -- crates/zoid-plugin/tests/superpowers_body_golden.txt` is empty (no task here should touch it).
- [ ] **Step 4:** `python contrib/zoid-releases-catalog/scripts/gen_index.py contrib/zoid-releases-catalog/plugins` leaves `index.json` unchanged (deterministic).
- [ ] **Step 5:** Update the SDD ledger; no commit needed.

---

## Self-Review notes

- **Spec coverage:** index format + gen (Task 6), fetch/TTL cache (Task 2), resolution seam / carry-manifest (Task 3), overlay + confirm (Task 5), `:plugin list` (Task 4), trust/provenance (Task 5 confirm), error handling (Tasks 2/3/5), testing (each task). MCP is out of scope per Global Constraints — no task adds an mcp path.
- **Async, no blocking client (Tasks 2 & 3):** workspace `reqwest` has NO `blocking` feature and `block_on` inside the tokio task would nest runtimes and panic. All network is async: the manifest fetch runs INSIDE the install spawn alongside the async `fetch_tree`; the index fetch runs inside a spawned task delivering `AgentUpdate::CatalogLoaded`. The sync `catalog.rs` helpers (`cache_if_fresh`/`store_and_parse`/`cached_any`) do only fast fs work and carry the deterministic TTL tests; the `IndexFetcher` trait + `load_catalog` are test-only.
- **Manifest carried in `res` (Task 3):** `PluginScan.res` is `Result<(PluginManifest, UpstreamScan), String>` so a manifest-stage failure is representable and clears the `installing_plugin` guard through the one message. Three existing `apply_plugin_scan` call sites (`main.rs:7000`, `7028` already-`Err`, `7078`) plus the dispatch (`3114`) must be updated — enumerated in Task 3 Step 5.
- **`resolve_cache_dir` is a bin fn** (`main.rs:75`), so `catalog.rs` (a lib module) never imports it. The bin resolves `…/catalog` and passes it into the catalog helpers. No visibility change needed.
- **zoid-tui dependency hygiene:** `PluginCatalogState` holds plain `String` rows, NOT `zoid_plugin`/`catalog` types — the bin maps across the boundary (mirrors `McpStatusRow`). Keeps zoid-tui dependency-light.
