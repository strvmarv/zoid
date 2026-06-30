use crate::state::Zoom;
use crate::syntax_view::highlight_lines;
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
use zoid_core::zoom::{digests, TurnDigest};
use zoid_syntax::{fold_regions, FoldRegion, Language};

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

/// Per-frame conversation view-model the bin assembles: altitude + caret blink
/// + an optional reveal cap (for the zoom transition animation, P4c Task 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatView {
    pub zoom: Zoom,
    pub caret_on: bool,
    pub reveal: Option<usize>,
}

/// Build the conversation lines at the requested altitude, capped to
/// `view.reveal` lines when set.
pub fn conversation_view(msgs: &[ChatMsg], view: &ChatView, streaming: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = match view.zoom {
        Zoom::Summary => digest_lines(&digests(msgs)),
        Zoom::Normal => conversation_lines(msgs, streaming, view.caret_on)
            .into_iter()
            .map(own_line)
            .collect(),
        Zoom::Detail => detail_lines(msgs),
    };
    if let Some(n) = view.reveal {
        lines.truncate(n);
    }
    lines
}

/// One digest line per turn: `› {headline}   ~ {tools}t · {files}f [⚠]`.
fn digest_lines(ds: &[TurnDigest]) -> Vec<Line<'static>> {
    ds.iter()
        .map(|d| {
            let mut spans = vec![
                Span::styled(format!("{} ", glyph::USER_TURN), Style::new().fg(color::CHAT_ACCENT)),
                // `.40` precision truncates to 40 chars; width 40 pads short ones —
                // a HEADLINE_MAX(60) headline can't blow past the column and misalign
                // the `~ Nt · Nf` field in the 140-col snapshot.
                Span::styled(format!("{:<40.40} ", d.headline), Style::new().fg(color::TXT)),
                Span::styled(format!("~ {}t · {}f", d.tools, d.files), Style::new().fg(color::DIM)),
            ];
            if d.has_error {
                spans.push(Span::styled(format!(" {}", glyph::WARNING), Style::new().fg(color::ERROR)));
            }
            Line::from(spans)
        })
        .collect()
}

/// Detail altitude: the normal view, but file tool-results are rendered with
/// syntax highlighting (Ⓡ3, P4a). The file's language is inferred from the
/// originating tool call's path (correlated by id).
fn detail_lines(msgs: &[ChatMsg]) -> Vec<Line<'static>> {
    use std::collections::HashMap;
    // id → file path, from assistant tool calls.
    let mut id_path: HashMap<&str, String> = HashMap::new();
    for m in msgs {
        if let ChatMsg::Assistant { tool_calls, .. } = m {
            for c in tool_calls {
                if let Some(p) = path_arg(&c.args) {
                    id_path.insert(c.id.as_str(), p);
                }
            }
        }
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for m in msgs {
        match m {
            ChatMsg::ToolResult { id, name, output, is_error } if !*is_error => {
                let header = Span::styled(
                    format!("  {} {}", glyph::PASS, name),
                    Style::new().fg(color::DIM),
                );
                out.push(Line::from(vec![header]));
                let lang = id_path.get(id.as_str()).map(|p| Language::from_path(p)).unwrap_or(Language::PlainText);
                out.extend(collapse_to_signatures(output, lang));
            }
            other => out.extend(conversation_lines(std::slice::from_ref(other), false, true).into_iter().map(own_line)),
        }
    }
    out
}

/// Collapse a code file to signatures: highlight every line, but replace each
/// **leaf** fold body's interior lines with a single `…` marker. "Leaf" = a fold
/// containing no other fold, so a container (`impl`/`mod`) keeps its method
/// signatures while each method/struct/enum leaf body folds. Realizes spec Ⓡ3↔①
/// "collapse to signatures"; uses P4a's `fold_regions` (function + type/impl bodies).
pub(crate) fn collapse_to_signatures(source: &str, lang: Language) -> Vec<Line<'static>> {
    let all = highlight_lines(source, lang); // one Line per source line
    let folds = fold_regions(source, lang);
    if folds.is_empty() {
        return all;
    }
    // 0-based line index of a byte offset = count of '\n' before it.
    let line_of = |byte: usize| {
        source[..byte.min(source.len())].bytes().filter(|&b| b == b'\n').count()
    };
    let is_leaf = |f: &FoldRegion, i: usize| {
        !folds
            .iter()
            .enumerate()
            .any(|(j, g)| j != i && g.start >= f.start && g.end <= f.end)
    };
    let mut elided = vec![false; all.len()];
    for (i, f) in folds.iter().enumerate() {
        if is_leaf(f, i) {
            // Keep the opening (signature) line and the closing line; elide between.
            for ln in (line_of(f.start) + 1)..line_of(f.end) {
                if ln < elided.len() {
                    elided[ln] = true;
                }
            }
        }
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < all.len() {
        if elided[i] {
            out.push(Line::from(Span::styled(
                format!("    {}", glyph::ELLIPSIS),
                Style::new().fg(color::DIM),
            )));
            while i < all.len() && elided[i] {
                i += 1;
            }
        } else {
            out.push(all[i].clone());
            i += 1;
        }
    }
    out
}

/// Extract a file path from a tool call's JSON args (mirrors core's tool_path,
/// kept local so chat.rs stays render-side).
fn path_arg(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    for key in ["path", "file_path", "file"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Convert a borrowed Line into an owned ('static) one by cloning span content.
fn own_line(l: Line) -> Line<'static> {
    Line::from(
        l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect::<Vec<_>>(),
    )
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

    use crate::state::Zoom;
    use zoid_core::projection::ToolCallRef;

    // The Assistant carries a tool_call whose id matches the ToolResult and whose
    // args name a `.rs` path — so `detail_lines` resolves id→path→Language::Rust
    // and actually highlights (without this, id_path is empty and Detail silently
    // falls back to PlainText, the exact gap that made the old fixture useless).
    // The body is multi-line so collapse-to-signatures (Task 3b) has something to fold.
    fn seeded() -> Vec<ChatMsg> {
        vec![
            ChatMsg::User("fix the parser bug".into()),
            ChatMsg::Assistant {
                text: "on it".into(),
                tool_calls: vec![ToolCallRef {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: r#"{"path":"src/parser.rs"}"#.into(),
                }],
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
                is_error: false,
            },
            ChatMsg::User("thanks".into()),
        ]
    }

    fn view(zoom: Zoom) -> ChatView {
        ChatView { zoom, caret_on: true, reveal: None }
    }

    #[test]
    fn summary_collapses_to_one_line_per_turn() {
        let lines = conversation_view(&seeded(), &view(Zoom::Summary), false);
        // two turns → two digest lines
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn detail_highlights_file_tool_results() {
        use crate::tokens::color;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false);
        // A keyword (`fn`/`let`) must carry the syntax keyword color — proves the
        // id→path→Rust resolution fired and highlighting actually ran, rather than
        // silently falling back to PlainText (which colors everything TXT).
        let has_keyword_color = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::SYN_KEYWORD)));
        assert!(has_keyword_color, "Detail must highlight the Rust tool-result body");
    }

    #[test]
    fn detail_collapses_function_bodies_to_signatures() {
        use crate::tokens::glyph;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        assert!(text.iter().any(|t| t.contains("fn parse")), "signature line is kept");
        assert!(text.iter().any(|t| t.contains(glyph::ELLIPSIS)), "body collapses to …");
        assert!(!text.iter().any(|t| t.contains("let n = 42")), "body interior is elided");
    }

    #[test]
    fn normal_matches_conversation_lines() {
        let msgs = seeded();
        let normal = conversation_view(&msgs, &view(Zoom::Normal), false);
        let baseline = conversation_lines(&msgs, false, true);
        assert_eq!(normal.len(), baseline.len());
    }

    #[test]
    fn reveal_caps_line_count() {
        let mut v = view(Zoom::Normal);
        v.reveal = Some(1);
        let lines = conversation_view(&seeded(), &v, false);
        assert_eq!(lines.len(), 1);
    }
}
