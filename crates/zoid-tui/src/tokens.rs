//! The single source of truth for glyphs and colors (spec §16). Values are
//! copied verbatim from docs/ux/README.md's visual-language table.

/// Glyphs (visual-language table, spec §16 / docs/ux/README.md).
pub mod glyph {
    pub const EDIT: char = '●';
    pub const PASS: char = '✓';
    pub const RUNNING: char = '◐';
    /// Tool indicator animation frames (moon-phase rotation, continuous while
    /// a tool is active). Spec: status-bar indicator refinement.
    pub const TOOL_FRAMES: [char; 4] = ['◐', '◑', '◓', '◒'];
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
    pub const MODE_SWITCH: char = '⇢'; // palette: switch mode
    pub const UNDO: char = '⤺'; // palette: undo (post-v1)
    pub const OPEN: char = '▤'; // palette: open file
    pub const EVICT: char = '✕'; // palette: evict cold
    pub const SETTINGS: char = '◆'; // palette: settings/quit
    pub const RECIPE: char = '▷'; // palette: run recipe (post-v1)
    pub const HEAT_FULL: char = '█'; // ⑤ heat bar — hot cell (Ⓡ4)
    pub const HEAT_SHADE: char = '░'; // ⑤ heat bar — empty cell (Ⓡ4)
    pub const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']; // churn sparkline ramp
    pub const PIN: char = '●'; // ⑤ pinned-item marker
    pub const ELLIPSIS: char = '…'; // collapsed-body marker (① collapse-to-signatures)
    pub const IDLE: char = '●'; // title-bar activity — waiting for the user (§2.2)
    pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']; // status-bar activity spinner — agent working (§2.2)
    pub const SCROLL_TRACK: char = '│'; // conversation scrollbar track (§16)
    pub const SCROLL_THUMB: char = '█'; // conversation scrollbar thumb (§16)
    pub const BULLET: char = '•'; // markdown unordered-list marker (§3.5)
    pub const QUOTE_BAR: char = '│'; // markdown blockquote bar (§3.5)
    pub const NEW: char = '+'; // palette: new session
    pub const RESUME: char = '↺'; // palette: resume session
    pub const RENAME: char = '✎'; // palette: rename session
    pub const MASK: char = '•'; // config screen: masked secret edit buffer
    pub const CODE_BAR: char = '▏'; // fenced-code container left rule (§3.5)
    pub const COPY: char = '⧉'; // code-block copy affordance (§3.5)
    pub const COMPACT: char = '⊟';
    /// Compaction indicator animation frames (box rotation, continuous while
    /// compaction is active). Spec: status-bar indicator refinement.
    pub const COMPACT_FRAMES: [char; 4] = ['⊟', '⊠', '⊞', '⊕']; // ⑤ compacted tool-result marker (ACM-1)
    /// Compaction status spinner — a 6-frame box-shuffle ramp, animated at ~120ms
    /// (slower than the working spinner, signaling a different kind of work).
    /// Purple (color::BRANCH). Only shown while automated compaction is running.
    pub const COMPACT_SPINNER: [char; 6] = ['⊟', '⊞', '⊟', '⊕', '⊞', '⊕'];

    // GFM table box-drawing borders (§3.5 tables reuse the box-drawing set).
    pub const TABLE_H: char = '─';   // horizontal border
    pub const TABLE_V: char = '│';   // vertical separator
    pub const TABLE_TL: char = '┌';  // top-left corner
    pub const TABLE_TR: char = '┐';  // top-right corner
    pub const TABLE_BL: char = '└';  // bottom-left corner
    pub const TABLE_BR: char = '┘';  // bottom-right corner
    pub const TABLE_LT: char = '├';  // left tee
    pub const TABLE_RT: char = '┤';  // right tee
    pub const TABLE_TT: char = '┬';  // top tee
    pub const TABLE_BT: char = '┴';  // bottom tee
    pub const TABLE_CR: char = '┼';  // cross

    // Repo drawer line-prefix emojis (§16) — visual markers for at-a-glance scanning.
    pub const REPO_NAME: char = '📦';
    pub const REPO_WORKTREE: char = '🔧';
    pub const REPO_CHANGES: char = '📝';

    // Session drawer line-prefix emojis (§16) — visual markers for at-a-glance scanning.
    pub const SESS_NAME: char = '🟢';
    pub const SESS_MODEL: char = '🤖';
    pub const SESS_DURATION: char = '⌚';
    pub const SESS_CONTEXT: char = '📊';
    pub const SESS_CWD: char = '📁';
}

/// Colors (visual-language table, spec §16 / docs/ux/README.md).
pub mod color {
    use ratatui::style::Color;
    pub const CHAT_ACCENT: Color = Color::Rgb(0x58, 0xa6, 0xff);
    pub const BUILD_ACCENT: Color = Color::Rgb(0xe3, 0xb3, 0x41);
    pub const OK: Color = Color::Rgb(0x3f, 0xb9, 0x50);
    pub const WARN: Color = Color::Rgb(0xd2, 0x99, 0x22);
    /// Dimmed steady-state for the tool indicator (after the 600ms pulse).
    pub const WARN_DIM: Color = Color::Rgb(0x6a, 0x50, 0x14);
    pub const ERROR: Color = Color::Rgb(0xf8, 0x51, 0x49);
    pub const BRANCH: Color = Color::Rgb(0xbc, 0x8c, 0xff);
    /// Dimmed steady-state for the compaction indicator (after the 600ms pulse).
    pub const COMPACT_DIM: Color = Color::Rgb(0x4b, 0x3a, 0x6a);
    pub const DIM: Color = Color::Rgb(0x6e, 0x76, 0x81);
    pub const TXT: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
    pub const SEL_BG: Color = Color::Rgb(0x16, 0x33, 0x5c);
    pub const CHAT_BG: Color = Color::Rgb(0x0d, 0x2a, 0x4d);
    /// Dark-purple fill for the SELECT pill — the purple sibling of `CHAT_BG`,
    /// paired with the light-purple `BRANCH` glyph the way Chat pairs
    /// `CHAT_ACCENT` (light blue) on `CHAT_BG` (dark blue).
    pub const SELECT_BG: Color = Color::Rgb(0x2a, 0x1a, 0x4d);
    pub const BUILD_BG: Color = Color::Rgb(0x3d, 0x2a, 0x0a);
    // ⑤ context heat — reuse the status palette so the visual language stays uniform.
    pub const HEAT_HOT: Color = OK;
    pub const HEAT_WARM: Color = WARN;
    pub const HEAT_COLD: Color = DIM;

    // repo drawer changes line — reuses the status palette (§16: uniform language).
    pub const ADDED: Color = OK; // +added lines
    pub const REMOVED: Color = ERROR; // -removed lines

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
    // Fenced-code container panel background (§3.5) — a subtly elevated dark so a
    // code block reads as a contained artifact against the pane, without a border.
    pub const CODE_BG: Color = Color::Rgb(0x16, 0x1b, 0x22);

    // GFM table (spec GFM-table §3): border = DIM, header = the Chat accent.
    pub const TABLE_BORDER: Color = DIM;
    pub const TABLE_HEADER: Color = CHAT_ACCENT;

    pub const DELEGATE_BG: Color = Color::Rgb(0x15, 0x10, 0x1f); // ▸ delegated card bg (chat-mode.html .chip)
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
        assert_eq!(color::SELECT_BG, Color::Rgb(0x2a, 0x1a, 0x4d));
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
    fn code_container_tokens_present() {
        use ratatui::style::Color;
        assert_eq!(glyph::CODE_BAR, '▏'); // fenced-code left rule
        assert_eq!(glyph::COPY, '⧉'); // copy affordance
        assert_eq!(color::CODE_BG, Color::Rgb(0x16, 0x1b, 0x22));
    }

    #[test]
    fn table_tokens_present() {
        assert_eq!(glyph::TABLE_H, '─');
        assert_eq!(glyph::TABLE_V, '│');
        assert_eq!(glyph::TABLE_TL, '┌');
        assert_eq!(glyph::TABLE_TR, '┐');
        assert_eq!(glyph::TABLE_BL, '└');
        assert_eq!(glyph::TABLE_BR, '┘');
        assert_eq!(glyph::TABLE_LT, '├');
        assert_eq!(glyph::TABLE_RT, '┤');
        assert_eq!(glyph::TABLE_TT, '┬');
        assert_eq!(glyph::TABLE_BT, '┴');
        assert_eq!(glyph::TABLE_CR, '┼');
        assert_eq!(color::TABLE_BORDER, color::DIM);
        assert_eq!(color::TABLE_HEADER, color::CHAT_ACCENT);
    }

    #[test]
    fn session_group_tokens_present() {
        assert_eq!(glyph::NEW, '+');
        assert_eq!(glyph::RESUME, '↺');
        assert_eq!(glyph::RENAME, '✎');
    }

    #[test]
    fn repo_changes_colors_reuse_status_palette() {
        assert_eq!(color::ADDED, color::OK);
        assert_eq!(color::REMOVED, color::ERROR);
    }

    #[test]
    fn p5_delegate_token_present() {
        use ratatui::style::Color;
        // Card background from docs/ux/chat-mode.html `.chip` (#15101f).
        assert_eq!(color::DELEGATE_BG, Color::Rgb(0x15, 0x10, 0x1f));
    }

    #[test]
    fn acm1_compact_token_present() {
        assert_eq!(glyph::COMPACT, '⊟');
    }
}
