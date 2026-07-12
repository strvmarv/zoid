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
            thinking: None,
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
            thinking: None,
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

/// A short, realistic task list for the hero scene's Tasks drawer.
fn seeded_tasks() -> Vec<zoid_core::tasks::TaskItem> {
    use zoid_core::tasks::{TaskItem, TaskStatus};
    vec![
        TaskItem {
            text: "reproduce the 500".into(),
            status: TaskStatus::Done,
        },
        TaskItem {
            text: "patch the unwrapped lookup".into(),
            status: TaskStatus::Active,
        },
    ]
}

/// Tasks a scene renders into the Tasks drawer (empty for scenes without tasks).
fn scene_tasks(name: &str) -> Vec<zoid_core::tasks::TaskItem> {
    match name {
        "economy" | "context-economy" => seeded_tasks(),
        _ => vec![],
    }
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
            // Populate the right-rail widgets so the frame reads as real usage.
            s.session_name = "diagnose 500".into();
            s.model = "glm-5.2".into();
            s.provider = "ollama".into();
            s.duration = "12m".into();
            s.session_tokens = 48_200;
            s.cached_tokens = 31_040;
            s.cache_supported = true;
            s.ctx_used = 18_000;
            s.ctx_ceiling = 128_000;
            s.repo_name = "api".into();
            s.branch = "main".into();
            s.changes_added = 24;
            s.changes_removed = 6;
            s.changes_files = 3;
            s.tasks_len = 2;
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

/// Render one frame (state + messages + economy + tasks) to a cloned buffer.
#[allow(dead_code)]
pub fn render_one(
    state: &ShellState,
    msgs: &[ChatMsg],
    economy: &EconomyView,
    tasks: &[zoid_core::tasks::TaskItem],
    w: u16,
    h: u16,
) -> Buffer {
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
            render_shell(f, state, economy, msgs, None, tasks, &input, false, &view);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Render a shell scene and return a clone of the rendered buffer.
// This module is compiled into both the `preview` and `web_capture` example
// crates; `preview` never calls this (only `web_capture` does), so it is dead
// code from `preview`'s build. Each consumer uses a subset — expected.
#[allow(dead_code)]
pub fn render_shell_scene(name: &str, w: u16, h: u16) -> Buffer {
    let (state, msgs, economy) = scene(name);
    render_one(&state, &msgs, &economy, &scene_tasks(name), w, h)
}

/// The context-economy story as an ordered set of states. Reuses the enriched
/// `economy` ShellState and progressively reveals the seeded turn; the context
/// rail fills once the compaction event lands (frame 2), so the player animates
/// "work happens → context becomes a managed, measured resource".
#[allow(dead_code)]
pub fn scene_seq(name: &str) -> Vec<(ShellState, Vec<ChatMsg>, EconomyView)> {
    match name {
        "context-economy" => {
            // The enriched right-rail state, reused for every frame.
            let base = || {
                let (s, _m, _e) = scene("economy");
                s
            };
            let turn = seeded(); // [user, assistant+search, compacted result, answer]
            vec![
                (base(), turn[..1].to_vec(), empty_economy()),
                (base(), turn[..2].to_vec(), empty_economy()),
                (base(), turn[..3].to_vec(), seeded_economy()),
                (base(), turn[..4].to_vec(), seeded_economy()),
            ]
        }
        _ => vec![scene(name)],
    }
}

/// Render every frame of a sequence to cloned buffers.
#[allow(dead_code)]
pub fn render_shell_scene_seq(name: &str, w: u16, h: u16) -> Vec<Buffer> {
    let tasks = scene_tasks(name);
    scene_seq(name)
        .into_iter()
        .map(|(state, msgs, economy)| render_one(&state, &msgs, &economy, &tasks, w, h))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn economy_scene_is_populated() {
        // The captured hero scene must read as real usage: a named session with
        // real token/cache/ctx numbers and a couple of tasks — not empty rails.
        let (s, _msgs, _econ) = scene("economy");
        assert!(!s.session_name.is_empty(), "session should be named");
        assert!(s.session_tokens > 0, "session tokens should be non-zero");
        assert!(
            s.ctx_used > 0 && s.ctx_ceiling > s.ctx_used,
            "ctx should be seeded"
        );
        assert_eq!(scene_tasks("economy").len(), 2, "two tasks expected");
    }

    #[test]
    fn context_economy_sequence_reveals_the_turn() {
        // Four frames: user prompt → +searching → +compaction → +answer.
        let seq = scene_seq("context-economy");
        assert_eq!(seq.len(), 4, "expected a 4-frame reveal");
        assert_eq!(seq[0].1.len(), 1, "frame 0 shows only the user prompt");
        assert_eq!(seq[3].1.len(), 4, "final frame shows the whole turn");
        // The rail fills once compaction happens (frame 2 onward).
        assert!(seq[0].2.rows.is_empty(), "frame 0 rail empty");
        assert!(!seq[2].2.rows.is_empty(), "frame 2 rail populated");

        // And each frame renders to a buffer at the required min size.
        let frames = render_shell_scene_seq("context-economy", 160, 40);
        assert_eq!(frames.len(), 4);
    }
}
