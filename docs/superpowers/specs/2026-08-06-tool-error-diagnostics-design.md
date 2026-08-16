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

**Pre-check in the agent loop:** Before dispatching a tool call in the Local
and Network arms, the agent loop checks `cwd.exists()`. If the CWD no longer
exists, it short-circuits: instead of running the tool, it returns a
`ToolOutput::err_kind(ErrorKind::CwdDeleted, ...)` with a recovery message.

**Emitting tools are explicitly exempt** from the pre-check. `exit_worktree`
and `enter_worktree` are `ToolKind::Emitting` — handled in a separate dispatch
arm that does not use `cwd` for file operations. `exit_worktree` is the
recovery action the pre-check tells the model to call; short-circuiting it
would trap the agent in a deleted-CWD state with no escape. The check belongs
only in the Local and Network arms, not above the `ToolKind` match.

The check is a single `Path::exists()` syscall — negligible overhead per tool
call. For Network tools (`web_search`, `web_fetch`) the check is technically a
no-op (they ignore `cwd`), but running it uniformly avoids a per-tool
branching decision and catches the case where a Network tool is later modified
to touch the filesystem.

**New ErrorKind variant:**

```rust
/// The working directory was deleted out from under the agent. Recovery:
/// call exit_worktree (if in a worktree) or navigate to an existing directory.
CwdDeleted,
```

**Recovery message format:**

When the agent loop knows it is in a worktree (it tracks `cwd_for_exec` and
handles `WorktreeAction`), the message is unambiguous:

```
[error: cwd_deleted] You are in a worktree — the working directory "{cwd}" no
longer exists. Call exit_worktree to return to the main checkout.
```

When not in a worktree (CWD is the main checkout or a user-specified dir):

```
[error: cwd_deleted] The working directory "{cwd}" no longer exists. Navigate
to an existing directory (e.g., the repo root) before running another command.
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
called, so the tool's own error paths are not reached in the common case. The
`EventKind::ToolResult` for a CWD-deleted short-circuit carries
`error_kind: Some(CwdDeleted)` like any other error.

**TOCTOU race:** `cwd.exists()` can pass and the CWD can be deleted before the
tool's file operation runs (e.g., a sibling `worktree: false` subagent deletes
the CWD between the pre-check and the tool's `current_dir(cwd)`/file op). In
this race window, the tool hits its own OS-error path and is categorized
according to that tool's error mapping (typically `Internal` for a generic
"No such file or directory"). This is acceptable — the pre-check catches the
common case (CWD already gone at dispatch time); the tool's error handling is
the fallback for the race. The spec does not overclaim that the pre-check
eliminates all CWD-deletion errors.

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

`EventKind::ToolResult` gains `error_kind: Option<ErrorKind>`. Since
`EventKind` is `#[derive(Serialize, Deserialize)]` and persisted to SQLite
(`store.rs` does `serde_json::to_string`/`from_str`), the new field **must**
have `#[serde(default)]` so existing sessions on disk (which lack the field)
deserialize successfully as `error_kind: None`:

```rust
ToolResult {
    id: String,
    name: String,
    output: String,
    is_error: bool,
    #[serde(default)]
    error_kind: Option<ErrorKind>,  // new
}
```

`ErrorKind` itself must also derive `Serialize, Deserialize` (it's carried
inside the persisted event). Add a test that deserializes a legacy
`ToolResult` JSON (without the `error_kind` field) and asserts
`error_kind: None`.

The agent loop, when constructing a `ToolResult` event from a `ToolOutput`:

```rust
EventKind::ToolResult {
    error_kind: out.error_kind,
    // ...
}
```

The **model-facing text** gets the `[error: <kind>]` prefix rendered at the
point where the tool result is converted to a provider `Message` — in
`agent.rs`'s `map_msg` function (`ChatMsg::ToolResult { id, name, output, .. }
=> Message::tool_with_call_id(name, id, output)`). The prefix is prepended to
`output` when `is_error && error_kind.is_some()`. This keeps it consistent
across all tools without each tool having to format it. `context.rs` does not
produce provider `Message`s (it builds `ContextItem`s for token estimation) and
is **not** the rendering location. The rendering:

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

`is_ddg_error_page` checks for known DDG error/diagnostic markers in the HTML
body. The check is heuristic (DDG doesn't document its error pages), based on
observed patterns. The exact predicate:

```rust
fn is_ddg_error_page(body: &str) -> bool {
    body.contains("error-lite@duckduckgo.com")
        || body.contains("error@duckduckgo.com")
        || body.contains("If this error persists")
}
```

This is intentionally simple: three substring checks, OR'd together. The
heuristic does **not** use "presence of non-page-chrome text" as a signal —
a genuine "no results" page also has non-chrome text (the "No results"
message), so that signal is ambiguous. Only the three named DDG
error/diagnostic markers trigger `BackendUnavailable`.

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

**Alternative considered:** returning a typed error from `zoid_web::search`
instead of string-matching. This is cleaner but requires changing the
`zoid_web` public API (returning a custom error enum instead of
`anyhow::Result`). The string-matching approach avoids coupling `zoid_web` to
`ErrorKind` (the leaf crate stays independent of the tool-layer type) and puts
the categorization in the tool layer. The trade-off is acceptable for now; a
future refactor can make `zoid_web` return typed errors.

**Note:** `zoid_web` is not left unchanged — `is_ddg_error_page` is added to
`search.rs`, and the 2xx-empty-extraction check changes `fetch`'s behavior
from `Ok(FetchResult{content: ""})` to `Err(...)` for unparseable 2xx
responses. This is a deliberate behavior change to the leaf crate: returning
empty content as `Ok` was always unhelpful (the model sees a blank page with
no diagnostic), and `Err` lets the tool layer categorize it as
`BackendUnavailable`. The change ripples to any caller of `zoid_web::fetch`,
but the sole caller is the `web_fetch` tool, which already handles `Err`.

**Decision: string-matching in the tool layer for categorization; behavior
change in `zoid_web` for detection.** `zoid_web` gains the detection logic
(`is_ddg_error_page`, empty-extraction `Err`); `zoid-tools` gains the
categorization (string-matching the error message to `ErrorKind`). This keeps
`ErrorKind` out of `zoid_web` while making the leaf crate's errors more
informative.

#### `fetch.rs` — Already mostly correct

`web_fetch` already handles non-2xx with a clear error (`HTTP {status}: {snippet}`).
The `zoid_web::fetch` function returns `Err` for non-2xx, so the tool layer maps
that to `BackendUnavailable`. For 2xx responses where `extract_markdown` returns
empty/garbage, the current code returns `Ok` with empty content — the tool should
check for this and return `BackendUnavailable` when the extracted content is empty
on a 2xx response (the page returned something but it wasn't parseable as content).

### Tool-by-tool error audit

Each tool's error paths are categorized. The base `registry()` tools are
listed first, followed by the chat-only tools (`invoke_skill.rs` registry).
Tools that currently have a single catch-all `io::Error` path (read, ls,
write) require inspecting `e.kind()` (`std::io::ErrorKind`) to categorize
correctly — e.g., `ErrorKind::NotFound` → `NotFound`,
`ErrorKind::PermissionDenied` → `PermissionDenied`, else → `Internal`. This is
not a simple label swap; the implementation plan should account for the
`io::ErrorKind` inspection in these tools.

**Base registry tools:**

| Tool | Error path | ErrorKind | Model-facing text |
|------|-----------|-----------|-------------------|
| **read** | missing file (`io::ErrorKind::NotFound`) | NotFound | `read({path}): file does not exist` |
| **read** | non-UTF8 file (`io::ErrorKind::InvalidData`) | InvalidInput | `read({path}): file is not valid UTF-8` |
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
| **ls** | dir doesn't exist (`io::ErrorKind::NotFound`) | NotFound | `ls({path}): directory does not exist` |
| **ls** | permission denied (`io::ErrorKind::PermissionDenied`) | PermissionDenied | `ls({path}): permission denied` |
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

**Chat-only tools (from `invoke_skill.rs` registry):**

| Tool | Kind | Error path | ErrorKind | Model-facing text |
|------|------|-----------|-----------|-------------------|
| **subagent_diff** | Local | subagent history not found | NotFound | `subagent_diff: history not found for {id}` |
| **subagent_diff** | Local | git rev-parse/diff failed | Internal | `subagent_diff: git rev-parse failed: {e}` |
| **recall** | Emitting | (handled by agent loop; validation errors) | InvalidInput | `recall: {error}` |
| **dispatch_subagent** | Emitting | task argument required | InvalidInput | `dispatch_subagent: 'task' is required` |
| **dispatch_subagent** | Emitting | profile/agent resolution failure | Internal | `dispatch_subagent: {e}` |
| **dispatch_subagent** | Emitting | pool full (queued) | N/A (not an error — returns queued status) | — |
| **cancel_subagent** | Emitting | (handled by agent loop) | Internal | `cancel_subagent: {e}` |
| **list_subagents** | Local | (no error paths — reads in-memory registry) | — | — |
| **list_agents** | Local | (no error paths — reads in-memory registry) | — | — |
| **enter_worktree** | Emitting | name required | InvalidInput | `enter_worktree: 'name' is required` |
| **enter_worktree** | Emitting | already in a worktree | Conflict | `already in a worktree — exit with exit_worktree first` |
| **exit_worktree** | Emitting | not in a worktree | Conflict | `not in a worktree` |
| **exit_worktree** | Emitting | subagent running | Conflict | `cannot exit worktree while a subagent is running` |
| **schedule_wake** | Emitting | (validation errors) | InvalidInput | `schedule_wake: {e}` |
| **cancel_wake** | Emitting | (validation errors) | InvalidInput | `cancel_wake: {e}` |
| **show** | Emitting | (handled by agent loop) | Internal | `show: {e}` |

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

#### Serialization compatibility

- Deserialize a legacy `ToolResult` JSON (without `error_kind`) and assert
  `error_kind: None` (verifies `#[serde(default)]` works).

#### Integration tests

- The `[error: <kind>]` prefix appears in the model-facing tool result text
  (tested at the `map_msg` conversion in `agent.rs`, not `context.rs`).

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

**`zoid-tools` (ErrorKind + tool error categorization):**
- `crates/zoid-tools/src/lib.rs` — `ErrorKind` enum, `ToolOutput` changes
- `crates/zoid-tools/src/read.rs` — categorize error paths (inspect `io::ErrorKind`)
- `crates/zoid-tools/src/write.rs` — categorize error paths (inspect `io::ErrorKind`)
- `crates/zoid-tools/src/edit.rs` — categorize error paths
- `crates/zoid-tools/src/ls.rs` — categorize error paths (inspect `io::ErrorKind`)
- `crates/zoid-tools/src/shell.rs` — categorize spawn failure
- `crates/zoid-tools/src/git_context.rs` — categorize error paths
- `crates/zoid-tools/src/web_search.rs` — categorize error paths + DDG outage mapping
- `crates/zoid-tools/src/web_fetch.rs` — categorize error paths + empty-extraction check
- `crates/zoid-tools/src/subagent_diff.rs` — categorize error paths (Local tool)

**`zoid-web` (outage detection):**
- `crates/zoid-web/src/search.rs` — `is_ddg_error_page` + `search_with_client` logic
- `crates/zoid-web/src/lib.rs` — `fetch` 2xx empty-extraction behavior change

**`zoid-core` (event propagation — blast radius from adding a field to a serde-persisted enum variant):**
- `crates/zoid-core/src/event.rs` — `ToolResult` event gains `#[serde(default)] error_kind: Option<ErrorKind>`
- `crates/zoid-core/src/projection.rs` — `ChatMsg::ToolResult` gains `error_kind`; ~10 construction/match sites updated
- `crates/zoid-core/src/compaction.rs` — propagate `error_kind` through compaction; construction sites updated
- `crates/zoid-core/src/eviction.rs` — propagate `error_kind`; construction sites updated
- `crates/zoid-core/src/reassert.rs` — propagate `error_kind` through reassert; construction sites updated
- `crates/zoid-core/src/store.rs` — `fts_content` match arm updated (reads `ToolResult { output, name, .. }`)
- `crates/zoid-core/src/context.rs` — construction sites updated (does NOT do prefix rendering)

**`zoid` (agent loop + prefix rendering + CWD pre-check):**
- `crates/zoid/src/agent.rs` — CWD-deleted pre-check in Local and Network arms (Emitting exempt); `[error: <kind>]` prefix rendering in `map_msg` (`ChatMsg` → `Message` conversion); ~15 construction sites updated
- `crates/zoid/src/eventlog.rs` — `ToolResult { output, .. }` match arm updated
- `crates/zoid/src/spawn_subagent.rs` — construction sites updated (if any `ToolResult` constructions exist)
- `crates/zoid/src/subagent.rs` — ~7 test construction sites updated
- `crates/zoid/src/main.rs` — ~22 construction/match sites updated
- `crates/zoid/src/invoke_skill.rs` — chat-only tool error paths categorized

**`zoid-tui` (UI display — Goal #3):**
- `crates/zoid-tui/src/chat.rs` — ~23 `ChatMsg::ToolResult` pattern match and construction sites updated; UI can read `error_kind` for icon/color differentiation
- `crates/zoid-tui/src/objects.rs` — construction/match sites updated
- `crates/zoid-tui/tests/chat_snapshot.rs` — snapshot test construction sites updated
- `crates/zoid-tui/tests/shell_snapshot.rs` — snapshot test construction sites updated
- `crates/zoid-tui/examples/scenes/mod.rs` — construction sites updated

**Integration tests:**
- `crates/zoid/tests/*.rs` — ~7 test files with `EventKind::ToolResult` constructions updated
- `crates/zoid-core/tests/*.rs` — any construction sites updated

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
- **CWD-deleted pre-check:** the agent loop checks `cwd.exists()` before
  every Local and Network tool call (Emitting tools are exempt — they are the
  recovery path); if false, short-circuits with `ErrorKind::CwdDeleted` and a
  context-aware recovery message pointing to `exit_worktree` (when in a
  worktree) or navigating to an existing directory. Not a per-tool check — it's
  a loop-level guard. A TOCTOU race exists (CWD deleted between pre-check and
  tool execution); the tool's own error handling is the fallback, typically
  categorized `Internal` for the resulting "No such file or directory."
- **No self-deletion guard:** the agent is trusted not to `rm -rf` its own CWD;
  the pre-check catches the aftermath, not the cause.
- **Worktree-flow audit:** the `exit_worktree` path is already hardened (WT-2:
  CWD repointed before removal). The primary CWD-deletion risk is
  `worktree: false` subagents sharing the parent's CWD and running destructive
  shell commands. No worktree-flow changes are needed; the pre-check is the
  safety net.