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

/// Which column has focus inside the config overlay. Sections are switched with
/// Tab (not a focusable column); focus moves between the field list and the
/// contextual picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCol {
    Fields,
    Picker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    CommandLine,
    Objects,
    Verbs,
    Sessions,
    Config,
    Question,
    ProviderSwitch,
}

/// Which pane has focus inside the quick-switch (`Alt+P`) overlay: the
/// provider list or the model list (Task 11 renders/populates it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchPane {
    Provider,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerId {
    Repo,
    Session,
    Context,
    Tasks,
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
    /// Tail-follow: when true the bin pins `conversation_scroll` to the last
    /// line every frame, so the view shows the latest output on startup and as
    /// new events stream in. Scrolling up detaches it; scrolling back to the
    /// bottom re-engages it.
    pub follow_tail: bool,
    /// True while the user is dragging the scrollbar thumb. Cross-event memory so
    /// the pure `route_mouse` can classify bare `Drag(Left)` events as scrollbar
    /// drags. Set on grab, cleared on release.
    pub scrollbar_drag: bool,
    /// Whether a turn is in flight (streaming or delegating). Refreshed by the
    /// bin each frame; the status bar shows an animated spinner while true and
    /// the static idle glyph otherwise.
    pub busy: bool,
    /// Current activity-spinner frame glyph, refreshed by the bin each frame
    /// from wall-clock elapsed. Defaults to a fixed frame so snapshot tests are
    /// deterministic unless they opt into `busy`.
    pub spinner: char,
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
    /// Number of items the Tasks rail drawer would show, sampled by the bin each
    /// frame from `tasks(&events)` so `layout::compute` can grow the drawer to fit
    /// a longer list (rehydrate-safe: this is a layout hint, not the task list —
    /// the rendered content still comes from the event log). Default 0 ("no tasks").
    pub tasks_len: u16,
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
    /// The secret env name currently being entered via the masked key-prompt
    /// (e.g. `"ANTHROPIC_API_KEY"`), or `None` when not prompting. Set when a
    /// selected provider needs a key we don't have yet (Task 15 gate); cleared
    /// on commit or cancel.
    pub config_key_prompt: Option<&'static str>,
    /// Compact elapsed-time-in-session label (e.g. "12m", "1h3m").
    pub duration: String,
    /// Total tokens spent in the active session (session drawer "tok" line).
    /// Excludes cache-read tokens (those are tracked separately in
    /// `cached_tokens`).
    pub session_tokens: u64,
    /// Cumulative cache-read tokens for the active session (session drawer
    /// "cac" line). The subset of input tokens served from the provider's
    /// prompt cache, summed across all turns.
    pub cached_tokens: u64,
    /// Whether the active provider/model reports a token-level prompt cache.
    /// When false, the session drawer shows "n/a" for `cac` and the context
    /// drawer dims its cache sparkline label. Set once at startup from
    /// `has_prompt_cache(model)`.
    pub cache_supported: bool,
    /// Current context-window token usage (session drawer "ctx" line).
    pub ctx_used: u64,
    /// Context-window ceiling in tokens (session drawer "ctx" line denominator).
    pub ctx_ceiling: u64,
    /// Whether `ctx_ceiling` was set by a user config override rather than the
    /// model's native value. When true, the session drawer renders a `*` after
    /// the ceiling to signal the override.
    pub ctx_ceiling_overridden: bool,
    /// Current working directory shown (truncated) in the session drawer.
    pub cwd: String,
    /// Highlighted section in the config overlay's left nav (Task 11).
    pub config_section: usize,
    /// Highlighted field row in the config overlay's active section (Task 11).
    pub config_field: usize,
    /// In-progress edit buffer for the current config field; `None` when not editing.
    pub config_edit: Option<String>,
    /// The resolved config sections rendered by the config overlay, computed by
    /// the bin once per frame from `Config` + `Provenance` + secret statuses
    /// (Task 12 wires population; empty here is a valid default).
    pub config_sections: Vec<crate::config_view::Section>,
    /// Focused column in the config overlay (fields vs the drilled-open picker).
    pub config_col: ConfigCol,
    /// The open col-3 picker options; empty when no picker is drilled open.
    pub config_picker: Vec<crate::config_view::PickOption>,
    /// Highlighted row within the open picker.
    pub config_picker_sel: usize,
    /// Name of the tool currently executing (in-flight indicator), or `None`.
    pub active_tool: Option<String>,
    /// The active `ask_user` question overlay's state, or `None` when no
    /// question is pending (Task 11 renders it; Task 9 populates it via
    /// `AgentUpdate::AskUser`).
    pub question: Option<crate::question::QuestionState>,
    /// Highlighted row in the quick-switch overlay's provider list (Task 11
    /// renders it; Task 10 only plumbs the state through).
    pub switch_provider_sel: usize,
    /// Highlighted row in the quick-switch overlay's model list.
    pub switch_model_sel: usize,
    /// Focused pane in the quick-switch overlay (provider list vs model list).
    pub switch_pane: SwitchPane,
    /// Provider options shown in the quick-switch overlay's left pane, seeded
    /// by the bin on `OpenProviderSwitch`/pane moves (Task 11).
    pub switch_providers: Vec<crate::config_view::PickOption>,
    /// Model options shown in the quick-switch overlay's right pane, tracking
    /// the highlighted provider (Task 11).
    pub switch_models: Vec<crate::config_view::PickOption>,
}

impl ShellState {
    /// The calm default: Chat mode, focus on the input, the Chat rail set
    /// (repo/session/context all open, matching `docs/ux/chat-mode.html`).
    pub fn new() -> Self {
        let drawers = vec![
            Drawer {
                id: DrawerId::Repo,
                title: "repo".into(),
                open: true,
            },
            Drawer {
                id: DrawerId::Session,
                title: "session".into(),
                open: true,
            },
            Drawer {
                id: DrawerId::Context,
                title: "context · tokens".into(),
                open: true,
            },
            Drawer {
                id: DrawerId::Tasks,
                title: "tasks".into(),
                open: true,
            },
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
            follow_tail: true,
            scrollbar_drag: false,
            busy: false,
            spinner: crate::tokens::glyph::SPINNER[0],
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
            tasks_len: 0,
            sessions: Vec::new(),
            session_selected: 0,
            session_name: String::new(),
            model: String::new(),
            provider: String::new(),
            config_key_prompt: None,
            duration: "0m".into(),
            session_tokens: 0,
            cached_tokens: 0,
            cache_supported: false,
            ctx_used: 0,
            ctx_ceiling: 0,
            ctx_ceiling_overridden: false,
            cwd: String::new(),
            config_section: 0,
            config_field: 0,
            config_edit: None,
            config_sections: Vec::new(),
            config_col: ConfigCol::Fields,
            config_picker: Vec::new(),
            config_picker_sel: 0,
            active_tool: None,
            question: None,
            switch_provider_sel: 0,
            switch_model_sel: 0,
            switch_pane: SwitchPane::Provider,
            switch_providers: Vec::new(),
            switch_models: Vec::new(),
        }
    }

    pub fn drawer(&self, id: DrawerId) -> Option<&Drawer> {
        self.drawers.iter().find(|d| d.id == id)
    }

    pub fn drawer_mut(&mut self, id: DrawerId) -> Option<&mut Drawer> {
        self.drawers.iter_mut().find(|d| d.id == id)
    }

    /// Drop a drawer from the rail entirely (not just collapse it). The layout
    /// allocator and renderer both iterate `drawers`, so a removed drawer takes
    /// up no rail rows and is never drawn. Used by the bin to hide the Repo
    /// drawer when the working directory is not inside a git repo (§16).
    pub fn remove_drawer(&mut self, id: DrawerId) {
        self.drawers.retain(|d| d.id != id);
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

    /// Increase detail (Summary → Normal → Detail), saturating. A real altitude
    /// change re-anchors `conversation_scroll` to the top: altitudes have
    /// incomparable line counts, so a carried-over offset could otherwise land
    /// past the new altitude's end and render blank.
    pub fn zoom_in(&mut self) {
        let next = match self.zoom {
            Zoom::Summary => Zoom::Normal,
            Zoom::Normal | Zoom::Detail => Zoom::Detail,
        };
        if next != self.zoom {
            self.conversation_scroll = 0;
        }
        self.zoom = next;
    }

    /// Decrease detail (Detail → Normal → Summary), saturating. Re-anchors scroll
    /// on a real change (see `zoom_in`).
    pub fn zoom_out(&mut self) {
        let next = match self.zoom {
            Zoom::Detail => Zoom::Normal,
            Zoom::Normal | Zoom::Summary => Zoom::Summary,
        };
        if next != self.zoom {
            self.conversation_scroll = 0;
        }
        self.zoom = next;
    }

    /// Apply a scroll delta to the conversation, clamped to `[0, max_scroll]`,
    /// and update tail-follow: landing at (or past) the bottom re-engages follow;
    /// any position above the bottom detaches it. `max_scroll` is the max offset
    /// at the current altitude, supplied by the bin from the last drawn frame.
    pub fn scroll_conversation(&mut self, delta: i32, max_scroll: u16) {
        let next = (self.conversation_scroll as i32 + delta).clamp(0, max_scroll as i32) as u16;
        self.conversation_scroll = next;
        self.follow_tail = next >= max_scroll;
    }

    /// Set the conversation scroll to an absolute `offset` (clamped to
    /// [0, max_scroll]) and re-derive tail-follow: landing at (or past) the
    /// bottom re-engages follow, any position above it detaches. Used by the
    /// scrollbar drag / track click.
    pub fn scroll_to_offset(&mut self, offset: u16, max_scroll: u16) {
        let next = offset.min(max_scroll);
        self.conversation_scroll = next;
        self.follow_tail = next >= max_scroll;
    }

    /// When following the tail, pin the offset to the latest line. Called by the
    /// bin before each draw (skipped during the zoom animation, which is
    /// top-anchored). No-op when the user has scrolled up (detached).
    pub fn apply_follow(&mut self, max_scroll: u16) {
        if self.follow_tail {
            self.conversation_scroll = max_scroll;
        }
    }

    /// Show the in-flight spinner for a tool that has just started running.
    pub fn set_active_tool(&mut self, name: impl Into<String>) {
        self.active_tool = Some(name.into());
    }

    /// Clear the in-flight spinner (its `ToolResult` arrived, or the turn ended).
    pub fn clear_active_tool(&mut self) {
        self.active_tool = None;
    }

    /// True when the col-3 contextual picker is drilled open.
    pub fn config_picker_open(&self) -> bool {
        !self.config_picker.is_empty()
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
        assert_eq!(
            ids,
            vec![
                DrawerId::Repo,
                DrawerId::Session,
                DrawerId::Context,
                DrawerId::Tasks
            ]
        );
        // All four expanded (mockup shows repo/session/context all `on`; tasks joins them).
        assert!(s.drawer(DrawerId::Repo).unwrap().open);
        assert!(s.drawer(DrawerId::Session).unwrap().open);
        assert!(s.drawer(DrawerId::Context).unwrap().open);
        assert!(s.drawer(DrawerId::Tasks).unwrap().open);
    }

    #[test]
    fn remove_drawer_drops_it_from_the_rail() {
        let mut s = ShellState::new();
        s.remove_drawer(DrawerId::Repo);
        let ids: Vec<DrawerId> = s.drawers.iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            vec![DrawerId::Session, DrawerId::Context, DrawerId::Tasks]
        );
        assert!(s.drawer(DrawerId::Repo).is_none());
        // Removing an absent drawer is a no-op (idempotent).
        s.remove_drawer(DrawerId::Repo);
        assert_eq!(s.drawers.len(), 3);
    }

    #[test]
    fn tasks_drawer_is_last_and_open() {
        let s = ShellState::new();
        let last = s.drawers.last().unwrap();
        assert_eq!(last.id, DrawerId::Tasks);
        assert!(last.open);
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

    #[test]
    fn zoom_change_resets_conversation_scroll_but_saturation_does_not() {
        let mut s = ShellState::new(); // Normal
        s.conversation_scroll = 42;
        s.zoom_in(); // Normal → Detail: real change → reset
        assert_eq!(s.zoom, Zoom::Detail);
        assert_eq!(
            s.conversation_scroll, 0,
            "a real zoom change re-anchors scroll"
        );

        // At an extreme, a no-op zoom must NOT clobber the scroll offset.
        s.conversation_scroll = 17;
        s.zoom_in(); // already Detail → no change
        assert_eq!(s.zoom, Zoom::Detail);
        assert_eq!(
            s.conversation_scroll, 17,
            "a saturating no-op zoom keypress preserves scroll"
        );
    }

    #[test]
    fn new_sessions_follow_the_tail_by_default() {
        assert!(
            ShellState::new().follow_tail,
            "a fresh session should show the latest output (scroll-to-latest on startup)"
        );
    }

    #[test]
    fn scrolling_up_detaches_follow_and_returning_to_bottom_reengages() {
        let mut s = ShellState::new();
        s.conversation_scroll = 100; // pinned to the bottom
                                     // Scroll up one line → detaches from the tail.
        s.scroll_conversation(-1, 100);
        assert_eq!(s.conversation_scroll, 99);
        assert!(
            !s.follow_tail,
            "scrolling up off the bottom detaches follow"
        );
        // Scroll back down to the bottom → re-engages.
        s.scroll_conversation(1, 100);
        assert_eq!(s.conversation_scroll, 100);
        assert!(s.follow_tail, "returning to the bottom re-engages follow");
    }

    #[test]
    fn scroll_clamps_and_a_downward_overshoot_at_bottom_keeps_following() {
        let mut s = ShellState::new();
        s.conversation_scroll = 5;
        // Overshoot the top: clamps to 0, detached (0 < max).
        s.scroll_conversation(-100, 50);
        assert_eq!(s.conversation_scroll, 0);
        assert!(!s.follow_tail);
        // Overshoot the bottom: clamps to max, re-engaged.
        s.scroll_conversation(1000, 50);
        assert_eq!(s.conversation_scroll, 50);
        assert!(s.follow_tail);
    }

    #[test]
    fn scroll_to_offset_clamps_and_toggles_follow() {
        let mut s = ShellState::new();
        // absolute jump above the bottom → detaches follow
        s.scroll_to_offset(20, 100);
        assert_eq!(s.conversation_scroll, 20);
        assert!(!s.follow_tail);
        // jump to (or past) the bottom → re-engages follow, clamps
        s.scroll_to_offset(999, 100);
        assert_eq!(s.conversation_scroll, 100);
        assert!(s.follow_tail);
    }

    #[test]
    fn scrollbar_drag_defaults_false() {
        assert!(!ShellState::new().scrollbar_drag);
    }

    #[test]
    fn apply_follow_pins_only_when_following() {
        let mut s = ShellState::new();
        s.follow_tail = true;
        s.apply_follow(276);
        assert_eq!(s.conversation_scroll, 276, "following pins to the tail");

        s.follow_tail = false;
        s.conversation_scroll = 50;
        s.apply_follow(276);
        assert_eq!(
            s.conversation_scroll, 50,
            "a detached view is left where the user parked it"
        );
    }

    #[test]
    fn active_tool_sets_and_clears() {
        let mut s = ShellState::new();
        assert_eq!(s.active_tool, None);
        s.set_active_tool("shell");
        assert_eq!(s.active_tool.as_deref(), Some("shell"));
        s.clear_active_tool();
        assert_eq!(s.active_tool, None);
    }

    #[test]
    fn config_picker_defaults_closed() {
        let s = ShellState::new();
        assert!(matches!(s.config_col, ConfigCol::Fields));
        assert!(!s.config_picker_open());
        assert_eq!(s.config_picker_sel, 0);
    }
}
