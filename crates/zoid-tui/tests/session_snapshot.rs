use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::context::ContextWindow;
use zoid_core::economy::ChurnTimeline;
use zoid_core::projection::ChatMsg;
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{Overlay, ShellState, Zoom};
use zoid_tui::EconomyView;

fn normal_view() -> ChatView {
    ChatView {
        zoom: Zoom::Normal,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    }
}

fn empty_economy() -> EconomyView {
    EconomyView::build(&ContextWindow::default(), &ChurnTimeline::default(), 0)
}

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                state,
                &empty_economy(),
                msgs,
                &input,
                false,
                &normal_view(),
            )
        })
        .unwrap();
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

#[test]
fn repo_drawer_frame() {
    let mut s = ShellState::new();
    s.repo_name = "zoid".into();
    s.branch = "main".into();
    s.changes_added = 128;
    s.changes_removed = 34;
    s.changes_files = 7;
    insta::assert_snapshot!(draw(&s, &[], 100, 24));
}

#[test]
fn session_drawer_truncates_long_cwd() {
    let mut s = ShellState::new();
    s.session_name = "fix 500 on GET /users".into();
    s.model = "glm-5.2".into();
    s.provider = "ollama".into();
    s.duration = "12m".into();
    s.session_tokens = 58_000;
    s.ctx_used = 58_000;
    s.ctx_ceiling = 200_000;
    s.cwd = "~/develop/projects/zoid/crates/zoid-tui/src/very/deep/nested/path".into();
    let out = draw(&s, &[], 100, 24);
    // The cwd never wraps — it is truncated with the §16 ellipsis.
    assert!(
        out.contains('\u{2026}'),
        "long cwd should be truncated with an ellipsis"
    );
    insta::assert_snapshot!(out);
}
