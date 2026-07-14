//! Fetch + cache + parse the public zoid-releases plugin catalog.
//! Unauthenticated raw.githubusercontent.com; see Spec 2 design.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
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
    /// Absent for `mcp` entries (an mcp manifest declares no `[source]`); the
    /// manifest is fetched + validated separately at confirm time.
    #[serde(default)]
    source: Option<RawSource>,
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
    Ok(raw.plugins.into_iter().map(|e| {
        let (source_repo, source_ref) = match e.source {
            Some(s) => (s.repo, s.ref_),
            None => (String::new(), String::new()),
        };
        CatalogEntry {
            id: e.id,
            name: e.name,
            kind: e.kind,
            description: e.description,
            license: e.license,
            source_repo,
            source_ref,
        }
    }).collect())
}

pub fn catalog_index_url() -> String {
    format!("{CATALOG_BASE}/index.json")
}

pub fn catalog_manifest_url(id: &str) -> String {
    format!("{CATALOG_BASE}/{id}.toml")
}

/// One-shot async raw GET of a public zoid-releases text file (unauthenticated).
pub async fn fetch_text(url: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder().user_agent("zoid").build()?;
    Ok(client.get(url).send().await?.error_for_status()?.text().await?)
}

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
    fn parse_index_allows_source_less_mcp_entry() {
        // mcp manifests declare no [source], so the index entry omits it too.
        // Installability is validated later when the manifest itself is fetched.
        let idx = r#"{ "schema": 1, "plugins": [ { "id": "github", "name": "GitHub MCP", "kind": ["mcp"], "description": "GitHub over MCP" } ] }"#;
        let v = parse_index(idx).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, vec!["mcp".to_string()]);
        assert_eq!(v[0].source_repo, "");
        assert_eq!(v[0].source_ref, "");
    }

    #[test]
    fn parse_index_reads_fixture_catalog() {
        let fixture = include_str!("../tests/fixtures/catalog/index.json");
        let v = parse_index(fixture).unwrap();
        assert!(v.len() >= 1, "fixture catalog must have at least one entry");
        let superpowers = v.iter().find(|e| e.id == "superpowers")
            .expect("fixture catalog must contain the superpowers entry");
        assert_eq!(superpowers.name, "Superpowers");
        assert_eq!(superpowers.kind, vec!["mode".to_string()]);
        assert_eq!(superpowers.source_repo, "obra/superpowers");
        assert_eq!(superpowers.source_ref, "d884ae04edebef577e82ff7c4e143debd0bbec99");
        let ok_skills = v.iter().find(|e| e.id == "ok-skills")
            .expect("fixture catalog must contain the ok-skills entry");
        assert_eq!(ok_skills.license.as_deref(), Some("MIT"));
    }

    #[test]
    fn urls_are_raw_unauthenticated() {
        assert_eq!(catalog_index_url(),
            "https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins/index.json");
        assert_eq!(catalog_manifest_url("ok-skills"),
            "https://raw.githubusercontent.com/strvmarv/zoid-releases/main/plugins/ok-skills.toml");
    }
}

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
