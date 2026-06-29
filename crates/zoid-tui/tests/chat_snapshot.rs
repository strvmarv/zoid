use ratatui::{backend::TestBackend, Terminal};
use zoid_tui::chat::render_chat;

#[test]
fn empty_chat_frame() {
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &[])).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
