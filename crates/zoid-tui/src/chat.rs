use crate::markdown::{render_body, BodyKind};
use crate::state::Zoom;
use crate::syntax_view::highlight_lines;
use crate::text::truncate;
use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use ratatui_textarea::TextArea;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zoid_core::economy::tool_path;
use zoid_core::projection::ChatMsg;
use zoid_core::zoom::{digests, TurnDigest};
use zoid_syntax::{fold_regions, FoldRegion, Language};

/// A clickable code block: the transcript line range it occupies (inclusive) and
/// its raw source, so the bin can copy the specific block the user clicks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeHit {
    pub header_line: usize,
    pub end_line: usize,
    pub source: String,
}

/// A clickable choice row in an open question card: the transcript line index
/// and the choice text, so a click can select+submit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionChoiceHit {
    pub line: usize,
    pub choice: String,
}

/// Build the conversation lines (user/assistant turns + inline tool cards).
/// Shared by `render_chat` and the modal `render_shell`. `width` is the text
/// column width used to word-wrap prose with a hanging indent (spec §3.5); pass
/// the conversation rect's inner width. Thin wrapper over `build_conversation`.
pub fn conversation_lines(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    let mut hits = Vec::new();
    build_conversation(
        msgs,
        streaming,
        caret_on,
        tz_offset_secs,
        width,
        &mut hits,
        &mut Vec::new(),
        &mut Vec::new(),
        question,
    )
}

/// The clickable code-block map (line ranges + source) for the same inputs
/// `conversation_lines` would render at Normal altitude. Called on demand (on a
/// click), so the extra build cost is paid then, not every frame.
pub fn code_hits(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<CodeHit> {
    let mut hits = Vec::new();
    build_conversation(
        msgs,
        streaming,
        caret_on,
        tz_offset_secs,
        width,
        &mut hits,
        &mut Vec::new(),
        &mut Vec::new(),
        question,
    );
    hits
}

/// The clickable choice-row map for an open question card, like `code_hits`.
/// Returns one entry per rendered choice line in the open card, so a click can
/// select+submit it. Empty if no question is open or the card is answered.
pub fn question_choice_hits(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<QuestionChoiceHit> {
    let mut choices = Vec::new();
    build_conversation(
        msgs,
        streaming,
        caret_on,
        tz_offset_secs,
        width,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut choices,
        question,
    );
    choices
}

#[allow(clippy::too_many_arguments)]
fn build_conversation(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    hits: &mut Vec<CodeHit>,
    msg_starts: &mut Vec<usize>,
    question_choices: &mut Vec<QuestionChoiceHit>,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    let last = msgs.len().saturating_sub(1);
    if msgs.is_empty() {
        return vec![Line::styled(
            "  (no messages yet)",
            Style::new().fg(color::DIM),
        )];
    }
    // Dim 24h `HH:MM ` stamp prefixing each user/assistant message row.
    let stamp = |ts: i64| {
        Span::styled(
            format!("{} ", crate::text::hhmm(ts, tz_offset_secs)),
            Style::new().fg(color::DIM),
        )
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    // (header_line, end_line, raw source) of every top-level code block, in
    // document order. Source is carried from the same render pass that produces
    // the range (via the `CodeHead` `BodyLine`), so a block and its clipboard
    // text can never desync — including when a message bails to plain text.
    let mut code_ranges: Vec<(usize, usize, String)> = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        // Record where each message's block begins (before its leading blank),
        // so a viewport-top line maps back to a message for cross-zoom anchoring.
        msg_starts.push(lines.len());
        match m {
            ChatMsg::User { text, ts } => {
                blank_between_turns(&mut lines);
                let prefix = vec![
                    stamp(*ts),
                    Span::styled(
                        format!("{} ", glyph::USER_TURN),
                        Style::new().fg(color::CHAT_ACCENT),
                    ),
                ];
                push_message(
                    &mut lines,
                    &mut code_ranges,
                    prefix,
                    render_body(text),
                    width,
                );
            }
            ChatMsg::Assistant {
                text,
                tool_calls,
                ts,
                thinking,
            } => {
                // Thinking marker (collapsed at Normal zoom).
                if let Some(thinking_text) = thinking {
                    if !thinking_text.is_empty() {
                        blank_between_turns(&mut lines);
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("{} ", glyph::EXPANDED),
                                Style::new().fg(color::DIM),
                            ),
                            Span::styled("Thinking…", Style::new().fg(color::DIM)),
                        ]));
                    }
                }
                let mut shown = text.clone();
                if streaming && caret_on && i == last && tool_calls.is_empty() {
                    shown.push(glyph::CARET);
                }
                if !shown.is_empty() || tool_calls.is_empty() {
                    blank_between_turns(&mut lines);
                    let prefix = vec![
                        stamp(*ts),
                        Span::styled("zoid ".to_string(), Style::new().fg(color::DIM)),
                    ];
                    push_message(
                        &mut lines,
                        &mut code_ranges,
                        prefix,
                        render_body(&shown),
                        width,
                    );
                }
                for tc in tool_calls {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {} ", glyph::EDIT),
                            Style::new().fg(color::CHAT_ACCENT),
                        ),
                        Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
                        Span::styled(
                            format!("({})", arg_summary(&tc.args)),
                            Style::new().fg(color::DIM),
                        ),
                        Span::styled(
                            format!(" {} peek", glyph::RETURN),
                            Style::new().fg(color::DIM),
                        ),
                    ]));
                }
            }
            ChatMsg::ToolResult {
                name,
                output,
                is_error,
                compacted,
                ..
            } => {
                let (mark, mark_color) = if *is_error {
                    (glyph::WARNING, color::ERROR)
                } else {
                    (glyph::PASS, color::OK)
                };
                let mut spans = vec![Span::styled(
                    format!("  {mark} "),
                    Style::new().fg(mark_color),
                )];
                if *compacted {
                    spans.push(Span::styled(
                        format!("{} compacted ", glyph::COMPACT),
                        Style::new().fg(color::DIM),
                    ));
                }
                spans.push(Span::styled(name.clone(), Style::new().fg(color::DIM)));
                spans.push(Span::styled(
                    format!(" → {}", first_line(output)),
                    Style::new().fg(color::DIM),
                ));
                lines.push(Line::from(spans));
            }
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok {
                    (glyph::PASS, color::OK)
                } else {
                    (glyph::WARNING, color::ERROR)
                };
                lines.push(Line::from(vec![
                    // Purple label with the card background = the collapsed chip.
                    Span::styled(
                        format!("{} delegated · {}", glyph::COLLAPSED, first_line(summary)),
                        Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG),
                    ),
                    Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                    Span::styled(
                        format!("{} peek", glyph::RETURN),
                        Style::new().fg(color::DIM),
                    ),
                ]));
            }
            ChatMsg::Question {
                id: _,
                kind,
                question: qtext,
                choices,
                state,
                ts: _,
            } => {
                blank_between_turns(&mut lines);
                let (selected, free_text) = match state {
                    zoid_core::projection::QuestionCardState::Open {
                        selected,
                        free_text,
                    } => {
                        // Overwrite the projection's placeholder cursor with the
                        // live cursor from ShellState.question (if present).
                        if let Some(q) = question {
                            (q.selected, q.free_text.clone())
                        } else {
                            (*selected, free_text.clone())
                        }
                    }
                    zoid_core::projection::QuestionCardState::Answered { .. } => (0, String::new()),
                };
                render_question_card(
                    &mut lines,
                    question_choices,
                    kind,
                    qtext,
                    choices,
                    state,
                    selected,
                    &free_text,
                    width,
                );
            }
        }
    }
    // Every top-level code block advertises the click-to-copy affordance on its
    // header row (spec §3.5). Clicking anywhere in the block copies its raw
    // source (the bin resolves the click via `code_hits`).
    for &(head, _end, _) in &code_ranges {
        add_copy_hint(&mut lines, head);
    }
    // The click-to-copy map falls straight out of the render pass: each range
    // already carries its own source, so there is no second parse to keep aligned.
    for (header_line, end_line, source) in code_ranges {
        hits.push(CodeHit {
            header_line,
            end_line,
            source,
        });
    }
    lines
}

/// Render an inline question card into `lines`. Open state shows the question,
/// choices with the live highlight, and a hint line. Answered state collapses
/// to the question title and the answer on the last line. The card uses a
/// purple border (the BRANCH color) so it stands out from the transcript.
/// Choice line indices are recorded into `question_choices` for click hit-testing.
#[allow(clippy::too_many_arguments)]
fn render_question_card(
    lines: &mut Vec<Line<'static>>,
    question_choices: &mut Vec<QuestionChoiceHit>,
    kind: &zoid_core::event::QuestionKind,
    question: &str,
    choices: &[String],
    state: &zoid_core::projection::QuestionCardState,
    selected: usize,
    free_text: &str,
    width: usize,
) {
    use zoid_core::event::QuestionKind;
    use zoid_core::projection::QuestionCardState;

    let title = match kind {
        QuestionKind::Ask => " Question ",
        QuestionKind::ModeMapping { .. } => " Mode mapping — review ",
        QuestionKind::Approval => " Approve tool ",
        QuestionKind::Feedback { .. } => " Submit feedback ",
    };
    let border = color::BRANCH;
    let content_w = width.saturating_sub(4).max(20);

    // At Normal zoom, elide the bundled-files and skipped sections for
    // ModeMapping cards — they're shown at Detail zoom (detail_lines). Ask
    // cards show the full question text at every zoom.
    let body = match kind {
        QuestionKind::Approval | QuestionKind::ModeMapping { .. } => {
            if let Some(idx) = question.find("\nBundled files") {
                let mut head = question[..idx].trim_end().to_string();
                let bundled_count = question[idx..].matches('\n').count().max(1);
                head.push_str(&format!(
                    "\n\n({bundled_count} bundled/skipped rows — Detail zoom to review)"
                ));
                head
            } else {
                question.to_string()
            }
        }
        _ => question.to_string(),
    };

    lines.push(card_border_top(title, content_w + 2, border));
    for para in body.split('\n') {
        if para.is_empty() {
            lines.push(Line::from(Span::styled(
                "│ ".to_string(),
                Style::new().fg(border),
            )));
        } else {
            for l in crate::render::wrap_plain(para, content_w) {
                lines.push(Line::from(Span::styled(
                    format!("│ {l}"),
                    Style::new().fg(color::TXT),
                )));
            }
        }
    }
    lines.push(Line::from(Span::styled(
        "│ ".to_string(),
        Style::new().fg(border),
    )));

    match state {
        QuestionCardState::Open { .. } => {
            for (i, c) in choices.iter().enumerate() {
                let marker = if i == selected { "●" } else { "○" };
                let style = if i == selected {
                    Style::new().fg(color::TXT).bg(color::SEL_BG)
                } else {
                    Style::new().fg(color::TXT)
                };
                let line_idx = lines.len();
                question_choices.push(QuestionChoiceHit {
                    line: line_idx,
                    choice: c.clone(),
                });
                lines.push(Line::from(Span::styled(
                    format!("│   {marker} {}", c),
                    style,
                )));
            }
            if !free_text.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("│   {}{}", free_text, glyph::CARET),
                    Style::new().fg(color::TXT),
                )));
            }
            lines.push(Line::from(Span::styled(
                "│ Type your answer, or pick above. Enter to submit · Esc to cancel.".to_string(),
                Style::new().fg(color::DIM),
            )));
            lines.push(card_border_bottom(content_w + 2, border));
        }
        QuestionCardState::Answered { answer } => {
            lines.push(Line::from(Span::styled(
                format!("└ ► {}", answer),
                Style::new().fg(color::TXT),
            )));
        }
    }
}

fn card_border_top(title: &str, width: usize, color: ratatui::style::Color) -> Line<'static> {
    let inner = width.saturating_sub(title.chars().count() + 2);
    let right = "─".repeat(inner);
    Line::from(vec![
        Span::styled(format!("┌─{title}"), Style::new().fg(color)),
        Span::styled(right, Style::new().fg(color)),
    ])
}

fn card_border_bottom(width: usize, color: ratatui::style::Color) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width), Style::new().fg(color)))
}

/// Attach the `⧉ copy` affordance to the code-block header at `idx`, stealing
/// room from its trailing background pad so it sits at the panel's right edge
/// without widening the row; if there's no stealable pad (a very narrow column)
/// it is appended and clips at the rail edge (the transcript renders with wrap
/// off) — an acceptable edge.
fn add_copy_hint(lines: &mut [Line<'static>], idx: usize) {
    let Some(line) = lines.get_mut(idx) else {
        return;
    };
    let hint = format!(" {} copy ", glyph::COPY);
    let hint_w = hint.width();
    if let Some(last) = line.spans.last_mut() {
        if last.style.bg == Some(color::CODE_BG)
            && last.content.chars().all(|c| c == ' ')
            && last.content.width() >= hint_w
        {
            let keep = last.content.width() - hint_w;
            *last = Span::styled(" ".repeat(keep), Style::new().bg(color::CODE_BG));
        }
    }
    line.spans.push(Span::styled(
        hint,
        Style::new().fg(color::CHAT_ACCENT).bg(color::CODE_BG),
    ));
}

/// Push a blank spacer line before a new turn, unless the transcript is empty
/// (no leading blank) — the vertical rhythm that keeps turns from feeling
/// squeezed (spec §3.5).
fn blank_between_turns(out: &mut Vec<Line<'static>>) {
    if !out.is_empty() {
        out.push(Line::from(""));
    }
}

/// Push a message (user/assistant) into `out`: the `prefix` (stamp + role) leads
/// the first body line; continuation rows are indented under the text column so
/// wrapped prose stays aligned (the hanging indent). Prose word-wraps to
/// `width`; code lines are emitted verbatim (their leading whitespace is
/// significant). Each top-level code block's `(header_line, end_line)` range is
/// appended to `code_ranges` (a `CodeHead` opens a range; contiguous `Code` lines
/// extend it; any prose closes it), powering the click-to-copy map.
fn push_message(
    out: &mut Vec<Line<'static>>,
    code_ranges: &mut Vec<(usize, usize, String)>,
    prefix: Vec<Span<'static>>,
    body: Vec<crate::markdown::BodyLine>,
    width: usize,
) {
    if body.is_empty() {
        out.push(Line::from(prefix));
        return;
    }
    let indent_w: usize = prefix.iter().map(|s| s.content.width()).sum();
    let indent = " ".repeat(indent_w);
    let content_w = width.saturating_sub(indent_w).max(1);
    let mut open: Option<usize> = None; // index into code_ranges of the open block
    for (i, bl) in body.into_iter().enumerate() {
        let lead: Vec<Span<'static>> = if i == 0 {
            prefix.clone()
        } else {
            vec![Span::styled(indent.clone(), Style::new())]
        };
        let crate::markdown::BodyLine { line, kind, source } = bl;
        match kind {
            BodyKind::Prose => {
                open = None; // a prose line closes any open code block
                let rows = wrap_content(&line.spans, content_w);
                for (r, row) in rows.into_iter().enumerate() {
                    let mut spans: Vec<Span<'static>> = if r == 0 {
                        lead.clone()
                    } else {
                        vec![Span::styled(indent.clone(), Style::new())]
                    };
                    spans.extend(row);
                    out.push(Line::from(spans));
                }
            }
            BodyKind::CodeHead => {
                let ln = out.len();
                // The head carries this block's raw source (see `markdown::BodyLine`).
                code_ranges.push((ln, ln, source.unwrap_or_default()));
                open = Some(code_ranges.len() - 1);
                out.push(code_line(lead, line.spans, content_w));
            }
            BodyKind::Code => {
                let ln = out.len();
                out.push(code_line(lead, line.spans, content_w));
                if let Some(o) = open {
                    code_ranges[o].1 = ln;
                }
            }
            BodyKind::Table => {
                open = None;
                let mut spans = lead;
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
        }
    }
}

/// Assemble one code-panel row: `lead` (prefix/indent) + the code spans, then
/// pad the code portion to `content_w` with the panel background so the block
/// reads as a rectangle. Padding is capped at `content_w` — a line already at or
/// over the width gets no pad (it wraps via the widget) rather than overflowing.
fn code_line(
    lead: Vec<Span<'static>>,
    code: Vec<Span<'static>>,
    content_w: usize,
) -> Line<'static> {
    let w: usize = code.iter().map(|s| s.content.width()).sum();
    let mut spans = lead;
    spans.extend(code);
    if content_w > w {
        spans.push(Span::styled(
            " ".repeat(content_w - w),
            Style::new().bg(color::CODE_BG),
        ));
    }
    Line::from(spans)
}

/// Break styled `content` into rows no wider than `width`, preserving each
/// span's style, breaking on spaces (dropping the break's whitespace), and
/// hard-splitting any single token longer than `width`. Returns at least one
/// (possibly empty) row.
fn wrap_content(content: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    // Tokenize into (text, style, is_space) runs, split at whitespace boundaries.
    let mut toks: Vec<(String, Style, bool)> = Vec::new();
    for s in content {
        let mut chars = s.content.chars().peekable();
        while let Some(&c) = chars.peek() {
            let is_space = c == ' ';
            let mut t = String::new();
            while let Some(&c2) = chars.peek() {
                if (c2 == ' ') != is_space {
                    break;
                }
                t.push(c2);
                chars.next();
            }
            toks.push((t, s.style, is_space));
        }
    }

    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    for (text, style, is_space) in toks {
        let w = text.width();
        if is_space {
            if cur.is_empty() {
                continue; // no leading spaces at the start of a wrapped row
            }
            if cur_w + w > width {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            } else {
                cur.push(Span::styled(text, style));
                cur_w += w;
            }
            continue;
        }
        if cur_w + w > width && !cur.is_empty() {
            // trim any trailing spaces before wrapping the row (cur_w resets to 0
            // right after, so no need to track its decrement here)
            while cur
                .last()
                .map(|s| s.content.chars().all(|c| c == ' '))
                .unwrap_or(false)
            {
                cur.pop();
            }
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if w > width {
            // Token longer than the column — hard-split by DISPLAY WIDTH, not char
            // count, so wide (CJK/emoji) glyphs never overflow the column and force
            // a widget re-wrap. Accumulate chars until the next one would exceed the
            // remaining width, then flush the row (always at least one char/row).
            let mut piece = String::new();
            let mut piece_w = 0usize;
            for ch in text.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if cur_w + piece_w + cw > width && cur_w + piece_w > 0 {
                    if !piece.is_empty() {
                        cur.push(Span::styled(std::mem::take(&mut piece), style));
                    }
                    piece_w = 0;
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                piece.push(ch);
                piece_w += cw;
            }
            if !piece.is_empty() {
                cur.push(Span::styled(piece, style));
                cur_w += piece_w;
            }
        } else {
            cur.push(Span::styled(text, style));
            cur_w += w;
        }
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

/// Per-frame conversation view-model the bin assembles: altitude + caret blink
/// + an optional reveal cap (for the zoom transition animation, P4c Task 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatView {
    pub zoom: Zoom,
    pub caret_on: bool,
    pub reveal: Option<usize>,
    /// Local UTC offset in seconds, supplied by the bin, for the message-row
    /// `HH:MM` stamps. Tests pass 0 (UTC) so snapshots stay reproducible.
    pub tz_offset_secs: i32,
}

/// Build the conversation lines at the requested altitude, capped to
/// `view.reveal` lines when set. `width` is the text column width for prose wrap.
pub fn conversation_view(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    conversation_view_indexed(msgs, view, streaming, width, question).0
}

/// Like `conversation_view`, but also returns `msg_starts` (length msgs.len()):
/// the body line where each message's block begins at this altitude. Used for
/// cross-zoom position anchoring.
pub fn conversation_view_indexed(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut starts: Vec<usize> = Vec::new();
    let mut lines: Vec<Line<'static>> = match view.zoom {
        Zoom::Overview => {
            // Overview is not a transcript view — the bin renders it via
            // `overview::overview_lines`, bypassing this builder. Return empty
            // so the match is exhaustive and any accidental call is harmless.
            Vec::new()
        }
        Zoom::Summary => {
            starts = summary_msg_starts(msgs);
            digest_lines(&digests(msgs))
        }
        Zoom::Normal => {
            let mut hits = Vec::new();
            build_conversation(
                msgs,
                streaming,
                view.caret_on,
                view.tz_offset_secs,
                width,
                &mut hits,
                &mut starts,
                &mut Vec::new(),
                question,
            )
        }
        Zoom::Detail => detail_lines(msgs, view.tz_offset_secs, width, &mut starts, question),
    };
    if let Some(n) = view.reveal {
        lines.truncate(n);
    } else {
        // One trailing blank line so the last message clears the message box
        // (visual breathing room at the bottom of the pane). It's part of the
        // body, so max_scroll and tail-follow see it: pinned to the bottom the
        // blank is the last row and the newest message sits one line above the
        // input. Skipped during a reveal (top-anchored, no bottom to pad).
        lines.push(Line::from(""));
    }
    (lines, starts)
}

/// Per-message digest-line index for the Summary altitude, mirroring the turn
/// grouping in `zoom::digests`: a new turn begins at each `User` message, and a
/// leading non-user message opens turn 0. Result length == msgs.len(), entry i =
/// the digest line index (0-based) message i is folded into.
fn summary_msg_starts(msgs: &[ChatMsg]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(msgs.len());
    let mut turn: i64 = -1;
    for m in msgs {
        match m {
            ChatMsg::User { .. } => turn += 1,
            _ if turn == -1 => turn = 0, // leading non-user opens turn 0
            _ => {}
        }
        starts.push(turn.max(0) as usize);
    }
    starts
}

/// One digest line per turn: `› {headline}   ~ {tools}t · {files}f [⚠]`.
fn digest_lines(ds: &[TurnDigest]) -> Vec<Line<'static>> {
    ds.iter()
        .map(|d| {
            let mut spans = vec![
                Span::styled(
                    format!("{} ", glyph::USER_TURN),
                    Style::new().fg(color::CHAT_ACCENT),
                ),
                // `.40` precision truncates to 40 chars; width 40 pads short ones —
                // a HEADLINE_MAX(60) headline can't blow past the column and misalign
                // the `~ Nt · Nf` field in the 140-col snapshot.
                Span::styled(
                    format!("{:<40.40} ", d.headline),
                    Style::new().fg(color::TXT),
                ),
                Span::styled(
                    format!("~ {}t · {}f", d.tools, d.files),
                    Style::new().fg(color::DIM),
                ),
            ];
            if d.has_error {
                spans.push(Span::styled(
                    format!(" {}", glyph::WARNING),
                    Style::new().fg(color::ERROR),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// Detail altitude: the normal view, but file tool-results are rendered with
/// syntax highlighting (Ⓡ3, P4a). The file's language is inferred from the
/// originating tool call's path (correlated by id).
fn detail_lines(
    msgs: &[ChatMsg],
    tz_offset_secs: i32,
    width: usize,
    msg_starts: &mut Vec<usize>,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    use std::collections::HashMap;
    // id → file path, from assistant tool calls.
    let mut id_path: HashMap<&str, String> = HashMap::new();
    for m in msgs {
        if let ChatMsg::Assistant { tool_calls, .. } = m {
            for c in tool_calls {
                if let Some(p) = tool_path(&c.args) {
                    id_path.insert(c.id.as_str(), p);
                }
            }
        }
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for m in msgs {
        // Record each message's start line for cross-zoom anchoring (see
        // `build_conversation`); length ends up == msgs.len().
        msg_starts.push(out.len());
        match m {
            ChatMsg::ToolResult {
                id,
                name,
                output,
                is_error,
                compacted,
                ..
            } if !*is_error => {
                let label = if *compacted {
                    format!("  {} {} {}", glyph::PASS, name, glyph::COMPACT)
                } else {
                    format!("  {} {}", glyph::PASS, name)
                };
                let header = Span::styled(label, Style::new().fg(color::DIM));
                out.push(Line::from(vec![header]));
                let lang = id_path
                    .get(id.as_str())
                    .map(|p| Language::from_path(p))
                    .unwrap_or(Language::PlainText);
                out.extend(collapse_to_signatures(output, lang));
            }
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok {
                    (glyph::PASS, color::OK)
                } else {
                    (glyph::WARNING, color::ERROR)
                };
                out.push(Line::from(vec![
                    Span::styled(
                        format!("{} delegated ", glyph::EXPANDED),
                        Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG),
                    ),
                    Span::styled(format!("{mark}"), Style::new().fg(mark_color)),
                ]));
                // PLAN-1 seam: route the summary through Plan 1's markdown renderer.
                for line in crate::markdown::render_markdown(summary) {
                    let mut spans = vec![Span::styled("    ", Style::new())];
                    spans.extend(line.spans);
                    out.push(Line::from(spans));
                }
            }
            ChatMsg::Question {
                kind,
                question: qtext,
                choices,
                state,
                ..
            } => {
                blank_between_turns(&mut out);
                let (selected, free_text) = match state {
                    zoid_core::projection::QuestionCardState::Open {
                        selected,
                        free_text,
                    } => {
                        if let Some(q) = question {
                            (q.selected, q.free_text.clone())
                        } else {
                            (*selected, free_text.clone())
                        }
                    }
                    zoid_core::projection::QuestionCardState::Answered { .. } => (0, String::new()),
                };
                render_question_card(
                    &mut out,
                    &mut Vec::new(),
                    kind,
                    qtext,
                    choices,
                    state,
                    selected,
                    &free_text,
                    width,
                );
            }
            ChatMsg::Assistant {
                thinking,
                ..
            } => {
                // Thinking section (full text at Detail zoom).
                if let Some(thinking_text) = thinking {
                    if !thinking_text.is_empty() {
                        blank_between_turns(&mut out);
                        out.push(Line::from(vec![
                            Span::styled(
                                "─ Thinking ─────────────────────",
                                Style::new().fg(color::DIM),
                            ),
                        ]));
                        for line in crate::markdown::render_markdown(thinking_text) {
                            let mut spans = vec![Span::styled("    ", Style::new())];
                            spans.extend(line.spans);
                            out.push(Line::from(spans));
                        }
                        out.push(Line::from(""));
                    }
                }
                // Answer text (reuse the existing conversation_lines path)
                out.extend(conversation_lines(
                    std::slice::from_ref(m),
                    false,
                    true,
                    tz_offset_secs,
                    width,
                    question,
                ));
            }
            other => out.extend(conversation_lines(
                std::slice::from_ref(other),
                false,
                true,
                tz_offset_secs,
                width,
                question,
            )),
        }
    }
    out
}

/// Collapse a code file to signatures: highlight every line, but replace each
/// **leaf** fold body's interior lines with a single `…` marker. "Leaf" = a fold
/// containing no other fold, so a container (`impl`/`mod`) keeps its method
/// signatures while each method/struct/enum leaf body folds. Realizes spec Ⓡ3↔①
/// "collapse to signatures"; uses P4a's `fold_regions` (function + type/impl bodies).
pub(crate) fn collapse_to_signatures(source: &str, lang: Language) -> Vec<Line<'static>> {
    let all = highlight_lines(source, lang); // one Line per source line
    let folds = fold_regions(source, lang);
    if folds.is_empty() {
        return all;
    }
    // 0-based line index of a byte offset = count of '\n' before it.
    let line_of = |byte: usize| {
        source[..byte.min(source.len())]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
    };
    let is_leaf = |f: &FoldRegion, i: usize| {
        !folds
            .iter()
            .enumerate()
            .any(|(j, g)| j != i && g.start >= f.start && g.end <= f.end)
    };
    let mut elided = vec![false; all.len()];
    for (i, f) in folds.iter().enumerate() {
        if is_leaf(f, i) {
            // Keep the opening (signature) line and the closing line; elide between.
            for ln in (line_of(f.start) + 1)..line_of(f.end) {
                if ln < elided.len() {
                    elided[ln] = true;
                }
            }
        }
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < all.len() {
        if elided[i] {
            out.push(Line::from(Span::styled(
                format!("    {}", glyph::ELLIPSIS),
                Style::new().fg(color::DIM),
            )));
            while i < all.len() && elided[i] {
                i += 1;
            }
        } else {
            out.push(all[i].clone());
            i += 1;
        }
    }
    out
}

/// Render the Chat surface: title bar, conversation column, input box, status bar.
/// When `streaming` is true, a caret `▌` trails the in-progress assistant text
/// (only when the last message is an Assistant turn with no tool calls).
pub fn render_chat(frame: &mut Frame, msgs: &[ChatMsg], input: &TextArea<'_>, streaming: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(3), // input box (bordered)
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    // Title bar (unchanged).
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(
            format!("{} main", glyph::BRANCH),
            Style::new().fg(color::BRANCH),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Conversation: user/assistant text turns + inline tool cards. This legacy
    // standalone renderer (tests only) has no view-model, so stamps in UTC.
    let body = conversation_lines(msgs, streaming, true, 0, chunks[1].width as usize, None);
    frame.render_widget(Paragraph::new(body), chunks[1]);

    // Input box (bordered text area).
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(input_block, chunks[2]);
    let inner = chunks[2].inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(input, inner);

    // Status bar.
    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!(
                "· {}Tab Build · {} send · ^Q quit",
                glyph::SHIFT,
                glyph::RETURN
            ),
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[3]);
}

/// A compact one-line summary of a tool call's JSON args for the inline card.
fn arg_summary(args_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    }
}

fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 30),
        other => truncate(&other.to_string(), 30),
    }
}

fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 40)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_shows_only_when_streaming_and_caret_on() {
        use crate::tokens::glyph;
        let msgs = vec![ChatMsg::Assistant {
            thinking: None,
            text: "hi".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let has_caret = |streaming, caret| {
            conversation_lines(&msgs, streaming, caret, 0, 80, None)
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains(glyph::CARET)))
        };
        assert!(has_caret(true, true), "streaming + caret_on → caret shown");
        assert!(
            !has_caret(true, false),
            "caret_on=false suppresses caret while streaming"
        );
        assert!(!has_caret(false, true), "not streaming → no caret");
    }

    use crate::state::Zoom;
    use zoid_core::projection::ToolCallRef;

    // The Assistant carries a tool_call whose id matches the ToolResult and whose
    // args name a `.rs` path — so `detail_lines` resolves id→path→Language::Rust
    // and actually highlights (without this, id_path is empty and Detail silently
    // falls back to PlainText, the exact gap that made the old fixture useless).
    // The body is multi-line so collapse-to-signatures (Task 3b) has something to fold.
    fn seeded() -> Vec<ChatMsg> {
        vec![
            ChatMsg::User {
                text: "fix the parser bug".into(),
                ts: 0,
            },
            ChatMsg::Assistant {
                    thinking: None,
                text: "on it".into(),
                tool_calls: vec![ToolCallRef {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: r#"{"path":"src/parser.rs"}"#.into(),
                }],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
                is_error: false,
                compacted: false,
                ts: 0,
            },
            ChatMsg::User {
                text: "thanks".into(),
                ts: 0,
            },
        ]
    }

    fn view(zoom: Zoom) -> ChatView {
        ChatView {
            zoom,
            caret_on: true,
            reveal: None,
            tz_offset_secs: 0,
        }
    }

    #[test]
    fn conversation_view_indexed_starts_len_matches_msgs_at_each_zoom() {
        let msgs = vec![
            ChatMsg::User {
                text: "first question".into(),
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: "an answer".into(),
                tool_calls: vec![],
                ts: 0,
            },
            ChatMsg::User {
                text: "second question".into(),
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: "another answer".into(),
                tool_calls: vec![],
                ts: 0,
            },
        ];
        for zoom in [Zoom::Summary, Zoom::Normal, Zoom::Detail] {
            let v = view(zoom);
            let (lines, starts) = conversation_view_indexed(&msgs, &v, false, 80, None);
            assert_eq!(
                starts.len(),
                msgs.len(),
                "one start per message at {zoom:?}"
            );
            assert!(
                starts.windows(2).all(|w| w[0] <= w[1]),
                "starts not monotonic at {zoom:?}: {starts:?}"
            );
            assert!(
                starts.iter().all(|&s| s < lines.len().max(1)),
                "start past body at {zoom:?}: {starts:?} vs {} lines",
                lines.len()
            );
            if zoom == Zoom::Summary {
                // two turns collapse: msgs 0&1 → line 0, msgs 2&3 → line 1
                assert_eq!(starts, vec![0, 0, 1, 1]);
            }
        }
    }

    #[test]
    fn summary_collapses_to_one_line_per_turn() {
        let lines = conversation_view(&seeded(), &view(Zoom::Summary), false, 80, None);
        // two turns → two digest lines, plus one trailing breathing-room blank
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn detail_highlights_file_tool_results() {
        use crate::tokens::color;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false, 80, None);
        // A keyword (`fn`/`let`) must carry the syntax keyword color — proves the
        // id→path→Rust resolution fired and highlighting actually ran, rather than
        // silently falling back to PlainText (which colors everything TXT).
        let has_keyword_color = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(color::SYN_KEYWORD))
        });
        assert!(
            has_keyword_color,
            "Detail must highlight the Rust tool-result body"
        );
    }

    #[test]
    fn detail_collapses_function_bodies_to_signatures() {
        use crate::tokens::glyph;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false, 80, None);
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            text.iter().any(|t| t.contains("fn parse")),
            "signature line is kept"
        );
        assert!(
            text.iter().any(|t| t.contains(glyph::ELLIPSIS)),
            "body collapses to …"
        );
        assert!(
            !text.iter().any(|t| t.contains("let n = 42")),
            "body interior is elided"
        );
    }

    #[test]
    fn normal_matches_conversation_lines() {
        let msgs = seeded();
        let normal = conversation_view(&msgs, &view(Zoom::Normal), false, 80, None);
        let baseline = conversation_lines(&msgs, false, true, 0, 80, None);
        // conversation_view appends one trailing breathing-room blank line.
        assert_eq!(normal.len(), baseline.len() + 1);
    }

    #[test]
    fn reveal_caps_line_count() {
        let mut v = view(Zoom::Normal);
        v.reveal = Some(1);
        let lines = conversation_view(&seeded(), &v, false, 80, None);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn assistant_body_renders_markdown() {
        use crate::tokens::color;
        let msgs = vec![ChatMsg::Assistant {
            thinking: None,
            text: "run **now**\n\n```rust\nfn x() {}\n```".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let spans: Vec<(String, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style)))
            .collect();
        // bold inline text survived markdown
        assert!(spans
            .iter()
            .any(|(t, st)| t == "now" && st.add_modifier.contains(ratatui::style::Modifier::BOLD)));
        // the fenced rust block was syntax-highlighted
        assert!(spans
            .iter()
            .any(|(_, st)| st.fg == Some(color::SYN_KEYWORD)));
        // the "zoid " role prefix still leads the first line
        assert!(spans.iter().any(|(t, _)| t == "zoid "));
    }

    // Each clickable code range must carry ITS OWN source. Two code blocks in
    // separate turns → two hits, each mapping to the block the user sees.
    #[test]
    fn code_hits_pair_each_block_with_its_own_source() {
        let msgs = vec![
            ChatMsg::Assistant {
                thinking: None,
                text: "first\n\n```rust\nlet a = 1;\n```".into(),
                tool_calls: vec![],
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: "second\n\n```rust\nlet b = 2;\n```".into(),
                tool_calls: vec![],
                ts: 0,
            },
        ];
        let hits = code_hits(&msgs, false, true, 0, 80, None);
        assert_eq!(hits.len(), 2, "one hit per top-level block");
        assert!(hits[0].source.contains("let a = 1;"));
        assert!(hits[1].source.contains("let b = 2;"));
        // The range points at real, rendered panel rows.
        for h in &hits {
            assert!(h.header_line <= h.end_line);
        }
    }

    // Regression (gilfoyle CRITICAL): a message whose markdown over-nests bails to
    // plain text, emitting NO code panel. Its fence must not leak a phantom source
    // that shifts every later block's clipboard mapping. Deriving source from the
    // same render pass makes the bail contribute 0 ranges AND 0 sources.
    #[test]
    fn bailed_message_does_not_desync_later_code_sources() {
        // 9 levels of list nesting > MAX_DEPTH(8) → render_body bails to plain text,
        // discarding the top-level fence that precedes it.
        let bailing = concat!(
            "```rust\nlet PHANTOM = 0;\n```\n\n",
            "- l1\n  - l2\n    - l3\n      - l4\n        - l5\n",
            "          - l6\n            - l7\n              - l8\n                - l9\n",
        );
        let msgs = vec![
            ChatMsg::Assistant {
                thinking: None,
                text: bailing.into(),
                tool_calls: vec![],
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: "```rust\nlet real = 42;\n```".into(),
                tool_calls: vec![],
                ts: 0,
            },
        ];
        let hits = code_hits(&msgs, false, true, 0, 80, None);
        // Only the second (non-bailed) block is clickable…
        assert_eq!(hits.len(), 1, "bailed message emits no clickable block");
        // …and it copies its OWN source, never the phantom from the bailed fence.
        assert!(
            hits[0].source.contains("let real = 42;"),
            "clicked block copies its own source"
        );
        assert!(
            !hits[0].source.contains("PHANTOM"),
            "no phantom source leaked from the bailed message"
        );
    }

    #[test]
    fn delegated_card_renders_chevron_status_and_bg() {
        use crate::tokens::{color, glyph};
        let msgs = vec![ChatMsg::Delegated {
            summary: "Added shared NotFound helper.".into(),
            ok: true,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(glyph::COLLAPSED),
            "collapsed chevron ▸ present"
        );
        assert!(joined.contains("delegated"));
        assert!(joined.contains(glyph::PASS), "done status ✓ present");
        // The card label carries the delegate background (proves §16 token use).
        assert!(lines.iter().any(|l| l
            .spans
            .iter()
            .any(|s| s.style.bg == Some(color::DELEGATE_BG))));
    }

    #[test]
    fn open_question_card_renders_choices_and_highlight() {
        use zoid_core::event::QuestionKind;
        use zoid_core::projection::QuestionCardState;
        let msgs = vec![ChatMsg::Question {
            id: "c1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
            state: QuestionCardState::Open {
                selected: 1,
                free_text: String::new(),
            },
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(joined.contains("retry or skip?"), "question text rendered");
        assert!(
            joined.contains("○ Retry"),
            "first choice rendered unselected"
        );
        assert!(joined.contains("● Skip"), "second choice rendered selected");
    }

    #[test]
    fn answered_question_card_collapses_to_answer_line() {
        use zoid_core::event::QuestionKind;
        use zoid_core::projection::QuestionCardState;
        let msgs = vec![ChatMsg::Question {
            id: "c1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
            state: QuestionCardState::Answered {
                answer: "Skip".into(),
            },
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains("└ ► Skip"),
            "answered card shows the answer"
        );
        assert!(
            !joined.contains("○ Retry"),
            "answered card does not re-render choices"
        );
    }
}
