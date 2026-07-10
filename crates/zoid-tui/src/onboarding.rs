//! Empty-state guidance rendered in the conversation pane when a session has
//! no messages. Two flavors: onboarding copy for first-time users, a brief
//! "welcome back" for returning users. Pure — no terminal, no state; the bin
//! calls it and paints the result into `BodyCache.body`.

use crate::tokens::{color, glyph};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The title line shown to a first-time user.
const NEW_USER_TITLE: &str = "zoid — a coding agent for your terminal";
/// The intro line above the suggested prompts.
const NEW_USER_INTRO: &str = "Try one of these to get started:";
/// Suggested first prompts for a new user. Static text in v1 (not clickable).
const NEW_USER_PROMPTS: &[&str] = &[
    "explain this codebase to me",
    "fix the failing tests",
    "add a feature from docs/TODO.md",
];
/// The hint shown to a returning user with an empty session.
const RETURNING_HINT: &str =
    "welcome back — type a message, or :resume to pick up another session";
/// The instructional line offering the Superpowers skill set install, shown
/// to first-time users who haven't installed it yet.
const SUPERPOWERS_OFFER: &str =
    "Run :plugin install superpowers to install the Superpowers skill set (structured TDD, debugging, planning, and review workflows).";
/// Discoverability hint pointing at the keyboard-shortcuts overlay. Shown on
/// every empty session (new and returning). Mentions `:help` as well as `?`
/// because the input box is focused by default, where `?` is a literal char.
const HELP_HINT: &str = "Press ? (or run :help) for keyboard shortcuts";

/// Build the empty-state lines for the conversation pane. `first_time_user`
/// selects onboarding copy (new user) vs. a welcome-back hint (returning user).
/// `offer_superpowers` appends an instructional line pointing at
/// `:mode install superpowers` (new-user path only). `width` is the text
/// column width for prose wrapping (same `width` the transcript body is
/// wrapped to). Pure; no terminal or state.
pub fn empty_state_lines(
    first_time_user: bool,
    offer_superpowers: bool,
    width: usize,
) -> Vec<Line<'static>> {
    if first_time_user {
        new_user_lines(offer_superpowers, width)
    } else {
        returning_user_lines(width)
    }
}

fn new_user_lines(offer_superpowers: bool, width: usize) -> Vec<Line<'static>> {
    let indent = "  ";
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title (accent, bold).
    for w in wrap_title(indent, NEW_USER_TITLE, width) {
        lines.push(Line::from(Span::styled(
            w,
            Style::new().fg(color::CHAT_ACCENT).bold(),
        )));
    }

    // Blank separator.
    lines.push(Line::from(""));

    // Intro (dim).
    for w in crate::render::wrap_plain(
        &format!("{indent}{NEW_USER_INTRO}"),
        width,
    ) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::DIM))));
    }

    // Prompts: › <prompt> (marker in accent, text in TXT).
    for prompt in NEW_USER_PROMPTS {
        let row = format!("{indent}  {} {}", glyph::USER_TURN, prompt);
        for w in crate::render::wrap_plain(&row, width) {
            lines.push(Line::from(vec![
                Span::styled(
                    // The › marker is the first non-space char; wrap_plain may
                    // break a long prompt across lines, but the marker only
                    // appears on the first row. For wrapped continuations the
                    // whole row is TXT (the marker is embedded in the string).
                    w.clone(),
                    Style::new().fg(color::TXT),
                ),
            ]));
        }
    }

    if offer_superpowers {
        lines.push(Line::from(""));
        for w in wrap_title(indent, SUPERPOWERS_OFFER, width) {
            lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
        }
    }

    lines.push(Line::from(""));
    for w in wrap_title(indent, HELP_HINT, width) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
    }

    lines
}

/// Wrap the title line, preserving the 2-space indent on continuation rows.
/// `wrap_plain` breaks on whitespace; we pass the full indented string and let
/// it wrap naturally.
fn wrap_title(indent: &str, title: &str, width: usize) -> Vec<String> {
    let full = format!("{indent}{title}");
    crate::render::wrap_plain(&full, width)
}

fn returning_user_lines(width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> =
        crate::render::wrap_plain(&format!("  {RETURNING_HINT}"), width)
            .into_iter()
            .map(|w| Line::from(Span::styled(w, Style::new().fg(color::DIM))))
            .collect();
    lines.push(Line::from(""));
    for w in wrap_title("  ", HELP_HINT, width) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first-time user sees the title, the intro, and all three suggested
    /// prompts. The title line carries the accent color.
    #[test]
    fn new_user_renders_title_and_prompts() {
        let lines = empty_state_lines(true, false, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(NEW_USER_TITLE),
            "title must appear: got {joined:?}"
        );
        assert!(
            joined.contains(NEW_USER_INTRO),
            "intro must appear: got {joined:?}"
        );
        for prompt in NEW_USER_PROMPTS {
            assert!(
                joined.contains(prompt),
                "prompt '{prompt}' must appear: got {joined:?}"
            );
        }
        // The title line carries the accent color.
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::CHAT_ACCENT))),
            "at least one line must use the accent color"
        );
    }

    /// A returning user sees only the welcome-back hint; none of the onboarding
    /// prompts appear.
    #[test]
    fn returning_user_renders_welcome_back_only() {
        let lines = empty_state_lines(false, false, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(RETURNING_HINT),
            "welcome-back hint must appear: got {joined:?}"
        );
        for prompt in NEW_USER_PROMPTS {
            assert!(
                !joined.contains(prompt),
                "onboarding prompt '{prompt}' must NOT appear for returning user: got {joined:?}"
            );
        }
        assert!(
            !joined.contains(NEW_USER_TITLE),
            "onboarding title must NOT appear for returning user: got {joined:?}"
        );
    }

    /// A very narrow width must not panic — `wrap_plain` handles it.
    #[test]
    fn wrap_respects_narrow_width() {
        // Both branches must survive a width of 10 without panicking.
        let _ = empty_state_lines(true, false, 10);
        let _ = empty_state_lines(false, false, 10);
        // Width 1 is degenerate but must not panic either.
        let _ = empty_state_lines(true, false, 1);
        let _ = empty_state_lines(false, false, 1);
    }

    /// The `›` glyph (USER_TURN) must appear in the new-user output.
    #[test]
    fn new_user_uses_turn_glyph() {
        let lines = empty_state_lines(true, false, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(glyph::USER_TURN),
            "the › turn glyph must appear in new-user output"
        );
    }

    #[test]
    fn superpowers_offer_line_shown_only_when_offered() {
        let joined = |ls: &[ratatui::text::Line]| ls.iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        let with = empty_state_lines(true, true, 80);
        let without = empty_state_lines(true, false, 80);
        assert!(joined(&with).contains(":plugin install superpowers"));
        assert!(!joined(&without).contains("Superpowers"));
    }

    #[test]
    fn help_hint_shown_in_both_empty_states() {
        let joined = |ls: &[ratatui::text::Line]| {
            ls.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                .collect::<String>()
        };
        assert!(joined(&empty_state_lines(true, true, 80)).contains(":help"));
        assert!(joined(&empty_state_lines(true, false, 80)).contains(":help"));
        assert!(joined(&empty_state_lines(false, false, 80)).contains(":help"));
    }
}
