//! zoid-web — the web tooling leaf crate: DuckDuckGo HTML search + readability
//! fetch with char-offset paging. A pure leaf (no zoid-tools/core/provider dep).
//! Tools in zoid-tools delegate to `search`/`fetch`; the agent loop calls them
//! via the new `ToolKind::Network` async seam.

pub(crate) mod search;
pub(crate) mod extract;

use std::time::Duration;

/// Connect timeout for web HTTP clients.
const CONNECT_TIMEOUT_SECS: u64 = 20;

/// Build a `reqwest::Client` with a connect timeout + a zoid-identifying
/// User-Agent (so DDG doesn't block the default reqwest UA). Falls back to the
/// default client if the builder fails.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .user_agent(concat!("zoid/", env!("CARGO_PKG_VERSION"), " (web tool)"))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// One DuckDuckGo search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// A heading landmark in extracted content, with its char offset in the
/// markdown. Appended to the first fetch (offset 0) so the model can jump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingMark {
    pub level: u8,
    pub text: String,
    pub char_offset: usize,
}

/// A fetched page's paged content + metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResult {
    pub url: String,
    pub title: String,
    /// The paged markdown window: content[offset..min(offset+limit, total)].
    pub content: String,
    pub total_chars: usize,
    pub offset: usize,
    pub limit: usize,
    /// Heading landmarks; non-empty only when offset == 0 (the first fetch).
    pub outline: Vec<HeadingMark>,
    pub content_type: String,
}

/// Search the web via DuckDuckGo HTML. Returns up to 8 results.
pub async fn search(query: &str) -> anyhow::Result<Vec<SearchResult>> {
    let client = http_client();
    search::search_with_client(&client, query).await
}

/// Fetch a URL, extract readable content, convert to markdown, page by char
/// offset/limit.
pub async fn fetch(url: &str, offset: usize, limit: usize) -> anyhow::Result<FetchResult> {
    use anyhow::anyhow;

    // URL-scheme guard: http/https only (no file:///data: exfiltration).
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("bad url: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(anyhow!("web_fetch supports http/https only (got {scheme})"));
    }

    let client = http_client();
    let resp = client.get(url).send().await?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(200).collect();
        return Err(anyhow!("HTTP {status}: {snippet}"));
    }
    let body = resp.text().await?;
    let (title, markdown) = extract::extract_markdown(&body, url)?;
    let total_chars = markdown.chars().count();
    let window = extract::page(&markdown, offset, limit)
        .ok_or_else(|| anyhow!("offset {offset} past end (total {total_chars})"))?;
    let outline = if offset == 0 {
        extract::build_outline(&markdown)
    } else {
        Vec::new()
    };
    Ok(FetchResult {
        url: url.to_string(),
        title,
        content: window,
        total_chars,
        offset,
        limit,
        outline,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::io::AsyncReadExt;

    #[test]
    fn http_client_builds_with_user_agent() {
        let c = http_client();
        // No public UA accessor; just assert it builds without panicking.
        let _ = c;
    }

    #[tokio::test]
    async fn search_empty_query_returns_err_without_network() {
        // An empty query is rejected before any network call.
        let r = search("").await;
        assert!(r.is_err());
    }

    async fn spawn_html_server(html: &'static str) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    html.len(), html
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    // Rich enough for readability to reliably extract the article (the short
    // version can flake — see FIXTURE_HTML note in extract.rs). The title is
    // asserted; the outline is asserted non-empty but NOT asserted to contain a
    // specific heading (readability's exact output isn't a contract).
    const ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html><head><title>Test Page</title></head>
<body>
<nav>nav links here</nav>
<article>
<h1>Top Heading</h1>
<p>First paragraph with enough text content that readability scores it highly
and reliably extracts the article rather than the navigation. The arc90
algorithm weighs text density, so a single short sentence can flake.</p>
<h2>Second Section</h2>
<p>Second paragraph continues the article body with more text content to
ensure the article node wins the readability scoring against the nav.</p>
</article>
<footer>footer text</footer>
</body></html>"#;

    #[tokio::test]
    async fn fetch_returns_markdown_with_outline_on_first_page() {
        let addr = spawn_html_server(ARTICLE_HTML).await;
        let r = fetch(&format!("http://{addr}"), 0, 100_000).await.unwrap();
        assert_eq!(r.title, "Test Page");
        assert!(!r.content.trim().is_empty(), "content should be extracted markdown");
        assert!(!r.outline.is_empty(), "first fetch (offset 0) includes outline");
        assert_eq!(r.offset, 0);
    }

    #[tokio::test]
    async fn fetch_omits_outline_on_nonzero_offset() {
        let addr = spawn_html_server(ARTICLE_HTML).await;
        let r = fetch(&format!("http://{addr}"), 1, 100_000).await.unwrap();
        assert!(r.outline.is_empty(), "non-zero offset omits outline");
        assert_eq!(r.offset, 1);
    }

    #[tokio::test]
    async fn fetch_offset_past_end_returns_err() {
        let addr = spawn_html_server(ARTICLE_HTML).await;
        let r = fetch(&format!("http://{addr}"), 9_999_999, 1000).await;
        assert!(r.is_err());
        let e = r.unwrap_err().to_string();
        assert!(e.contains("past end"), "got: {e}");
    }

    #[tokio::test]
    async fn fetch_rejects_non_http_scheme() {
        let r = fetch("file:///etc/passwd", 0, 1000).await;
        assert!(r.is_err());
        let e = r.unwrap_err().to_string();
        assert!(e.contains("http/https only"), "got: {e}");
    }

    #[tokio::test]
    async fn fetch_non_2xx_returns_err_with_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found")
                    .await;
            }
        });
        let r = fetch(&format!("http://{addr}"), 0, 1000).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("404"));
    }
}