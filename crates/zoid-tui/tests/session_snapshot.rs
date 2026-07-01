use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{Overlay, ShellState, Zoom};
use zoid_tui::EconomyView;
use zoid_core::context::ContextWindow;
use zoid_core::economy::{ChurnTimeline, TokenLedger};
use zoid_core::assembler::ContextPolicy;

fn normal_view() -> ChatView {
    ChatView { zoom: Zoom::Normal, caret_on: true, reveal: None, tz_offset_secs: 0 }
}

fn empty_economy() -> EconomyView {
    EconomyView::build(&ContextWindow::default(), &ChurnTimeline::default(), &TokenLedger::default(), &ContextPolicy::default(), 0)
}

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_shell(f, state, &empty_economy(), msgs, &input, false, &normal_view())).unwrap();
    terminal.backend().to_string()
}

#[test]
fn resume_session_overlay_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Sessions;
    s.sessions = vec![
        "fix 500 on GET /users  ·  12m ago  ·  58k".into(),
        "rail restructure       ·  3h ago   ·  120k".into(),
    ];
    insta::assert_snapshot!(draw(&s, &[], 100, 24));
}
