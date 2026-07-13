use anyhow::Context;

pub fn tree_url(repo: &str, sha: &str) -> String {
    format!("https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1")
}

fn client() -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().user_agent("zoid-plugin-import");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {tok}"))?);
        b = b.default_headers(h);
    }
    Ok(b.build()?)
}

pub async fn fetch_tree_paths(repo: &str, sha: &str) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = client()?.get(tree_url(repo, sha)).send().await?
        .error_for_status()?.json().await?;
    let tree = v.get("tree").and_then(|t| t.as_array()).context("no tree array")?;
    Ok(tree.iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("blob"))
        .filter_map(|e| e.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
        .collect())
}

pub async fn fetch_blob(repo: &str, sha: &str, path: &str) -> anyhow::Result<String> {
    let url = format!("https://raw.githubusercontent.com/{repo}/{sha}/{path}");
    Ok(client()?.get(url).send().await?.error_for_status()?.text().await?)
}

pub fn resolve_head_sha(repo: &str, branch: &str) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(["ls-remote", &format!("https://github.com/{repo}"), branch])
        .output()?;
    anyhow::ensure!(out.status.success(), "git ls-remote failed for {repo} {branch}");
    let line = String::from_utf8(out.stdout)?;
    let sha = line.split_whitespace().next().context("empty ls-remote output")?;
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
}
