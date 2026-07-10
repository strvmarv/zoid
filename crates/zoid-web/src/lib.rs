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

/// Search the web via DuckDuckGo HTML. Returns up to 8 results. Stub here;
/// Task 2 fills it.
pub async fn search(_query: &str) -> anyhow::Result<Vec<SearchResult>> {
    Err(anyhow::anyhow!("search not yet implemented"))
}

/// Fetch a URL, extract readable content, convert to markdown, page by char
/// offset/limit. Stub here; Task 3 fills it.
pub async fn fetch(_url: &str, _offset: usize, _limit: usize) -> anyhow::Result<FetchResult> {
    Err(anyhow::anyhow!("fetch not yet implemented"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_builds_with_user_agent() {
        let c = http_client();
        // No public UA accessor; just assert it builds without panicking.
        let _ = c;
    }

    #[tokio::test]
    async fn search_stub_returns_not_yet_implemented() {
        let r = search("test").await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn fetch_stub_returns_not_yet_implemented() {
        let r = fetch("https://example.com", 0, 1000).await;
        assert!(r.is_err());
    }
}