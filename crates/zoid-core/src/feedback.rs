//! User feedback & bug-report submission to GitHub issues on strvmarv/zoid-releases.
//! Pure: no TUI deps, fully unit-testable. The HTTP seam (`FeedbackApi`) and
//! `submit` live in this module (Task 2); this task establishes the data model
//! and the markdown/URL rendering.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

/// What kind of feedback this is. Maps to a GitHub label on zoid-releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackKind {
    Bug,
    FeatureRequest,
    General,
}

impl FeedbackKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::FeatureRequest => "enhancement",
            Self::General => "feedback",
        }
    }
    pub fn display(&self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::FeatureRequest => "Feature Request",
            Self::General => "General",
        }
    }
    pub fn all() -> [FeedbackKind; 3] {
        [Self::Bug, Self::FeatureRequest, Self::General]
    }
    /// Parse the tool-call / JSON string form ("bug"|"feature"|"general").
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bug" => Some(Self::Bug),
            "feature" => Some(Self::FeatureRequest),
            "general" => Some(Self::General),
            _ => None,
        }
    }
}

/// Auto-collected environment context, attached to every report.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostics {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub session_id: String,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub cwd: String,
    pub recent_error: Option<String>,
}

impl Diagnostics {
    /// Snapshot from explicit values — not a global read, so it stays testable.
    pub fn capture(
        version: String,
        os: String,
        arch: String,
        session_id: String,
        mode: String,
        provider: String,
        model: String,
        cwd: String,
        recent_error: Option<String>,
    ) -> Self {
        Self {
            version,
            os,
            arch,
            session_id,
            mode,
            provider,
            model,
            cwd,
            recent_error,
        }
    }
}

/// The public repo feedback targets. Mirrors `update.rs`'s `RELEASES_REPO`.
pub const REPO: &str = "strvmarv/zoid-releases";

/// A complete feedback submission, pre-submission.
#[derive(Debug, Clone)]
pub struct FeedbackReport {
    pub kind: FeedbackKind,
    pub title: String,
    pub body: String,
    pub diagnostics: Diagnostics,
}

/// The outcome of a submit attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Created { url: String, number: u64 },
    BrowserFallback { url: String },
}

const RECENT_ERROR_CAP: usize = 500;

/// Truncate `s` to at most `RECENT_ERROR_CAP` chars, marking the cut with "…".
fn cap(s: &str) -> String {
    if s.chars().count() <= RECENT_ERROR_CAP {
        return s.to_string();
    }
    let truncated: String = s.chars().take(RECENT_ERROR_CAP.saturating_sub(1)).collect();
    format!("{truncated}…")
}

impl FeedbackReport {
    /// Render the title prefixed with kind, e.g. "[Bug] <title>".
    pub fn to_issue_title(&self) -> String {
        format!("[{}] {}", self.kind.display(), self.title)
    }

    /// Render the issue body: user prose, then a `<details>` Environment block.
    pub fn to_issue_body(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.body);
        out.push_str("\n\n<details><summary>Environment</summary>\n\n");
        out.push_str(&format!("- zoid: {}\n", self.diagnostics.version));
        out.push_str(&format!(
            "- OS: {} ({})\n",
            self.diagnostics.os, self.diagnostics.arch
        ));
        out.push_str(&format!("- session: {}\n", self.diagnostics.session_id));
        out.push_str(&format!("- mode: {}\n", self.diagnostics.mode));
        out.push_str(&format!("- provider: {}\n", self.diagnostics.provider));
        out.push_str(&format!("- model: {}\n", self.diagnostics.model));
        out.push_str(&format!("- cwd: {}\n", self.diagnostics.cwd));
        if let Some(err) = &self.diagnostics.recent_error {
            out.push_str(&format!("- recent_error: {}\n", cap(err)));
        }
        out.push_str("\n</details>\n");
        out
    }

    /// Build the pre-filled `github.com/.../issues/new` URL for the browser fallback.
    /// Includes a `> Label:` line in the body since the URL can't pre-set labels.
    pub fn to_browser_url(&self) -> String {
        let issue_title = self.to_issue_title();
        let title = utf8_percent_encode(&issue_title, NON_ALPHANUMERIC);
        let mut body = format!("> Label: {}\n\n", self.kind.label());
        body.push_str(&self.to_issue_body());
        let body = utf8_percent_encode(&body, NON_ALPHANUMERIC);
        format!("https://github.com/{REPO}/issues/new?title={title}&body={body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_all_variants() {
        for k in FeedbackKind::all() {
            let s = match k {
                FeedbackKind::Bug => "bug",
                FeedbackKind::FeatureRequest => "feature",
                FeedbackKind::General => "general",
            };
            assert_eq!(FeedbackKind::parse(s), Some(k));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(FeedbackKind::parse("BUG"), None);
        assert_eq!(FeedbackKind::parse(""), None);
        assert_eq!(FeedbackKind::parse("enhancement"), None);
    }

    #[test]
    fn label_and_display_are_distinct() {
        assert_eq!(FeedbackKind::Bug.label(), "bug");
        assert_eq!(FeedbackKind::Bug.display(), "Bug");
        assert_eq!(FeedbackKind::FeatureRequest.label(), "enhancement");
        assert_eq!(
            FeedbackKind::FeatureRequest.display(),
            "Feature Request"
        );
        assert_eq!(FeedbackKind::General.label(), "feedback");
        assert_eq!(FeedbackKind::General.display(), "General");
    }

    #[test]
    fn capture_stores_all_fields() {
        let d = Diagnostics::capture(
            "0.1.2".into(),
            "linux".into(),
            "x86_64".into(),
            "01J".into(),
            "Chat".into(),
            "ollama".into(),
            "qwen".into(),
            "/home/u/proj".into(),
            Some("boom".into()),
        );
        assert_eq!(d.version, "0.1.2");
        assert_eq!(d.os, "linux");
        assert_eq!(d.recent_error.as_deref(), Some("boom"));
    }

    fn sample_report() -> FeedbackReport {
        FeedbackReport {
            kind: FeedbackKind::Bug,
            title: "Crash on :config".into(),
            body: "steps to reproduce".into(),
            diagnostics: Diagnostics::capture(
                "0.1.2".into(),
                "linux".into(),
                "x86_64".into(),
                "01J".into(),
                "Chat".into(),
                "ollama".into(),
                "qwen".into(),
                "/home/u/proj".into(),
                Some("boom".into()),
            ),
        }
    }

    #[test]
    fn issue_title_is_prefixed_with_kind() {
        assert_eq!(
            sample_report().to_issue_title(),
            "[Bug] Crash on :config"
        );
    }

    #[test]
    fn issue_body_has_prose_then_details_block() {
        let body = sample_report().to_issue_body();
        assert!(body.starts_with("steps to reproduce"));
        assert!(body.contains("<details><summary>Environment</summary>"));
        assert!(body.contains("- zoid: 0.1.2"));
        assert!(body.contains("- OS: linux (x86_64)"));
        assert!(body.contains("- session: 01J"));
        assert!(body.contains("- recent_error: boom"));
        assert!(body.ends_with("</details>\n"));
    }

    #[test]
    fn issue_body_omits_recent_error_when_none() {
        let mut r = sample_report();
        r.diagnostics.recent_error = None;
        assert!(!r.to_issue_body().contains("recent_error"));
    }

    #[test]
    fn issue_body_caps_long_recent_error() {
        let long = "x".repeat(800);
        let mut r = sample_report();
        r.diagnostics.recent_error = Some(long);
        let body = r.to_issue_body();
        let line = body
            .lines()
            .find(|l| l.starts_with("- recent_error:"))
            .unwrap();
        // "x"*499 + "…" = 500 chars after the prefix.
        let value = line
            .trim_start_matches("- recent_error: ")
            .trim_end();
        assert_eq!(value.chars().count(), 500);
        assert!(value.ends_with('…'));
    }

    #[test]
    fn browser_url_encodes_title_and_body_and_includes_label() {
        let url = sample_report().to_browser_url();
        assert!(url.starts_with(
            "https://github.com/strvmarv/zoid-releases/issues/new?title="
        ));
        assert!(url.contains("&body="));
        // The label line rides in the body (percent-encoded).
        assert!(url.contains("%3E%20Label%3A%20bug"));
    }
}