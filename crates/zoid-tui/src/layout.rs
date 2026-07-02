//! Pure layout geometry. `compute` is the single source of rects, shared by the
//! renderer (draws into them) and mouse hit-testing (maps points to them) — so a
//! click and a draw can never disagree (spec §13/§16). No `Frame`, no I/O.

use crate::state::{DrawerId, Overlay, ShellState};
use ratatui::layout::{Constraint, Layout, Rect};

/// Rail width in columns. Widened ~50% from the mockup's ≈30 so context labels,
/// churn, and branch/file drawers have breathing room (spec min ≈ 28).
pub const RAIL_WIDTH: u16 = 45;
/// Minimum total width before the rail is shown: keep a usable stream (≥ ~50)
/// alongside the wider rail, so the rail only appears once both fit (spec §6.2).
pub const RAIL_MIN_TOTAL: u16 = 95;
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
/// Session drawer body rows: name · model·provider · dur·tok · ctx · cwd.
pub const SESSION_BODY_ROWS: u16 = 5;
/// Context drawer body rows: items + the churn/cache sparkline line (the manual
/// evict toggle and token-budget line were removed — observe-only drawer).
pub const CONTEXT_BODY_ROWS: u16 = 5;
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
    }
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
    pub cmdline: Option<Rect>,
}

/// True when (col,row) falls inside `r` (half-open on right/bottom).
pub fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

pub fn compute(area: Rect, state: &ShellState) -> ShellLayout {
    // Vertical: title(1) · body(min) · input(grows with content) · status(1).
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_height(state.input_rows)),
        Constraint::Length(1),
    ])
    .split(area);
    let (title, body, input, status) = (rows[0], rows[1], rows[2], rows[3]);

    let show_rail = state.rail_visible && area.width >= RAIL_MIN_TOTAL;
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
        let mut y = inner.y;
        let bottom = inner.y.saturating_add(inner.height);
        for d in &state.drawers {
            let avail = bottom.saturating_sub(y);
            if avail < 2 {
                break; // no room for even an empty box (top+bottom border)
            }
            let want = if d.open {
                drawer_body_rows(d.id) + 2
            } else {
                2
            };
            let box_h = want.min(avail); // clamp so the box (incl. bottom border) stays in the rail
            drawer_headers.push((
                d.id,
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: box_h,
                },
            ));
            if d.open {
                let body_rows = box_h.saturating_sub(2); // inner height after top+bottom borders
                if body_rows > 0 {
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
            }
            y = y.saturating_add(box_h + 1); // 1-row gap between boxes (mock margin-bottom)
        }
    }

    // Overlays (rendered on top; rects only — content in Task 8). Center within
    // the conversation rect, not the whole terminal, so the picker never overlaps
    // the rail (overlays draw last and would otherwise clip the rail's drawers).
    // `centered` clamps the width to its area, so the box shrinks to fit a narrow
    // stream rather than bleeding right into the rail.
    let palette = if matches!(
        state.overlay,
        Overlay::Palette | Overlay::Objects | Overlay::Verbs | Overlay::Sessions
    ) {
        Some(centered(conversation, 72, 18))
    } else {
        None
    };
    let cmdline = if state.overlay == Overlay::CommandLine {
        Some(Rect {
            x: area.x,
            y: status.y,
            width: area.width,
            height: 1,
        })
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
        cmdline,
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

    #[test]
    fn narrow_hides_rail() {
        let s = ShellState::new();
        let l = compute(area(60, 12), &s);
        assert!(l.rail.is_none());
        assert!(l.drawer_headers.is_empty());
        assert!(l.drawer_bodies.is_empty());
        // conversation spans the full body width when there's no rail/gutter.
        assert_eq!(l.conversation.width, 60);
    }

    #[test]
    fn wide_shows_rail_and_drawer_headers() {
        let s = ShellState::new();
        let l = compute(area(100, 24), &s);
        let rail = l.rail.expect("rail visible at 100 cols");
        assert_eq!(rail.width, RAIL_WIDTH);
        assert_eq!(l.drawer_headers.len(), 3); // repo/session/context
                                               // headers stack downward
        assert!(l.drawer_headers[1].1.y > l.drawer_headers[0].1.y);
    }

    #[test]
    fn stream_fills_to_rail_on_ultrawide() {
        let s = ShellState::new();
        let l = compute(area(200, 24), &s);
        // No measure cap: the stream expands to all width up to the rail, and
        // stream + rail together span the full terminal (no gutter).
        assert_eq!(l.conversation.width, 200 - RAIL_WIDTH);
        assert_eq!(l.conversation.width + l.rail.unwrap().width, 200);
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

    #[test]
    fn overlay_rect_present_for_object_and_verb_pickers() {
        for ov in [Overlay::Objects, Overlay::Verbs] {
            let mut s = ShellState::new();
            s.overlay = ov;
            assert!(
                compute(area(100, 24), &s).palette.is_some(),
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
        assert!(in_rect(r, 5, 10)); // top-left corner: inside
        assert!(in_rect(r, 24, 17)); // bottom-right interior corner: inside
        assert!(!in_rect(r, 25, 17)); // right edge: exclusive
        assert!(!in_rect(r, 24, 18)); // bottom edge: exclusive
        assert!(!in_rect(r, 4, 10)); // left of rect: outside
        assert!(!in_rect(r, 5, 9)); // above rect: outside
    }

    #[test]
    fn rail_hidden_by_user_toggle() {
        let mut s = ShellState::new();
        s.rail_visible = false;
        let l = compute(area(100, 24), &s);
        assert!(l.rail.is_none());
        assert!(l.drawer_headers.is_empty());
    }

    #[test]
    fn open_drawer_gets_a_body_rect_sized_by_kind() {
        let mut s = ShellState::new(); // all three open by default
        s.toggle_drawer(DrawerId::Repo); // Repo now closed
        let l = compute(area(100, 30), &s);
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
        // closed drawers have no body
        assert!(l.drawer_bodies.iter().all(|(id, _)| *id != DrawerId::Repo));
        // body sits directly under the box's top border (which carries the title)
        let context_hdr = l
            .drawer_headers
            .iter()
            .find(|(id, _)| *id == DrawerId::Context)
            .unwrap()
            .1;
        assert_eq!(context.y, context_hdr.y + 1);
    }

    #[test]
    fn headers_stack_below_taller_economy_body() {
        let s = ShellState::new(); // all three open by default; Repo is first
        let l = compute(area(100, 30), &s);
        let repo_hdr = l.drawer_headers[0].1;
        let session_hdr = l.drawer_headers[1].1;
        // box_h(top border + REPO_BODY_ROWS + bottom border) + 1-row gap
        assert_eq!(session_hdr.y, repo_hdr.y + (REPO_BODY_ROWS + 2) + 1);
    }

    #[test]
    fn input_height_grows_and_clamps() {
        assert_eq!(
            input_height(1),
            3,
            "one line → 3 rows (content + 2 borders); post-submit resting height"
        );
        assert_eq!(input_height(4), 6, "grows with content");
        assert_eq!(
            input_height(MAX_INPUT_ROWS),
            MAX_INPUT_ROWS + 2,
            "at the cap"
        );
        assert_eq!(
            input_height(MAX_INPUT_ROWS + 5),
            MAX_INPUT_ROWS + 2,
            "clamps past the cap"
        );
        assert_eq!(input_height(0), 3, "min one content row");
    }

    #[test]
    fn compute_input_area_tracks_input_rows() {
        let mut s = ShellState::new();
        s.input_rows = 4;
        let l = compute(area(100, 30), &s);
        assert_eq!(l.input.height, input_height(4));
    }
}
