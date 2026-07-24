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
        Self {
            enabled: false,
            capacity: 0,
            context_target: 0,
            band_headroom_pct: 0,
            recent_n: 0,
            max_output: None,
        }
    }
    /// The band for this policy (spec §3.6a).
    pub fn band(&self) -> Band {
        derive_band(
            self.capacity,
            self.context_target,
            self.max_output,
            self.band_headroom_pct,
        )
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
        let p = EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 384_000,
            band_headroom_pct: 20,
            recent_n: 4,
            max_output: None,
        };
        assert_eq!(p.band().high_water, 384_000);
    }
}

use crate::economy::estimate_tokens;
use crate::event::{Event, EventKind, EvictedSpan};
use std::collections::{HashMap, HashSet};
use ulid::Ulid;

/// The set of currently-evicted event ids: every `TurnsEvicted.ids`, minus any
/// later `TurnsReadmitted.ids`. Projections skip this set (spec §3.3).
pub fn evicted_ids<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashSet<Ulid> {
    let mut set = HashSet::new();
    for e in events {
        match &e.kind {
            EventKind::TurnsEvicted { ids, .. } => set.extend(ids.iter().copied()),
            EventKind::TurnsReadmitted { ids } => {
                for id in ids {
                    set.remove(id);
                }
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
pub fn eviction_breadcrumb<'a>(events: impl IntoIterator<Item = &'a Event>) -> Option<String> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let live = evicted_ids(events.iter().copied());
    if live.is_empty() {
        return None;
    }
    // Fold currently-live spans from TurnsEvicted markers (skip fully-readmitted).
    let mut spans: Vec<&EvictedSpan> = Vec::new();
    let mut turns = 0usize;
    let mut tokens = 0u64;
    for e in visible {
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
    let topics: Vec<&str> = spans
        .iter()
        .take(5)
        .map(|s| s.topic_hint.as_str())
        .collect();
    Some(format!(
        "Earlier context ({turns} spans, ~{}k tokens; topics: {}) has been paged out. \
         Call recall(query) to retrieve any of it.",
        tokens / 1000,
        topics.join(", ")
    ))
}

#[cfg(test)]
mod directive_reasserted_test {
    use super::*;

    #[test]
    fn directive_reasserted_is_inert() {
        let k = EventKind::DirectiveReasserted { at_cumulative: 123 };
        assert!(is_inert(&k), "re-floor marker must not join evictable turn groups");
        assert_eq!(event_tokens(&k), 0, "marker is weightless");
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use crate::event::EvictionMarker;

    fn ev(id: u128, kind: EventKind) -> Event {
        Event::new(Ulid::from(id), None, id as i64, kind)
    }

    #[test]
    fn evicted_minus_readmitted() {
        let marker = EvictionMarker { spans: vec![] };
        let events = vec![
            ev(
                10,
                EventKind::TurnsEvicted {
                    ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
                    reclaimed_tokens: 5,
                    marker: marker.clone(),
                },
            ),
            ev(
                11,
                EventKind::TurnsReadmitted {
                    ids: vec![Ulid::from(2u128)],
                },
            ),
        ];
        let set = evicted_ids(&events);
        assert!(set.contains(&Ulid::from(1u128)));
        assert!(!set.contains(&Ulid::from(2u128))); // re-admitted
    }

    #[test]
    fn breadcrumb_present_when_evicted_absent_when_not() {
        assert!(eviction_breadcrumb(&[]).is_none());
        let events = vec![ev(
            10,
            EventKind::TurnsEvicted {
                ids: vec![Ulid::from(1u128)],
                reclaimed_tokens: 4200,
                marker: EvictionMarker {
                    spans: vec![EvictedSpan {
                        token_estimate: 4200,
                        topic_hint: "read config".into(),
                    }],
                },
            },
        )];
        let bc = eviction_breadcrumb(&events).unwrap();
        assert!(bc.contains("recall"));
        assert!(bc.contains("read config"));
    }
}

/// Rescue weight in "turns of recency" units (provisional; fixed by the replay
/// eval). Maximal relevance is worth ~this many turns of newness.
pub const DEFAULT_RESCUE_WEIGHT: f32 = 12.0;

/// Relevance context for a rescue-aware eviction pass. Empty `goal` ⇒ no rescue
/// ⇒ byte-identical to pure recency (the degradation path).
#[derive(Debug, Default, Clone)]
pub struct GoalContext {
    /// Goal (query) unit vector; empty ⇒ relevance term disabled.
    pub goal: Vec<f32>,
    /// event_id → cached unit vector, for candidate-turn events.
    pub vecs: HashMap<Ulid, Vec<f32>>,
    /// Rescue weight in turn-index units.
    pub weight: f32,
}

/// Cosine == dot product for L2-normalized vectors; 0.0 on length mismatch.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Max cosine(goal, cached vector) over the turn's events; 0.0 if none cached.
fn turn_relevance(turn: &TurnView, ctx: &GoalContext) -> f32 {
    turn.ids
        .iter()
        .filter_map(|id| ctx.vecs.get(id))
        .map(|v| cosine(&ctx.goal, v))
        .fold(0.0f32, f32::max)
}

/// Map raws to [0,1] by DISTINCT-VALUE rank: ties share a rank, and the lowest
/// distinct value pins to 0.0. All-equal (incl. all-zero) or len ≤ 1 ⇒ all 0.0.
/// CRITICAL: this must be value-based, not array-position-based. In production the
/// candidate set is mostly `raw == 0.0` (off-goal / no cached vector); those MUST
/// all map to 0.0 (zero bump). A position-based rank would spread equal zeros
/// across [0,1] and hand off-goal turns a spurious rescue — silently corrupting
/// the rescue-only guarantee.
fn rank_normalize(raws: &[f32]) -> Vec<f32> {
    let n = raws.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut distinct: Vec<f32> = raws.to_vec();
    distinct.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    distinct.dedup_by(|a, b| (*a - *b).abs() < f32::EPSILON);
    let d = distinct.len();
    if d <= 1 {
        return vec![0.0; n]; // all-equal ⇒ no rescue
    }
    raws.iter()
        .map(|r| {
            let rank = distinct
                .iter()
                .position(|v| (v - r).abs() < f32::EPSILON)
                .unwrap_or(0);
            rank as f32 / (d as f32 - 1.0)
        })
        .collect()
}

/// Newest-first concatenation of up to `n` non-trivial user messages, the
/// relevance query. "Non-trivial" filters empties and short confirmations
/// ("yes", "3", "ok") so terse turns don't poison the goal.
pub const GOAL_WINDOW_MSGS: usize = 3;
pub const MIN_GOAL_MSG_CHARS: usize = 8;

pub fn goal_text(events: &[&Event], n: usize) -> String {
    let mut picked: Vec<&str> = Vec::with_capacity(n);
    for e in events.iter().rev() {
        if let EventKind::UserMessage { text } = &e.kind {
            let t = text.trim();
            if t.chars().count() >= MIN_GOAL_MSG_CHARS {
                picked.push(t);
                if picked.len() == n {
                    break;
                }
            }
        }
    }
    picked.join("\n")
}

#[cfg(test)]
mod goal_text_tests {
    use super::*;

    fn user(id: u128, t: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::UserMessage { text: t.into() },
        )
    }
    fn asst(id: u128, t: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::AssistantMessage { text: t.into() },
        )
    }

    #[test]
    fn goal_text_takes_recent_nontrivial_user_msgs_newest_first() {
        let evs = vec![
            user(1, "implement the relevance rescue scorer"),
            asst(2, "ok"),
            user(3, "yes"), // trivial: dropped
            user(4, "wire it into preflight_gate under pressure"),
        ];
        let refs: Vec<&Event> = evs.iter().collect();
        let g = goal_text(&refs, GOAL_WINDOW_MSGS);
        let pos_wire = g.find("wire it into").unwrap();
        let pos_impl = g.find("implement the relevance").unwrap();
        assert!(pos_wire < pos_impl, "newest-first");
        assert!(!g.contains("yes"), "trivial confirmation filtered");
        assert!(!g.contains("ok"), "assistant text excluded");
    }

    #[test]
    fn goal_text_empty_when_no_nontrivial_user_msgs() {
        let evs = vec![user(1, "y"), user(2, "3")];
        let refs: Vec<&Event> = evs.iter().collect();
        assert!(goal_text(&refs, GOAL_WINDOW_MSGS).is_empty());
    }
}

#[cfg(test)]
mod relevance_tests {
    use super::*;

    #[test]
    fn cosine_is_dot_for_unit_vectors_and_guards_mismatch() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0, "length mismatch → 0");
    }

    #[test]
    fn turn_relevance_is_max_over_cached_event_vecs() {
        let mut vecs = std::collections::HashMap::new();
        vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]); // cos 1.0 vs goal
        vecs.insert(Ulid::from(2u128), vec![0.0, 1.0]); // cos 0.0 vs goal
        let ctx = GoalContext {
            goal: vec![1.0, 0.0],
            vecs,
            weight: DEFAULT_RESCUE_WEIGHT,
        };
        let turn = TurnView {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
            index: 0,
            token_estimate: 0,
            topic_hint: String::new(),
            protected: false,
        };
        assert!((turn_relevance(&turn, &ctx) - 1.0).abs() < 1e-6, "max, not mean");

        let none = TurnView {
            ids: vec![Ulid::from(9u128)],
            ..turn.clone()
        };
        assert_eq!(turn_relevance(&none, &ctx), 0.0, "no cached vec → 0");
    }

    #[test]
    fn rank_normalize_maps_to_unit_interval_and_degenerates_to_zero() {
        let n = rank_normalize(&[0.37, 0.81, 0.55]);
        assert_eq!(n[1], 1.0, "highest raw → 1.0");
        assert_eq!(n[0], 0.0, "lowest raw → 0.0");
        assert!((n[2] - 0.5).abs() < 1e-6, "middle → 0.5");
        // degenerate: all-equal (incl. all-zero) → no spurious rescue
        assert_eq!(rank_normalize(&[0.5, 0.5, 0.5]), vec![0.0, 0.0, 0.0]);
        assert_eq!(rank_normalize(&[0.9]), vec![0.0]);
        assert_eq!(rank_normalize(&[]), Vec::<f32>::new());
        // TIE GUARD (B1): the common case — one on-goal turn, the rest raw==0. All the
        // zeros MUST map to 0.0, not be spread by array position. A position-based
        // rank returns [1.0, 0.0, 0.25, 0.5] here and hands off-goal turns a bump.
        assert_eq!(rank_normalize(&[0.9, 0.0, 0.0, 0.0]), vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(rank_normalize(&[0.0, 0.5, 0.0, 0.5]), vec![0.0, 1.0, 0.0, 1.0]);
    }
}
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
            | EventKind::DirectiveReasserted { .. }
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
fn group_turns(events: &[&Event], evicted: &HashSet<Ulid>, recent_n: usize) -> Vec<TurnView> {
    let mut turns: Vec<TurnView> = Vec::new();
    // A compacted ToolResult's ranking weight must match what the request
    // actually carries — the summary — not the raw (possibly since-cleared,
    // #6b) body and not the pre-compaction `original_tokens`.
    let compacted: HashMap<&str, &str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, summary, .. } => {
                Some((id.as_str(), summary.as_str()))
            }
            _ => None,
        })
        .collect();
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
                EventKind::UserMessage { text } => {
                    text.lines().next().unwrap_or("").chars().take(60).collect()
                }
                _ => String::new(),
            };
            turns.push(TurnView {
                ids: Vec::new(),
                index: turns.len(),
                token_estimate: 0,
                topic_hint,
                protected: false,
            });
        }
        let t = turns.last_mut().unwrap();
        t.ids.push(e.id);
        let tokens = match &e.kind {
            EventKind::ToolResult { id, .. } if compacted.contains_key(id.as_str()) => {
                crate::economy::estimate_tokens(compacted[id.as_str()])
            }
            _ => event_tokens(&e.kind),
        };
        t.token_estimate += tokens;
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
pub fn plan_evictions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
    ctx: &GoalContext,
) -> EvictionPlan {
    if !policy.enabled {
        return EvictionPlan::default();
    }
    let band = policy.band();
    if current_tokens < band.high_water {
        return EvictionPlan::default();
    }
    let events: Vec<&Event> = events.into_iter().collect();
    let evicted = evicted_ids(events.iter().copied());
    let turns = group_turns(&events, &evicted, policy.recent_n);

    let mut candidates: Vec<&TurnView> = turns
        .iter()
        .filter(|t| !t.protected && !t.ids.is_empty())
        .collect();

    // Relevance layer: rank-normalized max-cosine, folded soft-additive into the
    // recency sort key. Empty goal ⇒ bump 0 ⇒ identical to pure recency.
    let bump: Vec<f32> = if ctx.goal.is_empty() {
        vec![0.0; candidates.len()]
    } else {
        let raws: Vec<f32> = candidates.iter().map(|t| turn_relevance(t, ctx)).collect();
        let norm = rank_normalize(&raws);
        norm.iter().map(|n| ctx.weight * n).collect()
    };

    let key = |i: usize, t: &TurnView| scorer.score(t, ctx) + bump[i];
    let mut idx: Vec<usize> = (0..candidates.len()).collect();
    idx.sort_by(|&a, &b| {
        key(a, candidates[a])
            .partial_cmp(&key(b, candidates[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut reclaimed = 0u64;
    let mut plan = EvictionPlan::default();
    for &i in &idx {
        if current_tokens.saturating_sub(reclaimed) <= band.low_water {
            break;
        }
        let t = candidates[i];
        reclaimed += t.token_estimate;
        plan.turns.push(EvictedTurn {
            ids: t.ids.clone(),
            token_estimate: t.token_estimate,
            topic_hint: t.topic_hint.clone(),
        });
    }
    plan
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::event::{Event, EventKind};

    fn user(id: u128, t: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::UserMessage { text: t.into() },
        )
    }
    fn asst(id: u128, t: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::AssistantMessage { text: t.into() },
        )
    }

    fn policy(target: u64, recent_n: usize) -> EvictionPolicy {
        EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: target,
            band_headroom_pct: 20,
            recent_n,
            max_output: None,
        }
    }

    #[test]
    fn no_plan_below_high_water() {
        let events = vec![user(1, "a"), asst(2, "b")];
        let plan = plan_evictions(&events, &policy(384_000, 4), 100, &RecencyScorer, &GoalContext::default());
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn evicts_oldest_first_down_to_low_water() {
        // 6 turns, each ~1000 tokens estimate; recent_n=2 protects the last two.
        let big = "x".repeat(3000); // ~1000 tokens (chars/3)
        let mut events = Vec::new();
        for i in 0..6u128 {
            events.push(user(i * 2 + 1, &big));
            events.push(asst(i * 2 + 2, "ok"));
        }
        // current well over high_water forces a wave; low_water = target - 20%.
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer, &GoalContext::default());
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
        let mut events = vec![
            user(1, &big),
            asst(2, "ok"),
            user(3, &big),
            asst(4, "ok"),
            user(5, "recent"),
            asst(6, "ok"),
        ];
        events.push(Event::new(
            Ulid::from(99u128),
            None,
            99,
            EventKind::TurnsEvicted {
                ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
                reclaimed_tokens: 1000,
                marker: crate::event::EvictionMarker { spans: vec![] },
            },
        ));
        // turn 1 already evicted → not re-selected
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer, &GoalContext::default());
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn never_evicts_protected_even_if_over() {
        // all turns are recent (recent_n huge) → empty plan even over high_water
        let big = "x".repeat(3000);
        let events = vec![user(1, &big), asst(2, "ok")];
        let plan = plan_evictions(&events, &policy(100, 10), 100_000, &RecencyScorer, &GoalContext::default());
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn readmitted_turn_is_protected_from_re_eviction() {
        // M10: an old, low-recency turn that was re-admitted via recall must not be
        // the immediate next eviction victim.
        let big = "x".repeat(3000);
        let mut events = vec![
            user(1, &big),
            asst(2, "ok"),
            user(3, &big),
            asst(4, "ok"),
            user(5, "recent"),
            asst(6, "ok"),
        ];
        // turn 1 was evicted then recalled back.
        events.push(Event::new(
            Ulid::from(90u128),
            None,
            90,
            EventKind::TurnsEvicted {
                ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
                reclaimed_tokens: 1000,
                marker: crate::event::EvictionMarker { spans: vec![] },
            },
        ));
        events.push(Event::new(
            Ulid::from(91u128),
            None,
            91,
            EventKind::TurnsReadmitted {
                ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
            },
        ));
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer, &GoalContext::default());
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(
            !ids.contains(&Ulid::from(1u128)),
            "recalled turn must not immediately re-evict"
        );
    }

    #[test]
    fn readmitted_turn_evictable_after_cooldown_lapses() {
        // M10 cooldown (final-review): re-admit protection is a COOLDOWN, not
        // permanent. Turn 0 is evicted+recalled while only 1 turn exists (mark=1),
        // then `recent_n`+ more turns start — its cooldown window lapses, so it is
        // eligible for eviction again and can never form an unevictable floor.
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok")];
        events.push(Event::new(
            Ulid::from(90u128),
            None,
            90,
            EventKind::TurnsEvicted {
                ids: vec![Ulid::from(1u128)],
                reclaimed_tokens: 1000,
                marker: crate::event::EvictionMarker { spans: vec![] },
            },
        ));
        events.push(Event::new(
            Ulid::from(91u128),
            None,
            91,
            EventKind::TurnsReadmitted {
                ids: vec![Ulid::from(1u128)],
            },
        ));
        // recent_n = 2 → four more turns start, well past the cooldown window.
        for i in 1..5u128 {
            events.push(user(i * 2 + 1, &big));
            events.push(asst(i * 2 + 2, "ok"));
        }
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer, &GoalContext::default());
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(
            ids.contains(&Ulid::from(1u128)),
            "recalled turn is evictable again once its cooldown lapses"
        );
    }

    #[test]
    fn compacted_turn_weighs_summary_not_raw_or_zero() {
        use crate::economy::estimate_tokens;
        use std::collections::HashSet;

        // A ToolResult whose body has ALREADY been cleared by #6b (output empty),
        // plus its compaction marker carrying the summary the request actually holds.
        let tr = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResult {
                id: "call-1".into(),
                name: "bash".into(),
                output: String::new(),
                is_error: false,
            },
        );
        let summary = "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".to_string();
        let comp = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResultCompacted {
                id: "call-1".into(),
                summary: summary.clone(),
                original_tokens: 4242,
            },
        );

        let events: Vec<&Event> = vec![&tr, &comp];
        let turns = group_turns(&events, &HashSet::new(), 0);

        assert_eq!(turns.len(), 1);
        // Weighed by the summary the request carries — not 0 (cleared body) and not
        // 4242 (the pre-compaction original_tokens).
        assert_eq!(turns[0].token_estimate, estimate_tokens(&summary));
    }

    // ── Relevance rescue layer (Task 5) ───────────────────────────────────

    fn turns8() -> Vec<Event> {
        let big = "x".repeat(3000); // ~1000 tokens (chars/3)
        let mut events = Vec::new();
        for i in 0..8u128 {
            events.push(user(i * 2 + 1, &big));
            events.push(asst(i * 2 + 2, "ok"));
        }
        events
    }
    // policy(5_000, 2): high_water=5_000, low_water=4_000; current 8_000 ⇒ reclaim 4 turns.
    fn evicted_ids_of(p: &EvictionPlan) -> Vec<Ulid> {
        p.turns.iter().flat_map(|t| t.ids.clone()).collect()
    }

    #[test]
    fn empty_goalcontext_is_byte_identical_to_recency() {
        let events = turns8();
        let a = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
        );
        let b = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
        );
        assert_eq!(a, b, "deterministic");
        assert!(
            evicted_ids_of(&a).contains(&Ulid::from(1u128)),
            "oldest evicted (recency)"
        );
        assert!(
            !evicted_ids_of(&a).contains(&Ulid::from(15u128)),
            "newest protected"
        );
    }

    #[test]
    fn relevant_old_turn_survives_while_newer_offgoal_is_evicted() {
        let events = turns8();
        // default: oldest (user id 1) is evicted
        let base = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
        );
        assert!(evicted_ids_of(&base).contains(&Ulid::from(1u128)));

        // rescue user id 1: goal matches only its vector; all others orthogonal
        let mut vecs = std::collections::HashMap::new();
        vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]);
        for id in [3u128, 5, 7, 9, 11] {
            vecs.insert(Ulid::from(id), vec![0.0, 1.0]);
        }
        let ctx = GoalContext {
            goal: vec![1.0, 0.0],
            vecs,
            weight: DEFAULT_RESCUE_WEIGHT,
        };
        let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
        assert!(
            !evicted_ids_of(&rescued).contains(&Ulid::from(1u128)),
            "on-goal old turn rescued"
        );
        assert!(
            evicted_ids_of(&rescued).contains(&Ulid::from(3u128)),
            "a newer off-goal turn dropped instead"
        );
    }

    #[test]
    fn band_preservation_rescue_never_shrinks_quota() {
        // GENUINELY distinct relevances (distinct unit-vector cosines) so bumps spread
        // 0..weight and the rescue path is actually exercised — NOT all-equal (which
        // the degenerate guard would zero out, making the test vacuous).
        let events = turns8();
        let base = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
        );
        let mut vecs = std::collections::HashMap::new();
        //                      cos vs [1,0]:  1.0   0.8        0.6        0.0       0.6      0.8
        let angled = [
            (1u128, vec![1.0, 0.0]),
            (3, vec![0.8, 0.6]),
            (5, vec![0.6, 0.8]),
            (7, vec![0.0, 1.0]),
            (9, vec![0.6, 0.8]),
            (11, vec![0.8, 0.6]),
        ];
        for (id, v) in angled {
            vecs.insert(Ulid::from(id), v);
        }
        let ctx = GoalContext {
            goal: vec![1.0, 0.0],
            vecs,
            weight: DEFAULT_RESCUE_WEIGHT,
        };
        let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
        // no-starve: rescue reorders WHICH turns go, never how MANY (same reclaim quota).
        assert_eq!(rescued.turns.len(), base.turns.len());
        assert!(!rescued.turns.is_empty(), "wave still fired");
    }

    #[test]
    fn bounded_reach_weight_zero_is_pure_recency() {
        // A maximally-relevant OLD turn is STILL evicted at weight 0 — proving the
        // bump = weight·norm is finite and scales with weight (reach 0 at weight 0).
        let events = turns8();
        let mut vecs = std::collections::HashMap::new();
        vecs.insert(Ulid::from(1u128), vec![1.0, 0.0]); // maximally relevant, but weight 0
        let ctx = GoalContext {
            goal: vec![1.0, 0.0],
            vecs,
            weight: 0.0,
        };
        let plan = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx);
        let base = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
        );
        assert_eq!(plan, base, "weight 0 ⇒ reach 0 ⇒ pure recency");
        assert!(
            evicted_ids_of(&plan).contains(&Ulid::from(1u128)),
            "no rescue at weight 0"
        );
    }
}

#[cfg(test)]
mod steady_state_tests {
    use super::*;
    use crate::context::{context_window_with, ContextOverhead};
    use crate::event::{Event, EventKind};

    fn apply(events: &mut Vec<Event>, plan: &EvictionPlan, seq: &mut u128) {
        if plan.turns.is_empty() {
            return;
        }
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        *seq += 1;
        events.push(Event::new(
            Ulid::from(*seq + 1_000_000),
            None,
            *seq as i64,
            EventKind::TurnsEvicted {
                ids,
                reclaimed_tokens: 0,
                marker: crate::event::EvictionMarker { spans: vec![] },
            },
        ));
    }

    #[test]
    fn holds_band_over_hundreds_of_turns() {
        let big = "x".repeat(3000); // ~1000 tokens
        let policy = EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 20_000,
            band_headroom_pct: 20,
            recent_n: 4,
            max_output: None,
        };
        let band = policy.band();
        let overhead = ContextOverhead::default();
        let mut events: Vec<Event> = Vec::new();
        let mut seq = 0u128;
        for turn in 0..400u128 {
            events.push(Event::new(
                Ulid::from(turn * 2 + 1),
                None,
                (turn * 2 + 1) as i64,
                EventKind::UserMessage { text: big.clone() },
            ));
            events.push(Event::new(
                Ulid::from(turn * 2 + 2),
                None,
                (turn * 2 + 2) as i64,
                EventKind::AssistantMessage { text: "ok".into() },
            ));
            let live = context_window_with(&events, overhead.clone()).total_tokens;
            let plan = plan_evictions(&events, &policy, live, &RecencyScorer, &GoalContext::default());
            apply(&mut events, &plan, &mut seq);
            let after = context_window_with(&events, overhead.clone()).total_tokens;
            // HARD: never exceeds capacity.
            assert!(after <= policy.capacity, "turn {turn}: {after} > capacity");
            // SOFT: with evictable content present, stays at/under high_water after a wave.
            // (Allow one turn of overshoot before the next wave; assert within high_water + one turn.)
            assert!(
                after <= band.high_water + 1_100,
                "turn {turn}: {after} over band"
            );
        }
    }
}
