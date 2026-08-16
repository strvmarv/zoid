//! Pre-TUI startup feedback.
//!
//! The interactive launch does real work before the first frame — opening the
//! session store, loading the session, scanning skills/modes, and (on first run)
//! downloading ~130MB of model weights. All of that used to be silent, so a
//! fresh user saw a frozen blank terminal. This reporter prints coarse progress
//! to **stderr** before the alt-screen opens; entering the alt-screen then wipes
//! it, so the feedback is visible only during launch.
//!
//! Output is gated on stderr being a TTY, so piped / non-interactive runs stay
//! clean (no stray lines in logs or captured output).

use std::io::{IsTerminal, Write};

/// Emits `banner` / `step` / `progress` lines to a writer when `enabled`.
pub struct Reporter<W: Write> {
    out: W,
    enabled: bool,
    /// True while an in-place (`\r`) progress line is on screen, so the next
    /// full line knows to break to a fresh row first.
    progress_open: bool,
}

impl Reporter<std::io::Stderr> {
    /// A reporter writing to stderr, active only when stderr is a terminal.
    pub fn stderr() -> Self {
        let out = std::io::stderr();
        let enabled = out.is_terminal();
        Self::new(out, enabled)
    }
}

impl<W: Write> Reporter<W> {
    pub fn new(out: W, enabled: bool) -> Self {
        Self {
            out,
            enabled,
            progress_open: false,
        }
    }

    /// Opening line, e.g. `zoid v0.2.0 — launching…`.
    pub fn banner(&mut self, label: &str) {
        self.write_line(&format!("{label} — launching…"));
    }

    /// A launch step, e.g. `  · opening session store`.
    pub fn step(&mut self, msg: &str) {
        self.write_line(&format!("  · {msg}"));
    }

    fn write_line(&mut self, s: &str) {
        if !self.enabled {
            return;
        }
        if self.progress_open {
            self.newline();
            self.progress_open = false;
        }
        let _ = write!(self.out, "{s}");
        self.newline();
        let _ = self.out.flush();
    }

    /// Emit an explicit CRLF. A bare `\n` only returns the carriage in cooked
    /// mode (via the terminal's `ONLCR` translation); the session picker leaves
    /// the terminal in raw mode, where `\n` alone would staircase every line.
    /// CRLF is correct in raw mode and harmless in cooked mode (`\r` is a no-op
    /// at column 0).
    fn newline(&mut self) {
        let _ = self.out.write_all(b"\r\n");
    }

    /// Update the in-place download-progress line (overwrites via `\r`).
    pub fn progress(&mut self, downloaded: u64, total: Option<u64>) {
        if !self.enabled {
            return;
        }
        let _ = write!(self.out, "\r    {}", format_progress(downloaded, total));
        let _ = self.out.flush();
        self.progress_open = true;
    }

    /// Close the progress line with a newline so later output starts fresh.
    pub fn progress_done(&mut self) {
        if !self.enabled || !self.progress_open {
            return;
        }
        self.newline();
        let _ = self.out.flush();
        self.progress_open = false;
    }
}

/// Human-readable byte progress, e.g. `41.2 / 133.0 MB (31%)`, or `41.2 MB`
/// when the total is unknown.
pub fn format_progress(downloaded: u64, total: Option<u64>) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    match total {
        Some(t) if t > 0 => format!(
            "{:.1} / {:.1} MB ({}%)",
            downloaded as f64 / MB,
            t as f64 / MB,
            downloaded.saturating_mul(100) / t
        ),
        _ => format!("{:.1} MB", downloaded as f64 / MB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_progress_with_total_shows_ratio_and_percent() {
        let s = format_progress(41 * 1024 * 1024, Some(133 * 1024 * 1024));
        assert_eq!(s, "41.0 / 133.0 MB (30%)");
    }

    #[test]
    fn format_progress_without_total_shows_downloaded_only() {
        let s = format_progress(5 * 1024 * 1024, None);
        assert_eq!(s, "5.0 MB");
        // zero-total is treated as unknown (no divide-by-zero)
        assert_eq!(format_progress(1024 * 1024, Some(0)), "1.0 MB");
    }

    #[test]
    fn disabled_reporter_writes_nothing() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = Reporter::new(&mut buf, false);
            r.banner("zoid v9.9.9");
            r.step("opening session store");
            r.progress(1, Some(2));
            r.progress_done();
        }
        assert!(buf.is_empty(), "no output when disabled");
    }

    #[test]
    fn enabled_reporter_emits_banner_and_steps() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = Reporter::new(&mut buf, true);
            r.banner("zoid v9.9.9");
            r.step("opening session store");
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("zoid v9.9.9 — launching…"));
        assert!(out.contains("· opening session store"));
    }

    #[test]
    fn lines_terminate_with_crlf_so_raw_mode_does_not_staircase() {
        // The reporter prints during startup, which straddles the session
        // picker's `enable_raw_mode()` (turns ONLCR off). A bare `\n` there
        // moves down a row but does NOT return the carriage, so each line
        // starts where the previous ended — the staircase. CRLF renders
        // correctly in raw mode and is harmless in cooked mode.
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = Reporter::new(&mut buf, true);
            r.banner("zoid v9.9.9");
            r.step("opening session store");
            r.progress(1, Some(4));
            r.progress_done();
        }
        let out = String::from_utf8(buf).unwrap();
        // Every line feed must be preceded by a carriage return.
        assert!(
            !out.replace("\r\n", "").contains('\n'),
            "bare LF (staircase) present: {out:?}"
        );
        assert!(
            out.contains("launching…\r\n"),
            "banner must end CRLF: {out:?}"
        );
    }

    #[test]
    fn progress_then_line_breaks_to_a_fresh_row() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = Reporter::new(&mut buf, true);
            r.progress(1, Some(4));
            // a following step must not stay glued to the \r progress line
            r.step("done");
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains('\r'));
        assert!(out.contains("· done"));
        // the step is on its own line, after a newline that closed the progress row
        assert!(out.ends_with("· done\r\n"));
    }
}
