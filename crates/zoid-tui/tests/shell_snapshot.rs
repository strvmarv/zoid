use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Focus, Mode, Overlay, ShellState};
use zoid_tui::EconomyView;
use zoid_core::context::ContextWindow;
use zoid_core::economy::{ChurnTimeline, TokenLedger};
use zoid_core::assembler::ContextPolicy;

fn empty_economy() -> EconomyView {
    EconomyView::build(&ContextWindow::default(), &ChurnTimeline::default(), &TokenLedger::default(), &ContextPolicy::default(), 0)
}

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    draw_econ(state, &empty_economy(), msgs, w, h)
}

fn draw_econ(state: &ShellState, econ: &EconomyView, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_shell(f, state, econ, msgs, &input, false)).unwrap();
    terminal.backend().to_string()
}

fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant { text: "an unwrapped lookup in the handler.".into(), tool_calls: vec![] },
    ]
}

fn seeded_economy() -> EconomyView {
    use zoid_core::context::{ContextItem, Heat, ItemKind};
    let it = |key: &str, label: &str, kind, tokens, heat, pinned| ContextItem {
        key: key.into(), label: label.into(), kind, tokens, heat, pinned, evicted: false,
    };
    // Items span kinds (ToolResult / File / Message), not files-only — mirrors
    // docs/ux/chat-mode.html and what context_window() actually produces in P3.
    let w = ContextWindow {
        items: vec![
            it("tool:grep:c9", "grep", ItemKind::ToolResult, 6000, Heat::Hot, false),
            it("file:schema.sql", "schema.sql", ItemKind::File, 5000, Heat::Cold, false),
            it("file:users.rs", "users.rs", ItemKind::File, 4000, Heat::Hot, true),
            it("msg:2", "ship it?", ItemKind::Message, 3000, Heat::Warm, false),
        ],
        total_tokens: 18000,
    };
    let churn = ChurnTimeline { points: vec![
        zoid_core::economy::ChurnPoint { turn: 0, tokens: 10, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 1, tokens: 30, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 2, tokens: 12, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 3, tokens: 48, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 4, tokens: 12, resent_tokens: 0 },
    ] };
    let ledger = TokenLedger { input: 142_000, output: 0, cached: 0, total: 142_000 };
    let policy = ContextPolicy { token_ceiling: Some(200_000), ..Default::default() };
    EconomyView::build(&w, &churn, &ledger, &policy, 0)
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

#[test]
fn economy_drawer_frame() {
    let s = ShellState::new(); // economy open by default
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 100, 24));
}

#[test]
fn economy_drawer_wide_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 140, 24));
}

#[test]
fn economy_drawer_selected_frame() {
    let mut s = ShellState::new();
    s.focus = Focus::Rail;
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 100, 24));
}
