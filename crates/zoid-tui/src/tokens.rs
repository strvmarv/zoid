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
}
