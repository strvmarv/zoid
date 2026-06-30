//! Syntax highlighting (spec Ⓡ3): map tree-sitter highlight captures onto the
//! six §16 syntax buckets. Pure — returns byte-range spans; `zoid-tui` maps
//! `HlKind` to colors.

use crate::{ts_language, Language};
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

/// The six syntax buckets in the §16 palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlKind {
    Keyword,
    Func,
    Type,
    Str,
    Number,
    Comment,
}

/// A highlighted byte range. `start`/`end` are byte offsets into the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HlKind,
}

/// Capture names we configure tree-sitter-highlight with, paired with the
/// bucket they map to. Order defines the `Highlight(idx)` index space.
const CAPTURES: &[(&str, HlKind)] = &[
    ("keyword", HlKind::Keyword),
    ("keyword.function", HlKind::Keyword),
    ("keyword.control", HlKind::Keyword),
    ("keyword.operator", HlKind::Keyword),
    ("function", HlKind::Func),
    ("function.method", HlKind::Func),
    ("function.macro", HlKind::Func),
    ("type", HlKind::Type),
    ("type.builtin", HlKind::Type),
    ("constructor", HlKind::Type),
    ("string", HlKind::Str),
    ("string.special", HlKind::Str),
    ("number", HlKind::Number),
    ("constant.numeric", HlKind::Number),
    ("constant.builtin", HlKind::Number),
    ("comment", HlKind::Comment),
];

fn highlights_query(lang: Language) -> Option<&'static str> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::HIGHLIGHTS_QUERY),
        // toml/json/yaml/markdown queries are added in Task 4.
        _ => None,
    }
}

fn config(lang: Language) -> Option<HighlightConfiguration> {
    let names: Vec<&str> = CAPTURES.iter().map(|(n, _)| *n).collect();
    // TODO(perf): rebuilds the HighlightConfiguration on every call; a
    // OnceLock<HashMap<Language, HighlightConfiguration>> cache is a clean
    // post-P4a refinement (not needed for the preview/substrate use today).
    let mut cfg = HighlightConfiguration::new(
        ts_language(lang)?,
        "source",
        highlights_query(lang)?,
        "", // injections
        "", // locals
    )
    .ok()?;
    cfg.configure(&names);
    Some(cfg)
}

/// Highlight `source`. Returns non-overlapping spans in source order; gaps are
/// uncaptured (the renderer treats them as plain text).
pub fn highlight(source: &str, lang: Language) -> Vec<HlSpan> {
    let Some(cfg) = config(lang) else {
        return Vec::new();
    };
    let mut hl = Highlighter::new();
    let events = match hl.highlight(&cfg, source.as_bytes(), None, |_| None) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<HlSpan> = Vec::new();
    let mut stack: Vec<HlKind> = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(Highlight(i))) => {
                // `i` indexes into the configured `names`, parallel to CAPTURES.
                stack.push(CAPTURES[i].1);
            }
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(&kind) = stack.last() {
                    out.push(HlSpan { start, end, kind });
                }
            }
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Language;

    fn slice<'a>(src: &'a str, s: &HlSpan) -> &'a str {
        &src[s.start..s.end]
    }

    #[test]
    fn rust_keyword_fn_string_are_classified() {
        let src = r#"fn greet() { let s = "hi"; }"#;
        let spans = highlight(src, Language::Rust);
        assert!(!spans.is_empty(), "rust must produce highlight spans");
        // `fn` is a keyword
        assert!(spans
            .iter()
            .any(|s| s.kind == HlKind::Keyword && slice(src, s) == "fn"));
        // the function name is a Func span
        assert!(spans
            .iter()
            .any(|s| s.kind == HlKind::Func && slice(src, s) == "greet"));
        // the string literal (including quotes) is a Str span
        assert!(spans
            .iter()
            .any(|s| s.kind == HlKind::Str && slice(src, s).contains("hi")));
        // spans are in source order and non-overlapping
        for pair in spans.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "spans must not overlap: {pair:?}"
            );
        }
    }

    #[test]
    fn rust_comment_is_classified() {
        let spans = highlight("// note\nfn x() {}\n", Language::Rust);
        assert!(spans.iter().any(|s| s.kind == HlKind::Comment));
    }

    #[test]
    fn plaintext_has_no_spans() {
        assert_eq!(
            highlight("anything at all", Language::PlainText),
            Vec::new()
        );
    }
}
