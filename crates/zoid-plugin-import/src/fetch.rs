use anyhow::Context;

pub fn tree_url(repo: &str, sha: &str) -> String {
    format!("https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1")
}

fn client() -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().user_agent("zoid-plugin-import");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {tok}"))?,
        );
        b = b.default_headers(h);
    }
    Ok(b.build()?)
}

/// Extract blob paths from a GitHub Trees API response. Errors if the API
/// truncated the listing (very large repos): a partial file list would
/// silently produce an incomplete/incorrect conversion.
fn parse_tree(v: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(
        v.get("truncated").and_then(|t| t.as_bool()) != Some(true),
        "GitHub truncated the recursive tree listing (repo too large); \
         partial results would corrupt the conversion"
    );
    let tree = v
        .get("tree")
        .and_then(|t| t.as_array())
        .context("no tree array")?;
    Ok(tree
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("blob"))
        .filter_map(|e| {
            e.get("path")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .collect())
}

pub async fn fetch_tree_paths(repo: &str, sha: &str) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = client()?
        .get(tree_url(repo, sha))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    parse_tree(&v)
}

pub async fn fetch_blob(repo: &str, sha: &str, path: &str) -> anyhow::Result<String> {
    let url = format!("https://raw.githubusercontent.com/{repo}/{sha}/{path}");
    Ok(client()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

pub fn resolve_head_sha(repo: &str, branch: &str) -> anyhow::Result<String> {
    // Guard against argv flag-smuggling: `repo`/`branch` come from CLI args and
    // marketplace manifests. A value beginning with `-` would be parsed by git
    // as an option (e.g. --upload-pack=...); reject those and pass `--` so
    // everything after it is treated positionally.
    anyhow::ensure!(
        !repo.starts_with('-') && !repo.contains(char::is_whitespace),
        "invalid repo '{repo}'"
    );
    anyhow::ensure!(!branch.starts_with('-'), "invalid branch '{branch}'");
    let out = std::process::Command::new("git")
        .args([
            "ls-remote",
            "--",
            &format!("https://github.com/{repo}"),
            branch,
        ])
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "git ls-remote failed for {repo} {branch}"
    );
    let line = String::from_utf8(out.stdout)?;
    let sha = line
        .split_whitespace()
        .next()
        .context("empty ls-remote output")?;
    Ok(sha.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_url_is_recursive_api_url() {
        assert_eq!(
            tree_url("obra/superpowers", "abc123"),
            "https://api.github.com/repos/obra/superpowers/git/trees/abc123?recursive=1"
        );
    }

    #[test]
    fn parse_tree_extracts_blob_paths_and_skips_trees() {
        let v = serde_json::json!({
            "truncated": false,
            "tree": [
                {"type": "blob", "path": "skills/a/SKILL.md"},
                {"type": "tree", "path": "skills/a"},
                {"type": "blob", "path": "README.md"},
            ]
        });
        assert_eq!(
            parse_tree(&v).unwrap(),
            vec!["skills/a/SKILL.md", "README.md"]
        );
    }

    #[test]
    fn parse_tree_errors_when_truncated() {
        let v = serde_json::json!({ "truncated": true, "tree": [] });
        let err = parse_tree(&v).unwrap_err().to_string();
        assert!(err.contains("truncated"), "got: {err}");
    }

    #[test]
    fn resolve_head_sha_rejects_flag_smuggling() {
        // Validation returns Err BEFORE shelling git, so this does not invoke git.
        assert!(resolve_head_sha("obra/superpowers", "--upload-pack=evil").is_err());
        assert!(resolve_head_sha("-x/evil", "main").is_err());
    }
}
