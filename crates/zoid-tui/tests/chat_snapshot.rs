use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::{Role, Turn};
use zoid_tui::chat::render_chat;

#[test]
fn empty_chat_frame() {
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &[], &input, false)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn seeded_transcript_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "what's causing the 500?".into() },
        Turn { role: Role::Assistant, text: "an unwrapped lookup in the handler.".into() },
    ];
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns, &input, false)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn streaming_caret_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "hi".into() },
        Turn { role: Role::Assistant, text: "thinking".into() },
    ];
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns, &input, true)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
