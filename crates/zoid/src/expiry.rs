//! Build expiration: a best-effort tripwire so a leaked hidden pre-release
//! build stops launching once it is >30 days past its build date. Not DRM —
//! the check reads the local clock inside the binary (build-expiration spec).

/// 30-day shelf life. Single source of truth for the window.
pub const WINDOW_SECS: u64 = 30 * 24 * 60 * 60;

/// Outcome of an age check. `ClockBeforeBuild` means the wall clock reads
/// earlier than the build stamp — impossible for a legit run, so treated as a
/// wrong/tampered clock.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Expired,
    ClockBeforeBuild,
}

/// Pure age check. All inputs injected — no clock read, no env read.
/// Valid up to and including `build_secs + window`; expired one second later.
pub fn evaluate(now_secs: u64, build_secs: u64, window: u64) -> Verdict {
    if now_secs < build_secs {
        Verdict::ClockBeforeBuild
    } else if now_secs > build_secs.saturating_add(window) {
        Verdict::Expired
    } else {
        Verdict::Ok
    }
}

/// Read the compile-time build stamp and the real clock, evaluate, and on a
/// non-`Ok` verdict print to stderr and exit(1) — call this BEFORE any terminal
/// setup so the message renders cleanly with no panic backtrace.
pub fn enforce() {
    // `env!` makes the stamp a hard compile-time requirement (build.rs emits
    // it). `unwrap_or(0)` only guards a malformed value, which cannot occur
    // from the numeric build.rs stamp.
    let build_secs: u64 = env!("ZOID_BUILD_EPOCH").parse().unwrap_or(0);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match evaluate(now_secs, build_secs, WINDOW_SECS) {
        Verdict::Ok => {}
        Verdict::Expired => {
            eprintln!("This zoid build has expired. Grab a newer build (`zoid update`).");
            std::process::exit(1);
        }
        Verdict::ClockBeforeBuild => {
            eprintln!("zoid can't verify this build's age — check your system clock.");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{evaluate, Verdict, WINDOW_SECS};

    const BUILD: u64 = 1_700_000_000; // arbitrary fixed build epoch

    #[test]
    fn fresh_build_is_ok() {
        assert_eq!(evaluate(BUILD, BUILD, WINDOW_SECS), Verdict::Ok);
    }

    #[test]
    fn last_valid_second_is_ok() {
        assert_eq!(
            evaluate(BUILD + WINDOW_SECS, BUILD, WINDOW_SECS),
            Verdict::Ok
        );
    }

    #[test]
    fn one_second_past_window_is_expired() {
        assert_eq!(
            evaluate(BUILD + WINDOW_SECS + 1, BUILD, WINDOW_SECS),
            Verdict::Expired
        );
    }

    #[test]
    fn clock_before_build_is_flagged() {
        assert_eq!(
            evaluate(BUILD - 1, BUILD, WINDOW_SECS),
            Verdict::ClockBeforeBuild
        );
    }

    #[test]
    fn window_is_exactly_thirty_days() {
        // Pins the constant so a fat-fingered edit to the window can't pass silently.
        assert_eq!(WINDOW_SECS, 2_592_000);
    }
}
