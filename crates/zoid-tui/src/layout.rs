//! Pure layout geometry. `compute` is the single source of rects, shared by the
//! renderer (draws into them) and mouse hit-testing (maps points to them) — so a
//! click and a draw can never disagree (spec §13/§16). No `Frame`, no I/O.

use crate::state::{Drawer, DrawerId, Overlay, ShellState};
use ratatui::layout::{Constraint, Layout, Rect};

/// Rail width in columns. Widened ~50% from the mockup's ≈30 so context labels,
/// churn, and branch/file drawers have breathing room (spec min ≈ 28).
pub const RAIL_WIDTH: u16 = 45;
/// Left/right breathing pad inside the conversation column (spec §3.5). The
/// stream's text is inset by this many columns on each side so turns don't sit
/// flush against the frame edge.
pub const CONV_PAD: u16 = 2;

/// The prose word-wrap width for a conversation rect of the given width: the
/// rect minus the left+right [`CONV_PAD`]. Shared by the renderer (which insets
/// the same amount) and the bin's zoom-reveal line measurement so both agree.
pub fn conv_text_width(conv_width: u16) -> u16 {
    conv_width.saturating_sub(CONV_PAD * 2)
}
/// Repo drawer body rows: name+branch · worktree · changes.
pub const REPO_BODY_ROWS: u16 = 3;
/// Session drawer body rows: name · model·provider · tok·cac·tps · ctx · cwd.
pub const SESSION_BODY_ROWS: u16 = 5;
/// Context drawer body rows: items + the churn/cache sparkline line (the manual
/// evict toggle and token-budget line were removed — observe-only drawer).
pub const CONTEXT_BODY_ROWS: u16 = 7;
/// Tasks drawer body rows: up to a handful of the model's current tasks.
pub const TASKS_BODY_ROWS: u16 = 5;
/// Subagents drawer body rows: up to a handful of in-flight subagents.
pub const SUBAGENTS_BODY_ROWS: u16 = 5;
/// Message-box max content rows before it stops growing and scrolls internally
/// (spec §2.2). Not a §16 token — a numeric layout constant, like RAIL_WIDTH.
pub const MAX_INPUT_ROWS: u16 = 8;

/// Total input-box height (content + top/bottom borders) for a wrapped line
/// count: grows with content, clamps at MAX_INPUT_ROWS, min one content row.
pub fn input_height(lines: u16) -> u16 {
    lines.clamp(1, MAX_INPUT_ROWS) + 2
}

/// Body height for a drawer kind.
pub fn drawer_body_rows(id: DrawerId) -> u16 {
    match id {
        DrawerId::Repo => REPO_BODY_ROWS,
        DrawerId::Session => SESSION_BODY_ROWS,
        DrawerId::Context => CONTEXT_BODY_ROWS,
        DrawerId::Tasks => TASKS_BODY_ROWS,
        DrawerId::Subagents => SUBAGENTS_BODY_ROWS,
    }
}

/// Hard minimum terminal size. Below this, a "too small" message is rendered
/// instead of the normal shell layout (no degradation/collapse path).
pub const MIN_WIDTH: u16 = 160;
pub const MIN_HEIGHT: u16 = 40;

/// Resolve each open drawer's body-row count for a rail `height` rows tall.
/// Every open drawer gets its full [`drawer_body_rows`]; closed drawers get 0.
/// No collapse/fill — the hard 160×40 minimum guarantees there's always room.
/// `task_count` lets the Tasks drawer grow beyond its base when it has more
/// items (unused now that degradation is gone, but kept for future content-
/// driven sizing).
pub fn allocate_drawer_bodies(
    drawers: &[Drawer],
    _height: u16,
    task_count: u16,
) -> Vec<u16> {
    drawers
        .iter()
        .map(|d| {
            if !d.open {
                return 0;
            }
            match d.id {
                DrawerId::Tasks => drawer_body_rows(d.id).min(task_count.max(1)),
                _ => drawer_body_rows(d.id),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLayout {
    pub title: Rect,
    pub body: Rect,
    pub conversation: Rect,
    pub rail: Option<Rect>,
    pub drawer_headers: Vec<(DrawerId, Rect)>,
    pub drawer_bodies: Vec<(DrawerId, Rect)>,
    pub input: Rect,
    pub status: Rect,
    pub palette: Option<Rect>,
    /// The peek popup rect (centered over the conversation at 65% height).
    /// `None` when no peek popup is open.
    pub peek: Option<Rect>,
}

/// True when (col,row) falls inside `r` (half-open on right/bottom).
pub fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

pub fn compute(area: Rect, state: &ShellState) -> ShellLayout {
    // Hard minimum: below 160×40, render only the "too small" message.
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return ShellLayout {
            title: Rect { x: area.x, y: area.y, width: area.width, height: 1 },
            body: Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(1) },
            conversation: Rect::default(),
            rail: None,
            drawer_headers: Vec::new(),
            drawer_bodies: Vec::new(),
            input: Rect::default(),
            status: Rect::default(),
            palette: None,
            peek: None,
        };
    }
    // Vertical: title(1) · body(min) · input(grows with content) · status(1).
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_height(state.input_rows)),
        Constraint::Length(1),
    ])
    .split(area);
    let (title, body, input, status) = (rows[0], rows[1], rows[2], rows[3]);

    let show_rail = state.rail_visible;
    let rail_w = if show_rail { RAIL_WIDTH } else { 0 };
    // The conversation fills all width up to the rail (no measure cap): on wide
    // terminals the stream grows to use the screen rather than leaving a gutter.
    let conv_w = body.width.saturating_sub(rail_w);

    let cols =
        Layout::horizontal([Constraint::Length(conv_w), Constraint::Length(rail_w)]).split(body);
    let conversation = cols[0];
    let rail = if show_rail { Some(cols[1]) } else { None };

    // Drawer box rects: each drawer renders as a rounded bordered box (spec
    // `docs/ux/chat-mode.html` `.drawer`), stacked from the rail top (1-col
    // inset; no "chat rail" label — it was removed). `drawer_headers` holds
    // the box's OUTER rect (top border carries the title); `drawer_bodies`
    // holds the INNER content rect for open drawers.
    let mut drawer_headers = Vec::new();
    let mut drawer_bodies = Vec::new();
    if let Some(rr) = rail {
        let inner = Rect {
            x: rr.x.saturating_add(1),
            y: rr.y,
            width: rr.width.saturating_sub(2),
            height: rr.height,
        };
        // Resolve each open drawer's body rows: every open drawer gets its
        // full body height (no collapse/fill — the 160×40 minimum guarantees
        // the rail has room for all drawers).
        let bodies = allocate_drawer_bodies(&state.drawers, inner.height, state.tasks_len);
        let mut y = inner.y;
        let bottom = inner.y.saturating_add(inner.height);
        for (d, &body) in state.drawers.iter().zip(bodies.iter()) {
            let avail = bottom.saturating_sub(y);
            if avail < 2 {
                break; // no room for even an empty box (top+bottom border)
            }
            // `body == 0` renders a header-only box (user-closed drawer).
            let want = if body > 0 { body + 2 } else { 2 };
            let box_h = want.min(avail); // final safety clamp against the rail bottom
            drawer_headers.push((
                d.id,
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: box_h,
                },
            ));
            let body_rows = box_h.saturating_sub(2); // inner height after top+bottom borders
            if body > 0 && body_rows > 0 {
                drawer_bodies.push((
                    d.id,
                    Rect {
                        x: inner.x.saturating_add(1),
                        y: y.saturating_add(1),
                        width: inner.width.saturating_sub(2),
                        height: body_rows,
                    },
                ));
            }
            y = y.saturating_add(box_h + 1); // 1-row gap between boxes (mock margin-bottom)
        }
    }

    // Overlays (rendered on top; rects only — content in Task 8). Center within
    // the conversation rect, not the whole terminal, so the picker never overlaps
    // the rail (overlays draw last and would otherwise clip the rail's drawers).
    // `centered` clamps the width to its area, so the box shrinks to fit a narrow
    // stream rather than bleeding right into the rail.
    // Exhaustive match (NOT `matches!`/`if`): every overlay must declare its
    // modal-rect policy here. A new `Overlay` variant that captures keys but
    // forgets its rect would render nothing (invisible overlay) — the compiler
    // now rejects that omission instead of silently falling through to `None`.
    let palette = match state.overlay {
        Overlay::Palette
        | Overlay::Objects
        | Overlay::Verbs
        | Overlay::Sessions
        | Overlay::Mcp
        | Overlay::Feedback
        | Overlay::PluginCatalog => Some(centered(conversation, 72, 18)),
        Overlay::Help => Some(centered(
            conversation,
            crate::help::HELP_RECT_W,
            crate::help::HELP_RECT_H,
        )),
        // Config and ProviderSwitch draw full-frame (`frame.area()`), so they
        // need no centered palette rect; None has no overlay at all.
        Overlay::Config | Overlay::ProviderSwitch | Overlay::None => None,
    };

    let peek = if state.peek.is_some() {
        let max_h = (conversation.height as f32 * 0.65).floor() as u16;
        Some(centered(conversation, conversation.width, max_h))
    } else {
        None
    };

    ShellLayout {
        title,
        body,
        conversation,
        rail,
        drawer_headers,
        drawer_bodies,
        input,
        status,
        palette,
        peek,
    }
}

/// A rect `w×h` (clamped to `area`) centered horizontally, near the top third.
pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x.saturating_add((area.width.saturating_sub(w)) / 2);
    let y = area.y.saturating_add((area.height.saturating_sub(h)) / 3);
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ShellState;

    fn area(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    /// The five default drawers, all open, in rail order.
    fn all_open() -> Vec<Drawer> {
        [
            DrawerId::Repo,
            DrawerId::Session,
            DrawerId::Context,
            DrawerId::Tasks,
            DrawerId::Subagents,
        ]
        .iter()
        .map(|&id| Drawer {
            id,
            title: String::new(),
            open: true,
        })
        .collect()
    }

    const REPO: usize = 0;
    const SESSION: usize = 1;
    const CONTEXT: usize = 2;
    const TASKS: usize = 3;
    const SUBAGENTS: usize = 4;

    #[test]
    fn alloc_open_drawers_get_full_body_at_baseline() {
        let body = allocate_drawer_bodies(&all_open(), 35, 3);
        assert_eq!(body[REPO], REPO_BODY_ROWS);
        assert_eq!(body[SESSION], SESSION_BODY_ROWS);
        assert_eq!(body[CONTEXT], CONTEXT_BODY_ROWS);
        assert_eq!(body[TASKS], 3, "3 tasks => 3 rows");
        assert_eq!(body[SUBAGENTS], SUBAGENTS_BODY_ROWS);
    }

    #[test]
    fn alloc_closed_drawers_take_no_body() {
        let mut drawers = all_open();
        drawers[CONTEXT].open = false;
        let body = allocate_drawer_bodies(&drawers, 35, 3);
        assert_eq!(body[CONTEXT], 0, "a closed drawer gets 0 body rows");
        assert_eq!(body[TASKS], 3);
    }

    #[test]
    fn alloc_empty_is_empty() {
        assert!(allocate_drawer_bodies(&[], 40, 5).is_empty());
    }

    #[test]
    fn wide_shows_rail_and_drawer_headers() {
        let s = ShellState::new();
        let l = compute(area(160, 40), &s);
        let rail = l.rail.expect("rail visible at 160 cols");
        assert_eq!(rail.width, RAIL_WIDTH);
        assert_eq!(l.drawer_headers.len(), 5);
        assert!(l.drawer_headers[1].1.y > l.drawer_headers[0].1.y);
    }

    #[test]
    fn stream_fills_to_rail_on_ultrawide() {
        let s = ShellState::new();
        let l = compute(area(200, 40), &s);
        assert_eq!(l.conversation.width, 200 - RAIL_WIDTH);
        assert_eq!(l.conversation.width + l.rail.unwrap().width, 200);
    }

    #[test]
    fn palette_rect_only_when_overlay_active() {
        let mut s = ShellState::new();
        assert!(compute(area(160, 40), &s).palette.is_none());
        s.overlay = Overlay::Palette;
        let l = compute(area(160, 40), &s);
        let p = l.palette.unwrap();
        assert!(in_rect(p, p.x + 1, p.y + 1));
        assert!(p.width <= 160 && p.height <= 40);
    }

    #[test]
    fn overlay_rect_present_for_object_and_verb_pickers() {
        for ov in [
            Overlay::Objects,
            Overlay::Verbs,
            Overlay::Sessions,
            Overlay::Mcp,
            Overlay::Help,
            Overlay::PluginCatalog,
        ] {
            let mut s = ShellState::new();
            s.overlay = ov;
            assert!(
                compute(area(160, 40), &s).palette.is_some(),
                "{ov:?} needs a rect"
            );
        }
    }

    #[test]
    fn in_rect_half_open_boundaries() {
        let r = Rect {
            x: 5,
            y: 10,
            width: 20,
            height: 8,
        };
        assert!(in_rect(r, 5, 10));
        assert!(in_rect(r, 24, 17));
        assert!(!in_rect(r, 25, 17));
        assert!(!in_rect(r, 24, 18));
        assert!(!in_rect(r, 4, 10));
        assert!(!in_rect(r, 5, 9));
    }

    #[test]
    fn rail_hidden_by_user_toggle() {
        let mut s = ShellState::new();
        s.rail_visible = false;
        let l = compute(area(160, 40), &s);
        assert!(l.rail.is_none());
        assert!(l.drawer_headers.is_empty());
    }

    #[test]
    fn open_drawer_gets_a_body_rect_sized_by_kind() {
        let mut s = ShellState::new();
        s.toggle_drawer(DrawerId::Repo);
        let l = compute(area(160, 40), &s);
        let context = l
            .drawer_bodies
            .iter()
            .find(|(id, _)| *id == DrawerId::Context)
            .unwrap()
            .1;
        let session = l
            .drawer_bodies
            .iter()
            .find(|(id, _)| *id == DrawerId::Session)
            .unwrap()
            .1;
        assert_eq!(context.height, CONTEXT_BODY_ROWS);
        assert_eq!(session.height, SESSION_BODY_ROWS);
        assert!(l.drawer_bodies.iter().all(|(id, _)| *id != DrawerId::Repo));
    }

    #[test]
    fn headers_stack_with_1row_gap() {
        let s = ShellState::new();
        let l = compute(area(160, 40), &s);
        let repo_hdr = l.drawer_headers[0].1;
        let session_hdr = l.drawer_headers[1].1;
        assert_eq!(session_hdr.y, repo_hdr.y + repo_hdr.height + 1);
    }

    #[test]
    fn input_height_grows_and_clamps() {
        assert_eq!(input_height(1), 3);
        assert_eq!(input_height(4), 6);
        assert_eq!(input_height(MAX_INPUT_ROWS), MAX_INPUT_ROWS + 2);
        assert_eq!(input_height(MAX_INPUT_ROWS + 5), MAX_INPUT_ROWS + 2);
        assert_eq!(input_height(0), 3);
    }

    #[test]
    fn compute_input_area_tracks_input_rows() {
        let mut s = ShellState::new();
        s.input_rows = 4;
        let l = compute(area(160, 40), &s);
        assert_eq!(l.input.height, input_height(4));
    }

    #[test]
    fn too_small_renders_message_not_shell() {
        let s = ShellState::new();
        let l = compute(area(80, 20), &s);
        assert!(l.rail.is_none());
        assert!(l.drawer_headers.is_empty());
        assert!(l.drawer_bodies.is_empty());
    }

    #[test]
    fn peek_rect_none_when_peek_closed() {
        let s = ShellState::new();
        let l = compute(area(160, 40), &s);
        assert!(l.peek.is_none());
    }

    #[test]
    fn peek_rect_some_when_peek_open() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        let l = compute(area(160, 40), &s);
        assert!(l.peek.is_some());
        let p = l.peek.unwrap();
        // 65% of a 38-row conversation area (40 - 1 title - 1 status = 38,
        // minus input height; roughly 34-36). Just check it's < conversation
        // height and > 0.
        assert!(p.height > 0);
        assert!(p.height <= l.conversation.height);
        // Centered: x should be at or within the conversation area.
        assert!(p.x >= l.conversation.x);
    }
}
