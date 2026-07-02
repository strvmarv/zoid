//! The constructed-context assembler (spec §4.4/§8): turn a `ContextWindow`
//! plus a `ContextPolicy` into the set of items that *would* be sent. Pure and
//! standalone — P5 wires it into subagent dispatch and the live request; in P3
//! it only feeds the economy view-model. Pin always overrides eviction.

use crate::context::{ContextItem, ContextWindow, Heat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPolicy {
    pub token_ceiling: Option<u64>,
    pub auto_evict_cold: bool,
    pub compact_threshold: Option<u64>,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            token_ceiling: None,
            auto_evict_cold: true,
            compact_threshold: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextSelection {
    pub included: Vec<ContextItem>,
    pub excluded: Vec<ContextItem>,
    pub tokens: u64,
    pub compacted: bool,
}

pub fn assemble_context(window: &ContextWindow, policy: &ContextPolicy) -> ContextSelection {
    let compacted = policy
        .compact_threshold
        .is_some_and(|t| window.total_tokens > t);
    let drop_cold = policy.auto_evict_cold || compacted;

    let mut included: Vec<ContextItem> = Vec::new();
    let mut excluded: Vec<ContextItem> = Vec::new();

    // Pass 1: pin/evict/auto-cold filtering (order preserved = tokens-desc).
    let mut survivors: Vec<ContextItem> = Vec::new();
    for it in &window.items {
        if it.pinned {
            survivors.push(it.clone());
        } else if it.evicted || (drop_cold && it.heat == Heat::Cold) {
            excluded.push(it.clone());
        } else {
            survivors.push(it.clone());
        }
    }

    // Pass 2: token ceiling (pinned always kept; non-pinned fit cumulatively).
    // Pinned items are always included regardless of the ceiling; only non-pinned
    // tokens accumulate against the budget, so running starts at 0.
    let mut running: u64 = 0;
    for it in survivors {
        if it.pinned {
            included.push(it);
            continue;
        }
        match policy.token_ceiling {
            Some(c) if running + it.tokens > c => excluded.push(it),
            _ => {
                running += it.tokens;
                included.push(it);
            }
        }
    }

    let tokens = included.iter().map(|i| i.tokens).sum();
    ContextSelection {
        included,
        excluded,
        tokens,
        compacted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextItem, ContextWindow, Heat, ItemKind};

    fn item(key: &str, tokens: u64, heat: Heat, pinned: bool, evicted: bool) -> ContextItem {
        ContextItem {
            key: key.into(),
            label: key.into(),
            kind: ItemKind::File,
            tokens,
            heat,
            pinned,
            evicted,
        }
    }
    fn window(items: Vec<ContextItem>) -> ContextWindow {
        let total = items.iter().map(|i| i.tokens).sum();
        ContextWindow {
            items,
            total_tokens: total,
        }
    }

    #[test]
    fn pin_overrides_evict_and_auto_cold() {
        let w = window(vec![
            item("pinned-cold", 100, Heat::Cold, true, true), // pinned wins
            item("cold", 50, Heat::Cold, false, false),       // auto-evicted (default on)
            item("hot", 30, Heat::Hot, false, false),
        ]);
        let s = assemble_context(&w, &ContextPolicy::default());
        let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"pinned-cold"));
        assert!(keys.contains(&"hot"));
        assert!(!keys.contains(&"cold")); // auto_evict_cold default true
        assert_eq!(s.tokens, 130);
    }

    #[test]
    fn manual_evict_excludes_unless_pinned() {
        let w = window(vec![item("e", 10, Heat::Hot, false, true)]);
        let s = assemble_context(
            &w,
            &ContextPolicy {
                auto_evict_cold: false,
                ..Default::default()
            },
        );
        assert!(s.included.is_empty());
        assert_eq!(s.excluded.len(), 1);
    }

    #[test]
    fn ceiling_drops_lowest_priority_keeps_pinned() {
        let w = window(vec![
            item("big-pinned", 1000, Heat::Warm, true, false),
            item("a", 60, Heat::Hot, false, false),
            item("b", 60, Heat::Hot, false, false),
        ]);
        let s = assemble_context(
            &w,
            &ContextPolicy {
                token_ceiling: Some(100),
                auto_evict_cold: false,
                ..Default::default()
            },
        );
        let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"big-pinned")); // pinned kept even over ceiling
        assert!(keys.contains(&"a")); // first non-pinned fits cumulative ≤100
        assert!(!keys.contains(&"b")); // would exceed
    }

    #[test]
    fn compaction_flag_trips_over_threshold() {
        let w = window(vec![
            item("cold", 500, Heat::Cold, false, false),
            item("hot", 10, Heat::Hot, false, false),
        ]);
        let s = assemble_context(
            &w,
            &ContextPolicy {
                compact_threshold: Some(100),
                auto_evict_cold: false,
                ..Default::default()
            },
        );
        assert!(s.compacted);
        assert!(s.included.iter().all(|i| i.key != "cold")); // compaction forced cold-evict
    }
}
