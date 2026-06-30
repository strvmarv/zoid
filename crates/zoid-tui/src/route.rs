//! The app-framework floor: contextual key routing, mouse hit-testing, and the
//! `Action` vocabulary. Pure — synthetic events in, `Action` out (spec §13/§14.1).
//! Precedence: an active overlay captures keys first; then global combos
//! (`^C`/`^P`/`⇧Tab`); then focus-contextual keys (Input edits; Conversation/Rail
//! navigate). `:` opens the command line only when focus ≠ Input.

use crate::command::{parse_command, Command};
use crate::layout::{in_rect, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches};
use crate::state::{DrawerId, Focus, Overlay, ShellState};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    SwitchMode,
    FocusNext,
    FocusRegion(Focus),
    OpenPalette,
    OpenCommandLine,
    CloseOverlay,
    ToggleDrawer(DrawerId),
    PaletteMove(i32),
    PaletteChar(char),
    PaletteBackspace,
    /// Run the palette's currently-selected command (resolved by the bin/state).
    PaletteRun,
    CmdlineChar(char),
    CmdlineBackspace,
    /// Run the command line buffer (parsed into a `Command`).
    RunCommand(Command),
    ScrollConversation(i32),
    Submit,
    Newline,
    Edit(KeyEvent),
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Conversation,
    Input,
    DrawerHeader(DrawerId),
    None,
}

fn ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

pub fn route_key(state: &ShellState, key: KeyEvent) -> Action {
    // 1. Overlays capture keys first.
    match state.overlay {
        Overlay::Palette => return route_palette_key(key),
        Overlay::CommandLine => return route_cmdline_key(state, key),
        Overlay::None => {}
    }

    // 2. Global combos.
    if ctrl(&key, 'c') {
        return Action::Quit;
    }
    if ctrl(&key, 'p') {
        return Action::OpenPalette;
    }
    match key.code {
        KeyCode::BackTab => return Action::SwitchMode,
        KeyCode::Tab => return Action::FocusNext,
        _ => {}
    }

    // 3. Focus-contextual.
    match state.focus {
        Focus::Input => match (key.code, key.modifiers) {
            (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) => Action::Newline,
            (KeyCode::Enter, _) => Action::Submit,
            _ => Action::Edit(key),
        },
        Focus::Conversation | Focus::Rail => match key.code {
            KeyCode::Char(':') => Action::OpenCommandLine,
            KeyCode::Char('j') | KeyCode::Down => {
                if state.focus == Focus::Rail {
                    Action::Noop // rail item nav lands with economy content (P3)
                } else {
                    Action::ScrollConversation(1)
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if state.focus == Focus::Rail {
                    Action::Noop
                } else {
                    Action::ScrollConversation(-1)
                }
            }
            KeyCode::Esc => Action::FocusRegion(Focus::Input),
            _ => Action::Noop,
        },
    }
}

fn route_palette_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::PaletteRun,
        KeyCode::Up => Action::PaletteMove(-1),
        KeyCode::Down => Action::PaletteMove(1),
        KeyCode::Backspace => Action::PaletteBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Action::PaletteChar(c),
        _ => Action::Noop,
    }
}

fn route_cmdline_key(state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::RunCommand(parse_command(&state.cmdline.buffer)),
        KeyCode::Backspace => Action::CmdlineBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Action::CmdlineChar(c),
        _ => Action::Noop,
    }
}

/// Map a screen point to a main-surface target (overlays are keyboard-driven).
pub fn hit_test(layout: &ShellLayout, col: u16, row: u16) -> Target {
    for (id, r) in &layout.drawer_headers {
        if in_rect(*r, col, row) {
            return Target::DrawerHeader(*id);
        }
    }
    if in_rect(layout.input, col, row) {
        return Target::Input;
    }
    if in_rect(layout.conversation, col, row) {
        return Target::Conversation;
    }
    Target::None
}

pub fn route_mouse(state: &ShellState, layout: &ShellLayout, m: MouseEvent) -> Action {
    match m.kind {
        MouseEventKind::ScrollDown => return Action::ScrollConversation(1),
        MouseEventKind::ScrollUp => return Action::ScrollConversation(-1),
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return Action::Noop,
    }
    // While an overlay is up, a click outside it dismisses (scrim behavior).
    if state.overlay != Overlay::None {
        return Action::CloseOverlay;
    }
    match hit_test(layout, m.column, m.row) {
        Target::DrawerHeader(id) => Action::ToggleDrawer(id),
        Target::Input => Action::FocusRegion(Focus::Input),
        Target::Conversation => Action::FocusRegion(Focus::Conversation),
        Target::None => Action::Noop,
    }
}

/// Resolve the palette's selected row to its command (bin calls after PaletteRun).
pub fn palette_selected_command(state: &ShellState) -> Option<Command> {
    let items = all_items(state.mode);
    let matches = selectable_matches(&items, &state.palette.query);
    let sel = nav(state.palette.selected, 0, matches.len());
    matches.get(sel).and_then(|&i| items[i].command.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::compute;
    use crate::state::ShellState;
    use ratatui::layout::Rect;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_c_quits_and_ctrl_p_opens_palette() {
        let s = ShellState::new();
        assert_eq!(route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Quit);
        assert_eq!(route_key(&s, key(KeyCode::Char('p'), KeyModifiers::CONTROL)), Action::OpenPalette);
    }

    #[test]
    fn backtab_switches_mode_tab_cycles_focus() {
        let s = ShellState::new();
        assert_eq!(route_key(&s, key(KeyCode::BackTab, KeyModifiers::NONE)), Action::SwitchMode);
        assert_eq!(route_key(&s, key(KeyCode::Tab, KeyModifiers::NONE)), Action::FocusNext);
    }

    #[test]
    fn input_focus_edits_and_submits() {
        let s = ShellState::new(); // focus Input
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::Submit);
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::ALT)), Action::Newline);
        assert!(matches!(route_key(&s, key(KeyCode::Char('h'), KeyModifiers::NONE)), Action::Edit(_)));
    }

    #[test]
    fn colon_opens_cmdline_only_when_not_input() {
        let mut s = ShellState::new();
        // focus Input → ':' is literal text
        assert!(matches!(route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)), Action::Edit(_)));
        s.focus = Focus::Conversation;
        assert_eq!(route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)), Action::OpenCommandLine);
    }

    #[test]
    fn overlay_captures_keys_first() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        // ^C no longer quits while palette is up — it's a non-char combo → Noop
        assert_eq!(route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Noop);
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::CloseOverlay);
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::PaletteRun);
        assert_eq!(route_key(&s, key(KeyCode::Char('x'), KeyModifiers::NONE)), Action::PaletteChar('x'));
    }

    #[test]
    fn cmdline_enter_parses_command() {
        let mut s = ShellState::new();
        s.overlay = Overlay::CommandLine;
        s.cmdline.buffer = ":build".into();
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::RunCommand(Command::SwitchMode(crate::state::Mode::Build))
        );
    }

    #[test]
    fn hit_test_drawer_header_and_panes() {
        let s = ShellState::new();
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let (id, r) = l.drawer_headers[1]; // files
        assert_eq!(hit_test(&l, r.x, r.y), Target::DrawerHeader(id));
        assert_eq!(hit_test(&l, l.input.x, l.input.y), Target::Input);
        assert_eq!(hit_test(&l, l.conversation.x, l.conversation.y), Target::Conversation);
    }

    #[test]
    fn mouse_click_toggles_drawer_and_focuses() {
        let s = ShellState::new();
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let (id, r) = l.drawer_headers[1];
        let click = |c, row| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: c, row, modifiers: KeyModifiers::NONE };
        assert_eq!(route_mouse(&s, &l, click(r.x, r.y)), Action::ToggleDrawer(id));
        assert_eq!(route_mouse(&s, &l, click(l.input.x, l.input.y)), Action::FocusRegion(Focus::Input));
    }

    #[test]
    fn click_outside_overlay_dismisses() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 23, modifiers: KeyModifiers::NONE };
        assert_eq!(route_mouse(&s, &l, click), Action::CloseOverlay);
    }
}
