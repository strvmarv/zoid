# Subagent Tool-Execution Verification Guard — Design

> **Date:** 2026-07-12
> **Status:** Approved design, pending implementation plan
> **Origin:** Follow-up to `2026-07-12-subagent-dispatch-smoke-findings.md`
> Finding 4. The smoke-test observed subagents that return `ok: true` with a
> plausible summary but performed **no actual work** — the subagent's model
> emitted assistant text ("I wrote the file") without emitting any tool call.
> This is upstream model behavior, not a dispatch/spawn defect, so the fix is a
> **verification layer** at result-distillation time, not a code fix to the
> spawn machinery.

---

## Problem

`distill()` (`crates/zoid/src/subagent.rs`) converts a subagent's branch events
into `(summary, ok)`. Today:

- `summary` = last non-empty assistant text (or a warn-glyph placeholder).
- `ok` = summary doesn't start with the warn glyph **AND** no `ToolResult` has
  `is_error: true`.

A **hallucinated no-op** slips through: the model emits only assistant text and
**zero** tool calls. `distill` sees a non-empty summary and no errored results,
so it reports `ok: true` with a confident-sounding summary — the worst failure
mode, because there is no error signal for the orchestrator to act on.

A second, distinct integrity gap: a `ToolCall` emitted with **no matching
`ToolResult`** ("claimed but never executed") is also reported `ok: true` today.

## Goal

Give the orchestrator a truthful signal about tool-execution integrity, without
falsely discarding legitimate text-only subagents (e.g. a "summarize this"
task that correctly performs no tool calls).

## Non-goals

- No NLP/claim-parsing of summary prose (brittle, non-deterministic — rejected).
- No changes to the event schema, `DelegationResult`, or UI. The signal rides
  in the existing `summary` string.
- No changes to the spawn/cancel machinery (Finding 5 confirmed not a bug).
- Not a config flag — the guard is always on; its output is advisory-or-hard
  per signal (below).

## Design

Two signals, computed from the subagent's own branch events, with
**deliberately asymmetric enforcement**:

| Signal | Nature | Effect on `ok` | Rationale |
|--------|--------|----------------|-----------|
| Orphan `ToolCall` (id has no matching `ToolResult`) | **Structural** | **`ok = false`** | Unambiguously wrong in a healthy subagent; near-zero false positives. |
| Zero tool calls | **Semantic** | **unchanged** (advisory note only) | Only the orchestrator knows if the task *needed* side effects; hard-failing would false-positive legitimate text-only tasks. |

**Why the asymmetry is safe.** The subagent profile
(`AgentProfile::builtin()`, `crates/zoid-core/src/agent_profile.rs:40-48`)
allows exactly seven tools: `read, write, edit, grep, glob, ls, shell`. Every
one is a normal paired tool that produces a `ToolResult`. **No `Emitting` tool**
(`dispatch_subagent`, `show`, `recall`, `tasks`, `worktree_enter`, …) is on the
list — `run_subagent`'s registry filter (`subagent.rs:136-142`,
`profile.allows(name)` + drop `Interactive`) removes them. So in a healthy run
every `ToolCall` produces a matching `ToolResult`, and an orphan is a genuine
anomaly worth hard-failing. The zero-activity case, by contrast, is legitimate
for text-only tasks, so it must stay advisory (`ok` unchanged).

> **Forward-looking caveat.** The orphan hard-fail relies on the allow-list
> containing zero `Emitting` tools (those emit their own events, not a paired
> `ToolResult`). If a future change adds an `Emitting` tool (e.g. `recall`) to
> the subagent profile, `verify_execution` must exclude `Emitting`-tool call
> ids from the orphan set, or the check will false-positive. The registry
> filter drops `Interactive` but **not** `Emitting`, so this is the one place
> the invariant could silently break.

### Component 1 — `verify_execution` (pure helper)

```rust
/// Structural tool-execution report for a subagent's own branch events.
/// Pure (no I/O); drives distill()'s ok flag + advisory notes.
struct ExecReport {
    tool_call_count: usize,
    /// ToolCall ids that never produced a matching ToolResult, in emit order,
    /// de-duplicated.
    orphan_ids: Vec<String>,
}

fn verify_execution(branch_events: &[Event]) -> ExecReport;
```

Single pass over `branch_events`:
- Collect `ToolCall` ids (in order) and the set of `ToolResult` ids.
- `orphan_ids` = call ids not present in the result-id set (dedup, preserve
  first-seen order).
- `tool_call_count` = number of `ToolCall` events.

### Component 2 — integration in `distill()`

`distill` calls `verify_execution(branch_events)` and folds the report in:

- **Final `ok`:**
  `ok = !summary.starts_with(WARN_GLYPH) && !has_errors && report.orphan_ids.is_empty()`
- **Notes** (appended to `summary`, each on its own line, composing with the
  existing "one or more tool calls errored" note):
  - If `!orphan_ids.is_empty()`:
    `⚠ {n} tool call(s) produced no result: {id, id, …}`
  - If `tool_call_count == 0`:
    `note: subagent emitted no tool calls — if this task required file or shell
    changes, its results are unverified`

The zero-activity note is phrased to invite the orchestrator to reason it away
when the task was legitimately text-only.

`distill` keeps its `(String, bool)` return type; all callers are unchanged.

### Data flow

```
run_subagent
  └─ produced events ──► branch_events (rebased to default branch)
                          └─ distill(branch_events)
                               ├─ verify_execution(branch_events) → ExecReport
                               ├─ summary  (+ notes)
                               └─ ok       (orphan ⇒ false)
                          └─ SubagentResult { summary, ok, … }
                               └─ DelegationResult.summary  (orchestrator reads)
```

## Edge cases

- **Duplicate `ToolResult` ids:** result-id set handles idempotently.
- **A `ToolCall` id appearing twice:** report the orphan once (dedup).
- **Zero calls AND orphan:** impossible — an orphan requires ≥1 call.
- **Warn-glyph summary (no assistant text) + zero calls:** `ok` already `false`;
  the zero-activity advisory note is still appended (adds useful context).
- **Errored result + orphan:** both notes appear; `ok` is `false` either way.

## Testing (TDD)

All tests target `verify_execution` and `distill` directly, using the existing
`call(id, path)` / `result(id, out)` helpers in the `subagent.rs` test module.
Each written and watched to fail before implementation.

1. **Orphan call flips ok:** one `ToolCall` with no `ToolResult` →
   `ok == false`, summary names the orphan id.
2. **Paired calls stay ok:** call + matching result → no orphan,
   `ok == true`, no orphan note.
3. **Zero calls, real summary, stays ok (key false-positive-safety test):**
   non-empty assistant text, no tool calls → `ok == true` + advisory note
   present.
4. **Zero calls, warn-glyph summary:** no assistant text, no calls →
   `ok == false` (existing) + advisory note present.
5. **Errored result regression guard:** existing "one or more tool calls
   errored" behavior → `ok == false`, unchanged.
6. **Orphan + errored compose:** both notes present; `ok == false`.

## Blast radius

- **Files touched:** `crates/zoid/src/subagent.rs` only.
- **Public surface:** none. `distill` signature unchanged; no schema/UI/main.rs.
- **Behavioral change:** subagents with orphan tool calls now report
  `ok: false` (previously `ok: true`); subagents get advisory notes in their
  summary. Orchestrator already reads `summary`, so no new plumbing.

## Rollback

Single-file, additive change. Revert the commit to restore prior behavior; no
migrations, no persisted state.
