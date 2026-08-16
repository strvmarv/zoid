use ratatui::{backend::TestBackend, widgets::Paragraph, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::{conversation_view, render_chat, ChatView};
use zoid_tui::state::Zoom;

fn draw(msgs: &[ChatMsg], streaming: bool) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_chat(f, msgs, &input, streaming))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn empty_chat_frame() {
    insta::assert_snapshot!(draw(&[], false));
}

#[test]
fn seeded_transcript_frame() {
    let msgs = vec![
        ChatMsg::User {
            text: "what's causing the 500?".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "an unwrapped lookup in the handler.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}

#[test]
fn streaming_caret_frame() {
    let msgs = vec![
        ChatMsg::User {
            text: "hi".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "thinking".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ];
    insta::assert_snapshot!(draw(&msgs, true));
}

#[test]
fn tool_call_and_result_frame() {
    let msgs = vec![
        ChatMsg::User {
            text: "read a.txt".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef {
                id: "".into(),
                name: "read_file".into(),
                args: r#"{"path":"a.txt"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "".into(),
            name: "read_file".into(),
            output: "file body".into(),
            is_error: false,
                error_kind: None,
            compacted: false,
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "it contains the config.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}

#[test]
fn tool_error_result_frame() {
    let msgs = vec![
        ChatMsg::User {
            text: "run the build".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "".into(),
            tool_calls: vec![ToolCallRef {
                id: "".into(),
                name: "shell".into(),
                args: r#"{"command":"false"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "".into(),
            name: "shell".into(),
            output: "boom\n[exit 1]".into(),
            is_error: true,
                error_kind: None,
            compacted: false,
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "the command failed.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}

// ACM-1 Task 8: a compacted tool-result reads like a tool-call chip at Normal
// (⊟ compacted marker on the row) and labels its header at Detail (⊟ suffix),
// where the body is already the summary text substituted by `conversation()`.
fn compacted_msgs() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "grep the repo for TODOs".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "searching".into(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "search".into(),
                args: r#"{"query":"TODO"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "search".into(),
            output: "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".into(),
            is_error: false,
                error_kind: None,
            compacted: true,
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "found a handful of TODOs.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ]
}

/// Render `conversation_view` at the given zoom/width as a bare `Paragraph`
/// (mirrors `syntax_snapshot.rs`'s pattern for width-controlled frames).
fn draw_zoom(msgs: &[ChatMsg], zoom: Zoom, width: u16) -> String {
    let view = ChatView {
        zoom,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    };
    let backend = TestBackend::new(width, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    let lines = conversation_view(msgs, &view, false, width as usize, None, &[], 0);
    terminal
        .draw(|f| f.render_widget(Paragraph::new(lines), f.area()))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn compacted_tool_result_normal_100() {
    insta::assert_snapshot!(draw_zoom(&compacted_msgs(), Zoom::Normal, 100));
}

#[test]
fn compacted_tool_result_normal_140() {
    insta::assert_snapshot!(draw_zoom(&compacted_msgs(), Zoom::Normal, 140));
}

#[test]
fn compacted_tool_result_detail_100() {
    insta::assert_snapshot!(draw_zoom(&compacted_msgs(), Zoom::Detail, 100));
}

#[test]
fn compacted_tool_result_detail_140() {
    insta::assert_snapshot!(draw_zoom(&compacted_msgs(), Zoom::Detail, 140));
}
