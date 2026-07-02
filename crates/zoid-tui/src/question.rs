//! The `ask_user` question overlay: pick-list (with synthetic "Other…" and
//! "let you decide" rows) or free-text. Selection wraps via `palette::nav`.

use crate::route::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

const OTHER_LABEL: &str = "Other… (type my own)";
const DECIDE_LABEL: &str = "— let you decide —";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionMode {
    Pick,
    FreeText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionState {
    pub question: String,
    pub choices: Vec<String>,
    pub selected: usize,
    pub free_text: String,
    pub mode: QuestionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionOutcome {
    Choice(String),
    FreeText(String),
    LetYouDecide,
    /// The user picked "Other…": switch the overlay into free-text entry.
    EnterFreeText,
}

impl QuestionState {
    pub fn new(question: impl Into<String>, choices: Vec<String>) -> Self {
        let mode = if choices.is_empty() {
            QuestionMode::FreeText
        } else {
            QuestionMode::Pick
        };
        Self {
            question: question.into(),
            choices,
            selected: 0,
            free_text: String::new(),
            mode,
        }
    }

    /// Rows shown in pick mode: the model's choices + the two synthetic entries.
    pub fn rows(&self) -> Vec<String> {
        let mut r = self.choices.clone();
        r.push(OTHER_LABEL.to_string());
        r.push(DECIDE_LABEL.to_string());
        r
    }

    /// What the current selection / buffer resolves to when committed.
    pub fn resolved(&self) -> QuestionOutcome {
        match self.mode {
            QuestionMode::FreeText => {
                if self.free_text.is_empty() {
                    QuestionOutcome::LetYouDecide
                } else {
                    QuestionOutcome::FreeText(self.free_text.clone())
                }
            }
            QuestionMode::Pick => {
                let rows = self.rows();
                let idx = self.selected.min(rows.len() - 1);
                if idx == rows.len() - 1 {
                    QuestionOutcome::LetYouDecide
                } else if idx == rows.len() - 2 {
                    QuestionOutcome::EnterFreeText
                } else {
                    QuestionOutcome::Choice(rows[idx].clone())
                }
            }
        }
    }
}

/// Map a keypress to an `Action` while the question overlay is up.
pub fn route_question_key(state: &QuestionState, key: KeyEvent) -> Action {
    match state.mode {
        QuestionMode::Pick => match key.code {
            KeyCode::Up => Action::QuestionMove(-1),
            KeyCode::Down => Action::QuestionMove(1),
            KeyCode::Enter => Action::QuestionSelect,
            KeyCode::Esc => Action::QuestionAbort,
            _ => Action::Noop,
        },
        QuestionMode::FreeText => match key.code {
            KeyCode::Enter => Action::QuestionSelect,
            KeyCode::Esc => Action::QuestionAbort,
            KeyCode::Backspace => Action::QuestionBackspace,
            KeyCode::Char(c) => Action::QuestionChar(c),
            _ => Action::Noop,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn empty_choices_starts_in_free_text() {
        let q = QuestionState::new("why?", vec![]);
        assert_eq!(q.mode, QuestionMode::FreeText);
    }

    #[test]
    fn pick_rows_append_other_and_decide() {
        let q = QuestionState::new("db?", vec!["pg".into(), "sqlite".into()]);
        let rows = q.rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2], OTHER_LABEL);
        assert_eq!(rows[3], DECIDE_LABEL);
    }

    #[test]
    fn last_row_resolves_to_let_you_decide() {
        let mut q = QuestionState::new("db?", vec!["pg".into()]);
        q.selected = q.rows().len() - 1;
        assert_eq!(q.resolved(), QuestionOutcome::LetYouDecide);
    }

    #[test]
    fn other_row_resolves_to_enter_free_text() {
        let mut q = QuestionState::new("db?", vec!["pg".into()]);
        q.selected = q.rows().len() - 2;
        assert_eq!(q.resolved(), QuestionOutcome::EnterFreeText);
    }

    #[test]
    fn choice_row_resolves_to_that_choice() {
        let q = QuestionState::new("db?", vec!["pg".into(), "sqlite".into()]);
        assert_eq!(q.resolved(), QuestionOutcome::Choice("pg".into()));
    }

    #[test]
    fn empty_free_text_submit_is_let_you_decide() {
        let q = QuestionState::new("why?", vec![]);
        assert_eq!(q.resolved(), QuestionOutcome::LetYouDecide);
    }

    #[test]
    fn esc_routes_to_abort_in_both_modes() {
        let pick = QuestionState::new("db?", vec!["pg".into()]);
        assert_eq!(
            route_question_key(&pick, k(KeyCode::Esc)),
            Action::QuestionAbort
        );
        let free = QuestionState::new("why?", vec![]);
        assert_eq!(
            route_question_key(&free, k(KeyCode::Esc)),
            Action::QuestionAbort
        );
    }
}
