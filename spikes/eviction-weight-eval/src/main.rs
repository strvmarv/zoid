//! Offline replay eval: fix DEFAULT_RESCUE_WEIGHT from real session logs.
//!
//! Usage:
//!   cargo run -p eviction-weight-eval -- /path/to/session.sqlite [embedder]
//!
//! embedder: "fake" (default — structural smoke test) or "candle" (real bge-small
//! embeddings; requires model weights in the zoid cache dir).
//!
//! Opens a real zoid session DB, replays its event log, and at every point the
//! live gate WOULD fire (est >= high_water), re-runs `plan_evictions` for each
//! weight in the sweep grid. Ground truth: a turn that was evicted and later
//! `recall`'d or `TurnsReadmitted` is a labeled "should have kept" example.
//! Metrics per weight: regret_rate, band_health, churn. Recommends the knee
//! (min regret, band health green).

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use ulid::Ulid;
use zoid_core::economy;
use zoid_core::eviction::{
    self, EvictionPolicy, EvictionScorer, GoalContext, RecencyScorer, DEFAULT_RESCUE_WEIGHT,
};
use zoid_core::event::{Event, EventKind};
use zoid_core::retrieval::Embedder;

#[cfg(feature = "candle")]
use zoid_embed::CandleEmbedder;

const WEIGHTS: &[f32] = &[0.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0];

/// Matches the gate's OVERCOUNT_BIAS (agent.rs) — the spike must use the same
/// scaling or fire points will be wrong.
const OVERCOUNT_BIAS: f64 = 1.15;

fn main() -> Result<()> {
    let db_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("usage: eviction-weight-eval <session.sqlite> [fake|candle]"))?;
    let embedder_kind = std::env::args().nth(2).unwrap_or_else(|| "fake".into());
    let conn = Connection::open(&db_path)?;
    let events = load_event_log(&conn)?;
    if events.is_empty() {
        eprintln!("no events in {db_path}");
        return Ok(());
    }
    eprintln!("loaded {} events from {db_path}", events.len());

    // Build the embedder for the goal vector.
    let embedder: Box<dyn Embedder> = match embedder_kind.as_str() {
        "fake" => Box::new(zoid_core::retrieval::FakeEmbedder::new(384)),
        #[cfg(feature = "candle")]
        "candle" => {
            let cache = std::env::var("ZOID_CACHE_DIR")
                .unwrap_or_else(|_| format!("{}/.cache/zoid", std::env::var("HOME").unwrap_or_default()));
            let emb = CandleEmbedder::load(std::path::Path::new(&cache), false)
                .map_err(|e| anyhow!("candle embedder load failed: {e}"))?;
            Box::new(emb)
        }
        #[cfg(not(feature = "candle"))]
        "candle" => return Err(anyhow!("candle embedder not compiled — rebuild with --features candle")),
        other => return Err(anyhow!("unknown embedder '{other}' — use 'fake' or 'candle'")),
    };
    eprintln!("embedder: {embedder_kind} (model={})", embedder.model_id());

    // The policy used by the session (we can't know it exactly, so we use the
    // default shipped policy; the eval is about relative weight comparison).
    let policy = EvictionPolicy {
        enabled: true,
        capacity: 1_000_000,
        context_target: 384_000,
        band_headroom_pct: 20,
        recent_n: 4,
        max_output: None,
    };
    let band = policy.band();

    // Find fire points: indices where est >= high_water.
    let fire_points = find_fire_points(&events, band.high_water);
    if fire_points.is_empty() {
        eprintln!("no eviction fire points found (session never exceeded high_water={})", band.high_water);
        return Ok(());
    }
    eprintln!("{} fire points (est >= high_water={})", fire_points.len(), band.high_water);

    // Ground truth: ids that were later recalled or readmitted after being evicted.
    let later_recalled = collect_later_recalled(&events);

    // Cached vectors: load from the DB for the session's model.
    // In `fake` mode, the embedder's model_id ("fake") won't match the session DB
    // (which stores under e.g. "bge-small-en-v1.5"). Discover the real model_id from
    // the DB instead. In `candle` mode, the embedder's model_id should match.
    let model_id = if embedder_kind == "fake" {
        match discover_model_id(&conn)? {
            Some(id) => id,
            None => {
                eprintln!("warning: no cached vectors in DB — rescue layer will be inert");
                String::new()
            }
        }
    } else {
        embedder.model_id().to_string()
    };
    let vecs_by_id = if model_id.is_empty() {
        std::collections::HashMap::new()
    } else {
        load_vectors_for_model(&conn, &model_id)?
    };
    eprintln!("{} cached vectors for model '{model_id}'", vecs_by_id.len());
    if vecs_by_id.is_empty() {
        eprintln!("warning: zero cached vectors — rescue layer is inert; use 'candle' mode for real data");
    }

    // For each weight, replay each fire point and accumulate metrics.
    let mut results: Vec<WeightMetrics> = Vec::new();
    for &weight in WEIGHTS {
        let m = replay_weight(&events, &policy, &fire_points, &later_recalled, &vecs_by_id, embedder.as_ref(), weight);
        results.push(m);
    }

    // Print the table.
    println!("\n{:>8} {:>12} {:>12} {:>12} {:>14}", "weight", "regret_rate", "band_health", "churn", "reclaim_avg");
    println!("{}", "-".repeat(62));
    for m in &results {
        let health = if m.band_green { "green" } else { "RED" };
        println!(
            "{:>8.1} {:>12.4} {:>12} {:>12.4} {:>14.0}",
            m.weight, m.regret_rate, health, m.churn_rate, m.reclaim_avg
        );
    }

    // Recommend: min regret subject to band green.
    let recommended = results
        .iter()
        .filter(|m| m.band_green)
        .min_by(|a, b| a.regret_rate.partial_cmp(&b.regret_rate).unwrap_or(std::cmp::Ordering::Equal));
    match recommended {
        Some(m) => {
            println!("\nRecommended weight: {:.1} (regret={:.4}, band=green)", m.weight, m.regret_rate);
            let current = DEFAULT_RESCUE_WEIGHT;
            if (m.weight - current).abs() > 0.01 {
                println!("Current DEFAULT_RESCUE_WEIGHT={current:.1} → update to {:.1}", m.weight);
            } else {
                println!("Current DEFAULT_RESCUE_WEIGHT={current:.1} — no change needed");
            }
        }
        None => println!("\nNo weight has green band health — investigate"),
    }
    Ok(())
}

struct WeightMetrics {
    weight: f32,
    regret_rate: f64,
    band_green: bool,
    churn_rate: f64,
    reclaim_avg: f64,
}

fn replay_weight(
    events: &[Event],
    policy: &EvictionPolicy,
    fire_points: &[usize],
    later_recalled: &std::collections::HashSet<Ulid>,
    vecs_by_id: &std::collections::HashMap<Ulid, Vec<f32>>,
    embedder: &dyn Embedder,
    weight: f32,
) -> WeightMetrics {
    let band = policy.band();
    let mut total_evicted = 0u64;
    let mut total_regret = 0u64;
    let mut total_churn = 0u64;
    let mut total_reclaim = 0u64;
    let mut fire_count = 0u64;
    let mut all_green = true;

    // Precompute the weight=0 baseline plans for churn comparison.
    let baseline_plans: Vec<Vec<Ulid>> = fire_points
        .iter()
        .map(|&fp| {
            let slice = &events[..=fp];
            let est = estimate_tokens(slice);
            let plan = eviction::plan_evictions(slice.iter(), policy, est, &RecencyScorer, &GoalContext::default());
            plan.turns.iter().flat_map(|t| t.ids.clone()).collect::<Vec<_>>()
        })
        .collect();

    for (i, &fp) in fire_points.iter().enumerate() {
        let slice = &events[..=fp];
        let est = estimate_tokens(slice);

        // Build GoalContext with a REAL goal vector (the fix for F1).
        let ctx = if weight > 0.0 {
            let refs: Vec<&Event> = slice.iter().collect();
            let text = eviction::goal_text(&refs, eviction::GOAL_WINDOW_MSGS);
            if text.is_empty() {
                GoalContext::default()
            } else {
                let goal = embedder
                    .embed(&[text.as_str()])
                    .ok()
                    .and_then(|mut v| v.pop())
                    .unwrap_or_default();
                if goal.is_empty() {
                    GoalContext::default()
                } else {
                    GoalContext {
                        goal,
                        vecs: vecs_by_id.clone(),
                        weight,
                    }
                }
            }
        } else {
            GoalContext::default()
        };

        let plan = eviction::plan_evictions(slice.iter(), policy, est, &RecencyScorer, &ctx);
        let evicted: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        let reclaimed: u64 = plan.turns.iter().map(|t| t.token_estimate).sum();

        // Regret: evicted ids that were later recalled.
        let regret: u64 = evicted.iter().filter(|id| later_recalled.contains(id)).count() as u64;

        // Band health: did the plan reclaim enough to reach low_water?
        let after = est.saturating_sub(reclaimed);
        if after > band.low_water {
            all_green = false;
        }

        // Churn: symmetric diff vs weight=0 baseline.
        let this_ids: std::collections::HashSet<Ulid> = evicted.iter().copied().collect();
        let base_ids: std::collections::HashSet<Ulid> = baseline_plans[i].iter().copied().collect();
        let churn = this_ids.symmetric_difference(&base_ids).count() as u64;

        total_evicted += evicted.len() as u64;
        total_regret += regret;
        total_churn += churn;
        total_reclaim += reclaimed;
        fire_count += 1;
    }

    let regret_rate = if total_evicted > 0 { total_regret as f64 / total_evicted as f64 } else { 0.0 };
    let churn_rate = if fire_count > 0 { total_churn as f64 / fire_count as f64 } else { 0.0 };
    let reclaim_avg = if fire_count > 0 { total_reclaim as f64 / fire_count as f64 } else { 0.0 };

    WeightMetrics {
        weight,
        regret_rate,
        band_green: all_green,
        churn_rate,
        reclaim_avg,
    }
}

fn find_fire_points(events: &[Event], high_water: u64) -> Vec<usize> {
    let mut points = Vec::new();
    for i in 0..events.len() {
        let est = estimate_tokens(&events[..=i]);
        if est >= high_water {
            points.push(i);
        }
    }
    points
}

fn collect_later_recalled(events: &[Event]) -> std::collections::HashSet<Ulid> {
    let mut evicted = std::collections::HashSet::new();
    let mut recalled = std::collections::HashSet::new();
    for e in events {
        match &e.kind {
            EventKind::TurnsEvicted { ids, .. } => evicted.extend(ids.iter().copied()),
            EventKind::TurnsReadmitted { ids } => {
                for id in ids {
                    if evicted.contains(id) {
                        recalled.insert(*id);
                    }
                }
            }
            _ => {}
        }
    }
    recalled
}

fn load_event_log(conn: &Connection) -> Result<Vec<Event>> {
    // The session DB schema has an `events` table. We read id, timestamp, kind (JSON).
    let mut stmt = conn.prepare("SELECT id, ts, kind FROM events ORDER BY ts ASC")?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let ts: i64 = row.get(1)?;
        let kind_json: String = row.get(2)?;
        Ok((id, ts, kind_json))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (id_str, ts, kind_json) = row?;
        let id = Ulid::from_string(&id_str).unwrap_or_default();
        // Skip unparseable events — they're not useful for the replay.
        if let Ok(kind) = serde_json::from_str::<EventKind>(&kind_json) {
            events.push(Event::new(id, None, ts, kind));
        }
    }
    Ok(events)
}

fn load_vectors_for_model(conn: &Connection, model_id: &str) -> Result<std::collections::HashMap<Ulid, Vec<f32>>> {
    let mut stmt = conn.prepare("SELECT event_id, vector FROM event_embeddings WHERE model_id = ?1")?;
    let rows = stmt.query_map([model_id], |row| {
        let id: String = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        Ok((id, blob))
    })?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (id_str, blob) = row?;
        if let Ok(id) = Ulid::from_string(&id_str) {
            let vec = blob_to_f32s(&blob);
            if !vec.is_empty() {
                out.insert(id, vec);
            }
        }
    }
    Ok(out)
}

/// Discover the model_id used in this session DB by querying for distinct values.
/// Used in `fake` mode where the embedder's model_id ("fake") won't match the DB.
fn discover_model_id(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT model_id FROM event_embeddings LIMIT 1")?;
    let mut rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        Ok(id)
    })?;
    match rows.next() {
        Some(Ok(id)) => Ok(Some(id)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

fn blob_to_f32s(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Estimate tokens matching the real gate's scaling: economy::estimate_tokens
/// (chars/3, ceiling) × OVERCOUNT_BIAS. Does NOT include ContextOverhead (system
/// prompt, tool defs) — the spike can't know those without the session's config,
/// so fire points may be slightly later than the real gate. This is acceptable
/// for relative weight comparison; document the limitation if using for absolute
/// reclaim targeting.
fn estimate_tokens(events: &[Event]) -> u64 {
    let chars: usize = events.iter().map(|e| event_text_len(&e.kind)).sum();
    let raw = economy::estimate_tokens(&" ".repeat(chars)) as u64; // div_ceil(chars/3)
    (raw as f64 * OVERCOUNT_BIAS) as u64
}

fn event_text_len(kind: &EventKind) -> usize {
    match kind {
        EventKind::UserMessage { text } | EventKind::AssistantMessage { text } => text.len(),
        EventKind::ToolResult { output, .. } => output.len(),
        EventKind::ToolResultCompacted { summary, .. } => summary.len(),
        _ => 0,
    }
}

// Trait stub so the compiler knows we use EvictionScorer.
#[allow(dead_code)]
fn _use_scorer(s: &dyn EvictionScorer, t: &eviction::TurnView, c: &GoalContext) -> f32 {
    s.score(t, c)
}