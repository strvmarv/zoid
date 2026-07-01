//! The app-framework floor: contextual key routing, mouse hit-testing, and the
//! `Action` vocabulary. Pure — synthetic events in, `Action` out (spec §13/§14.1).
//! Precedence: an active overlay captures keys first; then global combos
//! (`^C`/`^P`/`⇧Tab`); then focus-contextual keys (Input edits; Conversation/Rail
//! navigate). `:` opens the command line only when focus ≠ Input.

use crate::command::{parse_command, Command};
use crate::layout::{in_rect, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches};
use crate::state::{DrawerId, Focus, Mode, Overlay, ShellState};
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
    ZoomIn,
    ZoomOut,
    Submit,
    Newline,
    Edit(KeyEvent),
    OpenObjects,
    ObjectMove(i32),
    ObjectPick,
    VerbMove(i32),
    VerbPick,
    /// From the verb picker, step back to the object picker (not fully out).
    VerbBack,
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

/// Alt+char. Only the ALT bit is required (SHIFT may also be set, e.g. Alt+`+`),
/// so we don't test modifier equality. Alt avoids the terminal font-zoom keys
/// (Ctrl/Ctrl+Shift with +/-/=) that never reach the app.
fn alt(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char(c)
}

pub fn route_key(state: &ShellState, key: KeyEvent) -> Action {
    // 1. Overlays capture keys first.
    match state.overlay {
        Overlay::Palette => return route_palette_key(key),
        Overlay::CommandLine => return route_cmdline_key(state, key),
        Overlay::Objects => return route_objects_key(key),
        Overlay::Verbs => return route_verbs_key(key),
        Overlay::None => {}
    }

    // 2. Global combos.
    if ctrl(&key, 'c') {
        return Action::Quit;
    }
    if ctrl(&key, 'p') {
        return Action::OpenPalette;
    }
    if ctrl(&key, 'o') {
        return Action::OpenObjects;
    }
    // Zoom altitude from any focus (mirrors Ctrl+scroll) — a modifier is required
    // because plain =/- are text the message box must keep receiving. Alt, not
    // Ctrl: terminals grab Ctrl/Ctrl+Shift with +/-/= for their own font zoom.
    if alt(&key, '=') || alt(&key, '+') {
        return Action::ZoomIn;
    }
    if alt(&key, '-') || alt(&key, '_') {
        return Action::ZoomOut;
    }
    match key.code {
        KeyCode::BackTab => return Action::SwitchMode,
        KeyCode::Tab => return Action::FocusNext,
        _ => {}
    }

    // Esc returns to Chat from the Build surface (spec §6.2).
    if state.mode == Mode::Build && key.code == KeyCode::Esc {
        return Action::SwitchMode;
    }

    // 3. Focus-contextual.
    match state.focus {
        Focus::Input => match (key.code, key.modifiers) {
            (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) => Action::Newline,
            (KeyCode::Enter, _) => Action::Submit,
            _ => Action::Edit(key),
        },
        Focus::Conversation => match key.code {
            KeyCode::Char('=') | KeyCode::Char('+') => Action::ZoomIn,
            KeyCode::Char('-') | KeyCode::Char('_') => Action::ZoomOut,
            KeyCode::Char(':') => Action::OpenCommandLine,
            KeyCode::Char('j') | KeyCode::Down => Action::ScrollConversation(1),
            KeyCode::Char('k') | KeyCode::Up => Action::ScrollConversation(-1),
            KeyCode::Esc => Action::FocusRegion(Focus::Input),
            _ => Action::Noop,
        },
        Focus::Rail => match key.code {
            // Rail j/k item-nav lands with economy content in P3 — Noop for now.
            KeyCode::Char(':') => Action::OpenCommandLine,
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

fn route_objects_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::ObjectPick,
        KeyCode::Up => Action::ObjectMove(-1),
        KeyCode::Down => Action::ObjectMove(1),
        _ => Action::Noop,
    }
}

fn route_verbs_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::VerbBack, // step back to the object picker, not fully out
        KeyCode::Enter => Action::VerbPick,
        KeyCode::Up => Action::VerbMove(-1),
        KeyCode::Down => Action::VerbMove(1),
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
    // Dismiss modal overlays on any click or scroll.
    if state.overlay != Overlay::None {
        return match m.kind {
            MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp => Action::CloseOverlay,
            _ => Action::Noop,
        };
    }
    match m.kind {
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::CONTROL) => Action::ZoomIn,
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => Action::ZoomOut,
        MouseEventKind::ScrollDown => Action::ScrollConversation(1),
        MouseEventKind::ScrollUp => Action::ScrollConversation(-1),
        MouseEventKind::Down(MouseButton::Left) => match hit_test(layout, m.column, m.row) {
            Target::DrawerHeader(id) => Action::ToggleDrawer(id),
            Target::Input => Action::FocusRegion(Focus::Input),
            Target::Conversation => Action::FocusRegion(Focus::Conversation),
            Target::None => Action::Noop,
        },
        _ => Action::Noop,
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
        // ^C no longer quits while palette is up — the CONTROL guard rejects it from the char arm → Noop
        assert_eq!(route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Noop);
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::CloseOverlay);
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::PaletteRun);
        assert_eq!(route_key(&s, key(KeyCode::Char('x'), KeyModifiers::NONE)), Action::PaletteChar('x'));
        // Same guard applies to CommandLine overlay.
        s.overlay = Overlay::CommandLine;
        assert_eq!(route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Noop);
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
        let (id, r) = *l.drawer_headers.iter().find(|(id, _)| *id == DrawerId::Files).unwrap();
        assert_eq!(hit_test(&l, r.x, r.y), Target::DrawerHeader(id));
        assert_eq!(hit_test(&l, l.input.x, l.input.y), Target::Input);
        assert_eq!(hit_test(&l, l.conversation.x, l.conversation.y), Target::Conversation);
    }

    #[test]
    fn mouse_click_toggles_drawer_and_focuses() {
        let s = ShellState::new();
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let (id, r) = *l.drawer_headers.iter().find(|(id, _)| *id == DrawerId::Files).unwrap();
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

    #[test]
    fn route_mouse_scroll_moves_conversation() {
        let s = ShellState::new();
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let scroll_down = MouseEvent { kind: MouseEventKind::ScrollDown, column: 10, row: 10, modifiers: KeyModifiers::NONE };
        let scroll_up = MouseEvent { kind: MouseEventKind::ScrollUp, column: 10, row: 10, modifiers: KeyModifiers::NONE };
        // No overlay: scroll drives conversation.
        assert_eq!(route_mouse(&s, &l, scroll_down), Action::ScrollConversation(1));
        assert_eq!(route_mouse(&s, &l, scroll_up), Action::ScrollConversation(-1));
        // With overlay up: scroll dismisses instead of leaking through to the conversation.
        let mut s2 = ShellState::new();
        s2.overlay = Overlay::Palette;
        assert_eq!(route_mouse(&s2, &l, scroll_down), Action::CloseOverlay);
    }

    #[test]
    fn palette_selected_command_resolves_highlighted_row() {
        use crate::state::Mode;
        let mut s = ShellState::new(); // mode = Chat
        s.palette.query = "build".into();
        s.palette.selected = 0;
        assert_eq!(
            palette_selected_command(&s),
            Some(Command::SwitchMode(Mode::Build)),
        );
    }

    #[test]
    fn esc_exits_build_mode() {
        use crate::state::Mode;
        let mut s = ShellState::new();
        s.mode = Mode::Build;
        // Esc in Build mode → SwitchMode (back to Chat).
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::SwitchMode);
    }

    #[test]
    fn esc_in_chat_conversation_focus_returns_to_input() {
        let mut s = ShellState::new(); // mode = Chat
        s.focus = Focus::Conversation;
        // Esc in Chat mode with Conversation focus → FocusRegion(Input) (unchanged).
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::FocusRegion(Focus::Input));
    }

    #[test]
    fn zoom_keys_route_in_conversation_focus() {
        let mut s = ShellState::new();
        s.focus = Focus::Conversation;
        assert_eq!(route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)), Action::ZoomIn);
        assert_eq!(route_key(&s, key(KeyCode::Char('+'), KeyModifiers::NONE)), Action::ZoomIn);
        assert_eq!(route_key(&s, key(KeyCode::Char('-'), KeyModifiers::NONE)), Action::ZoomOut);
    }

    #[test]
    fn alt_zoom_routes_from_any_focus() {
        let mut s = ShellState::new();
        s.focus = Focus::Input; // typing in the message box, yet zoom still works
        assert_eq!(route_key(&s, key(KeyCode::Char('='), KeyModifiers::ALT)), Action::ZoomIn);
        assert_eq!(route_key(&s, key(KeyCode::Char('+'), KeyModifiers::ALT)), Action::ZoomIn);
        assert_eq!(route_key(&s, key(KeyCode::Char('-'), KeyModifiers::ALT)), Action::ZoomOut);
        assert_eq!(route_key(&s, key(KeyCode::Char('_'), KeyModifiers::ALT)), Action::ZoomOut);
        // Alt+`+` may also carry SHIFT; still routes to zoom.
        assert_eq!(route_key(&s, key(KeyCode::Char('+'), KeyModifiers::ALT | KeyModifiers::SHIFT)), Action::ZoomIn);
        // Plain =/- in the input are still text, not zoom.
        assert_eq!(route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)), Action::Edit(key(KeyCode::Char('='), KeyModifiers::NONE)));
    }

    #[test]
    fn ctrl_o_opens_object_overlay() {
        let s = ShellState::new();
        assert_eq!(route_key(&s, key(KeyCode::Char('o'), KeyModifiers::CONTROL)), Action::OpenObjects);
    }

    #[test]
    fn object_overlay_navigates_and_picks() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Objects;
        assert_eq!(route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)), Action::ObjectMove(1));
        assert_eq!(route_key(&s, key(KeyCode::Up, KeyModifiers::NONE)), Action::ObjectMove(-1));
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::ObjectPick);
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::CloseOverlay);
    }

    #[test]
    fn verb_overlay_navigates_and_picks() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Verbs;
        assert_eq!(route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)), Action::VerbMove(1));
        assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::VerbPick);
        // Esc steps BACK to the object picker, not all the way out.
        assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::VerbBack);
    }

    #[test]
    fn ctrl_scroll_zooms_plain_scroll_scrolls() {
        let s = ShellState::new();
        let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
        let ev = |kind, mods| MouseEvent { kind, column: 10, row: 10, modifiers: mods };
        // ctrl + scroll → zoom
        assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)), Action::ZoomIn);
        assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollDown, KeyModifiers::CONTROL)), Action::ZoomOut);
        // plain scroll → conversation scroll (unchanged)
        assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollDown, KeyModifiers::NONE)), Action::ScrollConversation(1));
    }
}
