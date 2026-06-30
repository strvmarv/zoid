//! Selectable symbols + fold regions (spec Ⓡ3): the primitives behind
//! object-first symbol selection (④, P4d) and code-aware zoom collapse (①,
//! P4c). Pure byte-range data.

use crate::{parse, Language};
use tree_sitter::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRegion {
    pub start: usize,
    pub end: usize,
}

/// Map a rust node kind to a `SymbolKind`, if it is one we surface.
fn symbol_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "function_item" => Some(SymbolKind::Function),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        _ => None,
    }
}

/// Walk every node once, depth-first, calling `f`.
fn walk(node: Node, f: &mut impl FnMut(Node)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, f);
    }
}

/// Top-level + nested named symbols, in source order.
pub fn symbols(source: &str, lang: Language) -> Vec<Symbol> {
    let Some(tree) = parse(source, lang) else {
        return Vec::new();
    };
    let mut out: Vec<Symbol> = Vec::new();
    walk(tree.root_node(), &mut |node| {
        if let Some(kind) = symbol_kind(node.kind()) {
            if let Some(name_node) = node.child_by_field_name("name") {
                let r = node.byte_range();
                out.push(Symbol {
                    name: source[name_node.byte_range()].to_string(),
                    kind,
                    start: r.start,
                    end: r.end,
                });
            }
        }
    });
    out.sort_by_key(|s| s.start);
    out
}

/// True for the rust node kinds whose multi-line bodies are collapse targets
/// for fold / code-aware zoom "collapse to signatures" (① P4c). Covers function
/// bodies (`block`) AND type/impl/trait bodies — the Rust grammar names the
/// latter `field_declaration_list` (struct), `enum_variant_list` (enum), and
/// `declaration_list` (impl + trait). Without the type-body kinds, "collapse to
/// signatures" would silently fold only fn bodies and leave every struct/enum/
/// impl fully expanded.
fn is_fold_body(node_kind: &str) -> bool {
    matches!(
        node_kind,
        "block" | "field_declaration_list" | "enum_variant_list" | "declaration_list"
    )
}

/// Multi-line collapsible bodies — the collapse targets for fold/zoom.
pub fn fold_regions(source: &str, lang: Language) -> Vec<FoldRegion> {
    let Some(tree) = parse(source, lang) else {
        return Vec::new();
    };
    let mut out: Vec<FoldRegion> = Vec::new();
    walk(tree.root_node(), &mut |node| {
        if is_fold_body(node.kind()) {
            let s = node.start_position().row;
            let e = node.end_position().row;
            if e > s {
                let r = node.byte_range();
                out.push(FoldRegion {
                    start: r.start,
                    end: r.end,
                });
            }
        }
    });
    out.sort_by_key(|f| f.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Language;
    use proptest::prelude::*;

    const SRC: &str = "\
struct Point { x: i32 }

fn area(p: Point) -> i32 {
    p.x * p.x
}

enum Dir { N, S }
";

    #[test]
    fn extracts_named_symbols_in_source_order() {
        // Malformed parsing would corrupt symbol output, so confirm the
        // fixture parses cleanly before trusting the extracted symbols.
        assert!(!parse(SRC, Language::Rust).unwrap().root_node().has_error());
        let syms = symbols(SRC, Language::Rust);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Point", "area", "Dir"]);
        assert_eq!(syms[0].kind, SymbolKind::Struct);
        assert_eq!(syms[1].kind, SymbolKind::Function);
        assert_eq!(syms[2].kind, SymbolKind::Enum);
        // byte ranges actually bracket the symbol text
        assert_eq!(&SRC[syms[1].start..syms[1].end][..2], "fn");
    }

    #[test]
    fn fold_region_covers_the_function_body() {
        let folds = fold_regions(SRC, Language::Rust);
        // the `{ p.x * p.x }` body spans more than one line → one fold region
        assert!(folds.iter().any(|f| {
            let body = &SRC[f.start..f.end];
            body.contains("p.x * p.x") && SRC[..f.start].contains("fn area")
        }));
    }

    #[test]
    fn fold_region_covers_type_and_impl_bodies() {
        // "Collapse to signatures" (① P4c) needs type/impl bodies to fold, not
        // just fn bodies. Each of these has a multi-line body that must produce
        // a fold region (field_declaration_list / enum_variant_list / declaration_list).
        const TYPES: &str = "\
struct Big {
    x: i32,
    y: i32,
}

enum Color {
    Red,
    Green,
}

impl Big {
    fn sum(&self) -> i32 {
        self.x + self.y
    }
}
";
        // Malformed parsing would corrupt fold output, so confirm the
        // fixture parses cleanly before trusting the extracted fold regions.
        assert!(!parse(TYPES, Language::Rust)
            .unwrap()
            .root_node()
            .has_error());
        let folds = fold_regions(TYPES, Language::Rust);
        let body = |needle: &str| folds.iter().any(|f| TYPES[f.start..f.end].contains(needle));
        assert!(body("x: i32"), "struct field body should fold");
        assert!(body("Red"), "enum variant body should fold");
        assert!(body("fn sum"), "impl body should fold");
    }

    #[test]
    fn plaintext_has_no_symbols_or_folds() {
        assert!(symbols("nothing", Language::PlainText).is_empty());
        assert!(fold_regions("nothing", Language::PlainText).is_empty());
    }

    proptest! {
        // Never panics and always returns in-bounds, ordered ranges.
        #[test]
        fn symbols_ranges_are_in_bounds_and_ordered(noise in "[a-zA-Z0-9 {}();\n]{0,200}") {
            let src = format!("fn f() {{}}\n{noise}\n");
            let syms = symbols(&src, Language::Rust);
            for s in &syms {
                prop_assert!(s.end <= src.len());
                prop_assert!(s.start <= s.end);
            }
            for pair in syms.windows(2) {
                prop_assert!(pair[0].start <= pair[1].start);
            }
        }
    }
}
