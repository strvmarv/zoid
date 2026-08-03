//! Context-windowing smoke test.
//!
//! Seeds a session with enough token-bearing content to cross the ~300k
//! soft target, then drives a single agent turn through `run_agent_turn` so
//! the pre-flight gate (`preflight_gate`) fires. We then inspect the event
//! log to observe:
//!   - whether compaction (`ToolResultCompacted`) fired,
//!   - whether eviction (`TurnsEvicted`) fired,
//!   - the context-window estimate before and after, vs. the band's
//!     `high_water` (= effective target) and `low_water`.
//!
//! This is an *observation* test, not a pass/fail gate: it prints the trace
//! so we can see how low compaction+eviction drives context, and how the
//! estimate behaves. Run with:
//!   cargo test --test context_smoke -- --nocapture --ignored

use std::sync::Arc;
use tokio::sync::mpsc;
use ulid::Ulid;
use zoid::agent::{chat_turn_config, run_agent_turn, AgentUpdate};
use zoid_core::context::{context_window_with, ContextOverhead};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{ProviderEvent, Usage};

fn now() -> i64 {
    0
}

/// A single "fat turn": a user message + assistant message, each ~big tokens.
/// We use plain `AssistantMessage` events (not `ModelDelta`) so the seed is
/// self-contained and needs no provider to materialize the assistant side.
fn fat_turn(i: u128, big: &str) -> [Event; 2] {
    let uid = i * 2 + 1;
    let aid = i * 2 + 2;
    [
        Event::new(
            Ulid::from(uid),
            None,
            uid as i64,
            EventKind::UserMessage {
                text: format!("turn {i}: {big}"),
            },
        ),
        Event::new(
            Ulid::from(aid),
            None,
            aid as i64,
            EventKind::AssistantMessage {
                text: format!("ok {i}: {big}"),
            },
        ),
    ]
}

/// Build a small overhead so the System item is realistic but not dominant.
fn overhead() -> ContextOverhead {
    ContextOverhead {
        system_tokens: 4_000,
        tools_tokens: 3_000,
    }
}

#[tokio::test]
#[ignore = "smoke test: run with --ignored --nocapture"]
async fn smoke_context_compaction_and_eviction_trace() {
    // --- Band parameters (mirror a 1M-capacity model, 300k target, 20% headroom) ---
    let capacity: u64 = 1_000_000;
    let context_target: u64 = 300_000;
    let headroom_pct: u8 = 20;
    let band = zoid_core::band::derive_band(capacity, context_target, None, headroom_pct);
    eprintln!("=== BAND ===");
    eprintln!("capacity            = {capacity}");
    eprintln!("context_target      = {context_target}");
    eprintln!("high_water (evict)  = {}", band.high_water);
    eprintln!("low_water  (stop)  = {}", band.low_water);
    eprintln!("headroom (tokens)   = {}", band.high_water - band.low_water);
    assert_eq!(band.high_water, context_target, "target < usable, so high_water == target");

    // --- Seed enough turns to cross high_water ---
    // Each turn: user ~ big + assistant ~ big. estimate_tokens = chars/3.
    // We want the estimate comfortably over 300k so the gate fires.
    // 120 turns * 2 msgs * (big chars / 3) tokens. Pick big so total > ~320k.
    let per_msg_chars: usize = 4_200; // ~1_400 tokens/msg
    let big = "x".repeat(per_msg_chars);
    let n_turns: u128 = 120;
    let mut seed = Vec::new();
    for i in 0..n_turns {
        seed.extend_from_slice(&fat_turn(i, &big));
    }
    let est_before = context_window_with(seed.iter(), overhead()).total_tokens;
    eprintln!("\n=== SEED ===");
    eprintln!("turns               = {n_turns}");
    eprintln!("est tokens BEFORE   = {est_before}");
    eprintln!("over tokens vs hw   = {}", est_before as i64 - band.high_water as i64);

    let session = SessionHandle::spawn(":memory:").unwrap();
    for e in &seed {
        session.append(e.clone()).await.unwrap();
    }

    // Provider: emit a Usage with a *realistic* input count so the calibration
    // ratio learns something sane, then a short text delta, then Done.
    // real input ≈ est_before (we pretend the provider's tokenizer agrees).
    let real_input = est_before;
    let provider = Arc::new(zoid_provider::FakeProvider::new(vec![
        ProviderEvent::Usage(Usage {
            input_tokens: real_input,
            output_tokens: 10,
            cached: 0,
            thinking_tokens: 0,
        }),
        ProviderEvent::TextDelta("done".into()),
        ProviderEvent::Done,
    ]));

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(256);
    let drain = tokio::spawn(async move {
        let mut saw_started = false;
        let mut saw_complete = false;
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::CompactionStarted => saw_started = true,
                AgentUpdate::CompactionComplete => saw_complete = true,
                _ => {}
            }
        }
        (saw_started, saw_complete)
    });

    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: true,
        capacity,
        context_target,
        band_headroom_pct: headroom_pct,
        recent_n: 4,
        max_output: None,
        rescue_weight: None, // no embedder → recency only
    };
    cfg.context_window = capacity;

    let out = run_agent_turn(
        cfg,
        provider,
        Arc::new(zoid_tools::registry()),
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "m".into(),
        tx,
        Ulid::new(),
        zoid_companion::CompanionHub::new(),
        now,
    )
    .await
    .unwrap();
    let (saw_started, saw_complete) = drain.await.unwrap();

    // --- Observe the aftermath ---
    let est_after = context_window_with(out.iter(), overhead()).total_tokens;

    let n_compacted = out
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ToolResultCompacted { .. }))
        .count();
    let n_evicted_events = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TurnsEvicted { ids, .. } => Some(ids.len()),
            _ => None,
        })
        .sum::<usize>();
    let n_eviction_waves = out
        .iter()
        .filter(|e| matches!(e.kind, EventKind::TurnsEvicted { .. }))
        .count();

    eprintln!("\n=== AFTER TURN ===");
    eprintln!("CompactionStarted   = {saw_started}");
    eprintln!("CompactionComplete   = {saw_complete}");
    eprintln!("ToolResultCompacted  = {n_compacted} events");
    eprintln!("TurnsEvicted waves   = {n_eviction_waves}");
    eprintln!("evicted event ids    = {n_evicted_events}");
    eprintln!("est tokens AFTER    = {est_after}");
    eprintln!("delta (before-after)= {}", est_before as i64 - est_after as i64);
    eprintln!("est after vs low    = {} (low_water={})",
        est_after as i64 - band.low_water as i64,
        band.low_water);
    eprintln!("est after vs high   = {} (high_water={})",
        est_after as i64 - band.high_water as i64,
        band.high_water);

    // The seed had no tool results, so compaction has nothing to compact.
    // The gate should fall through to eviction. Assert the shape:
    assert_eq!(n_compacted, 0, "no tool results in seed → no compactions");
    assert!(
        n_evicted_events > 0,
        "est > high_water and nothing to compact → eviction must fire"
    );
    // Eviction should drive the estimate down toward (ideally to) low_water.
    assert!(
        est_after < est_before,
        "eviction must reduce the estimate"
    );
    // Observation: eviction overshoots low_water substantially. The wave
    // evicts in whole-turn units (each turn ≈ 2*per_msg tokens), and it evicts
    // turns until the *estimate* drops below low_water — but the estimate uses
    // chars/3, while real tokens are higher, so the wave over-evicts relative
    // to the band. We record this as an observation, not a hard assertion.
    let overshoot = band.low_water as i64 - est_after as i64;
    eprintln!("\nOVERSHOOT below low_water = {overshoot} tokens");
    if overshoot > 0 {
        eprintln!("  → eviction landed BELOW low_water by {overshoot} tokens");
        eprintln!("  → likely cause: whole-turn granularity (turn ≈ {} tokens), last wave crossed low_water",
            2 * (per_msg_chars as u64 / 3));
    }

    eprintln!("\n=== CONTEXT WINDOW ITEMS (top 8 by tokens) ===");
    let win = context_window_with(out.iter(), overhead());
    for it in win.items.iter().take(8) {
        eprintln!(
            "  {:<24} {:?} heat={:?} pinned={} evicted={} compacted={} tokens={}",
            it.label, it.kind, it.heat, it.pinned, it.evicted, it.compacted, it.tokens
        );
    }
    eprintln!("  ... ({} items total, {} tokens)", win.items.len(), win.total_tokens);
}

/// Second variant: seed with large tool *results* in the log, then drive a
/// turn. Compaction should fire (it targets the largest uncompacted
/// ToolResult items) before eviction. We observe how far compaction alone
/// drives the estimate, and whether eviction is still needed afterward.
#[tokio::test]
#[ignore = "smoke test: run with --ignored --nocapture"]
async fn smoke_compaction_path_trace() {
    // Use a small target so the seed (≈42k tokens of tool output) crosses it;
    // this mirrors the real behavior at a 300k scale without a 300k seed.
    let capacity: u64 = 1_000_000;
    let context_target: u64 = 30_000;
    let headroom_pct: u8 = 20;
    let band = zoid_core::band::derive_band(capacity, context_target, None, headroom_pct);
    eprintln!("=== BAND === high_water={} low_water={}", band.high_water, band.low_water);

    // Seed: a few user/assistant pairs PLUS several large shell tool results.
    // `shell` has no path key → stays ItemKind::ToolResult (compactable).
    let big_output = "line of filler text padding the token count\n".repeat(400); // ~7k tokens
    let mut seed = Vec::new();
    let mut next_id: u128 = 1;
    for i in 0..6u128 {
        let uid = next_id; next_id += 1;
        let cid = next_id; next_id += 1;
        let rid = next_id; next_id += 1;
        let aid = next_id; next_id += 1;
        seed.push(Event::new(Ulid::from(uid), None, uid as i64, EventKind::UserMessage {
            text: format!("run command {i}"),
        }));
        seed.push(Event::new(Ulid::from(cid), None, cid as i64, EventKind::ToolCall {
            id: format!("c{cid}"),
            name: "shell".into(),
            args: format!(r#"{{"command":"echo big {i}"}}"#),
        }));
        seed.push(Event::new(Ulid::from(rid), None, rid as i64, EventKind::ToolResult {
            id: format!("c{cid}"),
            name: "shell".into(),
            output: big_output.clone(),
            is_error: false,
        }));
        seed.push(Event::new(Ulid::from(aid), None, aid as i64, EventKind::AssistantMessage {
            text: format!("ok {i}"),
        }));
    }

    let est_before = context_window_with(seed.iter(), overhead()).total_tokens;
    eprintln!("\n=== SEED === turns=6, est tokens BEFORE = {est_before}");
    eprintln!("over high_water by {}", est_before as i64 - band.high_water as i64);

    let session = SessionHandle::spawn(":memory:").unwrap();
    for e in &seed {
        session.append(e.clone()).await.unwrap();
    }

    let provider = Arc::new(zoid_provider::FakeProvider::new(vec![
        ProviderEvent::Usage(Usage {
            input_tokens: est_before,
            output_tokens: 10,
            cached: 0,
            thinking_tokens: 0,
        }),
        ProviderEvent::TextDelta("done".into()),
        ProviderEvent::Done,
    ]));

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(256);
    let drain = tokio::spawn(async move {
        let mut s = false; let mut c = false;
        while let Some(u) = rx.recv().await {
            match u { AgentUpdate::CompactionStarted => s = true, AgentUpdate::CompactionComplete => c = true, _ => {} }
        }
        (s, c)
    });

    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: true, capacity, context_target, band_headroom_pct: headroom_pct,
        recent_n: 2, max_output: None, rescue_weight: None,
    };
    cfg.context_window = capacity;

    let out = run_agent_turn(
        cfg, provider, Arc::new(zoid_tools::registry()), Arc::new(zoid_tools::AllowAll),
        session, zoid::eventlog::EventLog::from_vec(seed), "m".into(), tx, Ulid::new(),
        zoid_companion::CompanionHub::new(), now,
    ).await.unwrap();
    let (saw_started, saw_complete) = drain.await.unwrap();

    let est_after_compaction_only = {
        // Measure after compaction events but ignoring evicted ids: recompute
        // the window over `out` which includes both ToolResultCompacted and any
        // TurnsEvicted. To see compaction's effect in isolation we'd need an
        // intermediate snapshot; instead we report the final est_after and the
        // counts, then reason about which lever fired.
        context_window_with(out.iter(), overhead()).total_tokens
    };
    let n_compacted = out.iter().filter(|e| matches!(e.kind, EventKind::ToolResultCompacted { .. })).count();
    let n_evicted = out.iter().filter_map(|e| match &e.kind {
        EventKind::TurnsEvicted { ids, .. } => Some(ids.len()), _ => None,
    }).sum::<usize>();

    eprintln!("\n=== AFTER TURN ===");
    eprintln!("CompactionStarted/Complete = {saw_started}/{saw_complete}");
    eprintln!("ToolResultCompacted events = {n_compacted}");
    eprintln!("TurnsEvicted event ids     = {n_evicted}");
    eprintln!("est tokens AFTER          = {est_after_compaction_only}");
    eprintln!("delta                     = {}", est_before as i64 - est_after_compaction_only as i64);

    // Compaction fires first (largest-first). With 6 × ~7k-token tool results
    // = ~42k tokens of tool output, compaction should replace several with
    // short summaries. Whether eviction *also* fires depends on whether the
    // compacted estimate is still over high_water.
    assert!(n_compacted > 0, "large tool results over high_water → compaction must fire");
    assert!(saw_started && saw_complete, "compaction lifecycle must emit both updates");
    assert!(est_after_compaction_only < est_before, "context must shrink");

    eprintln!("\n=== WINDOW ITEMS (top 10) ===");
    let win = context_window_with(out.iter(), overhead());
    for it in win.items.iter().take(10) {
        eprintln!("  {:<20} {:?} compacted={} tokens={}", it.label, it.kind, it.compacted, it.tokens);
    }
    eprintln!("  ... ({} items, {} tokens)", win.items.len(), win.total_tokens);

    // Did eviction also fire? Report it.
    if n_evicted > 0 {
        eprintln!("\n→ eviction ALSO fired after compaction (est still > high_water after compaction)");
    } else {
        eprintln!("\n→ compaction alone brought est under high_water; eviction did NOT fire");
    }
}

/// Reproduce the over-eviction pathology: a calibration ratio + OVERCOUNT_BIAS
/// inflates `current_tokens` passed to `plan_evictions`, but each turn's
/// `token_estimate` is raw chars/3 — so the planner must evict far more turns
/// (by count) to bring `current - reclaimed <= low_water`.
///
/// This directly calls `plan_evictions` (pure, no agent loop) so we can see
/// the mismatch in isolation.
#[tokio::test]
#[ignore = "smoke test: run with --ignored --nocapture"]
async fn smoke_over_eviction_calibration_mismatch() {
    use zoid_core::eviction::{plan_evictions, EvictionPolicy, GoalContext, RecencyScorer};

    let capacity: u64 = 1_000_000;
    let context_target: u64 = 300_000;
    let headroom_pct: u8 = 20;
    let band = zoid_core::band::derive_band(capacity, context_target, None, headroom_pct);
    eprintln!("=== BAND === high_water={} low_water={}", band.high_water, band.low_water);

    // Simulate a session of ~100 turns, each ~2k tokens (raw chars/3).
    // Raw window total = ~200k. But with a calibration ratio of 1.4 (real
    // tokens exceed the chars/3 estimate for code-heavy content) and
    // OVERCOUNT_BIAS=1.15, the preflight `est` = 200k * 1.4 * 1.15 = 322k.
    // That's over high_water (300k) → eviction fires.
    let n_turns = 100usize;
    let per_turn_raw_tokens: u64 = 2_000;
    let raw_window_total: u64 = n_turns as u64 * per_turn_raw_tokens; // 200k
    let calibration_ratio: f64 = 1.4;
    let overcount_bias: f64 = 1.15;
    let est = (raw_window_total as f64 * calibration_ratio * overcount_bias) as u64; // 322k
    eprintln!("\nraw window total    = {raw_window_total}");
    eprintln!("calibration_ratio  = {calibration_ratio}");
    eprintln!("OVERCOUNT_BIAS      = {overcount_bias}");
    eprintln!("est (scaled)        = {est}  → over high_water by {}", est as i64 - band.high_water as i64);

    // Build synthetic events: 100 user/assistant pairs, each ~2k raw tokens.
    // per_turn_raw_tokens = 2k → each message ~1k tokens → ~3000 chars.
    let chars_per_msg = (per_turn_raw_tokens / 2 * 3) as usize; // 3000 chars
    let big = "x".repeat(chars_per_msg);
    let mut seed = Vec::new();
    for i in 0..n_turns as u128 {
        let uid = i * 2 + 1;
        let aid = i * 2 + 2;
        seed.push(Event::new(Ulid::from(uid), None, uid as i64, EventKind::UserMessage {
            text: format!("{big}"),
        }));
        seed.push(Event::new(Ulid::from(aid), None, aid as i64, EventKind::AssistantMessage {
            text: format!("{big}"),
        }));
    }

    let policy = EvictionPolicy {
        enabled: true,
        capacity,
        context_target,
        band_headroom_pct: headroom_pct,
        recent_n: 4,
        max_output: None,
        rescue_weight: None,
    };
    let scorer = RecencyScorer;
    let ctx = GoalContext::default();

    let plan = plan_evictions(seed.iter(), &policy, est, &scorer, &ctx, calibration_ratio * overcount_bias);

    let n_evicted_turns = plan.turns.len();
    let reclaimed_scaled: u64 = plan.turns.iter().map(|t| t.token_estimate).sum();
    let est_after = est as i64 - reclaimed_scaled as i64;
    let raw_evicted = n_evicted_turns as u64 * per_turn_raw_tokens;
    let raw_after = raw_window_total as i64 - raw_evicted as i64;

    eprintln!("\n=== EVICTION PLAN (with scale={:.2}) ===", calibration_ratio * overcount_bias);
    eprintln!("turns evicted         = {n_evicted_turns} of {n_turns}");
    eprintln!("reclaimed (scaled)    = {reclaimed_scaled}  ← sum of per-turn scaled token_estimate");
    eprintln!("est after (scaled)    = {est_after}  ← current_tokens - reclaimed");
    eprintln!("raw tokens evicted    = {raw_evicted}  ← {n_evicted_turns} turns × {per_turn_raw_tokens} raw");
    eprintln!("raw after (unscaled)  = {raw_after}");
    eprintln!("est after vs low      = {} (low_water={})", est_after - band.low_water as i64, band.low_water);
    eprintln!("raw after vs low      = {} (low_water={})", raw_after - band.low_water as i64, band.low_water);

    eprintln!("\n=== FIX VERIFICATION ===");
    eprintln!("current_tokens (est)  = {est}  (raw × {calibration_ratio} × {overcount_bias})");
    eprintln!("scale factor          = {:.2}", calibration_ratio * overcount_bias);
    eprintln!("reclaimed per turn    = {:.0}  (raw {per_turn_raw_tokens} × scale)", per_turn_raw_tokens as f64 * calibration_ratio * overcount_bias);
    eprintln!("Real context after    = {raw_after} raw tokens ({:.0}% of original)",
        raw_after as f64 / raw_window_total as f64 * 100.0);

    // With the scale applied, the planner evicts fewer turns because each
    // turn's reclaimed contribution is scaled to match current_tokens.
    // Before the fix: 41 turns evicted (over-eviction).
    // After the fix: ~26 turns (scaled math: each turn counts as 2000×1.61=3220).
    // est_after should be at or just below low_water.
    assert!(n_evicted_turns > 0, "est > high_water → eviction must fire");
    assert!(
        est_after <= band.low_water as i64,
        "scaled est after should be at or below low_water ({}) but was {est_after}",
        band.low_water
    );
}