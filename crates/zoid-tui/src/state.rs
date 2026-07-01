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

/// Conversation altitude (spec ① semantic zoom). `Normal` is the default
/// turn-by-turn view; `Summary` collapses each turn to a one-line digest;
/// `Detail` expands tool output with code highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Summary,
    Normal,
    Detail,
}

impl Zoom {
    /// Short altitude name for the status-bar indicator.
    pub fn label(self) -> &'static str {
        match self {
            Zoom::Summary => "summary",
            Zoom::Normal => "normal",
            Zoom::Detail => "detail",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    CommandLine,
    Objects,
    Verbs,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerId {
    Repo,
    Session,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawer {
    pub id: DrawerId,
    pub title: String,
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

/// Object-first picker state (spec ④): which object/verb row is highlighted
/// across the two-step Objects → Verbs overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectState {
    pub obj_selected: usize,
    pub verb_selected: usize,
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
    pub objects: ObjectState,
    pub conversation_scroll: u16,
    /// cwd entries shown in the Files drawer (populated by the bin; pure for tests).
    pub files: Vec<String>,
    /// Current branch label for the Branch drawer (P2: read from `.git/HEAD`).
    pub branch: String,
    /// Repo directory name shown in the repo drawer header line.
    pub repo_name: String,
    /// Worktree label for the repo drawer ("(none)" when not in a linked worktree).
    pub worktree: String,
    /// Working-tree changed-line counts (unstaged + staged) for the repo drawer's
    /// changes line, refreshed on an `Instant` cadence by the bin.
    pub changes_added: usize,
    pub changes_removed: usize,
    pub changes_files: usize,
    /// Reduced-motion accessibility setting (spec §13). When true, animations
    /// resolve to their final state instantly. Bin sets it from ZOID_REDUCED_MOTION.
    pub reduced_motion: bool,
    /// Conversation altitude (spec ① semantic zoom).
    pub zoom: Zoom,
    /// Transient one-line hint shown in the status bar (e.g. the ④ "queued · P5"
    /// notice). Lives on `ShellState` (not `App`) so the pure renderer can read
    /// it directly. Setting/clearing it on a verb pick is bin wiring (P4d T4).
    pub status_hint: Option<String>,
    /// Line count of the message input, sampled by the bin each frame so
    /// `layout::compute` can grow/shrink the box (spec §2.2). Default 1 (resting).
    pub input_rows: u16,
    /// Display rows for the resume-session picker (bin-formatted, most-recent-first).
    pub sessions: Vec<String>,
    /// Highlighted row in the resume-session picker.
    pub session_selected: usize,
    /// Session name shown in the session drawer header line.
    pub session_name: String,
    /// Active model id shown in the session drawer.
    pub model: String,
    /// Human provider label (e.g. "anthropic", "ollama") shown beside the model.
    pub provider: String,
    /// Compact elapsed-time-in-session label (e.g. "12m", "1h3m").
    pub duration: String,
    /// Total tokens spent in the active session (session drawer "tok" line).
    pub session_tokens: u64,
    /// Current context-window token usage (session drawer "ctx" line).
    pub ctx_used: u64,
    /// Context-window ceiling in tokens (session drawer "ctx" line denominator).
    pub ctx_ceiling: u64,
    /// Current working directory shown (truncated) in the session drawer.
    pub cwd: String,
}

impl ShellState {
    /// The calm default: Chat mode, focus on the input, the Chat rail set
    /// (repo/session/context all open, matching `docs/ux/chat-mode.html`).
    pub fn new() -> Self {
        use crate::tokens::glyph;
        let drawers = vec![
            Drawer { id: DrawerId::Repo,    title: "repo".into(),    open: true },
            Drawer { id: DrawerId::Session, title: "session".into(), open: true },
            Drawer { id: DrawerId::Context, title: format!("{} context · tokens", glyph::CONTEXT), open: true },
        ];
        Self {
            mode: Mode::Chat,
            focus: Focus::Input,
            overlay: Overlay::None,
            drawers,
            rail_visible: true,
            palette: PaletteState::default(),
            cmdline: CmdlineState::default(),
            objects: ObjectState::default(),
            conversation_scroll: 0,
            files: Vec::new(),
            branch: "main".into(),
            repo_name: String::new(),
            worktree: "(none)".into(),
            changes_added: 0,
            changes_removed: 0,
            changes_files: 0,
            reduced_motion: false,
            zoom: Zoom::Normal,
            status_hint: None,
            input_rows: 1,
            sessions: Vec::new(),
            session_selected: 0,
            session_name: String::new(),
            model: String::new(),
            provider: String::new(),
            duration: "0m".into(),
            session_tokens: 0,
            ctx_used: 0,
            ctx_ceiling: 0,
            cwd: String::new(),
        }
    }

    pub fn drawer(&self, id: DrawerId) -> Option<&Drawer> {
        self.drawers.iter().find(|d| d.id == id)
    }

    pub fn drawer_mut(&mut self, id: DrawerId) -> Option<&mut Drawer> {
        self.drawers.iter_mut().find(|d| d.id == id)
    }

    /// The focus ring (forward only; `⇧Tab` is mode-switch, not focus-prev — spec §6.2).
    /// Rail participates only when visible.
    pub fn focus_next(&mut self) {
        let ring: &[Focus] = if self.rail_visible {
            &[Focus::Conversation, Focus::Input, Focus::Rail]
        } else {
            &[Focus::Conversation, Focus::Input]
        };
        let i = ring.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = ring[(i + 1) % ring.len()];
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Chat => Mode::Build,
            Mode::Build => Mode::Chat,
        };
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn toggle_drawer(&mut self, id: DrawerId) {
        if let Some(d) = self.drawer_mut(id) {
            d.open = !d.open;
        }
    }

    /// Force a drawer open and reveal the rail (used by palette/command actions).
    pub fn open_drawer(&mut self, id: DrawerId) {
        self.rail_visible = true;
        if let Some(d) = self.drawer_mut(id) {
            d.open = true;
        }
    }

    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
        self.palette = PaletteState::default();
        self.cmdline = CmdlineState::default();
        self.objects = ObjectState::default();
        self.sessions.clear();
        self.session_selected = 0;
    }

    /// Increase detail (Summary → Normal → Detail), saturating.
    pub fn zoom_in(&mut self) {
        self.zoom = match self.zoom {
            Zoom::Summary => Zoom::Normal,
            Zoom::Normal | Zoom::Detail => Zoom::Detail,
        };
    }

    /// Decrease detail (Detail → Normal → Summary), saturating.
    pub fn zoom_out(&mut self) {
        self.zoom = match self.zoom {
            Zoom::Detail => Zoom::Normal,
            Zoom::Normal | Zoom::Summary => Zoom::Summary,
        };
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
    fn new_is_calm_chat_with_repo_session_context_rail() {
        let s = ShellState::new();
        assert_eq!(s.mode, Mode::Chat);
        assert!(s.rail_visible);
        assert_eq!(s.branch, "main");
        let ids: Vec<DrawerId> = s.drawers.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![DrawerId::Repo, DrawerId::Session, DrawerId::Context]);
        // All three expanded (mockup shows repo/session/context all `on`).
        assert!(s.drawer(DrawerId::Repo).unwrap().open);
        assert!(s.drawer(DrawerId::Session).unwrap().open);
        assert!(s.drawer(DrawerId::Context).unwrap().open);
    }

    #[test]
    fn drawer_lookup_returns_none_for_absent() {
        let mut s = ShellState::new();
        assert!(s.drawer_mut(DrawerId::Session).is_some());
        s.drawers.clear();
        assert!(s.drawer(DrawerId::Session).is_none());
    }

    #[test]
    fn focus_next_cycles_and_wraps() {
        let mut s = ShellState::new();
        s.focus = Focus::Conversation;
        s.focus_next();
        assert_eq!(s.focus, Focus::Input);
        s.focus_next();
        assert_eq!(s.focus, Focus::Rail);
        s.focus_next();
        assert_eq!(s.focus, Focus::Conversation); // wraps
    }

    #[test]
    fn focus_next_skips_rail_when_hidden() {
        let mut s = ShellState::new();
        s.rail_visible = false;
        s.focus = Focus::Input;
        s.focus_next();
        assert_eq!(s.focus, Focus::Conversation); // Rail not in the ring
    }

    #[test]
    fn toggle_mode_flips_chat_build() {
        let mut s = ShellState::new();
        s.toggle_mode();
        assert_eq!(s.mode, Mode::Build);
        s.toggle_mode();
        assert_eq!(s.mode, Mode::Chat);
    }

    #[test]
    fn toggle_drawer_flips_open_and_opens_rail() {
        let mut s = ShellState::new();
        s.toggle_drawer(DrawerId::Session);
        assert!(!s.drawer(DrawerId::Session).unwrap().open); // was open by default; toggled closed
        // open_drawer forces open (idempotent) and ensures the rail is visible.
        s.rail_visible = false;
        s.open_drawer(DrawerId::Repo);
        assert!(s.rail_visible);
        assert!(s.drawer(DrawerId::Repo).unwrap().open);
    }

    #[test]
    fn new_has_reduced_motion_off_by_default() {
        let s = ShellState::new();
        assert!(!s.reduced_motion); // motion on by default; bin flips it from env
    }

    #[test]
    fn close_overlay_resets_palette_query() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.query = "comp".into();
        s.palette.selected = 3;
        s.close_overlay();
        assert_eq!(s.overlay, Overlay::None);
        assert_eq!(s.palette, PaletteState::default());
        assert_eq!(s.cmdline, CmdlineState::default());
    }

    #[test]
    fn close_overlay_resets_object_state() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Verbs;
        s.objects.obj_selected = 2;
        s.objects.verb_selected = 1;
        s.close_overlay();
        assert_eq!(s.overlay, Overlay::None);
        assert_eq!(s.objects, ObjectState::default());
    }

    #[test]
    fn new_has_no_status_hint() {
        assert!(ShellState::new().status_hint.is_none());
    }

    #[test]
    fn zoom_defaults_to_normal() {
        assert_eq!(ShellState::new().zoom, Zoom::Normal);
    }

    #[test]
    fn zoom_in_out_saturate_at_ends() {
        let mut s = ShellState::new(); // Normal
        s.zoom_out();
        assert_eq!(s.zoom, Zoom::Summary);
        s.zoom_out();
        assert_eq!(s.zoom, Zoom::Summary); // saturates
        s.zoom_in();
        assert_eq!(s.zoom, Zoom::Normal);
        s.zoom_in();
        assert_eq!(s.zoom, Zoom::Detail);
        s.zoom_in();
        assert_eq!(s.zoom, Zoom::Detail); // saturates
    }
}
