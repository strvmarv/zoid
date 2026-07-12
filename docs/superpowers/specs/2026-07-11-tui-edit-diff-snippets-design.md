# TUI Edit/Write Diff Snippets — Design

**Date:** 2026-07-11
**Status:** Design (approved shape; pending spec review)

## Goal

Show compact, colored add/delete diff snippets in the zoid TUI chat for each
`edit`/`write` tool call — like Claude Code — so the user feels connected to what
the agent changed, without the performance cost of keeping every diff on screen
and without inflating the model's context or the on-disk event log.

## Core principle — diffs are an ephemeral view-layer garnish

Diffs live **only** in memory, for the current session:

- Computed live inside the `edit`/`write` tool at execution time (the tool holds
  before + after).
- Delivered to the TUI on a **non-persisted** `AgentUpdate` (the same transient
  class as `SubagentStarted`) — they never enter the event log, the SQLite DB, or
  the model's request context.
- Held in a **bounded** in-memory map in `App`, keyed by tool-call id.
- On reload/resume the map is empty, so `edit`/`write` rows fall back to today's
  plain summary line.

This keeps the feature entirely on the transient side of zoid's event-sourced
architecture: it cannot corrupt replay, cannot bloat the DB, and cannot spend
model tokens. **No `EventKind`, DB, or schema change anywhere.**

## Confirmed parameters

| Parameter | Value |
|-----------|-------|
| Inline window `K` (most-recent edits shown inline) | **5** |
| Per-diff line cap (before `…+N more`) | **20** |
| Context lines around each hunk | **1** |
| In-memory diff cache cap | **16** (constant; ≥ K for headroom) |
| Tools in scope | **`edit`, `write`** only |
| Ships enabled | **yes** |

## Architecture & data flow

```
edit/write tool (.run)          agent.rs dispatch (~1998)         TUI (App/shell)
──────────────────────          ─────────────────────────        ─────────────────
holds before + after   ──►  ToolOutput{ text, is_error,   ──►  if out.diff.is_some():
computes FileDiff             diff: Option<FileDiff> }            send AgentUpdate::EditDiff
sets out.diff                                                     { id, diff }  (UI-ONLY,
                              persists EventKind::ToolResult       never persisted)
                              { output: out.text }  ◄─ diff NOT in here
                                                                  ▼
                                                    App.edit_diffs: bounded map
                                                    <tool_id → FileDiff> (cap 16)
                                                                  ▼
                                                    chat.rs renders inline for last K=5
```

**The single fork point.** All ordinary tool execution funnels through
`zoid_tools::run_tool` (`crates/zoid-tools/src/lib.rs:174`, which calls
`Tool::run`), and `crates/zoid/src/agent.rs:~1998` is the one place a normal
`ToolOutput` becomes an `EventKind::ToolResult`. The diff is forked to the UI
there — exactly one site, not the ~15 `ToolResult` emit points.

**`ToolOutput` is transient.** Only `ToolOutput.text` is ever copied into a
persisted `EventKind::ToolResult`. Adding a `diff: Option<FileDiff>` field to
`ToolOutput` therefore has no persistence or model-context impact by construction.

## Components

### 1. Diff types (`zoid-tools`)

```rust
pub struct FileDiff {
    pub path: String,
    pub added: u32,        // total added lines (counted even when truncated)
    pub removed: u32,      // total removed lines
    pub lines: Vec<DiffLine>,   // capped to the per-diff line cap
    pub truncated_by: u32, // how many diff lines were dropped (0 = whole diff shown)
}

pub struct DiffLine {
    pub old_no: Option<u32>, // line number in the "before" (None for adds)
    pub new_no: Option<u32>, // line number in the "after" (None for dels)
    pub kind: DiffKind,      // Ctx | Add | Del
    pub text: String,
}

pub enum DiffKind { Ctx, Add, Del }
```

- `ToolOutput` gains `pub diff: Option<FileDiff>` (default `None`); `ToolOutput::ok`
  / `err` leave it `None`, so every existing tool is unaffected.
- `similar` becomes a direct dependency of `zoid-tools` (already present in
  `Cargo.lock` transitively via `insta`).

### 2. Diff computation (`edit`, `write`)

- `edit` already holds the old file contents and the post-edit contents; it
  computes a unified diff between them.
- `write` currently does **not** read the pre-existing file. It will read it first
  (best-effort) to obtain "before"; a brand-new file yields an all-additions diff.
  A failed pre-read degrades to `added = <line count>, removed = 0` with no line
  body (the write still succeeds — diff is best-effort and never fails the tool).
- Counts (`added`/`removed`) are computed over the **full** diff; only the `lines`
  vector is truncated to the cap, with `truncated_by` recording the remainder.
- Context is limited to 1 line around each hunk.

### 3. UI signal (`zoid`)

- New `AgentUpdate::EditDiff { id: String, diff: FileDiff }` (non-persisted).
- At the fork point, when `out.diff` is `Some`, the bin sends `EditDiff` alongside
  the usual `ToolResult` handling. The `ToolResult` event is unchanged.

### 4. TUI state & rendering (`zoid-tui`, `zoid` App)

- The bounded diff cache lives in **pure shell state** (`zoid_tui::state`), exactly
  as `subagent_rows` does today: `shell.edit_diffs: <bounded map tool_id →
  FileDiff>`, cap 16, evicting oldest. This is the single home for the cache so the
  pure renderer can read it. `main.rs`'s `EditDiff` handler is the only writer
  (`app.shell.edit_diffs.insert(id, diff)`), mirroring how `SubagentStarted` writes
  `app.shell.subagent_rows`.
- `chat.rs` `ChatMsg::ToolResult` rendering, when `name == "edit" || name == "write"`:
  - **Counts line (always, while cached):** append `· +A −R` to the tool line,
    colored with existing `color::ADDED` / `color::REMOVED`.
  - **Inline snippet (last K=5 only):** render the capped `DiffLine`s beneath the
    line — `+`/`−`/context prefixes, dim line numbers, add/del colors, and a final
    `…+N more` when `truncated_by > 0`. "Last K" = the K most-recent edit/write
    tool results in the transcript that still have a cached diff.
  - **Aged-out / cache-missing / post-reload:** the plain line exactly as today.
- Reuses existing color tokens (`color::ADDED`/`REMOVED` at `tokens.rs:105-106`);
  no new theming.

### 5. Config

A `[ui]` config block:

- `edit_diff: bool` (default `true`) — master on/off.
- `edit_diff_inline: u32` (default `5`) — the inline window `K`; `0` = counts-only
  (never inline). The per-diff line cap (20) and cache cap (16) are v1 constants.

## Error handling

- Diff computation is **best-effort and infallible from the tool's perspective**:
  any failure (unreadable pre-image, non-UTF-8) yields `diff: None` (or counts-only)
  and never affects the tool's success/`text`.
- A `ToolResult` with no cached diff renders the plain line — the exact current
  behavior — so the feature degrades to a no-op whenever data is absent.

## Testing

- **`zoid-tools`:** `edit`/`write` populate `FileDiff` with correct `added`/`removed`
  counts and correct truncation (`lines.len() <= cap`, `truncated_by` accurate) —
  pure unit tests. New-file `write` → all-additions. Pre-read failure → counts-only.
- **`zoid-tui`:** rendering a cached diff produces the colored snippet with counts;
  the (K+1)-th most-recent edit renders counts-only (no inline body); a
  cache-missing `ToolResult` renders the plain line.
- **No persistence tests** — nothing is persisted; the event log and DB are
  untouched by design.

## Non-goals (YAGNI)

- No persistence of diffs across reloads (explicitly ephemeral).
- No diffs for tools other than `edit`/`write` (e.g. `shell`, `subagent_diff`,
  `write`-adjacent tools) in v1.
- No on-zoom "full diff" expansion beyond the cap in v1 (the cap is the view).
- No syntax highlighting inside diff lines — add/del coloring only.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid-tools/Cargo.toml` | add `similar` dependency |
| `crates/zoid-tools/src/lib.rs` | `ToolOutput.diff` field; `FileDiff`/`DiffLine`/`DiffKind` types |
| `crates/zoid-tools/src/edit.rs` | compute `FileDiff` from before/after |
| `crates/zoid-tools/src/write.rs` | read pre-image; compute `FileDiff` |
| `crates/zoid/src/agent.rs` | `AgentUpdate::EditDiff`; fork at the dispatch site |
| `crates/zoid-tui/src/state.rs` | `shell.edit_diffs` bounded map (single home for the cache) |
| `crates/zoid/src/main.rs` | handle `EditDiff` → write `app.shell.edit_diffs` |
| `crates/zoid-tui/src/chat.rs` | render counts line + inline last-K snippet |
| `crates/zoid/src/config.rs` | `[ui]` `edit_diff`, `edit_diff_inline` |
