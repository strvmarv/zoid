//! `render_shell` — the modal Chat/Build frame: mode-aware title + status, the
//! main surface (conversation or the P6 Build placeholder), the rail of drawers,
//! the input box, and palette / command-line overlays. Every glyph/color comes
//! from `tokens` (spec §16). Geometry comes from `layout::compute` — the same
//! rects mouse hit-testing uses.

use crate::chat::{conversation_view, ChatView};
use crate::economy_view::EconomyView;
use crate::layout::{compute, ShellLayout, CONV_PAD};
use crate::palette::{all_items, nav, selectable_matches, PaletteItem};
use crate::state::{DrawerId, Focus, Mode, Overlay, ShellState};
use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Margin, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;
use zoid_core::projection::ChatMsg;

#[allow(clippy::too_many_arguments)]
pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    msgs: &[ChatMsg],
    // The pre-rendered conversation body (full, `reveal == None`), when the caller
    // has cached it (the hot path — `conversation_view` is the expensive wrap +
    // syntax-highlight pass). `None` falls back to rendering it here, which keeps
    // test/example call sites simple. Either way the zoom-reveal truncation + the
    // in-flight tool spinner are applied here before scroll/paint.
    body: Option<&[Line<'static>]>,
    tasks: &[zoid_core::tasks::TaskItem],
    input: &TextArea<'_>,
    streaming: bool,
    view: &ChatView,
) -> u16 {
    let layout = compute(frame.area(), state);
    // The max conversation scroll offset at the current altitude, returned so the
    // bin can clamp the STORED offset (not just the drawn one) and avoid a silent
    // dead-scroll-up zone. 0 unless the Chat conversation is drawn this frame.
    let mut conv_max_scroll = 0u16;

    // The top row is intentionally blank: the app name was removed from the
    // title bar and the activity indicator now lives in the status bar (right).
    // The empty row doubles as top breathing room.

    match state.mode {
        Mode::Chat => {
            // Inset the stream by CONV_PAD columns for left/right breathing room.
            // `conversation_view` has already wrapped prose (with the hanging
            // indent) and padded code to this exact width, so every line fits on
            // one row — we render WITHOUT widget wrap. That keeps a strict
            // 1-line-per-row transcript (exact mouse copy hit-testing) and avoids
            // ratatui's phantom-blank quirk on lines exactly at the width. The
            // rare over-wide code line clips; click-to-copy still yields its full
            // source, so nothing is lost to the clipboard.
            let text = layout.conversation.inner(Margin {
                horizontal: CONV_PAD,
                vertical: 0,
            });
            // Cached full render (reveal == None) → apply the zoom-reveal
            // truncation here (cheap take/clone, not a re-render). Without a cache
            // (tests), render it via conversation_view, which applies reveal itself.
            let mut body: Vec<Line<'static>> = match body {
                Some(full) => match view.reveal {
                    Some(n) => full.iter().take(n).cloned().collect(),
                    None => full.to_vec(),
                },
                None => conversation_view(msgs, view, streaming, text.width as usize),
            };
            // In-flight tool indicator: a dim spinner line below the last message,
            // above the input, shown while a Local tool call is running (cleared
            // once its `ToolResult` arrives or the turn completes). §16: glyph and
            // colors come from `tokens`, never a literal.
            if let Some(name) = &state.active_tool {
                body.push(Line::from(vec![
                    Span::styled(format!("{} ", glyph::RUNNING), Style::new().fg(color::WARN)),
                    Span::styled(
                        format!("running · {name} {}", glyph::ELLIPSIS),
                        Style::new().fg(color::DIM),
                    ),
                ]));
            }
            // Clamp the scroll offset to the body produced at THIS altitude. The three
            // zoom altitudes (Summary/Normal/Detail) yield very different line counts,
            // so an offset valid at one is meaningless at another; without this clamp a
            // large offset carried from a taller altitude renders past the end as blank
            // rows after a zoom switch (the "corrupted display" bug). Display-only — the
            // stored offset is reset to 0 on an actual zoom change.
            let max_scroll = body
                .len()
                .saturating_sub(text.height as usize)
                .min(u16::MAX as usize) as u16;
            conv_max_scroll = max_scroll;
            let scroll = state.conversation_scroll.min(max_scroll);
            frame.render_widget(Paragraph::new(body).scroll((scroll, 0)), text);
        }
        Mode::Build => render_build_placeholder(frame, layout.conversation),
    }

    if layout.rail.is_some() {
        render_rail(frame, state, economy, tasks, &layout);
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
    } else if state.overlay == Overlay::Sessions {
        if let Some(p) = layout.palette {
            render_sessions_overlay(frame, state, p);
        }
    } else if state.overlay == Overlay::Config {
        // render_config centers its own card within the given area.
        render_config(frame, state, &state.config_sections, frame.area());
    } else if state.overlay == Overlay::Question {
        if let Some(q) = &state.question {
            render_question(frame, frame.area(), q);
        }
    }
    conv_max_scroll
}

fn render_build_placeholder(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("  {} BUILD mode", glyph::RUNNING),
            Style::new().fg(color::BUILD_ACCENT).bold(),
        )]),
        Line::from(vec![Span::styled(
            "  The autonomous loop is coming soon.",
            Style::new().fg(color::DIM),
        )]),
        Line::from(vec![Span::styled(
            format!("  {}Tab / :chat → back to Chat", glyph::SHIFT),
            Style::new().fg(color::DIM),
        )]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_input(frame: &mut Frame, input: &TextArea<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(block, area);
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(input, inner);
}

fn render_status(frame: &mut Frame, state: &ShellState, view: &ChatView, area: Rect) {
    // Left segment: mode badge + hints.
    let mut left = match state.mode {
        Mode::Chat => vec![
            Span::styled(
                " CHAT ",
                Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG),
            ),
            Span::styled(
                format!(" zoom {} · ^P palette", view.zoom.label()),
                Style::new().fg(color::DIM),
            ),
        ],
        Mode::Build => vec![
            Span::styled(
                " BUILD ",
                Style::new().fg(color::BUILD_ACCENT).bg(color::BUILD_BG),
            ),
            Span::styled(" phase —/— · esc → Chat", Style::new().fg(color::DIM)),
        ],
    };
    // Transient ④ hint (e.g. "queued · runs as a subagent in P5"), set by a
    // verb pick (P4d T4). Pure-renderer-readable since it lives on ShellState.
    if let Some(hint) = &state.status_hint {
        left.push(Span::styled(
            format!(" · {hint}"),
            Style::new().fg(color::DIM),
        ));
    }

    // Center segment: live activity indicator (spec §2.2). Idle when waiting for
    // the user; an animated spinner (frame supplied by the bin) while a turn is
    // in flight — streaming OR delegating — so it's clear the agent is working
    // and not hung.
    let (icon, label, fg) = if state.busy {
        (state.spinner, "working", color::CHAT_ACCENT)
    } else {
        (glyph::IDLE, "idle", color::OK)
    };
    let center = format!("{icon} {label}");

    // Right segment: the wordmark, where the activity indicator used to live.
    let right = "zoid ";

    let w = area.width as usize;
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let center_w = center.width();
    let right_w = right.width();

    // Center the activity indicator in the bar and pin the wordmark to the right
    // edge. Saturating math means a narrow terminal clips the padding (segments
    // just abut) instead of panicking.
    let center_start = w.saturating_sub(center_w) / 2;
    let right_start = w.saturating_sub(right_w);

    let mut spans = left;
    let pad1 = center_start.saturating_sub(left_w);
    if pad1 > 0 {
        spans.push(Span::styled(" ".repeat(pad1), Style::new()));
    }
    spans.push(Span::styled(center, Style::new().fg(fg)));
    let pad2 = right_start.saturating_sub(left_w + pad1 + center_w);
    if pad2 > 0 {
        spans.push(Span::styled(" ".repeat(pad2), Style::new()));
    }
    spans.push(Span::styled(right, Style::new().fg(color::DIM)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_rail(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    tasks: &[zoid_core::tasks::TaskItem],
    layout: &ShellLayout,
) {
    // Each drawer is a rounded bordered box (spec `docs/ux/chat-mode.html`
    // `.drawer{border:1px solid var(--line2);border-radius:8px}`): border +
    // title are the Chat accent blue when open, dim when closed. No "chat
    // rail" head label — the boxes start at the rail top.
    for (id, boxr) in &layout.drawer_headers {
        let Some(d) = state.drawer(*id) else { continue };
        let chevron = if d.open {
            glyph::EXPANDED
        } else {
            glyph::COLLAPSED
        };
        let border = if d.open {
            color::CHAT_ACCENT
        } else {
            color::DIM
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border))
            .title(Span::styled(
                format!(" {chevron} {} ", d.title),
                Style::new().fg(color::CHAT_ACCENT),
            ));
        let inner = block.inner(*boxr);
        frame.render_widget(block, *boxr);
        if d.open {
            let body_rect = layout
                .drawer_bodies
                .iter()
                .find(|(bid, _)| bid == id)
                .map(|(_, r)| *r)
                .unwrap_or(inner);
            match d.id {
                DrawerId::Context => {
                    render_economy_body(frame, economy, body_rect, state.focus == Focus::Rail)
                }
                DrawerId::Repo => render_repo_body(frame, state, body_rect),
                DrawerId::Session => render_session_body(frame, state, body_rect), // Task 13
                DrawerId::Tasks => render_tasks_body(frame, body_rect, tasks),
            }
        }
    }
}

fn render_economy_body(frame: &mut Frame, econ: &EconomyView, area: Rect, rail_focused: bool) {
    use crate::economy_view::{heat_bar, heat_color, tail};
    use crate::text::{pad_to, truncate};
    let mut lines: Vec<Line> = Vec::new();
    let max_rows = area.height.saturating_sub(1) as usize; // leave room for the churn/cache line
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
    let tok_w = shown
        .clone()
        .map(|r| r.tokens.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let label_budget = (area.width as usize).saturating_sub(11 + tok_w).max(3);
    for (i, r) in shown.enumerate() {
        let marker = if r.pinned { glyph::PIN } else { ' ' };
        let sel = rail_focused && i == econ.selected;
        let base = if sel {
            Style::new().bg(color::SEL_BG)
        } else {
            Style::new()
        };
        let label = pad_to(&truncate(&r.label, label_budget), label_budget);
        let mut spans = vec![
            Span::styled(
                format!("{marker} {label} {:>tok_w$} ", r.tokens),
                base.fg(color::TXT),
            ),
            Span::styled(heat_bar(r.heat), base.fg(heat_color(r.heat))),
        ];
        if r.cold {
            spans.push(Span::styled(" cold", base.fg(color::HEAT_COLD)));
        }
        lines.push(Line::from(spans));
    }
    // churn sparkline (left) + per-turn prompt-cache sparkline (right). Both series
    // grow one cell per turn, so a long session would push the right-hand cache off
    // the rail. Window each to the last N turns, N = half the cells left after the
    // two fixed labels and a one-space gap — so cache stays visible at any length.
    // Cache is dimmed when the model/provider reported no cache reads. The manual
    // "evict cold" toggle and the token "budget" line were removed — eviction is
    // policy-driven and the drawer stays observe-only.
    let label_w = "churn ".chars().count(); // == "cache "
    let per_series = (area.width as usize).saturating_sub(2 * label_w + 1).max(2) / 2;
    let churn_s = tail(&econ.churn, per_series);
    let cache_s = tail(&econ.cache, per_series);
    let used = 2 * label_w + churn_s.chars().count() + cache_s.chars().count();
    let churn_pad = (area.width as usize).saturating_sub(used).max(1);
    let cache_color = if econ.cache_active {
        color::OK
    } else {
        color::DIM
    };
    lines.push(Line::from(vec![
        Span::styled("churn ", Style::new().fg(color::DIM)),
        Span::styled(churn_s, Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(" ".repeat(churn_pad), Style::new()),
        Span::styled("cache ", Style::new().fg(color::DIM)),
        Span::styled(cache_s, Style::new().fg(cache_color)),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_repo_body(frame: &mut Frame, state: &ShellState, area: Rect) {
    let name = if state.repo_name.is_empty() {
        "repo"
    } else {
        &state.repo_name
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::REPO_NAME),
                Style::new().fg(color::TXT),
            ),
            Span::styled(format!("{name}   "), Style::new().fg(color::TXT)),
            Span::styled(
                format!("{} {}", glyph::BRANCH, state.branch),
                Style::new().fg(color::BRANCH),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::REPO_WORKTREE),
                Style::new().fg(color::DIM),
            ),
            Span::styled("worktree ", Style::new().fg(color::DIM)),
            Span::styled(state.worktree.clone(), Style::new().fg(color::DIM)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::REPO_CHANGES),
                Style::new().fg(color::DIM),
            ),
            Span::styled("changes ", Style::new().fg(color::DIM)),
            Span::styled(
                format!("+{}", state.changes_added),
                Style::new().fg(color::ADDED),
            ),
            Span::styled(" ", Style::new()),
            Span::styled(
                format!("-{}", state.changes_removed),
                Style::new().fg(color::REMOVED),
            ),
            Span::styled(
                format!(" · {} files", state.changes_files),
                Style::new().fg(color::DIM),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_session_body(frame: &mut Frame, state: &ShellState, area: Rect) {
    use crate::economy_view::{gauge, human_tokens};
    use crate::text::truncate;
    let name = if state.session_name.is_empty() {
        "(unnamed)"
    } else {
        &state.session_name
    };
    let ctx = if state.ctx_ceiling > 0 {
        format!(
            "{}/{}",
            human_tokens(state.ctx_used),
            human_tokens(state.ctx_ceiling)
        )
    } else {
        human_tokens(state.ctx_used)
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::SESS_NAME),
                Style::new().fg(color::TXT),
            ),
            Span::styled(
                truncate(name, (area.width as usize).saturating_sub(2)),
                Style::new().fg(color::TXT),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::SESS_MODEL),
                Style::new().fg(color::CHAT_ACCENT),
            ),
            Span::styled(state.model.clone(), Style::new().fg(color::CHAT_ACCENT)),
            Span::styled(
                format!(" · {}", state.provider),
                Style::new().fg(color::DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", glyph::SESS_DURATION),
                Style::new().fg(color::DIM),
            ),
            Span::styled("dur ", Style::new().fg(color::DIM)),
            Span::styled(
                format!("{}   ", state.duration),
                Style::new().fg(color::TXT),
            ),
            Span::styled("tok ", Style::new().fg(color::DIM)),
            Span::styled(
                human_tokens(state.session_tokens),
                Style::new().fg(color::TXT),
            ),
        ]),
        {
            let mut spans = vec![
                Span::styled(
                    format!("{} ", glyph::SESS_CONTEXT),
                    Style::new().fg(color::DIM),
                ),
                Span::styled("ctx ", Style::new().fg(color::DIM)),
                Span::styled(format!("{ctx} "), Style::new().fg(color::TXT)),
            ];
            if state.ctx_ceiling > 0 {
                let frac = state.ctx_used as f64 / state.ctx_ceiling as f64;
                let col = if frac >= 0.9 {
                    color::ERROR
                } else if frac >= 0.7 {
                    color::WARN
                } else {
                    color::OK
                };
                spans.push(Span::styled(gauge(frac, 8), Style::new().fg(col)));
            }
            Line::from(spans)
        },
    ];
    // cwd: truncate to the drawer width, never wrap (paths get long).
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", glyph::SESS_CWD),
            Style::new().fg(color::DIM),
        ),
        Span::styled("cwd ", Style::new().fg(color::DIM)),
        Span::styled(
            truncate(&state.cwd, (area.width as usize).saturating_sub(7)),
            Style::new().fg(color::DIM),
        ),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The tasks drawer body (Task 8): one row per task, a token glyph+color for
/// status (☐ pending/dim, ◐ active/warn, ✓ done/ok — spec §16, no literals),
/// label truncated to fit, capped to the rows the box actually has. Empty →
/// a dim "no tasks" line rather than a blank body.
fn render_tasks_body(frame: &mut Frame, area: Rect, items: &[zoid_core::tasks::TaskItem]) {
    use crate::text::truncate;
    use zoid_core::tasks::TaskStatus;
    if items.is_empty() {
        let line = Line::from(Span::styled("no tasks", Style::new().fg(color::DIM)));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let rows: Vec<Line> = items
        .iter()
        .take(area.height as usize)
        .map(|it| {
            let (g, c) = match it.status {
                TaskStatus::Pending => (glyph::PENDING, color::DIM),
                TaskStatus::Active => (glyph::RUNNING, color::WARN),
                TaskStatus::Done => (glyph::PASS, color::OK),
            };
            let text_color = if matches!(it.status, TaskStatus::Done) {
                color::DIM
            } else {
                color::TXT
            };
            let label = truncate(&it.text, area.width.saturating_sub(2) as usize);
            Line::from(vec![
                Span::styled(format!("{g} "), Style::new().fg(c)),
                Span::styled(label, Style::new().fg(text_color)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
}

fn render_palette(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(
            format!(" {} {} ", glyph::USER_TURN, state.palette.query),
            Style::new().fg(color::TXT),
        ));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);

    let items = all_items(state.mode);
    let matches = selectable_matches(&items, &state.palette.query);
    let sel = nav(state.palette.selected, 0, matches.len());

    // Render the full grouped list; highlight the selected *selectable* row.
    // Track the line index of the selected row (group headers are interleaved)
    // so the viewport can scroll to keep it visible — group counts vary with
    // Plan 2's session group, so the list can exceed the overlay's fixed height.
    let mut lines: Vec<Line> = Vec::new();
    let mut last_group = String::new();
    let mut selected_line: usize = 0;
    for (i, it) in items.iter().enumerate() {
        if it.group != last_group {
            lines.push(Line::styled(
                it.group.to_uppercase(),
                Style::new().fg(color::CHAT_ACCENT),
            ));
            last_group = it.group.clone();
        }
        let is_sel = matches.get(sel) == Some(&i);
        if is_sel {
            selected_line = lines.len();
        }
        lines.push(palette_row_line(it, is_sel));
    }

    // Scroll-follow: keep the selected line within the visible viewport. When
    // the selection is near the top, offset is 0; as it moves past the bottom
    // edge, the offset grows so the selected row stays on the last visible line.
    let vh = inner.height as usize;
    let off = selected_line.saturating_sub(vh.saturating_sub(1));
    frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
}

fn palette_row_line(it: &PaletteItem, selected: bool) -> Line<'static> {
    let enabled = it.command.is_some();
    let fg = if enabled { color::TXT } else { color::DIM };
    let bg = |s: Style| if selected { s.bg(color::SEL_BG) } else { s };
    Line::from(vec![
        Span::styled(
            format!(" {} {}", it.icon, it.label),
            bg(Style::new().fg(fg)),
        ),
        Span::styled(format!("  {}", it.hint), bg(Style::new().fg(color::DIM))),
        Span::styled(format!("  {}", it.keybind), bg(Style::new().fg(color::DIM))),
    ])
}

fn render_cmdline(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);
    let line = Line::from(vec![
        Span::styled(":", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(state.cmdline.buffer.clone(), Style::new().fg(color::TXT)),
        Span::styled(
            glyph::CARET.to_string(),
            Style::new().fg(color::CHAT_ACCENT),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// A bordered, titled, single-selection list — shared by the object, verb, and
/// resume-session pickers (spec ④). Same chrome as the palette overlay. Scroll-
/// follows `selected` (the resume-session picker's row count is unbounded —
/// as many sessions as this repo has — so it can clip the same way the
/// palette did before its own scroll-follow fix).
fn list_overlay(frame: &mut Frame, area: Rect, title: String, rows: &[String], selected: usize) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(title, Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);
    let vh = inner.height as usize;
    let off = selected.saturating_sub(vh.saturating_sub(1));
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
    frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
}

fn render_object_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::selectable_objects;
    let objs = selectable_objects(msgs);
    let sel = nav(state.objects.obj_selected, 0, objs.len());
    let rows: Vec<String> = objs.iter().map(object_row).collect();
    let rows = if rows.is_empty() {
        vec!["(no objects yet)".to_string()]
    } else {
        rows
    };
    list_overlay(
        frame,
        area,
        format!(" {} select object ", glyph::OPEN),
        &rows,
        sel,
    );
}

fn render_verb_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::{selectable_objects, verbs_for};
    let objs = selectable_objects(msgs);
    let sel_obj = nav(state.objects.obj_selected, 0, objs.len());
    let (title, rows) = match objs.get(sel_obj) {
        Some(o) => (
            format!(" {} verbs · {} ", glyph::RECIPE, o.label),
            verbs_for(o.kind)
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>(),
        ),
        None => (" verbs ".to_string(), vec!["(no object)".to_string()]),
    };
    let sel = nav(state.objects.verb_selected, 0, rows.len());
    list_overlay(frame, area, title, &rows, sel);
}

fn render_sessions_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    let rows = if state.sessions.is_empty() {
        vec!["(no sessions for this repo)".to_string()]
    } else {
        state.sessions.clone()
    };
    let sel = nav(state.session_selected, 0, rows.len());
    list_overlay(
        frame,
        area,
        format!(" {} resume session ", glyph::RESUME),
        &rows,
        sel,
    );
}

/// The config overlay (Task 11): a contained, centered "zoid · settings" card.
/// Single column — the section list (active marked), then the active section's
/// rows (label, current value or the in-progress edit buffer + caret, and a
/// right-aligned provenance tag with a `⚠` when an env var shadows the field),
/// then a footer keybinding hint. Sized to its content and centered so a few
/// short fields don't leave a vast empty screen. `area` is the full frame.
pub fn render_config(
    frame: &mut Frame,
    state: &ShellState,
    sections: &[crate::config_view::Section],
    area: Rect,
) {
    use crate::layout::centered;
    use crate::text::{pad_to, truncate};

    frame.render_widget(Clear, area); // focus the card: clear the frame behind it
    if sections.is_empty() {
        return;
    }
    let active = state.config_section.min(sections.len() - 1);
    // Words, not arrow glyphs, keep the footer within §16 (no untokenized glyphs).
    let footer = "Tab section · Left/Right change · Enter edit · Esc close";

    // Content width: fit the footer and the widest section title, with a floor,
    // plus one column of left indent, capped to the frame. Field rows are padded/
    // truncated to this width so the provenance tag always lands at the card edge.
    let title_w = sections
        .iter()
        .map(|s| s.title.width() + 2)
        .max()
        .unwrap_or(0);
    let inner_w = (footer.width().max(title_w).max(40) + 1)
        .min(area.width.saturating_sub(2) as usize)
        .max(8);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("")); // top breathing room
    for (i, s) in sections.iter().enumerate() {
        let on = i == active;
        let marker = if on { glyph::COLLAPSED } else { ' ' };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {}", s.title),
            Style::new().fg(if on { color::CHAT_ACCENT } else { color::DIM }),
        )));
    }
    lines.push(Line::from(""));

    for (i, r) in sections[active].rows.iter().enumerate() {
        let cur = i == state.config_field;
        let val = if cur {
            if let Some(buf) = &state.config_edit {
                let shown = if matches!(r.kind, crate::config_view::FieldKind::Secret) {
                    glyph::MASK.to_string().repeat(buf.chars().count())
                } else {
                    buf.clone()
                };
                format!("{shown}{}", glyph::CARET)
            } else {
                r.value.clone()
            }
        } else {
            r.value.clone()
        };
        let (tag_txt, tag_col) = match r.source {
            zoid_core::config::Source::Default => ("[default]", color::DIM),
            zoid_core::config::Source::UserGlobal => ("[user]", color::CHAT_ACCENT),
            zoid_core::config::Source::Project => ("[repo]", color::BRANCH),
            zoid_core::config::Source::Local => ("[local]", color::BRANCH),
            zoid_core::config::Source::Env => ("[env]", color::WARN),
        };
        let warn = if r.env_shadowed {
            format!(" {}", glyph::WARNING)
        } else {
            String::new()
        };
        // Cursor marker + label on the left; value stretched (display-width padded)
        // so the tag lands at the card's right edge.
        let marker = if cur { glyph::COLLAPSED } else { ' ' };
        let left = format!(" {marker} {}", pad_to(r.label, 14));
        let fixed = left.width() + tag_txt.width() + warn.width();
        let mid = inner_w.saturating_sub(fixed).max(1);
        let val_shown = pad_to(&truncate(&val, mid), mid);
        let mut spans = vec![
            Span::styled(
                left,
                Style::new().fg(if cur { color::CHAT_ACCENT } else { color::TXT }),
            ),
            Span::styled(val_shown, Style::new().fg(color::TXT)),
            Span::styled(tag_txt.to_string(), Style::new().fg(tag_col)),
        ];
        if !warn.is_empty() {
            spans.push(Span::styled(warn, Style::new().fg(color::WARN)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" {footer}"),
        Style::new().fg(color::DIM),
    )));

    // Card = content + border, centered; content is already 1-space indented.
    let card_w = inner_w as u16 + 2;
    let card_h = (lines.len() as u16 + 2).min(area.height);
    let rect = centered(area, card_w, card_h);
    // (the full-frame Clear above already wiped the card region)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" zoid · settings ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The `ask_user` question overlay (Task 11): a centered, contained card —
/// same chrome as `render_config` (rounded border, cleared background,
/// content sized to fit). Pick mode lists `q.rows()` (the model's choices +
/// the two synthetic "Other…"/"— let you decide —" rows from `question.rs`)
/// with the selected row highlighted; free-text mode shows the buffer with a
/// caret and a dim hint line. All glyphs/colors come from `tokens` (§16).
const QUESTION_HINT: &str = "submit · empty = let you decide · Esc take over";

pub fn render_question(frame: &mut Frame, area: Rect, q: &crate::question::QuestionState) {
    use crate::layout::centered;
    use crate::question::QuestionMode;

    frame.render_widget(Clear, area); // focus the card: clear the frame behind it

    // Content width: fit the question text / the widest row / (in free-text
    // mode) the hint line, with a floor, capped to the frame (mirrors
    // render_config's inner_w derivation).
    let widest_row = match q.mode {
        QuestionMode::Pick => q.rows().iter().map(|r| r.width()).max().unwrap_or(0),
        QuestionMode::FreeText => (q.free_text.width() + 1) // + caret column
            .max(QUESTION_HINT.width() + 2), // + glyph::RETURN + separating space
    };
    let content_w = widest_row
        .max(q.question.width())
        .max(40)
        .min(area.width.saturating_sub(4) as usize);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for l in wrap_plain(&q.question, content_w) {
        lines.push(Line::from(Span::styled(
            format!(" {l}"),
            Style::new().fg(color::TXT),
        )));
    }
    lines.push(Line::from(""));

    match q.mode {
        QuestionMode::Pick => {
            for (i, r) in q.rows().iter().enumerate() {
                let style = if i == q.selected {
                    Style::new().fg(color::TXT).bg(color::SEL_BG)
                } else {
                    Style::new().fg(color::TXT)
                };
                // Wrap long choices so paragraph-length options are fully readable
                // instead of clipped at the card edge (the bug that made choices
                // look invisible). Continuation lines are indented for legibility;
                // every wrapped line of the selected row carries the highlight.
                for (j, wl) in wrap_plain(r, content_w.saturating_sub(1)).iter().enumerate() {
                    let indent = if j == 0 { " " } else { "   " };
                    lines.push(Line::from(Span::styled(format!("{indent}{wl}"), style)));
                }
            }
        }
        QuestionMode::FreeText => {
            lines.push(Line::from(vec![
                Span::styled(format!(" {}", q.free_text), Style::new().fg(color::TXT)),
                Span::styled(
                    glyph::CARET.to_string(),
                    Style::new().fg(color::CHAT_ACCENT),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(" {} {QUESTION_HINT}", glyph::RETURN),
                Style::new().fg(color::DIM),
            )));
        }
    }

    let card_w = content_w as u16 + 2 + 1; // + left indent + border
    let card_h = (lines.len() as u16 + 2).min(area.height);
    let rect = centered(area, card_w, card_h);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" zoid · question ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Greedy word-wrap to at most `width` display columns per line (no hyphenation).
/// Deterministic and simple — good enough for the short prose an `ask_user`
/// question is expected to carry; a word longer than `width` is left whole
/// (over-wide rather than mangled).
fn wrap_plain(s: &str, width: usize) -> Vec<String> {
    if s.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in s.split_whitespace() {
        let word_w = word.width();
        let sep_w = if cur.is_empty() { 0 } else { 1 };
        if cur_w + sep_w + word_w > width && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if !cur.is_empty() {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += word_w;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
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
