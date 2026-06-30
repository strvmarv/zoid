//! The shell's pure interaction state: mode, focus, overlay, and the rail's
//! drawer stack. Terminal-free and unit-tested (spec §13). Rendering, layout,
//! and routing all read from this; the `zoid` bin owns the side effects.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Build,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Conversation,
    Input,
    Rail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    CommandLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerId {
    Economy,
    Files,
    Branch,
    Palette,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawer {
    pub id: DrawerId,
    pub title: String,
    pub keybind: String,
    pub open: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdlineState {
    pub buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellState {
    pub mode: Mode,
    pub focus: Focus,
    pub overlay: Overlay,
    pub drawers: Vec<Drawer>,
    pub rail_visible: bool,
    pub palette: PaletteState,
    pub cmdline: CmdlineState,
    pub conversation_scroll: u16,
    /// cwd entries shown in the Files drawer (populated by the bin; pure for tests).
    pub files: Vec<String>,
    /// Current branch label for the Branch drawer (P2: read from `.git/HEAD`).
    pub branch: String,
}

impl ShellState {
    /// The calm default: Chat mode, focus on the input, the Chat rail set
    /// (economy ⑤ open as in the mockup; files/branch/palette collapsed).
    pub fn new() -> Self {
        let drawers = vec![
            Drawer { id: DrawerId::Economy, title: "⑤ context · tokens".into(), keybind: "^5".into(), open: true },
            Drawer { id: DrawerId::Files,   title: "files".into(),             keybind: "^F".into(), open: false },
            Drawer { id: DrawerId::Branch,  title: "branch".into(),            keybind: "^B".into(), open: false },
            Drawer { id: DrawerId::Palette, title: "palette".into(),           keybind: "^P".into(), open: false },
        ];
        Self {
            mode: Mode::Chat,
            focus: Focus::Input,
            overlay: Overlay::None,
            drawers,
            rail_visible: true,
            palette: PaletteState::default(),
            cmdline: CmdlineState::default(),
            conversation_scroll: 0,
            files: Vec::new(),
            branch: "main".into(),
        }
    }

    pub fn drawer(&self, id: DrawerId) -> Option<&Drawer> {
        self.drawers.iter().find(|d| d.id == id)
    }

    pub fn drawer_mut(&mut self, id: DrawerId) -> Option<&mut Drawer> {
        self.drawers.iter_mut().find(|d| d.id == id)
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_calm_chat_with_chat_rail() {
        let s = ShellState::new();
        assert_eq!(s.mode, Mode::Chat);
        assert_eq!(s.focus, Focus::Input);
        assert_eq!(s.overlay, Overlay::None);
        assert!(s.rail_visible);
        assert_eq!(s.branch, "main");
        // Chat rail set, in order, with the canonical keybinds.
        let ids: Vec<DrawerId> = s.drawers.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![DrawerId::Economy, DrawerId::Files, DrawerId::Branch, DrawerId::Palette]);
        // Economy is the default-open drawer (mockup `drawer on`); rest collapsed.
        assert!(s.drawer(DrawerId::Economy).unwrap().open);
        assert!(!s.drawer(DrawerId::Files).unwrap().open);
    }

    #[test]
    fn drawer_lookup_returns_none_for_absent() {
        let mut s = ShellState::new();
        assert!(s.drawer_mut(DrawerId::Files).is_some());
        s.drawers.clear();
        assert!(s.drawer(DrawerId::Files).is_none());
    }
}
