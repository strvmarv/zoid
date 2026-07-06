//! GitHub tree fetcher for the URL import wizard. Resolves a
//! `github.com/{owner}/{repo}/tree/{ref}/{path}` URL via the GitHub HTTP API
//! (api.github.com/repos/.../git/trees/...?recursive=1) and assembles an
//! `UpstreamScan`. `$GITHUB_TOKEN` is used if present (higher rate limit,
//! private repos). HTTP calls are behind a `GithubApi` trait so tests use
//! `FakeGithubApi` with no real network.

use anyhow::anyhow;
use serde_json::Value;
use zoid_core::wizard::{ScannedFile, UpstreamScan};

/// The parsed GitHub URL: owner/repo, ref, and subtree path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubUrl {
    pub owner: String,
    pub repo: String,
    pub ref_: String,
    pub subtree_path: String,
}

/// Parse a `github.com/{owner}/{repo}/tree/{ref}/{path}` URL. Also accepts
/// `/blob/{ref}/{path}` (a single file — the scan will have one entry). Returns
/// `Err` with a human-readable reason for non-GitHub URLs or malformed shapes.
pub fn parse_github_url(url: &str) -> Result<GithubUrl, String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://github.com/")
        .or_else(|| u.strip_prefix("http://github.com/"))
        .or_else(|| u.strip_prefix("github.com/"))
        .ok_or_else(|| format!("URL import supports github.com URLs only (got '{u}')"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 5 {
        return Err(format!(
            "expected github.com/{{owner}}/{{repo}}/tree/{{ref}}/{{path}}, got '{u}'"
        ));
    }
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let kind = parts[2];
    let ref_ = parts[3].to_string();
    if kind != "tree" && kind != "blob" {
        return Err(format!(
            "expected '/tree/{{ref}}/{{path}}' or '/blob/{{ref}}/{{path}}' in '{u}'"
        ));
    }
    let subtree_path = parts[4..].join("/");
    Ok(GithubUrl {
        owner,
        repo,
        ref_,
        subtree_path,
    })
}

/// The GitHub API seam. `HttpGithubApi` hits the real API; `FakeGithubApi`
/// returns canned JSON for tests.
#[async_trait::async_trait]
pub trait GithubApi: Send + Sync {
    async fn fetch_tree_json(&self, owner: &str, repo: &str, ref_: &str)
        -> anyhow::Result<Value>;

    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String>;
}

/// Real GitHub API client. `token` is `$GITHUB_TOKEN` if set.
pub struct HttpGithubApi {
    client: reqwest::Client,
    token: Option<String>,
}

impl Default for HttpGithubApi {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpGithubApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("zoid-wizard/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client builds"),
            token: std::env::var("GITHUB_TOKEN").ok(),
        }
    }
}

#[async_trait::async_trait]
impl GithubApi for HttpGithubApi {
    async fn fetch_tree_json(
        &self,
        owner: &str,
        repo: &str,
        ref_: &str,
    ) -> anyhow::Result<Value> {
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/git/trees/{ref_}?recursive=1"
        );
        let mut req = self.client.get(&url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        if resp.status().as_u16() == 403 {
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            if remaining == "0" {
                anyhow::bail!("GitHub rate-limited. Set $GITHUB_TOKEN for a higher limit.");
            }
        }
        resp.error_for_status()?.json().await.map_err(Into::into)
    }

    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String> {
        let mut req = self.client.get(download_url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        resp.error_for_status()?.text().await.map_err(Into::into)
    }
}

/// Fetch the subtree at `GithubUrl.subtree_path` and assemble an `UpstreamScan`.
pub async fn fetch_tree(
    api: &dyn GithubApi,
    url: &GithubUrl,
) -> anyhow::Result<UpstreamScan> {
    let tree_json = api.fetch_tree_json(&url.owner, &url.repo, &url.ref_).await?;
    let resolved_ref = tree_json
        .get("sha")
        .and_then(|v| v.as_str())
        .unwrap_or(&url.ref_)
        .to_string();
    let entries = tree_json
        .get("tree")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("GitHub tree response has no 'tree' array"))?;
    let prefix = if url.subtree_path.is_empty() {
        String::new()
    } else {
        format!("{}/", url.subtree_path)
    };
    let mut files = Vec::new();
    for entry in entries {
        let etype = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if etype != "blob" {
            continue;
        }
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !path.starts_with(&prefix) {
            continue;
        }
        let sha = entry
            .get("sha")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{path}",
            url.owner, url.repo, url.ref_
        );
        let content = api.fetch_blob_content(&raw_url).await?;
        files.push(ScannedFile {
            upstream_path: path.to_string(),
            sha,
            content,
        });
    }
    Ok(UpstreamScan {
        url: format!(
            "https://github.com/{}/{}/tree/{}/{}",
            url.owner, url.repo, url.ref_, url.subtree_path
        ),
        repo: format!("{}/{}", url.owner, url.repo),
        resolved_ref,
        subtree_path: url.subtree_path.clone(),
        files,
    })
}

/// A fake API for tests. Returns a canned tree JSON + per-path content.
pub struct FakeGithubApi {
    pub tree_json: Value,
    pub contents: std::collections::HashMap<String, String>,
}

#[async_trait::async_trait]
impl GithubApi for FakeGithubApi {
    async fn fetch_tree_json(
        &self,
        _owner: &str,
        _repo: &str,
        _ref_: &str,
    ) -> anyhow::Result<Value> {
        Ok(self.tree_json.clone())
    }

    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String> {
        self.contents
            .get(download_url)
            .cloned()
            .ok_or_else(|| anyhow!("FakeGithubApi: no content for {download_url}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_url() {
        let g = parse_github_url("github.com/obra/superpowers/tree/main/skills").unwrap();
        assert_eq!(g.owner, "obra");
        assert_eq!(g.repo, "superpowers");
        assert_eq!(g.ref_, "main");
        assert_eq!(g.subtree_path, "skills");
    }

    #[test]
    fn parses_blob_url() {
        let g = parse_github_url("https://github.com/o/r/blob/main/skills/a/SKILL.md").unwrap();
        assert_eq!(g.ref_, "main");
        assert_eq!(g.subtree_path, "skills/a/SKILL.md");
    }

    #[test]
    fn parses_nested_subtree_path() {
        let g =
            parse_github_url("github.com/o/r/tree/main/skills/brainstorming/scripts").unwrap();
        assert_eq!(g.subtree_path, "skills/brainstorming/scripts");
    }

    #[test]
    fn rejects_non_github() {
        let err = parse_github_url("gitlab.com/o/r/tree/main/skills").unwrap_err();
        assert!(err.contains("github.com URLs only"));
    }

    #[test]
    fn rejects_no_tree() {
        let err = parse_github_url("github.com/obra/superpowers").unwrap_err();
        assert!(err.contains("tree"));
    }

    #[test]
    fn rejects_malformed_kind() {
        let err = parse_github_url("github.com/o/r/branches/main/skills").unwrap_err();
        assert!(err.contains("tree") || err.contains("blob"));
    }

    fn fake_tree() -> Value {
        serde_json::json!({
            "sha": "abc123",
            "tree": [
                { "path": "skills/a/SKILL.md", "sha": "sha-a", "type": "blob" },
                { "path": "skills/README.md", "sha": "sha-r", "type": "blob" },
                { "path": "skills/sub", "sha": "sha-tree", "type": "tree" }
            ]
        })
    }

    fn fake_contents() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "https://raw.githubusercontent.com/o/r/main/skills/a/SKILL.md".into(),
            "A BODY".into(),
        );
        m.insert(
            "https://raw.githubusercontent.com/o/r/main/skills/README.md".into(),
            "README".into(),
        );
        m
    }

    #[tokio::test]
    async fn fetch_tree_assembles_scan_with_subtree_filter() {
        let api = FakeGithubApi {
            tree_json: fake_tree(),
            contents: fake_contents(),
        };
        let url = parse_github_url("github.com/o/r/tree/main/skills").unwrap();
        let scan = fetch_tree(&api, &url).await.unwrap();
        assert_eq!(scan.repo, "o/r");
        assert_eq!(scan.resolved_ref, "abc123");
        assert_eq!(scan.subtree_path, "skills");
        assert_eq!(scan.files.len(), 2);
        let a = scan.files.iter().find(|f| f.upstream_path == "skills/a/SKILL.md").unwrap();
        assert_eq!(a.sha, "sha-a");
        assert_eq!(a.content, "A BODY");
    }
}