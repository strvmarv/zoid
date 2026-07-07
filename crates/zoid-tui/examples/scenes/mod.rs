//! Shared scene fixtures for the `preview` and `web_capture` examples.
//! (Files under `examples/<dir>/` are modules, not example binaries.)

use ratatui::buffer::Buffer;
use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Overlay, ShellState, Zoom};
use zoid_tui::EconomyView;

pub fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "what's causing the 500?".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            text: "searching for the failing lookup".into(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "search".into(),
                args: r#"{"query":"lookup"}"#.into(),
            }],
            ts: 0,
        },
        // ACM-1: a compacted tool-result — the ⊟ chip at Normal, the ⊟ header
        // label at Detail (docs/ux/README.md visual-language table).
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "search".into(),
            output: "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".into(),
            is_error: false,
            compacted: true,
            ts: 0,
        },
        ChatMsg::Assistant {
            text: "an unwrapped lookup in the handler.".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ]
}

/// File + symbol + error objects (P4d ④) — same shape as the shell_snapshot
/// fixture, so `objects`/`verbs` scenes show a real, non-empty picker.
fn seeded_objects() -> Vec<ChatMsg> {
    vec![
        ChatMsg::Assistant {
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

fn empty_economy() -> EconomyView {
    use zoid_core::context::ContextWindow;
    use zoid_core::economy::ChurnTimeline;
    EconomyView::build(&ContextWindow::default(), &ChurnTimeline::default(), 0)
}

fn seeded_economy() -> EconomyView {
    use zoid_core::context::{ContextItem, ContextWindow, Heat, ItemKind};
    use zoid_core::economy::{ChurnPoint, ChurnTimeline};
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
            ChurnPoint {
                turn: 0,
                tokens: 10,
                cached: 0,
                resent_tokens: 0,
            },
            ChurnPoint {
                turn: 1,
                tokens: 30,
                cached: 8,
                resent_tokens: 0,
            },
            ChurnPoint {
                turn: 2,
                tokens: 12,
                cached: 20,
                resent_tokens: 0,
            },
            ChurnPoint {
                turn: 3,
                tokens: 48,
                cached: 40,
                resent_tokens: 0,
            },
            ChurnPoint {
                turn: 4,
                tokens: 12,
                cached: 12,
                resent_tokens: 0,
            },
        ],
    };
    EconomyView::build(&w, &churn, 0)
}

pub fn scene(name: &str) -> (ShellState, Vec<ChatMsg>, EconomyView) {
    let mut s = ShellState::new();
    match name {
        "files" => {
            s.toggle_drawer(DrawerId::Session);
        }
        "palette" => {
            s.overlay = Overlay::Palette;
            s.palette.query = "build".into();
        }
        "build" => {
            s.active_mode = "Build".into();
            return (s, vec![], empty_economy());
        }
        "economy" => {
            return (s, seeded(), seeded_economy());
        }
        "summary" => {
            s.zoom = Zoom::Summary;
        }
        "detail" => {
            s.zoom = Zoom::Detail;
        }
        "objects" => {
            s.overlay = Overlay::Objects;
            return (s, seeded_objects(), empty_economy());
        }
        "verbs" => {
            s.overlay = Overlay::Verbs;
            return (s, seeded_objects(), empty_economy());
        }
        _ => {} // "chat" / default
    }
    (s, seeded(), empty_economy())
}

/// Render a shell scene and return a clone of the rendered buffer.
// This module is compiled into both the `preview` and `web_capture` example
// crates; `preview` never calls this (only `web_capture` does), so it is dead
// code from `preview`'s build. Each consumer uses a subset — expected.
#[allow(dead_code)]
pub fn render_shell_scene(name: &str, w: u16, h: u16) -> Buffer {
    let (state, msgs, economy) = scene(name);
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let view = ChatView {
        zoom: state.zoom,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    };
    terminal
        .draw(|f| {
            render_shell(f, &state, &economy, &msgs, None, &[], &[], &input, false, &view);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}
