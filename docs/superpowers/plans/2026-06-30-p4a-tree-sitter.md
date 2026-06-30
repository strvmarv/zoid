# P4a · Ⓡ3 Tree-sitter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up read-side code intelligence (spec Ⓡ3) — a new pure `zoid-syntax` crate that turns source text into **syntax-highlight spans, selectable symbols, and fold regions** via tree-sitter — and surface highlighting in `zoid-tui` against the §16 syntax palette.

**Architecture:** `zoid-syntax` is a leaf crate (depends only on tree-sitter + grammars) that emits **pure data** — byte-range spans tagged with a `HlKind`, `Symbol`s, and `FoldRegion`s — with no ratatui dependency. `zoid-tui` depends on `zoid-syntax` and owns the one mapping from `HlKind` → `color::SYN_*`, plus a `highlight_lines` render helper. This is the **substrate phase**: highlighting is demonstrated via a `preview.rs` scene + snapshots, but live consumption (code-aware zoom collapse-to-signatures, symbol selection for object-verbs) lands in **P4c (① zoom)** and **P4d (④ verbs)** which consume this crate. Mirrors the P3 assembler precedent (pure primitive, wired later).

**Tech Stack:** Rust 2021, `tree-sitter` 0.24, `tree-sitter-highlight` 0.24, grammar crates (`tree-sitter-rust`, `tree-sitter-json`, `tree-sitter-toml-ng`, `tree-sitter-yaml`, `tree-sitter-md`), ratatui 0.29 (`TestBackend`/`insta` snapshots), proptest.

## Global Constraints

- **Crates & dep direction:** `zoid-core` (pure, no ratatui, **no tree-sitter**), `zoid-provider`, `zoid-tools`, `zoid-syntax` (**new** — tree-sitter only, no ratatui, no zoid-core), `zoid-tui` (deps core **+ syntax**), `zoid` bin. Never introduce a cycle. `zoid-syntax` produces pure data (byte ranges + enums); `zoid-tui` owns all `HlKind → Color` mapping and rendering.
- **Design tokens are the single source of truth (spec §16):** no literal hex colors or special glyphs outside `crates/zoid-tui/src/tokens.rs`. The tree-sitter syntax palette is **already authoritative** in `docs/ux/README.md` ("Syntax (tree-sitter Ⓡ3): keyword `#ff7b72` · fn `#d2a8ff` · type `#7ee787` · string `#a5d6ff` · number `#79c0ff` · comment `#8b949e`"); Task 5 transcribes those six values verbatim into `tokens.rs` — no new table row needed.
- **Grammar set (P4a scope decision, 2026-06-30):** bundle **rust, toml, json, yaml, markdown**. Any unbundled language (or a grammar that fails to resolve) falls back to `Language::PlainText` → no highlight/symbols/fold, never a hard error. `syntect` fallback is **not** in scope (post-P4).
- **Substrate-only (P4a scope decision):** do **not** wire highlighting into the live conversation, files drawer, or zoom. P4a delivers the crate + the `highlight_lines` helper + a `preview.rs` demonstration. Live consumption is P4c/P4d. (Same shape as P3's assembler being P5 substrate.)
- **`zoid-syntax` is the read-side intelligence boundary (spec §2/§3):** tree-sitter ≠ LSP — highlighting, folding, and symbol byte-ranges only. No diagnostics, no refactors, no `similar`/diff work (that rides with the diff drawer in a later phase).
- **UX testing is mandatory and multi-width:** the rendering task adds `TestBackend`+`insta` snapshots at **both 100×24 and 140×24** and a matching `crates/zoid-tui/examples/preview.rs` scene. Highlight/symbol/fold extraction are **pure functions with their own unit tests** in `zoid-syntax`.
- **TDD, DRY, YAGNI, frequent commits.**
- **No `Co-Authored-By` / co-author trailer in commits** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit. Accept new snapshots with `INSTA_UPDATE=always cargo test -p zoid-tui --test <file>` (cargo-insta is not installed), and review the `.snap` content before committing.

---

### Task 1: `zoid-syntax` crate scaffold + `Language` registry + parse smoke test

**Files:**
- Create: `crates/zoid-syntax/Cargo.toml`
- Create: `crates/zoid-syntax/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)
- Test: inline `#[cfg(test)]` in `lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `enum Language { Rust, Toml, Json, Yaml, Markdown, PlainText }` (derives `Debug, Clone, Copy, PartialEq, Eq, Hash`).
  - `fn Language::from_path(path: &str) -> Language` (by file extension; unknown → `PlainText`).
  - `pub(crate) fn ts_language(lang: Language) -> Option<tree_sitter::Language>` (`PlainText` and any not-yet-added grammar → `None`).
  - `fn parse(source: &str, lang: Language) -> Option<tree_sitter::Tree>`.

> **Why a parse smoke test first:** tree-sitter grammar crates are version/ABI-sensitive against the core crate. Task 1 locks the core API + the Rust grammar (both proven in `spikes/rust-spike`) before anything is built on top. Remaining grammars are added — with their own build-verification — in Task 4, where a bad version degrades to `PlainText` rather than blocking highlight/symbol work.

- [ ] **Step 1: Create the crate manifest**

`crates/zoid-syntax/Cargo.toml`:

```toml
[package]
name = "zoid-syntax"
version = "0.0.0"
edition.workspace = true

[dependencies]
tree-sitter = "0.24"
tree-sitter-rust = "0.23"

[dev-dependencies]
# (none yet; proptest added in Task 3)
```

> Grammars for toml/json/yaml/markdown are added in Task 4. Keep the core + rust only here so Task 1 builds clean and locks the proven API from `spikes/rust-spike` (tree-sitter 0.24.7, `tree_sitter_rust::LANGUAGE`).

- [ ] **Step 2: Add the crate to the workspace**

In the top-level `Cargo.toml`, extend `members`:

```toml
members = ["crates/zoid-core", "crates/zoid-provider", "crates/zoid-tui", "crates/zoid-tools", "crates/zoid-syntax", "crates/zoid"]
```

- [ ] **Step 3: Write the failing test**

`crates/zoid-syntax/src/lib.rs` (test module):

```rust
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
```

- [ ] **Step 4: Run it to confirm it fails**

Run: `cargo test -p zoid-syntax`
Expected: compile error — `Language` / `from_path` / `parse` don't exist.

- [ ] **Step 5: Implement**

`crates/zoid-syntax/src/lib.rs` (above the test module):

```rust
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
```

> `from_path`'s `.filter(|e| !e.contains('/'))` guards the `rsplit('.')` case where a path has no dot at all (e.g. `"noext"` → `rsplit` yields the whole string) by rejecting a "fake extension" that still contains a path separator; for `"noext"` the rsplit yields `"noext"` which has no `/` but also is not a known ext → `PlainText`. The directory-dot edge (`"a.b/c"`) is covered by the `/` check.

- [ ] **Step 6: Run tests to confirm they pass**

Run: `cargo test -p zoid-syntax`
Expected: PASS (3 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-syntax/Cargo.toml crates/zoid-syntax/src/lib.rs Cargo.toml
git commit -m "feat(syntax): zoid-syntax crate — Language registry + tree-sitter parse (rust)"
```

---

### Task 2: Syntax highlighting — `HlKind`, `HlSpan`, `highlight()`

**Files:**
- Modify: `crates/zoid-syntax/Cargo.toml` (add `tree-sitter-highlight`)
- Create: `crates/zoid-syntax/src/highlight.rs`
- Modify: `crates/zoid-syntax/src/lib.rs` (`pub mod highlight;`)
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `Language`, `ts_language`.
- Produces:
  - `enum HlKind { Keyword, Func, Type, Str, Number, Comment }` (derives `Debug, Clone, Copy, PartialEq, Eq`).
  - `struct HlSpan { pub start: usize, pub end: usize, pub kind: HlKind }` (byte offsets; `Debug, Clone, PartialEq, Eq`).
  - `fn highlight(source: &str, lang: Language) -> Vec<HlSpan>` — non-overlapping, in source order; uncaptured gaps are simply absent (the renderer fills them as plain text). `PlainText`/unsupported → empty `Vec`.

**Model:** Map tree-sitter highlight capture names to the **six** §16 syntax buckets. Everything not in the map is left uncaptured (plain). `tree-sitter-highlight` resolves capture precedence/nesting for us.

- [ ] **Step 1: Add the dependency**

In `crates/zoid-syntax/Cargo.toml`, under `[dependencies]`:

```toml
tree-sitter-highlight = "0.24"
```

- [ ] **Step 2: Write the failing tests**

`crates/zoid-syntax/src/highlight.rs` (test module):

```rust
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
        assert!(spans.iter().any(|s| s.kind == HlKind::Keyword && slice(src, s) == "fn"));
        // the function name is a Func span
        assert!(spans.iter().any(|s| s.kind == HlKind::Func && slice(src, s) == "greet"));
        // the string literal (including quotes) is a Str span
        assert!(spans.iter().any(|s| s.kind == HlKind::Str && slice(src, s).contains("hi")));
        // spans are in source order and non-overlapping
        for pair in spans.windows(2) {
            assert!(pair[0].end <= pair[1].start, "spans must not overlap: {pair:?}");
        }
    }

    #[test]
    fn rust_comment_is_classified() {
        let spans = highlight("// note\nfn x() {}\n", Language::Rust);
        assert!(spans.iter().any(|s| s.kind == HlKind::Comment));
    }

    #[test]
    fn plaintext_has_no_spans() {
        assert_eq!(highlight("anything at all", Language::PlainText), Vec::new());
    }
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid-syntax highlight`
Expected: compile error — `highlight`/`HlKind`/`HlSpan` undefined.

- [ ] **Step 4: Implement**

`crates/zoid-syntax/src/highlight.rs`:

```rust
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
```

In `crates/zoid-syntax/src/lib.rs`, add near the top: `pub mod highlight;` and a convenience re-export `pub use highlight::{highlight, HlKind, HlSpan};`.

> **Performance note (not P4a scope):** `config()` rebuilds the `HighlightConfiguration` per call. For the preview/substrate use this is fine. A `OnceLock<HashMap<Language, HighlightConfiguration>>` cache is a clean post-P4a refinement — leave a `// TODO(perf)` only if you wish; do not build it now (YAGNI).

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid-syntax highlight`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-syntax/Cargo.toml crates/zoid-syntax/src/highlight.rs crates/zoid-syntax/src/lib.rs
git commit -m "feat(syntax): highlight() — capture-name to §16 bucket spans (rust)"
```

---

### Task 3: Symbols + fold regions

**Files:**
- Modify: `crates/zoid-syntax/Cargo.toml` (add `proptest` dev-dep)
- Create: `crates/zoid-syntax/src/symbols.rs`
- Modify: `crates/zoid-syntax/src/lib.rs` (`pub mod symbols;`)
- Test: inline `mod tests` + a `proptest!` block.

**Interfaces:**
- Consumes: `Language`, `parse`.
- Produces:
  - `enum SymbolKind { Function, Struct, Enum, Trait }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `struct Symbol { pub name: String, pub kind: SymbolKind, pub start: usize, pub end: usize }` (`Debug, Clone, PartialEq, Eq`).
  - `fn symbols(source: &str, lang: Language) -> Vec<Symbol>` — top-of-tree-down, in source order (by `start`).
  - `struct FoldRegion { pub start: usize, pub end: usize }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `fn fold_regions(source: &str, lang: Language) -> Vec<FoldRegion>` — byte ranges of multi-line block bodies (collapse targets for ① zoom / diff folding).

> Grounded in `spikes/rust-spike/src/interop.rs`: `node.kind()`, `node.child_by_field_name("name")`, `node.byte_range()`, and `node.children(&mut node.walk())`.

- [ ] **Step 1: Add the dev-dependency**

In `crates/zoid-syntax/Cargo.toml`, under `[dev-dependencies]`:

```toml
proptest = "1"
```

- [ ] **Step 2: Write the failing tests**

`crates/zoid-syntax/src/symbols.rs` (test module):

```rust
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
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid-syntax symbols`
Expected: compile error — `symbols`/`fold_regions`/types undefined.

- [ ] **Step 4: Implement**

`crates/zoid-syntax/src/symbols.rs`:

```rust
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

/// Multi-line `block` bodies — the collapse targets for fold/zoom.
pub fn fold_regions(source: &str, lang: Language) -> Vec<FoldRegion> {
    let Some(tree) = parse(source, lang) else {
        return Vec::new();
    };
    let mut out: Vec<FoldRegion> = Vec::new();
    walk(tree.root_node(), &mut |node| {
        if node.kind() == "block" {
            let s = node.start_position().row;
            let e = node.end_position().row;
            if e > s {
                let r = node.byte_range();
                out.push(FoldRegion { start: r.start, end: r.end });
            }
        }
    });
    out.sort_by_key(|f| f.start);
    out
}
```

In `crates/zoid-syntax/src/lib.rs`: add `pub mod symbols;` and `pub use symbols::{fold_regions, symbols, FoldRegion, Symbol, SymbolKind};`.

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid-syntax`
Expected: PASS (all prior + 3 new + proptest).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-syntax/Cargo.toml crates/zoid-syntax/src/symbols.rs crates/zoid-syntax/src/lib.rs
git commit -m "feat(syntax): symbols() + fold_regions() (rust); proptest invariants"
```

---

### Task 4: Bundle the remaining grammars (toml/json/yaml/markdown) with graceful fallback

**Files:**
- Modify: `crates/zoid-syntax/Cargo.toml` (add grammar deps)
- Modify: `crates/zoid-syntax/src/lib.rs` (`ts_language` arms)
- Modify: `crates/zoid-syntax/src/highlight.rs` (`highlights_query` arms)
- Test: inline in `lib.rs`.

**Interfaces:**
- Consumes / Produces: extends `ts_language` and `highlights_query` to cover Toml/Json/Yaml/Markdown. No new public types.

> **ABI risk + graceful degradation:** grammar crate versions must be ABI-compatible with `tree-sitter` 0.24. Use the versions below as a starting point; if `cargo build` reports an ABI/`LanguageFn` mismatch for one grammar, pin to the newest version whose changelog lists tree-sitter 0.24/0.25 support, or — if none resolves cleanly — **leave that arm returning the default (so the language stays `PlainText`-equivalent: parses to `None`, no highlight) and note it in the commit body.** The `PlainText` fallback means an omitted grammar is a graceful capability gap, not a build failure. Do **not** let one stubborn grammar block the task.

- [ ] **Step 1: Add the grammar dependencies**

In `crates/zoid-syntax/Cargo.toml`, under `[dependencies]`:

```toml
tree-sitter-json = "0.24"
tree-sitter-toml-ng = "0.7"
tree-sitter-yaml = "0.7"
tree-sitter-md = "0.3"
```

> `tree-sitter-toml-ng` is the maintained TOML grammar (the original `tree-sitter-toml` is stale). `tree-sitter-md` exposes a block-level `LANGUAGE`; markdown's inline grammar + injections are out of scope — block-level highlight/fold is acceptable for P4a.

- [ ] **Step 2: Write the failing tests**

In `crates/zoid-syntax/src/lib.rs` test module, add:

```rust
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
```

> If, per the ABI note, a specific grammar cannot be resolved and you intentionally leave it `PlainText`-equivalent, weaken the corresponding assertion to document the gap (e.g. drop `Yaml` from the parse list) and explain in the commit body. JSON is the most stable; keep its highlight assertion.

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid-syntax`
Expected: FAIL — `parse(..., Language::Json)` is `None` (arm still returns default).

- [ ] **Step 4: Implement**

In `crates/zoid-syntax/src/lib.rs`, extend `ts_language`:

```rust
pub(crate) fn ts_language(lang: Language) -> Option<tree_sitter::Language> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        Language::Json => Some(tree_sitter_json::LANGUAGE.into()),
        Language::Toml => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        Language::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
        Language::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
        Language::PlainText => None,
    }
}
```

> If a grammar crate exposes its language as a function (`tree_sitter_yaml::language()`) rather than a `LANGUAGE: LanguageFn` const, adapt the arm accordingly — check the crate's docs at build time. Both shapes convert into `tree_sitter::Language`.

In `crates/zoid-syntax/src/highlight.rs`, extend `highlights_query`:

```rust
fn highlights_query(lang: Language) -> Option<&'static str> {
    match lang {
        Language::Rust => Some(tree_sitter_rust::HIGHLIGHTS_QUERY),
        Language::Json => Some(tree_sitter_json::HIGHLIGHTS_QUERY),
        Language::Toml => Some(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        Language::Yaml => Some(tree_sitter_yaml::HIGHLIGHTS_QUERY),
        // markdown block grammar ships no standalone highlights query we map
        // cleanly to the six buckets; highlight is best-effort/empty for P4a.
        Language::Markdown | Language::PlainText => None,
    }
}
```

> If a grammar crate names its highlights constant differently (e.g. `HIGHLIGHT_QUERY` or `HIGHLIGHTS_QUERY_PATH`), use the actual exported name. If a grammar exposes no highlights query, return `None` for that arm (parse/symbols still work; highlight degrades to empty).

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid-syntax && cargo clippy -p zoid-syntax --all-targets`
Expected: PASS + zero warnings. (If you intentionally left a grammar out per the ABI note, the weakened test passes and the gap is documented.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-syntax/Cargo.toml crates/zoid-syntax/src/lib.rs crates/zoid-syntax/src/highlight.rs
git commit -m "feat(syntax): bundle toml/json/yaml/markdown grammars; PlainText fallback"
```

---

### Task 5: zoid-tui syntax tokens + `highlight_lines` render helper + preview scene + snapshots

**Files:**
- Modify: `crates/zoid-tui/Cargo.toml` (dep `zoid-syntax`)
- Modify: `crates/zoid-tui/src/tokens.rs` (add `color::SYN_*` + test)
- Create: `crates/zoid-tui/src/syntax_view.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod syntax_view;` + re-export)
- Modify: `crates/zoid-tui/examples/preview.rs` (add `syntax` scene)
- Create: `crates/zoid-tui/tests/syntax_snapshot.rs` (snapshots @100 & @140)
- Test: inline unit test in `syntax_view.rs` + the snapshots.

**Interfaces:**
- Consumes: `zoid_syntax::{highlight, Language, HlKind, HlSpan}`, `tokens::color`.
- Produces:
  - `color::SYN_KEYWORD`, `SYN_FUNC`, `SYN_TYPE`, `SYN_STRING`, `SYN_NUMBER`, `SYN_COMMENT: Color` (in `tokens.rs`).
  - `fn syn_color(kind: HlKind) -> ratatui::style::Color` (in `syntax_view.rs`).
  - `fn highlight_lines(source: &str, lang: Language) -> Vec<ratatui::text::Line<'static>>` — owned (`'static`) lines so callers can store them; uncaptured gaps render in `color::TXT`.

**§16 palette (verbatim from `docs/ux/README.md`):** keyword `#ff7b72` · fn `#d2a8ff` · type `#7ee787` · string `#a5d6ff` · number `#79c0ff` · comment `#8b949e`.

- [ ] **Step 1: Add the dependency**

In `crates/zoid-tui/Cargo.toml`, under `[dependencies]`:

```toml
zoid-syntax = { path = "../zoid-syntax" }
```

- [ ] **Step 2: Write the failing token test**

In `crates/zoid-tui/src/tokens.rs` `mod tests`, add:

```rust
#[test]
fn p4a_syntax_tokens_present() {
    use ratatui::style::Color;
    assert_eq!(color::SYN_KEYWORD, Color::Rgb(0xff, 0x7b, 0x72));
    assert_eq!(color::SYN_FUNC, Color::Rgb(0xd2, 0xa8, 0xff));
    assert_eq!(color::SYN_TYPE, Color::Rgb(0x7e, 0xe7, 0x87));
    assert_eq!(color::SYN_STRING, Color::Rgb(0xa5, 0xd6, 0xff));
    assert_eq!(color::SYN_NUMBER, Color::Rgb(0x79, 0xc0, 0xff));
    assert_eq!(color::SYN_COMMENT, Color::Rgb(0x8b, 0x94, 0x9e));
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib tokens::tests::p4a_syntax_tokens_present`
Expected: FAIL — `no associated item named SYN_KEYWORD`.

- [ ] **Step 4: Add the tokens**

In `crates/zoid-tui/src/tokens.rs`, in `mod color` (after `HEAT_COLD`):

```rust
    // Ⓡ3 tree-sitter syntax palette (spec §16 / docs/ux/README.md, verbatim).
    pub const SYN_KEYWORD: Color = Color::Rgb(0xff, 0x7b, 0x72);
    pub const SYN_FUNC: Color = Color::Rgb(0xd2, 0xa8, 0xff);
    pub const SYN_TYPE: Color = Color::Rgb(0x7e, 0xe7, 0x87);
    pub const SYN_STRING: Color = Color::Rgb(0xa5, 0xd6, 0xff);
    pub const SYN_NUMBER: Color = Color::Rgb(0x79, 0xc0, 0xff);
    pub const SYN_COMMENT: Color = Color::Rgb(0x8b, 0x94, 0x9e);
```

Run: `cargo test -p zoid-tui --lib tokens::tests::p4a_syntax_tokens_present`
Expected: PASS.

- [ ] **Step 5: Write the failing render-helper test**

`crates/zoid-tui/src/syntax_view.rs` (test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::color;
    use zoid_syntax::Language;

    #[test]
    fn highlight_lines_splits_rows_and_colors_keywords() {
        let lines = highlight_lines("fn x() {}\nlet y = 1;\n", Language::Rust);
        // one Line per source row (trailing newline produces no extra empty row)
        assert_eq!(lines.len(), 2);
        // the first line contains a keyword-colored span ("fn")
        let has_keyword = lines[0]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(color::SYN_KEYWORD) && s.content.contains("fn"));
        assert!(has_keyword, "`fn` should be keyword-colored");
    }

    #[test]
    fn plaintext_renders_one_txt_span_per_line() {
        let lines = highlight_lines("hello\nworld\n", Language::PlainText);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans.iter().all(|s| s.style.fg == Some(color::TXT)));
    }
}
```

- [ ] **Step 6: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib syntax_view`
Expected: compile error — module/`highlight_lines` undefined.

- [ ] **Step 7: Implement the render helper**

`crates/zoid-tui/src/syntax_view.rs`:

```rust
//! Pure render helper for Ⓡ3 tree-sitter highlighting. Turns source text into
//! ratatui `Line`s colored from the §16 syntax palette. No `Frame`; unit-tested
//! independently. Live wiring (zoom collapse, file peek, symbol select) lands
//! in P4c/P4d which consume `zoid_syntax` directly.

use crate::tokens::color;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use zoid_syntax::{highlight, HlKind, HlSpan, Language};

/// Map a syntax bucket to its §16 color.
pub fn syn_color(kind: HlKind) -> Color {
    match kind {
        HlKind::Keyword => color::SYN_KEYWORD,
        HlKind::Func => color::SYN_FUNC,
        HlKind::Type => color::SYN_TYPE,
        HlKind::Str => color::SYN_STRING,
        HlKind::Number => color::SYN_NUMBER,
        HlKind::Comment => color::SYN_COMMENT,
    }
}

/// Highlight `source` into owned ratatui `Line`s (one per source row). Spans
/// covered by an `HlSpan` get the syntax color; gaps render in `color::TXT`.
pub fn highlight_lines(source: &str, lang: Language) -> Vec<Line<'static>> {
    let spans = highlight(source, lang); // sorted, non-overlapping byte ranges
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut span_idx = 0usize;

    // Iterate the source per line, slicing against the byte-range highlight spans.
    let mut line_start = 0usize; // byte offset of the current line's start
    for raw_line in lines_of(source) {
        let line_end = line_start + raw_line.len();
        let mut cur = line_start;
        let mut out: Vec<Span<'static>> = Vec::new();

        // Advance past spans that ended before this line.
        while span_idx < spans.len() && spans[span_idx].end <= line_start {
            span_idx += 1;
        }

        let mut i = span_idx;
        while cur < line_end {
            // Find the next span that overlaps [cur, line_end).
            while i < spans.len() && spans[i].end <= cur {
                i += 1;
            }
            match spans.get(i) {
                Some(sp) if sp.start < line_end => {
                    let s = clamp(sp, cur, line_end);
                    if s.0 > cur {
                        out.push(plain(&source[cur..s.0]));
                    }
                    out.push(Span::styled(
                        source[s.0..s.1].to_string(),
                        Style::new().fg(syn_color(sp.kind)),
                    ));
                    cur = s.1;
                    if sp.end <= line_end {
                        i += 1;
                    }
                }
                _ => {
                    out.push(plain(&source[cur..line_end]));
                    cur = line_end;
                }
            }
        }
        if out.is_empty() {
            out.push(plain("")); // preserve empty lines
        }
        lines.push(Line::from(out));
        line_start = line_end + 1; // skip the '\n'
    }
    lines
}

fn plain(s: &str) -> Span<'static> {
    Span::styled(s.to_string(), Style::new().fg(color::TXT))
}

/// Intersection of an `HlSpan` with `[lo, hi)` as byte offsets.
fn clamp(sp: &HlSpan, lo: usize, hi: usize) -> (usize, usize) {
    (sp.start.max(lo), sp.end.min(hi))
}

/// Lines WITHOUT the trailing newline, dropping a single trailing empty
/// segment so "a\n" → ["a"], not ["a", ""], and "" → [].
fn lines_of(source: &str) -> Vec<&str> {
    if source.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<&str> = source.split('\n').collect();
    if source.ends_with('\n') {
        v.pop(); // drop the empty segment after the final newline
    }
    v
}
```

> **Implementer note:** the byte-offset bookkeeping above assumes spans never cross a line boundary. tree-sitter string/comment spans *can* be multi-line; if a snapshot shows a color bleeding past a line end, split multi-line `HlSpan`s at `\n` before rendering (add a small `split_at_newlines(spans, source)` pass) — but verify with the snapshot first; the common single-line case is covered. Keep `highlight_lines` the only place this logic lives (DRY).

- [ ] **Step 8: Wire the module**

In `crates/zoid-tui/src/lib.rs`: add `pub mod syntax_view;` and `pub use syntax_view::{highlight_lines, syn_color};`.

Run: `cargo test -p zoid-tui --lib syntax_view`
Expected: PASS (2 tests).

- [ ] **Step 9: Add the preview scene**

In `crates/zoid-tui/examples/preview.rs`, add a `syntax` scene that renders a highlighted Rust snippet. Follow the file's existing scene-dispatch pattern; the body:

```rust
// scene: "syntax" — Ⓡ3 highlight demonstration
let sample = "\
fn main() {\n    let name = \"zoid\";\n    let n = 42; // answer\n    greet(name, n);\n}\n";
let lines = zoid_tui::highlight_lines(sample, zoid_syntax::Language::Rust);
terminal.draw(|f| {
    f.render_widget(ratatui::widgets::Paragraph::new(lines), f.area());
}).unwrap();
```

Add `zoid-syntax = { path = "../zoid-syntax" }` to `crates/zoid-tui/Cargo.toml` `[dev-dependencies]` as well if the example cannot see it through the main dependency (examples use dev + normal deps; the Step 1 normal dep covers it — only add to dev-deps if the build complains).

- [ ] **Step 10: Write the snapshot tests**

`crates/zoid-tui/tests/syntax_snapshot.rs`:

```rust
use ratatui::{backend::TestBackend, widgets::Paragraph, Terminal};
use zoid_syntax::Language;
use zoid_tui::highlight_lines;

const SAMPLE: &str = "\
fn main() {
    let name = \"zoid\";
    let n = 42; // answer
    greet(name, n);
}
";

fn draw(w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let lines = highlight_lines(SAMPLE, Language::Rust);
    terminal
        .draw(|f| f.render_widget(Paragraph::new(lines), f.area()))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn syntax_highlight_frame() {
    insta::assert_snapshot!(draw(100, 24));
}

#[test]
fn syntax_highlight_wide_frame() {
    insta::assert_snapshot!(draw(140, 24));
}
```

- [ ] **Step 11: Accept snapshots and verify fidelity**

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test syntax_snapshot`
Then **read** `crates/zoid-tui/tests/snapshots/syntax_snapshot__syntax_highlight_frame.snap` and the wide variant. Confirm the structure renders (`fn`, the string `"zoid"`, the number `42`, and the `// answer` comment all appear on their lines). `TestBackend::to_string()` captures text content, not color; the unit test in Step 5/7 is what asserts the colors. Re-run without the env var to confirm green:
Run: `cargo test -p zoid-tui --test syntax_snapshot`
Expected: PASS. Also confirm `cargo run -p zoid-tui --example preview -- syntax 100 24` shows colored output in a truecolor terminal.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tui/Cargo.toml crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/syntax_view.rs crates/zoid-tui/src/lib.rs crates/zoid-tui/examples/preview.rs crates/zoid-tui/tests/syntax_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): §16 syntax tokens + highlight_lines render helper + preview/snapshots (Ⓡ3)"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `cargo run -p zoid-tui --example preview -- syntax 100 24` and `-- syntax 140 24` render highlighted Rust.
- [ ] `zoid-core` has **no** tree-sitter dependency (grep `crates/zoid-core/Cargo.toml`); `zoid-syntax` has **no** ratatui dependency (grep `crates/zoid-syntax/Cargo.toml`).
- [ ] No literal hex colors outside `tokens.rs`; the six `SYN_*` values match `docs/ux/README.md` verbatim.
- [ ] Highlighting is **not** wired into the live conversation/zoom (grep to confirm — P4a is substrate-only; consumption is P4c/P4d).
- [ ] Snapshots exist at both 100 and 140 for the syntax scene.
- [ ] If any grammar was left out per the Task 4 ABI note, the gap is documented in that commit's body and the `Language` arm degrades to `PlainText`-equivalent.

## Self-Review notes (author)

- **Spec coverage (Ⓡ3):** real syntax highlighting (T2 `highlight` + T5 `highlight_lines`/§16 tokens), accurate **symbol selection** byte-ranges (T3 `symbols`, proven against `spikes/rust-spike`), **structural folding** regions (T3 `fold_regions`). Code breadcrumbs + code-aware semantic zoom (collapse-to-signatures) are **consumers** of T3's symbols/folds and land in **P4c (① zoom)**; live symbol-select verbs land in **P4d (④)** — P4a deliberately ships the substrate + a preview demonstration (P3-assembler precedent).
- **Type consistency:** `Language` (T1) is the single language enum threaded through `highlight`/`symbols`/`fold_regions` (T2/T3) and `highlight_lines`/preview/snapshots (T5). `HlKind`/`HlSpan` defined in T2 are consumed unchanged by `syn_color`/`highlight_lines` in T5. `highlight()` returns sorted, non-overlapping spans (T2) — the invariant `highlight_lines` relies on.
- **Dep direction:** new leaf crate `zoid-syntax` (tree-sitter only) → `zoid-tui` depends on it → bin. `zoid-core` stays pure (no tree-sitter); `zoid-syntax` stays render-free (no ratatui). No cycle.
- **Risk isolation:** the version/ABI-fragile multi-grammar work is quarantined in T4 with an explicit graceful-degradation path (`PlainText` fallback), so the proven rust pipeline (T1–T3, grounded in the spike) is never blocked by a stubborn grammar crate.
- **§16:** the syntax palette was already authoritative in `docs/ux/README.md`; T5 only transcribes the six hex values into `tokens.rs` — no table edit, no new glyphs.
