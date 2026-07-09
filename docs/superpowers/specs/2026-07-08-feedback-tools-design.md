# Feedback Tools Design

**Date:** 2026-07-08
**Status:** Approved (pending spec review)
**Scope:** User-facing feedback & bug-report submission, across all three agent surfaces (command, tool, skill), unified by a shared submission core.

---

## 1. Goal

Let a user submit feedback or report a bug about zoid to its maintainers, as a GitHub issue on the public `strvmarv/zoid-releases` repo. The system spans all three agent interaction surfaces:

- **Command** (`:feedback`) — the user initiates from the palette.
- **Tool** (`submit_feedback`) — the agent offers to file one during a chat turn; the user confirms/edits before submit.
- **Skill** (`feedback`) — agent instructions for when to offer the tool and how to write a good report.

All three funnel through one shared submission core in `zoid-core`.

## 2. Decisions (from brainstorming)

| Question | Decision |
|---|---|
| Destination of feedback | GitHub issues on `strvmarv/zoid-releases` (the public repo; the source repo `strvmarv/zoid` is private) |
| Auth model | Both: use `$GITHUB_TOKEN` if present to create the issue via API; otherwise fall back to opening a pre-filled `github.com/.../issues/new` URL in the browser |
| Surfaces | All three: command + tool + skill |
| Auto-attached diagnostics | Rich: version, OS/arch, session ID, current mode, model/provider, recent error excerpt, working directory |
| Feedback vs bug entry points | One unified entry point (`:feedback`) with a type selector (bug / feature / general), differentiated by GitHub labels |
| Interaction model | Single-form overlay: type selector + title + description together |
| Architecture | Approach A: a shared `zoid-core::feedback` module holding report + diagnostics + `submit()`; the command (TUI overlay), the tool (interactive agent tool), and the skill all call into it |
| Skill shipping | Ship as a third entry in `SkillRegistry::builtin()` so it ships with the binary and is available to every mode globally — no superpowers import needed |

## 3. Architecture Overview

```
                 ┌─────────────────────────────────────────┐
                 │        zoid-core::feedback               │
                 │  ┌─────────────┐   ┌──────────────────┐  │
   command ──►   │  │ FeedbackReport │  │  Diagnostics     │  │
   tool ──►      │  │ (kind,title,  │  │ (version,OS,arch,│  │
   skill ──►     │  │  body)        │  │  session,mode,   │  │
                 │  │              │  │  model,cwd,error)│  │
                 │  └──────┬───────┘   └──────────────────┘  │
                 │         │  submit()                       │
                 │  ┌──────▼─────────────────────────────┐  │
                 │  │ GitHubIssueClient                  │  │
                 │  │ - api.github.com/.../zoid-releases │  │
                 │  │ - token? create issue : build URL  │  │
                 │  └────────────────────────────────────┘  │
                 └─────────────────────────────────────────┘
```

**Data flow:**

1. A `FeedbackReport` is constructed (by the TUI form or by the agent via the tool).
2. A `Diagnostics` snapshot is attached automatically at submit time.
3. `submit()` renders the report into a GitHub issue body (markdown with a diagnostics block) and either POSTs to the GitHub API (token present) or returns a pre-filled `github.com/strvmarv/zoid-releases/issues/new` URL (no token).

**Three entry points into the same core:**

- **Command `:feedback`** (TUI): opens a single-form overlay (type selector + title + description), builds the report, submits. Fully user-driven.
- **Tool `submit_feedback`** (agent): an `Interactive`-kind tool — the agent proposes a report; the existing `ask_user` park-and-await path surfaces it for the user to confirm/edit before submit. Agent-driven, human-confirmed.
- **Skill `feedback`** (agent instructions): a built-in skill telling the agent *when* to offer the tool and *how* to write a good report. No code logic — just instructions.

## 4. Component: `zoid-core::feedback` module

The shared core that both the command and tool call into. It owns the data model and the GitHub submission logic — no TUI deps, fully unit-testable.

**File:** `crates/zoid-core/src/feedback.rs`

### 4.1 Data model

```rust
/// What kind of feedback this is. Maps to a GitHub label on zoid-releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackKind {
    Bug,            // label: "bug"
    FeatureRequest, // label: "enhancement"
    General,        // label: "feedback"
}

impl FeedbackKind {
    pub fn label(&self) -> &'static str;   // "bug" | "enhancement" | "feedback"
    pub fn all() -> [FeedbackKind; 3];      // [Bug, FeatureRequest, General]
    pub fn display(&self) -> &'static str; // "Bug" | "Feature Request" | "General"
    /// Parse the tool-call / JSON string form ("bug"|"feature"|"general").
    pub fn parse(s: &str) -> Option<Self>;
}
```

```rust
/// Auto-collected environment context, attached to every report.
/// All fields are derivable without user input at submit time.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostics {
    pub version: String,              // env!("CARGO_PKG_VERSION")
    pub os: String,                   // std::env::consts::OS
    pub arch: String,                 // std::env::consts::ARCH
    pub session_id: String,            // active session ULID
    pub mode: String,                  // active mode name
    pub provider: String,              // active provider key
    pub model: String,                 // active model id
    pub cwd: String,                   // working directory (display path)
    pub recent_error: Option<String>, // last error event excerpt, capped ~500 chars
}

impl Diagnostics {
    /// Snapshot from the current app/session state. The caller (TUI or tool)
    /// passes in the values it has on hand; this is a plain constructor, not
    /// a global read, so it stays testable.
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
    ) -> Self;
}
```

```rust
/// A complete feedback submission, pre-submission.
#[derive(Debug, Clone)]
pub struct FeedbackReport {
    pub kind: FeedbackKind,
    pub title: String,
    pub body: String,          // the user's prose description
    pub diagnostics: Diagnostics,
}

/// The outcome of a submit attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Issue created via the API. Carries the issue URL and number.
    Created { url: String, number: u64 },
    /// No token available; caller should open this pre-filled URL in a browser.
    BrowserFallback { url: String },
}
```

### 4.2 Rendering & submission

```rust
impl FeedbackReport {
    /// Render the report into a GitHub issue body (markdown): the user's prose
    /// first, then a <details><summary>Environment</summary> block holding the
    /// Diagnostics.
    pub fn to_issue_body(&self) -> String;

    /// Render the title (prefixed with kind), e.g. "[Bug] <title>".
    pub fn to_issue_title(&self) -> String;

    /// Build the pre-filled `github.com/.../issues/new` URL (query-encoded title
    /// + body) for the browser-fallback path.
    pub fn to_browser_url(&self) -> String;

    /// Submit to the `strvmarv/zoid-releases` repo. Uses `$GITHUB_TOKEN` if
    /// present (POST to the issues API, returns Created); otherwise returns
    /// BrowserFallback with the pre-filled URL. Never panics — errors become
    /// an `Err` the caller surfaces in the UI.
    pub async fn submit(&self) -> anyhow::Result<SubmitOutcome>;
}
```

### 4.3 Key design points

- **`Diagnostics::capture` is an explicit constructor, not a global read.** Callers pass in the values they have. This keeps the module pure and testable — tests build `Diagnostics` directly.
- **`recent_error` is optional and capped** (~500 chars) — only attached if the session log has a recent error event. Avoids dumping the whole log.
- **`submit()` is async** (it does HTTP via `reqwest`, already a workspace dep). The body is `to_issue_body()` — markdown with the user's prose first, then a `<details><summary>Environment</summary>` block holding `Diagnostics`.
- **`to_browser_url()`** builds `https://github.com/strvmarv/zoid-releases/issues/new?title=...&body=...` with percent-encoding. Adds the `percent-encoding` crate (a tiny, no-dep transitive of `url`/`reqwest` already in the tree) or reuses an existing encoder.
- **Labels:** the API path sends `{"labels": [kind.label()]}`. The browser path can't pre-set labels (GitHub's new-issue URL doesn't support them), so the browser body includes a line like `> Label: bug` and the user/maintainer sets it. Documented limitation.
- **Constants:** `const REPO: &str = "strvmarv/zoid-releases";` — same public repo as `update.rs`'s `RELEASES_REPO`. (Factoring `RELEASES_REPO` into a shared constant is an optional minor refactor; not required for this feature.)
- **`SubmitOutcome` is public** so both the command and tool can react identically (show URL in TUI, return URL as tool result).
- **User-Agent:** the GitHub API client sets `User-Agent: zoid-feedback/<version>` (mirroring `github_fetch.rs`'s `zoid-wizard/<version>` and `update.rs`'s `zoid-updater/<version>`).

## 5. Component: `:feedback` command + overlay

### 5.1 Command parsing

**File:** `crates/zoid-tui/src/command.rs`

Add a flat command (no sub-namespace), mirroring `:config` and `:compact`:

```rust
pub enum Command {
    // ... existing variants ...
    Feedback,
}

// in parse_command:
"feedback" => Command::Feedback,
```

Accepts `:feedback` and `feedback` (with/without leading colon, trimmed). No arguments — the form handles everything.

### 5.2 TUI state

**File:** `crates/zoid-tui/src/state.rs`

Add `Overlay::Feedback` to the `Overlay` enum (alongside `Palette`, `Config`, `Mcp`, …), and a `FeedbackState` struct mirroring `QuestionState`'s shape:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    Objects,
    Verbs,
    Sessions,
    Config,
    ProviderSwitch,
    Mcp,
    Feedback, // NEW
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackState {
    /// Which field has focus: kind picker, title, or body.
    pub focus: FeedbackField,
    pub kind: FeedbackKind,       // re-exported from zoid-core::feedback
    pub kind_selected: usize,     // 0..3, cycles Bug/Feature/General
    pub title: String,
    pub body: String,             // multi-line, edited via ratatui-textarea
    pub status: FeedbackStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackField { Kind, Title, Body }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackStatus {
    Idle,
    Submitting,
    Done(zoid_core::feedback::SubmitOutcome),  // Created{url} or BrowserFallback{url}
    Error(String),
}
```

### 5.3 Key routing

**File:** `crates/zoid-tui/src/feedback_view.rs` (new, mirroring `question.rs`)

`route_feedback_key(state, key) -> Action`:

- **Tab / Shift+Tab**: cycle focus across `Kind → Title → Body` (and back).
- **Up / Down**: cycle the kind when `Kind` is focused; otherwise no-op (textarea handles its own vertical movement).
- **Char / Backspace**: edit the focused title or body.
- **Enter**:
  - In `Title` field → move focus to `Body` (NOT submit — avoids accidental submit).
  - In `Body` field → insert newline (multi-line editing). Submit is Ctrl+Enter.
- **Ctrl+Enter** (or **Alt+Enter**): submit from the body field.
- **Esc**: abort (close overlay, return to chat).

Reuse existing `Action` variants where possible; add `Action::Feedback*` variants only as needed (e.g. `FeedbackMoveFocus`, `FeedbackSubmit`, `FeedbackAbort`, `FeedbackChar`, `FeedbackBackspace`, `FeedbackCycleKind`).

### 5.4 Rendering

A centered modal (like the config overlay), titled **"Submit feedback"**. Three stacked regions:

1. **Type** — a horizontal pick-list row: `[Bug] [Feature Request] [General]`, the selected kind highlighted.
2. **Title** — a single-line input (bordered box), placeholder "Short summary".
3. **Description** — a `ratatui-textarea` (multi-line, bordered), placeholder "Describe the issue or suggestion".

A footer hint line: `Tab next · Ctrl+Enter submit · Esc cancel`.

Status rendering:
- `Submitting` → disable input, show "Submitting…".
- `Done(Created { url })` → show "Created issue #N: <url>".
- `Done(BrowserFallback { url })` → show "No token — opened your browser at <url> (finish submitting there)".
- `Error(msg)` → show the error message with a retry option.

### 5.5 Submission wiring (the bin)

**File:** `crates/zoid/src/main.rs` (command dispatch)

- `Command::Feedback` → open `Overlay::Feedback`, initialize `FeedbackState { focus: Kind, kind: Bug, kind_selected: 0, title: "", body: "", status: Idle }`. Capture a `Diagnostics` snapshot from the current app state and hold it on the app state for use at submit time:
  - `version` = `env!("CARGO_PKG_VERSION")`
  - `os` = `std::env::consts::OS`
  - `arch` = `std::env::consts::ARCH`
  - `session_id` = active session ULID
  - `mode` = active mode name
  - `provider` / `model` = active provider key / model id
  - `cwd` = working directory display path
  - `recent_error` = scan recent events for the last error event, cap at ~500 chars
- On submit (Ctrl+Enter): build `FeedbackReport { kind, title, body, diagnostics }`, call `report.submit().await` on the bin's existing async event path (like the updater). Set `FeedbackStatus` to the outcome. On `Done`, surface the URL in the overlay and as a transcript line.

### 5.6 Palette integration

The palette (Ctrl+P) Pick list gains a "Submit feedback" row that resolves to `Command::Feedback`, so it's discoverable both via `:feedback` and the palette.

## 6. Component: `submit_feedback` tool (agent-initiated)

The tool lets the **agent** offer to file feedback during a chat turn, with the user confirming/editing before anything is submitted.

### 6.1 Tool definition

**File:** `crates/zoid-tools/src/feedback.rs` (new)

```rust
pub struct SubmitFeedback;

impl Tool for SubmitFeedback {
    fn name(&self) -> &str { "submit_feedback" }
    fn kind(&self) -> ToolKind { ToolKind::Interactive }   // parks like ask_user
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "submit_feedback".into(),
            description: "Offer to submit user feedback or a bug report to the zoid \
                maintainers (GitHub issues on strvmarv/zoid-releases). The user MUST \
                confirm/edit before it is submitted — never file silently. Use when \
                the user asks to report a bug or give feedback, or when a reproducible \
                error occurs and the user agrees to report it.".into(),
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
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("submit_feedback must be handled by the agent loop")
    }
}
```

### 6.2 Registry wiring

**File:** `crates/zoid-tools/src/lib.rs`

Add `Box::new(feedback::SubmitFeedback)` to both `registry()` and `registry_with_kill()`. Add `pub mod feedback;` to the module list.

**Subagents:** `submit_feedback` is `Interactive`, and `Interactive` tools are already filtered out of a subagent's tool set (`subagent.rs` filters `ToolKind::Interactive`). A headless subagent can't confirm a feedback form, so this exclusion is automatic — no extra code.

### 6.3 Agent loop interception

**File:** `crates/zoid/src/agent.rs`

Extend the `ToolKind::Interactive` match arm (currently `tc.name == "ask_user" || tc.name == "apply_mode_mapping"`) to also match `tc.name == "submit_feedback"`. When intercepted:

1. **Parse** `kind`/`title`/`body` from the tool call args. Validate:
   - `kind` must parse via `FeedbackKind::parse` ("bug"|"feature"|"general"). Invalid → emit a `ToolResult` error (`"submit_feedback: invalid kind '...'. Must be bug|feature|general."`) and `continue` (mirrors `apply_mode_mapping`'s error path).
   - `title` and `body` must be non-empty strings. Empty → same error-path `ToolResult` and `continue`.
2. **Emit** a new `EventKind::FeedbackProposed { id, kind, title, body }` — a new event variant that renders an inline card (like `QuestionAsked`) pre-filled with the agent's proposal. `id` = the tool call id (for reply correlation, like `QuestionAsked`).
3. **Park** on the same oneshot reply path as `ask_user`. The TUI opens the `Feedback` overlay (the same one the command uses), seeded with the agent's proposed values — the user can edit everything and hit Ctrl+Enter, or hit Esc to decline.
4. **Reply handling** — the user's reply carries either the edited report fields or a "declined" signal:
   - **Confirmed** → the loop builds a `FeedbackReport` from the edited fields + a `Diagnostics` snapshot (captured the same way the command captures it), calls `report.submit().await`, and feeds the `SubmitOutcome` back to the model as the tool result:
     - `Created { url, number }` → `"Created issue #N: <url>"`
     - `BrowserFallback { url }` → `"No GitHub token available — opened your browser at <url>. The user must finish submitting there."`
   - **Declined** → `"User declined to submit feedback."` (so the model knows not to retry).

### 6.4 Event variant

**File:** `crates/zoid-core/src/event.rs`

Add `FeedbackProposed { id: String, kind: String, title: String, body: String }` alongside `QuestionAsked`. The card renders inline like a question card; focusing it opens the `Feedback` overlay seeded with the proposed values (the same overlay the command uses). `kind` is stored as the string form ("bug"|"feature"|"general") to avoid coupling the event type to `FeedbackKind`; the TUI parses it on render.

### 6.5 Why reuse the same overlay

The `:feedback` command and the tool share the `FeedbackState` + `Overlay::Feedback` rendering from §5. The only difference is how they're *triggered* (command vs. tool-call park) and that the tool path seeds the form from the agent's proposal. One overlay, two entry points.

## 7. Component: `feedback` skill (built-in)

The skill ships **with the binary** as a third entry in `SkillRegistry::builtin()`, not as a superpowers-import file. It's available to **every mode** (Chat and all imported modes) as a global, with no import step and no dependency on the superpowers bundle.

### 7.1 Mechanism

**File:** `crates/zoid-core/src/skill.rs`

Add a third `Skill` to `builtin()`:

```rust
pub fn builtin() -> Self {
    Self::new(vec![
        Skill { /* spike-plan, unchanged */ },
        Skill { /* spike-implement, unchanged */ },
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

Define the body as a `const FEEDBACK_SKILL_BODY: &str` in the same file, keeping `builtin()` readable.

### 7.2 Why this works

- `build_registry` (the session skill registry, in `skill_import.rs`) seeds from `SkillRegistry::builtin()` then `push_unique`s imported skills — so `feedback` is always present, and an imported skill named `feedback` can't shadow it (first-wins protects built-ins).
- `effective_skills(global, active)` merges the global registry (which includes `feedback`) into every mode's skill set. So Chat mode and any imported mode (superpowers, etc.) all get `feedback` automatically.
- The skill appears in the `invoke_skill` menu (the system-prompt menu line `- feedback: ...`) for all sessions, and the agent can call `invoke_skill` with `name: "feedback"`.
- No file shipping needed: built-in skills live as Rust string literals compiled into the binary, exactly like the two spike skills already do.

### 7.3 Skill body content

```markdown
# Submitting Feedback & Bug Reports

zoid can file feedback or bug reports to the maintainers as GitHub issues on
`strvmarv/zoid-releases`. The `submit_feedback` tool proposes a report; the
user **always confirms and can edit** before it is submitted — never file
silently.

## When to Offer

Offer the tool when:
- The user explicitly asks to "report a bug", "give feedback", or "file an issue".
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
- **title**: One line, specific. Bad: "it crashed". Good: "Crash on `:config`
  open when no provider is configured".
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
```

### 7.4 Test impact

The existing `builtin_has_both_spike_skills_that_chain`, `menu_renders_one_line_per_skill`, `all_exposes_every_skill_in_order`, and `builtin_skills_have_no_base_dir` tests assert exactly the two-skill list. They'll be updated to expect three skills (the two spikes plus `feedback`), with new assertions that `feedback` exists, has a non-empty body referencing `submit_feedback`, and has `base_dir: None`.

## 8. Error Handling

| Scenario | Behavior |
|---|---|
| `$GITHUB_TOKEN` unset | `submit()` returns `BrowserFallback { url }`; UI opens the pre-filled browser URL. Not an error. |
| GitHub API returns 401/403 (bad token) | `submit()` returns `Err`; UI shows the error and offers retry. The model sees the error as a tool result. |
| GitHub API returns 5xx / network failure | `submit()` returns `Err`; same as above. |
| Rate-limited (429) | `submit()` returns `Err` with a message suggesting `$GITHUB_TOKEN`; same as above. |
| Invalid `kind` from agent tool call | Agent loop emits a `ToolResult` error and `continue`s (model recovers). |
| Empty `title`/`body` from agent tool call | Same — `ToolResult` error, `continue`. |
| User declines (tool path) | Tool result = `"User declined to submit feedback."`; model moves on. |
| User cancels overlay (Esc) | Overlay closes; no submit. For the command, nothing happens. For the tool, treated as declined. |

`submit()` never panics — all failures are `Err` the caller surfaces.

## 9. Testing Strategy

### 9.1 Unit tests (`zoid-core::feedback`)

- `FeedbackKind::label`/`display`/`parse` round-trips for all three variants; `parse` returns `None` for unknown strings.
- `Diagnostics::capture` constructs from explicit args (no global read).
- `to_issue_body` produces markdown with the user's prose first, then a `<details>` block containing every diagnostics field.
- `to_issue_title` prefixes with the kind (e.g. `[Bug] <title>`).
- `to_browser_url` percent-encodes title and body into `https://github.com/strvmarv/zoid-releases/issues/new?title=...&body=...`.
- `submit` with a mocked HTTP client:
  - Token present + 201 response → `Created { url, number }` parsed from the JSON response.
  - Token absent → `BrowserFallback { url }` (no HTTP call made).
  - Token present + error status → `Err`.
- `recent_error` capping: a >500-char error is truncated to ~500 chars.

### 9.2 Unit tests (`zoid-tools::feedback`)

- `SubmitFeedback::spec` advertises the `submit_feedback` name, `Interactive` kind, and a valid object schema with `kind`/`title`/`body` properties and `required`.
- `run` returns the "must be handled by the agent loop" error.
- Registry tests: `submit_feedback` is in `registry()` and `registry_with_kill()`; it is `Interactive` (excluded from subagents — the existing `assembled_tools_exclude_interactive_ask_user` test pattern extends to assert `submit_feedback` is also excluded).

### 9.3 Unit tests (`zoid-tui`)

- `parse_command(":feedback")` and `parse_command("feedback")` → `Command::Feedback`.
- `FeedbackState` focus cycling (Kind → Title → Body → Kind) via Tab/Shift+Tab.
- `route_feedback_key` maps keys to the right `Action`s for each focused field.
- Enter in Title moves to Body (not submit); Ctrl+Enter submits; Esc aborts.

### 9.4 Unit tests (`zoid-core::skill`)

- `builtin()` now returns three skills; `feedback` is present with a non-empty body containing `submit_feedback`, and `base_dir: None`.
- `menu` renders three lines including `- feedback: `.
- `push_unique` still protects `feedback` from being shadowed by an import of the same name.

### 9.5 Integration tests (`zoid` bin)

- `:feedback` command opens the `Feedback` overlay (smoke test of the command dispatch).
- Tool interception: a `submit_feedback` tool call with valid args emits `FeedbackProposed`, parks, and on a "confirm" reply produces a `ToolResult` referencing the outcome. (Uses a mocked/no-network `submit`.)
- Tool interception: a `submit_feedback` tool call with an invalid `kind` emits a `ToolResult` error and `continue`s without parking.

## 10. Dependencies

- `reqwest` — already a workspace dep (used by `github_fetch.rs`, `update.rs`). No new dep for HTTP.
- `percent-encoding` — needed for `to_browser_url`. Check if already transitively present (via `reqwest`/`url`); if so, add it directly to `zoid-core`'s `Cargo.toml`. If not present, add it (tiny, no transitive deps).
- `serde` / `serde_json` — already workspace deps, used for the `FeedbackKind` enum and GitHub API JSON.
- No new heavy deps. No new crates in the workspace.

## 11. Out of Scope (YAGNI)

- **Threading/routing feedback to a backend other than GitHub issues.** GitHub issues on `zoid-releases` is the only destination.
- **Attaching logs, screenshots, or session transcripts.** Only the `recent_error` excerpt is attached. Full log/session export is a separate feature.
- **User accounts or persistent identity.** `$GITHUB_TOKEN` is the only auth; no zoid account system.
- **Searching/listing past feedback.** One-way submission only; no UI for viewing past issues.
- **Anonymous submission without a browser.** GitHub requires auth to create issues via the API; without a token, the browser hand-off is the only path.
- **Browser-label pre-fill.** GitHub's new-issue URL doesn't support pre-setting labels; the body includes a `> Label:` line instead. Documented limitation, not fixable here.
- **Custom feedback templates beyond the three kinds.** Bug / feature / general is the fixed set.