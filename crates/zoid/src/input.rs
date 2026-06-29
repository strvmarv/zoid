//! Pure key classification for the Chat loop — terminal-free and unit-tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[allow(dead_code)] // allow(dead_code): consumed by the loop in Task 11
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    ToggleMode,
    Submit,
    Newline,
    Edit,
}

/// Classify a key press for the Chat loop. Order matters: control combos and
/// special keys are matched before falling through to plain editing.
#[allow(dead_code)] // allow(dead_code): consumed by the loop in Task 11
pub fn classify(key: KeyEvent) -> KeyAction {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
        (KeyCode::BackTab, _) => KeyAction::ToggleMode,
        (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) => KeyAction::Newline,
        (KeyCode::Enter, _) => KeyAction::Submit,
        _ => KeyAction::Edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_c_quits() {
        assert_eq!(classify(key(KeyCode::Char('c'), KeyModifiers::CONTROL)), KeyAction::Quit);
    }

    #[test]
    fn shift_tab_toggles_mode() {
        assert_eq!(classify(key(KeyCode::BackTab, KeyModifiers::SHIFT)), KeyAction::ToggleMode);
        // crossterm sometimes reports BackTab with no modifier flag
        assert_eq!(classify(key(KeyCode::BackTab, KeyModifiers::NONE)), KeyAction::ToggleMode);
    }

    #[test]
    fn plain_enter_submits_alt_enter_newlines() {
        assert_eq!(classify(key(KeyCode::Enter, KeyModifiers::NONE)), KeyAction::Submit);
        assert_eq!(classify(key(KeyCode::Enter, KeyModifiers::ALT)), KeyAction::Newline);
    }

    #[test]
    fn plain_char_is_edit() {
        assert_eq!(classify(key(KeyCode::Char('q'), KeyModifiers::NONE)), KeyAction::Edit);
        assert_eq!(classify(key(KeyCode::Char('a'), KeyModifiers::NONE)), KeyAction::Edit);
    }
}
