use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::context::ContextWindow;
use zoid_core::economy::ChurnTimeline;
use zoid_core::projection::ChatMsg;
use zoid_core::tasks::{TaskItem, TaskStatus};
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{ShellState, Zoom};
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

/// A taller frame than the usual 100×24: at the default 3-drawer rail height,
/// the tasks drawer (4th, below Context) has no room left at all (spec's
/// stacked-box layout clamps drawers to the rail's available rows). 32 rows
/// gives the tasks box a real 3-row body so its glyphs are actually visible.
fn draw(state: &ShellState, tasks: &[TaskItem]) -> String {
    draw_at(state, tasks, 100, 32)
}

fn draw_at(state: &ShellState, tasks: &[TaskItem], w: u16, h: u16) -> String {
    // Mirror the bin: publish the task count so the rail's fit allocator sizes
    // the Tasks drawer to the list it is about to render (otherwise it defaults
    // to the 1-row "none" height and truncates a multi-item list).
    let mut state = state.clone();
    state.tasks_len = tasks.len() as u16;
    let msgs: Vec<ChatMsg> = vec![];
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                &state,
                &empty_economy(),
                &msgs,
                None,
                tasks,
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    terminal.backend().to_string()
}

/// A list of `n` tasks (statuses cycle pending/active/done) for exercising the
/// Tasks drawer's content-driven growth.
fn many_tasks(n: usize) -> Vec<TaskItem> {
    (0..n)
        .map(|i| TaskItem {
            text: format!("task {i}"),
            status: match i % 3 {
                0 => TaskStatus::Done,
                1 => TaskStatus::Active,
                _ => TaskStatus::Pending,
            },
        })
        .collect()
}

#[test]
fn tasks_drawer_shows_pending_active_done_glyphs() {
    let s = ShellState::new();
    let items = vec![
        TaskItem {
            text: "write plan".into(),
            status: TaskStatus::Pending,
        },
        TaskItem {
            text: "implement drawer".into(),
            status: TaskStatus::Active,
        },
        TaskItem {
            text: "read brief".into(),
            status: TaskStatus::Done,
        },
    ];
    insta::assert_snapshot!(draw(&s, &items));
}

#[test]
fn tasks_drawer_empty_shows_no_tasks_line() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw(&s, &[]));
}

/// Baseline 160×40: the rail has slack, so every drawer sits at its ideal AND
/// the Tasks drawer grows past its 5-row base to show all 8 tasks — the
/// content-driven growth the fit allocator's second pass provides.
#[test]
fn tasks_drawer_grows_to_fit_eight_tasks_at_baseline() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw_at(&s, &many_tasks(8), 160, 40));
}

/// Squeezed rail (100×19 ⇒ ~14 inner rows, below the 16-row four-drawer
/// minimum): the two lowest-priority drawers (Repo, then Context) collapse to
/// header-only boxes so Session (its dense facts) and the Tasks list keep their
/// guaranteed row — every drawer is still visible as at least a title bar.
#[test]
fn squeezed_rail_collapses_repo_and_context_first() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw_at(&s, &many_tasks(4), 100, 19));
}
