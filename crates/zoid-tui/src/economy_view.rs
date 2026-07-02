//! Pure view-model for the ⑤ economy drawer (Ⓡ4 dataviz). Turns core
//! projections into render-ready strings (heat bars, churn + cache sparklines).
//! No `Frame`; unit-tested independently of rendering.

use crate::tokens::{color, glyph};
use ratatui::style::Color;
use zoid_core::context::{ContextWindow, Heat};
use zoid_core::economy::ChurnTimeline;

/// Compact token count: `4000 → "4k"`, `1_200_000 → "1.2M"`.
pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        format!("{m:.1}M")
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

pub fn heat_bar(heat: Heat) -> String {
    let (a, b) = match heat {
        Heat::Hot => (glyph::HEAT_FULL, glyph::HEAT_FULL),
        Heat::Warm => (glyph::HEAT_FULL, glyph::HEAT_SHADE),
        Heat::Cold => (glyph::HEAT_SHADE, glyph::HEAT_SHADE),
    };
    format!("{a}{b}")
}

pub fn heat_color(heat: Heat) -> Color {
    match heat {
        Heat::Hot => color::HEAT_HOT,
        Heat::Warm => color::HEAT_WARM,
        Heat::Cold => color::HEAT_COLD,
    }
}

/// A fixed-width progress gauge: `frac` (clamped to 0..1) filled over `width`
/// cells using the heat-bar glyphs (`█` filled, `░` empty). Shared by the
/// session context meter and the context-drawer cache bar.
pub fn gauge(frac: f64, width: usize) -> String {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = ((frac * width as f64).round() as usize).min(width);
    (0..width)
        .map(|i| {
            if i < filled {
                glyph::HEAT_FULL
            } else {
                glyph::HEAT_SHADE
            }
        })
        .collect()
}

/// The last `n` cells of a sparkline string, preserving order (each sparkline
/// glyph is one display cell). Windows a growing per-turn series to the most
/// recent `n` turns so a long session can't push it past the rail edge.
pub fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

/// Map values onto the 8-step sparkline ramp (max → top).
pub fn sparkline(values: &[u64]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| {
            let idx = if max == 0 {
                0
            } else {
                ((v as u128 * (glyph::SPARK.len() as u128 - 1)) / max as u128) as usize
            };
            glyph::SPARK[idx]
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomyRow {
    pub pinned: bool,
    pub label: String,
    pub tokens: String,
    pub heat: Heat,
    pub cold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomyView {
    pub rows: Vec<EconomyRow>,
    pub churn: String,
    pub selected: usize,
    /// Per-turn prompt-cache sparkline (cache-read tokens per turn), mirroring
    /// `churn`; the churn line renders this pulled right.
    pub cache: String,
    /// Whether any cache reads were reported this session (else the sparkline is
    /// flat/empty and dimmed — the model/provider doesn't report cache reads).
    pub cache_active: bool,
}

impl EconomyView {
    pub fn build(window: &ContextWindow, churn: &ChurnTimeline, selected: usize) -> Self {
        let rows = window
            .items
            .iter()
            .map(|i| EconomyRow {
                pinned: i.pinned,
                label: i.label.clone(),
                tokens: human_tokens(i.tokens),
                heat: i.heat,
                cold: i.heat == Heat::Cold,
            })
            .collect();
        let churn_vals: Vec<u64> = churn.points.iter().map(|p| p.tokens).collect();
        let cache_vals: Vec<u64> = churn.points.iter().map(|p| p.cached).collect();
        Self {
            rows,
            churn: sparkline(&churn_vals),
            selected,
            cache: sparkline(&cache_vals),
            cache_active: cache_vals.iter().any(|&c| c > 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{color, glyph};
    use zoid_core::context::{ContextItem, ContextWindow, Heat, ItemKind};
    use zoid_core::economy::{ChurnPoint, ChurnTimeline};

    #[test]
    fn human_tokens_scales() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(123), "123");
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1000), "1k");
        assert_eq!(human_tokens(4000), "4k");
        assert_eq!(human_tokens(4500), "4k"); // floor to k
        assert_eq!(human_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn heat_bar_glyphs() {
        assert_eq!(heat_bar(Heat::Hot), format!("{0}{0}", glyph::HEAT_FULL));
        assert_eq!(
            heat_bar(Heat::Warm),
            format!("{}{}", glyph::HEAT_FULL, glyph::HEAT_SHADE)
        );
        assert_eq!(heat_bar(Heat::Cold), format!("{0}{0}", glyph::HEAT_SHADE));
    }

    #[test]
    fn heat_color_tokens() {
        assert_eq!(heat_color(Heat::Hot), color::HEAT_HOT);
        assert_eq!(heat_color(Heat::Warm), color::HEAT_WARM);
        assert_eq!(heat_color(Heat::Cold), color::HEAT_COLD);
    }

    #[test]
    fn gauge_fills_proportionally_and_clamps() {
        assert_eq!(gauge(0.0, 5), format!("{}", glyph::HEAT_SHADE).repeat(5));
        assert_eq!(gauge(1.0, 5), format!("{}", glyph::HEAT_FULL).repeat(5));
        assert_eq!(gauge(2.0, 4), format!("{}", glyph::HEAT_FULL).repeat(4)); // clamps
                                                                              // 0.5 of 4 → 2 filled
        assert_eq!(
            gauge(0.5, 4),
            format!("{0}{0}{1}{1}", glyph::HEAT_FULL, glyph::HEAT_SHADE)
        );
        // non-finite guards to empty
        assert_eq!(
            gauge(f64::NAN, 3),
            format!("{}", glyph::HEAT_SHADE).repeat(3)
        );
    }

    #[test]
    fn tail_windows_to_last_n_cells() {
        assert_eq!(tail("abcde", 3), "cde"); // keeps the most recent cells, in order
        assert_eq!(tail("ab", 5), "ab"); // shorter than budget → unchanged
        assert_eq!(tail("abc", 0), ""); // zero budget → empty
        assert_eq!(tail("", 4), ""); // empty in → empty out
    }

    #[test]
    fn sparkline_maps_range() {
        assert_eq!(sparkline(&[]), "");
        assert_eq!(sparkline(&[0, 0]), format!("{0}{0}", glyph::SPARK[0]));
        let s = sparkline(&[1, 4, 8]);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(s.chars().next().unwrap(), glyph::SPARK[0]); // min (1 with max 8) → idx 0
        assert_eq!(s.chars().last().unwrap(), *glyph::SPARK.last().unwrap()); // max maps to top of ramp
    }

    #[test]
    fn build_populates_rows_and_sparklines() {
        let w = ContextWindow {
            items: vec![
                ContextItem {
                    key: "file:a.rs".into(),
                    label: "a.rs".into(),
                    kind: ItemKind::File,
                    tokens: 4000,
                    heat: Heat::Hot,
                    pinned: true,
                    evicted: false,
                },
                ContextItem {
                    key: "file:c.sql".into(),
                    label: "c.sql".into(),
                    kind: ItemKind::File,
                    tokens: 5000,
                    heat: Heat::Cold,
                    pinned: false,
                    evicted: false,
                },
            ],
            total_tokens: 9000,
        };
        let churn = ChurnTimeline {
            points: vec![
                ChurnPoint {
                    turn: 0,
                    tokens: 10,
                    cached: 0,
                    resent_tokens: 0,
                },
                ChurnPoint {
                    turn: 1,
                    tokens: 80,
                    cached: 0,
                    resent_tokens: 5,
                },
            ],
        };
        let v = EconomyView::build(&w, &churn, 0);
        assert_eq!(v.rows.len(), 2);
        assert!(v.rows[0].pinned);
        assert!(v.rows[1].cold);
        assert_eq!(v.churn.chars().count(), 2);
        // no cache reads → inactive; sparkline has one cell per turn (all min)
        assert!(!v.cache_active);
        assert_eq!(v.cache.chars().count(), 2);
        assert_eq!(v.cache, format!("{}", glyph::SPARK[0]).repeat(2));
    }

    #[test]
    fn build_reports_cache_when_present() {
        let w = ContextWindow {
            items: vec![],
            total_tokens: 0,
        };
        // per-turn cache reads → an active sparkline, one cell per turn.
        let churn = ChurnTimeline {
            points: vec![
                ChurnPoint {
                    turn: 0,
                    tokens: 100,
                    cached: 0,
                    resent_tokens: 0,
                },
                ChurnPoint {
                    turn: 1,
                    tokens: 100,
                    cached: 500,
                    resent_tokens: 0,
                },
            ],
        };
        let v = EconomyView::build(&w, &churn, 0);
        assert!(v.cache_active);
        assert_eq!(v.cache.chars().count(), 2); // one cell per turn
                                                // turn 0 had no cache (min ramp), turn 1 was the max (top ramp)
        assert_eq!(v.cache.chars().next().unwrap(), glyph::SPARK[0]);
        assert_eq!(
            v.cache.chars().last().unwrap(),
            *glyph::SPARK.last().unwrap()
        );
    }
}
