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
    let base = if enabled { color::TXT } else { color::DIM };
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
