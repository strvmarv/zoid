//! Pure view-model for the ⑤ economy drawer (Ⓡ4 dataviz). Turns core
//! projections into render-ready strings (heat bars, churn sparkline, token
//! ledger). No `Frame`; unit-tested independently of rendering.

use crate::tokens::{color, glyph};
use ratatui::style::Color;
use zoid_core::assembler::ContextPolicy;
use zoid_core::context::{ContextWindow, Heat};
use zoid_core::economy::{ChurnTimeline, TokenLedger};

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
    pub ledger: String,
    pub over_ceiling: bool,
    pub auto_evict_cold: bool,
    pub selected: usize,
}

impl EconomyView {
    pub fn build(
        window: &ContextWindow,
        churn: &ChurnTimeline,
        ledger: &TokenLedger,
        policy: &ContextPolicy,
        selected: usize,
    ) -> Self {
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
        let used = ledger.total;
        let ledger_label = match policy.token_ceiling {
            Some(c) => format!("{}/{}", human_tokens(used), human_tokens(c)),
            None => human_tokens(used),
        };
        Self {
            rows,
            churn: sparkline(&churn_vals),
            ledger: ledger_label,
            over_ceiling: policy.token_ceiling.is_some_and(|c| used > c),
            auto_evict_cold: policy.auto_evict_cold,
            selected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{color, glyph};
    use zoid_core::context::{ContextItem, ContextWindow, Heat, ItemKind};
    use zoid_core::economy::{ChurnPoint, ChurnTimeline, TokenLedger};
    use zoid_core::assembler::ContextPolicy;

    #[test]
    fn human_tokens_scales() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(123), "123");
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1000), "1k");
        assert_eq!(human_tokens(4000), "4k");
        assert_eq!(human_tokens(4500), "4k");        // floor to k
        assert_eq!(human_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn heat_bar_glyphs() {
        assert_eq!(heat_bar(Heat::Hot), format!("{0}{0}", glyph::HEAT_FULL));
        assert_eq!(heat_bar(Heat::Warm), format!("{}{}", glyph::HEAT_FULL, glyph::HEAT_SHADE));
        assert_eq!(heat_bar(Heat::Cold), format!("{0}{0}", glyph::HEAT_SHADE));
    }

    #[test]
    fn heat_color_tokens() {
        assert_eq!(heat_color(Heat::Hot), color::HEAT_HOT);
        assert_eq!(heat_color(Heat::Warm), color::HEAT_WARM);
        assert_eq!(heat_color(Heat::Cold), color::HEAT_COLD);
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
    fn build_populates_rows_and_ledger() {
        let w = ContextWindow {
            items: vec![
                ContextItem { key: "file:a.rs".into(), label: "a.rs".into(), kind: ItemKind::File, tokens: 4000, heat: Heat::Hot, pinned: true, evicted: false },
                ContextItem { key: "file:c.sql".into(), label: "c.sql".into(), kind: ItemKind::File, tokens: 5000, heat: Heat::Cold, pinned: false, evicted: false },
            ],
            total_tokens: 9000,
        };
        let churn = ChurnTimeline { points: vec![ChurnPoint { turn: 0, tokens: 10, resent_tokens: 0 }, ChurnPoint { turn: 1, tokens: 80, resent_tokens: 5 }] };
        let ledger = TokenLedger { input: 9000, output: 1000, cached: 0, total: 10_000 };
        let policy = ContextPolicy { token_ceiling: Some(200_000), ..Default::default() };
        let v = EconomyView::build(&w, &churn, &ledger, &policy, 0);
        assert_eq!(v.rows.len(), 2);
        assert!(v.rows[0].pinned);
        assert!(v.rows[1].cold);
        assert_eq!(v.ledger, "10k/200k");
        assert!(!v.over_ceiling);
        assert!(v.auto_evict_cold);
        assert_eq!(v.churn.chars().count(), 2);
    }
}
