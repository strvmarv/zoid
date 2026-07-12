use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::context::ContextWindow;
use zoid_core::economy::ChurnTimeline;
use zoid_core::projection::ChatMsg;
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Focus, Overlay, ShellState, Zoom};
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
    draw_econ(state, &empty_economy(), msgs, w, h)
}

fn draw_econ(state: &ShellState, econ: &EconomyView, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                state,
                econ,
                msgs,
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    terminal.backend().to_string()
}

fn draw_config(
    state: &ShellState,
    sections: &[zoid_tui::config_view::Section],
    w: u16,
    h: u16,
) -> String {
    use zoid_tui::render::render_config;
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            render_config(f, state, sections, area)
        })
        .unwrap();
    terminal.backend().to_string()
}

/// Draw a frame allowing the pre-rendered (cached) body to be supplied — the
/// hot path the bin uses. `None` exercises the uncached fallback.
fn draw_body(
    state: &ShellState,
    msgs: &[ChatMsg],
    w: u16,
    h: u16,
    body: Option<&[ratatui::text::Line<'static>]>,
    view: &ChatView,
) -> String {
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
                body,
                &[],
                &input,
                false,
                view,
            );
        })
        .unwrap();
    terminal.backend().to_string()
}

/// The cached-body window render (borrowing lines into the buffer) must produce
/// byte-identical frames to the uncached Paragraph render. This is the safety
/// net for the #7 per-frame-clone optimization, since every other snapshot test
/// uses the `None` (uncached) path.
///
/// Scope of the invariant: cached==uncached holds for `reveal == None` at any
/// scroll/tool state, and for `reveal == Some(k)` when `k <= raw_line_count`
/// (the reveal cap sits at or below the transcript body). It does NOT hold for
/// `k > raw_line_count`, because `conversation_view` appends a trailing blank
/// line only when `reveal == None`: the cached body (built reveal-None) carries
/// that blank, while the uncached reveal render drops it — a pre-existing
/// cached/uncached asymmetry that is unreachable at runtime (production always
/// passes the cached body). The reveal window logic is otherwise faithful to the
/// pre-optimization cached path by construction (`full[..min(k,len)]`).
#[test]
fn cached_body_window_matches_uncached_paragraph() {
    use ratatui::layout::{Margin, Rect};
    use zoid_tui::chat::{conversation_view, ChatView};
    use zoid_tui::layout::{compute, CONV_PAD};

    // A transcript tall enough to scroll at 160x40, plus a short one (fewer lines
    // than the viewport) to cover the base_len < height window.
    let tall: Vec<ChatMsg> = (0..30)
        .flat_map(|i| {
            [
                ChatMsg::User {
                    text: format!("question number {i} about the codebase"),
                    ts: 0,
                },
                ChatMsg::Assistant {
                    thinking: None,
                    text: format!("answer {i}: it is the unwrapped lookup in the handler again"),
                    tool_calls: vec![],
                    ts: 0,
                },
            ]
        })
        .collect();
    let short = seeded(); // two messages, well under a 40-row viewport

    let (w, h) = (160u16, 40u16);
    // Layout width is independent of conversation_scroll / active_tool (the only
    // fields the draws below vary), so building `full` once at the base width is
    // faithful to what every draw computes.
    let conv = compute(Rect::new(0, 0, w, h), &ShellState::new())
        .conversation
        .inner(Margin {
            horizontal: CONV_PAD,
            vertical: 0,
        });

    let reveal_view = |k: usize| ChatView {
        reveal: Some(k),
        ..normal_view()
    };

    for msgs in [&tall, &short] {
        // `full` is what the bin caches: built reveal-None.
        let full = conversation_view(msgs, &normal_view(), false, conv.width as usize, None, &[], 0);
        let raw_lines = full.len().saturating_sub(1); // reveal-None appends one blank

        // reveal == None at several scrolls × spinner off/on, plus a reveal case
        // capped at/below the raw body (where cached == uncached holds).
        let views = [normal_view(), reveal_view(raw_lines.min(3))];
        for view in &views {
            for &scroll in &[0u16, 5, full.len() as u16 /* clamps to bottom */] {
                for tool in [false, true] {
                    let mut state = ShellState::new();
                    state.conversation_scroll = scroll;
                    if tool {
                        state.set_active_tool("shell");
                    }
                    let uncached = draw_body(&state, msgs, w, h, None, view);
                    let cached = draw_body(&state, msgs, w, h, Some(&full), view);
                    assert_eq!(
                        uncached, cached,
                        "cached window must match uncached (scroll={scroll}, tool={tool}, reveal={:?})",
                        view.reveal
                    );
                }
            }
        }
    }
}

fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "what's causing the 500?".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "an unwrapped lookup in the handler.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ]
}

fn seeded_economy() -> EconomyView {
    use zoid_core::context::{ContextItem, Heat, ItemKind};
    let it = |key: &str, label: &str, kind, tokens, heat, pinned| ContextItem {
        key: key.into(),
        label: label.into(),
        kind,
        tokens,
        heat,
        pinned,
        evicted: false,
        compacted: false,
    };
    // Items span kinds (ToolResult / File / Message), not files-only — mirrors
    // docs/ux/chat-mode.html and what context_window() actually produces in P3.
    let w = ContextWindow {
        items: vec![
            it(
                "tool:grep:c9",
                "grep",
                ItemKind::ToolResult,
                6000,
                Heat::Hot,
                false,
            ),
            it(
                "file:schema.sql",
                "schema.sql",
                ItemKind::File,
                5000,
                Heat::Cold,
                false,
            ),
            it(
                "file:users.rs",
                "users.rs",
                ItemKind::File,
                4000,
                Heat::Hot,
                true,
            ),
            it(
                "msg:2",
                "ship it?",
                ItemKind::Message,
                3000,
                Heat::Warm,
                false,
            ),
        ],
        total_tokens: 18000,
    };
    let churn = ChurnTimeline {
        points: vec![
            zoid_core::economy::ChurnPoint {
                turn: 0,
                tokens: 10,
                cached: 0,
                resent_tokens: 0,
            },
            zoid_core::economy::ChurnPoint {
                turn: 1,
                tokens: 30,
                cached: 8,
                resent_tokens: 0,
            },
            zoid_core::economy::ChurnPoint {
                turn: 2,
                tokens: 12,
                cached: 20,
                resent_tokens: 0,
            },
            zoid_core::economy::ChurnPoint {
                turn: 3,
                tokens: 48,
                cached: 40,
                resent_tokens: 0,
            },
            zoid_core::economy::ChurnPoint {
                turn: 4,
                tokens: 12,
                cached: 12,
                resent_tokens: 0,
            },
        ],
    };
    EconomyView::build(&w, &churn, 0)
}

#[test]
fn chat_with_rail_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

/// In-flight tool indicator: a dim spinner line below the last message while a
/// Local tool call is running (P2 ①).
#[test]
fn active_tool_spinner_frame() {
    let mut s = ShellState::new();
    s.set_active_tool("shell");
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

/// Mode-chip fidelity (Task 7): the status bar's chip shows the active mode
/// name, uppercased, when the mode loaded cleanly.
#[test]
fn status_chip_shows_active_mode() {
    let mut s = ShellState::new();
    s.active_mode = "Superpowers".into();
    s.mode_names = vec!["Chat".into(), "Superpowers".into()];
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

/// Broken-mode fidelity (Task 7): when the active mode failed to load, the
/// chip shows a `⚠` warning glyph + the mode name, and the main surface is
/// replaced entirely by the mode-error card with the `:mode reload` hint
/// (spec §9).
#[test]
fn broken_mode_shows_warn_chip_and_error_card() {
    let mut s = ShellState::new();
    s.active_mode = "Superpowers".into();
    s.mode_names = vec!["Chat".into(), "Superpowers".into()];
    s.active_mode_broken = true;
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

/// Rail drawer headers show title + chevron only — no keybind labels (spec §2.1).
#[test]
fn rail_headers_have_no_keybind_labels() {
    let s = ShellState::new();
    let out = draw(&s, &seeded(), 160, 40);
    assert!(!out.contains("^5"), "economy keybind label must be gone");
    assert!(!out.contains("^F"), "files keybind label must be gone");
    assert!(!out.contains("^B"), "branch keybind label must be gone");
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

/// A turn wider than the conversation column must WRAP, not clip. Without
/// `Paragraph::wrap`, ratatui truncates the over-wide line mid-word at the right
/// edge and the tail is lost. Assert the tail sentinel is present AND lands on a
/// row below the head sentinel — proving it wrapped rather than merely fitting.
#[test]
fn long_turn_wraps_instead_of_clipping() {
    use ratatui::{backend::TestBackend, Terminal};

    let long = format!("HEADSENTINEL {} TAILSENTINEL", "wrap ".repeat(40));
    let msgs = vec![ChatMsg::Assistant {
        thinking: None,
        text: long,
        tool_calls: vec![],
        ts: 0,
    }];

    let s = ShellState::new();
    let input = TextArea::default();
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &msgs,
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();

    let row_of = |needle: &str| -> Option<u16> {
        (0..buf.area.height).find(|&y| {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            row.contains(needle)
        })
    };

    let head = row_of("HEADSENTINEL").expect("head sentinel must render");
    let tail =
        row_of("TAILSENTINEL").expect("tail sentinel must render (would be clipped without wrap)");
    assert!(
        tail > head,
        "tail (row {tail}) must wrap below head (row {head})"
    );
}

#[test]
fn session_drawer_open_frame() {
    let mut s = ShellState::new();
    s.toggle_drawer(DrawerId::Session);
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_overlay_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_overlay_empty_query_frame() {
    // Empty query renders the full curated list (14 rows) — the only snapshot
    // that exercises the full-list render path. `palette_overlay_frame` uses
    // query="build" which filters to one row, so it can't catch a render-layer
    // regression in the empty-query path. The 14-row curated order itself is
    // pinned by `palette::tests::all_items_is_flat_curated`.
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = String::new();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_direct_phase_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":mode Build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_direct_stage1_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.sessions = vec!["fix 500".into(), "add auth".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_direct_stage2_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":session ".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_direct_stage3_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.sessions = vec!["fix 500".into(), "add auth".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":session rename ".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn palette_arg_stage_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Palette;
    s.palette.stage = zoid_tui::state::PaletteStage::Arg {
        kind: zoid_tui::palette::ArgKind::Rename,
        input: "my-feature".into(),
    };
    insta::assert_snapshot!(draw(&s, &seeded(), 160, 40));
}

#[test]
fn economy_drawer_frame() {
    let s = ShellState::new(); // economy open by default
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 160, 40));
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
        // Tall enough that the Context drawer opens fully (it yields to Session
        // on a short rail); this test is about the selection highlight, not fit.
        let backend = TestBackend::new(160, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_shell(
                    f,
                    &s,
                    &seeded_economy(),
                    &seeded(),
                    None,
                    &[],
                    &input,
                    false,
                    &normal_view(),
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|c| c.bg == color::SEL_BG)
            .count()
    };

    assert_eq!(
        count_sel_bg(Focus::Input),
        0,
        "no highlight when rail unfocused"
    );
    assert!(
        count_sel_bg(Focus::Rail) > 0,
        "selected row highlighted when rail focused"
    );
}

/// A rail context label longer than its column must be truncated with the §16
/// ellipsis, not left to overflow and shove the tokens/heat-bar off the rail.
/// Assert the ellipsis is present, the label's tail is gone, and the token count
/// still renders (it wasn't pushed off-screen).
#[test]
fn long_economy_label_truncates_with_ellipsis() {
    use zoid_core::context::{ContextItem, Heat, ItemKind};
    use zoid_tui::tokens::glyph;

    let w = ContextWindow {
        items: vec![ContextItem {
            key: "msg:1".into(),
            label: "AVeryLongContextLabelThatWouldOverflowTheRail".into(),
            kind: ItemKind::Message,
            tokens: 9000,
            heat: Heat::Hot,
            pinned: false,
            evicted: false,
            compacted: false,
        }],
        total_tokens: 9000,
    };
    let econ = EconomyView::build(&w, &ChurnTimeline::default(), 0);
    let s = ShellState::new(); // economy drawer open by default
                               // Tall enough that the Context drawer opens fully (it yields to Session on a
                               // short rail); this test is about horizontal label truncation, not fit.
    let out = draw_econ(&s, &econ, &seeded(), 160, 40);

    assert!(
        out.contains(glyph::ELLIPSIS),
        "long label must be truncated with the ellipsis glyph"
    );
    assert!(
        !out.contains("AVeryLongContextLabel"),
        "the truncated head must not render"
    );
    assert!(
        out.contains("OverflowTheRail"),
        "the tail (most pertinent info) must be preserved"
    );
    assert!(
        out.contains("9k"),
        "the token count must still render (not pushed off the rail)"
    );
}

// Zoom altitude render (P4c Task 3). The P3 `seeded()` above has no
// `ToolResult`, so Detail rendered against it would be a plain conversation —
// the snapshot would silently bake a frame that proves nothing. `seeded_detail()`
// pairs a matched tool-call/result (id + `.rs` path) so the Detail snapshots
// actually show highlighted code.
use zoid_core::projection::ToolCallRef;

fn seeded_detail() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "show me parse".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
                thinking: None,
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "read_file".into(),
                args: r#"{"path":"src/parser.rs"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "read_file".into(),
            output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
            is_error: false,
            compacted: false,
            ts: 0,
        },
    ]
}

/// Snapshot the buffer's `Debug` form (not `to_string()`): it emits the
/// `styles:` block with `fg: Rgb(...)` entries, so Detail's syntax-color
/// payoff is actually captured by the snapshot, not just the glyphs.
fn draw_zoom(zoom: Zoom, w: u16, h: u16) -> String {
    let s = ShellState::new();
    let view = ChatView {
        zoom,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    };
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded_detail(),
                None,
                &[],
                &input,
                false,
                &view,
            );
        })
        .unwrap();
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn zoom_summary_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Summary, 160, 40));
}

#[test]
fn zoom_summary_wide_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Summary, 140, 24));
}

#[test]
fn zoom_detail_frame() {
    insta::assert_snapshot!(draw_zoom(Zoom::Detail, 160, 40));
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
                thinking: None,
            text: String::new(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "read_file".into(),
                args: r#"{"path":"src/ast.rs"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "read_file".into(),
            output: "fn parse() {}\nstruct Ast {}\n".into(),
            is_error: false,
            compacted: false,
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "c2".into(),
            name: "shell".into(),
            output: "FAILED\n".into(),
            is_error: true,
            compacted: false,
            ts: 0,
        },
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
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded_objects(),
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn object_overlay_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Objects, 160, 40));
}

#[test]
fn object_overlay_wide_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Objects, 140, 24));
}

#[test]
fn verb_overlay_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Verbs, 160, 40));
}

#[test]
fn verb_overlay_wide_frame() {
    insta::assert_snapshot!(draw_overlay(Overlay::Verbs, 140, 24));
}

/// Drives the real `render_shell` overlay dispatch (not `render_help_overlay`
/// directly) so a missing `Overlay::Help` if/else branch — which renders
/// invisibly rather than failing to compile — is caught.
#[test]
fn help_overlay_frame_dispatches_from_render_shell() {
    let frame = draw_overlay(Overlay::Help, 160, 40);
    assert!(
        frame.contains("keyboard shortcuts"),
        "help overlay must render via render_shell's overlay dispatch: {frame}"
    );
    assert!(frame.contains("Ctrl+P"), "help overlay must list shortcuts: {frame}");
}

/// The message box grows with its content (spec §2.2). A 3-line input yields a
/// 5-row box (3 content + 2 borders); Buffer-Debug captures the taller frame.
#[test]
fn growing_message_box_frame() {
    let mut s = ShellState::new();
    s.input_rows = 3;
    let input = TextArea::from(vec![
        "line one".to_string(),
        "line two".to_string(),
        "line three".to_string(),
    ]);
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded(),
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    insta::assert_snapshot!(format!("{:#?}", terminal.backend().buffer()));
}

/// Markdown message rendering (spec §3.5) — heading, bold, inline code, a list,
/// and a fenced rust block. Buffer-Debug captures the styled spans + syntax hues.
fn seeded_markdown() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User { text: "how do I read a file?".into(), ts: 0 },
        ChatMsg::Assistant {
            thinking: None,
            text: "Use **read_file**. Steps:\n\n- open the path\n- return `String`\n\n```rust\nfn read(p: &str) -> String { String::new() }\n```".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ]
}

#[test]
fn markdown_message_frame() {
    let s = ShellState::new();
    let input = TextArea::default();
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded_markdown(),
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    insta::assert_snapshot!(format!("{:#?}", terminal.backend().buffer()));
}

/// The busy activity arm (spinner + `working` in CHAT_ACCENT) has no coverage
/// elsewhere — every other shell snapshot renders idle. Buffer-Debug captures
/// the indicator's fg so the CHAT_ACCENT styling is asserted, not just the
/// glyph. `busy` drives the indicator now (not the streaming body flag); the
/// default spinner frame keeps the snapshot deterministic.
#[test]
fn running_title_frame() {
    let mut s = ShellState::new();
    s.busy = true;
    let input = TextArea::default();
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded(),
                None,
                &[],
                &input,
                true,
                &normal_view(),
            );
        })
        .unwrap();
    insta::assert_snapshot!(format!("{:#?}", terminal.backend().buffer()));
}

fn seeded_delegated() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "extract NotFound handling into a shared helper".into(),
            ts: 0,
        },
        ChatMsg::Delegated {
            summary: "Added shared NotFound helper; get_user reuses it.".into(),
            ok: true,
        },
    ]
}

fn draw_delegated(w: u16, h: u16) -> String {
    let s = ShellState::new();
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &s,
                &empty_economy(),
                &seeded_delegated(),
                None,
                &[],
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    // Buffer Debug per §8 snapshot standard — captures the DELEGATE_BG style.
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn delegated_card_frame() {
    insta::assert_snapshot!(draw_delegated(160, 40));
}
#[test]
fn delegated_card_wide_frame() {
    insta::assert_snapshot!(draw_delegated(140, 24));
}

/// Config overlay (Task 11): two-pane full-screen frame, left nav + right
/// detail, provenance tags, and a `⚠` marker on the env-shadowed `model` row.
#[test]
fn config_overlay_frame() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::build_sections;

    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    let cfg = Config::default();
    let prov = Provenance {
        // all Default except model, shadowed by env.
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Env,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);
    insta::assert_snapshot!(draw_config(&s, &sections, 160, 40));
}

/// API-key gate (Task 15): while `config_key_prompt` is set, the fields
/// column shows a dedicated masked key-entry view instead of the normal
/// per-row list — the literal secret text must never appear, only mask
/// glyphs + caret.
#[test]
fn config_key_prompt_masks_entry() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::build_sections;

    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    s.config_key_prompt = Some("ANTHROPIC_API_KEY");
    s.config_edit = Some("sk-secret".to_string());
    let cfg = Config::default();
    let prov = Provenance {
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);
    let snapshot = draw_config(&s, &sections, 160, 40);
    assert!(
        !snapshot.contains("sk-secret"),
        "masked key prompt must not leak the literal secret text"
    );
    insta::assert_snapshot!(snapshot);
}

/// Config overlay (Task 8): full-screen three-column layout — sections rail |
/// active section's fields | contextual picker — with the provider picker open
/// on the `provider` field (col 3 populated from `config_view::provider_options`).
#[test]
fn config_overlay_provider_picker() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::{build_sections, provider_options};
    use zoid_tui::state::ConfigCol;

    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    s.config_section = 0;
    s.config_field = 0; // "provider" row
    s.config_col = ConfigCol::Picker;
    let cfg = Config::default();
    let prov = Provenance {
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);
    s.config_picker = provider_options(&cfg.provider);
    s.config_picker_sel = 0;
    insta::assert_snapshot!(draw_config(&s, &sections, 160, 40));
}

/// The snapshot above renders via `to_string()`, which captures glyphs but not
/// cell styles — so the picker's `SEL_BG` selection highlight is otherwise
/// unverified. Build the identical state and render into a raw `Buffer` this
/// time, then assert the selected row's cell style directly.
///
/// Row/col offsets come from the layout in `render_config`: with the picker
/// open, columns are [rail 22 | fields 40 | picker Min(20)] inside the inner
/// area (which starts at x=1, y=1 — one cell in from the rounded border), so
/// the picker column starts at x = 1 + 22 + 40 = 63; `picker_x` below (70) is
/// comfortably inside that column's padded text. Picker rows start at y = 1
/// (right under the top border, no header row), one per `config_picker`
/// entry — confirmed against
/// `tests/snapshots/shell_snapshot__config_overlay_provider_picker.snap`,
/// where row y=1 is the selected `provider` entry. (The registry previously
/// also carried `[planned]`, non-selectable entries styled `DIM`; those were
/// removed in Task 8 and the styling assertion retired alongside them.)
#[test]
fn config_overlay_provider_picker_selection_styles() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::{build_sections, provider_options};
    use zoid_tui::render::render_config;
    use zoid_tui::state::ConfigCol;
    use zoid_tui::tokens::color;

    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    s.config_section = 0;
    s.config_field = 0; // "provider" row
    s.config_col = ConfigCol::Picker;
    let cfg = Config::default();
    let prov = Provenance {
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);
    s.config_picker = provider_options(&cfg.provider);
    s.config_picker_sel = 0;

    // The fixture's selected row must be selectable so the SEL_BG
    // highlight is meaningful; every surviving registry entry is
    // selectable now that the `[planned]` rows have been removed.
    assert!(
        s.config_picker[s.config_picker_sel].selectable,
        "fixture's selected row must be selectable"
    );

    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            render_config(f, &s, &sections, area);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();

    let picker_x: u16 = 70;
    let sel_y = 1 + s.config_picker_sel as u16;

    let sel_cell = &buffer[(picker_x, sel_y)];
    assert_eq!(
        sel_cell.bg,
        color::SEL_BG,
        "selected picker row must render the SEL_BG highlight"
    );
}

/// Config overlay (Task 9): graceful degradation below the 160×40 baseline.
/// At a narrower width the sections rail + fields still render, but three
/// columns (rail 22 + fields 40 + picker 20 = 82 cols min, checked against
/// the inner body width i.e. terminal width minus the 2-cell outer border)
/// no longer fit side-by-side, so the open provider picker floats as a
/// rounded overlay card on top of the fields column instead of squeezing
/// into a sliver third column. 80 cols of terminal width (78 cols of inner
/// body) is comfortably below the 82-col three-column minimum, so this
/// exercises the degraded path. Never blank: sections rail, fields, and the
/// overlaid picker card must all be visible.
#[test]
fn config_overlay_narrow_degrades() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::{build_sections, provider_options};
    use zoid_tui::state::ConfigCol;

    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    s.config_section = 0;
    s.config_field = 0; // "provider" row
    s.config_col = ConfigCol::Picker;
    let cfg = Config::default();
    let prov = Provenance {
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);
    s.config_picker = provider_options(&cfg.provider);
    s.config_picker_sel = 0;
    insta::assert_snapshot!(draw_config(&s, &sections, 80, 24));
}

/// Code review fix for Task 9: the degraded overlay picker (rendered when the
/// picker is open but three columns don't fit, e.g. 80x24) must only
/// highlight its selected row when focus is actually on the picker
/// (`ConfigCol::Picker`), matching the column-3 (non-degraded) behavior. A
/// prior regression hardcoded `active = true` for the overlay, so it stayed
/// highlighted even when focus moved to `ConfigCol::Fields`. Across the whole
/// config render the only source of a `color::SEL_BG` cell background is the
/// picker's selected row (rail/fields spans use fg only), so counting
/// `SEL_BG` cells is a reliable oracle for "is the picker rendered as
/// active."
#[test]
fn config_overlay_narrow_degrades_respects_focus() {
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    use zoid_tui::config_view::{build_sections, provider_options};
    use zoid_tui::render::render_config;
    use zoid_tui::state::ConfigCol;
    use zoid_tui::tokens::color;

    let cfg = Config::default();
    let prov = Provenance {
        provider: Source::Default,
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
            thinking_enabled: Source::Default,
            thinking_effort: Source::Default, approval: Source::Default, ui_edit_diff: Source::Default, ui_edit_diff_inline: Source::Default,
    };
    let ks = [
        ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
        ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
    ];
    let sections = build_sections(&cfg, &prov, &ks);

    let sel_bg_count = |config_col: ConfigCol| {
        let mut s = ShellState::new();
        s.overlay = Overlay::Config;
        s.config_section = 0;
        s.config_field = 0; // "provider" row
        s.config_col = config_col;
        s.config_picker = provider_options(&cfg.provider);
        s.config_picker_sel = 0;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                render_config(f, &s, &sections, area);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .iter()
            .filter(|c| c.bg == color::SEL_BG)
            .count()
    };

    // Case A: focus is on the picker — the overlay's selected row IS
    // highlighted.
    let with_focus = sel_bg_count(ConfigCol::Picker);
    assert!(
        with_focus > 0,
        "overlay picker must highlight its selected row when focus is on the picker"
    );

    // Case B: focus is on fields, but the picker is still open — the overlay
    // must NOT highlight. This is the case that fails against the pre-fix
    // hardcoded `true` and passes after the fix.
    let without_focus = sel_bg_count(ConfigCol::Fields);
    assert_eq!(
        without_focus, 0,
        "overlay picker must not highlight its selected row when focus is elsewhere"
    );
}

/// Quick-switch (`Alt+P`) overlay (Task 11): a two-pane floating card —
/// providers left (with a current-marker dot), models right — rendered via
/// `render_provider_switch`, mirroring the config picker's `picker_lines`
/// styling. Never blank: both panes and the word-based footer must render.
#[test]
fn provider_switch_card() {
    use zoid_tui::config_view::{model_options, provider_options};
    use zoid_tui::render::render_provider_switch;
    use zoid_tui::state::SwitchPane;

    let mut s = ShellState::new();
    s.overlay = Overlay::ProviderSwitch;
    s.switch_providers = provider_options("ollama-cloud");
    s.switch_models = model_options("anthropic-api", "");
    s.switch_provider_sel = 0;
    s.switch_model_sel = 0;
    s.switch_pane = SwitchPane::Provider;

    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            render_provider_switch(f, &s, area);
        })
        .unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn table_basic() {
    use zoid_core::projection::ChatMsg;
    let state = ShellState::default();
    let msgs = vec![ChatMsg::Assistant {
        text: "| Name | Kind | Note |\n| :--- | :---: | ---: |\n| alpha | code | long enough to maybe wrap if narrow |\n| **bold** | `x` | short |\n".to_string(),
        tool_calls: vec![],
        ts: 0,
        thinking: None,
    }];
    let rendered = draw(&state, &msgs, 60, 18);
    insta::assert_snapshot!(rendered);
}
