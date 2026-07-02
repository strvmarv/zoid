use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
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
    let msgs: Vec<ChatMsg> = vec![];
    let input = TextArea::default();
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_shell(
                f,
                state,
                &empty_economy(),
                &msgs,
                tasks,
                &input,
                false,
                &normal_view(),
            );
        })
        .unwrap();
    terminal.backend().to_string()
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
