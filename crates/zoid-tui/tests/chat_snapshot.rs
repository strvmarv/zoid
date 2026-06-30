use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::render_chat;

fn draw(msgs: &[ChatMsg], streaming: bool) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, msgs, &input, streaming)).unwrap();
    terminal.backend().to_string()
}

#[test]
fn empty_chat_frame() {
    insta::assert_snapshot!(draw(&[], false));
}

#[test]
fn seeded_transcript_frame() {
    let msgs = vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant { text: "an unwrapped lookup in the handler.".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}

#[test]
fn streaming_caret_frame() {
    let msgs = vec![
        ChatMsg::User("hi".into()),
        ChatMsg::Assistant { text: "thinking".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, true));
}

#[test]
fn tool_call_and_result_frame() {
    let msgs = vec![
        ChatMsg::User("read a.txt".into()),
        ChatMsg::Assistant {
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef { id: "".into(), name: "read_file".into(), args: r#"{"path":"a.txt"}"#.into() }],
        },
        ChatMsg::ToolResult { id: "".into(), name: "read_file".into(), output: "file body".into(), is_error: false },
        ChatMsg::Assistant { text: "it contains the config.".into(), tool_calls: vec![] },
    ];
    insta::assert_snapshot!(draw(&msgs, false));
}
