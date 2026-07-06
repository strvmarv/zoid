//! Pure layout geometry. `compute` is the single source of rects, shared by the
//! renderer (draws into them) and mouse hit-testing (maps points to them) — so a
//! click and a draw can never disagree (spec §13/§16). No `Frame`, no I/O.

use crate::state::{Drawer, DrawerId, Overlay, ShellState};
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
pub const CONTEXT_BODY_ROWS: u16 = 7;
/// Tasks drawer body rows: up to a handful of the model's current tasks.
pub const TASKS_BODY_ROWS: u16 = 5;
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
    }
}

/// The fewest body rows an open drawer keeps before it is collapsed to a
/// header-only box. One row still shows the first/active item, so an open
/// drawer never silently vanishes — worst case it degrades to its title bar.
pub const MIN_DRAWER_BODY_ROWS: u16 = 1;

/// Rail-fit priority: a higher rank keeps its rows longer when the rail is
/// short. `Repo` is near-static (yields first); `Context`'s drawer is mostly a
/// decorative sparkline so it yields before `Session`'s dense facts
/// (model/duration/tokens/ctx/cwd); `Tasks` is the live plan (yields last). A
/// single ranking drives both directions — collapse walks it ascending, surplus
/// fill walks it descending.
fn drawer_fit_priority(id: DrawerId) -> u8 {
    match id {
        DrawerId::Repo => 0,
        DrawerId::Context => 1,
        DrawerId::Session => 2,
        DrawerId::Tasks => 3,
    }
}

/// Resolve each open drawer's rendered body-row count for a rail `height` rows
/// tall, returned in the SAME ORDER as `drawers`. A `0` means header-only:
/// either the user closed it, or the rail was too short and it was collapsed to
/// its title bar (lowest priority first). Every OPEN drawer that survives gets
/// at least [`MIN_DRAWER_BODY_ROWS`].
///
/// Allocation is three steps over the single [`drawer_fit_priority`] ranking:
/// 1. reserve the minimum box for every open drawer (`MIN + 2` borders) plus a
///    1-row gap per drawer; if that stack overflows `height`, collapse open
///    drawers to header-only boxes lowest-priority-first until it fits;
/// 2. pass 1 — grow each surviving drawer from its minimum to its *base* ideal
///    ([`drawer_body_rows`]), highest priority first;
/// 3. pass 2 — pour any leftover rows into the Tasks drawer beyond its base,
///    up to `task_count`, so the task list grows to show more of itself when
///    the rail has room the other drawers don't want.
///
/// `task_count` is the number of items the Tasks drawer would show (min 1 for
/// the "no tasks" line); it makes the Tasks drawer content-driven rather than
/// capped at [`TASKS_BODY_ROWS`].
pub fn allocate_drawer_bodies(drawers: &[Drawer], height: u16, task_count: u16) -> Vec<u16> {
    let n = drawers.len();
    if n == 0 {
        return Vec::new();
    }
    // One gap row per drawer, matching the positioning loop in `compute`
    // (which advances `box_h + 1` after every box). Slightly conservative — the
    // trailing gap is slack — but it guarantees the stack never overflows.
    let gaps = n as u16;

    // The Tasks drawer's ideal is content-driven; every other drawer is fixed.
    let base_ideal = |d: &Drawer| -> u16 {
        match d.id {
            DrawerId::Tasks => drawer_body_rows(d.id).min(task_count.max(1)),
            _ => drawer_body_rows(d.id),
        }
    };

    // Step 1: start every open drawer expanded, then collapse the lowest
    // priority ones until the minimum stack fits.
    let mut expanded: Vec<bool> = drawers.iter().map(|d| d.open).collect();
    let box_min = 2 + MIN_DRAWER_BODY_ROWS; // top border + 1 body row + bottom border
    let min_cost = |expanded: &[bool]| -> u16 {
        let boxes: u16 = expanded.iter().map(|&e| if e { box_min } else { 2 }).sum();
        boxes + gaps
    };
    let mut collapse_order: Vec<usize> = (0..n).filter(|&i| expanded[i]).collect();
    collapse_order.sort_by_key(|&i| drawer_fit_priority(drawers[i].id));
    for &i in &collapse_order {
        if min_cost(&expanded) <= height {
            break;
        }
        expanded[i] = false; // fit-collapse to a header (still visible)
    }

    // Step 2 (pass 1): every survivor gets its minimum, then grows to base
    // ideal highest-priority-first with whatever surplus remains.
    let mut body = vec![0u16; n];
    for (i, d) in drawers.iter().enumerate() {
        if expanded[i] {
            body[i] = MIN_DRAWER_BODY_ROWS.min(base_ideal(d));
        }
    }
    let mut surplus = height.saturating_sub(min_cost(&expanded));
    let mut fill_order: Vec<usize> = (0..n).filter(|&i| expanded[i]).collect();
    fill_order.sort_by_key(|&i| std::cmp::Reverse(drawer_fit_priority(drawers[i].id)));
    for &i in &fill_order {
        if surplus == 0 {
            break;
        }
        let room = base_ideal(&drawers[i]).saturating_sub(body[i]);
        let add = room.min(surplus);
        body[i] += add;
        surplus -= add;
    }

    // Step 3 (pass 2): leftover rows grow Tasks beyond its base toward the full
    // task count, so a long list uses space the other drawers don't want.
    if surplus > 0 {
        if let Some(i) = drawers.iter().position(|d| d.id == DrawerId::Tasks) {
            if expanded[i] {
                let room = task_count.max(1).saturating_sub(body[i]);
                body[i] += room.min(surplus);
            }
        }
    }

    body
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
        // Resolve every open drawer's body rows up front so a short rail never
        // silently starves the last drawer: minimums are guaranteed, the lowest
        // priority drawers collapse to headers first, and leftover rows grow the
        // Tasks list to fit its content (see `allocate_drawer_bodies`).
        let bodies = allocate_drawer_bodies(&state.drawers, inner.height, state.tasks_len);
        let mut y = inner.y;
        let bottom = inner.y.saturating_add(inner.height);
        for (d, &body) in state.drawers.iter().zip(bodies.iter()) {
            let avail = bottom.saturating_sub(y);
            if avail < 2 {
                break; // no room for even an empty box (top+bottom border)
            }
            // `body == 0` renders a header-only box (user-closed or fit-collapsed).
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
    let palette = if matches!(
        state.overlay,
        Overlay::Palette | Overlay::Objects | Overlay::Verbs | Overlay::Sessions
    ) {
        Some(centered(conversation, 72, 18))
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

    /// The four default drawers, all open, in rail order.
    fn all_open() -> Vec<Drawer> {
        [
            DrawerId::Repo,
            DrawerId::Session,
            DrawerId::Context,
            DrawerId::Tasks,
        ]
        .iter()
        .map(|&id| Drawer {
            id,
            title: String::new(),
            open: true,
        })
        .collect()
    }

    // The four drawers stack in this order; index 3 is always Tasks.
    const REPO: usize = 0;
    const SESSION: usize = 1;
    const CONTEXT: usize = 2;
    const TASKS: usize = 3;

    #[test]
    fn alloc_roomy_gives_every_drawer_its_base_ideal() {
        // A tall rail (baseline 160x40 ⇒ ~35 rail rows): everyone reaches base,
        // and with only 3 tasks Tasks stays at 3 (content-bounded, not 5).
        let body = allocate_drawer_bodies(&all_open(), 35, 3);
        assert_eq!(body[REPO], REPO_BODY_ROWS);
        assert_eq!(body[SESSION], SESSION_BODY_ROWS);
        assert_eq!(body[CONTEXT], CONTEXT_BODY_ROWS);
        assert_eq!(body[TASKS], 3, "3 tasks ⇒ 3 rows, no wasted space");
    }

    #[test]
    fn alloc_tight_favors_tasks_others_hold_minimum() {
        // ~19 rail rows (a 24-row terminal): min stack (16) fits, surplus 3 goes
        // to Tasks first (highest priority), others sit at the 1-row minimum.
        let body = allocate_drawer_bodies(&all_open(), 19, 8);
        assert_eq!(body[REPO], MIN_DRAWER_BODY_ROWS);
        assert_eq!(body[SESSION], MIN_DRAWER_BODY_ROWS);
        assert_eq!(body[CONTEXT], MIN_DRAWER_BODY_ROWS);
        assert_eq!(body[TASKS], 4, "surplus 3 above its own 1-row min ⇒ 4");
        // Every open drawer is still visible (>=1 row); none silently vanished.
        assert!(body.iter().all(|&r| r >= MIN_DRAWER_BODY_ROWS));
    }

    #[test]
    fn alloc_very_tight_collapses_lowest_priority_first() {
        // 14 rail rows: the minimum stack for four open drawers is 16, so the
        // two lowest-priority drawers (Repo, then Context) collapse to headers;
        // Session (dense facts) and Tasks keep their one guaranteed row.
        let body = allocate_drawer_bodies(&all_open(), 14, 8);
        assert_eq!(body[REPO], 0, "Repo collapses first (header-only)");
        assert_eq!(body[CONTEXT], 0, "Context (decorative) collapses second");
        assert_eq!(body[SESSION], MIN_DRAWER_BODY_ROWS);
        assert_eq!(body[TASKS], MIN_DRAWER_BODY_ROWS);
    }

    #[test]
    fn alloc_tasks_grows_with_content_into_leftover() {
        // Rail with room past every base ideal: pass-2 grows Tasks toward the
        // full task count using rows the other drawers don't want.
        // base bodies = 3+5+5+5 = 18; borders 8; gaps 4 ⇒ 30 to seat all bases.
        // At height 40 there are 10 leftover rows ⇒ Tasks climbs from 5 toward 12.
        let body = allocate_drawer_bodies(&all_open(), 40, 12);
        assert_eq!(body[REPO], REPO_BODY_ROWS);
        assert_eq!(body[SESSION], SESSION_BODY_ROWS);
        assert_eq!(body[CONTEXT], CONTEXT_BODY_ROWS);
        assert!(
            body[TASKS] > TASKS_BODY_ROWS,
            "12 tasks + spare rows ⇒ Tasks grows past its base of 5, got {}",
            body[TASKS]
        );
        assert!(body[TASKS] <= 12, "never exceeds the task count");
    }

    #[test]
    fn alloc_closed_drawers_take_no_body() {
        let mut drawers = all_open();
        drawers[CONTEXT].open = false;
        let body = allocate_drawer_bodies(&drawers, 35, 3);
        assert_eq!(body[CONTEXT], 0, "a user-closed drawer is header-only");
        assert_eq!(body[TASKS], 3);
    }

    #[test]
    fn alloc_empty_is_empty() {
        assert!(allocate_drawer_bodies(&[], 40, 5).is_empty());
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
        // At 100×24 the rail has ~19 inner rows. The fit allocator guarantees
        // every open drawer at least a header, so ALL FOUR now appear — the
        // Tasks drawer no longer silently vanishes when the rail is short
        // (the pre-allocator behavior dropped it here). This is the T8 fix.
        assert_eq!(l.drawer_headers.len(), 4); // repo/session/context/tasks all visible
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
        let s = ShellState::new(); // all drawers open by default; Repo is first
        let l = compute(area(100, 30), &s);
        let repo_hdr = l.drawer_headers[0].1;
        let session_hdr = l.drawer_headers[1].1;
        // Session's box sits exactly one gap row below Repo's box, whatever
        // height the fit allocator resolved Repo to (it may shrink below its
        // ideal when the rail is short — the gap invariant still holds).
        assert_eq!(session_hdr.y, repo_hdr.y + repo_hdr.height + 1);
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
