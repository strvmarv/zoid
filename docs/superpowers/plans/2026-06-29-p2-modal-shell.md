# P2 · Modal Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's single-surface Chat loop into a real modal TUI driven by keyboard *and* mouse — an app-framework floor (focus ring, contextual key routing, mouse hit-testing, pane/drawer manager), a reusable right-rail with files & branch drawers, a `^P` command palette and `:` command line, and a persistent mode indicator (Chat ⇄ Build, with Build as a P6 placeholder surface).

**Architecture:** The reusable shell framework lives in **`zoid-tui`** (which nothing depends on, so no dependency cycle): a pure `ShellState` data model, a pure `layout::compute(area, &ShellState) → ShellLayout` that *both* the renderer and mouse hit-testing share (one geometry, DRY — spec §13/§16), a pure key/mouse **router** returning an `Action` enum, and the render functions. The `zoid` binary shrinks to *crossterm event → `route_*` → `Action` → apply/side-effect*, plus mouse-capture setup and cwd/branch population. All floor logic is unit-tested as pure functions over synthetic events (spec §13: "focus/keymap/mouse-hit-testing tested as pure logic"); every screen ships an `insta`/`TestBackend` snapshot bound to `docs/ux/` (spec §16).

**Tech Stack:** Rust 2021, ratatui 0.29 (+ its re-exported `ratatui::crossterm`), `tui-textarea`, `insta` + `TestBackend` (snapshots), plus the existing `zoid-core`/`zoid-provider`/`zoid-tools` crates. No new dependencies.

## Global Constraints

- **No new crate dependencies.** `zoid-tui` reaches crossterm types via `ratatui::crossterm::event::*` (ratatui 0.29 re-exports it); the `zoid` bin already depends on `crossterm` (with `event-stream`).
- **Design tokens are the single source of truth** (spec §16): every glyph/color/layout-constant renders from `zoid_tui::tokens`. Values copied verbatim from `docs/ux/README.md`. No literal glyphs/hex outside `tokens.rs`.
- **No dependency cycles:** `ShellState`, layout, router, and hit-testing live in `zoid-tui`. The `zoid` bin depends on `zoid-tui`, never the reverse.
- **Pure logic is terminal-free and unit-tested:** `compute`, `route_key`, `route_mouse`, `hit_test`, focus-ring transitions, palette filtering, and command parsing take values and return values — no `Frame`, no I/O.
- **DRY geometry:** mouse hit-testing and rendering MUST use the *same* `ShellLayout` from `layout::compute`. Hit-testing never re-derives rects independently.
- **Fidelity:** each new screen (chat-with-rail, palette overlay, command line, Build placeholder) ships a `TestBackend` + `insta` snapshot built to match its `docs/ux/` mockup (`chat-mode.html`, `palette.html`, `modes.html`). The first snapshot is visually checked against the mockup before acceptance.
- **Responsive rail:** the rail is shown only when `area.width >= RAIL_MIN_TOTAL` (80). This keeps the existing 60×12 chat snapshots structurally rail-free; new rail/palette snapshots use a wider backend (100×24).
- **Keymap (spec §6.2/§6.5):** `Tab` = focus-next (forward, wraps) · `⇧Tab` (`BackTab`) = switch mode · `^P` = palette (universal) · `:` = command line **only when focus ≠ Input** · `^C` = quit · `esc` = close overlay. Mode commands also via `:build`/`:chat`.
- **Scope cuts (state explicitly, defer cleanly):** the palette & command line are **keyboard-driven** in P2 (mouse hit-testing covers the main surface only: conversation/input focus + drawer-header toggle + scroll). Economy ⑤ drawer is a **placeholder** (real content = P3). Semantic zoom ①, object verbs ④, tree-sitter Ⓡ3, motion Ⓡ2 are **P4**. Build's real surface is **P6** (P2 ships an amber placeholder). Branch fork/undo/time-travel palette rows render **dimmed/disabled** (post-v1). `git2` is **P5** — branch name in P2 is read cheaply from `.git/HEAD` (no `git2`).
- **No `Co-Authored-By` / co-author trailer** on any commit (user's `~/CLAUDE.md`).
- TDD throughout: write the failing test, watch it fail, minimal impl, watch it pass, commit. `cargo clippy --all-targets` stays at 0 warnings.

---

## File Structure

**`zoid-tui` (the reusable shell framework + views):**
- `crates/zoid-tui/src/tokens.rs` *(modify)* — add `glyph::{COLLAPSED, EXPANDED}` and `color::{SEL_BG, CHAT_BG, BUILD_BG}`.
- `crates/zoid-tui/src/state.rs` *(create)* — `Mode`, `Focus`, `Overlay`, `DrawerId`, `Drawer`, `PaletteState`, `CmdlineState`, `ShellState` + constructors and pure mutators (focus ring, mode toggle, drawer toggle).
- `crates/zoid-tui/src/layout.rs` *(create)* — `ShellLayout` + `compute(area, &ShellState) → ShellLayout`; layout constants `RAIL_WIDTH`, `RAIL_MIN_TOTAL`, `MAX_MEASURE`; `in_rect` helper.
- `crates/zoid-tui/src/command.rs` *(create)* — `Command` enum + `parse_command(&str) → Command`.
- `crates/zoid-tui/src/palette.rs` *(create)* — `PaletteItem`, `all_items(Mode)`, `fuzzy_score`, `filtered_indices`, selection helpers.
- `crates/zoid-tui/src/route.rs` *(create)* — `Action`, `Target`, `route_key`, `hit_test`, `route_mouse`.
- `crates/zoid-tui/src/chat.rs` *(modify)* — extract the conversation-body builder into `conversation_lines(...)`; keep `render_chat` thin (used by `render_shell` for the conversation pane).
- `crates/zoid-tui/src/render.rs` *(create)* — `render_shell(frame, &ShellState, &[ChatMsg], &TextArea, streaming)` orchestrating title/main/rail/input/status + overlays; rail/palette/cmdline/Build-placeholder renderers.
- `crates/zoid-tui/src/lib.rs` *(modify)* — declare and re-export the new modules.
- `crates/zoid-tui/tests/shell_snapshot.rs` *(create)* — snapshots: chat-with-rail, files-drawer-open, palette overlay, command line, Build placeholder.

**`zoid` (bin wiring):**
- `crates/zoid/src/main.rs` *(modify)* — hold `ShellState`; enable/disable mouse capture; replace `classify` with `route_key`/`route_mouse`; handle `Action`s; populate files (cwd) + branch (`.git/HEAD`).
- `crates/zoid/src/input.rs` *(delete)* — superseded by `zoid_tui::route`.
- `crates/zoid/src/lib.rs` *(modify)* — drop `pub mod input;`.

---

## Task 1: Design tokens for P2

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs`

**Interfaces:**
- Produces: `glyph::COLLAPSED = '▸'`, `glyph::EXPANDED = '▾'`; `color::SEL_BG`, `color::CHAT_BG`, `color::BUILD_BG`.

- [ ] **Step 1: Add the failing assertions**

In `crates/zoid-tui/src/tokens.rs`, extend the existing `tests` module:

```rust
    #[test]
    fn p2_tokens_present() {
        assert_eq!(glyph::COLLAPSED, '▸');
        assert_eq!(glyph::EXPANDED, '▾');
        assert_eq!(color::SEL_BG, Color::Rgb(0x16, 0x33, 0x5c));
        assert_eq!(color::CHAT_BG, Color::Rgb(0x0d, 0x2a, 0x4d));
        assert_eq!(color::BUILD_BG, Color::Rgb(0x3d, 0x2a, 0x0a));
    }
```

- [ ] **Step 2: Run it — expect failure**

Run: `cargo test -p zoid-tui tokens`
Expected: FAIL (no `COLLAPSED`/`SEL_BG`/etc.).

- [ ] **Step 3: Add the tokens**

In `pub mod glyph` add (values from `docs/ux/README.md`: `▸`/`▾` collapsed/expanded):

```rust
    pub const COLLAPSED: char = '▸';
    pub const EXPANDED: char = '▾';
```

In `pub mod color` add (selection bg from `palette.html` `.it.sel{background:#16335c}`; mode chip bgs from `modes.html` `.mode.chat{background:#0d2a4d}` / `.mode.build{background:#3d2a0a}`):

```rust
    pub const SEL_BG: Color = Color::Rgb(0x16, 0x33, 0x5c);
    pub const CHAT_BG: Color = Color::Rgb(0x0d, 0x2a, 0x4d);
    pub const BUILD_BG: Color = Color::Rgb(0x3d, 0x2a, 0x0a);
```

- [ ] **Step 4: Run — expect pass**

Run: `cargo test -p zoid-tui tokens`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs
git commit -m "feat(tui): P2 design tokens (▸/▾ glyphs, selection + mode-chip bgs)"
```

---

## Task 2: Shell state model

**Files:**
- Create: `crates/zoid-tui/src/state.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Produces: `Mode`, `Focus`, `Overlay`, `DrawerId`, `Drawer`, `PaletteState`, `CmdlineState`, `ShellState`, and `ShellState::new()`. Later tasks (layout, route, render) consume these.

- [ ] **Step 1: Declare the module**

In `crates/zoid-tui/src/lib.rs`, add after `pub mod chat;`:

```rust
pub mod state;
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid-tui/src/state.rs` with the test first (drives the shape):

```rust
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
```

- [ ] **Step 3: Run — expect failure**

Run: `cargo test -p zoid-tui state`
Expected: FAIL (`ShellState::new`/`drawer` not found).

- [ ] **Step 4: Implement the constructor and lookups**

Add to `state.rs` (above the `tests` module):

```rust
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
```

- [ ] **Step 5: Run — expect pass**

Run: `cargo test -p zoid-tui state`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): ShellState model (mode/focus/overlay/rail drawers)"
```

---

## Task 3: Focus ring, mode toggle, drawer toggle (pure mutators)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`

**Interfaces:**
- Consumes: `ShellState`, `Focus`, `Mode`, `DrawerId` (Task 2).
- Produces: `ShellState::focus_next()`, `toggle_mode()`, `toggle_drawer(DrawerId)`, `open_drawer(DrawerId)`, `close_overlay()` — pure mutators the router and bin call.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `state.rs`:

```rust
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
        s.toggle_drawer(DrawerId::Files);
        assert!(s.drawer(DrawerId::Files).unwrap().open);
        // open_drawer forces open (idempotent) and ensures the rail is visible.
        s.rail_visible = false;
        s.open_drawer(DrawerId::Branch);
        assert!(s.rail_visible);
        assert!(s.drawer(DrawerId::Branch).unwrap().open);
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
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p zoid-tui state`
Expected: FAIL (mutators not defined).

- [ ] **Step 3: Implement the mutators**

Add into the `impl ShellState` block:

```rust
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
    }
```

- [ ] **Step 4: Run — expect pass**

Run: `cargo test -p zoid-tui state`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): focus ring + mode/drawer/overlay mutators (pure, tested)"
```

---

## Task 4: Layout function (shared geometry)

**Files:**
- Create: `crates/zoid-tui/src/layout.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Consumes: `ShellState`, `Overlay`, `DrawerId` (Task 2).
- Produces: `ShellLayout { title, body, conversation, rail: Option<Rect>, drawer_headers: Vec<(DrawerId, Rect)>, input, status, palette: Option<Rect>, cmdline: Option<Rect> }`; `compute(area: Rect, &ShellState) → ShellLayout`; constants `RAIL_WIDTH = 30`, `RAIL_MIN_TOTAL = 80`, `MAX_MEASURE = 100`; `pub fn in_rect(r: Rect, col: u16, row: u16) -> bool`. Consumed by the router (Task 7, hit-testing) and the renderer (Task 9).

- [ ] **Step 1: Declare the module**

In `crates/zoid-tui/src/lib.rs` add: `pub mod layout;`

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tui/src/layout.rs`:

```rust
//! Pure layout geometry. `compute` is the single source of rects, shared by the
//! renderer (draws into them) and mouse hit-testing (maps points to them) — so a
//! click and a draw can never disagree (spec §13/§16). No `Frame`, no I/O.

use crate::state::{DrawerId, Overlay, ShellState};
use ratatui::layout::{Constraint, Layout, Rect};

/// Rail width in columns (mockup right column ≈ 30 cols; spec min ≈ 28).
pub const RAIL_WIDTH: u16 = 30;
/// Minimum total width before the rail is shown (stream ≥ ~50 + rail ≥ ~28 — spec §6.2).
pub const RAIL_MIN_TOTAL: u16 = 80;
/// Conversation column measure cap (spec §6.1: ~80–100 cols, ergonomics).
pub const MAX_MEASURE: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLayout {
    pub title: Rect,
    pub body: Rect,
    pub conversation: Rect,
    pub rail: Option<Rect>,
    pub drawer_headers: Vec<(DrawerId, Rect)>,
    pub input: Rect,
    pub status: Rect,
    pub palette: Option<Rect>,
    pub cmdline: Option<Rect>,
}

/// True when (col,row) falls inside `r` (half-open on right/bottom).
pub fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x.saturating_add(r.width) && row >= r.y && row < r.y.saturating_add(r.height)
}

pub fn compute(area: Rect, state: &ShellState) -> ShellLayout {
    // Vertical: title(1) · body(min) · input(3) · status(1).
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);
    let (title, body, input, status) = (rows[0], rows[1], rows[2], rows[3]);

    let show_rail = state.rail_visible && area.width >= RAIL_MIN_TOTAL;
    let rail_w = if show_rail { RAIL_WIDTH } else { 0 };
    let avail = body.width.saturating_sub(rail_w);
    let conv_w = avail.min(MAX_MEASURE);
    let gutter_w = avail.saturating_sub(conv_w);

    let cols = Layout::horizontal([
        Constraint::Length(gutter_w),
        Constraint::Length(conv_w),
        Constraint::Length(rail_w),
    ])
    .split(body);
    let conversation = cols[1];
    let rail = if show_rail { Some(cols[2]) } else { None };

    // Drawer header rects: one row per drawer, stacked from the rail top (1-col inset).
    let mut drawer_headers = Vec::new();
    if let Some(rr) = rail {
        let inner = Rect { x: rr.x + 1, y: rr.y, width: rr.width.saturating_sub(2), height: rr.height };
        let mut y = inner.y;
        for d in &state.drawers {
            if y >= inner.y + inner.height {
                break;
            }
            drawer_headers.push((d.id, Rect { x: inner.x, y, width: inner.width, height: 1 }));
            // header(1) + body when open (P2: a fixed 4-row body budget), + 1 spacer.
            let body_rows = if d.open { 4 } else { 0 };
            y = y.saturating_add(1 + body_rows + 1);
        }
    }

    // Overlays (rendered on top; rects only — content in Task 8).
    let palette = if state.overlay == Overlay::Palette {
        Some(centered(area, 72, 18))
    } else {
        None
    };
    let cmdline = if state.overlay == Overlay::CommandLine {
        Some(Rect { x: area.x, y: status.y, width: area.width, height: 1 })
    } else {
        None
    };

    ShellLayout { title, body, conversation, rail, drawer_headers, input, status, palette, cmdline }
}

/// A rect `w×h` (clamped to `area`) centered horizontally, near the top third.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 3;
    Rect { x, y, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ShellState;

    fn area(w: u16, h: u16) -> Rect {
        Rect { x: 0, y: 0, width: w, height: h }
    }

    #[test]
    fn narrow_hides_rail() {
        let s = ShellState::new();
        let l = compute(area(60, 12), &s);
        assert!(l.rail.is_none());
        assert!(l.drawer_headers.is_empty());
        // conversation spans the full body width when there's no rail/gutter.
        assert_eq!(l.conversation.width, 60);
    }

    #[test]
    fn wide_shows_rail_and_drawer_headers() {
        let s = ShellState::new();
        let l = compute(area(100, 24), &s);
        let rail = l.rail.expect("rail visible at 100 cols");
        assert_eq!(rail.width, RAIL_WIDTH);
        assert_eq!(l.drawer_headers.len(), 4); // economy/files/branch/palette
        // headers stack downward
        assert!(l.drawer_headers[1].1.y > l.drawer_headers[0].1.y);
    }

    #[test]
    fn measure_is_capped_on_ultrawide() {
        let s = ShellState::new();
        let l = compute(area(200, 24), &s);
        assert_eq!(l.conversation.width, MAX_MEASURE);
    }

    #[test]
    fn palette_rect_only_when_overlay_active() {
        let mut s = ShellState::new();
        assert!(compute(area(100, 24), &s).palette.is_none());
        s.overlay = Overlay::Palette;
        let l = compute(area(100, 24), &s);
        let p = l.palette.unwrap();
        assert!(in_rect(p, p.x + 1, p.y + 1)); // sane non-empty rect
        assert!(p.width <= 100 && p.height <= 24);
    }
}
```

- [ ] **Step 3: Run — expect failure then pass**

Run: `cargo test -p zoid-tui layout`
Expected: FAIL first (module new), then after it compiles: PASS. (If `rail-hidden` width assertion needs tuning to the actual split, adjust the expected value to what `compute` produces — the invariant under test is "no rail/gutter at 60 cols", not a magic number.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/layout.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): pure layout geometry shared by render + hit-test"
```

---

## Task 5: Command parsing

**Files:**
- Create: `crates/zoid-tui/src/command.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Consumes: `Mode`, `DrawerId` (Task 2).
- Produces: `Command { SwitchMode(Mode), Quit, OpenDrawer(DrawerId), Unknown(String) }`; `parse_command(&str) → Command`. Consumed by the router's command-line/palette run (Tasks 6/7) and the bin (Task 10).

- [ ] **Step 1: Declare the module**

In `lib.rs` add: `pub mod command;`

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tui/src/command.rs`:

```rust
//! The `:`-command and palette-action vocabulary. Both the command line and the
//! palette resolve to a `Command`; the `zoid` bin executes it (spec §6.5).

use crate::state::{DrawerId, Mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SwitchMode(Mode),
    Quit,
    OpenDrawer(DrawerId),
    Unknown(String),
}

/// Parse a command-line string. Accepts an optional leading `:` and surrounding
/// whitespace. `:build`/`:chat`, `:q`/`:quit`, `:files`/`:branch`.
pub fn parse_command(raw: &str) -> Command {
    let t = raw.trim().trim_start_matches(':').trim();
    match t {
        "build" => Command::SwitchMode(Mode::Build),
        "chat" => Command::SwitchMode(Mode::Chat),
        "q" | "quit" => Command::Quit,
        "files" => Command::OpenDrawer(DrawerId::Files),
        "branch" => Command::OpenDrawer(DrawerId::Branch),
        other => Command::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_commands_with_or_without_colon() {
        assert_eq!(parse_command(":build"), Command::SwitchMode(Mode::Build));
        assert_eq!(parse_command("chat"), Command::SwitchMode(Mode::Chat));
        assert_eq!(parse_command("  :q "), Command::Quit);
        assert_eq!(parse_command(":files"), Command::OpenDrawer(DrawerId::Files));
        assert_eq!(parse_command(":branch"), Command::OpenDrawer(DrawerId::Branch));
    }

    #[test]
    fn unknown_is_captured_verbatim() {
        assert_eq!(parse_command(":wat"), Command::Unknown("wat".into()));
    }
}
```

- [ ] **Step 3: Run — expect failure then pass**

Run: `cargo test -p zoid-tui command`
Expected: PASS after it compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): :-command parser (build/chat/q/files/branch)"
```

---

## Task 6: Palette items + fuzzy filter

**Files:**
- Create: `crates/zoid-tui/src/palette.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Consumes: `Mode` (Task 2), `Command`/`DrawerId` (Task 5).
- Produces: `PaletteItem { group, icon, label, hint, keybind, command: Option<Command> }`; `all_items(Mode) -> Vec<PaletteItem>`; `fuzzy_score(label, query) -> Option<i32>`; `selectable_matches(items, query) -> Vec<usize>` (indices into `items`, ranked); `nav(selected, delta, len) -> usize`. Consumed by router (Task 7), renderer (Task 8), bin (Task 10).

- [ ] **Step 1: Declare the module**

In `lib.rs` add: `pub mod palette;`

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tui/src/palette.rs`:

```rust
//! The command palette's item set + fuzzy filtering (spec §6.5; mockup
//! `palette.html`). Grouped, mode-aware, each row teaching its keybind.
//! Post-v1 rows (branch/recipes) have `command: None` → rendered dimmed, not
//! selectable. Pure; rendering lives in `render.rs`.

use crate::command::Command;
use crate::state::{DrawerId, Mode};
use crate::tokens::glyph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub group: &'static str,
    pub icon: char,
    pub label: &'static str,
    pub hint: &'static str,
    pub keybind: &'static str,
    /// `None` = disabled (post-v1), shown dimmed and skipped by selection.
    pub command: Option<Command>,
}

/// The full, ordered item set for `mode` (grouped exactly as `palette.html`).
pub fn all_items(mode: Mode) -> Vec<PaletteItem> {
    // The mode row offers the *other* mode.
    let (mode_label, mode_cmd) = match mode {
        Mode::Chat => ("Switch to Build", Command::SwitchMode(Mode::Build)),
        Mode::Build => ("Switch to Chat", Command::SwitchMode(Mode::Chat)),
    };
    vec![
        PaletteItem { group: "mode", icon: '⇢', label: mode_label, hint: "continue this conversation into the loop", keybind: "⇧Tab", command: Some(mode_cmd) },
        // branch group — post-v1, disabled/dimmed
        PaletteItem { group: "branch ⎇ · post-v1", icon: glyph::BRANCH, label: "Fork from here", hint: "new branch at this turn", keybind: ":fork", command: None },
        PaletteItem { group: "branch ⎇ · post-v1", icon: '⤺', label: "Undo last turn", hint: "move head back", keybind: "u", command: None },
        // navigate
        PaletteItem { group: "navigate", icon: '▤', label: "Open files drawer", hint: "browse the working tree", keybind: "^F", command: Some(Command::OpenDrawer(DrawerId::Files)) },
        PaletteItem { group: "navigate", icon: glyph::BRANCH, label: "Open branch drawer", hint: "current branch", keybind: "^B", command: Some(Command::OpenDrawer(DrawerId::Branch)) },
        // context ⑤ — placeholder (real actions land P3), disabled for now
        PaletteItem { group: "context ⑤ · P3", icon: '●', label: "Pin file to context", hint: "lands in P3", keybind: "", command: None },
        PaletteItem { group: "context ⑤ · P3", icon: '✕', label: "Evict cold items", hint: "lands in P3", keybind: "", command: None },
        // settings
        PaletteItem { group: "settings", icon: '◆', label: "Quit zoid", hint: "exit", keybind: "^C", command: Some(Command::Quit) },
        // recipes — post-v1
        PaletteItem { group: "recipes · post-v1", icon: '▷', label: "Run recipe…", hint: "post-v1", keybind: "", command: None },
    ]
}

/// Case-insensitive fuzzy score: `Some(higher = better)` if `query` is a
/// subsequence of `label`; `None` otherwise. Empty query matches everything.
/// Contiguous substring beats scattered subsequence; earlier match beats later.
pub fn fuzzy_score(label: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = label.to_lowercase().chars().collect();
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    // Contiguous-substring bonus.
    let label_l = label.to_lowercase();
    if let Some(pos) = label_l.find(&query.to_lowercase()) {
        return Some(1000 - pos as i32);
    }
    // Scattered subsequence.
    let mut qi = 0usize;
    let mut first: Option<usize> = None;
    for (i, c) in hay.iter().enumerate() {
        if qi < needle.len() && *c == needle[qi] {
            if first.is_none() {
                first = Some(i);
            }
            qi += 1;
        }
    }
    if qi == needle.len() {
        Some(100 - first.unwrap_or(0) as i32)
    } else {
        None
    }
}

/// Indices into `items` of *selectable* rows (have a command) matching `query`,
/// ranked best-first (stable on ties).
pub fn selectable_matches(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.command.is_some())
        .filter_map(|(i, it)| fuzzy_score(it.label, query).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// Move a selection index by `delta`, clamped to `[0, len)` (no wrap).
pub fn nav(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len - 1;
    let next = selected as i64 + delta as i64;
    next.clamp(0, max as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_outranks_subsequence() {
        let sub = fuzzy_score("Open files drawer", "files").unwrap();
        let seq = fuzzy_score("Switch to Build", "sib").unwrap();
        assert!(sub > seq);
    }

    #[test]
    fn no_match_is_none() {
        assert!(fuzzy_score("Quit zoid", "zzz").is_none());
    }

    #[test]
    fn matches_exclude_disabled_rows() {
        let items = all_items(Mode::Chat);
        // "Fork from here" is post-v1 (command None) — never selectable.
        let idxs = selectable_matches(&items, "fork");
        assert!(idxs.is_empty());
        let idxs = selectable_matches(&items, "build");
        assert_eq!(items[idxs[0]].label, "Switch to Build");
    }

    #[test]
    fn empty_query_returns_all_selectable() {
        let items = all_items(Mode::Chat);
        let selectable = items.iter().filter(|i| i.command.is_some()).count();
        assert_eq!(selectable_matches(&items, "").len(), selectable);
    }

    #[test]
    fn nav_clamps() {
        assert_eq!(nav(0, -1, 3), 0);
        assert_eq!(nav(2, 1, 3), 2);
        assert_eq!(nav(1, 1, 3), 2);
        assert_eq!(nav(0, 1, 0), 0);
    }
}
```

- [ ] **Step 3: Run — expect pass**

Run: `cargo test -p zoid-tui palette`
Expected: PASS after compile.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/palette.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): palette items + fuzzy filter (mode-aware, post-v1 dimmed)"
```

---

## Task 7: Key/mouse router + hit-testing

**Files:**
- Create: `crates/zoid-tui/src/route.rs`
- Modify: `crates/zoid-tui/src/lib.rs`

**Interfaces:**
- Consumes: `ShellState`, `Focus`, `Overlay`, `DrawerId` (Tasks 2–3); `ShellLayout`, `in_rect` (Task 4); `Command`, `parse_command` (Task 5); palette `selectable_matches`/`nav` (Task 6).
- Produces:
  - `Action` enum (below).
  - `Target { Conversation, Input, DrawerHeader(DrawerId), None }`.
  - `route_key(&ShellState, KeyEvent) -> Action` — precedence overlay > global > focus-contextual.
  - `hit_test(&ShellLayout, col, row) -> Target`.
  - `route_mouse(&ShellState, &ShellLayout, MouseEvent) -> Action`.
  Consumed by the bin (Task 10) and the renderer indirectly. Note: palette/command-line are keyboard-driven (no palette-row hit-testing in P2 — Global Constraints).

- [ ] **Step 1: Declare the module**

In `lib.rs` add: `pub mod route;`

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tui/src/route.rs`:

```rust
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
```

- [ ] **Step 3: Run — expect pass**

Run: `cargo test -p zoid-tui route`
Expected: PASS after compile. (`MouseEvent` construction uses `ratatui::crossterm::event::MouseEvent` fields — confirm field names `kind/column/row/modifiers` against the crossterm version; adjust if the re-export differs.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): app-framework floor — contextual key routing + mouse hit-test"
```

---

## Task 8: Conversation-body extraction

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Produces: `pub fn conversation_lines<'a>(msgs: &'a [ChatMsg], streaming: bool) -> Vec<Line<'a>>` — the existing conversation-body builder, extracted so `render_shell` (Task 9) can draw it into the `conversation` rect. `render_chat` keeps working (delegates to the helper) so the existing 5 chat snapshots stay valid until Task 9 intentionally revisits them.

- [ ] **Step 1: Extract the body builder (refactor — behavior-preserving)**

In `crates/zoid-tui/src/chat.rs`, pull the `body: Vec<Line> = …` construction out of `render_chat` into a public function, and have `render_chat` call it:

```rust
/// Build the conversation lines (user/assistant turns + inline tool cards).
/// Shared by `render_chat` and the modal `render_shell`.
pub fn conversation_lines<'a>(msgs: &'a [ChatMsg], streaming: bool) -> Vec<Line<'a>> {
    let last = msgs.len().saturating_sub(1);
    if msgs.is_empty() {
        return vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))];
    }
    let mut lines: Vec<Line> = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        // ... move the existing per-`ChatMsg` match here verbatim ...
    }
    lines
}
```

Then in `render_chat`, replace the inline `let body = …` with:

```rust
    let body = conversation_lines(msgs, streaming);
    frame.render_widget(Paragraph::new(body), chunks[1]);
```

Keep everything else in `render_chat` (title, input box, status) byte-for-byte unchanged.

- [ ] **Step 2: Run the existing snapshots — expect unchanged**

Run: `cargo test -p zoid-tui`
Expected: PASS, **no snapshot changes** (pure refactor). If `insta` reports a diff, the extraction changed output — fix until the 5 existing snapshots are clean.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "refactor(tui): extract conversation_lines for reuse by render_shell"
```

---

## Task 9: `render_shell` — title/main/rail/input/status + overlays + Build placeholder

**Files:**
- Create: `crates/zoid-tui/src/render.rs`
- Modify: `crates/zoid-tui/src/lib.rs`
- Create: `crates/zoid-tui/tests/shell_snapshot.rs`

**Interfaces:**
- Consumes: `ShellState`/`Mode`/`Focus`/`Overlay`/`DrawerId` (Tasks 2–3), `compute`/`ShellLayout` (Task 4), `all_items`/`selectable_matches`/`nav` (Task 6), `conversation_lines` (Task 8), `tokens` (Task 1).
- Produces: `pub fn render_shell(frame: &mut Frame, state: &ShellState, msgs: &[ChatMsg], input: &TextArea, streaming: bool)`. Consumed by the bin (Task 10).

- [ ] **Step 1: Declare the module + re-exports**

In `lib.rs` add `pub mod render;` and convenience re-exports:

```rust
pub use render::render_shell;
pub use state::{DrawerId, Focus, Mode, Overlay, ShellState};
```

- [ ] **Step 2: Implement `render_shell`**

Create `crates/zoid-tui/src/render.rs`. It computes the layout once (the same `compute` hit-testing uses), then draws each region. Title and status are **mode-aware** (Chat=blue chip + `^P palette · ⇧Tab → Build`; Build=amber). The main area is the conversation (Chat) or a centered Build placeholder. The rail draws drawer headers (`▸`/`▾` + keybind) and, for open drawers, a small body (files list / branch label / economy & palette placeholders). Overlays (palette, command line) draw last over a `Clear`.

```rust
//! `render_shell` — the modal Chat/Build frame: mode-aware title + status, the
//! main surface (conversation or the P6 Build placeholder), the rail of drawers,
//! the input box, and palette / command-line overlays. Every glyph/color comes
//! from `tokens` (spec §16). Geometry comes from `layout::compute` — the same
//! rects mouse hit-testing uses.

use crate::chat::conversation_lines;
use crate::layout::{compute, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches, PaletteItem};
use crate::state::{DrawerId, Mode, Overlay, ShellState};
use crate::tokens::{color, glyph};
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;

pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    msgs: &[ChatMsg],
    input: &TextArea<'_>,
    streaming: bool,
) {
    let layout = compute(frame.area(), state);

    render_title(frame, state, layout.title);

    match state.mode {
        Mode::Chat => {
            let body = conversation_lines(msgs, streaming);
            frame.render_widget(Paragraph::new(body), layout.conversation);
        }
        Mode::Build => render_build_placeholder(frame, layout.conversation),
    }

    if let Some(rail) = layout.rail {
        render_rail(frame, state, &layout, rail);
    }

    render_input(frame, input, layout.input);
    render_status(frame, state, layout.status);

    // Overlays last, over a cleared region.
    if state.overlay == Overlay::Palette {
        if let Some(p) = layout.palette {
            render_palette(frame, state, p);
        }
    } else if state.overlay == Overlay::CommandLine {
        if let Some(c) = layout.cmdline {
            render_cmdline(frame, state, c);
        }
    }
}

fn render_title(frame: &mut Frame, state: &ShellState, area: Rect) {
    let (label, fg, bg) = match state.mode {
        Mode::Chat => ("CHAT", color::CHAT_ACCENT, color::CHAT_BG),
        Mode::Build => ("BUILD", color::BUILD_ACCENT, color::BUILD_BG),
    };
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled(format!(" {label} "), Style::new().fg(fg).bg(bg).bold()),
        Span::styled(format!(" {} {}", glyph::BRANCH, state.branch), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn render_build_placeholder(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(format!("  {} BUILD mode", glyph::RUNNING), Style::new().fg(color::BUILD_ACCENT).bold())]),
        Line::from(vec![Span::styled("  The autonomous loop arrives in P6.", Style::new().fg(color::DIM))]),
        Line::from(vec![Span::styled(format!("  {}Tab / :chat → back to Chat", glyph::SHIFT), Style::new().fg(color::DIM))]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_input(frame: &mut Frame, input: &TextArea<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(block, area);
    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(input, inner);
}

fn render_status(frame: &mut Frame, state: &ShellState, area: Rect) {
    let status = match state.mode {
        Mode::Chat => Line::from(vec![
            Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG)),
            Span::styled(
                format!(" {} main · ^P palette · {}Tab → Build · ^C quit", glyph::BRANCH, glyph::SHIFT),
                Style::new().fg(color::DIM),
            ),
        ]),
        Mode::Build => Line::from(vec![
            Span::styled(" BUILD ", Style::new().fg(color::BUILD_ACCENT).bg(color::BUILD_BG)),
            Span::styled(" phase —/— · esc → Chat", Style::new().fg(color::DIM)),
        ]),
    };
    frame.render_widget(Paragraph::new(status), area);
}

fn render_rail(frame: &mut Frame, state: &ShellState, layout: &ShellLayout, rail: Rect) {
    // Rail header.
    let head = Line::from(vec![
        Span::styled("chat rail", Style::new().fg(color::CHAT_ACCENT)),
    ]);
    frame.render_widget(Paragraph::new(head), Rect { x: rail.x + 1, y: rail.y, width: rail.width.saturating_sub(2), height: 1 });

    for (id, hr) in &layout.drawer_headers {
        let Some(d) = state.drawer(*id) else { continue };
        let chevron = if d.open { glyph::EXPANDED } else { glyph::COLLAPSED };
        let hdr = Line::from(vec![
            Span::styled(format!("{chevron} {}", d.title), Style::new().fg(if d.open { color::TXT } else { color::DIM })),
            Span::styled(format!("  {}", d.keybind), Style::new().fg(color::DIM)),
        ]);
        frame.render_widget(Paragraph::new(hdr), *hr);
        if d.open {
            let body_rect = Rect { x: hr.x + 2, y: hr.y + 1, width: hr.width.saturating_sub(2), height: 4 };
            frame.render_widget(Paragraph::new(drawer_body(*id, state)), body_rect);
        }
    }
}

fn drawer_body(id: DrawerId, state: &ShellState) -> Vec<Line<'static>> {
    match id {
        DrawerId::Files => {
            if state.files.is_empty() {
                vec![Line::styled("(empty)", Style::new().fg(color::DIM))]
            } else {
                state.files.iter().take(4).map(|f| Line::styled(f.clone(), Style::new().fg(color::TXT))).collect()
            }
        }
        DrawerId::Branch => vec![
            Line::from(vec![Span::styled(format!("{} {}", glyph::BRANCH, state.branch), Style::new().fg(color::BRANCH))]),
            Line::styled("full branch ops · P5", Style::new().fg(color::DIM)),
        ],
        DrawerId::Economy => vec![Line::styled("context economy · P3", Style::new().fg(color::DIM))],
        DrawerId::Palette => vec![Line::styled("press ^P to open", Style::new().fg(color::DIM))],
    }
}

fn render_palette(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(format!(" {} {} ", glyph::USER_TURN, state.palette.query), Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(block, area);

    let items = all_items(state.mode);
    let matches = selectable_matches(&items, &state.palette.query);
    let sel = nav(state.palette.selected, 0, matches.len());

    // Render the full grouped list; highlight the selected *selectable* row.
    let mut lines: Vec<Line> = Vec::new();
    let mut last_group = "";
    for (i, it) in items.iter().enumerate() {
        if it.group != last_group {
            lines.push(Line::styled(it.group.to_uppercase(), Style::new().fg(color::CHAT_ACCENT)));
            last_group = it.group;
        }
        let is_sel = matches.get(sel) == Some(&i);
        lines.push(palette_row_line(it, is_sel));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn palette_row_line(it: &PaletteItem, selected: bool) -> Line<'static> {
    let enabled = it.command.is_some();
    let base = if !enabled { color::DIM } else if selected { color::TXT } else { color::TXT };
    let mut style = Style::new().fg(base);
    if selected {
        style = style.bg(color::SEL_BG);
    }
    Line::from(vec![
        Span::styled(format!(" {} {}", it.icon, it.label), style),
        Span::styled(format!("  {}", it.hint), Style::new().fg(color::DIM)),
        Span::styled(format!("  {}", it.keybind), Style::new().fg(color::DIM)),
    ])
}

fn render_cmdline(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);
    let line = Line::from(vec![
        Span::styled(":", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(state.cmdline.buffer.clone(), Style::new().fg(color::TXT)),
        Span::styled(glyph::CARET.to_string(), Style::new().fg(color::CHAT_ACCENT)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
```

- [ ] **Step 3: Write the snapshot tests**

Create `crates/zoid-tui/tests/shell_snapshot.rs`:

```rust
use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Mode, Overlay, ShellState};

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_shell(f, state, msgs, &input, false)).unwrap();
    terminal.backend().to_string()
}

fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant { text: "an unwrapped lookup in the handler.".into(), tool_calls: vec![] },
    ]
}

#[test]
fn chat_with_rail_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn files_drawer_open_frame() {
    let mut s = ShellState::new();
    s.files = vec!["Cargo.toml".into(), "src".into(), "README.md".into()];
    s.toggle_drawer(DrawerId::Files);
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn palette_overlay_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Palette;
    s.palette.query = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn command_line_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::CommandLine;
    s.cmdline.buffer = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn build_placeholder_frame() {
    let mut s = ShellState::new();
    s.set_mode(Mode::Build);
    insta::assert_snapshot!(draw(&s, &[], 100, 24));
}
```

- [ ] **Step 4: Generate + visually verify snapshots against mockups**

Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: 5 new pending snapshots. Review each against its mockup — chat-with-rail & files-drawer vs `chat-mode.html` (rail at right: `▸ files ^F` etc.), palette vs `palette.html` (grouped, selected row highlighted, post-v1 rows dimmed), command line vs `palette.html` cmdline, Build placeholder vs the agreed amber surface. Then:

Run: `cargo insta accept`

- [ ] **Step 5: Run the whole crate — expect pass**

Run: `cargo test -p zoid-tui`
Expected: PASS (the 5 existing chat snapshots remain green — they render via `render_chat`, untouched).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/lib.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): render_shell — modal frame, rail, palette/cmdline overlays, Build placeholder"
```

---

## Task 10: Wire the modal shell into the binary

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Delete: `crates/zoid/src/input.rs`
- Modify: `crates/zoid/src/lib.rs`

**Interfaces:**
- Consumes: `zoid_tui::{render_shell, ShellState, Mode, Focus, DrawerId, Overlay}`, `zoid_tui::route::{Action, route_key, route_mouse, palette_selected_command}`, `zoid_tui::command::Command`, `zoid_tui::layout::compute`, `zoid_tui::palette::{all_items, selectable_matches, nav}`.
- Produces: a running modal TUI — keyboard + mouse, palette, command line, mode toggle, drawers — preserving the P1b agent-turn behavior.

- [ ] **Step 1: Drop the old input module**

Delete `crates/zoid/src/input.rs`. In `crates/zoid/src/lib.rs` remove `pub mod input;` (keep `pub mod agent;`).

Run: `cargo build -p zoid 2>&1 | head` — expect errors in `main.rs` (uses removed `classify`/`KeyAction`). That's the next step's work.

- [ ] **Step 2: Add `ShellState` to `App` and populate cwd + branch**

In `main.rs`, extend `App`:

```rust
struct App {
    session: SessionHandle,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    model: String,
    textarea: TextArea<'static>,
    streaming: bool,
    shell: zoid_tui::ShellState,
}
```

Add a helper to read the current branch cheaply (no `git2` until P5):

```rust
/// Best-effort current branch from `.git/HEAD` (`ref: refs/heads/<name>`); "main" otherwise.
fn current_branch() -> String {
    std::fs::read_to_string(".git/HEAD")
        .ok()
        .and_then(|s| s.trim().strip_prefix("ref: refs/heads/").map(|b| b.to_string()))
        .unwrap_or_else(|| "main".into())
}

/// Up to N entries of the cwd for the Files drawer (names only, sorted).
fn cwd_files(limit: usize) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(".")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names.truncate(limit);
    names
}
```

In `main()`, build the shell after `events`:

```rust
    let mut shell = zoid_tui::ShellState::new();
    shell.branch = current_branch();
    shell.files = cwd_files(64);
```

and add `shell,` to the `App { … }` initializer.

- [ ] **Step 3: Enable mouse capture (and restore it on exit)**

In `main()` extend the enter/leave sequences:

```rust
    use crossterm::event::{EnableMouseCapture, DisableMouseCapture};
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
```

and on teardown:

```rust
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
```

- [ ] **Step 4: Render via `render_shell` and route events**

Replace the body of `run`'s loop. Draw:

```rust
        terminal.draw(|f| {
            let msgs = conversation(&app.events);
            render_shell(f, &app.shell, &msgs, &app.textarea, app.streaming);
        })?;
```

(Drop the now-unused `render_chat` import; import `render_shell`.) Then handle terminal events through the router:

```rust
            maybe_term = term_events.next() => {
                match maybe_term {
                    Some(Ok(CEvent::Key(key))) => {
                        if handle_action(app, route_key(&app.shell, key)).await? {
                            return Ok(());
                        }
                    }
                    Some(Ok(CEvent::Mouse(me))) => {
                        let layout = compute(terminal.get_frame().area(), &app.shell);
                        let _ = handle_action(app, route_mouse(&app.shell, &layout, me)).await?;
                    }
                    Some(Ok(_)) => { /* resize: redraw next loop */ }
                    Some(Err(_)) | None => return Ok(()),
                }
            }
```

> If `terminal.get_frame()` is awkward mid-loop, cache the last drawn `Rect` instead: store `last_area: Rect` on `App`, set it inside the `draw` closure (`app`-free via a captured cell), or recompute `compute(terminal.size()?.into(), …)`. Simplest: `let area = terminal.size().map(|s| Rect{ x:0,y:0,width:s.width,height:s.height }).unwrap_or_default();` then `compute(area, &app.shell)`.
>
> **Type note:** `CEvent::Key(key)`/`CEvent::Mouse(me)` yield `crossterm::event::{KeyEvent, MouseEvent}` from the bin's `crossterm` 0.28; the router signatures use `ratatui::crossterm::event::{KeyEvent, MouseEvent}`. These are the *same* types because ratatui 0.29 re-exports crossterm 0.28 and the workspace pins crossterm 0.28 — Cargo unifies them, so `key`/`me` pass to `route_key`/`route_mouse` with no conversion. If a future ratatui bump changes the crossterm version, this is where the mismatch surfaces.

- [ ] **Step 5: Implement `handle_action` (the Action interpreter)**

Add a free async fn. It mutates `app.shell` for view actions and performs side effects (agent turn, quit, command exec) for the rest. Returns `Ok(true)` to quit.

```rust
async fn handle_action(app: &mut App, action: Action) -> Result<bool> {
    use zoid_tui::state::Overlay;
    match action {
        Action::Quit => return Ok(true),
        Action::SwitchMode => app.shell.toggle_mode(),
        Action::FocusNext => app.shell.focus_next(),
        Action::FocusRegion(f) => app.shell.focus = f,
        Action::OpenPalette => {
            app.shell.overlay = Overlay::Palette;
            app.shell.palette = Default::default();
        }
        Action::OpenCommandLine => {
            app.shell.overlay = Overlay::CommandLine;
            app.shell.cmdline = Default::default();
        }
        Action::CloseOverlay => app.shell.close_overlay(),
        Action::ToggleDrawer(id) => app.shell.toggle_drawer(id),
        Action::PaletteMove(d) => {
            let items = zoid_tui::palette::all_items(app.shell.mode);
            let n = zoid_tui::palette::selectable_matches(&items, &app.shell.palette.query).len();
            app.shell.palette.selected = zoid_tui::palette::nav(app.shell.palette.selected, d, n);
        }
        Action::PaletteChar(c) => {
            app.shell.palette.query.push(c);
            app.shell.palette.selected = 0;
        }
        Action::PaletteBackspace => {
            app.shell.palette.query.pop();
            app.shell.palette.selected = 0;
        }
        Action::PaletteRun => {
            let cmd = zoid_tui::route::palette_selected_command(&app.shell);
            app.shell.close_overlay();
            if let Some(c) = cmd {
                return exec_command(app, c).await;
            }
        }
        Action::CmdlineChar(c) => app.shell.cmdline.buffer.push(c),
        Action::CmdlineBackspace => { app.shell.cmdline.buffer.pop(); }
        Action::RunCommand(c) => {
            app.shell.close_overlay();
            return exec_command(app, c).await;
        }
        Action::ScrollConversation(d) => {
            let next = app.shell.conversation_scroll as i32 + d;
            app.shell.conversation_scroll = next.max(0) as u16;
        }
        Action::Newline => app.textarea.insert_newline(),
        Action::Edit(key) => { app.textarea.input(key); }
        Action::Submit => {
            if app.streaming { return Ok(false); }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() { return Ok(false); }
            app.textarea = TextArea::default();
            app.record(EventKind::UserMessage { text }).await?;
            app.streaming = true;
            spawn_turn(app);
        }
        Action::Noop => {}
    }
    Ok(false)
}

async fn exec_command(app: &mut App, cmd: zoid_tui::command::Command) -> Result<bool> {
    use zoid_tui::command::Command;
    match cmd {
        Command::Quit => Ok(true),
        Command::SwitchMode(m) => { app.shell.set_mode(m); Ok(false) }
        Command::OpenDrawer(id) => { app.shell.open_drawer(id); Ok(false) }
        Command::Unknown(_) => Ok(false), // P2: silently ignore (status-line error message is a follow-up)
    }
}
```

Extract the existing turn-spawn block (from the old `Submit` arm) into a helper so `handle_action` stays readable:

```rust
fn spawn_turn(app: &App) {
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        let _ = run_agent_turn(provider, tools, session, seed, model, ui, now_ms).await;
    });
}
```

> The `ui_tx` sender currently lives as a local in `run`. Move it onto `App` (`ui_tx: mpsc::Sender<AgentUpdate>`) so `spawn_turn` can clone it; create the channel before constructing `App`, store the sender on `App`, keep the receiver local to `run`. (Clean up the `#[allow(unreachable_patterns)]`/unused-import guard once the real match compiles without it.)

- [ ] **Step 6: Keep the `AgentUpdate` receive arm**

The `Some(update) = ui_rx.recv()` arm is unchanged (`Appended` pushes the event; `TurnComplete` clears `streaming`).

- [ ] **Step 7: Build, lint, test**

Run: `cargo build -p zoid`
Expected: clean build.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 0 warnings. (Remove the temporary unused-import guard; make the `handle_action` match exhaustive without the `_` catch-all if clippy flags it.)

Run: `cargo test --workspace`
Expected: PASS — the existing `agent_loop.rs` tests are unaffected (they drive `run_agent_turn` directly, not the UI).

- [ ] **Step 8: Manual smoke (headless-safe)**

Run: `cargo run -p zoid` in a terminal. Verify: typing + `⏎` still talks to the provider; `Tab` cycles focus; `⇧Tab` flips to the amber Build placeholder and back; `^P` opens the palette (type "build", `⏎` switches mode); after `Tab` to the conversation, `:chat`/`:q` work; clicking a rail drawer header toggles it; mouse scroll moves the conversation. `^C` quits cleanly (terminal restored, mouse capture off). *(If no TTY is available, skip and note it — the routing is covered by Task 7's unit tests.)*

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid/src/lib.rs
git rm crates/zoid/src/input.rs
git commit -m "feat(zoid): drive the modal shell — route events, palette, command line, mouse"
```

---

## Definition of Done (whole branch)

- `cargo build --workspace --locked` — clean.
- `cargo test --workspace` — all pass (core proptests, tool tests, agent-loop, the 5 existing chat snapshots + 5 new shell snapshots, all pure-logic floor tests).
- `cargo clippy --all-targets -- -D warnings` — 0 warnings.
- No `Co-Authored-By` trailer on any commit.
- The binary runs as a modal TUI: focus ring (`Tab`), mode toggle (`⇧Tab`, amber Build placeholder), `^P` palette (fuzzy, grouped, post-v1 rows dimmed), `:` command line (when not editing the message), files & branch drawers, mouse focus + drawer-header toggle + scroll, persistent mode indicator in title + status bar.
- Snapshots visually match `docs/ux/chat-mode.html`, `palette.html`, `modes.html` (spec §16 fidelity).
- Deferred-and-documented: economy ⑤ content (P3), zoom ①/verbs ④/tree-sitter Ⓡ3/motion Ⓡ2 (P4), Build's real surface (P6), branch ops/`git2` (P5), palette-row mouse hit-testing, command-line error surfacing.
