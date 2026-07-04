//! Pure eviction controller (spec §3.1). This file grows in Slice 1 (planner,
//! scorer, breadcrumb); Slice 0 lands only the policy the turn config carries.

use crate::band::{derive_band, Band};

/// The live turn's eviction parameters. `enabled: false` is a total bypass
/// (byte-identical to pre-ACM behavior) used by the zero-arg test constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub recent_n: usize,
    pub max_output: Option<u64>,
}

impl EvictionPolicy {
    pub fn disabled() -> Self {
        Self { enabled: false, capacity: 0, context_target: 0, band_headroom_pct: 0, recent_n: 0, max_output: None }
    }
    /// The band for this policy (spec §3.6a).
    pub fn band(&self) -> Band {
        derive_band(self.capacity, self.context_target, self.max_output, self.band_headroom_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_policy_has_zero_band() {
        let b = EvictionPolicy::disabled().band();
        assert_eq!(b.high_water, 0);
    }
    #[test]
    fn enabled_policy_band_matches_derivation() {
        let p = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 384_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
        assert_eq!(p.band().high_water, 384_000);
    }
}

use crate::event::{Event, EventKind, EvictionMarker, EvictedSpan};
use std::collections::HashSet;
use ulid::Ulid;

/// The set of currently-evicted event ids: every `TurnsEvicted.ids`, minus any
/// later `TurnsReadmitted.ids`. Projections skip this set (spec §3.3).
pub fn evicted_ids(events: &[Event]) -> HashSet<Ulid> {
    let mut set = HashSet::new();
    for e in events {
        match &e.kind {
            EventKind::TurnsEvicted { ids, .. } => set.extend(ids.iter().copied()),
            EventKind::TurnsReadmitted { ids } => {
                for id in ids { set.remove(id); }
            }
            _ => {}
        }
    }
    set
}

/// The out-of-band breadcrumb (spec §3.3): a single line appended to the system
/// prompt so the model knows history was paged out and how to reach it. NOT a
/// standalone message (that would break Anthropic alternation). None when the
/// currently-evicted set is empty.
pub fn eviction_breadcrumb(events: &[Event]) -> Option<String> {
    let live = evicted_ids(events);
    if live.is_empty() {
        return None;
    }
    // Fold currently-live spans from TurnsEvicted markers (skip fully-readmitted).
    let mut spans: Vec<&EvictedSpan> = Vec::new();
    let mut turns = 0usize;
    let mut tokens = 0u64;
    for e in events {
        if let EventKind::TurnsEvicted { ids, marker, .. } = &e.kind {
            if ids.iter().any(|id| live.contains(id)) {
                for s in &marker.spans {
                    spans.push(s);
                    turns += 1;
                    tokens += s.token_estimate;
                }
            }
        }
    }
    let topics: Vec<&str> = spans.iter().take(5).map(|s| s.topic_hint.as_str()).collect();
    Some(format!(
        "Earlier context ({turns} spans, ~{}k tokens; topics: {}) has been paged out. \
         Call recall(query) to retrieve any of it.",
        tokens / 1000,
        topics.join(", ")
    ))
}

#[cfg(test)]
mod fold_tests {
    use super::*;

    fn ev(id: u128, kind: EventKind) -> Event { Event::new(Ulid::from(id), None, id as i64, kind) }

    #[test]
    fn evicted_minus_readmitted() {
        let marker = EvictionMarker { spans: vec![] };
        let events = vec![
            ev(10, EventKind::TurnsEvicted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 5, marker: marker.clone() }),
            ev(11, EventKind::TurnsReadmitted { ids: vec![Ulid::from(2u128)] }),
        ];
        let set = evicted_ids(&events);
        assert!(set.contains(&Ulid::from(1u128)));
        assert!(!set.contains(&Ulid::from(2u128))); // re-admitted
    }

    #[test]
    fn breadcrumb_present_when_evicted_absent_when_not() {
        assert!(eviction_breadcrumb(&[]).is_none());
        let events = vec![ev(10, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128)], reclaimed_tokens: 4200,
            marker: EvictionMarker { spans: vec![EvictedSpan { id_range_label: "turns 1–2".into(), token_estimate: 4200, topic_hint: "read config".into() }] },
        })];
        let bc = eviction_breadcrumb(&events).unwrap();
        assert!(bc.contains("recall"));
        assert!(bc.contains("read config"));
    }
}
