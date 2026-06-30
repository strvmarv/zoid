use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Mode, Overlay, ShellState};

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_shell(f, state, msgs, &input, false)).unwrap();
    terminal.backend().to_string()
}

fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant { text: "an unwrapped lookup in the handler.".into(), tool_calls: vec![] },
    ]
}

#[test]
fn chat_with_rail_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

/// Wide frame: at >130 cols the measure-cap slack appears as a gutter. It must
/// sit *between* the stream and the rail (stream flush-left), never to the left
/// of the stream. The 100-col frames collapse this gutter to zero, so only a
/// wide snapshot guards the ordering.
#[test]
fn chat_wide_gutter_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw(&s, &seeded(), 140, 24));
}

#[test]
fn files_drawer_open_frame() {
    let mut s = ShellState::new();
    s.files = vec!["Cargo.toml".into(), "src".into(), "README.md".into()];
    s.toggle_drawer(DrawerId::Files);
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn palette_overlay_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Palette;
    s.palette.query = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn command_line_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::CommandLine;
    s.cmdline.buffer = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn build_placeholder_frame() {
    let mut s = ShellState::new();
    s.set_mode(Mode::Build);
    insta::assert_snapshot!(draw(&s, &[], 100, 24));
}
