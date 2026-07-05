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

use crate::economy::estimate_tokens;
use crate::event::{Event, EventKind, EvictionMarker, EvictedSpan};
use std::collections::{HashMap, HashSet};
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

/// Slice-4 relevance context (empty now; keeps the scorer signature stable).
#[derive(Debug, Default)]
pub struct GoalContext {}

/// A candidate turn for eviction, derived positionally from the non-inert log.
#[derive(Debug, Clone)]
pub struct TurnView {
    pub ids: Vec<Ulid>,
    pub index: usize,
    pub token_estimate: u64,
    pub topic_hint: String,
    /// System / recent-N / already-evicted / re-admitted-cooldown → never selected.
    pub protected: bool,
}

/// Victim-selection seam (spec §3.7). Higher score = more worth keeping.
pub trait EvictionScorer {
    fn score(&self, turn: &TurnView, ctx: &GoalContext) -> f32;
}

/// Default: recency (newer index kept). Deterministic and safe.
pub struct RecencyScorer;
impl EvictionScorer for RecencyScorer {
    fn score(&self, turn: &TurnView, _ctx: &GoalContext) -> f32 {
        turn.index as f32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictedTurn {
    pub ids: Vec<Ulid>,
    pub token_estimate: u64,
    pub topic_hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictionPlan {
    pub turns: Vec<EvictedTurn>,
}

/// Is this event inert for turn-grouping (never starts/joins a conversational turn)?
fn is_inert(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. }
            | EventKind::Tasks { .. }
            | EventKind::TurnsDropped { .. }
            | EventKind::TurnsEvicted { .. }
            | EventKind::TurnsReadmitted { .. }
    )
}

/// The estimated token cost of one event's payload (chars/3), 0 for inert.
fn event_tokens(kind: &EventKind) -> u64 {
    match kind {
        EventKind::UserMessage { text }
        | EventKind::AssistantMessage { text }
        | EventKind::ModelDelta { text } => estimate_tokens(text),
        EventKind::ToolCall { args, name, .. } => estimate_tokens(args) + estimate_tokens(name),
        EventKind::ToolResult { output, .. } => estimate_tokens(output),
        EventKind::DelegationResult { summary, .. } => estimate_tokens(summary),
        _ => 0,
    }
}

/// Group the main-branch, non-inert log into positional turns. A turn begins at
/// each `UserMessage` (spec §3.1 / M6: grouping is over the non-inert projection,
/// so an interleaved inert event can't fragment a tool_use/tool_result pair).
fn group_turns(events: &[Event], evicted: &HashSet<Ulid>, recent_n: usize) -> Vec<TurnView> {
    let mut turns: Vec<TurnView> = Vec::new();
    // M10 (spec §3.1): a turn re-admitted via recall gets a COOLDOWN — for each
    // re-admitted id, `readmit_mark` records how many turns had started when its
    // `TurnsReadmitted` event fired (the marker is inert, so we capture it before
    // the inert-skip below, on the main branch only). It is protected while fewer
    // than `recent_n` turns have started since, then becomes evictable again.
    let mut readmit_mark: HashMap<Ulid, usize> = HashMap::new();
    for e in events {
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        if let EventKind::TurnsReadmitted { ids } = &e.kind {
            // latest re-admission wins (resets the cooldown clock)
            for id in ids {
                readmit_mark.insert(*id, turns.len());
            }
        }
        if is_inert(&e.kind) {
            continue;
        }
        let starts_turn = matches!(e.kind, EventKind::UserMessage { .. });
        if starts_turn || turns.is_empty() {
            let topic_hint = match &e.kind {
                EventKind::UserMessage { text } => text.lines().next().unwrap_or("").chars().take(60).collect(),
                _ => String::new(),
            };
            turns.push(TurnView { ids: Vec::new(), index: turns.len(), token_estimate: 0, topic_hint, protected: false });
        }
        let t = turns.last_mut().unwrap();
        t.ids.push(e.id);
        t.token_estimate += event_tokens(&e.kind);
    }
    let n = turns.len();
    for (i, t) in turns.iter_mut().enumerate() {
        let is_recent = i + recent_n >= n;
        let is_evicted = t.ids.iter().any(|id| evicted.contains(id));
        // Within the re-admit cooldown: protected only for `recent_n` turns after
        // the re-admission, so recall→evict→recall can't oscillate but recalled
        // content can never form a permanent unevictable floor (final-review M10).
        let in_readmit_cooldown = t
            .ids
            .iter()
            .any(|id| readmit_mark.get(id).is_some_and(|mark| n - mark < recent_n));
        t.protected = is_recent || is_evicted || in_readmit_cooldown;
    }
    turns
}

/// Plan an eviction wave (spec §3.1). Empty unless `current_tokens >= high_water`.
/// Ranks evictable turns by `scorer` (lowest first), evicting until
/// `current_tokens - reclaimed <= low_water`, never touching protected turns.
pub fn plan_evictions(
    events: &[Event],
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
) -> EvictionPlan {
    if !policy.enabled {
        return EvictionPlan::default();
    }
    let band = policy.band();
    if current_tokens < band.high_water {
        return EvictionPlan::default();
    }
    let evicted = evicted_ids(events);
    let turns = group_turns(events, &evicted, policy.recent_n);
    let ctx = GoalContext::default();

    let mut candidates: Vec<&TurnView> = turns.iter().filter(|t| !t.protected && !t.ids.is_empty()).collect();
    candidates.sort_by(|a, b| {
        scorer.score(a, &ctx).partial_cmp(&scorer.score(b, &ctx)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut reclaimed = 0u64;
    let mut plan = EvictionPlan::default();
    for t in candidates {
        if current_tokens.saturating_sub(reclaimed) <= band.low_water {
            break;
        }
        reclaimed += t.token_estimate;
        plan.turns.push(EvictedTurn { ids: t.ids.clone(), token_estimate: t.token_estimate, topic_hint: t.topic_hint.clone() });
    }
    plan
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::event::{Event, EventKind};

    fn user(id: u128, t: &str) -> Event { Event::new(Ulid::from(id), None, id as i64, EventKind::UserMessage { text: t.into() }) }
    fn asst(id: u128, t: &str) -> Event { Event::new(Ulid::from(id), None, id as i64, EventKind::AssistantMessage { text: t.into() }) }

    fn policy(target: u64, recent_n: usize) -> EvictionPolicy {
        EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: target, band_headroom_pct: 20, recent_n, max_output: None }
    }

    #[test]
    fn no_plan_below_high_water() {
        let events = vec![user(1, "a"), asst(2, "b")];
        let plan = plan_evictions(&events, &policy(384_000, 4), 100, &RecencyScorer);
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn evicts_oldest_first_down_to_low_water() {
        // 6 turns, each ~1000 tokens estimate; recent_n=2 protects the last two.
        let big = "x".repeat(3000); // ~1000 tokens (chars/3)
        let mut events = Vec::new();
        for i in 0..6u128 { events.push(user(i*2+1, &big)); events.push(asst(i*2+2, "ok")); }
        // current well over high_water forces a wave; low_water = target - 20%.
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer);
        assert!(!plan.turns.is_empty());
        // never evicts the protected (newest) turns
        let evicted_ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!evicted_ids.contains(&Ulid::from(11u128))); // 6th user msg (newest)
        // oldest turn is evicted first
        assert!(evicted_ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn idempotent_skips_already_evicted() {
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok"), user(3, &big), asst(4, "ok"), user(5, "recent"), asst(6, "ok")];
        events.push(Event::new(Ulid::from(99u128), None, 99, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 1000, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
        // turn 1 already evicted → not re-selected
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn never_evicts_protected_even_if_over() {
        // all turns are recent (recent_n huge) → empty plan even over high_water
        let big = "x".repeat(3000);
        let events = vec![user(1, &big), asst(2, "ok")];
        let plan = plan_evictions(&events, &policy(100, 10), 100_000, &RecencyScorer);
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn readmitted_turn_is_protected_from_re_eviction() {
        // M10: an old, low-recency turn that was re-admitted via recall must not be
        // the immediate next eviction victim.
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok"), user(3, &big), asst(4, "ok"), user(5, "recent"), asst(6, "ok")];
        // turn 1 was evicted then recalled back.
        events.push(Event::new(Ulid::from(90u128), None, 90, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 1000, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
        events.push(Event::new(Ulid::from(91u128), None, 91, EventKind::TurnsReadmitted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)] }));
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)), "recalled turn must not immediately re-evict");
    }

    #[test]
    fn readmitted_turn_evictable_after_cooldown_lapses() {
        // M10 cooldown (final-review): re-admit protection is a COOLDOWN, not
        // permanent. Turn 0 is evicted+recalled while only 1 turn exists (mark=1),
        // then `recent_n`+ more turns start — its cooldown window lapses, so it is
        // eligible for eviction again and can never form an unevictable floor.
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok")];
        events.push(Event::new(Ulid::from(90u128), None, 90, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128)], reclaimed_tokens: 1000, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
        events.push(Event::new(Ulid::from(91u128), None, 91, EventKind::TurnsReadmitted { ids: vec![Ulid::from(1u128)] }));
        // recent_n = 2 → four more turns start, well past the cooldown window.
        for i in 1..5u128 { events.push(user(i*2+1, &big)); events.push(asst(i*2+2, "ok")); }
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(ids.contains(&Ulid::from(1u128)), "recalled turn is evictable again once its cooldown lapses");
    }
}

#[cfg(test)]
mod steady_state_tests {
    use super::*;
    use crate::event::{Event, EventKind};
    use crate::context::{context_window_with, ContextOverhead};

    fn apply(events: &mut Vec<Event>, plan: &EvictionPlan, seq: &mut u128) {
        if plan.turns.is_empty() { return; }
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        *seq += 1;
        events.push(Event::new(Ulid::from(*seq + 1_000_000), None, *seq as i64, EventKind::TurnsEvicted {
            ids, reclaimed_tokens: 0, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
    }

    #[test]
    fn holds_band_over_hundreds_of_turns() {
        let big = "x".repeat(3000); // ~1000 tokens
        let policy = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 20_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
        let band = policy.band();
        let overhead = ContextOverhead::default();
        let mut events: Vec<Event> = Vec::new();
        let mut seq = 0u128;
        for turn in 0..400u128 {
            events.push(Event::new(Ulid::from(turn*2+1), None, (turn*2+1) as i64, EventKind::UserMessage { text: big.clone() }));
            events.push(Event::new(Ulid::from(turn*2+2), None, (turn*2+2) as i64, EventKind::AssistantMessage { text: "ok".into() }));
            let live = context_window_with(&events, overhead.clone()).total_tokens;
            let plan = plan_evictions(&events, &policy, live, &RecencyScorer);
            apply(&mut events, &plan, &mut seq);
            let after = context_window_with(&events, overhead.clone()).total_tokens;
            // HARD: never exceeds capacity.
            assert!(after <= policy.capacity, "turn {turn}: {after} > capacity");
            // SOFT: with evictable content present, stays at/under high_water after a wave.
            // (Allow one turn of overshoot before the next wave; assert within high_water + one turn.)
            assert!(after <= band.high_water + 1_100, "turn {turn}: {after} over band");
        }
    }
}
