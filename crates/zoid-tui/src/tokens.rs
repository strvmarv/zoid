//! The single source of truth for glyphs and colors (spec §16). Values are
//! copied verbatim from docs/ux/README.md's visual-language table.

/// Glyphs (visual-language table, spec §16 / docs/ux/README.md).
pub mod glyph {
    pub const EDIT: char = '●';
    pub const PASS: char = '✓';
    pub const RUNNING: char = '◐';
    pub const PENDING: char = '☐';
    pub const STREAM: char = '⠿';
    pub const BRANCH: char = '⎇';
    pub const BLOCKER: char = '⛔';
    pub const USER_TURN: char = '›';
    pub const CARET: char = '▌';
    pub const SHIFT: char = '⇧';
    pub const RETURN: char = '⏎';
    pub const WARNING: char = '⚠';
    pub const COLLAPSED: char = '▸';
    pub const EXPANDED: char = '▾';
    pub const CONTEXT: char = '⑤';      // context-economy ⑤ motif
    pub const MODE_SWITCH: char = '⇢';  // palette: switch mode
    pub const UNDO: char = '⤺';         // palette: undo (post-v1)
    pub const OPEN: char = '▤';         // palette: open file
    pub const EVICT: char = '✕';        // palette: evict cold
    pub const SETTINGS: char = '◆';     // palette: settings/quit
    pub const RECIPE: char = '▷';       // palette: run recipe (post-v1)
    pub const HEAT_FULL: char = '█';   // ⑤ heat bar — hot cell (Ⓡ4)
    pub const HEAT_SHADE: char = '░';  // ⑤ heat bar — empty cell (Ⓡ4)
    pub const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']; // churn sparkline ramp
    pub const PIN: char = '●';         // ⑤ pinned-item marker
    pub const ELLIPSIS: char = '…';     // collapsed-body marker (① collapse-to-signatures)
    pub const IDLE: char = '●';        // title-bar activity — waiting for the user (§2.2)
    pub const BULLET: char = '•';      // markdown unordered-list marker (§3.5)
    pub const QUOTE_BAR: char = '│';   // markdown blockquote bar (§3.5)
    pub const NEW: char = '＋';        // palette: new session
    pub const RESUME: char = '↺';      // palette: resume session
    pub const RENAME: char = '✎';      // palette: rename session
}

/// Colors (visual-language table, spec §16 / docs/ux/README.md).
pub mod color {
    use ratatui::style::Color;
    pub const CHAT_ACCENT: Color = Color::Rgb(0x58, 0xa6, 0xff);
    pub const BUILD_ACCENT: Color = Color::Rgb(0xe3, 0xb3, 0x41);
    pub const OK: Color = Color::Rgb(0x3f, 0xb9, 0x50);
    pub const WARN: Color = Color::Rgb(0xd2, 0x99, 0x22);
    pub const ERROR: Color = Color::Rgb(0xf8, 0x51, 0x49);
    pub const BRANCH: Color = Color::Rgb(0xbc, 0x8c, 0xff);
    pub const DIM: Color = Color::Rgb(0x6e, 0x76, 0x81);
    pub const TXT: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
    pub const SEL_BG: Color = Color::Rgb(0x16, 0x33, 0x5c);
    pub const CHAT_BG: Color = Color::Rgb(0x0d, 0x2a, 0x4d);
    pub const BUILD_BG: Color = Color::Rgb(0x3d, 0x2a, 0x0a);
    // ⑤ context heat — reuse the status palette so the visual language stays uniform.
    pub const HEAT_HOT: Color = OK;
    pub const HEAT_WARM: Color = WARN;
    pub const HEAT_COLD: Color = DIM;

    // Ⓡ3 tree-sitter syntax palette (spec §16 / docs/ux/README.md, verbatim).
    pub const SYN_KEYWORD: Color = Color::Rgb(0xff, 0x7b, 0x72);
    pub const SYN_FUNC: Color = Color::Rgb(0xd2, 0xa8, 0xff);
    pub const SYN_TYPE: Color = Color::Rgb(0x7e, 0xe7, 0x87);
    pub const SYN_STRING: Color = Color::Rgb(0xa5, 0xd6, 0xff);
    pub const SYN_NUMBER: Color = Color::Rgb(0x79, 0xc0, 0xff);
    pub const SYN_COMMENT: Color = Color::Rgb(0x8b, 0x94, 0x9e);

    // Markdown message rendering (spec §3.5) — reuse the existing palette so the
    // visual language stays uniform: inline/fenced `code` = string hue, links = accent.
    pub const MD_CODE: Color = SYN_STRING;
    pub const MD_LINK: Color = CHAT_ACCENT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn tokens_match_visual_language_table() {
        assert_eq!(glyph::BRANCH, '⎇');
        assert_eq!(glyph::USER_TURN, '›');
        assert_eq!(color::CHAT_ACCENT, Color::Rgb(0x58, 0xa6, 0xff));
        assert_eq!(color::OK, Color::Rgb(0x3f, 0xb9, 0x50));
    }

    #[test]
    fn p2_tokens_present() {
        assert_eq!(glyph::COLLAPSED, '▸');
        assert_eq!(glyph::EXPANDED, '▾');
        assert_eq!(color::SEL_BG, Color::Rgb(0x16, 0x33, 0x5c));
        assert_eq!(color::CHAT_BG, Color::Rgb(0x0d, 0x2a, 0x4d));
        assert_eq!(color::BUILD_BG, Color::Rgb(0x3d, 0x2a, 0x0a));
    }

    #[test]
    fn p3_economy_tokens_present() {
        assert_eq!(glyph::HEAT_FULL, '█');
        assert_eq!(glyph::HEAT_SHADE, '░');
        assert_eq!(glyph::SPARK, ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']);
        assert_eq!(glyph::PIN, '●');
        // Heat colors reuse the status palette (spec §16: uniform language).
        assert_eq!(color::HEAT_HOT, color::OK);
        assert_eq!(color::HEAT_WARM, color::WARN);
        assert_eq!(color::HEAT_COLD, color::DIM);
    }

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

    #[test]
    fn p4c_collapse_token_present() {
        assert_eq!(glyph::ELLIPSIS, '…');
    }

    #[test]
    fn chat_polish_activity_token_present() {
        assert_eq!(glyph::IDLE, '●'); // title-bar idle activity indicator (§2.2)
        assert_eq!(glyph::STREAM, '⠿'); // running indicator reuses the stream glyph
    }

    #[test]
    fn markdown_tokens_present() {
        assert_eq!(glyph::BULLET, '•');
        assert_eq!(glyph::QUOTE_BAR, '│');
        assert_eq!(color::MD_CODE, color::SYN_STRING); // inline/`code` reuses the string hue
        assert_eq!(color::MD_LINK, color::CHAT_ACCENT); // links use the Chat accent
    }

    #[test]
    fn session_group_tokens_present() {
        assert_eq!(glyph::NEW, '＋');
        assert_eq!(glyph::RESUME, '↺');
        assert_eq!(glyph::RENAME, '✎');
    }
}
