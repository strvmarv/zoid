# Subagent Tool-Execution Verification Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the orchestrator a truthful tool-execution-integrity signal for each subagent — hard-fail structurally-broken runs (orphan tool calls) and advise on zero-activity runs — without falsely discarding legitimate text-only subagents.

**Architecture:** Add a pure `verify_execution(branch_events) -> ExecReport` helper in `crates/zoid/src/subagent.rs`, and fold its report into the existing `distill()` function. Orphan `ToolCall`s (a call id with no matching `ToolResult`) flip `ok = false` and append a note; zero tool calls append an advisory note only (`ok` unchanged). `distill` keeps its `(String, bool)` signature — no caller, schema, UI, or `main.rs` change.

**Tech Stack:** Rust, `cargo test`, the `zoid` crate's existing `subagent.rs` test module (which already provides `ev()`, `call(id, path)`, `result(id, out)` helpers).

## Global Constraints

- Touch **only** `crates/zoid/src/subagent.rs`. No event-schema, `DelegationResult`, UI, or `main.rs` changes.
- `distill()` MUST keep its `fn(&[Event]) -> (String, bool)` signature.
- `ok` is flipped to `false` **only** by an orphan `ToolCall`. Zero-activity is **advisory only** (never changes `ok`).
- Preserve existing behavior: warn-glyph summary ⇒ `ok=false`; any `ToolResult { is_error: true }` ⇒ `ok=false` with the existing "one or more tool calls errored" note.
- Commit messages: no `Co-Authored-By` / co-author trailer (per repo CLAUDE.md).
- Exact note strings (verbatim):
  - Orphan: `⚠ {n} tool call(s) produced no result: {comma-joined ids}`
  - Zero-activity: `note: subagent emitted no tool calls — if this task required file or shell changes, its results are unverified`
- `WARN_GLYPH` is the existing constant already used in `subagent.rs`.

---

### Task 1: `verify_execution` pure helper + `ExecReport`

**Files:**
- Modify: `crates/zoid/src/subagent.rs` (add `ExecReport` struct + `verify_execution` fn near `distill`, ~line 236; add unit tests in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `zoid_core::event::{Event, EventKind}` (already imported in the module; `EventKind::ToolCall { id, name, args }`, `EventKind::ToolResult { id, name, output, is_error }`).
- Produces:
  - `struct ExecReport { tool_call_count: usize, orphan_ids: Vec<String> }`
  - `fn verify_execution(branch_events: &[Event]) -> ExecReport`
  - `orphan_ids` are `ToolCall` ids with no matching `ToolResult` id, in first-seen order, de-duplicated.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid/src/subagent.rs` (the `call`/`result` helpers already exist there):

```rust
#[test]
fn verify_execution_flags_orphan_call() {
    let evs = vec![call("c1", "a.rs"), result("c1", "ok"), call("c2", "b.rs")];
    let r = verify_execution(&evs);
    assert_eq!(r.tool_call_count, 2);
    assert_eq!(r.orphan_ids, vec!["c2".to_string()]);
}

#[test]
fn verify_execution_no_orphans_when_all_paired() {
    let evs = vec![call("c1", "a.rs"), result("c1", "ok")];
    let r = verify_execution(&evs);
    assert_eq!(r.tool_call_count, 1);
    assert!(r.orphan_ids.is_empty());
}

#[test]
fn verify_execution_counts_zero_calls() {
    let evs = vec![ev(EventKind::UserMessage { text: "hi".into() })];
    let r = verify_execution(&evs);
    assert_eq!(r.tool_call_count, 0);
    assert!(r.orphan_ids.is_empty());
}

#[test]
fn verify_execution_dedups_orphan_ids() {
    // Same call id emitted twice with no result — reported once.
    let evs = vec![call("c1", "a.rs"), call("c1", "a.rs")];
    let r = verify_execution(&evs);
    assert_eq!(r.tool_call_count, 2);
    assert_eq!(r.orphan_ids, vec!["c1".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib subagent::tests::verify_execution -- --nocapture`
Expected: FAIL — `cannot find function verify_execution in this scope` (and `ExecReport` not found).

- [ ] **Step 3: Write the minimal implementation**

Add above `distill` (around line 237) in `crates/zoid/src/subagent.rs`:

```rust
/// Structural tool-execution report for a subagent's own branch events.
/// Pure (no I/O); drives `distill`'s `ok` flag + advisory notes.
struct ExecReport {
    tool_call_count: usize,
    /// `ToolCall` ids that never produced a matching `ToolResult`, in
    /// first-seen order, de-duplicated.
    orphan_ids: Vec<String>,
}

/// A `ToolCall` whose id has no matching `ToolResult` is "claimed but never
/// executed". Subagents carry only paired tools (read/write/edit/grep/glob/
/// ls/shell — see `AgentProfile::builtin`), so an orphan is a genuine anomaly.
fn verify_execution(branch_events: &[Event]) -> ExecReport {
    use std::collections::HashSet;
    let mut call_ids: Vec<String> = Vec::new();
    let mut result_ids: HashSet<String> = HashSet::new();
    for e in branch_events {
        match &e.kind {
            EventKind::ToolCall { id, .. } => call_ids.push(id.clone()),
            EventKind::ToolResult { id, .. } => {
                result_ids.insert(id.clone());
            }
            _ => {}
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let orphan_ids = call_ids
        .iter()
        .filter(|id| !result_ids.contains(*id))
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect();
    ExecReport {
        tool_call_count: call_ids.len(),
        orphan_ids,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib subagent::tests::verify_execution`
Expected: PASS (4 tests). Output pristine — no warnings about the new items (they are used by the tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "feat(subagent): add verify_execution tool-integrity helper"
```

---

### Task 2: Fold `ExecReport` into `distill` (orphan hard-fail + advisory note)

**Files:**
- Modify: `crates/zoid/src/subagent.rs` — `distill` (lines ~241-263) and its tests.

**Interfaces:**
- Consumes: `verify_execution(&[Event]) -> ExecReport` from Task 1.
- Produces: unchanged `distill(&[Event]) -> (String, bool)`. New behavior:
  orphan present ⇒ `ok=false` + orphan note; zero calls ⇒ advisory note, `ok`
  unchanged.

- [ ] **Step 1: Write the failing tests**

Add to `#[cfg(test)] mod tests` in `crates/zoid/src/subagent.rs`. `distill` folds events through `conversation()`, which yields the last non-empty `ChatMsg::Assistant`. The canonical assistant-text event is `EventKind::AssistantMessage { text: String }` (`crates/zoid-core/src/event.rs:74-76`):

```rust
// Helper: an assistant text event (canonical assistant-text variant).
fn assistant(text: &str) -> Event {
    ev(EventKind::AssistantMessage { text: text.into() })
}

#[test]
fn distill_orphan_call_forces_not_ok() {
    let evs = vec![assistant("did the thing"), call("c1", "a.rs")]; // no result
    let (summary, ok) = distill(&evs);
    assert!(!ok, "orphan tool call must force ok=false");
    assert!(
        summary.contains("produced no result") && summary.contains("c1"),
        "summary must name the orphan id: {summary}"
    );
}

#[test]
fn distill_zero_calls_stays_ok_with_advisory() {
    // KEY false-positive-safety test: a legitimate text-only subagent.
    let evs = vec![assistant("here is your summary")];
    let (summary, ok) = distill(&evs);
    assert!(ok, "zero tool calls must NOT flip ok for a text-only subagent");
    assert!(
        summary.contains("emitted no tool calls"),
        "advisory note must be present: {summary}"
    );
}

#[test]
fn distill_paired_calls_stay_ok() {
    let evs = vec![assistant("done"), call("c1", "a.rs"), result("c1", "ok")];
    let (_summary, ok) = distill(&evs);
    assert!(ok, "a healthy subagent with paired calls stays ok");
}

#[test]
fn distill_errored_result_still_not_ok() {
    // Regression guard: existing behavior preserved.
    let evs = vec![
        assistant("tried"),
        call("c1", "a.rs"),
        ev(EventKind::ToolResult {
            id: "c1".into(),
            name: "read_file".into(),
            output: "boom".into(),
            is_error: true,
        }),
    ];
    let (summary, ok) = distill(&evs);
    assert!(!ok);
    assert!(summary.contains("errored"), "keeps existing errored note: {summary}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib subagent::tests::distill_`
Expected: FAIL — `distill_orphan_call_forces_not_ok` fails (`ok` is currently `true`); `distill_zero_calls_stays_ok_with_advisory` fails (no advisory note in summary). `distill_paired_calls_stay_ok` and `distill_errored_result_still_not_ok` should already PASS (they assert existing behavior) — if either errors on the `assistant` helper, fix the variant name before proceeding.

- [ ] **Step 3: Write the minimal implementation**

Replace the body of `distill` (lines ~241-263) with:

```rust
fn distill(branch_events: &[Event]) -> (String, bool) {
    let mut summary = conversation(branch_events)
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("{WARN_GLYPH} subagent produced no output"));

    let has_errors = branch_events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::ToolResult { is_error: true, .. }));

    let report = verify_execution(branch_events);
    let has_orphans = !report.orphan_ids.is_empty();

    let ok = !summary.starts_with(WARN_GLYPH) && !has_errors && !has_orphans;

    if has_errors && !summary.starts_with(WARN_GLYPH) {
        summary = format!("{summary}\n\n{WARN_GLYPH} one or more tool calls errored");
    }
    if has_orphans {
        summary = format!(
            "{summary}\n\n{WARN_GLYPH} {} tool call(s) produced no result: {}",
            report.orphan_ids.len(),
            report.orphan_ids.join(", ")
        );
    }
    if report.tool_call_count == 0 {
        summary = format!(
            "{summary}\n\nnote: subagent emitted no tool calls — if this task \
             required file or shell changes, its results are unverified"
        );
    }
    (summary, ok)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib subagent::tests`
Expected: PASS — all `distill_*`, `verify_execution_*`, and the pre-existing `subagent.rs` tests green. No warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "feat(subagent): distill flags orphan calls (ok=false) + advises on zero tool activity"
```

---

### Task 3: Full-suite regression + clippy

**Files:** none (verification only).

- [ ] **Step 1: Run the crate test suite**

Run: `cargo test -p zoid`
Expected: PASS. Confirm no existing subagent/dispatch/worktree test regressed.

- [ ] **Step 2: Lint**

Run: `cargo clippy -p zoid --all-targets -- -D warnings`
Expected: no warnings. (Watch for `HashSet` import scoping and an unused-variable lint on `_summary`.)

- [ ] **Step 3: Commit (only if clippy required a fix)**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "chore(subagent): satisfy clippy for verify_execution"
```

---

## Self-Review

**1. Spec coverage:**
- `verify_execution` pure helper (spec Component 1) → Task 1. ✓
- Orphan ⇒ `ok=false` + note (spec Component 2, structural signal) → Task 2 (`distill_orphan_call_forces_not_ok`). ✓
- Zero calls ⇒ advisory note, `ok` unchanged (semantic signal) → Task 2 (`distill_zero_calls_stays_ok_with_advisory`, the key false-positive test). ✓
- Compose with existing errored note; warn-glyph preserved → Task 2 (`distill_errored_result_still_not_ok`). ✓
- Edge: duplicate orphan id reported once → Task 1 (`verify_execution_dedups_orphan_ids`). ✓
- Blast radius = single file, `distill` signature unchanged → enforced by Global Constraints; no caller edits appear in any task. ✓
- Forward-looking `Emitting`-tool caveat → documentation-only in the spec; **intentionally not implemented** (v1 defers it), so no task. Confirmed with user.

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every run step shows the exact command + expected result. The assistant-text variant was pinned to `EventKind::AssistantMessage { text }` (`event.rs:74-76`) against source — no deferred unknowns remain.

**3. Type consistency:** `ExecReport { tool_call_count: usize, orphan_ids: Vec<String> }` and `verify_execution(&[Event]) -> ExecReport` are named identically in Tasks 1 and 2. `distill(&[Event]) -> (String, bool)` unchanged throughout. Note strings match the spec verbatim. `EventKind::{ToolCall, ToolResult, AssistantMessage, UserMessage}` field shapes match `event.rs`.
