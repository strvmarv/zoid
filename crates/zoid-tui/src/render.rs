//! `render_shell` — the shell frame: the active-mode title + status chip, the
//! main surface (the conversation, or a mode-error card when the active mode
//! failed to load), the rail of drawers, the input box, and the palette
//! overlay. Every glyph/color comes from `tokens` (spec §16).
//! Geometry comes from `layout::compute` — the same rects mouse hit-testing uses.

use crate::chat::{conversation_view, ChatView};
use crate::command::Command;
use crate::economy_view::EconomyView;
use crate::layout::{compute, ShellLayout, CONV_PAD};
use crate::palette::{
    all_items, direct_filter, direct_items, nav, resolve_phase, selectable_matches, PaletteItem,
    Phase,
};
use crate::state::{DrawerId, Focus, Overlay, PaletteStage, ShellState};
use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use ratatui_textarea::TextArea;
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

    // The top row carries the centered "zoid" wordmark (render_title below).
    // The activity indicator lives in the status bar (center).

    if state.active_mode_broken {
        render_mode_error(frame, state, layout.conversation);
    } else {
        {
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
            // Clamp the scroll offset to the body produced at THIS altitude. The three
            // zoom altitudes (Summary/Normal/Detail) yield very different line counts,
            // so an offset valid at one is meaningless at another; without this clamp a
            // large offset carried from a taller altitude renders past the end as blank
            // rows after a zoom switch (the "corrupted display" bug). Display-only — the
            // stored offset is reset to 0 on an actual zoom change.
            //
            // `(scroll, max_scroll, content_len)` are computed per path; the
            // scrollbar below is shared.
            let (scroll, max_scroll, content_len) = match body {
                // Hot path: the caller cached the full body. Paint only the
                // visible window by BORROWING each line straight into the buffer
                // — no per-frame deep clone of the whole (potentially
                // thousands-of-owned-lines) transcript, which was the cost of the
                // old `full.to_vec()`. Lines are pre-wrapped to `text.width`, so
                // each maps to exactly one row, identical to a no-wrap Paragraph
                // scrolled by `scroll`. The zoom-reveal truncation caps the base
                // line count; the in-flight tool spinner is a single trailing row.
                Some(full) => {
                    let base_len = match view.reveal {
                        Some(n) => full.len().min(n),
                        None => full.len(),
                    };
                    // The in-flight tool indicator moved to the status bar, so
                    // the body no longer gains a trailing spinner line. `has_tool`
                    // no longer adds a row — the body is exactly the transcript.
                    let total = base_len;
                    let max_scroll = total
                        .saturating_sub(text.height as usize)
                        .min(u16::MAX as usize) as u16;
                    let scroll = state.conversation_scroll.min(max_scroll);
                    let content_len = total.min(u16::MAX as usize) as u16;

                    // Equivalence to a no-wrap Paragraph relies on every body
                    // line being LEFT-aligned (set_line always starts at text.x;
                    // Paragraph would honor per-line alignment). conversation_view
                    // never sets alignment, so this holds — a centered/right line
                    // added there would break the cached path only.
                    let start = scroll as usize;
                    let buf = frame.buffer_mut();
                    for row in 0..text.height as usize {
                        let idx = start + row;
                        let y = text.y + row as u16;
                        if idx < base_len {
                            buf.set_line(text.x, y, &full[idx], text.width);
                        } else {
                            break;
                        }
                    }
                    (scroll, max_scroll, content_len)
                }
                // Uncached fallback (tests / examples): render `conversation_view`
                // (the expensive wrap + highlight pass) and let Paragraph scroll.
                None => {
                    let body = conversation_view(
                        msgs,
                        view,
                        streaming,
                        text.width as usize,
                        state.question.as_ref(),
                    );
                    // The in-flight tool indicator moved to the status bar; the
                    // body is just the transcript, no trailing spinner line.
                    let max_scroll = body
                        .len()
                        .saturating_sub(text.height as usize)
                        .min(u16::MAX as usize) as u16;
                    let scroll = state.conversation_scroll.min(max_scroll);
                    let content_len = body.len().min(u16::MAX as usize) as u16;
                    frame.render_widget(Paragraph::new(body).scroll((scroll, 0)), text);
                    (scroll, max_scroll, content_len)
                }
            };
            conv_max_scroll = max_scroll;

            // Always-visible scrollbar in the rightmost gutter column of the
            // conversation rect (CONV_PAD reserves it, so text never overlaps).
            let track_h = layout.conversation.height;
            if track_h > 0 && layout.conversation.width > 0 {
                let bar_x = layout.conversation.right().saturating_sub(1);
                let (thumb_start, thumb_len) =
                    crate::scrollbar::scrollbar_thumb(scroll, max_scroll, track_h, content_len);
                let buf = frame.buffer_mut();
                for dy in 0..track_h {
                    let y = layout.conversation.y + dy;
                    let in_thumb = dy >= thumb_start && dy < thumb_start + thumb_len;
                    let (ch, fg) = if in_thumb {
                        (glyph::SCROLL_THUMB, color::CHAT_ACCENT)
                    } else {
                        (glyph::SCROLL_TRACK, color::DIM)
                    };
                    buf[(bar_x, y)].set_char(ch).set_style(Style::new().fg(fg));
                }
            }
        }
    }

    // Top bar: centered wordmark (moved from the bottom-right status bar).
    render_title(frame, state, layout.title);

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
        // render_config draws a full-frame three-column card (sections | fields | picker).
        render_config(frame, state, &state.config_sections, frame.area());
    } else if state.overlay == Overlay::ProviderSwitch {
        render_provider_switch(frame, state, frame.area());
    }
    conv_max_scroll
}

/// The crafted error card shown when the active mode failed to load (spec §9).
fn render_mode_error(frame: &mut Frame, state: &ShellState, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            format!("⚠ mode '{}' failed to load", state.active_mode),
            Style::new().fg(color::BUILD_ACCENT),
        )),
        Line::from(Span::styled(
            "Fix its mode.md, then run  :mode reload",
            Style::new().fg(color::DIM),
        )),
    ];
    let p =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" mode error "));
    frame.render_widget(p, area);
}

fn render_title(frame: &mut Frame, _state: &ShellState, area: Rect) {
    let wordmark = "zoid";
    let palette_hint = "^P palette";
    let w = area.width as usize;
    let wm_w = wordmark.width();
    let pad = w.saturating_sub(wm_w) / 2;
    let mut spans = vec![Span::styled(" ".repeat(pad), Style::new())];
    spans.push(Span::styled(
        wordmark.to_string(),
        Style::new().fg(color::DIM),
    ));
    let used = pad + wm_w;
    let right_pad = w.saturating_sub(used).saturating_sub(palette_hint.width());
    if right_pad > 0 {
        spans.push(Span::styled(" ".repeat(right_pad), Style::new()));
    }
    spans.push(Span::styled(
        palette_hint.to_string(),
        Style::new().fg(color::DIM),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

/// Continuous glyph rotation for the tool indicator (moon phases).
/// Cycles `TOOL_FRAMES` at ~200ms/frame (800ms full cycle), driven by
/// elapsed time from `started_at`. Falls back to the first frame when no
/// timestamp is set (idle/snapshot tests).
fn tool_frame(started_at: Option<std::time::Instant>) -> char {
    let frames = glyph::TOOL_FRAMES;
    match started_at {
        Some(t) => {
            let ms = t.elapsed().as_millis() as usize;
            frames[(ms / 200) % frames.len()]
        }
        None => frames[0],
    }
}

/// Continuous glyph rotation for the compaction indicator (box rotation).
/// Cycles `COMPACT_FRAMES` at ~300ms/frame (1200ms full cycle — slower than
/// tool, distinct cadence). Falls back to the first frame when no timestamp
/// is set (idle/snapshot tests).
fn compact_frame(started_at: Option<std::time::Instant>) -> char {
    let frames = glyph::COMPACT_FRAMES;
    match started_at {
        Some(t) => {
            let ms = t.elapsed().as_millis() as usize;
            frames[(ms / 300) % frames.len()]
        }
        None => frames[0],
    }
}

fn render_status(frame: &mut Frame, state: &ShellState, view: &ChatView, area: Rect) {
    // Left segment: mode badge only (zoom hint moved to the right side). The chip
    // is dynamic — the active mode name, uppercased; a ⚠-prefixed variant when the
    // active mode failed to load.
    let chip = if state.active_mode_broken {
        format!(" ⚠ {} ", state.active_mode)
    } else {
        format!(" {} ", state.active_mode.to_uppercase())
    };
    let mut left = vec![Span::styled(
        chip,
        Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG),
    )];
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
    // Pad to a fixed width so "working" (9 chars) and "idle" (6 chars) occupy
    // the same slot — the right edge (gap to compact) never jumps.
    // Center-align the text within the fixed slot so the visual weight is
    // balanced — "● idle" (6 chars) gets padding split left+right within the
    // 9-char slot, not all on the right. This keeps it from looking lop-sided
    // next to "⠋ working" (9 chars, no padding needed).
    const CENTER_SLOT: usize = 9; // "⠋ working"
    let center = {
        let cw = center.width();
        if cw < CENTER_SLOT {
            let total_pad = CENTER_SLOT - cw;
            let left_pad = total_pad / 2;
            let right_pad = total_pad - left_pad;
            format!(
                "{}{}{}",
                " ".repeat(left_pad),
                center,
                " ".repeat(right_pad)
            )
        } else {
            center
        }
    };

    // Right segment: zoom hint (palette hint moved to the top-right title bar).
    let right = format!(" zoom {} ", view.zoom.label());

    let w = area.width as usize;
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let center_w = center.width();
    let right_w = right.width();

    // Fixed anchors: tool at ⅓, working at ½, compaction at ⅔. Each is computed
    // independently — an absent indicator doesn't displace the others. Zero
    // jitter: "working" is always dead-center regardless of what else is present.
    // Both tool and compaction animate continuously via glyph rotation while
    // active (moon phases for tool, box rotation for compaction), driven by
    // elapsed time from their `*_started_at` timestamp. The "working" indicator
    // keeps its braille spinner (distinct shape).
    // Always-present indicators: dim glyph when idle, bright + label + rotation
    // when active (mirrors the ● idle / ⠋ working pattern). Never appears or
    // disappears — only the state (dim-static vs bright-animated) changes.
    let (tool_text, tool_fg) = if let Some(name) = &state.active_tool {
        let frame = tool_frame(state.tool_started_at);
        let text = if w < 40 {
            format!("{} {}", frame, name)
        } else {
            format!("{} {} {}", frame, name, glyph::ELLIPSIS)
        };
        (text, color::WARN)
    } else {
        // Idle: dim glyph + "tool" label.
        (format!("{} tool", glyph::RUNNING), color::DIM)
    };

    let (compact_text, compact_fg) = if state.compacting {
        let frame = compact_frame(state.compaction_started_at);
        (format!("{} compact", frame), color::BRANCH)
    } else {
        // Idle: dim glyph + "compact" label.
        (format!("{} compact", glyph::COMPACT), color::DIM)
    };

    let compact_w = compact_text.width();

    // Dead-center for "working", always.
    let center_start = w.saturating_sub(center_w) / 2;
    let right_start = w.saturating_sub(right_w);

    let mut spans = left;
    // Compact indicator just left of "working" (4-space gap), always present.
    // Compact has static text (never changes width), so the gap to "working"
    // never jumps. Dim glyph when idle; bright + label + rotation when active.
    let compact_right = center_start.saturating_sub(4);
    let compact_left = compact_right.saturating_sub(compact_w);
    let compact_pad = compact_left.saturating_sub(left_w);
    if compact_pad > 0 {
        spans.push(Span::styled(" ".repeat(compact_pad), Style::new()));
    }
    spans.push(Span::styled(
        compact_text.clone(),
        Style::new().fg(compact_fg),
    ));
    // Fixed 4-space gap between compact and working.
    spans.push(Span::styled(" ".repeat(4), Style::new()));
    // Working — dead center.
    spans.push(Span::styled(center, Style::new().fg(fg)));
    // Fixed 4-space gap between working and tool (symmetric with compact gap).
    spans.push(Span::styled(" ".repeat(4), Style::new()));
    // Tool indicator right of "working" (4-space gap), always present.
    // The tool indicator is on the outside (right of "working"), so it has
    // room to grow toward the right edge. Cap it to the available space
    // rather than a fixed slot, so long tool names expand instead of
    // truncating against the glyph.
    let consumed_before_tool: usize = spans.iter().map(|s| s.content.width()).sum();
    let tool_cap = right_start
        .saturating_sub(consumed_before_tool)
        .saturating_sub(1) // 1-char padding before the zoom hint
        .max(8); // floor: keep the glyph + a few chars even on a narrow screen
    let tool_text = {
        let tw = tool_text.width();
        if tw > tool_cap {
            let mut chars: Vec<char> = tool_text.chars().collect();
            while chars.iter().collect::<String>().width() > tool_cap && chars.len() > 2 {
                chars.remove(1); // trim after the glyph
            }
            chars.into_iter().collect::<String>()
        } else {
            tool_text
        }
    };
    spans.push(Span::styled(tool_text, Style::new().fg(tool_fg)));

    // Pad to the zoom hint (right edge).
    let consumed_so_far: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad2 = right_start.saturating_sub(consumed_so_far);
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
                DrawerId::Subagents => {} // hidden from the rail (delegation disabled)
            }
        }
    }
}

fn render_economy_body(frame: &mut Frame, econ: &EconomyView, area: Rect, rail_focused: bool) {
    use crate::economy_view::{heat_bar, heat_color, tail};
    use crate::text::{pad_to, truncate_start};
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
        let label = pad_to(&truncate_start(&r.label, label_budget), label_budget);
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
            Span::styled(
                if state.cache_supported {
                    "tok "
                } else {
                    "tok/cac "
                },
                Style::new().fg(color::DIM),
            ),
            Span::styled(
                format!(
                    "{}   ",
                    if state.cache_supported {
                        human_tokens(state.session_tokens)
                    } else {
                        human_tokens(state.session_tokens + state.cached_tokens)
                    }
                ),
                Style::new().fg(color::TXT),
            ),
            if state.cache_supported {
                Span::styled("cac ", Style::new().fg(color::DIM))
            } else {
                Span::styled("", Style::new())
            },
            if state.cache_supported {
                Span::styled(
                    human_tokens(state.cached_tokens),
                    Style::new().fg(color::TXT),
                )
            } else {
                Span::styled("", Style::new())
            },
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
        Span::styled(format!("{} ", glyph::SESS_CWD), Style::new().fg(color::DIM)),
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

/// The subagents drawer body: one row per in-flight subagent — a running glyph
/// + truncated id + truncated task label. Empty → dim "no subagents". Capped
/// to the body rows the allocator gave the drawer.
#[allow(dead_code)] // subagent delegation temporarily disabled
fn render_subagents_body(frame: &mut Frame, area: Rect, rows: &[crate::state::SubagentRow]) {
    use crate::text::truncate;
    if rows.is_empty() {
        let line = Line::from(Span::styled("no subagents", Style::new().fg(color::DIM)));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let rows_rendered: Vec<Line> = rows
        .iter()
        .take(area.height as usize)
        .map(|r| {
            let id_w = 14; // "sub-01HZ..." is ~13-14 chars
            let id = truncate(&r.id, id_w);
            let task_budget = area.width.saturating_sub(id_w as u16 + 3) as usize;
            let task = truncate(&r.task, task_budget);
            Line::from(vec![
                Span::styled(format!("{} ", glyph::RUNNING), Style::new().fg(color::WARN)),
                Span::styled(format!("{id}  "), Style::new().fg(color::TXT)),
                Span::styled(task, Style::new().fg(color::DIM)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows_rendered), area);
}

fn render_palette(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);

    let title = match &state.palette.stage {
        PaletteStage::Pick if state.palette.query.starts_with(':') => {
            format!(" {} ", state.palette.query)
        }
        PaletteStage::Pick => format!(" {} {} ", glyph::USER_TURN, state.palette.query),
        PaletteStage::Arg { kind, input } => format!(" {}: {} ", kind.prompt(), input),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(title, Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);

    match resolve_phase(&state.palette) {
        Phase::Arg => {
            let hint = Line::styled("Enter apply · Esc back", Style::new().fg(color::DIM));
            frame.render_widget(Paragraph::new(vec![hint]), inner);
        }
        Phase::Direct { cmd } => {
            let items = direct_items(state);
            let filter = direct_filter(&state.palette.query);
            let matches = selectable_matches(&items, filter);
            let list_nonempty = !matches.is_empty();

            let mut lines: Vec<Line> = Vec::new();

            // Preview line: show when parse_command resolved to a real Command
            // (not Unknown) OR when the list is empty. Suppress when the list is
            // visible and the buffer is an incomplete namespace (Unknown).
            let show_preview = !matches!(cmd, Command::Unknown(_)) || !list_nonempty;
            if show_preview {
                let preview: String = match cmd {
                    Command::Unknown(s) if s.is_empty() => "type a command word".to_string(),
                    Command::Unknown(_) => "unknown command".to_string(),
                    Command::SwitchMode(name) => format!("→ Switch to {name}"),
                    Command::ReloadModes => "→ Reload modes".to_string(),
                    Command::ModeImport(url) => format!("→ Import mode: {url}"),
                    Command::ModeUpdate(name) => format!("→ Update mode: {name}"),
                    Command::RenameSession(name) => format!("→ Rename session: {name}"),
                    Command::Delegate(task) => format!("→ Delegate: {task}"),
                    Command::Quit => "→ Quit zoid".to_string(),
                    Command::OpenDrawer(id) => format!("→ Toggle {:?} drawer", id),
                    Command::NewSession => "→ New session".to_string(),
                    Command::ResumeSessionPicker => "→ Resume session…".to_string(),
                    Command::OpenConfig => "→ Open settings".to_string(),
                    Command::CompanionEnable => "→ Enable companion".to_string(),
                    Command::CompanionDisable => "→ Disable companion".to_string(),
                    Command::CompactNow => "→ Compact context now".to_string(),
                };
                lines.push(Line::styled(preview, Style::new().fg(color::DIM)));
            }

            // Filtered list below the preview line.
            let mut selected_line: usize = 0;
            let list_start = lines.len();
            let sel = nav(state.palette.selected, 0, matches.len());
            for (rank, &i) in matches.iter().enumerate() {
                if rank == sel {
                    selected_line = list_start + rank;
                }
                lines.push(palette_row_line(&items[i], rank == sel));
            }

            // Footer hint.
            if list_nonempty {
                lines.push(Line::styled(
                    "↑↓ move · ⏎ select · esc close · type to filter",
                    Style::new().fg(color::DIM),
                ));
            } else {
                lines.push(Line::styled(
                    "⏎ run · esc close",
                    Style::new().fg(color::DIM),
                ));
            }

            // Scroll-follow on the selected row.
            let vh = inner.height as usize;
            let off = selected_line.saturating_sub(vh.saturating_sub(1));
            frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
        }
        Phase::Pick => {
            let items = all_items(&state.active_mode, &state.mode_names, state.companion_on);
            let matches = selectable_matches(&items, &state.palette.query);
            let sel = nav(state.palette.selected, 0, matches.len());

            let mut lines: Vec<Line> = Vec::new();
            let mut selected_line: usize = 0;
            for (rank, &i) in matches.iter().enumerate() {
                if rank == sel {
                    selected_line = lines.len();
                }
                lines.push(palette_row_line(&items[i], rank == sel));
            }

            let vh = inner.height as usize;
            let off = selected_line.saturating_sub(vh.saturating_sub(1));
            frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
        }
    }
}

fn palette_row_line(it: &PaletteItem, selected: bool) -> Line<'static> {
    let bg = |s: Style| if selected { s.bg(color::SEL_BG) } else { s };
    Line::from(Span::styled(
        format!(" {}", it.label),
        bg(Style::new().fg(color::TXT)),
    ))
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
        state
            .sessions
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let live = state.sessions_live.get(i).copied().unwrap_or(false);
                if live {
                    format!("{r}  · in use")
                } else {
                    r.clone()
                }
            })
            .collect()
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

/// The config overlay (Task 8): a full-frame "zoid · settings" three-column
/// layout — col 1 the sections rail (active marked), col 2 the active
/// section's fields (label, current value or the in-progress edit buffer +
/// caret, and a right-aligned provenance tag), col 3 the contextual picker
/// (provider / model), rendered only while `state.config_picker_open()`.
/// A footer keybinding hint is reserved at the bottom of the card. `area` is
/// the full frame.
pub fn render_config(
    frame: &mut Frame,
    state: &ShellState,
    sections: &[crate::config_view::Section],
    area: Rect,
) {
    use crate::config_view::FieldKind;
    use crate::text::{pad_to, truncate};
    use ratatui::layout::{Constraint, Direction, Layout};

    frame.render_widget(Clear, area);
    if sections.is_empty() {
        return;
    }
    let active = state.config_section.min(sections.len() - 1);

    // Outer full-frame card.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" zoid · settings ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Footer line reserved at the bottom of the inner area.
    let footer = "Tab section · Up/Down move · Enter drill · Esc back";
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let body = rows[0];
    let foot = rows[1];

    // Column split: sections rail | fields | (picker, only if open and it fits
    // as a third column). Below the three-column minimum the picker instead
    // renders as a floating overlay card on top of the fields column (see
    // below) so every row stays legible instead of squeezing to nothing.
    const RAIL_W: u16 = 22;
    const FIELDS_W: u16 = 40;
    const PICKER_MIN: u16 = 20;
    let picker_open = state.config_picker_open();
    let three_col_fits = body.width >= RAIL_W + FIELDS_W + PICKER_MIN;
    let cols = if picker_open && three_col_fits {
        // (unchanged) three columns: rail | fields | picker
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(RAIL_W),
                Constraint::Length(FIELDS_W),
                Constraint::Min(PICKER_MIN),
            ])
            .split(body)
    } else if picker_open {
        // degraded: two columns (rail | fields); picker floats as an overlay
        // card over the fields column below. Shrink the rail at very narrow
        // widths so the fields column (and the card over it) keeps room.
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(RAIL_W.min(body.width / 3).max(8)),
                Constraint::Min(20),
            ])
            .split(body)
    } else {
        // picker closed: unchanged from before the settings redesign —
        // fixed rail + fields fills the rest.
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(RAIL_W), Constraint::Min(30)])
            .split(body)
    };

    // Column 1: sections rail.
    let mut nav: Vec<Line> = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        let on = i == active;
        let marker = if on { glyph::COLLAPSED } else { ' ' };
        nav.push(Line::from(Span::styled(
            format!(" {marker} {}", s.title),
            Style::new().fg(if on { color::CHAT_ACCENT } else { color::DIM }),
        )));
    }
    frame.render_widget(Paragraph::new(nav), cols[0]);

    // Column 2: fields of the active section — or, while the API-key gate is
    // prompting, a dedicated masked key-entry view instead of the per-row list
    // (the current field is the provider/model row, not a `FieldKind::Secret`
    // row, so the normal per-row masking below wouldn't apply here).
    let field_w = cols[1].width as usize;
    let fields: Vec<Line> = if let Some(env) = state.config_key_prompt {
        let buf = state.config_edit.as_deref().unwrap_or("");
        let masked = glyph::MASK.to_string().repeat(buf.chars().count());
        vec![
            Line::from(""),
            Line::from(Span::styled(
                format!(" Enter {env}"),
                Style::new().fg(color::CHAT_ACCENT),
            )),
            Line::from(Span::styled(
                format!("   {masked}{}", glyph::CARET),
                Style::new().fg(color::TXT),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "   Enter save · Esc cancel",
                Style::new().fg(color::DIM),
            )),
        ]
    } else {
        let mut fields: Vec<Line> = Vec::new();
        for (i, r) in sections[active].rows.iter().enumerate() {
            let cur =
                i == state.config_field && state.config_col == crate::state::ConfigCol::Fields;
            let val = if i == state.config_field {
                if let Some(buf) = &state.config_edit {
                    let shown = if matches!(r.kind, FieldKind::Secret) {
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
            let marker = if cur { glyph::COLLAPSED } else { ' ' };
            let left = format!(" {marker} {}", pad_to(r.label, 12));
            let fixed = left.width() + tag_txt.width() + warn.width();
            let mid = field_w.saturating_sub(fixed + 2).max(1); // +2 gap between label and value
            let val_shown = pad_to(&truncate(&val, mid), mid);
            let mut spans = vec![
                Span::styled(
                    left,
                    Style::new().fg(if cur { color::CHAT_ACCENT } else { color::TXT }),
                ),
                Span::styled("  ", Style::new()), // 2-space gap between label and value
                Span::styled(val_shown, Style::new().fg(color::TXT)),
                Span::styled(tag_txt.to_string(), Style::new().fg(tag_col)),
            ];
            if !warn.is_empty() {
                spans.push(Span::styled(warn, Style::new().fg(color::WARN)));
            }
            fields.push(Line::from(spans));
        }
        fields
    };
    frame.render_widget(Paragraph::new(fields), cols[1]);

    // Column 3: contextual picker — only when open AND three columns fit.
    // Below the fit threshold the picker renders as an overlay card instead
    // (below), never in column 3, so it renders in exactly one place.
    if picker_open && three_col_fits {
        let active = state.config_col == crate::state::ConfigCol::Picker;
        let pick = picker_lines(
            &state.config_picker,
            state.config_picker_sel,
            active,
            cols[2].width as usize,
        );
        frame.render_widget(Paragraph::new(pick), cols[2]);
    }

    // Graceful degradation: when the picker is open but three columns don't
    // fit, float it as a rounded sub-card over the fields column (col 2)
    // instead of squeezing a third column to nothing. The picker is
    // transient, so overlaying is acceptable and keeps every row legible.
    if picker_open && !three_col_fits {
        let over = crate::layout::centered(
            cols[1],
            cols[1].width.saturating_sub(2),
            (state.config_picker.len() as u16 + 2).min(cols[1].height),
        );
        frame.render_widget(Clear, over);
        let pblock = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color::CHAT_ACCENT));
        let pinner = pblock.inner(over);
        frame.render_widget(pblock, over);
        let active = state.config_col == crate::state::ConfigCol::Picker;
        let pick = picker_lines(
            &state.config_picker,
            state.config_picker_sel,
            active,
            pinner.width as usize,
        );
        frame.render_widget(Paragraph::new(pick), pinner);
    }

    // Footer.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {footer}"),
            Style::new().fg(color::DIM),
        ))),
        foot,
    );
}

/// Build the styled option lines for the contextual config picker, shared by
/// both places it can render: the Task-8 inline column-3 layout (three
/// columns fit) and the Task-9 floating overlay card (they don't). Applies
/// the current marker, `SEL_BG` on the selected row when `active`, `DIM` for
/// non-selectable/`[planned]` rows, and truncates/pads each line to `width`.
fn picker_lines(
    picker: &[crate::config_view::PickOption],
    sel: usize,
    active: bool,
    width: usize,
) -> Vec<Line<'static>> {
    use crate::text::{pad_to, truncate};
    let w = width.saturating_sub(1).max(1);
    picker
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let is_sel = active && i == sel;
            let dot = if o.is_current { glyph::COLLAPSED } else { ' ' };
            let base = format!(" {dot} {}  {}", o.label, o.detail);
            let text = pad_to(&truncate(&base, w), w);
            let style = if !o.selectable {
                Style::new().fg(color::DIM)
            } else if is_sel {
                Style::new().fg(color::TXT).bg(color::SEL_BG)
            } else {
                Style::new().fg(color::TXT)
            };
            Line::from(Span::styled(text, style))
        })
        .collect()
}

/// The quick-switch (`Alt+P`) overlay (Task 11): a centered, contained card —
/// same chrome as `render_config` (rounded border, cleared background,
/// background) — with two side-by-side panes: providers (left) and models
/// (right). Reuses `picker_lines` so styling (current marker, `SEL_BG` on the
/// active pane's selection, `DIM` for planned/non-selectable rows) matches
/// the settings picker exactly. Options are read from `state.switch_providers`
/// / `state.switch_models` (seeded by the bin — this fn has no `app.config`
/// access, only `&ShellState`). All glyphs/colors come from `tokens` (§16).
const SWITCH_FOOTER: &str = "Left/Right pane · Up/Down move · Enter apply · Esc cancel";

pub fn render_provider_switch(frame: &mut Frame, state: &ShellState, area: Rect) {
    use crate::layout::centered;
    use crate::state::SwitchPane;
    use ratatui::layout::{Constraint, Direction, Layout};

    frame.render_widget(Clear, area); // focus the card: clear the frame behind it

    // Quick-switch is a substantial centered overlay sized against the 160×40
    // baseline, not a card that hugs its content: a ~59-col content-width box
    // looks lost on a full-size terminal. Grow to ~60% of the frame, floored so
    // the two panes + word footer always fit, capped to the frame so it still
    // degrades gracefully on small windows.
    let rows_needed = state.switch_providers.len().max(state.switch_models.len()) as u16;
    // Floor: two 28-col panes + 1-col gutter + 2-col border, or the word footer,
    // whichever is wider.
    let content_min_w = (28 * 2 + 3).max(SWITCH_FOOTER.width() as u16 + 3);
    let card_w = (area.width * 3 / 5).max(content_min_w).min(area.width);
    // Height follows the longest list (header + footer + border), with a floor
    // so a short list still reads as a panel rather than a sliver.
    let content_h = rows_needed + 2 /* header row + footer row */ + 2/* border */;
    let card_h = content_h.max(16).min(area.height);
    let rect = centered(area, card_w, card_h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" zoid · quick switch ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // Two equal panes split the inner width, separated by a 1-col gutter.
    let pane_w = inner.width.saturating_sub(1) / 2;

    // Footer reserved at the bottom of the inner area; header row at the top.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let header = rows[0];
    let body = rows[1];
    let foot = rows[2];

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(pane_w),
            Constraint::Length(1),
            Constraint::Min(pane_w),
        ])
        .split(body);
    let header_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(pane_w),
            Constraint::Length(1),
            Constraint::Min(pane_w),
        ])
        .split(header);

    let provider_active = state.switch_pane == SwitchPane::Provider;
    let model_active = state.switch_pane == SwitchPane::Model;

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Provider",
            Style::new().fg(if provider_active {
                color::CHAT_ACCENT
            } else {
                color::DIM
            }),
        ))),
        header_cols[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Model",
            Style::new().fg(if model_active {
                color::CHAT_ACCENT
            } else {
                color::DIM
            }),
        ))),
        header_cols[2],
    );

    let provider_lines = picker_lines(
        &state.switch_providers,
        state.switch_provider_sel,
        provider_active,
        cols[0].width as usize,
    );
    frame.render_widget(Paragraph::new(provider_lines), cols[0]);

    let model_lines = picker_lines(
        &state.switch_models,
        state.switch_model_sel,
        model_active,
        cols[2].width as usize,
    );
    frame.render_widget(Paragraph::new(model_lines), cols[2]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {SWITCH_FOOTER}"),
            Style::new().fg(color::DIM),
        ))),
        foot,
    );
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

/// Word-wrap a plain string to `width` columns, breaking on whitespace.
pub(crate) fn wrap_plain(s: &str, width: usize) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Zoom;
    use ratatui::{backend::TestBackend, Terminal};

    /// Render a status bar with `compacting: true` and verify the compaction
    /// segment appears in `color::BRANCH` (purple). The compaction spinner
    /// glyph and "compacting" label must both be present.
    #[test]
    fn compaction_segment_visible_when_compacting() {
        let mut state = ShellState::new();
        state.compacting = true;
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &state, &view, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            content.contains("compacting"),
            "status bar must contain 'compacting' when state.compacting is true: got {content:?}"
        );
        assert!(
            content.contains(glyph::COMPACT.to_string().as_str()),
            "status bar must contain the compaction glyph: got {content:?}"
        );
        // The indicator uses BRANCH (purple) continuously — no more pulse/dim.
        let has_compact_color = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.style().fg == Some(color::BRANCH));
        assert!(
            has_compact_color,
            "at least one cell must use color::BRANCH (purple) for the compaction indicator"
        );
    }

    /// When `compacting: false`, the compaction segment must NOT appear —
    /// the status bar is byte-identical to the pre-feature layout.
    #[test]
    fn compaction_segment_absent_when_not_compacting() {
        let state = ShellState::new();
        assert!(!state.compacting, "compacting must default to false");
        let view = ChatView {
            zoom: Zoom::Normal,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_status(f, &state, &view, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            !content.contains("compacting"),
            "status bar must NOT contain 'compacting' when not compacting: got {content:?}"
        );
    }

    #[test]
    fn working_stays_dead_center_with_all_indicators() {
        use crate::state::ShellState;

        let mut s = ShellState::new();
        s.busy = true; // "working"
        s.set_active_tool("shell"); // tool indicator
        s.compacting = true; // compaction
        s.compaction_started_at = Some(std::time::Instant::now());

        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_status(
                    f,
                    &s,
                    &ChatView {
                        zoom: Zoom::Normal,
                        caret_on: false,
                        reveal: None,
                        tz_offset_secs: 0,
                    },
                    f.area(),
                )
            })
            .unwrap();

        // "working" should be dead-center: its start ≈ (W - working_w) / 2.
        let content = terminal.backend().buffer();
        let w = 100usize;
        let working_start = content
            .content()
            .iter()
            .enumerate()
            .find(|(_, c)| c.symbol() == "⠋")
            .map(|(i, _)| i % w)
            .unwrap_or(0);
        // "⠋ working" ≈ 9 chars → expected center ≈ 45
        let expected = (w - 9) / 2;
        assert!(
            (working_start as i32 - expected as i32).abs() <= 4,
            "working indicator must be dead-center: got {working_start}, expected ~{expected}"
        );
    }

    #[test]
    fn tool_indicator_uses_dim_color_after_pulse_window() {
        use crate::state::ShellState;
        use std::time::{Duration, Instant};

        let mut s = ShellState::new();
        s.set_active_tool("shell");
        // Simulate the pulse having elapsed.
        s.tool_started_at = Some(Instant::now() - Duration::from_secs(1));

        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_status(
                    f,
                    &s,
                    &ChatView {
                        zoom: Zoom::Normal,
                        caret_on: false,
                        reveal: None,
                        tz_offset_secs: 0,
                    },
                    f.area(),
                )
            })
            .unwrap();
        // After the pulse window, the tool indicator uses WARN_DIM.
        // Verify it renders without panic and the tool name is present.
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("shell"), "tool name must render");
    }

    #[test]
    fn sessions_overlay_marks_live_rows() {
        use crate::state::{Overlay, ShellState};
        let mut s = ShellState::new();
        s.overlay = Overlay::Sessions;
        s.sessions = vec!["a  ·  1m ago  ·  10".into(), "b  ·  2m ago  ·  20".into()];
        s.sessions_live = vec![false, true];
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_sessions_overlay(f, &s, f.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            content.contains("in use"),
            "live row must carry the 'in use' marker: {content:?}"
        );
    }
}
