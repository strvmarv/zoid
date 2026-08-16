//! Shared scene fixtures for the `preview` and `web_capture` examples.
//! (Files under `examples/<dir>/` are modules, not example binaries.)

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::ChatView;
use zoid_tui::config_view::PickOption;
use zoid_tui::layout::{compute, conv_text_width};
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, McpStatusRow, Overlay, ShellState, SwitchPane, Zoom};
use zoid_tui::{onboarding, EconomyView};

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
                error_kind: None,
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
                error_kind: None,
            compacted: false,
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "c2".into(),
            name: "shell".into(),
            output: "FAILED\n".into(),
            is_error: true,
                error_kind: None,
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

/// Connected MCP servers for the "your tools" frame (server list + tool counts —
/// the only MCP state the renderer shows; it does not list individual tools).
fn seeded_mcp_status() -> Vec<McpStatusRow> {
    vec![
        McpStatusRow {
            name: "filesystem".into(),
            state: "ready".into(),
            tool_count: 8,
        },
        McpStatusRow {
            name: "github".into(),
            state: "ready".into(),
            tool_count: 12,
        },
        McpStatusRow {
            name: "postgres".into(),
            state: "ready".into(),
            tool_count: 6,
        },
    ]
}

/// Provider options for the quick-switch — HAND-SEEDED to the publicly-announced
/// providers only. The 0.4.0 release notes name Ollama, Anthropic, OpenAI, Google
/// Gemini, and OpenCode Zen; the latter three are reached through the `opencode-zen`
/// gateway (its model catalog carries the `gpt-*`/`gemini-*`/`claude-*` ids), so
/// Zen is the faithful provider row and its detail names those families. Ollama
/// stays current so the downstream "chosen"/"runs" frames read coherently.
/// Do NOT use config_view::provider_options(): it enumerates the whole registry,
/// including `zai` (not publicly announced) and any planned providers, and would
/// leak them into the frame.
fn seeded_switch_providers() -> Vec<PickOption> {
    vec![
        PickOption {
            id: "ollama".into(),
            label: "Ollama".into(),
            detail: "local & cloud".into(),
            selectable: true,
            is_current: true,
        },
        PickOption {
            id: "anthropic".into(),
            label: "Anthropic".into(),
            detail: "cloud".into(),
            selectable: true,
            is_current: false,
        },
        PickOption {
            id: "opencode-zen".into(),
            label: "OpenCode Zen".into(),
            detail: "openai · gemini · claude".into(),
            selectable: true,
            is_current: false,
        },
    ]
}

/// Models shown for the highlighted (Ollama) provider. Real registry ids; the
/// default `glm-5.2:cloud` (a 1M-context model) is current.
fn seeded_switch_models() -> Vec<PickOption> {
    vec![
        PickOption {
            id: "glm-5.2:cloud".into(),
            label: "glm-5.2:cloud".into(),
            detail: "1M context".into(),
            selectable: true,
            is_current: true,
        },
        PickOption {
            id: "glm-5.2".into(),
            label: "glm-5.2".into(),
            detail: "local".into(),
            selectable: true,
            is_current: false,
        },
    ]
}

/// A short, realistic turn that calls an MCP-provided tool (dotted name signals
/// it comes from the `github` server — reinforcing "your tools").
fn seeded_tools_models_turn() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "any open issues about the login flow?".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "checking the github MCP server".into(),
            tool_calls: vec![ToolCallRef {
                id: "t1".into(),
                name: "github.search_issues".into(),
                args: r#"{"q":"login flow"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "t1".into(),
            name: "github.search_issues".into(),
            output: "#412 login redirect loop\n#419 2FA prompt after logout".into(),
            is_error: false,
                error_kind: None,
            compacted: false,
            ts: 0,
        },
    ]
}

/// Tasks for the tools-models scene's Tasks drawer — coherent with the story
/// (a server connected, a model chosen). Two tasks, matching the base state's
/// `tasks_len = 2`, so the drawer shows real rows, not empty reserved space.
fn seeded_tools_models_tasks() -> Vec<zoid_core::tasks::TaskItem> {
    use zoid_core::tasks::{TaskItem, TaskStatus};
    vec![
        TaskItem {
            text: "connect the github MCP server".into(),
            status: TaskStatus::Done,
        },
        TaskItem {
            text: "switch to glm-5.2:cloud".into(),
            status: TaskStatus::Active,
        },
    ]
}

/// The brainstorming skill firing inside the Superpowers mode: the assistant
/// announces the skill (as `using-superpowers` instructs) and asks a single
/// focused design question (brainstorming's one-question-at-a-time rule). Seeded
/// transcript — the same staging discipline as the other scenes; the SUPERPOWERS
/// mode chip (real state) is what marks the mode in use.
fn seeded_brainstorm_turn() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User {
            text: "let's add rate limiting to the API".into(),
            ts: 0,
        },
        ChatMsg::Assistant {
            thinking: None,
            text: "Using the brainstorming skill to turn this into a design — one question at a time.\n\nFirst: should limits be per-API-key, per-IP, or global?".into(),
            tool_calls: vec![],
            ts: 0,
        },
    ]
}

/// Tasks a scene renders into the Tasks drawer (empty for scenes without tasks).
fn scene_tasks(name: &str) -> Vec<zoid_core::tasks::TaskItem> {
    match name {
        "economy" | "context-economy" => seeded_tasks(),
        "tools-models" => seeded_tools_models_tasks(),
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
    // Pre-rendered conversation body, when the frame supplies one (the
    // extensibility scene's first-run frame injects the real onboarding
    // empty-state here). `None` lets render_shell render `msgs` itself.
    body: Option<&[Line<'static>]>,
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
            render_shell(f, state, economy, msgs, body, tasks, &input, false, &view);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// The real first-run empty-state body. The app paints this (via
/// `onboarding::empty_state_lines`) into the conversation pane whenever a
/// session has no messages; we reproduce it here — at the same
/// `conv_text_width` the app uses — so the capture shows the genuine
/// "Run :plugin install superpowers …" hint rather than a hand-drawn mock.
fn onboarding_body(state: &ShellState, w: u16, h: u16) -> Vec<Line<'static>> {
    let area = Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    let layout = compute(area, state);
    let body_w = conv_text_width(layout.conversation.width) as usize;
    // first_time_user = true, offer_superpowers = true: the new-user path that
    // surfaces the Superpowers install hint.
    onboarding::empty_state_lines(true, true, body_w)
}

/// Per-frame pre-rendered body, if the scene/frame supplies one. Only the
/// extensibility scene's first-run frame (index 0) does.
fn scene_frame_body(
    name: &str,
    idx: usize,
    state: &ShellState,
    w: u16,
    h: u16,
) -> Option<Vec<Line<'static>>> {
    match (name, idx) {
        ("extensibility", 0) => Some(onboarding_body(state, w, h)),
        _ => None,
    }
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
        "tools-models" => {
            // Enriched right-rail (repo/session), reused across frames.
            let base = || {
                let (s, _m, _e) = scene("economy");
                s
            };
            let turn = seeded_tools_models_turn();

            // F0 — your tools: the MCP servers overlay.
            let mut f0 = base();
            f0.overlay = Overlay::Mcp;
            f0.mcp_status = seeded_mcp_status();

            // F1 — your models: the provider/model quick-switch, Model pane.
            let mut f1 = base();
            f1.overlay = Overlay::ProviderSwitch;
            f1.switch_providers = seeded_switch_providers();
            f1.switch_models = seeded_switch_models();
            f1.switch_pane = SwitchPane::Model;
            f1.switch_provider_sel = 0; // Ollama
            f1.switch_model_sel = 0; // glm-5.2:cloud (current)

            // F2 — chosen: overlay closed, session drawer shows model·provider,
            // the user asks a question.
            let mut f2 = base();
            f2.model = "glm-5.2:cloud".into();
            f2.provider = "ollama".into();

            // F3 — it runs, locally: a tool is executing.
            let mut f3 = base();
            f3.model = "glm-5.2:cloud".into();
            f3.provider = "ollama".into();
            f3.busy = true;
            f3.active_tool = Some("github.search_issues".into());

            // F3 shows the tool genuinely in flight: the assistant's tool CALL
            // is visible (turn[..2]) but its result is NOT yet on screen, so the
            // "running" status indicator is coherent (not paired with a returned
            // result). The ToolResult (turn[2]) intentionally stays unrevealed.
            vec![
                (f0, turn[..1].to_vec(), empty_economy()),
                (f1, turn[..1].to_vec(), empty_economy()),
                (f2, turn[..1].to_vec(), seeded_economy()),
                (f3, turn[..2].to_vec(), seeded_economy()),
            ]
        }
        "extensibility" => {
            // F0 — first run: an empty session. `msgs` is empty, so
            // render_shell_scene_seq injects the real onboarding empty-state
            // (the "Run :plugin install superpowers …" hint) as the body. Chat
            // is the non-removable floor mode.
            let mut f0 = ShellState::new();
            f0.first_time_user = true;
            f0.repo_name = "api".into();
            f0.branch = "main".into();
            // active_mode defaults to "Chat"; mode_names defaults to ["Chat"].

            // F1 — Superpowers installed & active: the mode chip flips to
            // SUPERPOWERS and the brainstorming skill fires. Enriched rail so
            // the frame reads as a real session.
            let mut f1 = ShellState::new();
            f1.active_mode = "Superpowers".into();
            f1.mode_names = vec!["Chat".into(), "Superpowers".into()];
            f1.session_name = "rate limiting".into();
            f1.model = "glm-5.2".into();
            f1.provider = "ollama".into();
            f1.session_tokens = 12_800;
            f1.cached_tokens = 8_100;
            f1.cache_supported = true;
            f1.ctx_used = 9_000;
            f1.ctx_ceiling = 128_000;
            f1.repo_name = "api".into();
            f1.branch = "main".into();

            vec![
                (f0, vec![], empty_economy()),
                (f1, seeded_brainstorm_turn(), empty_economy()),
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
        .enumerate()
        .map(|(idx, (state, msgs, economy))| {
            let body = scene_frame_body(name, idx, &state, w, h);
            render_one(&state, &msgs, &economy, &tasks, body.as_deref(), w, h)
        })
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

    #[test]
    fn tools_models_sequence_stages_tools_then_models_then_run() {
        let seq = scene_seq("tools-models");
        assert_eq!(seq.len(), 4, "expected a 4-frame tools-models sequence");

        // F0: the MCP servers overlay (your tools).
        assert_eq!(seq[0].0.overlay, Overlay::Mcp, "frame 0 shows MCP overlay");
        assert!(!seq[0].0.mcp_status.is_empty(), "frame 0 has MCP servers");

        // F1: the provider/model quick-switch (your models).
        assert_eq!(
            seq[1].0.overlay,
            Overlay::ProviderSwitch,
            "frame 1 shows the quick-switch picker"
        );
        assert!(!seq[1].0.switch_providers.is_empty(), "providers seeded");
        assert!(!seq[1].0.switch_models.is_empty(), "models seeded");
        // Leak guard: only publicly-announced providers may appear. The 0.4.0
        // release notes name Ollama, Anthropic, OpenAI, Google Gemini, and
        // OpenCode Zen (the latter three via the opencode-zen gateway). `zai`
        // and any internal/planned provider ids must never reach a frame.
        for p in &seq[1].0.switch_providers {
            assert!(
                matches!(p.id.as_str(), "ollama" | "anthropic" | "opencode-zen"),
                "public providers only; got leaked provider id {:?}",
                p.id
            );
        }

        // F2/F3: overlays closed; F3 shows a tool running.
        assert_eq!(seq[2].0.overlay, Overlay::None, "frame 2 overlay closed");
        assert_eq!(seq[3].0.overlay, Overlay::None, "frame 3 overlay closed");
        assert!(seq[3].0.busy, "frame 3 is busy (a tool is running)");
        assert!(
            seq[3].0.active_tool.is_some(),
            "frame 3 names the running tool"
        );

        // Renders at the required min size.
        let frames = render_shell_scene_seq("tools-models", 160, 40);
        assert_eq!(frames.len(), 4);
    }

    /// Read a rendered buffer back to a flat string (row-major) for text asserts.
    fn buffer_text(buf: &Buffer) -> String {
        let area = *buf.area();
        (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn extensibility_sequence_onboarding_then_superpowers_firing() {
        let seq = scene_seq("extensibility");
        assert_eq!(seq.len(), 2, "expected a 2-frame extensibility sequence");

        // F0 — first run: empty session (onboarding body injected at render), Chat floor.
        assert!(
            seq[0].1.is_empty(),
            "frame 0 is the empty-state (onboarding)"
        );
        assert_eq!(
            seq[0].0.active_mode, "Chat",
            "frame 0 mode is the Chat floor"
        );

        // F1 — Superpowers mode active with the brainstorming turn.
        assert_eq!(
            seq[1].0.active_mode, "Superpowers",
            "frame 1 mode is Superpowers"
        );
        assert!(
            seq[1].0.mode_names.iter().any(|n| n == "Superpowers"),
            "Superpowers is an installed mode"
        );
        assert!(!seq[1].1.is_empty(), "frame 1 has the brainstorming turn");

        // Rendered frames carry the REAL onboarding hint (F0) and the SUPERPOWERS
        // mode chip (F1) — not hand-authored mock text.
        let frames = render_shell_scene_seq("extensibility", 160, 40);
        assert_eq!(frames.len(), 2);
        let f0 = buffer_text(&frames[0]);
        assert!(
            f0.contains(":plugin install superpowers"),
            "frame 0 must render the real onboarding install hint"
        );
        let f1 = buffer_text(&frames[1]);
        assert!(
            f1.contains("SUPERPOWERS"),
            "frame 1 must render the SUPERPOWERS mode chip"
        );
        assert!(
            f1.contains("brainstorming"),
            "frame 1 must show the brainstorming skill firing"
        );
    }
}
