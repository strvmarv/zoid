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
