//! Key routing for the `:feedback` overlay. Tab/Shift+Tab cycle focus across
//! Kind → Title → Body; Up/Down cycle the kind when focused; Ctrl+Enter submits;
//! Esc aborts. Mirrors `question.rs`'s `route_question_key`.

use crate::route::Action;
use crate::state::{FeedbackField, FeedbackState};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Map a keypress to an `Action` while the feedback overlay is open.
pub fn route_feedback_key(state: &FeedbackState, key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Action::FeedbackAbort,
        KeyCode::Tab => Action::FeedbackMoveFocus(1),
        KeyCode::BackTab => Action::FeedbackMoveFocus(-1),
        KeyCode::Enter if ctrl => Action::FeedbackSubmit,
        KeyCode::Enter => match state.focus {
            FeedbackField::Title => Action::FeedbackMoveFocus(1),
            FeedbackField::Body => Action::Noop,
            FeedbackField::Kind => Action::FeedbackMoveFocus(1),
        },
        KeyCode::Up if state.focus == FeedbackField::Kind => Action::FeedbackCycleKind(-1),
        KeyCode::Down if state.focus == FeedbackField::Kind => Action::FeedbackCycleKind(1),
        KeyCode::Backspace => Action::FeedbackBackspace,
        KeyCode::Char(c) if ctrl && (c == 'm' || c == '\n' || c == '\r') => Action::FeedbackSubmit,
        KeyCode::Char(c) => Action::FeedbackChar(c),
        _ => Action::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn esc_aborts() {
        let s = FeedbackState::new();
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Esc, KeyModifiers::NONE)),
            Action::FeedbackAbort
        );
    }

    #[test]
    fn tab_moves_focus_forward() {
        let s = FeedbackState::new();
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Tab, KeyModifiers::NONE)),
            Action::FeedbackMoveFocus(1)
        );
    }

    #[test]
    fn up_down_cycle_kind_only_when_kind_focused() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Kind;
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Up, KeyModifiers::NONE)),
            Action::FeedbackCycleKind(-1)
        );
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Down, KeyModifiers::NONE)),
            Action::FeedbackCycleKind(1)
        );
        s.focus = FeedbackField::Title;
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Up, KeyModifiers::NONE)),
            Action::Noop
        );
    }

    #[test]
    fn enter_in_title_moves_to_body_not_submit() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Title;
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Enter, KeyModifiers::NONE)),
            Action::FeedbackMoveFocus(1)
        );
    }

    #[test]
    fn ctrl_enter_submits_in_body() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Body;
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Enter, KeyModifiers::CONTROL)),
            Action::FeedbackSubmit
        );
    }

    #[test]
    fn char_routes_to_feedback_char() {
        let s = FeedbackState::new();
        assert_eq!(
            route_feedback_key(&s, k(KeyCode::Char('x'), KeyModifiers::NONE)),
            Action::FeedbackChar('x')
        );
    }
}
