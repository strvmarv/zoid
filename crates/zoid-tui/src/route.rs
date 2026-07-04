//! The app-framework floor: contextual key routing, mouse hit-testing, and the
//! `Action` vocabulary. Pure — synthetic events in, `Action` out (spec §13/§14.1).
//! Precedence: an active overlay captures keys first; then global combos
//! (`^C`/`^P`/`⇧Tab`); then focus-contextual keys (Input edits; Conversation/Rail
//! navigate). `:` opens the command line only when focus ≠ Input.

use crate::command::{parse_command, Command};
use crate::config_view::FieldKind;
use crate::layout::{in_rect, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches};
use crate::state::{ConfigCol, DrawerId, Focus, Mode, Overlay, ShellState};
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
    /// Left-button press landed on the scrollbar at this screen row (begin drag).
    ScrollbarGrab(u16),
    /// Scrollbar drag in progress; the thumb should track this screen row.
    ScrollbarDrag(u16),
    /// Scrollbar drag ended (button released).
    ScrollbarRelease,
    /// A left-click landed in the conversation at this screen row. The bin
    /// focuses the conversation and, if the row falls on a code block, copies it.
    ConversationClick(u16),
    ZoomIn,
    ZoomOut,
    Submit,
    Newline,
    Edit(KeyEvent),
    /// ⇧Delete in the message box: delete the whole current line.
    InputDeleteLine,
    /// ⇧Home in the message box: move the cursor to the very start of the buffer.
    InputCursorTop,
    /// ⇧End in the message box: move the cursor to the very end of the buffer.
    InputCursorBottom,
    /// ⇧Home in the conversation pane: jump the scroll to the top.
    ScrollToTop,
    /// ⇧End in the conversation pane: jump the scroll to the bottom (re-engages follow).
    ScrollToBottom,
    OpenObjects,
    ObjectMove(i32),
    ObjectPick,
    VerbMove(i32),
    VerbPick,
    /// From the verb picker, step back to the object picker (not fully out).
    VerbBack,
    SessionMove(i32),
    SessionPick,
    ConfigMoveField(i32),
    ConfigMoveSection(i32),
    ConfigBeginEdit,
    ConfigEditChar(char),
    ConfigEditBackspace,
    ConfigCommitEdit,
    ConfigCancelEdit,
    ConfigToggle,
    ConfigDrillOpen,
    ConfigPickerMove(i32),
    ConfigPickerSelect,
    ConfigPickerBack,
    ConfigSaveToRepo,
    ConfigClearSecret,
    QuestionMove(i32),
    QuestionSelect,
    QuestionChar(char),
    QuestionBackspace,
    QuestionAbort,
    OpenProviderSwitch,
    SwitchPaneMove(i32),
    SwitchItemMove(i32),
    SwitchApply,
    SwitchCancel,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Conversation,
    Input,
    DrawerHeader(DrawerId),
    Scrollbar,
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
        Overlay::Sessions => return route_sessions_key(key),
        Overlay::Config => return route_config_key(state, key),
        Overlay::Question => {
            return match &state.question {
                Some(q) => crate::question::route_question_key(q, key),
                None => Action::Noop,
            };
        }
        Overlay::ProviderSwitch => return route_provider_switch_key(state, key),
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
    if alt(&key, 'p') {
        return Action::OpenProviderSwitch;
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
            // ⇧⏎ (keyboard-enhancement flags on) or Alt+⏎ (fallback) → newline.
            (KeyCode::Enter, m)
                if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::SHIFT) =>
            {
                Action::Newline
            }
            (KeyCode::Enter, _) => Action::Submit,
            // Editing chords the message box needs beyond tui-textarea's defaults
            // (§30): ⇧Delete drops the whole line, ⇧Home/⇧End jump to the buffer
            // extremes. Guarded on SHIFT so a plain Delete/Home/End still reaches
            // tui-textarea via Edit.
            (KeyCode::Delete, m) if m.contains(KeyModifiers::SHIFT) => Action::InputDeleteLine,
            (KeyCode::Home, m) if m.contains(KeyModifiers::SHIFT) => Action::InputCursorTop,
            (KeyCode::End, m) if m.contains(KeyModifiers::SHIFT) => Action::InputCursorBottom,
            _ => Action::Edit(key),
        },
        Focus::Conversation => match key.code {
            KeyCode::Char('=') | KeyCode::Char('+') => Action::ZoomIn,
            KeyCode::Char('-') | KeyCode::Char('_') => Action::ZoomOut,
            KeyCode::Char(':') => Action::OpenCommandLine,
            KeyCode::Char('j') | KeyCode::Down => Action::ScrollConversation(1),
            KeyCode::Char('k') | KeyCode::Up => Action::ScrollConversation(-1),
            // ⇧Home/⇧End jump the scroll to the top/bottom of the transcript.
            // ⇧Delete has no meaning in a read-only pane → the `_` arm's Noop.
            KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => Action::ScrollToTop,
            KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => Action::ScrollToBottom,
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
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PaletteChar(c)
        }
        _ => Action::Noop,
    }
}

fn route_cmdline_key(state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::RunCommand(parse_command(&state.cmdline.buffer)),
        KeyCode::Backspace => Action::CmdlineBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::CmdlineChar(c)
        }
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

fn route_sessions_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::SessionPick,
        KeyCode::Up | KeyCode::Char('k') => Action::SessionMove(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::SessionMove(1),
        _ => Action::Noop,
    }
}

/// Route keys while the quick-switch (`Alt+P`) overlay is up. Left/Right move
/// between the provider and model panes; Up/Down move the highlighted row
/// within the focused pane; Enter applies; Esc cancels. Task 11 renders the
/// overlay and implements the apply/cancel side effects.
fn route_provider_switch_key(_state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Left | KeyCode::Right => {
            Action::SwitchPaneMove(if key.code == KeyCode::Left { -1 } else { 1 })
        }
        KeyCode::Up => Action::SwitchItemMove(-1),
        KeyCode::Down => Action::SwitchItemMove(1),
        KeyCode::Enter => Action::SwitchApply,
        KeyCode::Esc => Action::SwitchCancel,
        _ => Action::Noop,
    }
}

/// Route keys while the config overlay is up. Precedence:
/// 1. The col-3 picker (open via `ConfigDrillOpen` on a `Pick` field) captures
///    ↑/↓/Enter/←/Esc for movement/select/back while it's open.
/// 2. An in-flight inline text edit buffer (Text/Uint/Secret fields) captures
///    Enter/Esc/Backspace/Char — the same buffer/commit/cancel shape as the
///    palette/cmdline overlays, scoped to a single field.
/// 3. Otherwise, field-list navigation: Up/Down move fields, Tab/Shift+Tab
///    switch sections, and Right/Enter act on the focused field (drill into a
///    `Pick` field's picker, toggle a `Bool`, or begin editing Text/Uint).
fn route_config_key(state: &ShellState, key: KeyEvent) -> Action {
    // 1. Picker column captures keys while a Pick field is drilled open.
    if state.config_col == ConfigCol::Picker && state.config_picker_open() {
        return match key.code {
            KeyCode::Up => Action::ConfigPickerMove(-1),
            KeyCode::Down => Action::ConfigPickerMove(1),
            KeyCode::Enter => Action::ConfigPickerSelect,
            KeyCode::Left | KeyCode::Esc => Action::ConfigPickerBack,
            _ => Action::Noop,
        };
    }

    // 2. Inline text edit buffer (Text/Uint/Secret fields).
    if state.config_edit.is_some() {
        return match key.code {
            KeyCode::Enter => Action::ConfigCommitEdit,
            KeyCode::Esc => Action::ConfigCancelEdit,
            KeyCode::Backspace => Action::ConfigEditBackspace,
            KeyCode::Char(c) => Action::ConfigEditChar(c),
            _ => Action::Noop,
        };
    }

    // 3. Field-list navigation.
    let kind = state
        .config_sections
        .get(state.config_section)
        .and_then(|s| s.rows.get(state.config_field))
        .map(|r| r.kind.clone());

    match key.code {
        KeyCode::Up => Action::ConfigMoveField(-1),
        KeyCode::Down => Action::ConfigMoveField(1),
        KeyCode::Tab => Action::ConfigMoveSection(1),
        KeyCode::BackTab => Action::ConfigMoveSection(-1),
        KeyCode::Esc => Action::CloseOverlay,
        // Right/Enter act on the focused field.
        KeyCode::Right | KeyCode::Enter => match kind {
            Some(FieldKind::Pick) => Action::ConfigDrillOpen,
            Some(FieldKind::Bool) => Action::ConfigToggle,
            Some(FieldKind::Text) | Some(FieldKind::Uint) => Action::ConfigBeginEdit,
            _ => Action::Noop,
        },
        KeyCode::Char('r') => Action::ConfigSaveToRepo,
        KeyCode::Char('x') => {
            if matches!(kind, Some(FieldKind::Secret)) {
                Action::ConfigClearSecret
            } else {
                Action::Noop
            }
        }
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
        // `drawer_headers` now holds the whole box rect (spec: rounded bordered
        // drawers). Only the box's top border row (which carries the title)
        // toggles the drawer — otherwise clicking anywhere in an open drawer's
        // body would collapse it.
        if row == r.y && in_rect(*r, col, row) {
            return Target::DrawerHeader(*id);
        }
    }
    if in_rect(layout.input, col, row) {
        return Target::Input;
    }
    // The scrollbar occupies the rightmost column of the conversation rect.
    if in_rect(layout.conversation, col, row)
        && col == layout.conversation.right().saturating_sub(1)
    {
        return Target::Scrollbar;
    }
    if in_rect(layout.conversation, col, row) {
        return Target::Conversation;
    }
    Target::None
}

pub fn route_mouse(state: &ShellState, layout: &ShellLayout, m: MouseEvent) -> Action {
    // The question overlay is a BLOCKING prompt (the agent turn is suspended
    // awaiting its answer), not a transient palette. A stray scroll/click must
    // NOT silently dismiss it — doing so orphans the reply channel and hangs the
    // turn. Scroll navigates choices; other mouse input is ignored. Dismissal
    // happens only via Enter (select) / Esc (abort).
    if state.overlay == Overlay::Question {
        return match m.kind {
            MouseEventKind::ScrollDown => Action::QuestionMove(1),
            MouseEventKind::ScrollUp => Action::QuestionMove(-1),
            _ => Action::Noop,
        };
    }
    // Other (transient) overlays dismiss on any click or scroll.
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
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::ZoomOut
        }
        MouseEventKind::ScrollDown => Action::ScrollConversation(1),
        MouseEventKind::ScrollUp => Action::ScrollConversation(-1),
        MouseEventKind::Drag(MouseButton::Left) if state.scrollbar_drag => {
            Action::ScrollbarDrag(m.row)
        }
        MouseEventKind::Up(MouseButton::Left) if state.scrollbar_drag => Action::ScrollbarRelease,
        MouseEventKind::Down(MouseButton::Left) => match hit_test(layout, m.column, m.row) {
            Target::DrawerHeader(id) => Action::ToggleDrawer(id),
            Target::Input => Action::FocusRegion(Focus::Input),
            Target::Scrollbar => Action::ScrollbarGrab(m.row),
            Target::Conversation => Action::ConversationClick(m.row),
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

    fn pick_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn state_on_provider() -> ShellState {
        use crate::config_view::{FieldKind, FieldRow, Section};
        use crate::state::Overlay;
        use zoid_core::config::Source;

        let mut s = ShellState::new();
        s.overlay = Overlay::Config;
        s.config_sections = vec![Section {
            title: "Provider & Model".into(),
            rows: vec![FieldRow {
                label: "provider",
                value: "ollama-cloud".into(),
                kind: FieldKind::Pick,
                source: Source::Default,
                env_shadowed: false,
            }],
        }];
        s
    }

    #[test]
    fn enter_on_pick_field_drills_open() {
        let s = state_on_provider();
        let a = route_config_key(&s, pick_key(KeyCode::Enter));
        assert!(matches!(a, Action::ConfigDrillOpen));
    }

    #[test]
    fn picker_open_routes_movement_and_select() {
        use crate::config_view::PickOption;
        use crate::state::ConfigCol;

        let mut s = state_on_provider();
        s.config_col = ConfigCol::Picker;
        s.config_picker = vec![PickOption {
            id: "ollama-local".into(),
            label: "ollama · local".into(),
            detail: String::new(),
            selectable: true,
            is_current: false,
        }];
        assert!(matches!(
            route_config_key(&s, pick_key(KeyCode::Down)),
            Action::ConfigPickerMove(1)
        ));
        assert!(matches!(
            route_config_key(&s, pick_key(KeyCode::Enter)),
            Action::ConfigPickerSelect
        ));
        assert!(matches!(
            route_config_key(&s, pick_key(KeyCode::Esc)),
            Action::ConfigPickerBack
        ));
        assert!(matches!(
            route_config_key(&s, pick_key(KeyCode::Left)),
            Action::ConfigPickerBack
        ));
    }

    #[test]
    fn ctrl_c_quits_and_ctrl_p_opens_palette() {
        let s = ShellState::new();
        assert_eq!(
            route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Action::OpenPalette
        );
    }

    #[test]
    fn backtab_switches_mode_tab_cycles_focus() {
        let s = ShellState::new();
        assert_eq!(
            route_key(&s, key(KeyCode::BackTab, KeyModifiers::NONE)),
            Action::SwitchMode
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Tab, KeyModifiers::NONE)),
            Action::FocusNext
        );
    }

    #[test]
    fn input_focus_edits_and_submits() {
        let s = ShellState::new(); // focus Input
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Submit
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::ALT)),
            Action::Newline
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::SHIFT)),
            Action::Newline
        );
        assert!(matches!(
            route_key(&s, key(KeyCode::Char('h'), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
    }

    #[test]
    fn shift_editing_chords_are_focus_contextual() {
        let mut s = ShellState::new(); // focus Input
                                       // Message box: ⇧Delete deletes the line, ⇧Home/⇧End jump the cursor.
        assert_eq!(
            route_key(&s, key(KeyCode::Delete, KeyModifiers::SHIFT)),
            Action::InputDeleteLine
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Home, KeyModifiers::SHIFT)),
            Action::InputCursorTop
        );
        assert_eq!(
            route_key(&s, key(KeyCode::End, KeyModifiers::SHIFT)),
            Action::InputCursorBottom
        );
        // Without SHIFT they fall through to tui-textarea (Edit).
        assert!(matches!(
            route_key(&s, key(KeyCode::Home, KeyModifiers::NONE)),
            Action::Edit(_)
        ));

        // Conversation pane: ⇧Home/⇧End scroll to the extremes, ⇧Delete is inert.
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Home, KeyModifiers::SHIFT)),
            Action::ScrollToTop
        );
        assert_eq!(
            route_key(&s, key(KeyCode::End, KeyModifiers::SHIFT)),
            Action::ScrollToBottom
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Delete, KeyModifiers::SHIFT)),
            Action::Noop
        );
    }

    #[test]
    fn colon_opens_cmdline_only_when_not_input() {
        let mut s = ShellState::new();
        // focus Input → ':' is literal text
        assert!(matches!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenCommandLine
        );
    }

    #[test]
    fn overlay_captures_keys_first() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        // ^C no longer quits while palette is up — the CONTROL guard rejects it from the char arm → Noop
        assert_eq!(
            route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Noop
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CloseOverlay
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::PaletteRun
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Action::PaletteChar('x')
        );
        // Same guard applies to CommandLine overlay.
        s.overlay = Overlay::CommandLine;
        assert_eq!(
            route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Noop
        );
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
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let (id, r) = *l
            .drawer_headers
            .iter()
            .find(|(id, _)| *id == DrawerId::Session)
            .unwrap();
        assert_eq!(hit_test(&l, r.x, r.y), Target::DrawerHeader(id));
        assert_eq!(hit_test(&l, l.input.x, l.input.y), Target::Input);
        assert_eq!(
            hit_test(&l, l.conversation.x, l.conversation.y),
            Target::Conversation
        );
    }

    #[test]
    fn mouse_click_toggles_drawer_and_focuses() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let (id, r) = *l
            .drawer_headers
            .iter()
            .find(|(id, _)| *id == DrawerId::Session)
            .unwrap();
        let click = |c, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: c,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            route_mouse(&s, &l, click(r.x, r.y)),
            Action::ToggleDrawer(id)
        );
        assert_eq!(
            route_mouse(&s, &l, click(l.input.x, l.input.y)),
            Action::FocusRegion(Focus::Input)
        );
    }

    fn test_layout(w: u16, h: u16) -> ShellLayout {
        compute(
            Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
            &ShellState::new(),
        )
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn hit_test_detects_scrollbar_column() {
        let layout = test_layout(100, 24);
        let conv = layout.conversation;
        let bar_x = conv.right() - 1;
        assert_eq!(hit_test(&layout, bar_x, conv.y + 1), Target::Scrollbar);
        // one column left of the bar is still the conversation
        assert_eq!(
            hit_test(&layout, bar_x - 1, conv.y + 1),
            Target::Conversation
        );
    }

    #[test]
    fn scrollbar_grab_then_drag_then_release() {
        let mut s = ShellState::new();
        let layout = test_layout(100, 24);
        let bar_x = layout.conversation.right() - 1;
        let row = layout.conversation.y + 5;
        // grab on the bar
        let a = route_mouse(
            &s,
            &layout,
            mouse(MouseEventKind::Down(MouseButton::Left), bar_x, row),
        );
        assert!(matches!(a, Action::ScrollbarGrab(r) if r == row));
        // once dragging, a bare Drag(Left) anywhere is a scrollbar drag
        s.scrollbar_drag = true;
        let a = route_mouse(
            &s,
            &layout,
            mouse(MouseEventKind::Drag(MouseButton::Left), 3, row + 2),
        );
        assert!(matches!(a, Action::ScrollbarDrag(r) if r == row + 2));
        // release
        let a = route_mouse(
            &s,
            &layout,
            mouse(MouseEventKind::Up(MouseButton::Left), 3, row),
        );
        assert!(matches!(a, Action::ScrollbarRelease));
    }

    #[test]
    fn drag_without_grab_is_ignored() {
        let s = ShellState::new(); // scrollbar_drag == false
        let layout = test_layout(100, 24);
        let a = route_mouse(
            &s,
            &layout,
            mouse(MouseEventKind::Drag(MouseButton::Left), 3, 5),
        );
        assert_eq!(a, Action::Noop);
    }

    #[test]
    fn click_outside_overlay_dismisses() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 23,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &l, click), Action::CloseOverlay);
    }

    #[test]
    fn question_overlay_is_not_dismissed_by_scroll_or_click() {
        // A blocking ask_user prompt must survive stray scroll/click (which used
        // to CloseOverlay → orphan the reply channel → hang the turn). Scroll
        // navigates choices; a click is ignored.
        let mut s = ShellState::new();
        s.overlay = Overlay::Question;
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &l, scroll_down), Action::QuestionMove(1));
        let scroll_up = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &l, scroll_up), Action::QuestionMove(-1));
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &l, click), Action::Noop);
    }

    #[test]
    fn route_mouse_scroll_moves_conversation() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        let scroll_up = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        // No overlay: scroll drives conversation.
        assert_eq!(
            route_mouse(&s, &l, scroll_down),
            Action::ScrollConversation(1)
        );
        assert_eq!(
            route_mouse(&s, &l, scroll_up),
            Action::ScrollConversation(-1)
        );
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
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::SwitchMode
        );
    }

    #[test]
    fn esc_in_chat_conversation_focus_returns_to_input() {
        let mut s = ShellState::new(); // mode = Chat
        s.focus = Focus::Conversation;
        // Esc in Chat mode with Conversation focus → FocusRegion(Input) (unchanged).
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::FocusRegion(Focus::Input)
        );
    }

    #[test]
    fn conversation_click_carries_row() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: l.conversation.x,
            row: l.conversation.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            route_mouse(&s, &l, click),
            Action::ConversationClick(l.conversation.y)
        );
    }

    #[test]
    fn zoom_keys_route_in_conversation_focus() {
        let mut s = ShellState::new();
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)),
            Action::ZoomIn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('+'), KeyModifiers::NONE)),
            Action::ZoomIn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('-'), KeyModifiers::NONE)),
            Action::ZoomOut
        );
    }

    #[test]
    fn alt_zoom_routes_from_any_focus() {
        let mut s = ShellState::new();
        s.focus = Focus::Input; // typing in the message box, yet zoom still works
        assert_eq!(
            route_key(&s, key(KeyCode::Char('='), KeyModifiers::ALT)),
            Action::ZoomIn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('+'), KeyModifiers::ALT)),
            Action::ZoomIn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('-'), KeyModifiers::ALT)),
            Action::ZoomOut
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('_'), KeyModifiers::ALT)),
            Action::ZoomOut
        );
        // Alt+`+` may also carry SHIFT; still routes to zoom.
        assert_eq!(
            route_key(
                &s,
                key(KeyCode::Char('+'), KeyModifiers::ALT | KeyModifiers::SHIFT)
            ),
            Action::ZoomIn
        );
        // Plain =/- in the input are still text, not zoom.
        assert_eq!(
            route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)),
            Action::Edit(key(KeyCode::Char('='), KeyModifiers::NONE))
        );
    }

    #[test]
    fn alt_p_opens_provider_switch() {
        let s = ShellState::new(); // overlay None, focus Input
        let a = route_key(&s, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert!(matches!(a, Action::OpenProviderSwitch));
    }

    #[test]
    fn ctrl_o_opens_object_overlay() {
        let s = ShellState::new();
        assert_eq!(
            route_key(&s, key(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            Action::OpenObjects
        );
    }

    #[test]
    fn object_overlay_navigates_and_picks() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Objects;
        assert_eq!(
            route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)),
            Action::ObjectMove(1)
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Up, KeyModifiers::NONE)),
            Action::ObjectMove(-1)
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::ObjectPick
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CloseOverlay
        );
    }

    #[test]
    fn verb_overlay_navigates_and_picks() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Verbs;
        assert_eq!(
            route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)),
            Action::VerbMove(1)
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::VerbPick
        );
        // Esc steps BACK to the object picker, not all the way out.
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::VerbBack
        );
    }

    #[test]
    fn config_overlay_nav_and_escape() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Config;
        // Up/Down move between fields; Tab/Shift+Tab switch the section.
        assert!(matches!(
            route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)),
            Action::ConfigMoveField(1)
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Tab, KeyModifiers::NONE)),
            Action::ConfigMoveSection(1)
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::BackTab, KeyModifiers::NONE)),
            Action::ConfigMoveSection(-1)
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CloseOverlay
        ));
    }

    #[test]
    fn config_overlay_toggle_and_edit_buffer() {
        use crate::config_view::{FieldKind, FieldRow, Section};
        use zoid_core::config::Source;

        let mut s = ShellState::new();
        s.overlay = Overlay::Config;
        s.config_sections = vec![Section {
            title: "Interface".into(),
            rows: vec![FieldRow {
                label: "reduced motion",
                value: "off".into(),
                kind: FieldKind::Bool,
                source: Source::Default,
                env_shadowed: false,
            }],
        }];
        s.config_section = 0;
        s.config_field = 0;

        // Bool field: Enter and Right both toggle (act-on-field); Left is inert
        // (only Right/Enter act on the focused field now).
        assert!(matches!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::ConfigToggle
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Right, KeyModifiers::NONE)),
            Action::ConfigToggle
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Left, KeyModifiers::NONE)),
            Action::Noop
        ));

        // Pick field (provider): Right/Enter drill the picker open; Left is inert.
        s.config_sections = vec![Section {
            title: "Provider & Model".into(),
            rows: vec![FieldRow {
                label: "provider",
                value: "ollama".into(),
                kind: FieldKind::Pick,
                source: Source::Default,
                env_shadowed: false,
            }],
        }];
        assert!(matches!(
            route_key(&s, key(KeyCode::Right, KeyModifiers::NONE)),
            Action::ConfigDrillOpen
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::ConfigDrillOpen
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Left, KeyModifiers::NONE)),
            Action::Noop
        ));

        // Once editing, char/esc route into the edit buffer, not navigation.
        s.config_edit = Some(String::new());
        assert!(matches!(
            route_key(&s, key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Action::ConfigEditChar('a')
        ));
        assert!(matches!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::ConfigCancelEdit
        ));
    }

    #[test]
    fn ctrl_scroll_zooms_plain_scroll_scrolls() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 24,
            },
            &s,
        );
        let ev = |kind, mods| MouseEvent {
            kind,
            column: 10,
            row: 10,
            modifiers: mods,
        };
        // ctrl + scroll → zoom
        assert_eq!(
            route_mouse(&s, &l, ev(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)),
            Action::ZoomIn
        );
        assert_eq!(
            route_mouse(
                &s,
                &l,
                ev(MouseEventKind::ScrollDown, KeyModifiers::CONTROL)
            ),
            Action::ZoomOut
        );
        // plain scroll → conversation scroll (unchanged)
        assert_eq!(
            route_mouse(&s, &l, ev(MouseEventKind::ScrollDown, KeyModifiers::NONE)),
            Action::ScrollConversation(1)
        );
    }
}
