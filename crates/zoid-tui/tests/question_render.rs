//! Regression: `ask_user` choices that are paragraph-length must wrap and be
//! fully visible, not clipped at the card edge (the bug that made the overlay's
//! choices look invisible and provoked the scroll that dismissed it).
use ratatui::{backend::TestBackend, Terminal};
use zoid_tui::question::QuestionState;
use zoid_tui::render::render_question;

#[test]
fn long_choices_wrap_and_tails_are_visible() {
    let q = QuestionState::new(
        "Which approach should we take for the status bar redesign given the constraints?",
        vec![
            "Add brand new tokenized glyphs for repo branch worktree and changes which follows existing conventions but requires snapshot updates across the entire shell layout".into(),
            "Reuse the existing glyph set and only add color coding to distinguish the groups which is a smaller change with far less snapshot churn overall".into(),
        ],
    );
    let backend = TestBackend::new(80, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_question(f, f.area(), &q)).unwrap();
    let dump = terminal.backend().to_string();
    // The FINAL word of each long choice must appear — proof it wrapped rather
    // than being horizontally clipped (which would drop these tails).
    assert!(dump.contains("layout"), "tail of choice 1 clipped:\n{dump}");
    assert!(
        dump.contains("overall"),
        "tail of choice 2 clipped:\n{dump}"
    );
}
