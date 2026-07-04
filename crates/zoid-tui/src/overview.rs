//! The Overview zoom page: a whole-session metrics dashboard rendered in the
//! conversation pane at `Zoom::Overview`. `overview_lines` is pure; the bin
//! assembles `OverviewData` from the obs aggregate snapshot + economy.

use crate::text::truncate;
use crate::tokens::color;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverviewData {
    pub session_id: String,
    pub model: String,
    pub provider: String,
    pub uptime: String,
    pub turns: u64,
    pub tok_in: u64,
    pub tok_out: u64,
    pub tok_total: u64,
    pub cache_read: u64,
    pub cache_hit_pct: u8,
    pub spark: String,
    pub turn_last_ms: u64,
    pub turn_avg_ms: u64,
    pub turn_p90_ms: u64,
    pub ttft_ms: u64,
    pub stream_ms: u64,
    pub iter_avg: u64,
    pub tools: Vec<(String, u64, u64)>, // (name, count, avg_ms)
    pub frame_avg_ms: u64,
    pub frame_p90_ms: u64,
    pub frame_max_ms: u64,
    pub render_cache_pct: u8,
    pub proj_rebuilds: u64,
    pub event_count: u64,
    pub errors: Vec<(String, String)>, // (prefix, message)
}

/// Two-column layout kicks in at this width; below it we stack (the 100×24
/// floor lands at ~51 content columns and must degrade, never overflow).
const TWO_COL_MIN: usize = 90;

/// A pre-styled cell: owned spans plus their total display width.
struct Cell {
    spans: Vec<Span<'static>>,
    w: usize,
}

fn plain(s: String) -> Cell {
    let w = UnicodeWidthStr::width(s.as_str());
    Cell {
        spans: vec![Span::raw(s)],
        w,
    }
}

fn styled(s: String, st: Style) -> Cell {
    let w = UnicodeWidthStr::width(s.as_str());
    Cell {
        spans: vec![Span::styled(s, st)],
        w,
    }
}

fn empty() -> Cell {
    Cell {
        spans: vec![],
        w: 0,
    }
}

/// Shrink a cell to at most `max` columns. When it already fits, spans (and
/// their colors) are preserved; on overflow we collapse to a single truncated
/// span — a graceful, non-panicking degrade rather than a clip at render time.
fn fit(cell: Cell, max: usize) -> Cell {
    if cell.w <= max {
        return cell;
    }
    let text: String = cell.spans.iter().map(|s| s.content.as_ref()).collect();
    plain(truncate(&text, max))
}

/// Build a `Line` from spans, guaranteeing `<= max` display columns (collapse
/// to one truncated span on overflow).
fn fit_line(spans: Vec<Span<'static>>, max: usize) -> Line<'static> {
    let w: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if w <= max {
        Line::from(spans)
    } else {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        Line::from(Span::raw(truncate(&text, max)))
    }
}

fn accent() -> Style {
    Style::new().fg(color::CHAT_ACCENT)
}
fn dim() -> Style {
    Style::new().fg(color::DIM)
}

/// Compact token count with one decimal: `48_200 → "48.2k"`, `1_200_000 → "1.2M"`.
fn fmt_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Milliseconds → seconds with one decimal: `4200 → "4.2s"`.
fn fmt_s(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Thousands-grouped integer: `1204 → "1,204"`.
fn commas(n: u64) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn economy_cells(d: &OverviewData) -> Vec<Cell> {
    let mut spark_spans = vec![Span::raw("  per-turn ".to_string())];
    let spark_w = UnicodeWidthStr::width(d.spark.as_str());
    spark_spans.push(Span::styled(d.spark.clone(), accent()));
    vec![
        styled("ECONOMY".to_string(), accent()),
        plain(format!("  {:<9}{}", "input", fmt_k(d.tok_in))),
        plain(format!("  {:<9}{}", "output", fmt_k(d.tok_out))),
        plain(format!(
            "  {:<9}{}   hit {}%",
            "cache",
            fmt_k(d.cache_read),
            d.cache_hit_pct
        )),
        Cell {
            spans: spark_spans,
            w: 11 + spark_w,
        },
    ]
}

fn timing_cells(d: &OverviewData) -> Vec<Cell> {
    vec![
        styled("TIMING".to_string(), accent()),
        plain(format!(
            "  turn   last {}   avg {}   p90 {}",
            fmt_s(d.turn_last_ms),
            fmt_s(d.turn_avg_ms),
            fmt_s(d.turn_p90_ms)
        )),
        plain(format!(
            "  provider   ttft {}   stream {}",
            fmt_s(d.ttft_ms),
            fmt_s(d.stream_ms)
        )),
        plain(format!("  iterations   {} avg / turn", d.iter_avg)),
    ]
}

fn tools_cells(d: &OverviewData) -> Vec<Cell> {
    let mut cells = vec![styled("TOOLS".to_string(), accent())];
    for (name, count, avg_ms) in &d.tools {
        cells.push(plain(format!(
            "  {:<11}×{:<4}avg {}ms",
            name, count, avg_ms
        )));
    }
    cells
}

fn runtime_cells(d: &OverviewData) -> Vec<Cell> {
    vec![
        styled("RUNTIME".to_string(), accent()),
        plain(format!(
            "  frame   avg {}ms   p90 {}ms   max {}ms",
            d.frame_avg_ms, d.frame_p90_ms, d.frame_max_ms
        )),
        plain(format!(
            "  cache-hit   {}%   (body render)",
            d.render_cache_pct
        )),
        plain(format!("  projections rebuilt   {}", d.proj_rebuilds)),
        plain(format!("  event log   {} events", commas(d.event_count))),
    ]
}

/// Join a left+right cell with the dim ` │ ` separator, padding the left cell to
/// `col_width` so the rules align. When the right cell is empty the line ends at
/// the bar (no trailing space).
fn compose(left: Cell, right: Cell, col_width: usize) -> Line<'static> {
    let left = fit(left, col_width);
    let right = fit(right, col_width);
    let mut spans = left.spans;
    let pad = col_width.saturating_sub(left.w);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if right.w == 0 {
        spans.push(Span::styled(" │".to_string(), dim()));
    } else {
        spans.push(Span::styled(" │ ".to_string(), dim()));
        spans.extend(right.spans);
    }
    Line::from(spans)
}

fn header_line(d: &OverviewData, width: usize) -> Line<'static> {
    let meta = format!(
        "   session {} · {} ({}) · up {} · {} turns",
        d.session_id, d.model, d.provider, d.uptime, d.turns
    );
    fit_line(
        vec![
            Span::styled("OVERVIEW".to_string(), accent()),
            Span::styled(meta, dim()),
        ],
        width,
    )
}

fn kpi_line(d: &OverviewData, width: usize) -> Line<'static> {
    let sep = || Span::styled(" · ".to_string(), dim());
    let errs = d.errors.len();
    let ok = Style::new().fg(color::OK);
    let err_style = if errs > 0 {
        Style::new().fg(color::ERROR)
    } else {
        dim()
    };
    let spans = vec![
        Span::raw("  ".to_string()),
        Span::styled(format!("{} tokens", fmt_k(d.tok_total)), ok),
        sep(),
        Span::styled(format!("{}% cache-hit", d.cache_hit_pct), ok),
        sep(),
        Span::raw(format!("{} avg turn", fmt_s(d.turn_avg_ms))),
        sep(),
        Span::raw(format!("{}ms/frame", d.frame_avg_ms)),
        sep(),
        Span::styled(format!("{} errors", errs), err_style),
    ];
    fit_line(spans, width)
}

fn error_lines(d: &OverviewData, width: usize) -> Vec<Line<'static>> {
    d.errors
        .iter()
        .map(|(prefix, msg)| {
            let pw = UnicodeWidthStr::width(prefix.as_str());
            let avail = width.saturating_sub(4 + pw);
            let msg = truncate(msg, avail);
            fit_line(
                vec![
                    Span::raw("  ".to_string()),
                    Span::styled(prefix.clone(), Style::new().fg(color::WARN)),
                    Span::raw(format!("  {msg}")),
                ],
                width,
            )
        })
        .collect()
}

/// Layout C: header, heavy-ruled KPI strip, a two-column body (ECONOMY+TOOLS |
/// TIMING+RUNTIME) when `width >= 90` else a stacked single column, then an
/// ERRORS band. Pure: takes only plain data + width; never panics or overflows.
pub fn overview_lines(data: &OverviewData, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let heavy = "═".repeat(width);

    lines.push(header_line(data, width));
    lines.push(Line::from(Span::styled(heavy.clone(), dim())));
    lines.push(kpi_line(data, width));
    lines.push(Line::from(Span::styled(heavy, dim())));

    if width >= TWO_COL_MIN {
        let col_width = (width - 3) / 2;

        let econ = economy_cells(data);
        let tim = timing_cells(data);
        let tools = tools_cells(data);
        let run = runtime_cells(data);

        // Align the second block (TOOLS / RUNTIME) by padding each top block to a
        // shared height (one guaranteed spacer row between the blocks).
        let top = econ.len().max(tim.len()) + 1;
        let mut left = econ;
        while left.len() < top {
            left.push(empty());
        }
        left.extend(tools);
        let mut right = tim;
        while right.len() < top {
            right.push(empty());
        }
        right.extend(run);

        let rows = left.len().max(right.len());
        while left.len() < rows {
            left.push(empty());
        }
        while right.len() < rows {
            right.push(empty());
        }
        for (l, r) in left.into_iter().zip(right.into_iter()) {
            lines.push(compose(l, r, col_width));
        }

        // Rule joining the two columns, with a ┴ under the separator bar.
        let bar = col_width + 1;
        let after = width.saturating_sub(bar + 1);
        let join = format!("{}┴{}", "─".repeat(bar), "─".repeat(after));
        lines.push(Line::from(Span::styled(join, dim())));
    } else {
        // Degrade floor: stack the sections in a single readable column.
        for section in [
            economy_cells(data),
            timing_cells(data),
            tools_cells(data),
            runtime_cells(data),
        ] {
            for cell in section {
                lines.push(fit_line(fit(cell, width).spans, width));
            }
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled("─".repeat(width), dim())));
    }

    lines.push(Line::from(Span::styled(
        format!("ERRORS ({})", data.errors.len()),
        Style::new().fg(color::ERROR),
    )));
    lines.extend(error_lines(data, width));

    lines
}

#[cfg(test)]
fn sample() -> OverviewData {
    OverviewData {
        session_id: "a3f2".into(),
        model: "glm-5.2:cloud".into(),
        provider: "ollama".into(),
        uptime: "12m".into(),
        turns: 8,
        tok_in: 48200,
        tok_out: 6100,
        tok_total: 54300,
        cache_read: 31000,
        cache_hit_pct: 64,
        spark: "▁▂▃▅▇▆▄".into(),
        turn_last_ms: 4200,
        turn_avg_ms: 3800,
        turn_p90_ms: 7100,
        ttft_ms: 600,
        stream_ms: 3100,
        iter_avg: 3,
        tools: vec![
            ("read_file".into(), 14, 12),
            ("shell".into(), 6, 240),
            ("edit_file".into(), 3, 18),
        ],
        frame_avg_ms: 7,
        frame_p90_ms: 11,
        frame_max_ms: 16,
        render_cache_pct: 98,
        proj_rebuilds: 42,
        event_count: 1204,
        errors: vec![
            ("⚠ 12m provider".into(), "HTTP 429: rate limited".into()),
            (
                "⛔ 3m shell".into(),
                "exit 1: ./deploy.sh: no such file".into(),
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn overview_dashboard_160x40() {
        // conversation content width at 160×40 baseline ≈ 110 cols.
        let lines = overview_lines(&sample(), 110);
        for l in &lines {
            let w: usize = l
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= 110, "line exceeded width 110: {w}");
        }
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!("overview_160x40", text);
    }

    #[test]
    fn overview_dashboard_100x24_floor() {
        // degrade floor: ~51 cols. Must not panic or overflow.
        let lines = overview_lines(&sample(), 51);
        assert!(!lines.is_empty());
        for l in &lines {
            let w: usize = l
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= 51, "line exceeded width 51: {w}");
        }
        insta::assert_snapshot!(
            "overview_100x24",
            lines
                .iter()
                .map(|l| l
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
