//! The app-framework floor: contextual key routing, mouse hit-testing, and the
//! `Action` vocabulary. Pure — synthetic events in, `Action` out (spec §13/§14.1).
//! Precedence: an active overlay captures keys first; then global combos
//! (`^Q`/`^P`/`⇧Tab`); then focus-contextual keys (Input edits; Conversation/Rail
//! navigate). `:` opens the palette in Direct phase only when focus ≠ Input.

use crate::command::Command;
use crate::config_view::FieldKind;
use crate::layout::{in_rect, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches};
use crate::state::{ConfigCol, DrawerId, Focus, Overlay, ShellState};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    CycleMode,
    FocusNext,
    FocusRegion(Focus),
    OpenPalette,
    OpenPaletteDirect,
    CloseOverlay,
    ToggleDrawer(DrawerId),
    PaletteMove(i32),
    PaletteChar(char),
    PaletteBackspace,
    /// Run the palette's currently-selected command (resolved by the bin/state).
    PaletteRun,
    /// Esc while in the palette's Arg (argument-entry) phase: return to the Pick
    /// list without closing the overlay.
    PaletteArgCancel,
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
    /// Esc / Ctrl-C while a turn is in flight: cooperatively cancel it. The bin
    /// fires the turn's cancellation token; a no-op if no cancellable turn.
    CancelTurn,
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
    /// Open the keyboard-shortcuts help overlay (`?` from conversation focus).
    OpenHelp,
    ObjectMove(i32),
    ObjectPick,
    VerbMove(i32),
    VerbPick,
    /// From the verb picker, step back to the object picker (not fully out).
    VerbBack,
    SessionMove(i32),
    SessionPick,
    /// The user pressed Enter on a live ("in use") resume-picker row. Raise a
    /// confirm card before taking it over. Spec §3.2.
    SessionTakeoverConfirm,
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
    FeedbackMoveFocus(i32),
    FeedbackCycleKind(i32),
    FeedbackChar(char),
    FeedbackBackspace,
    FeedbackSubmit,
    FeedbackAbort,
    OpenProviderSwitch,
    SwitchPaneMove(i32),
    SwitchItemMove(i32),
    SwitchApply,
    SwitchCancel,
    /// Scroll the keyboard-shortcuts overlay by N rows (bin clamps the range).
    ScrollHelp(i32),
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

/// Where a bracketed-paste string should be inserted. Bracketed paste arrives
/// as a distinct `Event::Paste(String)` rather than a burst of `Char` keys, so
/// it never passes through `route_key`. `route_paste` gives it the *same*
/// precedence — an open question card and text-bearing overlays capture paste
/// before the message box — so pasting an API key into the config Secret field
/// (or a palette arg, feedback body, …) lands there instead of leaking into the
/// message textarea. Read-only surfaces (Conversation/Rail focus, selection-only
/// overlays, a config field that isn't being edited) return `None` and drop the
/// paste, mirroring how a typed `Char` is a Noop in those contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteTarget {
    Input,
    ConfigEdit,
    PaletteQuery,
    PaletteArg,
    Question,
    FeedbackTitle,
    FeedbackBody,
    None,
}

pub fn route_paste(state: &ShellState) -> PasteTarget {
    // 0. An open question card soft-captures input (mirrors route_key step 0).
    if state.question.is_some() {
        return PasteTarget::Question;
    }
    // 1. Text-bearing overlays capture paste before the focus region.
    match state.overlay {
        Overlay::Palette => {
            return if matches!(state.palette.stage, crate::state::PaletteStage::Arg { .. }) {
                PasteTarget::PaletteArg
            } else {
                PasteTarget::PaletteQuery
            };
        }
        // Only while an inline edit buffer is active (Text/Uint/Secret field).
        Overlay::Config => {
            return if state.config_edit.is_some() {
                PasteTarget::ConfigEdit
            } else {
                PasteTarget::None
            };
        }
        Overlay::Feedback => {
            return match state.feedback.as_ref().map(|f| f.focus) {
                Some(crate::state::FeedbackField::Title) => PasteTarget::FeedbackTitle,
                Some(crate::state::FeedbackField::Body) => PasteTarget::FeedbackBody,
                // Kind is a selection field with no text buffer.
                _ => PasteTarget::None,
            };
        }
        // Selection-only overlays have nowhere to put pasted text.
        Overlay::Objects
        | Overlay::Verbs
        | Overlay::Sessions
        | Overlay::Mcp
        | Overlay::Help
        | Overlay::ProviderSwitch => return PasteTarget::None,
        Overlay::None => {}
    }
    // 2. Focus-contextual: only the message box accepts text.
    match state.focus {
        Focus::Input => PasteTarget::Input,
        Focus::Conversation | Focus::Rail => PasteTarget::None,
    }
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
    // 0. An open inline question card captures input (soft-capture): while
    // state.question is Some, typing goes to the card's free-text buffer,
    // arrows move the highlight, Enter submits, Esc cancels. The message
    // textarea is not focused during a question.
    if let Some(q) = &state.question {
        return crate::question::route_question_key(q, key);
    }

    // 1. Overlays capture keys first.
    match state.overlay {
        Overlay::Palette => return route_palette_key(state, key),
        Overlay::Objects => return route_objects_key(key),
        Overlay::Verbs => return route_verbs_key(key),
        Overlay::Sessions => return route_sessions_key(state, key),
        Overlay::Config => return route_config_key(state, key),
        Overlay::ProviderSwitch => return route_provider_switch_key(state, key),
        Overlay::Mcp => return route_mcp_key(state, key),
        Overlay::Help => return route_help_key(state, key),
        Overlay::Feedback => {
            if let Some(fs) = &state.feedback {
                return crate::feedback_view::route_feedback_key(fs, key);
            }
            return Action::Noop;
        }
        Overlay::None => {}
    }

    // 2. Global combos.
    // While a cancellable chat turn is in flight, Esc or Ctrl-C requests
    // cancellation (the bin fires the turn's token; the agent loop drains
    // pending tool calls and ends the turn). Gated on `cancellable`, NOT `busy`:
    // a subagent delegation has no token, so during one Esc/Ctrl-C keep their
    // normal focus behavior instead of a silent no-op. Checked before Ctrl-Q so
    // a mid-turn interrupt never quits by accident.
    if state.cancellable && (key.code == KeyCode::Esc || ctrl(&key, 'c')) {
        return Action::CancelTurn;
    }
    if ctrl(&key, 'q') {
        return Action::Quit;
    }
    if ctrl(&key, 'p') {
        return Action::OpenPalette;
    }
    if ctrl(&key, 'o') {
        return Action::OpenObjects;
    }
    // Zoom altitude from any focus (mirrors Ctrl+scroll) — a modifier is required
    // because plain arrow keys navigate the conversation/input. Alt, not Ctrl:
    // terminals grab Ctrl with +/-/= for their own font zoom. Alt+Left zooms
    // out (wider view), Alt+Right zooms in (narrower view).
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Right {
        return Action::ZoomIn;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Left {
        return Action::ZoomOut;
    }
    if alt(&key, 'p') {
        return Action::OpenProviderSwitch;
    }
    match key.code {
        KeyCode::BackTab => return Action::CycleMode,
        KeyCode::Tab => return Action::FocusNext,
        _ => {}
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
            // A leading `:` typed into an empty message box opens the palette in
            // direct-command mode — the same UX as `:` from Conversation/Rail
            // focus — instead of inserting a literal colon. Once the buffer has
            // any text, `:` is literal again (mid-sentence colons, URLs, …).
            (KeyCode::Char(':'), KeyModifiers::NONE) if state.input_empty => {
                Action::OpenPaletteDirect
            }
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
            KeyCode::Char(':') => Action::OpenPaletteDirect,
            KeyCode::Char('j') | KeyCode::Down => Action::ScrollConversation(1),
            KeyCode::Char('k') | KeyCode::Up => Action::ScrollConversation(-1),
            // ⇧Home/⇧End jump the scroll to the top/bottom of the transcript.
            // ⇧Delete has no meaning in a read-only pane → the `_` arm's Noop.
            KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => Action::ScrollToTop,
            KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => Action::ScrollToBottom,
            KeyCode::Esc => Action::FocusRegion(Focus::Input),
            KeyCode::Char('?') => Action::OpenHelp,
            _ => Action::Noop,
        },
        Focus::Rail => match key.code {
            // Rail j/k item-nav lands with economy content in P3 — Noop for now.
            KeyCode::Char(':') => Action::OpenPaletteDirect,
            KeyCode::Esc => Action::FocusRegion(Focus::Input),
            _ => Action::Noop,
        },
    }
}

fn route_palette_key(state: &ShellState, key: KeyEvent) -> Action {
    use crate::palette::{direct_filter, direct_items, selectable_matches};
    let in_arg = matches!(state.palette.stage, crate::state::PaletteStage::Arg { .. });
    let in_direct = !in_arg && state.palette.query.starts_with(':');
    let direct_list_nonempty = in_direct && {
        let items = direct_items(state);
        !selectable_matches(&items, direct_filter(&state.palette.query)).is_empty()
    };
    match key.code {
        // Esc: in Arg phase return to the Pick list; otherwise close.
        KeyCode::Esc if in_arg => Action::PaletteArgCancel,
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::PaletteRun,
        // Selection nav applies to Pick, and to Direct when its filtered list is non-empty.
        KeyCode::Up if !in_arg && (!in_direct || direct_list_nonempty) => Action::PaletteMove(-1),
        KeyCode::Down if !in_arg && (!in_direct || direct_list_nonempty) => Action::PaletteMove(1),
        KeyCode::Backspace => Action::PaletteBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PaletteChar(c)
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

fn route_sessions_key(state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => {
            // If the highlighted row is live, raise the takeover confirm card
            // instead of resuming directly. Spec §3.2.
            let live = state
                .sessions_live
                .get(state.session_selected)
                .copied()
                .unwrap_or(false);
            if live {
                Action::SessionTakeoverConfirm
            } else {
                Action::SessionPick
            }
        }
        KeyCode::Up | KeyCode::Char('k') => Action::SessionMove(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::SessionMove(1),
        _ => Action::Noop,
    }
}

/// Route keys while the read-only `/mcp` server status overlay is up. The
/// overlay has no navigation or actions, just a close: Esc or `q` closes it,
/// everything else is a no-op.
fn route_mcp_key(_state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::CloseOverlay,
        _ => Action::Noop,
    }
}

/// Route keys while the read-only keyboard-shortcuts overlay is up. Esc or `q`
/// close it; Up/Down/j/k and PageUp/PageDown scroll (the bin clamps the range).
fn route_help_key(_state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::CloseOverlay,
        KeyCode::Down | KeyCode::Char('j') => Action::ScrollHelp(1),
        KeyCode::Up | KeyCode::Char('k') => Action::ScrollHelp(-1),
        KeyCode::PageDown => Action::ScrollHelp(10),
        KeyCode::PageUp => Action::ScrollHelp(-10),
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
///    palette overlay (Pick/Arg phases), scoped to a single field.
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
            Some(FieldKind::Text) | Some(FieldKind::Uint) | Some(FieldKind::Secret) => {
                Action::ConfigBeginEdit
            }
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
    // An open inline question card does NOT capture mouse input: scroll still
    // navigates the conversation, scrollbar drag still works, so the user can
    // review the context above the card while answering. Choice navigation is
    // keyboard-only (↑↓), not mouse-scroll.
    // Overlays are keyboard-driven. A stray mouse click or scroll outside the
    // overlay must NOT silently dismiss it — accidental clicks are common, and
    // losing an in-progress edit buffer, query, or selection position is
    // jarring. Dismissal happens only via Esc (every overlay) or Enter (pickers
    // that commit). Mouse input is a Noop while any overlay is up.
    if state.overlay != Overlay::None {
        return match m.kind {
            MouseEventKind::ScrollDown => Action::ScrollConversation(1),
            MouseEventKind::ScrollUp => Action::ScrollConversation(-1),
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
/// `None` means no row matched the current query.
pub fn palette_selected_command(state: &ShellState) -> Option<Command> {
    let items = all_items(&state.active_mode, &state.mode_names, state.companion_on);
    let matches = selectable_matches(&items, &state.palette.query);
    let sel = nav(state.palette.selected, 0, matches.len());
    matches.get(sel).map(|&i| items[i].command.clone())
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
                secret_key: None,
            }],
        }];
        s
    }

    // ---- route_paste: bracketed paste follows the same precedence as keys ----

    #[test]
    fn paste_defaults_to_the_message_box() {
        let s = ShellState::new(); // overlay None, focus Input
        assert_eq!(route_paste(&s), PasteTarget::Input);
    }

    #[test]
    fn paste_targets_config_edit_buffer_not_the_message_box() {
        // Regression: editing a Secret/Text field in the config overlay, a paste
        // (e.g. an API key) must land in config_edit, not the message textarea.
        let mut s = state_on_provider();
        s.config_edit = Some(String::new());
        assert_eq!(route_paste(&s), PasteTarget::ConfigEdit);
    }

    #[test]
    fn paste_in_config_without_active_edit_is_dropped() {
        // Field-list navigation (no edit buffer) has no text target — like a
        // typed Char, which is a Noop there.
        let s = state_on_provider();
        assert_eq!(route_paste(&s), PasteTarget::None);
    }

    #[test]
    fn paste_targets_question_card_free_text() {
        use crate::question::QuestionState;
        let mut s = ShellState::new();
        s.question = Some(QuestionState::new("pick?", vec![]));
        assert_eq!(route_paste(&s), PasteTarget::Question);
    }

    #[test]
    fn paste_targets_palette_query_and_arg() {
        use crate::state::PaletteStage;
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        assert_eq!(route_paste(&s), PasteTarget::PaletteQuery);
        s.palette.stage = PaletteStage::Arg {
            kind: crate::palette::ArgKind::Rename,
            input: String::new(),
        };
        assert_eq!(route_paste(&s), PasteTarget::PaletteArg);
    }

    #[test]
    fn paste_targets_feedback_text_fields_only() {
        use crate::state::{FeedbackField, FeedbackState};
        let mut s = ShellState::new();
        s.overlay = Overlay::Feedback;
        let mut fs = FeedbackState {
            focus: FeedbackField::Body,
            kind: zoid_core::feedback::FeedbackKind::Bug,
            kind_selected: 0,
            title: String::new(),
            body: String::new(),
            status: crate::state::FeedbackStatus::Idle,
        };
        s.feedback = Some(fs.clone());
        assert_eq!(route_paste(&s), PasteTarget::FeedbackBody);
        fs.focus = FeedbackField::Title;
        s.feedback = Some(fs.clone());
        assert_eq!(route_paste(&s), PasteTarget::FeedbackTitle);
        fs.focus = FeedbackField::Kind; // selection field: no text target
        s.feedback = Some(fs);
        assert_eq!(route_paste(&s), PasteTarget::None);
    }

    #[test]
    fn paste_while_conversation_focused_is_dropped() {
        let mut s = ShellState::new();
        s.focus = Focus::Conversation;
        assert_eq!(route_paste(&s), PasteTarget::None);
    }

    #[test]
    fn question_mark_opens_help_only_from_conversation() {
        let q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        let mut s = ShellState::new();
        s.focus = Focus::Conversation;
        assert_eq!(route_key(&s, q), Action::OpenHelp);
        s.focus = Focus::Input;
        assert!(matches!(route_key(&s, q), Action::Edit(_)));
    }

    #[test]
    fn esc_or_ctrl_c_while_cancellable_cancels_turn() {
        let mut s = ShellState::new();
        s.cancellable = true;
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CancelTurn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::CancelTurn
        );
    }

    #[test]
    fn esc_or_ctrl_c_when_idle_does_not_cancel() {
        let s = ShellState::new(); // cancellable = false
        assert_ne!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CancelTurn
        );
        assert_ne!(
            route_key(&s, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::CancelTurn
        );
    }

    #[test]
    fn busy_delegation_without_a_token_keeps_normal_esc_behavior() {
        // A subagent delegation sets `busy` but NOT `cancellable` (no token).
        // Esc must fall through to focus behavior, not become a silent no-op.
        let mut s = ShellState::new();
        s.busy = true;
        s.cancellable = false;
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::FocusRegion(Focus::Input)
        );
    }

    #[test]
    fn cancel_does_not_pre_empt_an_open_overlay() {
        // ask_user (Question) and the pickers have their own Esc handling; the
        // cancel intercept must not fire while an overlay is captured (overlay
        // routing runs first, in section 1).
        let mut s = ShellState::new();
        s.cancellable = true;
        s.overlay = crate::state::Overlay::Palette;
        assert_ne!(
            route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::CancelTurn
        );
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
    fn ctrl_q_quits_and_ctrl_p_opens_palette() {
        let s = ShellState::new();
        assert_eq!(
            route_key(&s, key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
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
            Action::CycleMode
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
    fn colon_opens_palette_direct_when_not_input() {
        let mut s = ShellState::new();
        // focus Input with a non-empty buffer → ':' is literal text.
        s.input_empty = false;
        assert!(matches!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
        // focus Input with an empty buffer → ':' opens the palette (direct).
        s.input_empty = true;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
        s.focus = Focus::Rail;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
    }

    #[test]
    fn input_focus_colon_is_literal_once_buffer_has_text() {
        let mut s = ShellState::new(); // focus Input, input_empty defaults true
                                       // Empty box → opens the palette.
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
        // Once the buffer has any text, ':' is a literal edit again — mid-sentence
        // colons, `:shrug:`, pasted URLs, … must not grab focus.
        s.input_empty = false;
        assert!(matches!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
        // A control-modified ':' (Ctrl+:) never triggers — falls through to Edit.
        assert!(matches!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::CONTROL)),
            Action::Edit(_)
        ));
    }

    #[test]
    fn palette_pick_phase_routing() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette; // stage defaults to Pick
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::PaletteMove(-1));
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::PaletteMove(1));
        assert_eq!(
            route_key(&s, k(KeyCode::Backspace)),
            Action::PaletteBackspace
        );
        assert_eq!(
            route_key(&s, k(KeyCode::Char('r'))),
            Action::PaletteChar('r')
        );
    }

    #[test]
    fn palette_direct_phase_routing() {
        let mut s = ShellState::new();
        s.mode_names = vec!["Chat".into(), "Build".into()];
        s.overlay = Overlay::Palette;

        // Direct with a non-empty list (`:mode ` → Stage 2 subcommands + mode
        // names) → arrows navigate.
        s.palette.query = ":mode ".into();
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::PaletteMove(-1));
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::PaletteMove(1));
        assert_eq!(
            route_key(&s, k(KeyCode::Char('x'))),
            Action::PaletteChar('x')
        );
        assert_eq!(
            route_key(&s, k(KeyCode::Backspace)),
            Action::PaletteBackspace
        );

        // Direct with an empty list (`:wat` → no fuzzy match) → arrows inert.
        s.palette.query = ":wat".into();
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::Noop);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::Noop);
    }

    #[test]
    fn palette_arg_phase_routing() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.stage = zoid_tui_stage_arg(); // helper below
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Esc goes BACK to Pick, not close.
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::PaletteArgCancel);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        // Arrows are inert in Arg (no list to move).
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::Noop);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::Noop);
        // Char/Backspace still edit (the bin routes them to `input` in Arg phase).
        assert_eq!(
            route_key(&s, k(KeyCode::Char('x'))),
            Action::PaletteChar('x')
        );
        assert_eq!(
            route_key(&s, k(KeyCode::Backspace)),
            Action::PaletteBackspace
        );
    }

    fn zoid_tui_stage_arg() -> crate::state::PaletteStage {
        crate::state::PaletteStage::Arg {
            kind: crate::palette::ArgKind::Rename,
            input: String::new(),
        }
    }

    #[test]
    fn sessions_live_row_enter_raises_confirm_not_pick() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a".into(), "b".into()];
        s.sessions_live = vec![false, true];
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Non-live row → direct pick.
        s.session_selected = 0;
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::SessionPick);
        // Live row → confirm card.
        s.session_selected = 1;
        assert_eq!(
            route_key(&s, k(KeyCode::Enter)),
            Action::SessionTakeoverConfirm
        );
    }

    #[test]
    fn overlay_captures_keys_first() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        // ^Q no longer quits while palette is up — the CONTROL guard rejects it from the char arm → Noop
        assert_eq!(
            route_key(&s, key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
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
    }

    #[test]
    fn hit_test_drawer_header_and_panes() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 160,
                height: 40,
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
                width: 160,
                height: 40,
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
        let layout = test_layout(160, 40);
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
        let layout = test_layout(160, 40);
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
    fn click_outside_overlay_is_ignored() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 160,
                height: 40,
            },
            &s,
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 23,
            modifiers: KeyModifiers::NONE,
        };
        // Overlays are keyboard-driven — accidental clicks must NOT dismiss.
        assert_eq!(route_mouse(&s, &l, click), Action::Noop);
    }

    #[test]
    fn route_mouse_scroll_moves_conversation() {
        let s = ShellState::new();
        let l = compute(
            Rect {
                x: 0,
                y: 0,
                width: 160,
                height: 40,
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
        // With overlay up: scroll still drives the conversation (doesn't dismiss).
        let mut s2 = ShellState::new();
        s2.overlay = Overlay::Palette;
        assert_eq!(
            route_mouse(&s2, &l, scroll_down),
            Action::ScrollConversation(1)
        );
    }

    #[test]
    fn palette_selected_command_resolves_highlighted_row() {
        let mut s = ShellState::new(); // active_mode = Chat
        s.mode_names = vec!["Chat".into(), "Build".into()];
        s.palette.query = "build".into();
        s.palette.selected = 0;
        assert_eq!(
            palette_selected_command(&s),
            Some(Command::SwitchMode("Build".into())),
        );
        // The companion row resolves to the state-appropriate command.
        s.palette.query = "companion".into();
        s.companion_on = false;
        assert_eq!(palette_selected_command(&s), Some(Command::CompanionEnable),);
        s.companion_on = true;
        assert_eq!(
            palette_selected_command(&s),
            Some(Command::CompanionDisable),
        );
    }

    #[test]
    fn esc_in_chat_conversation_focus_returns_to_input() {
        let mut s = ShellState::new(); // active_mode = Chat
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
                width: 160,
                height: 40,
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
            route_key(&s, key(KeyCode::Right, KeyModifiers::ALT)),
            Action::ZoomIn
        );
        assert_eq!(
            route_key(&s, key(KeyCode::Left, KeyModifiers::ALT)),
            Action::ZoomOut
        );
        // Alt+Right may also carry SHIFT; still routes to zoom.
        assert_eq!(
            route_key(
                &s,
                key(KeyCode::Right, KeyModifiers::ALT | KeyModifiers::SHIFT)
            ),
            Action::ZoomIn
        );
        // Plain arrow keys in the input are still text/navigation, not zoom.
        assert!(matches!(
            route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
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
                secret_key: None,
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
                secret_key: None,
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
                width: 160,
                height: 40,
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

    #[test]
    fn esc_closes_the_mcp_overlay() {
        use crate::state::{Overlay, ShellState};
        let mut s = ShellState::new();
        s.overlay = Overlay::Mcp;
        let action = route_mcp_key(&s, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, Action::CloseOverlay));
    }

    #[test]
    fn help_overlay_close_and_scroll_route() {
        use crate::state::{Overlay, ShellState};
        let mut s = ShellState::new();
        s.overlay = Overlay::Help;
        let k = |c| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Char('q'))), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::ScrollHelp(1));
        assert_eq!(route_key(&s, k(KeyCode::Char('k'))), Action::ScrollHelp(-1));
    }
}
