//! The context-economy projections (spec §8): token ledger, churn timeline,
//! and the per-item token estimator. All pure functions of the event log.

use crate::event::Event;

/// Aggregate token spend over a scope of the log (spec §8). `total` is
/// `input + output`; `cached` is the cache-read subset of input, surfaced
/// separately (it is *not* added into `total` again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub total: u64,
}

/// Fold the log into a `TokenLedger` by summing every event's `tokens`.
pub fn token_ledger(events: &[Event]) -> TokenLedger {
    let mut l = TokenLedger::default();
    for e in events {
        if let Some(t) = e.tokens {
            l.input += t.input;
            l.output += t.output;
            l.cached += t.cached;
        }
    }
    l.total = l.input + l.output;
    l
}

/// Estimate the token cost of a string as `ceil(chars / 4)` — the standard
/// rough heuristic (≈4 chars/token). Aggregate ledger numbers use real
/// provider `Usage`; this is for per-item context sizing where the provider
/// gives no breakdown.
pub fn estimate_tokens(s: &str) -> u64 {
    let chars = s.chars().count() as u64;
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, TokenStat};
    use proptest::prelude::*;
    use ulid::Ulid;

    #[test]
    fn estimate_tokens_is_chars_over_four_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);     // ceil(1/4)
        assert_eq!(estimate_tokens("abcd"), 1);  // 4/4
        assert_eq!(estimate_tokens("abcde"), 2); // ceil(5/4)
        // counts chars, not bytes
        assert_eq!(estimate_tokens("é"), 1);
    }

    fn usage(input: u64, output: u64, cached: u64) -> Event {
        Event {
            id: Ulid::new(),
            parent: None,
            branch: Default::default(),
            ts: 0,
            kind: EventKind::Usage,
            tokens: Some(TokenStat { input, output, cached }),
        }
    }

    #[test]
    fn ledger_sums_usage_and_ignores_untokened_events() {
        let evs = vec![
            Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() }),
            usage(100, 40, 10),
            usage(50, 20, 5),
        ];
        let l = token_ledger(&evs);
        assert_eq!(l.input, 150);
        assert_eq!(l.output, 60);
        assert_eq!(l.cached, 15);
        assert_eq!(l.total, 210); // input + output, cached not double-counted
    }

    #[test]
    fn ledger_of_empty_log_is_zero() {
        assert_eq!(token_ledger(&[]), TokenLedger::default());
    }

    proptest! {
        #[test]
        fn ledger_total_equals_input_plus_output(stats in proptest::collection::vec((0u64..10_000, 0u64..10_000, 0u64..10_000), 0..50)) {
            let evs: Vec<Event> = stats.iter().map(|&(i,o,c)| usage(i,o,c)).collect();
            let l = token_ledger(&evs);
            prop_assert_eq!(l.total, l.input + l.output);
            prop_assert_eq!(l.input, stats.iter().map(|s| s.0).sum::<u64>());
        }
    }
}
