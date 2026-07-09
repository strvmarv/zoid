# Feedback Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users submit feedback or report bugs about zoid as GitHub issues on `strvmarv/zoid-releases`, via three surfaces — a `:feedback` command (user-initiated TUI overlay), a `submit_feedback` interactive agent tool, and a built-in `feedback` skill — all funneling through one shared `zoid-core::feedback` submission core.

**Architecture:** A pure `zoid-core::feedback` module owns the data model (`FeedbackReport`, `Diagnostics`, `FeedbackKind`, `SubmitOutcome`) and the GitHub submission logic behind a `FeedbackApi` trait seam (HTTP in production, fake in tests — mirroring `github_fetch.rs`'s `GithubApi` pattern). The TUI owns a `Feedback` overlay (form + key routing + render), the bin wires command dispatch + diagnostics capture + async submit, the agent loop intercepts the `submit_feedback` tool via the existing `Interactive`/`QuestionAsked` park-and-await path, and a built-in skill ships via `SkillRegistry::builtin()`.

**Tech Stack:** Rust 2021, `ratatui` 0.30, `ratatui-textarea` 0.9, `reqwest` 0.12 (rustls), `serde`/`serde_json`, `anyhow`, `percent-encoding`. Workspace at `crates/` with members `zoid-core`, `zoid-tools`, `zoid-tui`, `zoid` (bin).

## Global Constraints

- **Repo:** GitHub issues target `strvmarv/zoid-releases` (the public mirror; source repo `strvmarv/zoid` is private). Constant: `const REPO: &str = "strvmarv/zoid-releases";`.
- **Auth:** `$GITHUB_TOKEN` if present → POST to `https://api.github.com/repos/{REPO}/issues`. If absent → return a pre-filled `https://github.com/{REPO}/issues/new?title=...&body=...` URL (no HTTP call). Never panic on auth/network failure — return `Err`.
- **User-Agent:** the HTTP client sets `User-Agent: zoid-feedback/{version}` (mirrors `github_fetch.rs`'s `zoid-wizard/{version}`).
- **Diagnostics fields:** version (`env!("CARGO_PKG_VERSION")`), os (`std::env::consts::OS`), arch (`std::env::consts::ARCH`), session_id (ULID), mode, provider, model, cwd, recent_error (most recent `ToolResult { is_error: true, .. }` `output`, capped ~500 chars).
- **Labels:** API path sends `{"labels": [kind.label()]}`. Browser path can't pre-set labels; body includes a `> Label: <kind>` line instead.
- **No new workspace crates.** No new heavy deps. `reqwest` and `percent-encoding` are added to `zoid-core`'s `Cargo.toml` (both already transitive in the tree).
- **Interactive tool:** `submit_feedback` is `ToolKind::Interactive` (parks like `ask_user`); excluded from subagents automatically by the existing `Interactive` filter in `subagent.rs`.
- **Event reuse:** the tool path reuses `QuestionAsked`/`QuestionAnswered` with a new `QuestionKind::Feedback { kind, title, body }` variant — NOT a new `EventKind` variant.
- **Skill shipping:** the `feedback` skill is a third entry in `SkillRegistry::builtin()` (Rust string literal, `base_dir: None`), available to every mode globally. No on-disk `SKILL.md` ships.
- **TDD:** every task writes the failing test first, runs it to confirm failure, implements minimally, runs to confirm pass, then commits.

---

## File Structure

**New files:**
- `crates/zoid-core/src/feedback.rs` — the submission core: data model, rendering, `FeedbackApi` trait + `HttpFeedbackApi`/`FakeFeedbackApi`, `submit()`.
- `crates/zoid-tools/src/feedback.rs` — the `submit_feedback` `Interactive` tool.
- `crates/zoid-tui/src/feedback_view.rs` — `route_feedback_key` (key routing only; the state types live in `state.rs`).

**Modified files:**
- `crates/zoid-core/Cargo.toml` — add `reqwest`, `percent-encoding`, `async-trait` deps.
- `crates/zoid-core/src/lib.rs` — add `pub mod feedback;`.
- `crates/zoid-core/src/event.rs` — add `QuestionKind::Feedback { kind, title, body }`.
- `crates/zoid-core/src/skill.rs` — add the `feedback` built-in skill + `FEEDBACK_SKILL_BODY` const; update affected tests.
- `crates/zoid-tools/src/lib.rs` — add `pub mod feedback;`; register `SubmitFeedback` in `registry()` + `registry_with_kill()`; extend `registry_has_unique_named_tools` + `registry_tools_are_all_local_by_default` + `registry_excludes_chat_only_tools` assertions.
- `crates/zoid-tui/src/command.rs` — add `Command::Feedback`; parse `feedback`; add a test.
- `crates/zoid-tui/src/state.rs` — add `Overlay::Feedback`; add `FeedbackState`/`FeedbackField`/`FeedbackStatus`; add `feedback: Option<FeedbackState>` to `ShellState`; default it in `new()`.
- `crates/zoid-tui/src/route.rs` — route `Overlay::Feedback` to `route_feedback_key`; add `Action::Feedback*` variants; route submit/abort/char keys in `route_key`.
- `crates/zoid-tui/src/layout.rs` — add `Overlay::Feedback` to the overlay-list match (blocks conversation while open).
- `crates/zoid-tui/src/render.rs` — render the `Feedback` overlay modal; add `Command::Feedback` preview arm.
- `crates/zoid-tui/src/palette.rs` — add "Submit feedback" `PaletteItem`.
- `crates/zoid/src/agent.rs` — extend the `Interactive` match arm to intercept `submit_feedback`; parse/validate args; emit `QuestionAsked` with `QuestionKind::Feedback`; park; handle confirm/decline reply; build report + submit; emit `ToolResult`.
- `crates/zoid/src/main.rs` — `Command::Feedback` dispatch in `exec_command`; capture `Diagnostics`; open overlay; wire submit (Ctrl+Enter) to `report.submit().await`; surface outcome.
- `crates/zoid/src/subagent.rs` — extend the existing `assembled_tools_exclude_interactive_ask_user` test to also assert `submit_feedback` is excluded.

**Interfaces (cross-task contracts):**

- **Task 1 produces** (`zoid-core::feedback`): `FeedbackKind` (`parse`/`label`/`display`/`all`), `Diagnostics::capture(...)`, `FeedbackReport { kind, title, body, diagnostics }`, `SubmitOutcome { Created{url,number} | BrowserFallback{url} }`, `FeedbackReport::to_issue_body() -> String`, `to_issue_title() -> String`, `to_browser_url() -> String`.
- **Task 2 produces** (`zoid-core::feedback`): `FeedbackApi` trait (`async fn create_issue(&self, repo: &str, title: &str, body: &str, labels: Vec<String>) -> anyhow::Result<(String, u64)>`), `HttpFeedbackApi`, `FakeFeedbackApi`, `FeedbackReport::submit_via(&self, api: &dyn FeedbackApi) -> anyhow::Result<SubmitOutcome>`.
- **Task 3 produces** (`zoid-core::event`): `QuestionKind::Feedback { kind: String, title: String, body: String }`.
- **Task 4 produces** (`zoid-tools::feedback`): `SubmitFeedback` tool (name `submit_feedback`, `Interactive` kind), registered in both registries.
- **Task 5 produces** (`zoid-tui::command` + `state` + `feedback_view`): `Command::Feedback`, `Overlay::Feedback`, `FeedbackState`/`FeedbackField`/`FeedbackStatus`, `route_feedback_key(state, key) -> Action`, `Action::Feedback*` variants.
- **Task 6 produces** (`zoid-tui::render` + `palette` + `layout`): the rendered modal, the palette row, the conversation-block match arm.
- **Task 7 produces** (`zoid-core::skill`): the `feedback` built-in skill in `builtin()`.
- **Task 8 produces** (`zoid` bin, `agent.rs`): the `submit_feedback` interception in the agent loop.
- **Task 9 produces** (`zoid` bin, `main.rs`): `Command::Feedback` dispatch + diagnostics capture + async submit.

---

### Task 1: `zoid-core::feedback` data model + rendering

**Files:**
- Create: `crates/zoid-core/src/feedback.rs`
- Modify: `crates/zoid-core/src/lib.rs:9` (add `pub mod feedback;` in alphabetical order, after `event`)
- Modify: `crates/zoid-core/Cargo.toml` (add `percent-encoding`)

**Interfaces:**
- Produces: `FeedbackKind` (enum with `Bug`/`FeatureRequest`/`General`, `Serialize`/`Deserialize`), `FeedbackKind::parse(&str) -> Option<Self>`, `label() -> &'static str`, `display() -> &'static str`, `all() -> [FeedbackKind; 3]`; `Diagnostics` struct + `Diagnostics::capture(...)`; `FeedbackReport { kind, title, body, diagnostics }`; `SubmitOutcome { Created{url, number} | BrowserFallback{url} }`; `FeedbackReport::to_issue_body()`, `to_issue_title()`, `to_browser_url()`.

- [ ] **Step 1: Add `percent-encoding` to `zoid-core/Cargo.toml`**

Append to the `[dependencies]` block in `crates/zoid-core/Cargo.toml`:

```toml
percent-encoding = "2"
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/zoid-core/src/lib.rs`, add `pub mod feedback;` after `pub mod event;` (alphabetical):

```rust
pub mod event;
pub mod feedback;
```

- [ ] **Step 3: Write the failing tests for `FeedbackKind`**

Create `crates/zoid-core/src/feedback.rs` with only the tests + a stub that won't compile yet. First, write the `FeedbackKind` tests:

```rust
//! User feedback & bug-report submission to GitHub issues on strvmarv/zoid-releases.
//! Pure: no TUI deps, fully unit-testable. The HTTP seam (`FeedbackApi`) and
//! `submit` live in this module (Task 2); this task establishes the data model
//! and the markdown/URL rendering.

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
        assert_eq!(FeedbackKind::FeatureRequest.display(), "Feature Request");
        assert_eq!(FeedbackKind::General.label(), "feedback");
        assert_eq!(FeedbackKind::General.display(), "General");
    }
}
```

- [ ] **Step 4: Run tests to verify they pass (the impl is already there)**

Run: `cargo test -p zoid-core feedback::tests`
Expected: PASS (3 tests). The impl is in the same paste; tests and impl land together since this is pure data.

- [ ] **Step 5: Add `Diagnostics` + tests**

Append to `crates/zoid-core/src/feedback.rs` (above the `#[cfg(test)]` block):

```rust
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
```

Add to the `tests` module:

```rust
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
```

- [ ] **Step 6: Run the new test**

Run: `cargo test -p zoid-core feedback::tests::capture_stores_all_fields`
Expected: PASS.

- [ ] **Step 7: Add `FeedbackReport` + `SubmitOutcome` + `to_issue_title`/`to_issue_body`/`to_browser_url` + tests**

Append to `crates/zoid-core/src/feedback.rs` (above the `#[cfg(test)]` block):

```rust
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

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
    /// For the browser path (no labels), prepend a `> Label:` line.
    pub fn to_issue_body(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.body);
        out.push_str("\n\n<details><summary>Environment</summary>\n\n");
        out.push_str(&format!("- zoid: {}\n", self.diagnostics.version));
        out.push_str(&format!("- OS: {} ({})\n", self.diagnostics.os, self.diagnostics.arch));
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
        let title = utf8_percent_encode(&self.to_issue_title(), NON_ALPHANUMERIC);
        let mut body = format!("> Label: {}\n\n", self.kind.label());
        body.push_str(&self.to_issue_body());
        let body = utf8_percent_encode(&body, NON_ALPHANUMERIC);
        format!("https://github.com/{REPO}/issues/new?title={title}&body={body}")
    }
}
```

Add to the `tests` module:

```rust
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
        assert_eq!(sample_report().to_issue_title(), "[Bug] Crash on :config");
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
        let line = body.lines().find(|l| l.starts_with("- recent_error:")).unwrap();
        // "x"*499 + "…" = 500 chars after the prefix.
        let value = line.trim_start_matches("- recent_error: ").trim_end();
        assert_eq!(value.chars().count(), 500);
        assert!(value.ends_with('…'));
    }

    #[test]
    fn browser_url_encodes_title_and_body_and_includes_label() {
        let url = sample_report().to_browser_url();
        assert!(url.starts_with("https://github.com/strvmarv/zoid-releases/issues/new?title="));
        assert!(url.contains("&body="));
        // The label line rides in the body (percent-encoded).
        assert!(url.contains("%3E%20Label%3A%20bug"));
    }
```

- [ ] **Step 8: Run all `feedback` tests**

Run: `cargo test -p zoid-core feedback`
Expected: PASS (all tests in the module).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/feedback.rs crates/zoid-core/src/lib.rs crates/zoid-core/Cargo.toml
git commit -m "feat(core): add feedback data model, diagnostics, and issue/url rendering"
```

---

### Task 2: `submit()` via the `FeedbackApi` trait seam

**Files:**
- Modify: `crates/zoid-core/src/feedback.rs` (add `FeedbackApi` trait, `HttpFeedbackApi`, `FakeFeedbackApi`, `submit_via`)
- Modify: `crates/zoid-core/Cargo.toml` (add `reqwest`, `async-trait`)

**Interfaces:**
- Consumes: Task 1's `FeedbackReport`, `SubmitOutcome`, `REPO`, `FeedbackKind::label`.
- Produces: `FeedbackApi` trait (`async fn create_issue(&self, repo, title, body, labels) -> Result<(String, u64)>`), `HttpFeedbackApi::new()`, `FakeFeedbackApi`, `FeedbackReport::submit_via(&self, api: &dyn FeedbackApi) -> Result<SubmitOutcome>`.

- [ ] **Step 1: Add `reqwest` and `async-trait` to `zoid-core/Cargo.toml`**

Append to the `[dependencies]` block in `crates/zoid-core/Cargo.toml`:

```toml
reqwest = { workspace = true }
async-trait = { workspace = true }
```

- [ ] **Step 2: Write the failing test for `submit_via` with a token present**

Add to the `tests` module in `crates/zoid-core/src/feedback.rs`:

```rust
    #[tokio::test]
    async fn submit_via_with_token_creates_issue() {
        let api = FakeFeedbackApi::created("https://github.com/strvmarv/zoid-releases/issues/7", 7);
        let outcome = sample_report().submit_via(&api).await.unwrap();
        match outcome {
            SubmitOutcome::Created { url, number } => {
                assert_eq!(url, "https://github.com/strvmarv/zoid-releases/issues/7");
                assert_eq!(number, 7);
            }
            _ => panic!("expected Created"),
        }
    }

    #[tokio::test]
    async fn submit_via_without_token_returns_browser_fallback() {
        let api = FakeFeedbackApi::no_token();
        let outcome = sample_report().submit_via(&api).await.unwrap();
        match outcome {
            SubmitOutcome::BrowserFallback { url } => {
                assert!(url.starts_with("https://github.com/strvmarv/zoid-releases/issues/new?"));
            }
            _ => panic!("expected BrowserFallback"),
        }
    }

    #[tokio::test]
    async fn submit_via_api_error_propagates() {
        let api = FakeFeedbackApi::err("401 unauthorized");
        let res = sample_report().submit_via(&api).await;
        assert!(res.is_err());
    }
```

- [ ] **Step 3: Run tests to verify they fail to compile**

Run: `cargo test -p zoid-core feedback::tests`
Expected: FAIL — `FakeFeedbackApi` and `submit_via` not defined.

- [ ] **Step 4: Implement the `FeedbackApi` seam + `submit_via`**

Append to `crates/zoid-core/src/feedback.rs` (above `#[cfg(test)]`):

```rust
use async_trait::async_trait;

/// The GitHub issue-creation seam. `HttpFeedbackApi` hits the real API;
/// `FakeFeedbackApi` returns canned outcomes for tests. Mirrors the
/// `GithubApi` trait pattern in `crates/zoid/src/github_fetch.rs`.
#[async_trait]
pub trait FeedbackApi: Send + Sync {
    /// Create an issue on `repo` (e.g. "strvmarv/zoid-releases"). Returns
    /// `(url, number)` on success. Returns `Err` for any HTTP/auth/network
    /// failure. If no token is available, the implementation returns
    /// `Err(NoToken)` so `submit_via` can fall back to the browser URL.
    async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: Vec<String>,
    ) -> anyhow::Result<(String, u64)>;
}

/// The sentinel error when no `$GITHUB_TOKEN` is set.
#[derive(Debug, thiserror::Error)]
#[error("no GITHUB_TOKEN set")]
pub struct NoToken;

/// Real GitHub API client. Token is `$GITHUB_TOKEN` if set.
pub struct HttpFeedbackApi {
    client: reqwest::Client,
    token: Option<String>,
}

impl HttpFeedbackApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("zoid-feedback/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client builds"),
            token: std::env::var("GITHUB_TOKEN").ok(),
        }
    }
}

impl Default for HttpFeedbackApi {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FeedbackApi for HttpFeedbackApi {
    async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: Vec<String>,
    ) -> anyhow::Result<(String, u64)> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::Error::new(NoToken))?;
        let url = format!("https://api.github.com/repos/{repo}/issues");
        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "labels": labels,
        });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;
        if resp.status().as_u16() == 403 {
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            if remaining == "0" {
                anyhow::bail!("GitHub rate-limited. Set $GITHUB_TOKEN for a higher limit.");
            }
        }
        let resp = resp.error_for_status()?;
        let v: serde_json::Value = resp.json().await?;
        let number = v["number"].as_u64().ok_or_else(|| anyhow::anyhow!("missing issue number"))?;
        let html_url = v["html_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing html_url"))?
            .to_string();
        Ok((html_url, number))
    }
}

/// Test double. Configured with a canned outcome.
#[cfg(test)]
pub struct FakeFeedbackApi {
    outcome: std::sync::Mutex<Option<anyhow::Result<(String, u64)>>>,
}

#[cfg(test)]
impl FakeFeedbackApi {
    pub fn created(url: &str, number: u64) -> Self {
        Self {
            outcome: std::sync::Mutex::new(Some(Ok((url.to_string(), number)))),
        }
    }
    pub fn err(msg: &str) -> Self {
        Self {
            outcome: std::sync::Mutex::new(Some(Err(anyhow::anyhow!("{msg}")))),
        }
    }
    pub fn no_token() -> Self {
        Self {
            outcome: std::sync::Mutex::new(Some(Err(anyhow::Error::new(NoToken)))),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl FeedbackApi for FakeFeedbackApi {
    async fn create_issue(
        &self,
        _repo: &str,
        _title: &str,
        _body: &str,
        _labels: Vec<String>,
    ) -> anyhow::Result<(String, u64)> {
        self.outcome
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Err(anyhow::anyhow!("fake already consumed")))
    }
}

impl FeedbackReport {
    /// Submit via `api`. With a token → `Created`; without → `BrowserFallback`.
    pub async fn submit_via(&self, api: &dyn FeedbackApi) -> anyhow::Result<SubmitOutcome> {
        let title = self.to_issue_title();
        let body = self.to_issue_body();
        match api.create_issue(REPO, &title, &body, vec![self.kind.label().to_string()]).await {
            Ok((url, number)) => Ok(SubmitOutcome::Created { url, number }),
            Err(e) if e.downcast_ref::<NoToken>().is_some() => {
                Ok(SubmitOutcome::BrowserFallback { url: self.to_browser_url() })
            }
            Err(e) => Err(e),
        }
    }
}
```

- [ ] **Step 5: Add `thiserror` to the workspace and `zoid-core/Cargo.toml`**

`NoToken` uses `thiserror::Error`. First add to the workspace `Cargo.toml` `[workspace.dependencies]` block:

```toml
thiserror = "2"
```

Then in `crates/zoid-core/Cargo.toml`'s `[dependencies]` block:

```toml
thiserror = { workspace = true }
```

- [ ] **Step 6: Run the `submit_via` tests**

Run: `cargo test -p zoid-core feedback::tests`
Expected: PASS (all tests including the three new async ones).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/feedback.rs crates/zoid-core/Cargo.toml
git commit -m "feat(core): add FeedbackApi seam and submit_via (token or browser fallback)"
```

---

### Task 3: `QuestionKind::Feedback` event variant

**Files:**
- Modify: `crates/zoid-core/src/event.rs:46-59` (add the `Feedback` variant)

**Interfaces:**
- Consumes: nothing (pure addition to the existing enum).
- Produces: `QuestionKind::Feedback { kind: String, title: String, body: String }`, available to the agent loop (Task 8) and the TUI renderer (Task 6).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module at the bottom of `crates/zoid-core/src/event.rs` (if a `tests` module already exists, add inside it; otherwise add one):

```rust
    #[test]
    fn question_kind_feedback_round_trips() {
        let k = QuestionKind::Feedback {
            kind: "bug".into(),
            title: "Crash".into(),
            body: "steps".into(),
        };
        let s = serde_json::to_string(&k).unwrap();
        let back: QuestionKind = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, QuestionKind::Feedback { kind, .. } if kind == "bug"));
    }
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test -p zoid-core event::tests::question_kind_feedback_round_trips`
Expected: FAIL — `QuestionKind::Feedback` does not exist.

- [ ] **Step 3: Add the `Feedback` variant**

In `crates/zoid-core/src/event.rs`, extend the `QuestionKind` enum (after the `Approval` variant):

```rust
pub enum QuestionKind {
    Ask,
    ModeMapping { mapping: Box<crate::wizard::ModeMapping> },
    Approval,
    /// The `submit_feedback` tool's proposal: the agent's draft report. The
    /// bin seeds the `Feedback` overlay from these fields; the user edits and
    /// confirms. `kind` is the string form ("bug"|"feature"|"general").
    Feedback {
        kind: String,
        title: String,
        body: String,
    },
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid-core event::tests::question_kind_feedback_round_trips`
Expected: PASS.

- [ ] **Step 5: Check for exhaustive matches that now need an arm**

Run: `cargo build -p zoid-core -p zoid-tui 2>&1 | grep -i "non-exhaustive\|QuestionKind"`
Expected: any non-exhaustive match errors are listed. Fix each by adding a `QuestionKind::Feedback { .. } =>` arm. The known site is `crates/zoid-core/src/projection.rs:113` (`!matches!(kind, QuestionKind::Approval)`) — that one already treats `Feedback` correctly (it is not `Approval`, so it suppresses the ToolResult). If `projection.rs` has a rendering match on `QuestionKind` that needs a `Feedback` arm, add one that renders the proposed title/body (a simple one-line card; full rendering is Task 6). Check `crates/zoid/src/agent.rs` for matches on `QuestionKind` and add arms returning the appropriate string.

- [ ] **Step 6: Add a projection test asserting `Feedback` suppresses the ToolResult**

The spec (§6.4) relies on `QuestionKind::Feedback` suppressing the paired `ToolResult` from the conversation view (same as `Ask`/`ModeMapping`). Add a test to `crates/zoid-core/src/projection.rs`'s tests, mirroring the existing `Ask` suppression test (search for a test that pairs `QuestionAsked` + `ToolResult` by id and asserts the `ToolResult` is hidden):

```rust
    #[test]
    fn feedback_question_suppresses_paired_tool_result() {
        // A QuestionAsked(Feedback) followed by a ToolResult with the same id:
        // the ToolResult must be suppressed from the conversation view (the card
        // is the human-facing record), same as Ask/ModeMapping.
        let events = vec![
            Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
                id: "fb1".into(), name: "submit_feedback".into(), args: "{}".into(),
            }),
            Event::new(Ulid::new(), None, 1, EventKind::QuestionAsked {
                id: "fb1".into(),
                kind: QuestionKind::Feedback {
                    kind: "bug".into(), title: "Crash".into(), body: "steps".into(),
                },
                question: "Submit Bug feedback?".into(),
                choices: vec!["Submit".into(), "Cancel".into()],
            }),
            Event::new(Ulid::new(), None, 2, EventKind::ToolResult {
                id: "fb1".into(), name: "submit_feedback".into(),
                output: "Created issue #7".into(), is_error: false,
            }),
        ];
        let msgs = crate::projection::project(&events, &Default::default());
        // The ToolResult must NOT appear as a standalone line; it's folded into
        // the question card. Assert no ChatMsg carries the raw "Created issue #7"
        // as tool-result text outside the card.
        for m in &msgs {
            if let crate::projection::ChatMsg::Assistant { tool_calls, .. } = m {
                for c in tool_calls {
                    assert!(!c.result.as_deref().unwrap_or("").contains("Created issue #7"),
                        "ToolResult must be suppressed by the Feedback question card");
                }
            }
        }
    }
```

(Adjust the assertion shape to match the actual `ChatMsg`/`ToolCallRef` projection types — read the existing `Ask` suppression test in `projection.rs` and mirror its exact assertion pattern.)

- [ ] **Step 7: Run the full core + tui build + tests**

Run: `cargo build -p zoid-core -p zoid-tui && cargo test -p zoid-core event::tests projection::tests::feedback_question_suppresses_paired_tool_result`
Expected: clean build; projection test PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-core/src/event.rs
git commit -m "feat(core): add QuestionKind::Feedback variant for the submit_feedback tool"
```

---

### Task 4: `submit_feedback` tool + registry wiring

**Files:**
- Create: `crates/zoid-tools/src/feedback.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add `pub mod feedback;`; register in both registries; extend three tests)

**Interfaces:**
- Consumes: `zoid_tools::{Tool, ToolKind, ToolOutput}` (existing), `zoid_provider::ToolSpec`.
- Produces: `SubmitFeedback` (name `submit_feedback`, `ToolKind::Interactive`), registered in `registry()` and `registry_with_kill()`.

- [ ] **Step 1: Write the failing test for the tool spec**

Create `crates/zoid-tools/src/feedback.rs`:

```rust
//! `submit_feedback` — an Interactive tool. The agent loop intercepts it by
//! kind (alongside `ask_user` and `apply_mode_mapping`), surfaces the proposal
//! to the user via the `Feedback` overlay, and submits on confirm. `run()` is
//! never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct SubmitFeedback;

impl Tool for SubmitFeedback {
    fn name(&self) -> &str {
        "submit_feedback"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "submit_feedback".into(),
            description: "Offer to submit user feedback or a bug report to the zoid \
                maintainers (GitHub issues on strvmarv/zoid-releases). The user MUST \
                confirm/edit before it is submitted — never file silently. Use when \
                the user asks to report a bug or give feedback, or when a reproducible \
                error occurs and the user agrees to report it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["bug","feature","general"] },
                    "title": { "type": "string", "description": "Short summary of the issue or feedback" },
                    "body":  { "type": "string", "description": "Detailed description: steps to reproduce, expected vs actual, or the suggestion" }
                },
                "required": ["kind", "title", "body"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Interactive
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("submit_feedback must be handled by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_advertises_submit_feedback_schema() {
        let s = SubmitFeedback.spec();
        assert_eq!(s.name, "submit_feedback");
        assert_eq!(SubmitFeedback.kind(), ToolKind::Interactive);
        assert!(s.parameters["properties"]["kind"].is_object());
        assert!(s.parameters["properties"]["title"].is_object());
        assert!(s.parameters["properties"]["body"].is_object());
        assert_eq!(s.parameters["required"][0], "kind");
    }

    #[test]
    fn run_is_error_not_panic() {
        let out = SubmitFeedback.run(&json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("must be handled by the agent loop"));
    }
}
```

- [ ] **Step 2: Register the module and the tool**

In `crates/zoid-tools/src/lib.rs`:
- Add `pub mod feedback;` to the module list (after `edit` to stay roughly alphabetical).
- In `registry()`, add `Box::new(feedback::SubmitFeedback),` after the `AskUser` line.
- In `registry_with_kill(kill)`, add `Box::new(feedback::SubmitFeedback),` after the `AskUser` line.

```rust
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::Read),
        Box::new(write::Write),
        Box::new(edit::Edit),
        Box::new(search::Grep),
        Box::new(glob::GlobTool),
        Box::new(ls::Ls),
        Box::new(shell::Shell::default()),
        Box::new(tasks::UpdateTasks),
        Box::new(ask::AskUser),
        Box::new(feedback::SubmitFeedback),
    ]
}
```

Mirror the addition in `registry_with_kill`.

- [ ] **Step 3: Run the tool's own tests**

Run: `cargo test -p zoid-tools feedback::tests`
Expected: PASS (2 tests).

- [ ] **Step 4: Extend the registry assertions in `lib.rs` tests**

In `crates/zoid-tools/src/lib.rs`'s `tests` module, update `registry_has_unique_named_tools` to assert `submit_feedback` is present, and `registry_tools_are_all_local_by_default` to exclude `submit_feedback` alongside `update_tasks`/`ask_user`:

```rust
        assert!(names.contains(&"submit_feedback"));
```

```rust
            .filter(|t| t.name() != "update_tasks" && t.name() != "ask_user" && t.name() != "submit_feedback")
```

- [ ] **Step 5: Extend the subagent exclusion test**

In `crates/zoid/src/subagent.rs`, find the `assembled_tools_exclude_interactive_ask_user` test. Add an assertion that `submit_feedback` is also excluded:

```rust
            !tools.iter().any(|t| t.name() == "submit_feedback"),
            "submit_feedback must be filtered out of a subagent's tool set"
```

And in the matching assertion about advertised tool specs (the `!req.tools.iter().any(|s| s.name == "ask_user")` pattern), add:

```rust
            !req.tools.iter().any(|s| s.name == "submit_feedback"),
            "submit_feedback must not be advertised to the provider for a subagent"
```

- [ ] **Step 6: Run all zoid-tools + the subagent test**

Run: `cargo test -p zoid-tools && cargo test -p zoid subagent::tests`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/feedback.rs crates/zoid-tools/src/lib.rs crates/zoid/src/subagent.rs
git commit -m "feat(tools): add submit_feedback Interactive tool and register it"
```

---

### Task 5: `:feedback` command, `Overlay::Feedback`, and `route_feedback_key`

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (add `Command::Feedback`; parse `feedback`)
- Modify: `crates/zoid-tui/src/state.rs` (add `Overlay::Feedback`, `FeedbackState`, `FeedbackField`, `FeedbackStatus`; add `feedback: Option<FeedbackState>` field; default it)
- Create: `crates/zoid-tui/src/feedback_view.rs` (state helpers + `route_feedback_key`)
- Modify: `crates/zoid-tui/src/lib.rs` (add `pub mod feedback_view;`)

**Interfaces:**
- Consumes: `zoid_core::feedback::{FeedbackKind, SubmitOutcome}` (Task 1/2).
- Produces: `Command::Feedback`; `Overlay::Feedback`; `FeedbackState` (with `focus`/`kind`/`kind_selected`/`title`/`body`/`status`); `route_feedback_key(&FeedbackState, KeyEvent) -> Action`; new `Action::Feedback*` variants (consumed by Task 6 render + Task 9 bin wiring).

- [ ] **Step 1: Write the failing command-parse test**

Add to `crates/zoid-tui/src/command.rs`'s `tests` module:

```rust
    #[test]
    fn parses_feedback_command() {
        assert_eq!(parse_command(":feedback"), Command::Feedback);
        assert_eq!(parse_command("feedback"), Command::Feedback);
    }
```

- [ ] **Step 2: Run it to confirm it fails to compile**

Run: `cargo test -p zoid-tui command::tests::parses_feedback_command`
Expected: FAIL — `Command::Feedback` does not exist.

- [ ] **Step 3: Add `Command::Feedback` and the parse arm**

In `crates/zoid-tui/src/command.rs`, add the variant to the `Command` enum (before `Unknown`):

```rust
    /// Open the feedback submission overlay (`:feedback`).
    Feedback,
```

In `parse_command`, add a flat arm (near `"compact" => Command::CompactNow,`):

```rust
        "feedback" => Command::Feedback,
```

- [ ] **Step 4: Run the parse test**

Run: `cargo test -p zoid-tui command::tests::parses_feedback_command`
Expected: PASS.

- [ ] **Step 5: Add `Overlay::Feedback` + `FeedbackState` types to `state.rs`**

In `crates/zoid-tui/src/state.rs`, add `Feedback` to the `Overlay` enum (before the closing brace):

```rust
    Feedback,
```

Add the new types (after the `QuestionState`-related types or near the other overlay state):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackField {
    Kind,
    Title,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackStatus {
    Idle,
    Submitting,
    Done(zoid_core::feedback::SubmitOutcome),
    Error(String),
}

/// State for the `:feedback` overlay: a single form (kind picker, title, body).
/// Seeded empty by the command, or pre-filled by the agent tool's proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackState {
    pub focus: FeedbackField,
    pub kind: zoid_core::feedback::FeedbackKind,
    pub kind_selected: usize,
    pub title: String,
    pub body: String,
    pub status: FeedbackStatus,
}

impl FeedbackState {
    pub fn new() -> Self {
        Self {
            focus: FeedbackField::Kind,
            kind: zoid_core::feedback::FeedbackKind::Bug,
            kind_selected: 0,
            title: String::new(),
            body: String::new(),
            status: FeedbackStatus::Idle,
        }
    }
}
```

Add the field to `ShellState` (near the `question` field). `FeedbackState` is defined in `state.rs` (this task, Step 5), so use the local type:

```rust
    pub feedback: Option<FeedbackState>,
```

In `ShellState::new()`, default it: `feedback: None,`.

- [ ] **Step 6: Add `Action::Feedback*` variants**

In `crates/zoid-tui/src/route.rs`, add to the `Action` enum (before `Noop`):

```rust
    FeedbackMoveFocus(i32),
    FeedbackCycleKind(i32),
    FeedbackChar(char),
    FeedbackBackspace,
    FeedbackSubmit,
    FeedbackAbort,
```

- [ ] **Step 7: Write `route_feedback_key` + state helpers**

Create `crates/zoid-tui/src/feedback_view.rs`:

```rust
//! Key routing for the `:feedback` overlay. Tab/Shift+Tab cycle focus across
//! Kind → Title → Body; Up/Down cycle the kind when focused; Ctrl+Enter submits;
//! Esc aborts. Mirrors `question.rs`'s `route_question_key`.

use crate::route::Action;
use crate::state::{FeedbackField, FeedbackState};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Map a keypress to an `Action` while the feedback overlay is open.
pub fn route_feedback_key(state: &FeedbackState, key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Action::FeedbackAbort,
        KeyCode::Tab => Action::FeedbackMoveFocus(1),
        KeyCode::BackTab => Action::FeedbackMoveFocus(-1),
        KeyCode::Enter => match state.focus {
            FeedbackField::Title => Action::FeedbackMoveFocus(1),
            FeedbackField::Body => Action::Noop, // newline handled by the textarea in the bin
            FeedbackField::Kind => Action::FeedbackMoveFocus(1),
        },
        KeyCode::Up if state.focus == FeedbackField::Kind => Action::FeedbackCycleKind(-1),
        KeyCode::Down if state.focus == FeedbackField::Kind => Action::FeedbackCycleKind(1),
        KeyCode::Backspace => Action::FeedbackBackspace,
        KeyCode::Char(c) if ctrl && c == 'm' => Action::FeedbackSubmit, // Ctrl+Enter often arrives as Ctrl+M
        KeyCode::Char(c) if ctrl && (c == '\n' || c == '\r') => Action::FeedbackSubmit,
        KeyCode::Char(c) => Action::FeedbackChar(c),
        _ => Action::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn esc_aborts() {
        let s = FeedbackState::new();
        assert_eq!(route_feedback_key(&s, k(KeyCode::Esc, KeyModifiers::NONE)), Action::FeedbackAbort);
    }

    #[test]
    fn tab_moves_focus_forward() {
        let s = FeedbackState::new();
        assert_eq!(route_feedback_key(&s, k(KeyCode::Tab, KeyModifiers::NONE)), Action::FeedbackMoveFocus(1));
    }

    #[test]
    fn up_down_cycle_kind_only_when_kind_focused() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Kind;
        assert_eq!(route_feedback_key(&s, k(KeyCode::Up, KeyModifiers::NONE)), Action::FeedbackCycleKind(-1));
        assert_eq!(route_feedback_key(&s, k(KeyCode::Down, KeyModifiers::NONE)), Action::FeedbackCycleKind(1));
        s.focus = FeedbackField::Title;
        assert_eq!(route_feedback_key(&s, k(KeyCode::Up, KeyModifiers::NONE)), Action::Noop);
    }

    #[test]
    fn enter_in_title_moves_to_body_not_submit() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Title;
        assert_eq!(route_feedback_key(&s, k(KeyCode::Enter, KeyModifiers::NONE)), Action::FeedbackMoveFocus(1));
    }

    #[test]
    fn ctrl_enter_submits_in_body() {
        let mut s = FeedbackState::new();
        s.focus = FeedbackField::Body;
        assert_eq!(route_feedback_key(&s, k(KeyCode::Enter, KeyModifiers::CONTROL)), Action::FeedbackSubmit);
    }

    #[test]
    fn char_routes_to_feedback_char() {
        let s = FeedbackState::new();
        assert_eq!(route_feedback_key(&s, k(KeyCode::Char('x'), KeyModifiers::NONE)), Action::FeedbackChar('x'));
    }
}
```

Add `pub mod feedback_view;` to `crates/zoid-tui/src/lib.rs` (alphabetical, after `command`).

- [ ] **Step 8: Wire `Overlay::Feedback` into `route_key`**

In `crates/zoid-tui/src/route.rs`'s `route_key`, near the `Overlay::Config => return route_config_key(state, key)` line, add:

```rust
        Overlay::Feedback => {
            if let Some(fs) = &state.feedback {
                return route_feedback_key(fs, key);
            }
            return Action::Noop;
        }
```

- [ ] **Step 9: Run the feedback_view tests + command tests + build**

Run: `cargo test -p zoid-tui feedback_view::tests command::tests::parses_feedback_command && cargo build -p zoid-tui`
Expected: PASS + clean build. (The render match arms are added in Task 6; if the build fails on a non-exhaustive overlay match in `layout.rs`/`render.rs`, add a temporary `Overlay::Feedback =>` no-op arm now and replace it in Task 6.)

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/feedback_view.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): add :feedback command, Overlay::Feedback, FeedbackState, and key routing"
```

---

### Task 6: Render the `Feedback` overlay + palette row + layout arm

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (render the modal; add `Command::Feedback` preview arm; add the overlay arm replacing any temporary no-op from Task 5)
- Modify: `crates/zoid-tui/src/layout.rs:275` (add `Overlay::Feedback` to the overlay match that blocks the conversation)
- Modify: `crates/zoid-tui/src/palette.rs` (add "Submit feedback" `PaletteItem`)

**Interfaces:**
- Consumes: `FeedbackState`, `FeedbackField`, `FeedbackStatus`, `Overlay::Feedback`, `Command::Feedback` (Task 5), `zoid_core::feedback::FeedbackKind` (Task 1).
- Produces: a rendered modal; a discoverable palette row; the conversation-block arm.

- [ ] **Step 1: Add the layout arm (blocks conversation while the overlay is open)**

In `crates/zoid-tui/src/layout.rs`, find the match at ~line 275 that lists `Overlay::Palette | Overlay::Objects | ... | Overlay::Mcp` and add `| Overlay::Feedback` to that arm (so the conversation doesn't capture keys/render behind the modal).

- [ ] **Step 2: Add the `Command::Feedback` preview arm in `render.rs`**

In `crates/zoid-tui/src/render.rs`, in the `match cmd` block (~line 882), add an arm (before the closing brace of the match):

```rust
                    Command::Feedback => "→ Submit feedback".to_string(),
```

- [ ] **Step 3: Render the `Feedback` overlay modal**

In `crates/zoid-tui/src/render.rs`, near the `else if state.overlay == Overlay::Mcp` / `Overlay::Config` block (~line 194), add:

```rust
    } else if state.overlay == Overlay::Feedback {
        if let Some(fs) = &state.feedback {
            render_feedback_modal(frame, area, fs);
        }
    }
```

Add the `render_feedback_modal` function in `render.rs` (or a small helper section near the other overlay renderers):

```rust
/// Render the `:feedback` modal: kind picker, title, body, status line.
fn render_feedback_modal(frame: &mut Frame, area: Rect, fs: &crate::state::FeedbackState) {
    use crate::state::FeedbackField;
    use ratatui::widgets::{Block, Borders, Paragraph};
    use ratatui::layout::{Constraint, Layout};

    // Centered modal, ~60% width, enough height for the form.
    let modal = centered_rect(area, 60, 18);
    frame.render_widget(
        Block::default().borders(Borders::ALL).title(" Submit feedback "),
        modal,
    );
    let inner = modal.inner(&ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let chunks = Layout::default()
        .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    // 1. Kind row.
    let kinds = zoid_core::feedback::FeedbackKind::all();
    let kind_row: String = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| {
            if i == fs.kind_selected {
                format!("[{}]", k.display())
            } else {
                format!(" {} ", k.display())
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let kind_style = if fs.focus == FeedbackField::Kind {
        Style::new().fg(color::CHAT_ACCENT)
    } else {
        Style::new().fg(color::TXT)
    };
    frame.render_widget(Paragraph::new(kind_row).style(kind_style), chunks[0]);

    // 2. Title input.
    let title_block = Block::default().borders(Borders::ALL).title(if fs.focus == FeedbackField::Title { " Title " } else { " Title " });
    frame.render_widget(Paragraph::new(fs.title.as_str()).block(title_block), chunks[1]);

    // 3. Body textarea (plain paragraph for v1; multi-line editing via the bin's buffer).
    let body_block = Block::default().borders(Borders::ALL).title(" Description ");
    frame.render_widget(Paragraph::new(fs.body.as_str()).block(body_block), chunks[2]);

    // 4. Footer hint.
    frame.render_widget(
        Paragraph::new("Tab next · Ctrl+Enter submit · Esc cancel").style(Style::new().fg(color::DIM)),
        chunks[3],
    );

    // 5. Status line.
    let status = match &fs.status {
        crate::state::FeedbackStatus::Idle => String::new(),
        crate::state::FeedbackStatus::Submitting => "Submitting…".to_string(),
        crate::state::FeedbackStatus::Done(zoid_core::feedback::SubmitOutcome::Created { url, number }) => {
            format!("Created issue #{}: {}", number, url)
        }
        crate::state::FeedbackStatus::Done(zoid_core::feedback::SubmitOutcome::BrowserFallback { url }) => {
            format!("No token — opened your browser: {}", url)
        }
        crate::state::FeedbackStatus::Error(msg) => format!("Error: {}", msg),
    };
    if !status.is_empty() {
        frame.render_widget(Paragraph::new(status).style(Style::new().fg(color::WARNING)), chunks[4]);
    }
}

/// Center a rect of the given width%/height inside `area`.
fn centered_rect(area: Rect, width_pct: u16, height: u16) -> Rect {
    let pop = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([Constraint::Min(0)])
        .split(area)[0];
    let h = height.min(pop.height);
    let w = pop.width * width_pct / 100;
    let x = pop.x + (pop.width - w) / 2;
    let y = pop.y + (pop.height - h) / 2;
    Rect { x, y, width: w, height: h }
}
```

Note: if `centered_rect` already exists in `render.rs`, reuse it; do not define a duplicate. Grep for `fn centered_rect` first.

- [ ] **Step 4: Add the palette row**

In `crates/zoid-tui/src/palette.rs`'s `all_items`, add a row before the companion row:

```rust
    items.push(PaletteItem {
        label: "Submit feedback…".to_string(),
        command: Command::Feedback,
    });
```

- [ ] **Step 5: Build the TUI**

Run: `cargo build -p zoid-tui`
Expected: clean build.

- [ ] **Step 6: Run all zoid-tui tests**

Run: `cargo test -p zoid-tui`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/layout.rs crates/zoid-tui/src/palette.rs
git commit -m "feat(tui): render the Feedback overlay modal and add the palette row"
```

---

### Task 7: Built-in `feedback` skill in `SkillRegistry::builtin()`

**Files:**
- Modify: `crates/zoid-core/src/skill.rs` (add `FEEDBACK_SKILL_BODY` const + third `Skill` in `builtin()`; update affected tests)

**Interfaces:**
- Consumes: `Skill` struct (existing).
- Produces: a third built-in skill named `feedback` with `base_dir: None`, available globally to all modes.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-core/src/skill.rs`'s `tests` module:

```rust
    #[test]
    fn builtin_includes_feedback_skill() {
        let r = SkillRegistry::builtin();
        assert_eq!(
            r.names(),
            vec!["spike-plan".to_string(), "spike-implement".to_string(), "feedback".to_string()]
        );
        let fb = r.get("feedback").unwrap();
        assert!(fb.body.contains("submit_feedback"), "feedback skill must reference the submit_feedback tool");
        assert!(fb.body.contains("strvmarv/zoid-releases"));
        assert!(fb.base_dir.is_none());
    }

    #[test]
    fn push_unique_protects_feedback_builtin_from_shadow() {
        let mut r = SkillRegistry::builtin();
        let shadow = Skill {
            name: "feedback".into(),
            description: "shadow".into(),
            body: "SHADOW".into(),
            base_dir: None,
        };
        assert!(!r.push_unique(shadow), "an import must not shadow the built-in feedback");
        assert_eq!(r.get("feedback").unwrap().body, FEEDBACK_SKILL_BODY);
    }
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p zoid-core skill::tests::builtin_includes_feedback_skill skill::tests::push_unique_protects_feedback_builtin_from_shadow`
Expected: FAIL — `FEEDBACK_SKILL_BODY` undefined; `feedback` not in `builtin()`.

- [ ] **Step 3: Add the `FEEDBACK_SKILL_BODY` const + the third skill**

In `crates/zoid-core/src/skill.rs`, add the const above `impl SkillRegistry`:

```rust
/// The body of the built-in `feedback` skill. References the `submit_feedback`
/// tool and the `strvmarv/zoid-releases` repo.
const FEEDBACK_SKILL_BODY: &str = "\
# Submitting Feedback & Bug Reports

zoid can file feedback or bug reports to the maintainers as GitHub issues on
`strvmarv/zoid-releases`. The `submit_feedback` tool proposes a report; the
user **always confirms and can edit** before it is submitted — never file
silently.

## When to Offer

Offer the tool when:
- The user explicitly asks to \"report a bug\", \"give feedback\", or \"file an issue\".
- A reproducible error occurs AND the user agrees to report it (ask first via
  `ask_user` — don't assume).
- The user expresses frustration about zoid's behavior and a concrete, actionable
  issue can be identified.

Do NOT offer when:
- The user is frustrated with *their own code* (that's not a zoid bug).
- The error is clearly user error (wrong path, bad config) — help them instead.
- The user just wants to vent; only file if there's something actionable.

## Writing a Good Report

Call `submit_feedback` with a well-structured report:

- **kind**: `bug`, `feature`, or `general`.
- **title**: One line, specific. Bad: \"it crashed\". Good: \"Crash on `:config`
  open when no provider is configured\".
- **body**: For bugs — steps to reproduce, expected behavior, actual behavior.
  For features — the use case and the proposed solution. For general — what's
  on your mind.

Diagnostics (version, OS, session, mode, model, cwd, recent error) are
attached automatically — you don't need to gather them. But **describe the
user's situation in the body**, since you know the context that led here.

## After Submitting

The tool result tells you the outcome:
- `Created issue #N: <url>` — tell the user the issue number and URL.
- `Opened browser at <url>` — tell the user to finish submitting in the
  browser (no token was available), and give them the URL.
- `User declined` — acknowledge and move on; don't push.

Never call `submit_feedback` twice for the same issue in one session unless the
user asks.
";
```

In `builtin()`, add the third `Skill`:

```rust
    pub fn builtin() -> Self {
        Self::new(vec![
            Skill {
                name: "spike-plan".into(),
                description: "Draft the plan for the spike task, then hand off to spike-implement.".into(),
                body: "You are executing the 'spike-plan' skill.\n\n\
                    The task: create a file at ./spike-artifact.txt whose only line is: spike ok\n\n\
                    Step 1: restate that plan in one short sentence.\n\
                    Step 2: to carry the plan out, call the invoke_skill tool with name \
                    \"spike-implement\".\n\
                    Do NOT write the file yourself in this step — spike-implement does that."
                    .into(),
                base_dir: None,
            },
            Skill {
                name: "spike-implement".into(),
                description: "Write the spike artifact file described by the plan.".into(),
                body: "You are executing the 'spike-implement' skill.\n\n\
                    Create the file ./spike-artifact.txt with exactly one line of content: spike ok\n\
                    Use the Write tool. Then confirm in one sentence that you wrote it."
                    .into(),
                base_dir: None,
            },
            Skill {
                name: "feedback".into(),
                description: "Use when the user asks to report a bug or give feedback, \
                    or when a reproducible error occurs and the user agrees to report it — \
                    offers the submit_feedback tool to file a GitHub issue on \
                    strvmarv/zoid-releases, with the user confirming before anything \
                    is submitted.".into(),
                body: FEEDBACK_SKILL_BODY.into(),
                base_dir: None,
            },
        ])
    }
```

- [ ] **Step 4: Update the other tests that assert the two-skill list**

In `crates/zoid-core/src/skill.rs`'s `tests` module, update:
- `builtin_has_both_spike_skills_that_chain`: keep the spike assertions (it still checks `spike-plan` chains to `spike-implement`); the test name is fine but the `names` assertion in `all_exposes_every_skill_in_order` must now expect three.
- `menu_renders_one_line_per_skill`: assert `menu.lines().count() == 3` and that it contains `- feedback: `.
- `all_exposes_every_skill_in_order`: expect `["spike-plan", "spike-implement", "feedback"]`.
- `builtin_skills_have_no_base_dir`: add an assertion that `feedback` also has `base_dir: None`.

For example, update `menu_renders_one_line_per_skill`:

```rust
    #[test]
    fn menu_renders_one_line_per_skill() {
        let menu = SkillRegistry::builtin().menu();
        assert!(menu.contains("- spike-plan: "));
        assert!(menu.contains("- spike-implement: "));
        assert!(menu.contains("- feedback: "));
        assert_eq!(menu.lines().count(), 3);
    }
```

And `all_exposes_every_skill_in_order`:

```rust
        assert_eq!(names, vec!["spike-plan", "spike-implement", "feedback"]);
```

- [ ] **Step 5: Run all skill tests**

Run: `cargo test -p zoid-core skill`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/skill.rs
git commit -m "feat(core): add built-in feedback skill to SkillRegistry::builtin()"
```

---

### Task 8: Agent loop interception of `submit_feedback`

**Files:**
- Modify: `crates/zoid/src/agent.rs:1162-1163` (extend the `Interactive` match arm)

**Interfaces:**
- Consumes: `FeedbackKind::parse` (Task 1), `QuestionKind::Feedback` (Task 3), `FeedbackReport`/`Diagnostics`/`FeedbackApi`/`HttpFeedbackApi` (Tasks 1/2), `SubmitOutcome` (Task 2). The existing `ask_user`/`apply_mode_mapping` park-and-await plumbing (oneshot reply channel, `QuestionAsked`/`QuestionAnswered` events, `ToolResult` emit).
- Produces: the `submit_feedback` interception that emits `QuestionAsked { kind: Feedback, .. }`, parks, and on reply either submits (building a `FeedbackReport` + `Diagnostics` + `submit_via`) or returns the decline string.

**Note on the reply mechanism (verified against `agent.rs:1245-1281`):** the existing `ask_user`/`apply_mode_mapping` path works as follows: the loop creates a `oneshot::channel::<Answer>()`, sends `AgentUpdate::AskUser { question, choices, reply: rtx }` on `ui`, then `rrx.await` returns an `Answer` (`Choice(String)` | `FreeText(String)` | `LetYouDecide`) or `Err` (the bin drops the sender on Esc → `Err` → `"[user aborted]"`). The `Answer` enum (`agent.rs:117`) has **no explicit decline variant** — Esc is modeled by dropping the sender.\n\nThe `Feedback` overlay fits this mechanism by carrying the report *inside* the `Answer` (no shared state between `agent.rs` and `main.rs`):\n- **Submit** → the bin builds the `FeedbackReport` from `FeedbackState` + `Diagnostics`, then sends `Answer::Feedback(report)` on `rtx`.\n- **Cancel/Esc** → the bin drops `rtx` (→ `Err` in the loop → treated as declined).\n- This requires adding a new `Answer::Feedback(FeedbackReport)` variant (`agent.rs:117`). The loop, on receiving `Answer::Feedback(report)`, submits it via the injected `FeedbackApi` and emits the `ToolResult`. The loop stays **stateless** — no `App` handle, no `feedback_pending` stash. This matches the existing oneshot pattern exactly and avoids threading `App` into the turn.\n\nBecause of this, **Task 8's loop code builds nothing** — it receives the pre-built `FeedbackReport` in the `Answer` and submits it. **Task 9's bin code** (the `Action::FeedbackSubmit` handler) builds the `FeedbackReport` + `Diagnostics` from `FeedbackState` + `App` state and sends `Answer::Feedback(report)` down the oneshot. This is a clean split: the bin owns report assembly (it has the state), the loop owns submission + tool-result emission.
- **Submit** → the bin sends `Answer::Choice("submit")` (or `FreeText` with a marker) on `rtx`.
- **Cancel/Esc** → the bin drops `rtx` (→ `Err` in the loop → treated as declined).

This requires adding a new `Answer::Feedback(FeedbackReport)` variant (`agent.rs:117`). The loop, on receiving `Answer::Feedback(report)`, submits it via the injected `FeedbackApi` and emits the `ToolResult`. The loop stays **stateless** — no `App` handle, no shared stash. **Task 9's bin code** (the `Action::FeedbackSubmit` handler) builds the `FeedbackReport` + `Diagnostics` from `FeedbackState` + `App` state and sends `Answer::Feedback(report)` down the oneshot. Clean split: the bin owns report assembly, the loop owns submission + tool-result emission.

- [ ] **Step 1: Write the failing unit test for the parse/validate helper**

The agent-loop turn harness is heavy to set up for a full integration test, so this task tests the parse/validate helper directly. Full loop-integration testing for the `submit_feedback` path is deferred (the harness would need a mocked provider + tool-call stream + `FeedbackApi` injection — a worthwhile follow-up, but out of scope for this plan). Add to `crates/zoid/src/agent.rs`'s test module:

```rust
    #[test]
    fn submit_feedback_parse_validates_kind_title_body() {
        let ok = parse_feedback_args(&json!({"kind":"bug","title":"t","body":"b"}));
        assert!(ok.is_some());
        let bad_kind = parse_feedback_args(&json!({"kind":"x","title":"t","body":"b"}));
        assert!(bad_kind.is_none());
        let empty_title = parse_feedback_args(&json!({"kind":"bug","title":"","body":"b"}));
        assert!(empty_title.is_none());
        let empty_body = parse_feedback_args(&json!({"kind":"bug","title":"t","body":""}));
        assert!(empty_body.is_none());
    }
```

- [ ] **Step 2: Add the `parse_feedback_args` helper**

In `crates/zoid/src/agent.rs`, add a helper near the `Interactive` match arm:

```rust
/// Parse + validate the `submit_feedback` tool-call args.
/// Returns `(kind, title, body)` on success, or `None` on any validation failure.
fn parse_feedback_args(
    args: &serde_json::Value,
) -> Option<(zoid_core::feedback::FeedbackKind, String, String)> {
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(zoid_core::feedback::FeedbackKind::parse)?;
    let title = args.get("title").and_then(|v| v.as_str())?;
    let body = args.get("body").and_then(|v| v.as_str())?;
    if title.trim().is_empty() || body.trim().is_empty() {
        return None;
    }
    Some((kind, title.to_string(), body.to_string()))
}
```

- [ ] **Step 3: Extend the `Interactive` match arm to intercept `submit_feedback`**

In `crates/zoid/src/agent.rs`, find the match arm at ~line 1162:

```rust
                Some(zoid_tools::ToolKind::Interactive)
                    if tc.name == "ask_user" || tc.name == "apply_mode_mapping" =>
                {
```

Change the guard to also match `submit_feedback`:

```rust
                Some(zoid_tools::ToolKind::Interactive)
                    if tc.name == "ask_user"
                        || tc.name == "apply_mode_mapping"
                        || tc.name == "submit_feedback" =>
                {
```

Inside the arm, add a branch for `submit_feedback` before the existing `ask_user`/`apply_mode_mapping` branches. It mirrors their structure (verified against `agent.rs:1245-1281`): parse+validate; on failure emit a `ToolResult { is_error: true }` and `continue`; on success emit a `QuestionAsked`, then create a `oneshot::channel::<Answer>()`, send `AgentUpdate::AskUser { question, choices, reply: rtx }` on `ui`, and `rrx.await`. The bin sends `Answer::Feedback(report)` on submit (carrying the pre-built `FeedbackReport` — no shared state), or drops `rtx` on cancel (`Err` → declined).

**First, add the `Answer::Feedback` variant** to the `Answer` enum (`agent.rs:117`):

```rust
pub enum Answer {
    Choice(String),
    FreeText(String),
    LetYouDecide,
    /// The `submit_feedback` tool's confirmed report (built by the bin from
    /// the edited `FeedbackState` + diagnostics). Carries the report back to
    /// the loop so it can submit without shared state.
    Feedback(zoid_core::feedback::FeedbackReport),
}
```

Then the interception branch:

```rust
                    if tc.name == "submit_feedback" {
                        let (kind, title, body) = match parse_feedback_args(&tc.args) {
                            Some(v) => v,
                            None => {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: tc.id.clone(),
                                        name: tc.name.clone(),
                                        output: "submit_feedback: invalid args. kind must be \
                                            bug|feature|general; title and body must be non-empty."
                                            .into(),
                                        is_error: true,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                                continue;
                            }
                        };
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::QuestionAsked {
                                id: tc.id.clone(),
                                kind: zoid_core::event::QuestionKind::Feedback {
                                    kind: kind_str(kind).to_string(),
                                    title: title.clone(),
                                    body: body.clone(),
                                },
                                question: format!("Submit {} feedback?", kind.display()),
                                choices: vec!["Submit".into(), "Cancel".into()],
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        // Park on a fresh oneshot, mirroring ask_user (agent.rs:1245-1263).
                        let (rtx, rrx) = oneshot::channel::<Answer>();
                        let _ = ui.send(AgentUpdate::AskUser {
                            question: format!("Submit {} feedback?", kind.display()),
                            choices: vec!["Submit".into(), "Cancel".into()],
                            reply: rtx,
                        }).await;
                        let ans = rrx.await;
                        let output = match ans {
                            Ok(Answer::Feedback(report)) => {
                                // The bin built the report (FeedbackState + Diagnostics)
                                // and sent it back inside the Answer. Submit it.
                                let api = zoid_core::feedback::HttpFeedbackApi::new();
                                match report.submit_via(&api).await {
                                    Ok(zoid_core::feedback::SubmitOutcome::Created { url, number }) =>
                                        format!("Created issue #{}: {}", number, url),
                                    Ok(zoid_core::feedback::SubmitOutcome::BrowserFallback { url }) =>
                                        format!("No GitHub token available — opened your browser at {}. \
                                            The user must finish submitting there.", url),
                                    Err(e) => format!("Failed to submit feedback: {e}"),
                                }
                            }
                            _ => "User declined to submit feedback.".to_string(),
                        };
                        emit(
                            &session, &mut events, ui, &config.branch,
                            EventKind::QuestionAnswered { id: tc.id.clone(), answer: output.clone() },
                            session_id, now,
                        ).await?;
                        emit(
                            &session, &mut events, ui, &config.branch,
                            EventKind::ToolResult { id: tc.id.clone(), name: tc.name.clone(),
                                output, is_error: false },
                            session_id, now,
                        ).await?;
                        continue;
                    }
```

**Test injection:** the `HttpFeedbackApi::new()` above is production-only. For tests that exercise the submit path without a network, thread an `Arc<dyn FeedbackApi>` through the turn state (add a `feedback_api` field to the turn args, defaulting to `Arc::new(HttpFeedbackApi::new())`, overridable with `Arc::new(FakeFeedbackApi::created(...))`). The exact threading follows the turn's existing construction (see where `session`/`ui` are assembled into the turn). This keeps the loop stateless while allowing injection.

- [ ] **Step 4: Add the `kind_str` helper**

Add `kind_str` in `crates/zoid/src/agent.rs`:

```rust
fn kind_str(k: zoid_core::feedback::FeedbackKind) -> &'static str {
    match k {
        zoid_core::feedback::FeedbackKind::Bug => "bug",
        zoid_core::feedback::FeedbackKind::FeatureRequest => "feature",
        zoid_core::feedback::FeedbackKind::General => "general",
    }
}
```

**Why `Answer::Feedback` (no shared state):** the bin owns `FeedbackState` (the editable form) and `App` (diagnostics sources: `session_id: Ulid`, `modes.active_name()`, `config.provider: String`, `config.model: String`, `events.snapshot()`, `std::env::current_dir()`). Building the report in the bin and carrying it back inside the `Answer` means the loop never needs an `App` handle — it matches the existing `ask_user` oneshot pattern exactly. The `Config` struct (verified in `crates/zoid-core/src/config.rs:28-39`) has `provider: String` and `model: String` (NOT `Option`); the mode comes from `app.modes.active_name()` (the `ModeRegistry`), not from `Config`.

- [ ] **Step 6: Build the bin**

Run: `cargo build -p zoid`
Expected: clean build. Fix any field-name mismatches in `capture_diagnostics` against the real `Config` struct.

- [ ] **Step 7: Run the parse/validate test**

Run: `cargo test -p zoid submit_feedback_parse_validates_kind_title_body`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(bin): intercept submit_feedback in the agent loop and submit on confirm"
```

---

### Task 9: `Command::Feedback` dispatch + diagnostics capture + async submit (the bin)

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `Command::Feedback` arm in `exec_command`; open the overlay; wire `Action::FeedbackSubmit` to build a report + `submit_via().await`; wire the other `Action::Feedback*` variants to mutate `FeedbackState`; capture diagnostics)

**Interfaces:**
- Consumes: `FeedbackState`/`FeedbackField`/`FeedbackStatus`/`Overlay::Feedback` (Task 5), `FeedbackReport`/`Diagnostics`/`HttpFeedbackApi`/`submit_via`/`SubmitOutcome` (Tasks 1/2), `Action::Feedback*` (Task 5). The existing `exec_command(app, cmd)` + the existing async event path (mirroring `Command::CompactNow`'s `tokio::spawn`).

- [ ] **Step 1: Add the `Command::Feedback` arm in `exec_command`**

In `crates/zoid/src/main.rs`'s `exec_command` (~line 4016), add an arm (before `Command::Unknown(_)`):

```rust
        Command::Feedback => {
            app.shell.overlay = zoid_tui::Overlay::Feedback;
            app.shell.feedback = Some(zoid_tui::feedback_view::FeedbackState::new());
            Ok(false)
        }
```

Wait — `FeedbackState` is in `state.rs` (Task 5 placed the type there); `feedback_view` has `route_feedback_key`. Use the correct path:

```rust
        Command::Feedback => {
            app.shell.overlay = zoid_tui::Overlay::Feedback;
            app.shell.feedback = Some(zoid_tui::state::FeedbackState::new());
            Ok(false)
        }
```

- [ ] **Step 2: Wire the `Action::Feedback*` variants in the main event loop**

Find where `Action` variants are matched in `main.rs` (the key-dispatch site that handles `Action::QuestionSelect` etc.). Add arms:

```rust
        Action::FeedbackAbort => {
            app.shell.feedback = None;
            app.shell.overlay = zoid_tui::Overlay::None;
        }
        Action::FeedbackMoveFocus(dir) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                let order = [FeedbackField::Kind, FeedbackField::Title, FeedbackField::Body];
                let idx = order.iter().position(|f| *f == fs.focus).unwrap_or(0);
                let n = order.len() as i32;
                let next = ((idx as i32 + dir).rem_euclid(n)) as usize;
                fs.focus = order[next];
            }
        }
        Action::FeedbackCycleKind(dir) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                let n = zoid_core::feedback::FeedbackKind::all().len() as i32;
                fs.kind_selected = ((fs.kind_selected as i32 + dir).rem_euclid(n)) as usize;
                fs.kind = zoid_core::feedback::FeedbackKind::all()[fs.kind_selected];
            }
        }
        Action::FeedbackChar(c) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                match fs.focus {
                    FeedbackField::Title => fs.title.push(c),
                    FeedbackField::Body => fs.body.push(c),
                    FeedbackField::Kind => {}
                }
            }
        }
        Action::FeedbackBackspace => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                match fs.focus {
                    FeedbackField::Title => { fs.title.pop(); }
                    FeedbackField::Body => { fs.body.pop(); }
                    FeedbackField::Kind => {}
                }
            }
        }
        Action::FeedbackSubmit => {
            let fs = match app.shell.feedback.clone() {
                Some(fs) => fs,
                None => return, // adjust to the surrounding control flow
            };
            // Validate non-empty title/body before submitting.
            if fs.title.trim().is_empty() || fs.body.trim().is_empty() {
                if let Some(f) = app.shell.feedback.as_mut() {
                    f.status = FeedbackStatus::Error("Title and description are required.".into());
                }
                return;
            }
            let diagnostics = capture_app_diagnostics(app);
            let report = zoid_core::feedback::FeedbackReport {
                kind: fs.kind,
                title: fs.title,
                body: fs.body,
                diagnostics,
            };

            if let Some(reply) = app.feedback_reply.take() {
                // TOOL PATH: the agent loop is parked awaiting this reply.
                // Send the built report back inside the Answer; the loop submits.
                let _ = reply.send(zoid::agent::Answer::Feedback(report));
            } else {
                // COMMAND PATH: no parked loop. Submit async, mirroring CompactNow.
                if let Some(f) = app.shell.feedback.as_mut() {
                    f.status = FeedbackStatus::Submitting;
                }
                let ui_tx = app.ui_tx.clone();
                let api: std::sync::Arc<dyn zoid_core::feedback::FeedbackApi> =
                    std::sync::Arc::new(zoid_core::feedback::HttpFeedbackApi::new());
                tokio::spawn(async move {
                    let outcome = report.submit_via(api.as_ref()).await;
                    let _ = ui_tx.send(zoid::agent::AgentUpdate::FeedbackOutcome(outcome)).await; // see Step 3
                });
            }
        }
```

Use the real `FeedbackField`/`FeedbackStatus` import paths (`zoid_tui::state::FeedbackField`, `zoid_tui::state::FeedbackStatus`). The surrounding control flow's `return`/early-exit convention should match the existing `Action` match arms in `main.rs`.

The branch needs two things on `App`:
- `app.feedback_reply: Option<oneshot::Sender<Answer>>` — set when the agent loop parks on a `submit_feedback` tool call (the loop already creates the oneshot at `agent.rs:1245`; the bin stores the `rtx` here when it receives `AgentUpdate::AskUser` for a feedback question). Cleared by `take()` on submit or on `Action::FeedbackAbort`.
- The command path (`None` reply) uses `tokio::spawn` + `AgentUpdate::FeedbackOutcome` (Step 3).

- [ ] **Step 3: Add an `AgentUpdate::FeedbackOutcome` variant to carry the async result back**

The bin's UI channel carries `AgentUpdate` (verified: `CompactNow`'s spawn uses `ui_tx.send(AgentUpdate::CompactionStarted).await`). Add a variant to `AgentUpdate` (`crates/zoid/src/agent.rs:125`):

```rust
    /// A feedback submit finished; the bin updates the overlay's status line.
    FeedbackOutcome(anyhow::Result<zoid_core::feedback::SubmitOutcome>),
```

Handle it in the main UI-receive loop (where `AgentUpdate::CompactionComplete` is handled): on `Ok(SubmitOutcome::Created { url, number })` set `app.shell.feedback.as_mut().unwrap().status = FeedbackStatus::Done(SubmitOutcome::Created { url, number })`, and surface the URL as a `status_hint`. On `Ok(BrowserFallback { url })`, set `Done` and open the browser (check `crates/zoid/src/main.rs` for the companion's existing `open` call; if none exists, use `std::process::Command::new("open").arg(&url)` on macOS / `xdg-open` on Linux / `start` on Windows, or the `open` crate if already a dep). On `Err(e)`, set `FeedbackStatus::Error(e.to_string())`.

**For the tool path:** the loop parks on a `oneshot` and sends `AgentUpdate::AskUser { reply: rtx }`. The bin stores `rtx` on `app.feedback_reply` when it receives that `AgentUpdate` for a `QuestionKind::Feedback` question (detect via the pending `QuestionAsked` event). On `Action::FeedbackSubmit`, the bin builds the `FeedbackReport` + sends `Answer::Feedback(report)` on `app.feedback_reply.take()` (Step 2 above). On `Action::FeedbackAbort`, drop `app.feedback_reply` (the loop's `rrx.await` returns `Err` → "User declined"). No `feedback_pending` stash — the report rides inside the `Answer`.

- [ ] **Step 4: Add `capture_app_diagnostics`**

In `crates/zoid/src/main.rs`, add a helper. Verified `App` fields (`main.rs:1382+`): `session_id: Ulid`, `config: Config` (`provider: String`, `model: String` — both `String`, NOT `Option`), `modes: ModeRegistry` (`modes.active_name()`), `events: EventLog` (`.snapshot()`). There is no `cwd` field on `App`; use `std::env::current_dir()`.

```rust
/// Capture diagnostics from the running `App` for a feedback report.
fn capture_app_diagnostics(app: &App) -> zoid_core::feedback::Diagnostics {
    let recent_error = app.events.snapshot().iter().rev().find_map(|e| match &e.kind {
        zoid_core::event::EventKind::ToolResult { is_error: true, output, .. } => Some(output.clone()),
        _ => None,
    });
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .display()
        .to_string();
    zoid_core::feedback::Diagnostics::capture(
        env!("CARGO_PKG_VERSION").to_string(),
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
        app.session_id.to_string(),
        app.modes.active_name().to_string(),
        app.config.provider.clone(),
        app.config.model.clone(),
        cwd,
        recent_error,
    )
}
```

- [ ] **Step 5: Build the bin**

Run: `cargo build -p zoid`
Expected: clean build. Fix field-name mismatches.

- [ ] **Step 6: Run the full workspace build + tests**

Run: `cargo build && cargo test`
Expected: clean build; all tests pass (including the updated skill/tool/registry/command/event tests).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): wire Command::Feedback dispatch, diagnostics capture, and async submit"
```

---

## Self-Review

**Spec coverage (§-by-§):**
- §4 (`zoid-core::feedback` module): Tasks 1 (data model + rendering) + 2 (`submit_via` + `FeedbackApi` seam). ✓
- §5 (`:feedback` command + overlay): Task 5 (command/state/key routing) + Task 6 (render + palette) + Task 9 (bin dispatch + submit). ✓
- §6 (`submit_feedback` tool + agent loop): Task 4 (tool + registry) + Task 8 (agent loop interception). ✓
- §6.4 (`QuestionKind::Feedback`): Task 3. ✓
- §7 (built-in skill): Task 7. ✓
- §8 (error handling): covered by `submit_via`'s `NoToken`→`BrowserFallback` + `Err` propagation (Task 2) and the `ToolResult { is_error }` paths (Tasks 8, 9). ✓
- §9 (testing): each task has its own tests; §9.5 integration (invalid-kind path) is Task 8 Step 1; the confirm path is exercised by Task 8's `take_feedback_report_and_submit` (with `FakeFeedbackApi` injection, Task 8 Step 4) + Task 2's `FakeFeedbackApi` unit tests. ✓
- §10 (deps): `percent-encoding` (Task 1), `reqwest` + `async-trait` (Task 2), `thiserror` (Task 2). ✓
- §11 (out of scope): no tasks touch it. ✓

**Placeholder scan:** No "TBD"/"TODO"/"implement later". The `Config`/`App` field names referenced in Tasks 8 and 9 have been verified against the actual structs (`Config` at `crates/zoid-core/src/config.rs:28-39`: `provider: String`, `model: String`; `App` at `crates/zoid/src/main.rs:1382+`: `session_id: Ulid`, `modes: ModeRegistry`, `events: EventLog`, `config: Config`; cwd via `std::env::current_dir()`). The `Answer`/oneshot reply mechanism is verified against `agent.rs:1245-1281`. The `AgentUpdate` channel is verified against `CompactNow`'s spawn (`main.rs:4296`). The one remaining implementation-time detail is the exact handle through which `take_feedback_report_and_submit` reaches the stashed `feedback_pending` + injected `FeedbackApi` — this depends on the turn-state threading, which the plan describes as a contract (Task 8 Step 4) rather than guessing the field name.

**Type consistency:** `FeedbackKind`/`Diagnostics`/`FeedbackReport`/`SubmitOutcome` (Task 1) are used unchanged in Tasks 2, 8, 9. `FeedbackApi::create_issue` signature (Task 2) is used by `submit_via` (Task 2) and `HttpFeedbackApi` (Task 2). `QuestionKind::Feedback { kind, title, body }` (Task 3) is constructed in Task 8 and rendered in Task 6. `FeedbackState`/`FeedbackField`/`FeedbackStatus`/`Overlay::Feedback` (Task 5, in `state.rs`) are consumed by Tasks 6 and 9. `Command::Feedback` (Task 5) is consumed by Tasks 6 (palette/preview) and 9 (dispatch). `Action::Feedback*` (Task 5) is consumed by Task 9. `route_feedback_key` (Task 5, in `feedback_view.rs`) is called from `route_key` (Task 5). The built-in skill `feedback` (Task 7) references `submit_feedback` (Task 4) — consistent. `Answer::Feedback(FeedbackReport)` (Task 8) carries the report from the bin (Task 9) back to the loop (Task 8) — the loop stays stateless, no shared `feedback_pending` stash.