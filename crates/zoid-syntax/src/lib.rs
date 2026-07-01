//! zoid-syntax — read-side code intelligence (spec Ⓡ3). Pure data only:
//! syntax-highlight spans, selectable symbols, and fold regions extracted via
//! tree-sitter. No ratatui, no zoid-core. `zoid-tui` maps the data onto the
//! §16 palette and renders it.

pub mod highlight;
pub use highlight::{highlight, HlKind, HlSpan};

pub mod symbols;
pub use symbols::{fold_regions, symbols, FoldRegion, Symbol, SymbolKind};

/// Languages with a bundled tree-sitter grammar (spec §16 grammar set:
/// rust/toml/json/yaml/markdown). Everything else is `PlainText` and degrades
/// gracefully (no highlight/symbols/fold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Toml,
    Json,
    Yaml,
    Markdown,
    PlainText,
}

impl Language {
    /// Pick a language from a path's extension (case-insensitive). Unknown or
    /// extension-less paths → `PlainText`.
    pub fn from_path(path: &str) -> Language {
        let ext = path.rsplit('.').next().filter(|e| !e.contains('/'));
        match ext.map(str::to_ascii_lowercase).as_deref() {
            Some("rs") => Language::Rust,
            Some("toml") => Language::Toml,
            Some("json") => Language::Json,
            Some("yaml") | Some("yml") => Language::Yaml,
            Some("md") | Some("markdown") => Language::Markdown,
            _ => Language::PlainText,
        }
    }
}

/// The tree-sitter grammar for a language, or `None` for `PlainText`.
pub(crate) fn ts_language(lang: Language) -> Option<tree_sitter::Language> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        Language::Json => Some(tree_sitter_json::LANGUAGE.into()),
        Language::Toml => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        Language::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
        // tree-sitter-md exposes a block-level LANGUAGE (and a separate
        // INLINE_LANGUAGE for inline content); block-level parse/symbols/fold
        // is acceptable for P4a per the task brief.
        Language::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
        Language::PlainText => None,
    }
}

/// Parse `source` for `lang`. Returns `None` for `PlainText` / unsupported
/// grammars or if the parser cannot be configured.
pub fn parse(source: &str, lang: Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_language(lang)?).ok()?;
    parser.parse(source, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_maps_extensions() {
        assert_eq!(Language::from_path("src/main.rs"), Language::Rust);
        assert_eq!(Language::from_path("Cargo.toml"), Language::Toml);
        assert_eq!(Language::from_path("data.json"), Language::Json);
        assert_eq!(Language::from_path("ci.yaml"), Language::Yaml);
        assert_eq!(Language::from_path("ci.yml"), Language::Yaml);
        assert_eq!(Language::from_path("README.md"), Language::Markdown);
        assert_eq!(Language::from_path("a.bin"), Language::PlainText);
        assert_eq!(Language::from_path("noext"), Language::PlainText);
    }

    #[test]
    fn parses_rust_into_a_source_file() {
        let tree = parse("fn main() {}\n", Language::Rust).expect("rust parses");
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn plaintext_does_not_parse() {
        assert!(parse("anything", Language::PlainText).is_none());
    }

    #[test]
    fn bundled_grammars_parse_their_languages() {
        // Each bundled grammar must parse a trivial doc to a non-error root.
        assert!(parse("{\"k\": 1}\n", Language::Json).is_some());
        assert!(parse("k = 1\n", Language::Toml).is_some());
        assert!(parse("k: 1\n", Language::Yaml).is_some());
        assert!(parse("# Title\n", Language::Markdown).is_some());
    }

    #[test]
    fn json_highlights_strings_and_numbers() {
        use crate::highlight::{highlight, HlKind};
        let spans = highlight("{\"name\": 42}\n", Language::Json);
        assert!(spans.iter().any(|s| s.kind == HlKind::Str));
        assert!(spans.iter().any(|s| s.kind == HlKind::Number));
    }

    #[test]
    fn toml_and_yaml_emit_some_highlight_spans() {
        use crate::highlight::highlight;
        // Don't over-assert capture kinds (grammar query names vary); just prove the
        // wired query actually produces spans, so a misnamed HIGHLIGHTS_QUERY (which
        // silently yields zero spans) is caught instead of passing as "no highlight".
        assert!(
            !highlight("k = \"v\"\n", Language::Toml).is_empty(),
            "toml emits spans"
        );
        assert!(
            !highlight("k: \"v\"\n", Language::Yaml).is_empty(),
            "yaml emits spans"
        );
    }
}
