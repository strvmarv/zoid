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
