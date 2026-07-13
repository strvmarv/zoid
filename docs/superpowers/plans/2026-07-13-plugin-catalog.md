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

Add `pub mod catalog;` to `crates/zoid/src/lib.rs` (and ensure the bin sees it). Confirm whether `zoid` exposes a lib (`lib.rs`) — if modules are declared in `main.rs`, add `mod catalog;` there and re-export as needed. Match the existing module-declaration convention (`github_fetch`, `plugin_install` are already modules — declare `catalog` the same way).

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
- Consumes: `parse_index`, `catalog_index_url` (Task 1); `resolve_cache_dir(env)` from `main.rs` — if it is not importable from `catalog.rs`, add a small `pub(crate) fn cache_root(env) -> PathBuf` mirroring it, or make `resolve_cache_dir` `pub(crate)`.
- Produces:
  - `pub trait IndexFetcher { fn get(&self, url: &str) -> anyhow::Result<String>; }` (blocking; the real impl uses `reqwest::blocking` OR the async client via the caller — see note)
  - `pub fn load_catalog(now: DateTime<Utc>, ttl: Duration, cache_dir: &Path, fetcher: &dyn IndexFetcher) -> anyhow::Result<Vec<CatalogEntry>>`

**Design note (async vs blocking):** the app fetches off-thread already (`tokio::spawn`). Keep `load_catalog` **synchronous over an injected fetcher trait** so it is unit-testable with a fake; the real caller runs it inside a spawned task with a fetcher that wraps the existing async `reqwest::Client` via `futures::executor::block_on` OR, simpler, a real fetcher built on `reqwest::blocking::Client`. Decide at implementation time based on what the crate already links; a fake fetcher is used in tests regardless.

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

/// Load the catalog honoring a TTL. Fresh cache → no network. Stale/missing →
/// fetch, cache, parse; on fetch failure fall back to any cache on disk; if
/// none, error.
pub fn load_catalog(
    now: DateTime<Utc>,
    ttl: Duration,
    cache_dir: &Path,
    fetcher: &dyn IndexFetcher,
) -> anyhow::Result<Vec<CatalogEntry>> {
    let fresh = read_stamp(cache_dir).map(|t| now - t < ttl).unwrap_or(false);
    if fresh {
        if let Ok(cached) = std::fs::read_to_string(cache_file(cache_dir)) {
            if let Ok(v) = parse_index(&cached) {
                return Ok(v);
            }
        }
    }
    match fetcher.get(&catalog_index_url()) {
        Ok(body) => {
            // Parse BEFORE overwriting the cache so garbage never clobbers a good cache.
            let v = parse_index(&body)?;
            std::fs::create_dir_all(cache_dir).ok();
            std::fs::write(cache_file(cache_dir), &body).ok();
            std::fs::write(stamp_file(cache_dir), now.to_rfc3339()).ok();
            Ok(v)
        }
        Err(e) => {
            // Network failed — serve a stale cache if we have a parseable one.
            if let Ok(cached) = std::fs::read_to_string(cache_file(cache_dir)) {
                if let Ok(v) = parse_index(&cached) {
                    return Ok(v);
                }
            }
            Err(anyhow::anyhow!("catalog unavailable: {e}"))
        }
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
- `AgentUpdate::PluginScan { id, origin, over, manifest: zoid_plugin::manifest::PluginManifest, res }` — new `manifest` field.
- `apply_plugin_scan` signature gains `manifest: PluginManifest` and uses it instead of `bundled_manifest(&id)`.

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

- [ ] **Step 4: Thread `manifest` through `PluginScan` + `install_plugin`**

In `agent.rs`, add the field:

```rust
PluginScan {
    id: String,
    origin: String,
    over: crate::plugin_install::KindOverride,
    manifest: zoid_plugin::manifest::PluginManifest,
    res: Result<zoid_core::wizard::UpstreamScan, String>,
},
```

In `main.rs::install_plugin`, resolve the manifest up front (bundled or catalog), then spawn the existing tree fetch, carrying the manifest. Replace the current `(manifest, id)` match:

```rust
let (manifest, id, origin) = match (&r, resolve_source(&r, zoid_plugin::bundled::bundled_ids(), false, false)) {
    (PluginRef::Id(id), ManifestSource::Bundled) => (
        zoid_plugin::bundled::bundled_manifest(id).expect("bundled id resolves"),
        id.clone(), "bundled".to_string(),
    ),
    (PluginRef::Id(id), ManifestSource::Catalog) => {
        // Fetch <id>.toml from the catalog (raw, unauthenticated), parse + validate.
        match zoid::catalog::fetch_catalog_manifest_blocking(id) {
            Ok(m) => (m, id.clone(), "catalog".to_string()),
            Err(e) => { app.shell.status_hint = Some(format!("plugin '{id}': {e}")); return; }
        }
    }
    (PluginRef::Url(_), _) => {
        app.shell.status_hint = Some("installing plugins from a URL is not supported yet; use a catalog id".into());
        return;
    }
    (PluginRef::Id(id), _) => { app.shell.status_hint = Some(format!("unknown plugin '{id}'")); return; }
};
if let Err(e) = manifest.validate() { app.shell.status_hint = Some(e); return; }
```

Add to `catalog.rs` a small blocking helper used here (fetch + `parse_manifest` + `validate`):

```rust
pub fn fetch_catalog_manifest_blocking(id: &str) -> anyhow::Result<zoid_plugin::manifest::PluginManifest> {
    // Reuse the same fetcher construction as the index fetch; here a one-shot raw GET.
    let body = crate::catalog::real_fetcher().get(&catalog_manifest_url(id))?;
    zoid_plugin::manifest::parse_manifest(&body).map_err(|e| anyhow::anyhow!(e))
}
```

Where `real_fetcher()` returns the crate's production `IndexFetcher`. Because `install_plugin` is called on the main loop (not async), and a blocking network call there would stall the UI, prefer instead to move the manifest fetch INTO the spawned task alongside the tree fetch: fetch `<id>.toml` first, parse, then `fetch_tree`, then send `PluginScan { manifest, .. }`. Implement it that way — the match above resolves *bundled* synchronously, but the *catalog* branch sets up an async fetch of both the manifest and the tree in the spawned task. Concretely, for the catalog branch, carry `id`+`over` into the spawn; inside: `let manifest = parse_manifest(fetch(manifest_url))?; let scan = fetch_tree(source)?; send PluginScan { id, origin:"catalog", over, manifest, res: Ok(scan) }`. For the bundled branch, the manifest is already in hand; still send it via `PluginScan { manifest, .. }`.

Carry the manifest to the `PluginScan` send in both branches.

In `apply_plugin_scan`, change the signature to accept `manifest` and delete the `bundled_manifest` lookup:

```rust
fn apply_plugin_scan(
    app: &mut App,
    id: String,
    origin: String,
    over: zoid::plugin_install::KindOverride,
    manifest: zoid_plugin::manifest::PluginManifest,   // carried, not re-derived
    res: Result<zoid_core::wizard::UpstreamScan, String>,
) -> bool {
    // ... unchanged guard/scan handling ...
    let mut manifest = manifest;      // was: bundled_manifest(&id) lookup
    // ... KindOverride application, build_plan, install, effects — unchanged ...
}
```

Update the call site (`main.rs` ~3113) to pass `manifest` out of the `PluginScan` message.

- [ ] **Step 5: Write the failing test (apply_plugin_scan with a carried catalog manifest)**

Mirror the existing `apply_plugin_scan_reports_honest_status_and_clears_guard` test, but build a NON-bundled manifest in-test and pass it as the carried `manifest`, with `origin = "catalog"`, asserting it installs as a mode and activates (proving the path no longer depends on `bundled_manifest`).

```rust
#[tokio::test]
async fn apply_plugin_scan_installs_a_carried_catalog_manifest() {
    use zoid_plugin::manifest::{PluginManifest, ModeRecipe, BodyStrategy};
    use zoid_plugin::effect::Effect;
    // ... build UpstreamScan like the neighboring test ...
    let manifest = PluginManifest {
        id: "demo".into(), schema: 1, kind: vec!["mode".into()],
        name: "Demo".into(), description: "d".into(),
        source: Some(zoid_plugin::manifest::PluginSource { repo: "o/demo".into(), ref_: "SHA".into(), subtree: "skills".into() }),
        mode: Some(ModeRecipe { loader: "using-demo/SKILL.md".into(), strip_prefix: "skills/".into(), body: BodyStrategy::FromSkillFrontmatter, description: "Demo mode".into() }),
        install: vec![Effect::Activate],
    };
    // scan must contain skills/using-demo/SKILL.md (loader) so build_plan succeeds
    let activated = apply_plugin_scan(&mut app, "demo".into(), "catalog".into(),
        zoid::plugin_install::KindOverride::None, manifest, Ok(scan));
    assert!(activated);
    assert!(app.modes.names().iter().any(|n| n == "Demo"));
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid-plugin resolve && cargo test -p zoid apply_plugin_scan`
Expected: PASS.

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
Expected: FAIL — variants undefined / bare `:plugin` currently maps to `PluginInstall("")`.

- [ ] **Step 3: Implement**

In `command.rs`, add variants to `enum Command` and adjust the parse arms (order matters — match `install ` and `list` before the bare fallthrough):

```rust
s if s.starts_with("plugin install ") =>
    Command::PluginInstall(s["plugin install ".len()..].trim().to_string()),
"plugin install" => Command::PluginInstall(String::new()),
"plugin list" => Command::PluginList,
"plugin" => Command::PluginCatalog,
```

In `main.rs` command dispatch: `Command::PluginList` → spawn a catalog load, then print `id  [kind]  description` lines to the scrollback/status; `Command::PluginCatalog` → set `app.shell.overlay = Overlay::PluginCatalog` and kick off the catalog load that populates `PluginCatalogState` (Task 5 owns the state; here just open + trigger load).

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
- On open (`Command::PluginCatalog`): set `plugin_catalog = Some(PluginCatalogState::loading())`, set overlay, spawn a catalog load; when it returns, map `CatalogEntry`→`PluginCatalogRow` (`source_label = format!("{repo} @ {}", &sha[..sha.len().min(7)])`) and set `status = Ready` (or `Error`).

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
- **Async wrinkle (Task 3):** `install_plugin` runs on the main loop; the catalog manifest fetch MUST happen in the spawned task (not synchronously on the UI thread). The plan calls this out explicitly; the implementer should not add a blocking network call on the main loop. `fetch_catalog_manifest_blocking` is only safe to call *inside* the spawn.
- **`resolve_cache_dir` reuse (Task 2):** confirm it is reachable from `catalog.rs` (it is `#[cfg_attr(not(feature="local-embed"), allow(dead_code))]` in `main.rs`); if not `pub(crate)`, make it so, or mirror it. The overlay/bin passes the resolved cache dir into `load_catalog`.
- **zoid-tui dependency hygiene:** `PluginCatalogState` holds plain `String` rows, NOT `zoid_plugin`/`catalog` types — the bin maps across the boundary (mirrors `McpStatusRow`). Keeps zoid-tui dependency-light.
