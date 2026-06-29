use ratatui::{backend::TestBackend, Terminal};
use zoid_core::projection::{Role, Turn};
use zoid_tui::chat::render_chat;

#[test]
fn empty_chat_frame() {
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &[])).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn seeded_transcript_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "what's causing the 500?".into() },
        Turn { role: Role::Assistant, text: "an unwrapped lookup in the handler.".into() },
    ];
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
