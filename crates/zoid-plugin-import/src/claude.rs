use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSourceRef {
    InRepo {
        path: String,
    },
    GitSubdir {
        url: String,
        path: String,
        sha: String,
    },
    Github {
        repo: String,
        sha: String,
    },
}

#[derive(Debug, Clone)]
pub struct MarketplaceEntry {
    pub name: String,
    pub description: String,
    pub source: PluginSourceRef,
}

#[derive(Deserialize)]
struct RawMarket {
    plugins: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    name: String,
    #[serde(default)]
    description: String,
    source: RawSource,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSource {
    Str(String),
    Obj {
        source: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        sha: Option<String>,
    },
}

pub fn parse_marketplace(json: &str) -> anyhow::Result<Vec<MarketplaceEntry>> {
    let raw: RawMarket = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for e in raw.plugins {
        let source = match e.source {
            RawSource::Str(p) => PluginSourceRef::InRepo { path: p },
            RawSource::Obj {
                source,
                url,
                path,
                repo,
                sha,
            } => match source.as_str() {
                "git-subdir" => PluginSourceRef::GitSubdir {
                    url: url.ok_or_else(|| anyhow::anyhow!("git-subdir missing url"))?,
                    path: path.unwrap_or_default(),
                    sha: sha.ok_or_else(|| anyhow::anyhow!("git-subdir missing sha"))?,
                },
                "github" => PluginSourceRef::Github {
                    repo: repo.ok_or_else(|| anyhow::anyhow!("github missing repo"))?,
                    sha: sha.ok_or_else(|| anyhow::anyhow!("github missing sha"))?,
                },
                other => anyhow::bail!("unknown source kind '{other}'"),
            },
        };
        out.push(MarketplaceEntry {
            name: e.name,
            description: e.description,
            source,
        });
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct PluginJson {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

pub fn parse_plugin_json(json: &str) -> anyhow::Result<PluginJson> {
    Ok(serde_json::from_str(json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_three_source_shapes() {
        let json = include_str!("../tests/fixtures/marketplace_snippet.json");
        let entries = parse_marketplace(json).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            matches!(&entries[0].source, PluginSourceRef::GitSubdir { sha, .. } if sha.len() == 40)
        );
        assert!(
            matches!(&entries[1].source, PluginSourceRef::InRepo { path } if path == "./plugins/b")
        );
        assert!(
            matches!(&entries[2].source, PluginSourceRef::Github { repo, .. } if repo == "o2/r2")
        );
    }

    #[test]
    fn parses_plugin_json() {
        let p = parse_plugin_json(r#"{"name":"github","description":"gh"}"#).unwrap();
        assert_eq!(p.name, "github");
    }
}
