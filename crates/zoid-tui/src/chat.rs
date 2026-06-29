use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use zoid_core::projection::{Role, Turn};

/// Render the Chat surface: title bar, conversation column, input box, status bar.
/// When `streaming` is true, a caret `▌` trails the in-progress assistant text.
pub fn render_chat(frame: &mut Frame, turns: &[Turn], input: &TextArea<'_>, streaming: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(3), // input box (bordered)
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    // Title bar.
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Conversation.
    let last = turns.len().saturating_sub(1);
    let body: Vec<Line> = if turns.is_empty() {
        vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))]
    } else {
        turns
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (prefix, accent) = match t.role {
                    Role::User => (format!("{} ", glyph::USER_TURN), color::CHAT_ACCENT),
                    Role::Assistant => ("zoid ".to_string(), color::DIM),
                };
                let mut text = t.text.clone();
                if streaming && i == last && t.role == Role::Assistant {
                    text.push(glyph::CARET);
                }
                Line::from(vec![
                    Span::styled(prefix, Style::new().fg(accent)),
                    Span::styled(text, Style::new().fg(color::TXT)),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(body), chunks[1]);

    // Input box (bordered text area).
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(input_block, chunks[2]);
    let inner = chunks[2].inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(input, inner);

    // Status bar.
    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!("· {}Tab Build · {} send · ^C quit", glyph::SHIFT, glyph::RETURN),
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[3]);
}
