//! zoid-syntax — read-side code intelligence (spec Ⓡ3). Pure data only:
//! syntax-highlight spans, selectable symbols, and fold regions extracted via
//! tree-sitter. No ratatui, no zoid-core. `zoid-tui` maps the data onto the
//! §16 palette and renders it.

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

/// The tree-sitter grammar for a language, or `None` for `PlainText` and any
/// grammar not yet wired in (Task 4 adds toml/json/yaml/markdown).
pub(crate) fn ts_language(lang: Language) -> Option<tree_sitter::Language> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        _ => None,
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
}
