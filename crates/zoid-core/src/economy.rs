//! The context-economy projections (spec §8): token ledger, churn timeline,
//! and the per-item token estimator. All pure functions of the event log.

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

    #[test]
    fn estimate_tokens_is_chars_over_four_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);     // ceil(1/4)
        assert_eq!(estimate_tokens("abcd"), 1);  // 4/4
        assert_eq!(estimate_tokens("abcde"), 2); // ceil(5/4)
        // counts chars, not bytes
        assert_eq!(estimate_tokens("é"), 1);
    }
}
