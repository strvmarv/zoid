# Tool Error Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a structured `ErrorKind` enum to tool outputs and the event system, categorize every tool's error paths, detect DDG backend outages, and add a CWD-deleted pre-check with recovery instructions — so the model can distinguish "retry me" from "your input was wrong" from "the backend is down."

**Architecture:** `ErrorKind` lives in `zoid-core` (because `zoid-core` does not depend on `zoid-tools`, but `EventKind::ToolResult` needs the type). `zoid-tools` imports it from `zoid-core`. The enum propagates: `ToolOutput` → `EventKind::ToolResult` → `ChatMsg::ToolResult` → `Message` (with a `[error: <kind>]` prefix rendered in `agent.rs`'s `map_msg`). The CWD-deleted pre-check is a loop-level guard in `agent.rs`. DDG outage detection is a heuristic in `zoid-web`.

**Tech Stack:** Rust workspace (14 crates), `serde` for event persistence, `scraper` for DDG HTML parsing, `reqwest` for HTTP.

## Global Constraints

- Do not modify any file under `crates/*/src/` except as explicitly listed in each task's Files section.
- `ErrorKind` must derive `Serialize, Deserialize` (it's carried inside the serde-persisted `EventKind`).
- The `error_kind` field on `EventKind::ToolResult` must have `#[serde(default)]` so existing SQLite sessions deserialize without error.
- `ErrorKind` lives in `zoid-core` (`crates/zoid-core/src/event.rs` or a new module), NOT in `zoid-tools` — `zoid-core` does not depend on `zoid-tools`.
- Do not hand-edit `.github/workflows/release.yml`.
- Source of truth for scope: `docs/superpowers/specs/2026-08-06-tool-error-diagnostics-design.md`.
- `err()` defaults to `Some(Internal)` — existing uncategorized call sites still compile.
- grep/glob "no matches" is NOT an error — returns empty result set, no `ErrorKind`.
- shell nonzero exit is NOT an error — already has `[exit N]` signal; only spawn failures get an `ErrorKind`.
- Emitting tools (`exit_worktree`, `enter_worktree`, etc.) are exempt from the CWD-deleted pre-check.

---

### Task 1: Define `ErrorKind` in `zoid-core` and update `ToolOutput` in `zoid-tools`

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (add `ErrorKind` enum + `as_str()`)
- Modify: `crates/zoid-core/src/lib.rs` (re-export `ErrorKind`)
- Modify: `crates/zoid-tools/src/lib.rs` (import `ErrorKind`, add `error_kind` field + `err_kind()` constructor)

**Interfaces:**
- Produces: `zoid_core::ErrorKind` enum (8 variants), `ErrorKind::as_str() -> &'static str`, `ToolOutput::error_kind: Option<ErrorKind>`, `ToolOutput::err_kind(kind, text)` constructor. Later tasks use these.

- [ ] **Step 1: Define the `ErrorKind` enum in `zoid-core/src/event.rs`**

Add after the `use` lines and before `BranchId`, or at the end of the type definitions (before `EventKind`). The enum must derive `Serialize, Deserialize`:

```rust
/// Machine-readable error category for tool failures. Propagated from
/// `ToolOutput` through `EventKind::ToolResult` to the projection/UI. The
/// model sees a rendered `[error: <kind>]` prefix in the tool-result text;
/// the loop and UI get the enum directly for future retry logic and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorKind {
    /// External service down or returned an error page (web_search DDG
    /// outage, web_fetch non-2xx or unparseable 2xx body).
    BackendUnavailable,
    /// Operation exceeded a time limit (network connect timeout).
    Timeout,
    /// File, path, or resource does not exist.
    NotFound,
    /// Bad arguments from the model (missing arg, wrong type, empty query,
    /// limit < 1, offset past end, bad URL scheme).
    InvalidInput,
    /// OS-level permission failure (write to read-only path, dir read denied).
    PermissionDenied,
    /// Ambiguous or precondition failure (edit: `old_string` ambiguous or not
    /// found).
    Conflict,
    /// The working directory was deleted out from under the agent. Recovery:
    /// call exit_worktree (if in a worktree) or navigate to an existing
    /// directory.
    CwdDeleted,
    /// Unexpected internal error (serialization failure, spawn failure,
    /// anything that doesn't fit above).
    Internal,
}

impl ErrorKind {
    /// Canonical snake_case string for the `[error: <kind>]` prefix.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::BackendUnavailable => "backend_unavailable",
            ErrorKind::Timeout => "timeout",
            ErrorKind::NotFound => "not_found",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::Conflict => "conflict",
            ErrorKind::CwdDeleted => "cwd_deleted",
            ErrorKind::Internal => "internal",
        }
    }
}
```

- [ ] **Step 2: Re-export `ErrorKind` from `zoid-core`'s lib**

In `crates/zoid-core/src/lib.rs`, add to the `pub use` or `pub mod` section:

```rust
pub use event::ErrorKind;
```

- [ ] **Step 3: Add `error_kind` field and `err_kind()` to `ToolOutput` in `zoid-tools/src/lib.rs`**

Import `ErrorKind` from `zoid-core`:

```rust
use zoid_core::ErrorKind;
```

Update the `ToolOutput` struct:

```rust
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub diff: Option<diff::FileDiff>,
    pub error_kind: Option<ErrorKind>,
}
```

Update the constructors:

```rust
impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            diff: None,
            error_kind: None,
        }
    }
    pub fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            diff: None,
            error_kind: Some(ErrorKind::Internal),
        }
    }
    pub fn err_kind(kind: ErrorKind, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            diff: None,
            error_kind: Some(kind),
        }
    }
    pub fn with_diff(mut self, diff: diff::FileDiff) -> Self {
        self.diff = Some(diff);
        self
    }
}
```

Also update `str_arg` to return `InvalidInput` instead of the `Internal`
default (this fixes the missing-arg error kind for ALL tools at once —
read, write, edit, shell, ls, glob, grep, web_search, web_fetch,
subagent_diff all use `str_arg`):

```rust
pub(crate) fn str_arg(args: &Value, key: &str) -> Result<String, ToolOutput> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolOutput::err_kind(ErrorKind::InvalidInput, format!("missing or non-string argument: {key}")))
}
```

- [ ] **Step 4: Write unit tests for `ErrorKind` and `ToolOutput`**

Add to the test module in `crates/zoid-tools/src/lib.rs`:

```rust
#[test]
fn error_kind_as_str_returns_snake_case() {
    assert_eq!(ErrorKind::BackendUnavailable.as_str(), "backend_unavailable");
    assert_eq!(ErrorKind::Timeout.as_str(), "timeout");
    assert_eq!(ErrorKind::NotFound.as_str(), "not_found");
    assert_eq!(ErrorKind::InvalidInput.as_str(), "invalid_input");
    assert_eq!(ErrorKind::PermissionDenied.as_str(), "permission_denied");
    assert_eq!(ErrorKind::Conflict.as_str(), "conflict");
    assert_eq!(ErrorKind::CwdDeleted.as_str(), "cwd_deleted");
    assert_eq!(ErrorKind::Internal.as_str(), "internal");
}

#[test]
fn err_kind_sets_error_flag_and_kind() {
    let out = ToolOutput::err_kind(ErrorKind::NotFound, "file missing");
    assert!(out.is_error);
    assert_eq!(out.error_kind, Some(ErrorKind::NotFound));
    assert_eq!(out.text, "file missing");
}

#[test]
fn err_defaults_to_internal() {
    let out = ToolOutput::err("something broke");
    assert!(out.is_error);
    assert_eq!(out.error_kind, Some(ErrorKind::Internal));
}

#[test]
fn ok_has_no_error_kind() {
    let out = ToolOutput::ok("success");
    assert!(!out.is_error);
    assert_eq!(out.error_kind, None);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid-core -p zoid-tools --lib`
Expected: all tests pass, including the 4 new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/lib.rs crates/zoid-tools/src/lib.rs
git commit -m "feat(error-kind): define ErrorKind enum and add error_kind to ToolOutput"
```

---

### Task 2: Propagate `error_kind` through `EventKind::ToolResult` and `ChatMsg::ToolResult`

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (add `#[serde(default)] error_kind` to `ToolResult` variant)
- Modify: `crates/zoid-core/src/projection.rs` (add `error_kind` to `ChatMsg::ToolResult`, update construction sites)
- Modify: `crates/zoid-core/src/compaction.rs` (update construction sites)
- Modify: `crates/zoid-core/src/eviction.rs` (update construction sites)
- Modify: `crates/zoid-core/src/reassert.rs` (update construction sites)
- Modify: `crates/zoid-core/src/context.rs` (update construction sites)
- Modify: `crates/zoid-core/src/zoom.rs` (update `ChatMsg::ToolResult` construction sites)
- Modify: `crates/zoid-core/src/store.rs` (update match arm)

**Interfaces:**
- Consumes: `ErrorKind` from Task 1.
- Produces: `EventKind::ToolResult { error_kind: Option<ErrorKind> }` and `ChatMsg::ToolResult { error_kind: Option<ErrorKind> }`. Later tasks (agent loop, TUI) construct these.

- [ ] **Step 1: Add `error_kind` to `EventKind::ToolResult` with `#[serde(default)]`**

In `crates/zoid-core/src/event.rs`, update the `ToolResult` variant:

```rust
ToolResult {
    id: String,
    name: String,
    output: String,
    is_error: bool,
    #[serde(default)]
    error_kind: Option<ErrorKind>,
},
```

- [ ] **Step 2: Write the serde backward-compatibility test**

In `crates/zoid-core/src/event.rs` test module, add:

```rust
#[test]
fn tool_result_deserializes_without_error_kind() {
    // Legacy JSON from before error_kind was added — must still load.
    let json = r#"{"ToolResult":{"id":"tc1","name":"read","output":"ok","is_error":false}}"#;
    let kind: EventKind = serde_json::from_str(json).unwrap();
    match kind {
        EventKind::ToolResult { id, error_kind, .. } => {
            assert_eq!(id, "tc1");
            assert_eq!(error_kind, None, "legacy ToolResult must deserialize error_kind as None");
        }
        _ => panic!("expected ToolResult"),
    }
}
```

- [ ] **Step 3: Add `error_kind` to `ChatMsg::ToolResult` in `projection.rs`**

Update the `ToolResult` variant of `ChatMsg`:

```rust
ToolResult {
    id: String,
    name: String,
    output: String,
    is_error: bool,
    error_kind: Option<crate::ErrorKind>,
    compacted: bool,
    ts: i64,
},
```

Then update every construction site in `projection.rs` that builds a `ChatMsg::ToolResult`. There are ~3 non-test sites (lines ~254, ~436) and several test sites. Add `error_kind: None,` (or `error_kind: *error_kind,` where the source `EventKind::ToolResult` provides it) to each. For the projection's fold from `EventKind::ToolResult` → `ChatMsg::ToolResult`, pass through the `error_kind` from the event:

```rust
out.push(ChatMsg::ToolResult {
    id: id.clone(),
    name: name.clone(),
    output,
    is_error: *is_error,
    error_kind: *error_kind,
    compacted: was_compacted,
    ts: e.ts,
});
```

For test construction sites, add `error_kind: None,`.

- [ ] **Step 4: Update construction sites in `compaction.rs`, `eviction.rs`, `reassert.rs`, `context.rs`**

Every `EventKind::ToolResult { ... }` construction in these files needs `error_kind: None,` added (for test/non-error sites) or `error_kind: out.error_kind,` (for the agent-loop sites — but those are in `agent.rs`, covered in Task 5). In `zoid-core`, all `ToolResult` constructions are in test code or event-replay code — add `error_kind: None,` to each.

Use `grep -rn "EventKind::ToolResult {" crates/zoid-core/src/` to find all sites in zoid-core (including compaction, eviction, reassert, context, zoom, event tests). Add `error_kind: None,` to each construction. Also run `grep -rn "ChatMsg::ToolResult {" crates/zoid-core/src/` to find all `ChatMsg::ToolResult` constructions (including projection, zoom) — add `error_kind: None,` (or pass-through `*error_kind` for the projection fold site) to each.

- [ ] **Step 5: Update `store.rs` match arm**

In `crates/zoid-core/src/store.rs`, the `fts_content` function matches `ToolResult { output, name, .. }` — the `..` already ignores extra fields, so no change needed. Verify with:

```bash
grep -n "ToolResult" crates/zoid-core/src/store.rs
```

If any match does NOT use `..`, add `error_kind: _,`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid-core`
Expected: all tests pass, including the new serde backward-compat test. If any construction site was missed, the compiler will error with "missing field `error_kind`" — find and fix it.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs crates/zoid-core/src/compaction.rs crates/zoid-core/src/eviction.rs crates/zoid-core/src/reassert.rs crates/zoid-core/src/context.rs crates/zoid-core/src/store.rs
git commit -m "feat(error-kind): propagate error_kind through EventKind::ToolResult and ChatMsg::ToolResult"
```

---

### Task 3: Update `agent.rs` and `main.rs` construction sites + prefix rendering

**Files:**
- Modify: `crates/zoid/src/agent.rs` (all `EventKind::ToolResult` constructions + `map_msg` prefix rendering)
- Modify: `crates/zoid/src/main.rs` (all `EventKind::ToolResult` and `ChatMsg::ToolResult` constructions)
- Modify: `crates/zoid/src/subagent.rs` (test construction sites)
- Modify: `crates/zoid/src/eventlog.rs` (match arm — verify `..` catches it)
- Modify: `crates/zoid/src/spawn_subagent.rs` (verify no construction sites, or update)

**Interfaces:**
- Consumes: `ErrorKind` from Task 1, `EventKind::ToolResult { error_kind }` from Task 2.
- Produces: `[error: <kind>]` prefix on model-facing tool-result text. Later tasks (CWD pre-check, tool categorization) rely on this rendering existing.

- [ ] **Step 1: Update all `EventKind::ToolResult` constructions in `agent.rs`**

Find every construction:

```bash
grep -n "EventKind::ToolResult {" crates/zoid/src/agent.rs
```

For each, add `error_kind: out.error_kind,` if it has access to a `ToolOutput` named `out`, or `error_kind: None,` for synthetic/test constructions (e.g., the `[skipped: turn aborted]` and `[killed: hard-stop]` short-circuits).

- [ ] **Step 2: Add `[error: <kind>]` prefix rendering in `map_msg`**

In `crates/zoid/src/agent.rs`, find the `map_msg` function (search for `ChatMsg::ToolResult { id, name, output, .. }`). The current code binds `id`, `name`, `output` by value (moved from `ChatMsg`). Update it to also bind `is_error` and `error_kind`, and prepend the prefix when both are set:

```rust
ChatMsg::ToolResult {
    id, name, output, is_error, error_kind, ..
} => {
    let text = if is_error && error_kind.is_some() {
        format!("[error: {}] {}", error_kind.unwrap().as_str(), output)
    } else {
        output
    };
    Message::tool_with_call_id(name, id, &text)
},
```

This moves `output` (no clone needed — `ChatMsg` is consumed by value in
`map_msg`), uses `is_error` directly (it's a `bool` moved out of the
struct, not a reference), and `error_kind.unwrap()` is safe inside the
`is_some()` guard.

- [ ] **Step 3: Update all `EventKind::ToolResult` and `ChatMsg::ToolResult` constructions in `main.rs`**

```bash
grep -n "EventKind::ToolResult {\|ChatMsg::ToolResult {" crates/zoid/src/main.rs
```

Add `error_kind: None,` to each construction site (these are test/utility sites).

- [ ] **Step 4: Update test construction sites in `subagent.rs`**

```bash
grep -n "EventKind::ToolResult {" crates/zoid/src/subagent.rs
```

Add `error_kind: None,` to each.

- [ ] **Step 5: Verify `eventlog.rs` and `spawn_subagent.rs`**

```bash
grep -n "ToolResult" crates/zoid/src/eventlog.rs crates/zoid/src/spawn_subagent.rs
```

`eventlog.rs` matches `ToolResult { output, .. }` — `..` handles the new field. `spawn_subagent.rs` should have no `ToolResult` constructions. If either has a construction without `..`, add `error_kind: None,`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid --lib`
Expected: all tests pass. Compiler errors about missing `error_kind` indicate missed construction sites — find and fix them.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/src/subagent.rs crates/zoid/src/eventlog.rs
git commit -m "feat(error-kind): update agent.rs construction sites and render [error: kind] prefix in map_msg"
```

---

### Task 4: Update `zoid-tui` construction sites

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (all `ChatMsg::ToolResult` constructions + pattern matches)
- Modify: `crates/zoid-tui/src/objects.rs` (construction/match sites)
- Modify: `crates/zoid-tui/tests/chat_snapshot.rs` (test construction sites)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (test construction sites)
- Modify: `crates/zoid-tui/examples/scenes/mod.rs` (construction sites)

**Interfaces:**
- Consumes: `ChatMsg::ToolResult { error_kind }` from Task 2.
- Produces: TUI can read `error_kind` for future icon/color differentiation (no visual change required in this task — just make it compile).

- [ ] **Step 1: Find all construction sites**

```bash
grep -rn "ChatMsg::ToolResult {" crates/zoid-tui/src/ crates/zoid-tui/tests/ crates/zoid-tui/examples/
```

- [ ] **Step 2: Add `error_kind: None,` to every construction site**

Pattern matches that use `..` (e.g., `ChatMsg::ToolResult { id, name, is_error, .. }`) do NOT need changes. Only construction sites (with `id: ..., name: ...,` field assignments) need `error_kind: None,` added.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tui`
Expected: all tests pass. Compiler errors indicate missed construction sites.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/
git commit -m "feat(error-kind): update zoid-tui ChatMsg::ToolResult construction sites"
```

---

### Task 5: Update integration test construction sites

**Files:**
- Modify: `crates/zoid/tests/*.rs` (any file with `EventKind::ToolResult` constructions)
- Modify: `crates/zoid-core/tests/*.rs` (any file with `EventKind::ToolResult` constructions)
- Modify: `crates/zoid-testkit/src/lib.rs` (two `EventKind::ToolResult` constructions at lines ~55 and ~94)

**Interfaces:**
- Consumes: `EventKind::ToolResult { error_kind }` from Task 2.
- Produces: nothing — just makes integration tests and testkit compile.

- [ ] **Step 1: Find all construction sites**

```bash
grep -rn "EventKind::ToolResult {" crates/zoid/tests/ crates/zoid-core/tests/ crates/zoid-testkit/src/ 2>/dev/null
```

- [ ] **Step 2: Add `error_kind: None,` to each construction**

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace`
Expected: all tests pass across the workspace.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/tests/ crates/zoid-core/tests/
git commit -m "test(error-kind): update integration test ToolResult construction sites"
```

---

### Task 6: Add DDG error-page detection in `zoid-web`

**Files:**
- Modify: `crates/zoid-web/src/search.rs` (add `is_ddg_error_page`, update `search_with_client`)
- Create: `crates/zoid-web/tests/fixtures/ddg_error_page.html` (test fixture)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `search_with_client` returns distinct error messages for "backend unavailable" vs "no results found" — the `web_search` tool (Task 9) string-matches these to assign `ErrorKind`.

- [ ] **Step 1: Add `is_ddg_error_page` function**

In `crates/zoid-web/src/search.rs`, add:

```rust
/// Heuristic check for DDG error/diagnostic pages. When `parse_ddg_html`
/// returns zero results, this distinguishes "DDG is broken" from "your query
/// matched nothing." Conservative: only known error markers trigger it.
pub(crate) fn is_ddg_error_page(body: &str) -> bool {
    body.contains("error-lite@duckduckgo.com")
        || body.contains("error@duckduckgo.com")
        || body.contains("If this error persists")
}
```

- [ ] **Step 2: Update `search_with_client` to use it**

In `search_with_client`, replace the empty-results branch:

```rust
let body = resp.text().await?;
let results = parse_ddg_html(&body);
if results.is_empty() {
    if is_ddg_error_page(&body) {
        return Err(anyhow!("DuckDuckGo backend unavailable (error page returned, no result links parsed)"));
    }
    return Err(anyhow!("no results found for: {q}"));
}
Ok(results)
```

- [ ] **Step 3: Create the error-page test fixture**

Create `crates/zoid-web/tests/fixtures/ddg_error_page.html`:

```html
<!DOCTYPE html>
<html><head><title>DuckDuckGo</title></head>
<body>
<div class="content">
<p>If this error persists, please contact us at error-lite@duckduckgo.com</p>
</div>
</body></html>
```

- [ ] **Step 4: Write tests for `is_ddg_error_page`**

In `crates/zoid-web/src/search.rs` test module, add:

```rust
#[test]
fn is_ddg_error_page_detects_error_markers() {
    assert!(is_ddg_error_page("contact error-lite@duckduckgo.com for help"));
    assert!(is_ddg_error_page("If this error persists, try again"));
    assert!(is_ddg_error_page("error@duckduckgo.com"));
}

#[test]
fn is_ddg_error_page_false_for_normal_html() {
    assert!(!is_ddg_error_page("<html><body>normal page</body></html>"));
    assert!(!is_ddg_error_page(""));
}

#[test]
fn is_ddg_error_page_false_for_genuine_no_results() {
    // A genuine "no results" page has no error markers.
    let html = r#"<html><body><div class="no-results">No results found</div></body></html>"#;
    assert!(!is_ddg_error_page(html));
}
```

- [ ] **Step 5: Verify `search_with_client` integration by direct test**

`search_with_client` POSTs to the hardcoded `DDG_URL` constant, so it can't
be redirected to a local mock. Verify the integration by testing
`parse_ddg_html` + `is_ddg_error_page` together — the same logic
`search_with_client` uses in its empty-results branch:

```rust
#[test]
fn search_error_page_detected_as_backend_unavailable() {
    let error_html = r#"<html><body><p>If this error persists, contact error-lite@duckduckgo.com</p></body></html>"#;
    let results = parse_ddg_html(error_html);
    assert!(results.is_empty(), "error page has no result links");
    assert!(is_ddg_error_page(error_html), "error page detected");
}

#[test]
fn search_genuine_no_results_not_detected_as_error() {
    let no_results_html = r#"<html><body><div class="no-results">No results found</div></body></html>"#;
    let results = parse_ddg_html(no_results_html);
    assert!(results.is_empty(), "genuine no-results has no result links");
    assert!(!is_ddg_error_page(no_results_html), "genuine no-results not flagged as error");
}
```

- [ ] **Step 6: Add 2xx empty-extraction check to `zoid_web::fetch`**

In `crates/zoid-web/src/lib.rs`, in the `fetch` function, after `extract::extract_markdown` returns, check for empty content:

```rust
let (title, markdown) = extract::extract_markdown(&body, url)?;
if markdown.trim().is_empty() {
    return Err(anyhow!("page returned no extractable content"));
}
let total_chars = markdown.chars().count();
```

- [ ] **Step 7: Write a test for the empty-extraction check**

In `crates/zoid-web/src/lib.rs` test module, add:

```rust
#[tokio::test]
async fn fetch_2xx_empty_extraction_returns_err() {
    // A page with no article-like content — readability extracts nothing.
    let empty_html = r#"<!DOCTYPE html><html><head><title>Empty</title></head><body><nav>nav</nav></body></html>"#;
    let addr = spawn_html_server(empty_html).await;
    let r = fetch(&format!("http://{addr}"), 0, 100_000).await;
    assert!(r.is_err());
    let e = r.unwrap_err().to_string();
    assert!(e.contains("no extractable content"), "got: {e}");
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p zoid-web`
Expected: all tests pass, including the new detection tests.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-web/src/search.rs crates/zoid-web/src/lib.rs crates/zoid-web/tests/fixtures/ddg_error_page.html
git commit -m "feat(web): detect DDG error pages and empty-extraction 2xx responses"
```

---

### Task 7: Categorize `read`, `ls`, `write` error paths (io::ErrorKind inspection)

**Files:**
- Modify: `crates/zoid-tools/src/read.rs`
- Modify: `crates/zoid-tools/src/ls.rs`
- Modify: `crates/zoid-tools/src/write.rs`

**Interfaces:**
- Consumes: `ErrorKind`, `err_kind()` from Task 1.
- Produces: these tools return categorized errors. No interface change for other tasks.

- [ ] **Step 1: Categorize `read` error paths**

In `crates/zoid-tools/src/read.rs`, the current error path at line ~39 is:

```rust
Err(e) => return ToolOutput::err(format!("read({path}): {e}")),
```

This single `read_to_string` error needs to be split by `e.kind()`:

```rust
Err(e) => {
    let kind = match e.kind() {
        std::io::ErrorKind::NotFound => ErrorKind::NotFound,
        std::io::ErrorKind::InvalidData => ErrorKind::InvalidInput,
        _ => ErrorKind::Internal,
    };
    return ToolOutput::err_kind(kind, format!("read({path}): {e}"));
}
```

Add `use zoid_core::ErrorKind;` at the top if not already imported.

- [ ] **Step 2: Update `read` tests to assert `error_kind`**

In the existing tests for missing file, non-UTF8, and other errors, add assertions:

For `missing_file_is_error`:
```rust
assert_eq!(out.error_kind, Some(ErrorKind::NotFound));
```

For `non_utf8_is_error`:
```rust
assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));
```

For `limit_zero_is_error` (this is a validation error, already a separate path):
```rust
assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));
```

- [ ] **Step 3: Categorize `ls` error paths**

In `crates/zoid-tools/src/ls.rs`, the `read_dir` error needs `e.kind()` inspection:

```rust
Err(e) => {
    let kind = match e.kind() {
        std::io::ErrorKind::NotFound => ErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        _ => ErrorKind::Internal,
    };
    return ToolOutput::err_kind(kind, format!("ls({path}): {e}"));
}
```

- [ ] **Step 4: Categorize `write` error paths**

In `crates/zoid-tools/src/write.rs`, the `std::fs::write` error at line ~46:

```rust
Err(e) => {
    let kind = match e.kind() {
        std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        _ => ErrorKind::Internal,
    };
    ToolOutput::err_kind(kind, format!("write({path}): {e}"))
}
```

- [ ] **Step 5: Update `ls` and `write` tests to assert `error_kind`**

Add `assert_eq!(out.error_kind, Some(ErrorKind::X))` to existing error tests where applicable. For `ls` tests that don't currently test error paths, add a basic missing-dir test if one doesn't exist:

```rust
#[test]
fn missing_dir_is_not_found() {
    let out = Ls.run(&json!({"path": "/nonexistent/path/that/does/not/exist"}), std::path::Path::new("."));
    assert!(out.is_error);
    assert_eq!(out.error_kind, Some(ErrorKind::NotFound));
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p zoid-tools --lib read ls write`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/read.rs crates/zoid-tools/src/ls.rs crates/zoid-tools/src/write.rs
git commit -m "feat(error-kind): categorize read, ls, write error paths with io::ErrorKind inspection"
```

---

### Task 8: Categorize `edit`, `shell`, `git_context` error paths

**Files:**
- Modify: `crates/zoid-tools/src/edit.rs`
- Modify: `crates/zoid-tools/src/shell.rs`
- Modify: `crates/zoid-tools/src/git_context.rs`

**Interfaces:**
- Consumes: `ErrorKind`, `err_kind()` from Task 1.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Categorize `edit` error paths**

In `crates/zoid-tools/src/edit.rs`, update each `ToolOutput::err(...)` call.
Note: `str_arg` was already fixed to `InvalidInput` in Task 1 Step 3 —
missing-arg errors already return the correct kind. Focus on the remaining
paths:

- `old_string` not found (in `apply_edit`): the function returns
  `Result<(), String>`. The caller in `run()` maps this to `ToolOutput::err(...)`.
  Update the caller:

```rust
Err(msg) => {
    let kind = if msg.contains("not found") {
        ErrorKind::NotFound
    } else if msg.contains("ambiguous") {
        ErrorKind::Conflict
    } else {
        ErrorKind::Internal
    };
    return ToolOutput::err_kind(kind, format!("edit({path}) edit #{}: {msg}", i + 1));
}
```

- Empty edits list: `ToolOutput::err_kind(ErrorKind::InvalidInput, format!("edit({path}): empty edits list"))`
- Other IO error (read/write file): inspect `e.kind()` like Task 7, default to `Internal`.

- [ ] **Step 2: Categorize `shell` error paths**

In `crates/zoid-tools/src/shell.rs`:
- Spawn failure: `ToolOutput::err_kind(ErrorKind::Internal, format!("shell({command}): {e}"))`
- Missing command arg: already handled by `str_arg` (fixed in Task 1).

- [ ] **Step 3: Categorize `git_context` error paths**

In `crates/zoid-tools/src/git_context.rs`:
- All errors: `ToolOutput::err_kind(ErrorKind::Internal, format!("git_context: {e}"))`

- [ ] **Step 4: Update tests to assert `error_kind`**

For `edit` tests:
- `absent_match_is_error`: `assert_eq!(out.error_kind, Some(ErrorKind::NotFound));`
- `ambiguous_match_is_error`: `assert_eq!(out.error_kind, Some(ErrorKind::Conflict));`
- `empty_edits_list_is_error`: `assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));`
- Missing arg test: `assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));`

For `shell` tests:
- `missing_command_is_error`: `assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));`

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid-tools --lib edit shell git_context`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/src/edit.rs crates/zoid-tools/src/shell.rs crates/zoid-tools/src/git_context.rs
git commit -m "feat(error-kind): categorize edit, shell, git_context error paths"
```

---

### Task 9: Categorize `web_search` and `web_fetch` error paths

**Files:**
- Modify: `crates/zoid-tools/src/web_search.rs`
- Modify: `crates/zoid-tools/src/web_fetch.rs`

**Interfaces:**
- Consumes: `ErrorKind` from Task 1, distinct error messages from `zoid-web` (Task 6).
- Produces: `web_search` returns `BackendUnavailable` for DDG outages, `NotFound` for genuine empty results. `web_fetch` returns `BackendUnavailable` for non-2xx/empty-extraction.

- [ ] **Step 1: Categorize `web_search` error paths**

In `crates/zoid-tools/src/web_search.rs`, update `run_async`:

```rust
fn run_async<'a>(&'a self, args: &'a Value, _cwd: &'a Path) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
    Box::pin(async move {
        let query = match crate::str_arg(args, "query") {
            Ok(q) => q,
            Err(e) => return e,  // str_arg already returns InvalidInput
        };
        match zoid_web::search(&query).await {
            Ok(results) => ToolOutput::ok(format_results(&results)),
            Err(e) => {
                let msg = e.to_string();
                let kind = if msg.contains("backend unavailable") {
                    ErrorKind::BackendUnavailable
                } else if msg.contains("no results found") {
                    ErrorKind::NotFound
                } else if msg.contains("empty query") {
                    ErrorKind::InvalidInput
                } else if msg.contains("timeout") || msg.contains("timed out") {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::BackendUnavailable
                };
                ToolOutput::err_kind(kind, format!("web_search failed: {msg}"))
            }
        }
    })
}
```

- [ ] **Step 2: Categorize `web_fetch` error paths**

In `crates/zoid-tools/src/web_fetch.rs`, update `run_async`:

```rust
fn run_async<'a>(&'a self, args: &'a Value, _cwd: &'a Path) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
    Box::pin(async move {
        let url = match crate::str_arg(args, "url") {
            Ok(u) => u,
            Err(e) => return e,  // str_arg already returns InvalidInput
        };
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20_000) as usize;
        match zoid_web::fetch(&url, offset, limit).await {
            Ok(r) => ToolOutput::ok(format_fetch(&r)),
            Err(e) => {
                let msg = e.to_string();
                let kind = if msg.contains("HTTP ") {
                    ErrorKind::BackendUnavailable
                } else if msg.contains("no extractable content") {
                    ErrorKind::BackendUnavailable
                } else if msg.contains("http/https only") {
                    ErrorKind::InvalidInput
                } else if msg.contains("past end") {
                    ErrorKind::InvalidInput
                } else if msg.contains("timeout") || msg.contains("timed out") {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::BackendUnavailable
                };
                ToolOutput::err_kind(kind, format!("web_fetch failed: {msg}"))
            }
        }
    })
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tools --lib web_search web_fetch`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/src/web_search.rs crates/zoid-tools/src/web_fetch.rs
git commit -m "feat(error-kind): categorize web_search and web_fetch error paths"
```

---

### Task 10: Categorize `subagent_diff` and chat-only Emitting tool error paths

**Files:**
- Modify: `crates/zoid-tools/src/subagent_diff.rs`
- Modify: `crates/zoid/src/agent.rs` (Emitting tool error paths: `enter_worktree`, `exit_worktree`, `dispatch_subagent`, `recall`, `show`, `schedule_wake`, `cancel_wake`)
- Modify: `crates/zoid/src/invoke_skill.rs` (two `ToolOutput::err` calls)

**Interfaces:**
- Consumes: `ErrorKind` from Task 1.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Categorize `subagent_diff` error paths**

In `crates/zoid-tools/src/subagent_diff.rs`, update the two `ToolOutput::err(...)` calls:

```rust
// history not found
return ToolOutput::err_kind(ErrorKind::NotFound, format!("subagent_diff: history not found for {id}"));

// git rev-parse failed
return ToolOutput::err_kind(ErrorKind::Internal, format!("subagent_diff: git rev-parse failed: {e}"));
```

- [ ] **Step 2: Categorize Emitting tool errors in `agent.rs`**

Each Emitting tool arm in `agent.rs` constructs `EventKind::ToolResult` with
`is_error: true` for validation/failure cases. Task 3 set these to
`error_kind: None`. Now update each to the correct `ErrorKind`. Find each
by searching for `is_error: true` in the Emitting arms and set
`error_kind` per this exact mapping:

- `enter_worktree` — `'name' is required` (search for `"enter_worktree: 'name' is required"`): `error_kind: Some(ErrorKind::InvalidInput)`
- `enter_worktree` — worktree switch failed (search for `"worktree switch failed"`): `error_kind: Some(ErrorKind::Internal)`
- `exit_worktree` — error from `compute_worktree_switch` (search for the `other =>` arm in `exit_worktree`): `error_kind: Some(ErrorKind::Conflict)` if the message contains "not in a worktree" or "subagent running", else `error_kind: Some(ErrorKind::Internal)`
- `dispatch_subagent` — `'task' is required` (search for `"dispatch_subagent: 'task' is required"`): `error_kind: Some(ErrorKind::InvalidInput)`
- `dispatch_subagent` — pool/agent/profile failure (search for `is_error: true` in the `dispatch_subagent` arm): `error_kind: Some(ErrorKind::Internal)`
- `recall` — error from recall handler (search for `is_error: true` in the `recall` arm): `error_kind: Some(ErrorKind::InvalidInput)`
- `show` — error from show handler (search for `is_error: true` in the `show` arm): `error_kind: Some(ErrorKind::Internal)`
- `schedule_wake` — validation error (search for `is_error: true` in the `schedule_wake` arm): `error_kind: Some(ErrorKind::InvalidInput)`
- `cancel_wake` — validation error (search for `is_error: true` in the `cancel_wake` arm): `error_kind: Some(ErrorKind::InvalidInput)`

- [ ] **Step 3: Categorize `invoke_skill` error paths**

In `crates/zoid/src/invoke_skill.rs`, there are two `ToolOutput::err(...)`
calls:
- Missing/empty skill name (search for `"invoke_skill: missing or empty"`):
  change to `ToolOutput::err_kind(ErrorKind::InvalidInput, ...)`
- Unknown skill (search for `"invoke_skill: unknown skill"`):
  change to `ToolOutput::err_kind(ErrorKind::NotFound, ...)`

Add `use zoid_core::ErrorKind;` if not already imported.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tools --lib subagent_diff && cargo test -p zoid --lib`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/src/subagent_diff.rs crates/zoid/src/agent.rs
git commit -m "feat(error-kind): categorize subagent_diff and chat-only Emitting tool errors"
```

---

### Task 11: CWD-deleted pre-check in the agent loop

**Files:**
- Modify: `crates/zoid/src/agent.rs` (add `cwd.exists()` check in Local and Network tool-dispatch arms)

**Interfaces:**
- Consumes: `ErrorKind::CwdDeleted` from Task 1, `err_kind()` from Task 1.
- Produces: short-circuit `ToolOutput` with recovery message when CWD is deleted.

- [ ] **Step 1: Add the CWD-deleted pre-check to the Local tool-dispatch arm**

In `crates/zoid/src/agent.rs`, find the Local tool-dispatch arm — it's the
`_ => { // Local tools (the default): run in the working directory` match
arm (search for `// Local tools (the default)`). Place the check as the
**first statement** inside that arm, after the `ToolStarted` UI send and
before `let tools_for_exec = ...`.

**Do NOT place this check above the `match` on `tool_kind`** — it must be
inside the Local arm only, so Emitting tools (the recovery path) are exempt.

```rust
// CWD-deleted pre-check (spec: CWD-deleted detection and recovery).
// Emitting tools are exempt — they're the recovery path.
if !cwd.exists() {
    let in_worktree = cwd.iter().any(|c| c == ".zoid");
    let msg = if in_worktree {
        format!(
            "You are in a worktree — the working directory \"{}\" no longer exists. \
             Call exit_worktree to return to the main checkout.",
            cwd.display()
        )
    } else {
        format!(
            "The working directory \"{}\" no longer exists. \
             Navigate to an existing directory (e.g., the repo root) \
             before running another command.",
            cwd.display()
        )
    };
    let out = ToolOutput::err_kind(ErrorKind::CwdDeleted, msg);
    emit(&session, &mut events, ui, &config.branch, EventKind::ToolResult {
        id: tc.id,
        name: tc.name,
        output: out.text,
        is_error: out.is_error,
        error_kind: out.error_kind,
    }, session_id, now).await?;
    continue;  // skip to the next tool in the batch
}
```

The `in_worktree` check uses `cwd.iter().any(|c| c == ".zoid")` — this
covers the `.zoid/worktrees/<name>` convention used by zoid's worktree
system. Git-linked worktrees (`.git/worktrees/...`) are not covered by
this heuristic; v1 only needs to handle zoid's own worktree paths.

- [ ] **Step 2: Add the same check to the Network tool-dispatch arm**

In the Network arm (search for `ToolKind::Network` or `tools_for_async`), add the same `cwd.exists()` check before the async dispatch. The message and `ToolOutput` construction are identical.

- [ ] **Step 3: Write a test for the CWD-deleted pre-check**

This is an integration-level test. In `agent.rs` test module, or as a standalone test, verify that when `cwd` doesn't exist, the pre-check produces a `CwdDeleted` error. If the agent loop is too complex to unit-test in isolation, write a focused test of the message-construction logic:

```rust
#[test]
fn cwd_deleted_message_contains_exit_worktree_when_in_worktree() {
    let cwd = std::path::PathBuf::from("/repo/.zoid/worktrees/feature-x");
    let in_worktree = cwd.iter().any(|c| c == ".zoid" || c == ".worktrees");
    assert!(in_worktree);
    let msg = format!(
        "You are in a worktree — the working directory \"{}\" no longer exists. \
         Call exit_worktree to return to the main checkout.",
        cwd.display()
    );
    assert!(msg.contains("exit_worktree"));
}

#[test]
fn cwd_deleted_message_does_not_mention_worktree_when_not_in_worktree() {
    let cwd = std::path::PathBuf::from("/home/user/project");
    let in_worktree = cwd.iter().any(|c| c == ".zoid" || c == ".worktrees");
    assert!(!in_worktree);
    let msg = format!(
        "The working directory \"{}\" no longer exists. \
         Navigate to an existing directory (e.g., the repo root) \
         before running another command.",
        cwd.display()
    );
    assert!(!msg.contains("exit_worktree"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid --lib`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(cwd-check): add CWD-deleted pre-check with recovery instructions in agent loop"
```

---

## Definition of done

- `cargo test --workspace` passes (including `zoid-testkit`, `zoid-tui`, and all integration tests).
- `ErrorKind` is defined in `zoid-core`, re-exported, and derives `Serialize, Deserialize`.
- `ToolOutput` has `error_kind: Option<ErrorKind>` with `ok()`, `err()`, `err_kind()` constructors.
- `str_arg` returns `InvalidInput` for missing/non-string arguments (affects all tools using `str_arg`).
- `EventKind::ToolResult` has `#[serde(default)] error_kind: Option<ErrorKind>`.
- Legacy `ToolResult` JSON without `error_kind` deserializes to `None`.
- `ChatMsg::ToolResult` has `error_kind` (including `zoid-core/src/zoom.rs` and `zoid-tui` construction sites).
- `[error: <kind>]` prefix appears in model-facing tool-result text when `is_error && error_kind.is_some()`.
- `is_ddg_error_page` detects DDG error markers; `search_with_client` returns "backend unavailable" for error pages.
- `zoid_web::fetch` returns `Err` for 2xx responses with no extractable content.
- Every tool's error paths return the correct `ErrorKind` per the spec's audit table (including `subagent_diff`, `invoke_skill`, and Emitting tools).
- CWD-deleted pre-check runs before Local and Network tool calls; Emitting tools are exempt.
- Recovery message contains "exit_worktree" when in a worktree (path contains `.zoid`).