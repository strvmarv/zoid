# Tool Error Diagnostics — Design Spec

## Problem

zoid's tools return errors as a boolean `is_error` flag plus free-text. The agent
(model) cannot distinguish "retry me" from "your input was wrong" from "the backend
is down." The motivating case: `web_search` wraps DuckDuckGo's HTML endpoint, and
when DDG returns an error page (HTTP 200 with diagnostic HTML instead of result
links), `parse_ddg_html` extracts zero `.result` elements and `search_with_client`
returns `Err("no results found for: {q}")` — indistinguishable from a genuine empty
result. The model sees "no results found" and moves on, never knowing the search
backend was down.

This is not isolated to `web_search`. Every tool's error path produces a flat string
with no machine-readable category. The agent loop, eviction policy, and UI all see
only `is_error: true` — the same signal for "file not found," "invalid argument,"
and "backend unavailable."

## Goal

Add a structured `ErrorKind` to `ToolOutput` so that:

1. The **model** sees actionable error text (`[error: backend_unavailable] … try
   again later`) and can choose to retry, fix its input, or abandon.
2. The **agent loop** gets a typed enum it can use for future retry logic, eviction
   policy, and telemetry.
3. The **UI** can display error severity/category visually (not just red text).
4. **All tools** are audited — every error path is categorized, not just
   `web_search`.

## Non-goals

- No automatic retry logic in the agent loop (the model decides whether to retry
  based on the error text; the enum is a future hook for programmatic retry).
- No changes to `ToolKind`, the tool trait signature, or the tool registry.
- No new tools.
- No changes to the shell tool's nonzero-exit handling — it already has a rich
  signal (`[exit N]` + stdout/stderr); only its spawn-failure path gets an
  `ErrorKind`.
- No guarding against self-deletion (e.g., `shell` running `rm -rf` on the CWD).
  The agent is trusted not to delete its own working directory; the pre-check
  catches the aftermath and provides recovery, not prevention of the deletion
  itself.

## CWD-deleted detection and recovery

A recurring failure mode: the agent's working directory is deleted out from
under it. Every subsequent tool call fails with a confusing OS error (`No such
file or directory`) that sends the model into a diagnostic spiral — it tries
`ls`, `read`, `shell` to investigate, all of which fail the same way, wasting
tokens and turns.

### Worktree-flow audit (where does CWD deletion happen?)

A review of the worktree lifecycle identified the paths where the working
directory can be deleted:

**Safe paths (no issue — already hardened):**
- **`exit_worktree` (Chat agent):** `compute_worktree_switch` in `main.rs`
  computes the new CWD (the main checkout root) *before* calling
  `remove_worktree`, and updates `cwd_for_exec` to the new path. The worktree
  directory is removed only after the CWD is repointed. This was an explicit
  fix (labeled "WT-2" in the code: "computed BEFORE any removal, so tooling
  never points at a deleted dir"). No hardening needed here.
- **Subagent with `worktree: true` completes/fails:** The `WorktreeGuard` is
  consumed (`into_kept_branch` on success, `drop` on failure) *after*
  `run_subagent` returns — the subagent is no longer running tool calls when
  its worktree is removed. The parent's CWD was never the subagent's worktree,
  so the parent is unaffected. No hardening needed here.
- **`enter_worktree` while already in a worktree:** `compute_worktree_switch`
  returns an error ("already in a worktree — exit first"). Already guarded.

**The actual risk — `worktree: false` subagents sharing the parent's CWD:**
- A subagent dispatched with `worktree: false` (the default when the task
  doesn't request isolation) inherits the parent's `cwd_for_exec` as its
  working directory. If the subagent runs `shell rm -rf .` or a similar
  destructive command, it deletes the parent's CWD. The parent's subsequent
  tool calls all fail. Sibling subagents (also sharing the CWD) are likewise
  broken.
- This is the primary real-world cause of CWD deletion: not a worktree-flow
  bug, but a subagent running a destructive shell command in a shared CWD.
- The non-goal (no self-deletion guard) applies to the Chat agent itself, but
  the subagent case is worth noting: a `worktree: false` subagent has the
  *same* power to delete the CWD as the parent, and the parent has no defense.
  The CWD-deleted pre-check is the safety net — it catches the aftermath and
  gives the parent agent recovery instructions (`exit_worktree` if in a
  worktree, or navigate to an existing directory).

### Design

**Pre-check in the agent loop:** Before dispatching any tool call (both Local
and Network arms), the agent loop checks `cwd.exists()`. If the CWD no longer
exists, it short-circuits: instead of running the tool, it returns a
`ToolOutput::err_kind(ErrorKind::CwdDeleted, ...)` with a recovery message.

The check is a single `Path::exists()` syscall — negligible overhead per tool
call.

**New ErrorKind variant:**

```rust
/// The working directory was deleted out from under the agent. Recovery:
/// call exit_worktree (if in a worktree) or navigate to an existing directory.
CwdDeleted,
```

**Recovery message format:**

```
[error: cwd_deleted] The working directory "{cwd}" no longer exists. If you
are in a worktree, call exit_worktree to return to the main checkout. If you
deleted the directory intentionally, navigate to an existing directory first
(e.g., cd to the repo root before running another command).
```

The message is intentionally prescriptive — it tells the model exactly what to
do (call `exit_worktree`) rather than leaving it to diagnose the problem. The
most common causes are (1) a `worktree: false` subagent running a destructive
shell command in the shared CWD, and (2) a worktree being removed by an
external process. `exit_worktree` is the primary recovery path when in a
worktree; the fallback ("navigate to an existing directory") covers non-worktree
scenarios.

**Where the check lives:** In the agent loop (`agent.rs`), in both the Local
and Network tool-dispatch arms, before the tool is executed. The check is not
in the tools themselves — it's a loop-level guard, not a per-tool concern,
because every tool that uses `cwd` is affected and the check only needs to
happen once per tool call.

**Interaction with the `ToolOutput`/`ErrorKind` changes:** The CWD-deleted
check produces a `ToolOutput` with `ErrorKind::CwdDeleted` before the tool is
ever called, so the tool's own error paths are never reached. The
`EventKind::ToolResult` for a CWD-deleted short-circuit carries
`error_kind: Some(CwdDeleted)` like any other error.

### Testing

- Agent loop test: when `cwd` is deleted before a tool call, the tool is not
  executed and the `ToolResult` has `error_kind: CwdDeleted` with the
  recovery message.
- The recovery message contains "exit_worktree" (so the model can discover the
  recovery action from the text alone).

## Architecture

Three layers, changed bottom-up:

### Layer 1: `zoid-tools` — `ToolOutput` + `ErrorKind`

Add `ErrorKind` enum and `error_kind` field to `ToolOutput`:

```rust
/// Machine-readable error category for tool failures. Propagated from
/// `ToolOutput` through `EventKind::ToolResult` to the projection/UI. The
/// model sees a rendered `[error: <kind>]` prefix in the tool-result text;
/// the loop and UI get the enum directly for future retry logic and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// found, write: path exists and can't overwrite).
    Conflict,
    /// The working directory was deleted out from under the agent. Recovery:
    /// call exit_worktree (if in a worktree) or navigate to an existing
    /// directory.
    CwdDeleted,
    /// Unexpected internal error (serialization failure, spawn failure,
    /// anything that doesn't fit above).
    Internal,
}
```

`ToolOutput` gains `error_kind: Option<ErrorKind>`:

```rust
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub diff: Option<diff::FileDiff>,
    pub error_kind: Option<ErrorKind>,
}
```

Constructors:

- `ok(text)` — `error_kind: None`, `is_error: false` (unchanged behavior).
- `err(text)` — `error_kind: Some(Internal)`, `is_error: true` (default for
  existing call sites that haven't been categorized yet — safe fallback).
- `err_kind(kind, text)` — `error_kind: Some(kind)`, `is_error: true`. Used by
  all categorized error paths.
- `with_diff(diff)` — unchanged, chains on an `ok` output.

The `err()` default of `Internal` is intentional: any call site not yet migrated
to `err_kind()` still compiles and produces a categorized error (just a generic
one). Migration replaces `err("...")` with `err_kind(ErrorKind::X, "...")` one
tool at a time.

### Layer 2: `zoid-core` — Event propagation

`EventKind::ToolResult` gains `error_kind: Option<ErrorKind>`:

```rust
ToolResult {
    id: String,
    name: String,
    output: String,
    is_error: bool,
    error_kind: Option<ErrorKind>,  // new
    // ... existing fields ...
}
```

The agent loop, when constructing a `ToolResult` event from a `ToolOutput`:

```rust
EventKind::ToolResult {
    error_kind: out.error_kind,
    // ...
}
```

The **model-facing text** gets the `[error: <kind>]` prefix rendered at the point
where the tool result is converted to the provider message (in the context/request
builder, not in the tool itself). This keeps the prefix consistent across all tools
without each tool having to format it. The rendering:

```
[error: backend_unavailable] web_search failed: DuckDuckGo returned an error page
[error: not_found] read(/foo.rs): file does not exist
[error: invalid_input] read: limit must be >= 1
[error: conflict] edit(/foo.rs) edit #1: `old_string` not found
```

The `ErrorKind` name is snake_case in the prefix (the `Debug` impl or a `as_str()`
method on `ErrorKind` provides the canonical string).

`ChatMsg::ToolResult` in the projection layer gains `error_kind: Option<ErrorKind>`
for UI display. The UI can use this for icon/color differentiation.

### Layer 3: `zoid-web` — Outage detection

#### `search.rs` — DDG error-page detection

Current code (the bug):

```rust
let body = resp.text().await?;
let results = parse_ddg_html(&body);
if results.is_empty() {
    return Err(anyhow!("no results found for: {q}"));
}
Ok(results)
```

New code:

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

`is_ddg_error_page` checks for known DDG error/diagnostic markers in the HTML body.
The check is heuristic (DDG doesn't document its error pages), based on observed
patterns:

- Presence of `error-lite@duckduckgo.com` or `error@duckduckgo.com` in the body.
- Presence of DDG anomaly/diagnostic text patterns (e.g., "If this error persists").
- Absence of any `.result` CSS class divs (already true since `results.is_empty()`)
  combined with the presence of non-page-chrome text (indicating an error message
  rather than a genuine "no results" page, which still has the normal DDG page
  structure with a "No results" message).

The heuristic is intentionally conservative: false positives (classifying a genuine
empty-results page as `BackendUnavailable`) cause the model to retry unnecessarily,
which is harmless. False negatives (classifying an error page as `NotFound`) are the
current bug — the heuristic should catch the known patterns and be updated as new
ones are observed.

The `search_with_client` function's return type stays `Result<Vec<SearchResult>>`.
The `web_search` tool in `zoid-tools` maps the error:

- Error message contains "backend unavailable" → `ErrorKind::BackendUnavailable`
- Error message contains "no results found" → `ErrorKind::NotFound`
- Error message contains "empty query" → `ErrorKind::InvalidInput`
- Network/timeout error → `ErrorKind::Timeout` or `BackendUnavailable`

**Alternative considered:** returning a typed error from `zoid_web::search` instead
of string-matching. This is cleaner but requires changing the `zoid_web` public API
(returning a custom error enum instead of `anyhow::Result`). The string-matching
approach keeps `zoid_web` unchanged and puts the categorization in the tool layer
where `ErrorKind` lives. The trade-off is acceptable for now; a future refactor can
make `zoid_web` return typed errors.

**Decision: string-matching in the tool layer.** The `zoid_web` crate stays a pure
leaf with `anyhow::Result`; the `zoid-tools` layer categorizes. This keeps the
change small and avoids coupling `zoid_web` to `ErrorKind`.

#### `fetch.rs` — Already mostly correct

`web_fetch` already handles non-2xx with a clear error (`HTTP {status}: {snippet}`).
The `zoid_web::fetch` function returns `Err` for non-2xx, so the tool layer maps
that to `BackendUnavailable`. For 2xx responses where `extract_markdown` returns
empty/garbage, the current code returns `Ok` with empty content — the tool should
check for this and return `BackendUnavailable` when the extracted content is empty
on a 2xx response (the page returned something but it wasn't parseable as content).

### Tool-by-tool error audit

Each tool's error paths are categorized. Here is the complete mapping:

| Tool | Error path | ErrorKind | Model-facing text |
|------|-----------|-----------|-------------------|
| **read** | missing file | NotFound | `read({path}): file does not exist` |
| **read** | non-UTF8 file | InvalidInput | `read({path}): file is not valid UTF-8` |
| **read** | limit < 1 | InvalidInput | `read: limit must be >= 1` |
| **read** | offset past end | InvalidInput | `read({path}): offset {offset} past end (total {total} lines)` |
| **read** | other IO error | Internal | `read({path}): {e}` |
| **write** | missing path/content arg | InvalidInput | `missing or non-string argument: {key}` |
| **write** | OS write failure (permission) | PermissionDenied | `write({path}): {e}` |
| **write** | OS write failure (other) | Internal | `write({path}): {e}` |
| **edit** | missing arg | InvalidInput | `missing or non-string argument: {key}` |
| **edit** | old_string not found | NotFound | `edit({path}) edit #{i}: \`old_string\` not found` |
| **edit** | old_string ambiguous | Conflict | `edit({path}) edit #{i}: \`old_string\` is ambiguous ({count} matches)` |
| **edit** | empty edits list | InvalidInput | `edit({path}): empty edits list` |
| **edit** | other IO error | Internal | `edit({path}): {e}` |
| **grep** | no matches | N/A (not an error — returns empty result set) | — |
| **glob** | no matches | N/A (not an error — returns empty result set) | — |
| **ls** | dir doesn't exist | NotFound | `ls({path}): directory does not exist` |
| **ls** | permission denied | PermissionDenied | `ls({path}): permission denied` |
| **ls** | other IO error | Internal | `ls({path}): {e}` |
| **shell** | nonzero exit | N/A (already has `[exit N]` signal) | — |
| **shell** | spawn failure | Internal | `shell({command}): {e}` |
| **shell** | missing command arg | InvalidInput | `missing or non-string argument: command` |
| **git_context** | git failure | Internal | `git_context: {e}` |
| **web_search** | DDG error page (0 results, error markers) | BackendUnavailable | `web_search failed: DuckDuckGo backend unavailable (error page returned)` |
| **web_search** | genuine 0 results | NotFound | `web_search failed: no results found for: {q}` |
| **web_search** | empty query | InvalidInput | `web_search failed: empty query` |
| **web_search** | network/connect timeout | Timeout | `web_search failed: {e}` (reqwest timeout error) |
| **web_search** | other network error | BackendUnavailable | `web_search failed: {e}` |
| **web_fetch** | HTTP non-2xx | BackendUnavailable | `web_fetch failed: HTTP {status}: {snippet}` |
| **web_fetch** | connect timeout | Timeout | `web_fetch failed: {e}` (reqwest timeout error) |
| **web_fetch** | 2xx but empty extraction | BackendUnavailable | `web_fetch failed: page returned no extractable content` |
| **web_fetch** | bad URL scheme | InvalidInput | `web_fetch failed: web_fetch supports http/https only (got {scheme})` |
| **web_fetch** | offset past end | InvalidInput | `web_fetch failed: offset {offset} past end (total {total})` |
| **web_fetch** | other network error | BackendUnavailable | `web_fetch failed: {e}` |
| **ask_user** | (interactive — no error paths) | — | — |
| **update_tasks** | (emitting — no error paths) | — | — |
| **submit_feedback** | (emitting — no error paths) | — | — |

### Testing

#### Unit tests

- `ErrorKind` enum: `as_str()` returns canonical snake_case for each variant.
- `ToolOutput::err_kind()` sets `is_error: true` and `error_kind: Some(kind)`.
- `ToolOutput::err()` defaults to `ErrorKind::Internal`.
- `ToolOutput::ok()` has `error_kind: None`.
- Each tool: existing error tests verify the new `error_kind` field in addition to
  `is_error` and text.

#### `zoid-web` tests

- `is_ddg_error_page`: returns `true` for a fixture containing
  `error-lite@duckduckgo.com`; returns `false` for a normal DDG results page and
  for a genuine "no results" page.
- `search_with_client`: when the mock server returns an error page, the error
  message contains "backend unavailable" (not "no results found").
- `search_with_client`: when the mock server returns a genuine empty-results page,
  the error message contains "no results found" (not "backend unavailable").
- `fetch`: when the mock server returns a 2xx with no extractable content, the
  tool returns `BackendUnavailable`.

#### CWD-deleted detection tests

- Agent loop: when `cwd` is deleted before a Local tool call, the tool is not
  executed and the `ToolResult` has `error_kind: CwdDeleted` with the recovery
  message containing "exit_worktree".
- Agent loop: same for the Network tool-dispatch arm.
- The recovery message is prescriptive (names `exit_worktree` explicitly).

#### Integration tests

- The `[error: <kind>]` prefix appears in the model-facing tool result text (tested
  at the context/request builder level).

### Migration path

1. Add `ErrorKind` enum (including `CwdDeleted`) and update `ToolOutput` (constructors, field).
2. Update `EventKind::ToolResult` and `ChatMsg::ToolResult` with `error_kind`.
3. Add `[error: <kind>]` prefix rendering in the agent loop's `ChatMsg` → `Message` conversion.
4. Add CWD-deleted pre-check in the agent loop (Local and Network arms).
5. Add `is_ddg_error_page` to `zoid-web/src/search.rs`.
6. Add 2xx-empty-extraction check to `zoid-web/src/lib.rs` (`fetch`).
7. Audit and categorize every tool's error paths (one tool file at a time).
8. Update all existing tests to assert `error_kind` where applicable.
9. Add new tests for DDG error-page detection, prefix rendering, and CWD-deleted detection.

### Files touched

- `crates/zoid-tools/src/lib.rs` — `ErrorKind` enum, `ToolOutput` changes
- `crates/zoid-tools/src/read.rs` — categorize error paths
- `crates/zoid-tools/src/write.rs` — categorize error paths
- `crates/zoid-tools/src/edit.rs` — categorize error paths
- `crates/zoid-tools/src/ls.rs` — categorize error paths
- `crates/zoid-tools/src/shell.rs` — categorize spawn failure
- `crates/zoid-tools/src/git_context.rs` — categorize error paths
- `crates/zoid-tools/src/web_search.rs` — categorize error paths + DDG outage mapping
- `crates/zoid-tools/src/web_fetch.rs` — categorize error paths + empty-extraction check
- `crates/zoid-web/src/search.rs` — `is_ddg_error_page` + `search_with_client` logic
- `crates/zoid-web/src/lib.rs` — `fetch` 2xx empty-extraction check
- `crates/zoid-core/src/event.rs` — `ToolResult` event gains `error_kind`
- `crates/zoid-core/src/projection.rs` — `ChatMsg::ToolResult` gains `error_kind`
- `crates/zoid-core/src/context.rs` — `[error: <kind>]` prefix rendering in request builder
- `crates/zoid-core/src/compaction.rs` — propagate `error_kind` through compaction
- `crates/zoid-core/src/eviction.rs` — propagate `error_kind` (if needed for ranking)
- `crates/zoid-core/src/reassert.rs` — propagate `error_kind` through reassert
- `crates/zoid/src/agent.rs` — CWD-deleted pre-check in Local and Network tool-dispatch arms; `[error: <kind>]` prefix rendering in `ChatMsg` → `Message` conversion
- `crates/zoid/src/zoom.rs` — `ChatMsg::ToolResult` display (if it reads `error_kind`)

### Locked decisions

- **ErrorKind taxonomy:** 8 variants (BackendUnavailable, Timeout, NotFound,
  InvalidInput, PermissionDenied, Conflict, CwdDeleted, Internal).
- **Model sees the kind:** `[error: <kind>]` prefix rendered in the tool-result
  text that goes to the provider.
- **Loop gets the enum:** `error_kind` is a field on `ToolOutput` and
  `EventKind::ToolResult`, available for future retry logic.
- **DDG outage detection:** heuristic `is_ddg_error_page` in `zoid-web`,
  string-matching categorization in `zoid-tools` (not typed errors from
  `zoid_web`).
- **`err()` defaults to `Internal`:** existing call sites that aren't migrated
  still compile and produce a categorized error.
- **grep/glob "no matches" is not an error:** these return empty result sets, not
  errors. No `ErrorKind` applies.
- **shell nonzero exit is not an error:** it already has `[exit N]` + stderr; only
  spawn failures get an `ErrorKind`.
- **CWD-deleted pre-check:** the agent loop checks `cwd.exists()` before every
  tool call; if false, short-circuits with `ErrorKind::CwdDeleted` and a
  recovery message pointing to `exit_worktree`. Not a per-tool check — it's a
  loop-level guard.
- **No self-deletion guard:** the agent is trusted not to `rm -rf` its own CWD;
  the pre-check catches the aftermath, not the cause.
- **Worktree-flow audit:** the `exit_worktree` path is already hardened (WT-2:
  CWD repointed before removal). The primary CWD-deletion risk is
  `worktree: false` subagents sharing the parent's CWD and running destructive
  shell commands. No worktree-flow changes are needed; the pre-check is the
  safety net.