//! `render_shell` — the modal Chat/Build frame: mode-aware title + status, the
//! main surface (conversation or the P6 Build placeholder), the rail of drawers,
//! the input box, and palette / command-line overlays. Every glyph/color comes
//! from `tokens` (spec §16). Geometry comes from `layout::compute` — the same
//! rects mouse hit-testing uses.

use crate::chat::{conversation_view, ChatView};
use crate::economy_view::EconomyView;
use crate::layout::{compute, ShellLayout};
use crate::palette::{all_items, nav, selectable_matches, PaletteItem};
use crate::state::{DrawerId, Focus, Mode, Overlay, ShellState};
use crate::tokens::{color, glyph};
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;

pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    msgs: &[ChatMsg],
    input: &TextArea<'_>,
    streaming: bool,
    view: &ChatView,
) {
    let layout = compute(frame.area(), state);

    render_title(frame, state, layout.title);

    match state.mode {
        Mode::Chat => {
            let body = conversation_view(msgs, view, streaming);
            // `trim: false` so indentation survives on wrapped continuation rows —
            // Detail altitude renders syntax-highlighted code whose leading space is
            // meaningful. Without wrap, ratatui clips any turn wider than the column
            // mid-word with no ellipsis. Scroll offset is row-based either way.
            frame.render_widget(
                Paragraph::new(body)
                    .wrap(Wrap { trim: false })
                    .scroll((state.conversation_scroll, 0)),
                layout.conversation,
            );
        }
        Mode::Build => render_build_placeholder(frame, layout.conversation),
    }

    if let Some(rail) = layout.rail {
        render_rail(frame, state, economy, &layout, rail);
    }

    render_input(frame, input, layout.input);
    render_status(frame, state, view, layout.status);

    // Overlays last, over a cleared region.
    if state.overlay == Overlay::Palette {
        if let Some(p) = layout.palette {
            render_palette(frame, state, p);
        }
    } else if state.overlay == Overlay::CommandLine {
        if let Some(c) = layout.cmdline {
            render_cmdline(frame, state, c);
        }
    } else if state.overlay == Overlay::Objects {
        if let Some(p) = layout.palette {
            render_object_overlay(frame, msgs, state, p);
        }
    } else if state.overlay == Overlay::Verbs {
        if let Some(p) = layout.palette {
            render_verb_overlay(frame, msgs, state, p);
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

fn render_status(frame: &mut Frame, state: &ShellState, view: &ChatView, area: Rect) {
    let mut spans = match state.mode {
        Mode::Chat => vec![
            Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG)),
            Span::styled(
                format!(
                    " {} {} · zoom {} · ^P palette · {}Tab → Build · ^C quit",
                    glyph::BRANCH, state.branch, view.zoom.label(), glyph::SHIFT
                ),
                Style::new().fg(color::DIM),
            ),
        ],
        Mode::Build => vec![
            Span::styled(" BUILD ", Style::new().fg(color::BUILD_ACCENT).bg(color::BUILD_BG)),
            Span::styled(" phase —/— · esc → Chat", Style::new().fg(color::DIM)),
        ],
    };
    // Transient ④ hint (e.g. "queued · runs as a subagent in P5"), set by a
    // verb pick (P4d T4). Pure-renderer-readable since it lives on ShellState.
    if let Some(hint) = &state.status_hint {
        spans.push(Span::styled(format!(" · {hint}"), Style::new().fg(color::DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_rail(frame: &mut Frame, state: &ShellState, economy: &EconomyView, layout: &ShellLayout, rail: Rect) {
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
        ]);
        frame.render_widget(Paragraph::new(hdr), *hr);
        if d.open {
            let body = layout.drawer_bodies.iter().find(|(bid, _)| bid == id).map(|(_, r)| *r);
            if let Some(rect) = body {
                if d.id == DrawerId::Economy {
                    render_economy_body(frame, economy, rect, state.focus == Focus::Rail);
                } else {
                    frame.render_widget(Paragraph::new(drawer_body(d.id, state)), rect);
                }
            }
        }
    }
}

fn render_economy_body(frame: &mut Frame, econ: &EconomyView, area: Rect, rail_focused: bool) {
    use crate::economy_view::{heat_bar, heat_color};
    use crate::text::{pad_to, truncate};
    let mut lines: Vec<Line> = Vec::new();
    let max_rows = area.height.saturating_sub(2) as usize; // leave room for churn + footer
    let shown = econ.rows.iter().take(max_rows);
    // Both the label and token columns are arbitrary-width strings, and Rust's
    // `{:<N}`/`{:>N}` only *pad* to a minimum — they never cap. So a long label
    // (or an unexpectedly wide token) used to overflow, shove the heat-bar off the
    // rail, and clip at the terminal edge. Fit the label to a budget = rail width
    // minus every fixed column after it, and derive the token column width from the
    // rows actually shown (so it can't overflow no matter what `human_tokens`
    // emits) rather than a hardcoded 4:
    //   marker(1) + space(1) + space(1) + tokens(tok_w) + space(1) + heat_bar(2)
    //   + " cold"(5, reserved for every row so the label column aligns whether or
    //   not a row is cold).
    // `.max(4)` keeps the historical 4-col token column when tokens are narrow;
    // `.max(3)` guards a pathologically narrow rail (unreachable at today's fixed
    // RAIL_WIDTH, but keeps the math sound if the rail ever grows responsive).
    let tok_w = shown.clone().map(|r| r.tokens.chars().count()).max().unwrap_or(4).max(4);
    let label_budget = (area.width as usize).saturating_sub(11 + tok_w).max(3);
    for (i, r) in shown.enumerate() {
        let marker = if r.pinned { glyph::PIN } else { ' ' };
        let sel = rail_focused && i == econ.selected;
        let base = if sel { Style::new().bg(color::SEL_BG) } else { Style::new() };
        let label = pad_to(&truncate(&r.label, label_budget), label_budget);
        let mut spans = vec![
            Span::styled(format!("{marker} {label} {:>tok_w$} ", r.tokens), base.fg(color::TXT)),
            Span::styled(heat_bar(r.heat), base.fg(heat_color(r.heat))),
        ];
        if r.cold {
            spans.push(Span::styled(" cold", base.fg(color::HEAT_COLD)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(vec![
        Span::styled("churn ", Style::new().fg(color::DIM)),
        Span::styled(econ.churn.clone(), Style::new().fg(color::CHAT_ACCENT)),
    ]));
    let check = if econ.auto_evict_cold { "[x]" } else { "[ ]" };
    let left = format!("{check} evict cold");
    let ledger_color = if econ.over_ceiling { color::WARN } else { color::DIM };
    let pad = (area.width as usize)
        .saturating_sub(left.chars().count() + econ.ledger.chars().count());
    lines.push(Line::from(vec![
        Span::styled(left, Style::new().fg(color::DIM)),
        Span::styled(format!("{}{}", " ".repeat(pad), econ.ledger), Style::new().fg(ledger_color)),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
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
    let mut last_group = String::new();
    for (i, it) in items.iter().enumerate() {
        if it.group != last_group {
            lines.push(Line::styled(it.group.to_uppercase(), Style::new().fg(color::CHAT_ACCENT)));
            last_group = it.group.clone();
        }
        let is_sel = matches.get(sel) == Some(&i);
        lines.push(palette_row_line(it, is_sel));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn palette_row_line(it: &PaletteItem, selected: bool) -> Line<'static> {
    let enabled = it.command.is_some();
    let fg = if enabled { color::TXT } else { color::DIM };
    let bg = |s: Style| if selected { s.bg(color::SEL_BG) } else { s };
    Line::from(vec![
        Span::styled(format!(" {} {}", it.icon, it.label), bg(Style::new().fg(fg))),
        Span::styled(format!("  {}", it.hint), bg(Style::new().fg(color::DIM))),
        Span::styled(format!("  {}", it.keybind), bg(Style::new().fg(color::DIM))),
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

/// A bordered, titled, single-selection list — shared by the object and verb
/// pickers (spec ④). Same chrome as the palette overlay.
fn list_overlay(frame: &mut Frame, area: Rect, title: String, rows: &[String], selected: usize) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(title, Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(block, area);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == selected {
                Style::new().fg(color::TXT).bg(color::SEL_BG)
            } else {
                Style::new().fg(color::TXT)
            };
            Line::from(Span::styled(format!(" {r}"), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_object_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::selectable_objects;
    let objs = selectable_objects(msgs);
    let sel = nav(state.objects.obj_selected, 0, objs.len());
    let rows: Vec<String> = objs.iter().map(object_row).collect();
    let rows = if rows.is_empty() { vec!["(no objects yet)".to_string()] } else { rows };
    list_overlay(frame, area, format!(" {} select object ", glyph::OPEN), &rows, sel);
}

fn render_verb_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::{selectable_objects, verbs_for};
    let objs = selectable_objects(msgs);
    let sel_obj = nav(state.objects.obj_selected, 0, objs.len());
    let (title, rows) = match objs.get(sel_obj) {
        Some(o) => (
            format!(" {} verbs · {} ", glyph::RECIPE, o.label),
            verbs_for(o.kind).iter().map(|v| v.to_string()).collect::<Vec<_>>(),
        ),
        None => (" verbs ".to_string(), vec!["(no object)".to_string()]),
    };
    let sel = nav(state.objects.verb_selected, 0, rows.len());
    list_overlay(frame, area, title, &rows, sel);
}

fn object_row(o: &crate::objects::Obj) -> String {
    use crate::objects::ObjectKind;
    let g = match o.kind {
        ObjectKind::File => glyph::OPEN,
        ObjectKind::Symbol => glyph::EDIT,
        ObjectKind::Error => glyph::WARNING,
    };
    format!("{g} {}", o.label)
}
