//! DuckDuckGo HTML scrape. POSTs a form-encoded `q=<query>` to
//! `https://html.duckduckgo.com/html/` and parses up to 8 results out of the
//! no-JS HTML response. Uses `scraper` (CSS selectors) — DDG's HTML is nested,
//! not regex-friendly.

use crate::SearchResult;
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

/// The DuckDuckGo HTML endpoint.
const DDG_URL: &str = "https://html.duckduckgo.com/html/";

/// Max results to return (keeps tool output bounded).
const MAX_RESULTS: usize = 8;

/// Parse DuckDuckGo's no-JS HTML into `SearchResult`s. Pure (no network) so
/// it's testable with a fixture file. Extracts the real URL from the
/// `uddg=<encoded>` query param of DDG's redirect links.
pub(crate) fn parse_ddg_html(html: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let result_sel = match Selector::parse(".result") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let link_sel = match Selector::parse(".result__a") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let snippet_sel = match Selector::parse(".result__snippet") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for result in doc.select(&result_sel) {
        if out.len() >= MAX_RESULTS {
            break;
        }
        let title = result
            .select(&link_sel)
            .next()
            .map(|a| a.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let href = result
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .unwrap_or_default();
        let url = extract_uddg_url(href).unwrap_or_else(|| href.to_string());
        let snippet = result
            .select(&snippet_sel)
            .next()
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if !title.is_empty() && !url.is_empty() {
            out.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    out
}

/// Extract the real destination URL from a DDG redirect link of the form
/// `//duckduckgo.com/l/?uddg=<encoded-url>&rut=…`. Returns None if no `uddg`.
fn extract_uddg_url(href: &str) -> Option<String> {
    let href = href.trim();
    // The href may start with `//` (scheme-relative). Parse as a query string.
    let query = href.split('?').nth(1)?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "uddg" {
            return Some(
                urlencoding::decode(v)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| v.to_string()),
            );
        }
    }
    None
}

pub(crate) async fn search_with_client(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<SearchResult>> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    let resp = client.post(DDG_URL).form(&[("q", q)]).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("DuckDuckGo returned HTTP {}", resp.status()));
    }
    let body = resp.text().await?;
    let results = parse_ddg_html(&body);
    if results.is_empty() {
        return Err(anyhow!("no results found for: {q}"));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string("tests/fixtures/ddg_sample.html").unwrap_or_else(|_| {
            // Fall back to inline fixture if the file isn't found (e.g. run from
            // a different cwd). Keeps the test hermetic.
            include_str!("../tests/fixtures/ddg_sample.html").to_string()
        })
    }

    #[test]
    fn parse_ddg_html_extracts_two_results() {
        let results = parse_ddg_html(&fixture());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Asynchronous Programming in Rust");
        assert_eq!(results[0].url, "https://doc.rust-lang.org/async-book/");
        assert!(results[0].snippet.contains("Async/await in Rust"));
        assert_eq!(results[1].title, "async fn in traits - Stack Overflow");
        assert_eq!(results[1].url, "https://stackoverflow.com/q/12345");
    }

    #[test]
    fn parse_ddg_html_empty_returns_empty() {
        assert!(parse_ddg_html("<html><body></body></html>").is_empty());
    }

    #[test]
    fn parse_ddg_html_caps_at_max_results() {
        // Build a fixture with 10 results; assert we get 8.
        let mut html = String::from("<html><body>");
        for i in 0..10 {
            html.push_str(&format!(
                r#"<div class="result"><div class="links_main"><h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F{i}">Result {i}</a></h2><div class="result__snippet">snippet {i}</div></div></div>"#
            ));
        }
        html.push_str("</body></html>");
        let results = parse_ddg_html(&html);
        assert_eq!(results.len(), 8);
    }

    #[test]
    fn extract_uddg_url_decodes_encoded_url() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpath&rut=abc";
        assert_eq!(
            extract_uddg_url(href),
            Some("https://example.com/path".to_string())
        );
    }

    #[test]
    fn extract_uddg_url_returns_none_without_uddg() {
        assert!(extract_uddg_url("//duckduckgo.com/l/?rut=abc").is_none());
        assert!(extract_uddg_url("https://example.com").is_none());
    }
}
