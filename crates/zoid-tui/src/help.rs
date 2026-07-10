//! The static keyboard-shortcuts reference shown by the Help overlay
//! (`Overlay::Help`). Pure content: one styled line per row, grouped into
//! sections. Kept in one place so the overlay and its test stay in sync.
//! NOTE: this is a hand-maintained mirror of the keymap in `route.rs`; when a
//! binding changes there, update the matching row here.

use crate::tokens::color;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Target size of the Help overlay rect (clamped to the conversation area by
/// `layout::centered`). Larger than the 72x18 palette box because the help
/// reference is denser. Defined here so the size has a single source of truth.
pub const HELP_RECT_W: u16 = 84;
pub const HELP_RECT_H: u16 = 26;

/// Build the help overlay's content as styled lines: dim section headers,
/// normal-text shortcut rows. Pure; no terminal or state.
pub fn help_lines() -> Vec<Line<'static>> {
    // (section header, then (keys, description) rows). Keep descriptions short.
    let sections: &[(&str, &[(&str, &str)])] = &[
        ("Global", &[
            ("Ctrl+P", "command palette"),
            ("Ctrl+O", "object / action picker"),
            ("Ctrl+Q", "quit zoid"),
            ("Esc / Ctrl+C", "cancel turn (Esc again forces)"),
            ("Shift+Tab", "switch mode"),
            ("Tab", "change focus"),
            ("Alt+P", "switch provider / model"),
            ("Alt+Left / Right", "semantic zoom"),
            ("?", "this help (conversation)"),
        ]),
        ("Input", &[
            ("Enter", "send message"),
            ("Shift+Enter", "newline (or Alt+Enter)"),
            (":", "command palette (empty box)"),
            ("Shift+Del", "delete line"),
            ("Shift+Home / End", "cursor to start / end"),
        ]),
        ("Conversation", &[
            ("j / Down", "scroll down"),
            ("k / Up", "scroll up"),
            ("= / -", "zoom in / out"),
            ("Shift+Home / End", "scroll to top / bottom"),
            ("Esc", "return to input"),
        ]),
        ("Overlays", &[
            ("Up / Down", "move selection"),
            ("Enter", "choose"),
            ("Esc / q", "close"),
        ]),
        ("Commands", &[
            (":help", "this help"),
            (":compact", "condense the session"),
            (":config", "settings"),
            (":feedback", "send feedback"),
            (":mode install superpowers", "install skills"),
            (":q", "quit"),
        ]),
        ("Mouse", &[
            ("scroll", "scroll conversation"),
            ("Ctrl+scroll", "semantic zoom"),
        ]),
    ];

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, (header, rows)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from("")); // blank between sections
        }
        lines.push(Line::from(Span::styled(
            header.to_string(),
            Style::new().fg(color::DIM),
        )));
        for (keys, desc) in *rows {
            let row = format!("  {keys:<22}{desc}");
            lines.push(Line::from(Span::styled(row, Style::new().fg(color::TXT))));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined() -> String {
        help_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect()
    }

    #[test]
    fn lists_core_shortcuts_and_sections() {
        let s = joined();
        for token in [
            "Global", "Input", "Conversation", "Overlays", "Commands",
            "Ctrl+P", "Ctrl+Q", "Shift+Tab", "Alt+P", "Esc", "?", ":help",
        ] {
            assert!(s.contains(token), "help must mention {token:?}: {s:?}");
        }
    }

    #[test]
    fn has_a_dim_section_header() {
        assert!(
            help_lines()
                .iter()
                .any(|l| l.spans.iter().any(|sp| sp.style.fg == Some(color::DIM))),
            "at least one section header must use the DIM color"
        );
    }

    /// Rows must stay compact so they don't clip on a typical (rail-visible,
    /// ~50-60 col) conversation width. Keep the widest logical row modest.
    #[test]
    fn rows_are_reasonably_narrow() {
        for l in help_lines() {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(w <= 56, "row too wide ({w}): {l:?}");
        }
    }
}
