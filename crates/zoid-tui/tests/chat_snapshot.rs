use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::render_chat;

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
            ts: 0,
        },
        ChatMsg::Assistant {
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
            ts: 0,
        },
        ChatMsg::Assistant {
            text: "the command failed.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}
