use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;

/// Build the conversation lines (user/assistant turns + inline tool cards).
/// Shared by `render_chat` and the modal `render_shell`.
pub fn conversation_lines<'a>(msgs: &'a [ChatMsg], streaming: bool, caret_on: bool) -> Vec<Line<'a>> {
    let last = msgs.len().saturating_sub(1);
    if msgs.is_empty() {
        return vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))];
    }
    let mut lines: Vec<Line> = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        match m {
            ChatMsg::User(text) => {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", glyph::USER_TURN), Style::new().fg(color::CHAT_ACCENT)),
                    Span::styled(text.clone(), Style::new().fg(color::TXT)),
                ]));
            }
            ChatMsg::Assistant { text, tool_calls } => {
                let mut shown = text.clone();
                if streaming && caret_on && i == last && tool_calls.is_empty() {
                    shown.push(glyph::CARET);
                }
                if !shown.is_empty() || tool_calls.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("zoid ".to_string(), Style::new().fg(color::DIM)),
                        Span::styled(shown, Style::new().fg(color::TXT)),
                    ]));
                }
                for tc in tool_calls {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {} ", glyph::EDIT), Style::new().fg(color::CHAT_ACCENT)),
                        Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
                        Span::styled(format!("({})", arg_summary(&tc.args)), Style::new().fg(color::DIM)),
                        Span::styled(format!(" {} peek", glyph::RETURN), Style::new().fg(color::DIM)),
                    ]));
                }
            }
            ChatMsg::ToolResult { name, output, is_error, .. } => {
                let (mark, mark_color) = if *is_error {
                    (glyph::WARNING, color::ERROR)
                } else {
                    (glyph::PASS, color::OK)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                    Span::styled(name.clone(), Style::new().fg(color::DIM)),
                    Span::styled(format!(" → {}", first_line(output)), Style::new().fg(color::DIM)),
                ]));
            }
        }
    }
    lines
}

/// Render the Chat surface: title bar, conversation column, input box, status bar.
/// When `streaming` is true, a caret `▌` trails the in-progress assistant text
/// (only when the last message is an Assistant turn with no tool calls).
pub fn render_chat(frame: &mut Frame, msgs: &[ChatMsg], input: &TextArea<'_>, streaming: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(3), // input box (bordered)
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    // Title bar (unchanged).
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Conversation: user/assistant text turns + inline tool cards.
    let body = conversation_lines(msgs, streaming, true);
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

/// A compact one-line summary of a tool call's JSON args for the inline card.
fn arg_summary(args_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    }
}

fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 30),
        other => truncate(&other.to_string(), 30),
    }
}

fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 40)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_shows_only_when_streaming_and_caret_on() {
        use crate::tokens::glyph;
        let msgs = vec![ChatMsg::Assistant { text: "hi".into(), tool_calls: vec![] }];
        let has_caret = |streaming, caret| {
            conversation_lines(&msgs, streaming, caret)
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains(glyph::CARET)))
        };
        assert!(has_caret(true, true), "streaming + caret_on → caret shown");
        assert!(!has_caret(true, false), "caret_on=false suppresses caret while streaming");
        assert!(!has_caret(false, true), "not streaming → no caret");
    }
}
