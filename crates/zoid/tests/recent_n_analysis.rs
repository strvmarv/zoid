//! Analysis: how does `recent_n` interact with the band in long sessions?
//!
//! Simulates sessions with varying turn sizes and recent_n values to show
//! the protected floor as a fraction of the band. Run with:
//!   cargo test --test recent_n_analysis -- --nocapture --ignored

use ulid::Ulid;
use zoid_core::band::derive_band;
use zoid_core::context::{context_window_with, ContextOverhead};
use zoid_core::event::{Event, EventKind};
use zoid_core::eviction::{plan_evictions, EvictionPolicy, GoalContext, RecencyScorer};

/// Build N turns where each turn has a user msg + assistant msg + M tool calls
/// with tool results of `tool_result_chars` chars each. This simulates a
/// multi-step coding turn (read files, run commands, etc).
fn build_session(
    n_turns: u128,
    tool_calls_per_turn: usize,
    tool_result_chars: usize,
) -> Vec<Event> {
    let mut events = Vec::new();
    let mut id = 1u128;
    for t in 0..n_turns {
        // User message
        events.push(Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::UserMessage {
                text: format!("implement feature {t} with tests and docs"),
            },
        ));
        id += 1;
        // Tool calls + results — each turn reads DIFFERENT files so they
        // accumulate (File items are keyed by path; latest-wins per path).
        for c in 0..tool_calls_per_turn {
            let call_id = format!("call-{t}-{c}");
            events.push(Event::new(
                Ulid::from(id),
                None,
                id as i64,
                EventKind::ToolCall {
                    id: call_id.clone(),
                    name: "read_file".into(),
                    args: format!(r#"{{"path":"src/module_{t}_{c}.rs"}}"#),
                },
            ));
            id += 1;
            let output = "line of code\n".repeat(tool_result_chars / 14);
            events.push(Event::new(
                Ulid::from(id),
                None,
                id as i64,
                EventKind::ToolResult {
                    id: call_id,
                    name: "read_file".into(),
                    output,
                    is_error: false,
                },
            ));
            id += 1;
        }
        // Assistant message
        events.push(Event::new(
            Ulid::from(id),
            None,
            id as i64,
            EventKind::AssistantMessage {
                text: format!("I've implemented feature {t}. Here's what I did..."),
            },
        ));
        id += 1;
    }
    events
}

fn overhead() -> ContextOverhead {
    ContextOverhead {
        system_tokens: 4_000,
        tools_tokens: 3_000,
    }
}

#[tokio::test]
#[ignore = "analysis: run with --ignored --nocapture"]
async fn recent_n_protected_floor_analysis() {
    let capacity = 1_000_000u64;
    let target = 300_000u64;
    let headroom = 20u8;
    let band = derive_band(capacity, target, None, headroom);

    eprintln!(
        "Band: high_water={} low_water={}",
        band.high_water, band.low_water
    );
    eprintln!();

    // Simulate 3 session profiles: light, medium, heavy turns.
    // tool_result_chars is the raw chars of each tool result (chars/3 = tokens).
    // A real file read of 500 lines ≈ 15k chars ≈ 5k tokens.
    let profiles: &[(&str, usize, usize)] = &[
        // (name, tool_calls_per_turn, tool_result_chars)
        ("light (2 small reads)", 2, 3_000), // ~2k tokens/turn
        ("medium (5 file reads)", 5, 9_000), // ~15k tokens/turn
        ("heavy (8 big reads+subagent)", 8, 30_000), // ~80k tokens/turn
    ];

    for &(name, tcpt, trc) in profiles {
        eprintln!("=== Profile: {name} ===");
        eprintln!("  tool_calls/turn={tcpt}, tool_result_chars={trc}");

        // Build enough turns to cross high_water.
        let n = 500u128;
        let events = build_session(n, tcpt, trc);
        let raw_total = context_window_with(events.iter(), overhead()).total_tokens;

        // Measure per-turn token size (average).
        // Each turn = 1 user + (1 tool_call + 1 tool_result) * tcpt + 1 assistant.
        let per_turn = raw_total / n as u64;
        eprintln!("  raw window total = {raw_total}, per-turn ≈ {per_turn} raw tokens");

        for &recent_n in &[2usize, 4, 6, 8, 12] {
            let protected_tokens = per_turn * recent_n as u64;
            let pct_of_low = protected_tokens as f64 / band.low_water as f64 * 100.0;
            let pct_of_high = protected_tokens as f64 / band.high_water as f64 * 100.0;
            let evictable_headroom = band.high_water as i64 - protected_tokens as i64;
            eprintln!(
                "  recent_n={recent_n:>2}: protected floor ≈ {protected_tokens:>6} tokens \
                 ({pct_of_low:>5.1}% of low_water, {pct_of_high:>5.1}% of high_water) \
                 evictable headroom = {evictable_headroom:>7}",
            );
        }

        // Now actually run an eviction wave and see where it lands for each recent_n.
        eprintln!("\n  Eviction wave results (scale=1.0, est=raw_total):");
        for &recent_n in &[2usize, 4, 6, 8, 12] {
            let policy = EvictionPolicy {
                enabled: true,
                capacity,
                context_target: target,
                band_headroom_pct: headroom,
                min_protected_turns: recent_n,
                protection_pct: 15,
                max_output: None,
                rescue_weight: None,
            };
            // Only run if raw_total > high_water (enough turns)
            if raw_total < band.high_water {
                eprintln!(
                    "    recent_n={recent_n:>2}: (raw_total < high_water, no eviction needed)"
                );
                continue;
            }
            let plan = plan_evictions(
                events.iter(),
                &policy,
                raw_total,
                &RecencyScorer,
                &GoalContext::default(),
                1.0,
            );
            let n_evicted = plan.turns.len();
            let reclaimed: u64 = plan.turns.iter().map(|t| t.token_estimate).sum();
            let after = raw_total as i64 - reclaimed as i64;
            let turns_remaining = n as usize - n_evicted;
            eprintln!(
                "    recent_n={recent_n:>2}: evicted {n_evicted:>3} turns, reclaimed {reclaimed:>7}, \
                 est_after={after:>7} (vs low={}), {turns_remaining} turns remain",
                band.low_water as i64,
            );
        }
        eprintln!();
    }
}
