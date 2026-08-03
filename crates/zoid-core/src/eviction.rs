//! Pure eviction controller (spec §3.1). This file grows in Slice 1 (planner,
//! scorer, breadcrumb); Slice 0 lands only the policy the turn config carries.

use crate::band::{derive_band, Band};
use serde::{Deserialize, Serialize};

/// The live turn's eviction parameters. `enabled: false` is a total bypass
/// (byte-identical to pre-ACM behavior) used by the zero-arg test constructors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub min_protected_turns: usize,
    pub protection_pct: u8,
    pub max_output: Option<u64>,
    pub rescue_weight: Option<f32>,
}

impl EvictionPolicy {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            capacity: 0,
            context_target: 0,
            band_headroom_pct: 0,
            min_protected_turns: 0,
            protection_pct: 0,
            max_output: None,
            rescue_weight: None,
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
            min_protected_turns: 4,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
        assert_eq!(p.band().high_water, 384_000);
    }

    #[test]
    fn resolve_rescue_weight_none_uses_default() {
        assert_eq!(resolve_rescue_weight(None), DEFAULT_RESCUE_WEIGHT);
    }

    #[test]
    fn resolve_rescue_weight_some_finite_passes_through_capped() {
        assert_eq!(resolve_rescue_weight(Some(8.0)), 8.0);
        assert_eq!(resolve_rescue_weight(Some(0.0)), 0.0);
        assert_eq!(resolve_rescue_weight(Some(RESCUE_WEIGHT_MAX)), RESCUE_WEIGHT_MAX);
    }

    #[test]
    fn resolve_rescue_weight_large_finite_clamped_to_max() {
        assert_eq!(resolve_rescue_weight(Some(100.0)), RESCUE_WEIGHT_MAX);
    }

    #[test]
    fn resolve_rescue_weight_negative_clamped_to_zero() {
        assert_eq!(resolve_rescue_weight(Some(-5.0)), 0.0);
    }

    #[test]
    fn resolve_rescue_weight_non_finite_clamped_to_zero() {
        assert_eq!(resolve_rescue_weight(Some(f32::INFINITY)), 0.0);
        assert_eq!(resolve_rescue_weight(Some(f32::NEG_INFINITY)), 0.0);
        assert_eq!(resolve_rescue_weight(Some(f32::NAN)), 0.0);
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
                    rescue: None,
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
                rescue: None,
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

/// Upper cap for `resolve_rescue_weight`: 4× the default. Anything above this
/// makes rescue so over-protective that it's effectively a misconfiguration;
/// clamping here prevents the band-starve pathology while still allowing ample
/// tuning range.
pub const RESCUE_WEIGHT_MAX: f32 = DEFAULT_RESCUE_WEIGHT * 4.0; // 48.0

/// Resolve the rescue weight, clamping to a safe positive range.
/// Negative / NaN / +∞ / -∞ all collapse to 0.0 (pure recency), preserving
/// the rescue-only invariant and the band-preservation guarantee. Large finite
/// values are capped at `RESCUE_WEIGHT_MAX`. `None` ⇒ `DEFAULT_RESCUE_WEIGHT`.
pub fn resolve_rescue_weight(raw: Option<f32>) -> f32 {
    let w = raw.unwrap_or(DEFAULT_RESCUE_WEIGHT);
    if w.is_finite() && w >= 0.0 {
        w.min(RESCUE_WEIGHT_MAX)
    } else {
        0.0
    }
}

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
    /// The goal text that drove the rescue decision (for `RescueRationale`).
    /// Empty when rescue is inactive. `Default::default()` ⇒ `String::new()`.
    pub goal_text: String,
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

/// Tolerance for treating two cosine values as the same distinct rank tier.
/// `f32::EPSILON` (~1.2e-7) is too tight for real bge cosines — two off-goal
/// turns with cosines 0.3700001 and 0.3700003 would escape the dedup and spread
/// across the rank range, handing them a spurious rescue bump. 1e-5 is still
/// tight enough to separate genuinely different cosines while being immune to
/// float noise from dot products over 384 dims.
const RANK_TOL: f32 = 1e-5;

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
    distinct.dedup_by(|a, b| (*a - *b).abs() < RANK_TOL);
    let d = distinct.len();
    if d <= 1 {
        return vec![0.0; n]; // all-equal ⇒ no rescue
    }
    raws.iter()
        .map(|r| {
            let rank = distinct
                .iter()
                .position(|v| (v - r).abs() < RANK_TOL)
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
        let evs = [
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
        let evs = [user(1, "y"), user(2, "3")];
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
            goal_text: String::new(),
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

/// Per-turn rescue rationale for candidates that were *kept* (not evicted).
/// Present only when rescue was active (non-empty goal). Survivors with
/// `rescue_bump == 0.0` are excluded — they were kept by recency, not rescue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RescueRationale {
    /// The goal text that drove the rescue decision.
    pub goal_text: String,
    /// The rescue weight used in the bump computation.
    pub weight: f32,
    /// Candidates that were kept (not evicted) with `rescue_bump > 0.0`.
    pub survivors: Vec<RescuedTurn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RescuedTurn {
    pub ids: Vec<Ulid>,
    pub topic_hint: String,
    pub base_score: f32,
    pub rescue_bump: f32,
    pub keep_score: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvictionPlan {
    pub turns: Vec<EvictedTurn>,
    pub rescue: Option<RescueRationale>,
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

/// Three-layer turn protection (spec §2.1/§3). Walks backward from the newest
/// turn, protecting until: (a) min_count met AND cumulative scaled tokens
/// reach the budget, OR (b) cumulative tokens approach capacity − SAFETY_MARGIN
/// (shrink min_count toward 1). Always protects at least the newest turn.
///
/// `budget` and `capacity_limit` are in *scaled* units (raw × scale).
/// `token_estimate` per turn is raw chars/3; `scale` converts it to match.
fn compute_protection(
    turns: &[TurnView],
    min_count: usize,
    budget: u64,
    capacity_limit: u64,
    scale: f64,
) -> Vec<bool> {
    let n = turns.len();
    let mut protected = vec![false; n];
    if n == 0 {
        return protected;
    }
    let s = if scale > 0.0 { scale } else { 1.0 };

    // Hard floor: always protect the newest turn.
    protected[n - 1] = true;

    let mut count = 1; // newest already protected
    let mut cumulative = (turns[n - 1].token_estimate as f64 * s) as u64;

    for i in (0..n.saturating_sub(1)).rev() {
        let turn_tokens = (turns[i].token_estimate as f64 * s) as u64;

        // Capacity backstop: stop if adding this turn would overflow capacity.
        // Overflowing capacity is worse than protecting fewer turns. The hard
        // floor of 1 is already protected and never revoked.
        if cumulative.saturating_add(turn_tokens) > capacity_limit {
            break;
        }

        // Minimum count: protect regardless of budget until min_count is met.
        // Budget ceiling: after min_count, protect only while under budget.
        if count < min_count || cumulative.saturating_add(turn_tokens) <= budget {
            protected[i] = true;
            count += 1;
            cumulative = cumulative.saturating_add(turn_tokens);
        } else {
            break;
        }
    }

    protected
}

/// Group the main-branch, non-inert log into positional turns. A turn begins at
/// each `UserMessage` (spec §3.1 / M6: grouping is over the non-inert projection,
/// so an interleaved inert event can't fragment a tool_use/tool_result pair).
fn group_turns(
    events: &[&Event],
    evicted: &HashSet<Ulid>,
    min_protected_turns: usize,
    budget: u64,
    capacity_limit: u64,
    scale: f64,
) -> Vec<TurnView> {
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
    // than `min_protected_turns` turns have started since, then becomes evictable
    // again.
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
    // Three-layer protection (spec §3): hard floor, min count, budget ceiling,
    // capacity backstop. Computed in a backward pass over scaled token estimates.
    let protection = compute_protection(
        &turns,
        min_protected_turns,
        budget,
        capacity_limit,
        scale,
    );
    for (i, t) in turns.iter_mut().enumerate() {
        let is_protected = protection[i];
        let is_evicted = t.ids.iter().any(|id| evicted.contains(id));
        // Within the re-admit cooldown: protected only for `min_protected_turns`
        // turns after the re-admission, so recall→evict→recall can't oscillate
        // but recalled content can never form a permanent unevictable floor
        // (final-review M10).
        let in_readmit_cooldown = t
            .ids
            .iter()
            .any(|id| readmit_mark.get(id).is_some_and(|mark| n - mark < min_protected_turns));
        t.protected = is_protected || is_evicted || in_readmit_cooldown;
    }
    turns
}

/// Plan an eviction wave (spec §3.1). Empty unless `current_tokens >= high_water`.
/// Ranks evictable turns by `scorer` (lowest first), evicting until
/// `current_tokens - reclaimed <= low_water`, never touching protected turns.
///
/// **Scale:** `current_tokens` and the band thresholds are in the same token
/// units (real/scaled), but each turn's `token_estimate` is raw chars/3. The
/// `scale` factor (calibration_ratio × OVERCOUNT_BIAS from the preflight gate)
/// converts per-turn raw estimates into the same units as `current_tokens`
/// before accumulating into `reclaimed`. Without this, the planner would evict
/// `scale`× too many turns, dropping real context far below `low_water`.
/// `scale <= 0.0` is treated as 1.0 (raw, no scaling — the safe default).
pub fn plan_evictions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
    ctx: &GoalContext,
    scale: f64,
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
    // Budget for protection extension beyond min_count: protection_pct of
    // low_water. Must be < band_headroom_pct (default 20) so the extension
    // never eats the wave's drop distance (spec §5.1). Clamp at runtime as a
    // defensive guard — a misconfigured protection_pct ≥ band_headroom_pct
    // would make the protected floor equal low_water and stall every wave.
    let pct = (policy.protection_pct as u64).min(policy.band_headroom_pct as u64);
    let budget = band.low_water.saturating_mul(pct) / 100;
    // capacity_limit = capacity − CAPACITY_SAFETY_MARGIN. The safety margin
    // (8192) also covers the typical ~7k system-prompt + tool-spec overhead;
    // the caller does not add system overhead separately (spec §3.2).
    let capacity_limit = policy.capacity.saturating_sub(
        crate::band::CAPACITY_SAFETY_MARGIN,
    );
    let turns = group_turns(
        &events,
        &evicted,
        policy.min_protected_turns,
        budget,
        capacity_limit,
        scale,
    );

    let candidates: Vec<&TurnView> = turns
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
    let s = if scale > 0.0 { scale } else { 1.0 };
    for &i in &idx {
        if current_tokens.saturating_sub(reclaimed) <= band.low_water {
            break;
        }
        let t = candidates[i];
        let scaled_estimate = (t.token_estimate as f64 * s) as u64;
        reclaimed += scaled_estimate;
        plan.turns.push(EvictedTurn {
            ids: t.ids.clone(),
            token_estimate: scaled_estimate,
            topic_hint: t.topic_hint.clone(),
        });
    }

    // Rescue rationale: survivors are candidates NOT evicted AND with bump > 0.0.
    // Only populated when rescue was active (non-empty goal).
    let rescue = if ctx.goal.is_empty() {
        None
    } else {
        let evicted_set: HashSet<Ulid> = plan.turns.iter()
            .flat_map(|t| t.ids.iter().copied())
            .collect();
        let survivors: Vec<RescuedTurn> = candidates
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                bump[*i] > 0.0
                    && !t.ids.iter().any(|id| evicted_set.contains(id))
            })
            .map(|(i, t)| {
                let base = scorer.score(t, ctx);
                RescuedTurn {
                    ids: t.ids.clone(),
                    topic_hint: t.topic_hint.clone(),
                    base_score: base,
                    rescue_bump: bump[i],
                    keep_score: base + bump[i],
                }
            })
            .collect();
        if survivors.is_empty() {
            None
        } else {
            Some(RescueRationale {
                goal_text: ctx.goal_text.clone(),
                weight: ctx.weight,
                survivors,
            })
        }
    };
    plan.rescue = rescue;
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

    fn policy(target: u64, min_protected_turns: usize) -> EvictionPolicy {
        EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: target,
            band_headroom_pct: 20,
            min_protected_turns,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        }
    }

    #[test]
    fn no_plan_below_high_water() {
        let events = vec![user(1, "a"), asst(2, "b")];
        let plan = plan_evictions(&events, &policy(384_000, 4), 100, &RecencyScorer, &GoalContext::default(), 1.0);
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
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer, &GoalContext::default(), 1.0);
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
                rescue: None,
            },
        ));
        // turn 1 already evicted → not re-selected
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer, &GoalContext::default(), 1.0);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn never_evicts_protected_even_if_over() {
        // all turns are recent (recent_n huge) → empty plan even over high_water
        let big = "x".repeat(3000);
        let events = vec![user(1, &big), asst(2, "ok")];
        let plan = plan_evictions(&events, &policy(100, 10), 100_000, &RecencyScorer, &GoalContext::default(), 1.0);
        assert!(plan.turns.is_empty());
    }

    /// Regression: when `current_tokens` is in SCALED units (raw × scale, as
    /// `preflight_gate` passes: raw × calibration_ratio × OVERCOUNT_BIAS) but
    /// per-turn `token_estimate` is in RAW chars/3 units, the planner must
    /// scale `reclaimed` to match. Without scaling, the planner evicts
    /// `scale`× too many turns, dropping real context far below low_water.
    ///
    /// Setup: 10 turns, each ~1000 raw tokens. recent_n=2 protects the last 2,
    /// leaving 8 evictable candidates (turns 0-7). high_water=3000, low_water=2400.
    /// current_tokens = 5000 (scaled). To reach low_water=2400 from 5000, the
    /// planner needs to reclaim 2600 *scaled* tokens. At scale=1.6, that's
    /// 2600/1.6 = 1625 *raw* tokens ≈ 2 turns. Without the scale, it would
    /// evict 2600 raw tokens ≈ 3 turns — overshooting.
    #[test]
    fn scaled_current_tokens_does_not_over_evict() {
        let big = "x".repeat(3000); // ~1000 raw tokens per message
        let mut events = Vec::new();
        for i in 0..10u128 {
            events.push(user(i * 2 + 1, &big));
            events.push(asst(i * 2 + 2, "ok"));
        }
        // Each turn ≈ 1000 raw tokens (user msg ~1000 + assistant "ok" ~1).
        // 10 turns = ~10k raw. recent_n=2 → turns 8,9 protected.
        // Band: high_water=3000, low_water=2400 (20% headroom).
        let p = policy(3_000, 2);
        let scale = 1.6_f64;
        // current_tokens in scaled units: pretend real tokens = raw × 1.6.
        let current_scaled = 5_000_u64;
        // Without scale: planner evicts until current_scaled - reclaimed <= 2400.
        // reclaimed (raw, unscaled) needs 2600 → 3 turns evicted (3000 raw).
        // Real context after: 10k - 3000 = 7000 raw. But scaled "after" = 5000-3000=2000.
        // With scale: reclaimed is scaled. 5000 - (3000×1.6) = 5000-4800=200 ≤ 2400.
        //   → only 3 turns needed... wait, let me recalc.
        //   Turn 0: reclaimed += 1000×1.6=1600. 5000-1600=3400 > 2400 → continue.
        //   Turn 1: reclaimed += 1600. 5000-3200=1800 ≤ 2400 → stop. 2 turns.
        // So with scale=1.6: 2 turns evicted. Without scale: 3 turns.
        let plan = plan_evictions(
            &events, &p, current_scaled,
            &RecencyScorer, &GoalContext::default(),
            scale,
        );
        let n_evicted = plan.turns.len();
        // With scale applied, only 2 turns should be evicted (not 3).
        assert_eq!(
            n_evicted, 2,
            "scale=1.6: 5000-(2×1000×1.6)=1800 ≤ 2400 → 2 turns, got {n_evicted}"
        );
        // The oldest turns (0, 1) should be the victims.
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(ids.contains(&Ulid::from(1u128)), "oldest turn evicted");
        assert!(ids.contains(&Ulid::from(3u128)), "second-oldest turn evicted");
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
                rescue: None,
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
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer, &GoalContext::default(), 1.0);
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
                rescue: None,
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
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer, &GoalContext::default(), 1.0);
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
        let turns = group_turns(&events, &HashSet::new(), 0, 0, u64::MAX, 1.0);

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
            1.0,
        );
        let b = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
            1.0,
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
        // (add to empty_goalcontext_is_byte_identical_to_recency, after existing asserts)
        assert!(a.rescue.is_none(), "empty goal ⇒ no rescue rationale");
    }

    #[test]
    fn rescue_is_none_when_goal_empty() {
        let events = turns8();
        let plan = plan_evictions(
            &events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default(),
            1.0,
        );
        assert!(plan.rescue.is_none(), "empty goal ⇒ no rescue rationale");
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
            1.0,
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
            goal_text: String::new(),
        };
        let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx, 1.0);
        assert!(
            !evicted_ids_of(&rescued).contains(&Ulid::from(1u128)),
            "on-goal old turn rescued"
        );
        assert!(
            evicted_ids_of(&rescued).contains(&Ulid::from(3u128)),
            "a newer off-goal turn dropped instead"
        );

        // Rescue rationale is populated.
        let rescue = rescued.rescue.as_ref().expect("rescue should be Some");
        assert_eq!(rescue.goal_text, ctx.goal_text);
        assert_eq!(rescue.weight, ctx.weight);
        // The rescued turn (id 1) should be in survivors with bump > 0.
        let survivor = rescue.survivors.iter().find(|s| s.ids.contains(&Ulid::from(1u128)));
        assert!(survivor.is_some(), "rescued turn id 1 in survivors");
        let survivor = survivor.unwrap();
        assert!(survivor.rescue_bump > 0.0, "rescue bump > 0");
        // Score arithmetic: keep_score == base_score + rescue_bump.
        assert!((survivor.keep_score - (survivor.base_score + survivor.rescue_bump)).abs() < 1e-6,
            "keep_score == base_score + rescue_bump");
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
            1.0,
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
            goal_text: String::new(),
        };
        let rescued = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx, 1.0);
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
            goal_text: String::new(),
        };
        let plan = plan_evictions(&events, &policy(5_000, 2), 8_000, &RecencyScorer, &ctx, 1.0);
        let base = plan_evictions(
            &events,
            &policy(5_000, 2),
            8_000,
            &RecencyScorer,
            &GoalContext::default(),
            1.0,
        );
        assert_eq!(plan, base, "weight 0 ⇒ reach 0 ⇒ pure recency");
        assert!(
            evicted_ids_of(&plan).contains(&Ulid::from(1u128)),
            "no rescue at weight 0"
        );
    }
}

// ── protection_tests: compute_protection (Task 1) ──────────────────────────
// These tests pin the three-layer turn-protection algorithm (hard floor,
// min count, budget ceiling, capacity backstop) in isolation, before it is
// wired into group_turns (Task 2). TurnViews are built directly.
#[cfg(test)]
mod protection_tests {
    use super::*;

    fn tv(tokens: u64) -> TurnView {
        TurnView {
            ids: vec![],
            index: 0,
            token_estimate: tokens,
            topic_hint: String::new(),
            protected: false,
        }
    }

    /// Hard floor: the newest turn (index n-1) is always protected, even when
    /// it alone exceeds capacity.
    #[test]
    fn hard_floor_protects_current_turn() {
        let turns = vec![tv(100_000)];
        let p = compute_protection(&turns, 3, 1_000, 500, 1.0);
        assert!(p[0], "newest (only) turn always protected");
    }

    /// Minimum count: turns larger than the budget are still protected up to
    /// min_count. The budget does not restrict the minimum.
    #[test]
    fn protects_min_count_regardless_of_size() {
        // 5 turns, each 10k tokens. min_count=3, budget=1 (tiny).
        // All 3 newest must be protected despite budget=1.
        let turns: Vec<TurnView> = (0..5).map(|_| tv(10_000)).collect();
        let p = compute_protection(&turns, 3, 1, 1_000_000, 1.0);
        // turns 2,3,4 protected (newest 3). 0,1 not.
        assert!(p[2] && p[3] && p[4], "min_count turns protected");
        assert!(!p[0] && !p[1], "beyond min_count not protected");
    }

    /// Budget ceiling: small turns beyond min_count protected up to budget.
    #[test]
    fn budget_extends_protection_for_small_turns() {
        // 10 turns, each 100 tokens. min_count=3, budget=500.
        // 3 min (cumulative 300) + 2 bonus (cumulative 400, 500 ≤ budget) = 5
        // protected; 6th would make cumulative 600 > 500 → stop.
        let turns: Vec<TurnView> = (0..10).map(|_| tv(100)).collect();
        let p = compute_protection(&turns, 3, 500, 1_000_000, 1.0);
        let count = p.iter().filter(|&&x| x).count();
        assert_eq!(count, 5, "3 min + 2 bonus = 5");
        // turn 4 (6th from end) is the first beyond budget → not protected
        assert!(!p[0] && !p[4], "turn 4 (6th from end) beyond budget not protected");
    }

    /// Capacity backstop: when the protected floor exceeds capacity, shrink
    /// min_count toward 1. Never revoke the hard floor of 1.
    #[test]
    fn capacity_backstop_shrinks_min_count() {
        // 3 turns, each 74k tokens. min_count=3, capacity_limit=120k.
        // turn 2 (newest): 74k < 120k → protected.
        // turn 1: 74k + 74k = 148k > 120k → stop. min_count not met but 1 protected.
        let turns: Vec<TurnView> = (0..3).map(|_| tv(74_000)).collect();
        let p = compute_protection(&turns, 3, 1_000_000, 120_000, 1.0);
        assert!(p[2], "hard floor protected");
        assert!(!p[0] && !p[1], "capacity backstop shrank min_count to 1");
    }

    /// Scale: the backward pass scales token_estimate by the scale factor,
    /// matching the band's units.
    #[test]
    fn protection_uses_scale() {
        // 10 turns, each 100 raw tokens. scale=2.0 → 200 scaled per turn.
        // min_count=3, budget=800 scaled. 3 min (cumulative 600) + 1 bonus
        // (cumulative 800 ≤ budget 800) = 4; 5th would make cumulative 1000
        // > 800 → stop.
        let turns: Vec<TurnView> = (0..10).map(|_| tv(100)).collect();
        let p = compute_protection(&turns, 3, 800, 1_000_000, 2.0);
        let count = p.iter().filter(|&&x| x).count();
        assert_eq!(count, 4, "scale=2.0: 3 min + 1 bonus (cumulative 800 ≤ budget 800) = 4");
    }

    /// Empty turns → empty protection vector (no panic).
    #[test]
    fn empty_turns_no_panic() {
        let turns: Vec<TurnView> = vec![];
        let p = compute_protection(&turns, 3, 1_000, 500, 1.0);
        assert!(p.is_empty());
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
                rescue: None,
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
            min_protected_turns: 4,
            protection_pct: 0,
            max_output: None,
            rescue_weight: None,
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
            let plan = plan_evictions(&events, &policy, live, &RecencyScorer, &GoalContext::default(), 1.0);
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
