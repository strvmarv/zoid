use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Focus, Mode, Overlay, ShellState, Zoom};
use zoid_tui::EconomyView;
use zoid_core::context::ContextWindow;
use zoid_core::economy::{ChurnTimeline, TokenLedger};
use zoid_core::assembler::ContextPolicy;

fn normal_view() -> ChatView {
    ChatView { zoom: Zoom::Normal, caret_on: true, reveal: None }
}

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
    terminal.draw(|f| render_shell(f, state, econ, msgs, &input, false, &normal_view())).unwrap();
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

/// The selected row gets a `SEL_BG` background — invisible to text snapshots
/// (`TestBackend::to_string()` captures glyphs, not styles), so assert on the
/// buffer's cell styles directly: the highlight must appear when the rail is
/// focused and be absent otherwise.
#[test]
fn economy_drawer_selection_highlights_only_when_rail_focused() {
    use ratatui::{backend::TestBackend, Terminal};
    use zoid_tui::tokens::color;

    let count_sel_bg = |focus: Focus| -> usize {
        let mut s = ShellState::new(); // economy open by default
        s.focus = focus;
        let input = TextArea::default();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_shell(f, &s, &seeded_economy(), &seeded(), &input, false, &normal_view()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|c| c.bg == color::SEL_BG)
            .count()
    };

    assert_eq!(count_sel_bg(Focus::Input), 0, "no highlight when rail unfocused");
    assert!(count_sel_bg(Focus::Rail) > 0, "selected row highlighted when rail focused");
}

// Zoom altitude render (P4c Task 3). The P3 `seeded()` above has no
// `ToolResult`, so Detail rendered against it would be a plain conversation —
// the snapshot would silently bake a frame that proves nothing. `seeded_detail()`
// pairs a matched tool-call/result (id + `.rs` path) so the Detail snapshots
// actually show highlighted code.
use zoid_core::projection::ToolCallRef;

fn seeded_detail() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("show me parse".into()),
        ChatMsg::Assistant {
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "read_file".into(),
                args: r#"{"path":"src/parser.rs"}"#.into(),
            }],
        },
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "read_file".into(),
            output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
            is_error: false,
        },
    ]
}

/// Snapshot the buffer's `Debug` form (not `to_string()`): it emits the
/// `styles:` block with `fg: Rgb(...)` entries, so Detail's syntax-color
/// payoff is actually captured by the snapshot, not just the glyphs.
fn draw_zoom(zoom: Zoom, w: u16, h: u16) -> String {
    let s = ShellState::new();
    let view = ChatView { zoom, caret_on: true, reveal: None };
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_detail(), &input, false, &view))
        .unwrap();
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn zoom_summary_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Summary, 100, 24));
}

#[test]
fn zoom_summary_wide_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Summary, 140, 24));
}

#[test]
fn zoom_detail_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Detail, 100, 24));
}

#[test]
fn zoom_detail_wide_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Detail, 140, 24));
}

// Object/verb picker overlays (P4d Task 3). Seed a file read + an error so the
// object picker has all three kinds (File/Symbol/Error) to list, and the verb
// picker has a real scoped verb set for the first (selected) object.
fn seeded_objects() -> Vec<ChatMsg> {
    vec![
        ChatMsg::Assistant {
            text: String::new(),
            tool_calls: vec![ToolCallRef { id: "c1".into(), name: "read_file".into(), args: r#"{"path":"src/ast.rs"}"#.into() }],
        },
        ChatMsg::ToolResult { id: "c1".into(), name: "read_file".into(), output: "fn parse() {}\nstruct Ast {}\n".into(), is_error: false },
        ChatMsg::ToolResult { id: "c2".into(), name: "shell".into(), output: "FAILED\n".into(), is_error: true },
    ]
}

/// Buffer-Debug snapshot (style + glyphs), per the snapshot standard used by
/// `draw_zoom` above — `to_string()` alone would hide the `SEL_BG` selection
/// highlight on the picker's selected row.
fn draw_overlay(overlay: Overlay, w: u16, h: u16) -> String {
    let mut s = ShellState::new();
    s.overlay = overlay;
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_objects(), &input, false, &normal_view()))
        .unwrap();
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn object_overlay_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Objects, 100, 24));
}

#[test]
fn object_overlay_wide_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Objects, 140, 24));
}

#[test]
fn verb_overlay_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Verbs, 100, 24));
}

#[test]
fn verb_overlay_wide_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Verbs, 140, 24));
}
