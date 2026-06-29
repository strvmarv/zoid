use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use zoid_core::projection::{Role, Turn};

/// Render the Chat surface: title bar, conversation column, status bar.
pub fn render_chat(frame: &mut Frame, turns: &[Turn]) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    let body: Vec<Line> = if turns.is_empty() {
        vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))]
    } else {
        turns
            .iter()
            .map(|t| {
                let (prefix, accent) = match t.role {
                    Role::User => (format!("{} ", glyph::USER_TURN), color::CHAT_ACCENT),
                    Role::Assistant => ("zoid ".to_string(), color::DIM),
                };
                Line::from(vec![
                    Span::styled(prefix, Style::new().fg(accent)),
                    Span::styled(t.text.clone(), Style::new().fg(color::TXT)),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(body), chunks[1]);

    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!("· {}Tab Build · q quit", "\u{21e7}"), // ⇧Tab
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[2]);
}
