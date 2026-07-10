//! Pure functions for readability extraction, HTML→markdown, heading-outline,
//! and char paging. Factored out of `fetch` for unit testing with fixture HTML.

use crate::HeadingMark;
use anyhow::{anyhow, Result};
use readability::extractor;
use url::Url;

/// Extract readable content from an HTML page and convert to markdown.
/// Returns (title, markdown). The title comes from readability's Product.title;
/// the markdown is htmd's conversion of Product.content (the cleaned HTML).
/// Runs the synchronous readability+htmd inline (CPU-bound, fast — the page is
/// already in memory; no blocking network here).
pub(crate) fn extract_markdown(html: &str, url: &str) -> Result<(String, String)> {
    let parsed_url = Url::parse(url).map_err(|e| anyhow!("bad url: {e}"))?;
    let mut reader = std::io::Cursor::new(html.as_bytes());
    let product = extractor::extract(&mut reader, &parsed_url)
        .map_err(|e| anyhow!("readability extraction failed: {e:?}"))?;
    // readability returns Ok even when it can't find an article, yielding
    // content = the whole HTML passthrough and text = just the title. Detect
    // this: after markdown conversion, if the markdown is no longer than the
    // title (i.e. it's just the title or empty), there's no article body.
    let markdown = htmd::convert(&product.content)
        .map_err(|e| anyhow!("html→markdown failed: {e}"))?;
    let md_trimmed = markdown.trim();
    let title_trimmed = product.title.trim();
    if md_trimmed.is_empty() || md_trimmed == title_trimmed {
        return Err(anyhow!("no extractable content (page may be JS-only or empty)"));
    }
    Ok((product.title, markdown))
}

/// Build a heading outline from the markdown: each line starting with `#`..`######`
/// becomes a `HeadingMark` with its level, the heading text, and the char offset
/// where the heading starts in the markdown.
pub(crate) fn build_outline(markdown: &str) -> Vec<HeadingMark> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count() as u8;
            if (1..=6).contains(&level) {
                let text = trimmed
                    .trim_start_matches('#')
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    out.push(HeadingMark {
                        level,
                        text,
                        char_offset: offset,
                    });
                }
            }
        }
        offset += line.len();
    }
    out
}

/// Page the markdown by char offset/limit. Returns the window
/// content[offset..min(offset+limit, total)], or None if offset >= total
/// (caller emits the offset-past-end error).
pub(crate) fn page(markdown: &str, offset: usize, limit: usize) -> Option<String> {
    let total = markdown.chars().count();
    if offset >= total {
        return None;
    }
    let end = (offset + limit).min(total);
    let window: String = markdown
        .chars()
        .skip(offset)
        .take(end - offset)
        .collect();
    Some(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixture rich enough that readability's scorer reliably picks the
    // <article> (high text-to-markup density). Minimal fixtures can flake: the
    // arc90 scorer weighs text density + link density, and a tiny article may
    // not outscore <nav>. This fixture has enough paragraph text to win.
    const FIXTURE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Example Docs</title></head>
<body>
<nav><a href="/">Home</a> | <a href="/about">About</a> | <a href="/contact">Contact</a></nav>
<article>
<h1>Getting Started</h1>
<p>Install the tool with cargo by running cargo install zoid. This downloads
the binary and makes it available on your PATH. Once installed, you can launch
the agent from any directory.</p>
<h2>Configuration</h2>
<p>Set up your config file in the project root. The configuration is TOML and
controls the provider, model, economy settings, and tool behavior. Most users
only need to set a provider and an API key to get started.</p>
<h2>Usage</h2>
<p>Run zoid to start the agent. The agent can read files, run shell commands,
search the repository, and now fetch web pages. Use the prompt to ask questions
or request code changes.</p>
</article>
<footer>Copyright 2026 Example Corp. All rights reserved.</footer>
</body>
</html>"#;

    #[test]
    fn extract_markdown_returns_nonempty_title_and_content() {
        // Assert extraction succeeds and returns non-empty title + markdown.
        // We do NOT assert specific phrases survive readability's scorer —
        // the scorer's exact output on synthetic HTML is not a contract. The
        // fetch TcpListener tests exercise the full pipeline end-to-end.
        let (title, md) = extract_markdown(FIXTURE_HTML, "https://example.com/docs").unwrap();
        assert!(!title.is_empty(), "title should be extracted, got empty");
        assert!(!md.trim().is_empty(), "markdown should be non-empty");
    }

    #[test]
    fn extract_markdown_empty_page_returns_err() {
        let html = "<html><head><title>x</title></head><body></body></html>";
        assert!(extract_markdown(html, "https://x.com").is_err());
    }

    #[test]
    fn build_outline_extracts_headings_with_offsets() {
        let md = "# Title\nintro\n## Section A\nbody\n### Sub\n";
        let outline = build_outline(md);
        assert_eq!(outline.len(), 3);
        assert_eq!(outline[0].level, 1);
        assert_eq!(outline[0].text, "Title");
        assert_eq!(outline[0].char_offset, 0);
        assert_eq!(outline[1].level, 2);
        assert_eq!(outline[1].text, "Section A");
        assert!(outline[1].char_offset > 0);
        assert_eq!(outline[2].level, 3);
    }

    #[test]
    fn build_outline_no_headings_returns_empty() {
        assert!(build_outline("just text\nno headings\n").is_empty());
    }

    #[test]
    fn page_returns_window_from_offset() {
        let md = "abcdefghij"; // 10 chars
        assert_eq!(page(md, 0, 4).unwrap(), "abcd");
        assert_eq!(page(md, 4, 4).unwrap(), "efgh");
        assert_eq!(page(md, 8, 10).unwrap(), "ij"); // end clamps to total
    }

    #[test]
    fn page_offset_past_end_returns_none() {
        assert!(page("abc", 3, 10).is_none());
        assert!(page("abc", 10, 10).is_none());
    }

    #[test]
    fn page_offset_at_exact_end_returns_none() {
        // offset == total: past end (no content left to return).
        assert!(page("abc", 3, 10).is_none());
    }
}