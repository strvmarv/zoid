# TUI Edit/Write Diff Snippets — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show compact, colored add/delete diff snippets for `edit`/`write` tool calls in the zoid TUI chat, computed live and held only in memory (never persisted, never in the model's context).

**Architecture:** The `edit`/`write` tools compute a capped `FileDiff` at execution time and attach it to their (transient) `ToolOutput`. At the single tool-dispatch site the bin forwards it on a non-persisted `AgentUpdate::EditDiff`, which the bin maps into a TUI render type and stores in a bounded shell-state cache keyed by tool-call id. `chat.rs` renders a `+N −M` counts line always (while cached) and an inline capped snippet for the last K edits. Nothing touches the event log, the DB, or the model request.

**Tech Stack:** Rust, `similar` (line diffing), ratatui (TUI). Crates: `zoid-tools`, `zoid` (bin: agent.rs + main.rs), `zoid-tui`, `zoid-core` (config).

## Global Constraints

- Diffs are **ephemeral**: never written to `EventKind`, the SQLite event log, or the model request. They ride `ToolOutput.diff` (transient) and `AgentUpdate::EditDiff` (transient) only. **No `EventKind`/DB/schema change.**
- Inline window **K = 5** most-recent edits; per-diff line cap **20**; context radius **1** line; in-memory cache cap **16**.
- Scope: **`edit` and `write` tools only.**
- Diff computation is **best-effort and infallible from the tool's perspective**: any failure yields `diff: None` (or counts-only) and never changes the tool's `text`/`is_error`.
- Crate boundary: `zoid-tui` must NOT depend on `zoid-tools`. The compute-type lives in `zoid-tools`; a mirror render-type lives in `zoid-tui::state`; the bin maps between them (exactly like `AgentUpdate::SubagentStarted { id, task }` → `zoid_tui::state::SubagentRow`).
- Ships enabled (`[ui].edit_diff = true`, `edit_diff_inline = 5`).

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid-tools/Cargo.toml` | add `similar = "2"` |
| `crates/zoid-tools/src/diff.rs` (NEW) | `FileDiff`/`DiffLine`/`DiffKind` compute types + `compute_file_diff` |
| `crates/zoid-tools/src/lib.rs` | `mod diff` + re-export; `ToolOutput.diff` field + `with_diff` |
| `crates/zoid-tools/src/edit.rs` | capture before/after, populate diff on success |
| `crates/zoid-tools/src/write.rs` | read pre-image, populate diff on success |
| `crates/zoid-tui/src/state.rs` | mirror render types + `ShellState.edit_diffs` bounded cache + `push_edit_diff` |
| `crates/zoid/src/agent.rs` | `AgentUpdate::EditDiff`; fork at the dispatch site |
| `crates/zoid/src/main.rs` | handle `EditDiff` → map → `shell.push_edit_diff`; thread cache to renderer |
| `crates/zoid-tui/src/chat.rs` | render counts line + inline last-K snippet |
| `crates/zoid-core/src/config.rs` | `UiConfig { edit_diff, edit_diff_inline }` |

---

### Task 1: Diff computation core (`zoid-tools/src/diff.rs`)

**Files:**
- Modify: `crates/zoid-tools/Cargo.toml`
- Create: `crates/zoid-tools/src/diff.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add `pub mod diff;` + re-export)

**Interfaces:**
- Produces:
  - `pub struct FileDiff { pub path: String, pub added: u32, pub removed: u32, pub lines: Vec<DiffLine>, pub truncated_by: u32 }`
  - `pub struct DiffLine { pub old_no: Option<u32>, pub new_no: Option<u32>, pub kind: DiffKind, pub text: String }`
  - `pub enum DiffKind { Ctx, Add, Del }`
  - `pub fn compute_file_diff(path: &str, before: &str, after: &str, line_cap: usize) -> FileDiff`
  - Consts: `pub const INLINE_LINE_CAP: usize = 20; pub const CONTEXT_RADIUS: usize = 1;`

- [ ] **Step 1: Add the dependency**

In `crates/zoid-tools/Cargo.toml`, under `[dependencies]`, add:

```toml
similar = "2"
```

- [ ] **Step 2: Write the failing test** (create `crates/zoid-tools/src/diff.rs` with only the test module + type/fn signatures stubbed to `unimplemented!()`)

Create `crates/zoid-tools/src/diff.rs`:

```rust
//! Compact, capped line diffs for the TUI's ephemeral edit/write snippets.
//! Pure and dependency-light: `similar` line diff → a bounded `FileDiff`.
//! Never persisted; never sent to the model (see the diff-snippets spec).

/// Default number of diff lines shown inline before truncation.
pub const INLINE_LINE_CAP: usize = 20;
/// Unchanged context lines kept around each changed hunk.
pub const CONTEXT_RADIUS: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// 1-based line number in the "before" file (None for additions).
    pub old_no: Option<u32>,
    /// 1-based line number in the "after" file (None for deletions).
    pub new_no: Option<u32>,
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// Total added lines over the WHOLE diff (counted even when truncated).
    pub added: u32,
    /// Total removed lines over the whole diff.
    pub removed: u32,
    /// The (possibly truncated) rendered lines, capped to `line_cap`.
    pub lines: Vec<DiffLine>,
    /// How many `lines` were dropped by the cap (0 = whole diff shown).
    pub truncated_by: u32,
}

pub fn compute_file_diff(path: &str, before: &str, after: &str, line_cap: usize) -> FileDiff {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_adds_and_removes_over_whole_diff() {
        let before = "a\nb\nc\n";
        let after = "a\nB\nc\nd\n";
        let d = compute_file_diff("f.rs", before, after, 100);
        // one line changed (b→B) = 1 del + 1 add; one new line (d) = 1 add.
        assert_eq!(d.added, 2, "B and d are additions");
        assert_eq!(d.removed, 1, "b is a deletion");
        assert_eq!(d.path, "f.rs");
        assert_eq!(d.truncated_by, 0);
        // Add/Del lines carry the changed text.
        assert!(d.lines.iter().any(|l| l.kind == DiffKind::Add && l.text == "B"));
        assert!(d.lines.iter().any(|l| l.kind == DiffKind::Del && l.text == "b"));
    }

    #[test]
    fn new_file_is_all_additions() {
        let d = compute_file_diff("new.rs", "", "x\ny\n", 100);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 0);
        assert!(d.lines.iter().all(|l| l.kind != DiffKind::Del));
    }

    #[test]
    fn truncates_to_cap_and_reports_remainder() {
        // 10 fresh additions, cap the rendered lines at 4.
        let after: String = (0..10).map(|i| format!("line{i}\n")).collect();
        let d = compute_file_diff("big.rs", "", &after, 4);
        assert_eq!(d.added, 10, "count is over the whole diff, not the cap");
        assert_eq!(d.lines.len(), 4, "rendered lines capped");
        assert_eq!(d.truncated_by, 6, "10 changed lines - 4 shown = 6 dropped");
    }

    #[test]
    fn keeps_one_context_line_around_a_change() {
        // Change only the middle line; expect surrounding context retained.
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nb\nC\nd\ne\n";
        let d = compute_file_diff("f.rs", before, after, 100);
        // The changed hunk is around line 3; context radius 1 keeps b and d.
        assert!(d.lines.iter().any(|l| l.kind == DiffKind::Ctx && l.text == "b"));
        assert!(d.lines.iter().any(|l| l.kind == DiffKind::Ctx && l.text == "d"));
        // But far-away lines (a, e) are NOT included.
        assert!(!d.lines.iter().any(|l| l.text == "a"));
        assert!(!d.lines.iter().any(|l| l.text == "e"));
    }

    #[test]
    fn identical_inputs_yield_empty_diff() {
        let d = compute_file_diff("f.rs", "x\ny\n", "x\ny\n", 100);
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
        assert!(d.lines.is_empty());
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p zoid-tools diff:: 2>&1 | tail -20`
Expected: the tests panic with `not implemented` (or the crate fails to build until `mod diff` is declared — do Step 4's `lib.rs` line first if so).

Add to `crates/zoid-tools/src/lib.rs` near the other `pub mod` lines (after `pub mod edit;`):

```rust
pub mod diff;
```

and after the `ToolOutput` block re-export the types (near `pub use kill::KillSlot;`):

```rust
pub use diff::{compute_file_diff, DiffKind, DiffLine, FileDiff};
```

Re-run: `cargo test -p zoid-tools diff::` → still FAIL (`unimplemented!`), now compiling.

- [ ] **Step 4: Implement `compute_file_diff`**

Replace the `unimplemented!()` body in `crates/zoid-tools/src/diff.rs`:

```rust
pub fn compute_file_diff(path: &str, before: &str, after: &str, line_cap: usize) -> FileDiff {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(before, after);

    // Total counts over the WHOLE diff (independent of the render cap).
    let mut added = 0u32;
    let mut removed = 0u32;
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    // Rendered lines: grouped hunks with CONTEXT_RADIUS context lines,
    // flattened and capped to `line_cap`.
    let mut lines: Vec<DiffLine> = Vec::new();
    let mut total_changed_or_ctx = 0u32;
    'hunks: for group in diff.grouped_ops(CONTEXT_RADIUS).iter() {
        for op in group {
            for ch in diff.iter_changes(op) {
                let (kind, old_no, new_no) = match ch.tag() {
                    ChangeTag::Insert => (
                        DiffKind::Add,
                        None,
                        ch.new_index().map(|i| i as u32 + 1),
                    ),
                    ChangeTag::Delete => (
                        DiffKind::Del,
                        ch.old_index().map(|i| i as u32 + 1),
                        None,
                    ),
                    ChangeTag::Equal => (
                        DiffKind::Ctx,
                        ch.old_index().map(|i| i as u32 + 1),
                        ch.new_index().map(|i| i as u32 + 1),
                    ),
                };
                total_changed_or_ctx += 1;
                if lines.len() < line_cap {
                    // `ch.value()` keeps the trailing newline; strip it for display.
                    let text = ch.value().strip_suffix('\n').unwrap_or(ch.value()).to_string();
                    lines.push(DiffLine { old_no, new_no, kind, text });
                } else {
                    // Cap reached; stop collecting but we already have full counts.
                    break 'hunks;
                }
            }
        }
    }

    let truncated_by = total_changed_or_ctx.saturating_sub(lines.len() as u32);

    FileDiff {
        path: path.to_string(),
        added,
        removed,
        lines,
        truncated_by,
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zoid-tools diff::`
Expected: all 5 tests PASS.

Note on the truncation test: `truncated_by` counts *rendered* lines dropped (changed + context) beyond `line_cap`, not the add/remove totals. The `truncates_to_cap_and_reports_remainder` test uses an all-additions input (no context), so rendered-line count == added count == 10, and `truncated_by == 10 - 4 == 6`.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/Cargo.toml crates/zoid-tools/src/diff.rs crates/zoid-tools/src/lib.rs
git commit -m "feat(tools): capped line-diff core (compute_file_diff)"
```

---

### Task 2: `ToolOutput.diff` + edit/write populate it

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs:33-51` (`ToolOutput` + impl)
- Modify: `crates/zoid-tools/src/edit.rs:72-88`
- Modify: `crates/zoid-tools/src/write.rs:28-41`

**Interfaces:**
- Consumes: `compute_file_diff`, `FileDiff`, `INLINE_LINE_CAP` (Task 1).
- Produces: `ToolOutput.diff: Option<FileDiff>`; `ToolOutput::with_diff(self, FileDiff) -> Self`. `edit`/`write` success outputs carry a `FileDiff`; all other tools and all error paths leave `diff: None`.

- [ ] **Step 1: Write the failing test** (append to `crates/zoid-tools/src/edit.rs` tests module)

```rust
    #[test]
    fn successful_edit_carries_a_file_diff() {
        let (_d, path) = seed("alpha beta gamma");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "beta", "new_string": "BETA" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        let diff = out.diff.expect("edit success must carry a diff");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
    }

    #[test]
    fn failed_edit_has_no_diff() {
        let (_d, path) = seed("hello");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "zzz", "new_string": "y" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.diff.is_none(), "error path carries no diff");
    }
```

And append to `crates/zoid-tools/src/write.rs` tests module:

```rust
    #[test]
    fn write_of_new_file_carries_all_additions_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap(), "content": "a\nb\n" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        let diff = out.diff.expect("write success must carry a diff");
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
    }

    #[test]
    fn overwrite_diffs_against_prior_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, "a\nb\n").unwrap();
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap(), "content": "a\nB\n" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        let diff = out.diff.expect("write success must carry a diff");
        assert_eq!(diff.added, 1, "B is added");
        assert_eq!(diff.removed, 1, "b is removed");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tools -- successful_edit_carries write_of_new_file overwrite_diffs failed_edit_has_no_diff 2>&1 | tail -20`
Expected: FAIL — `ToolOutput` has no field `diff`.

- [ ] **Step 3: Add the `diff` field + `with_diff`**

In `crates/zoid-tools/src/lib.rs`, change the `ToolOutput` struct and impl:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    /// Ephemeral, UI-only diff for file-editing tools (edit/write). Never
    /// persisted and never sent to the model — the bin forwards it on a
    /// non-persisted `AgentUpdate` and drops it here. `None` for every other
    /// tool and every error path.
    pub diff: Option<diff::FileDiff>,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            diff: None,
        }
    }
    pub fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            diff: None,
        }
    }
    /// Attach an ephemeral UI diff to a successful output.
    pub fn with_diff(mut self, diff: diff::FileDiff) -> Self {
        self.diff = Some(diff);
        self
    }
}
```

- [ ] **Step 4: Populate in `edit.rs`**

In `crates/zoid-tools/src/edit.rs`, capture the pre-image and diff on success. Replace lines 72-88 (from `let full = ...` through the final `match std::fs::write(...)` block):

```rust
        let full = crate::resolve(cwd, &path);
        let before = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("edit({path}): {e}")),
        };
        let mut contents = before.clone();
        // Apply all edits in memory; bail (writing nothing) on the first failure.
        for (i, (old, new, replace_all)) in edits.iter().enumerate() {
            match apply_one(&contents, old, new, *replace_all) {
                Ok(updated) => contents = updated,
                Err(msg) => return ToolOutput::err(format!("edit({path}) edit #{}: {msg}", i + 1)),
            }
        }
        match std::fs::write(&full, contents.as_bytes()) {
            Ok(()) => {
                let fd = crate::compute_file_diff(&path, &before, &contents, crate::diff::INLINE_LINE_CAP);
                ToolOutput::ok(format!("edited {path} ({} change(s))", edits.len())).with_diff(fd)
            }
            Err(e) => ToolOutput::err(format!("edit({path}): {e}")),
        }
```

- [ ] **Step 5: Populate in `write.rs`**

In `crates/zoid-tools/src/write.rs`, read the pre-image (best-effort) and diff on success. Replace the `run` body's final `match` (lines 37-40):

```rust
        let full = crate::resolve(cwd, &path);
        // Best-effort pre-image for the ephemeral diff; a new/unreadable file
        // is treated as empty (all-additions).
        let before = std::fs::read_to_string(&full).unwrap_or_default();
        match std::fs::write(&full, content.as_bytes()) {
            Ok(()) => {
                let fd = crate::compute_file_diff(&path, &before, &content, crate::diff::INLINE_LINE_CAP);
                ToolOutput::ok(format!("wrote {} bytes to {path}", content.len())).with_diff(fd)
            }
            Err(e) => ToolOutput::err(format!("write({path}): {e}")),
        }
```

(Note: this replaces the single `match std::fs::write(crate::resolve(cwd, &path), ...)` call — resolve once into `full` and reuse it.)

- [ ] **Step 6: Run tests to verify they pass + whole-crate build**

Run: `cargo test -p zoid-tools 2>&1 | tail -15`
Expected: all zoid-tools tests PASS (the new ones + all existing — `ToolOutput::ok/err` still compile everywhere since `diff` defaults via those constructors).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/lib.rs crates/zoid-tools/src/edit.rs crates/zoid-tools/src/write.rs
git commit -m "feat(tools): edit/write attach an ephemeral FileDiff to ToolOutput"
```

---

### Task 3: TUI render types + bounded shell cache (`zoid-tui/src/state.rs`)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add types near `SubagentRow` at :131; add field to `ShellState` at :172+; init in `Default`/constructor at :434 area)

**Interfaces:**
- Produces (all in `zoid_tui::state`):
  - `pub struct RenderDiff { pub path: String, pub added: u32, pub removed: u32, pub lines: Vec<RenderDiffLine>, pub truncated_by: u32 }`
  - `pub struct RenderDiffLine { pub old_no: Option<u32>, pub new_no: Option<u32>, pub kind: RenderDiffKind, pub text: String }`
  - `pub enum RenderDiffKind { Ctx, Add, Del }`
  - `ShellState.edit_diffs: Vec<(String, RenderDiff)>` — insertion-ordered bounded cache (id → diff).
  - `pub const EDIT_DIFF_CACHE_CAP: usize = 16;`
  - `impl ShellState { pub fn push_edit_diff(&mut self, id: String, diff: RenderDiff); pub fn edit_diff(&self, id: &str) -> Option<&RenderDiff>; }`
- Note: mirror types (not `zoid_tools::FileDiff`) keep `zoid-tui` free of a `zoid-tools` dependency (crate-boundary constraint). The bin maps between them (Task 4).

- [ ] **Step 1: Write the failing test** (append to `crates/zoid-tui/src/state.rs` tests module — find the existing `#[cfg(test)] mod tests`)

```rust
    #[test]
    fn edit_diff_cache_is_bounded_and_evicts_oldest() {
        let mut s = ShellState::default();
        for i in 0..(EDIT_DIFF_CACHE_CAP + 3) {
            s.push_edit_diff(
                format!("id{i}"),
                RenderDiff { path: "f".into(), added: 1, removed: 0, lines: vec![], truncated_by: 0 },
            );
        }
        assert_eq!(s.edit_diffs.len(), EDIT_DIFF_CACHE_CAP, "cache is capped");
        assert!(s.edit_diff("id0").is_none(), "oldest evicted");
        assert!(s.edit_diff(&format!("id{}", EDIT_DIFF_CACHE_CAP + 2)).is_some(), "newest kept");
    }

    #[test]
    fn edit_diff_reinsert_updates_in_place_without_growth() {
        let mut s = ShellState::default();
        let mk = |a| RenderDiff { path: "f".into(), added: a, removed: 0, lines: vec![], truncated_by: 0 };
        s.push_edit_diff("x".into(), mk(1));
        s.push_edit_diff("x".into(), mk(9));
        assert_eq!(s.edit_diffs.len(), 1, "same id updates, does not duplicate");
        assert_eq!(s.edit_diff("x").unwrap().added, 9);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui edit_diff_ 2>&1 | tail -20`
Expected: FAIL — `RenderDiff`, `EDIT_DIFF_CACHE_CAP`, `push_edit_diff`, `edit_diff` undefined.

- [ ] **Step 3: Add the types** (in `crates/zoid-tui/src/state.rs`, after the `SubagentRow` struct at :131-134)

```rust
/// Ephemeral, UI-only diff cap for the in-memory edit/write snippet cache.
pub const EDIT_DIFF_CACHE_CAP: usize = 16;

/// Render-side mirror of `zoid_tools::FileDiff` (kept here so `zoid-tui` needn't
/// depend on `zoid-tools`; the bin maps between them, like SubagentRow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderDiffKind {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderDiffLine {
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub kind: RenderDiffKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderDiff {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub lines: Vec<RenderDiffLine>,
    pub truncated_by: u32,
}
```

- [ ] **Step 4: Add the field + methods**

In the `ShellState` struct (near `subagent_rows` at :258) add:

```rust
    /// Ephemeral, UI-only edit/write diffs, keyed by tool-call id in insertion
    /// order (bounded to `EDIT_DIFF_CACHE_CAP`, oldest evicted). Populated by the
    /// bin from `AgentUpdate::EditDiff`; NOT persisted, empty after reload.
    pub edit_diffs: Vec<(String, RenderDiff)>,
```

In `ShellState`'s `Default`/constructor (near `subagent_rows: Vec::new(),` at :434) add:

```rust
            edit_diffs: Vec::new(),
```

Add the methods in an `impl ShellState` block (place beside the other `ShellState` methods; if there is no `impl ShellState`, add one after the struct):

```rust
impl ShellState {
    /// Insert or update an ephemeral edit diff, keeping the cache bounded to
    /// `EDIT_DIFF_CACHE_CAP` (oldest evicted). Re-inserting an existing id
    /// updates it in place without growing the cache.
    pub fn push_edit_diff(&mut self, id: String, diff: RenderDiff) {
        if let Some(slot) = self.edit_diffs.iter_mut().find(|(k, _)| *k == id) {
            slot.1 = diff;
            return;
        }
        self.edit_diffs.push((id, diff));
        if self.edit_diffs.len() > EDIT_DIFF_CACHE_CAP {
            self.edit_diffs.remove(0);
        }
    }

    /// Look up a cached diff by tool-call id.
    pub fn edit_diff(&self, id: &str) -> Option<&RenderDiff> {
        self.edit_diffs.iter().find(|(k, _)| k == id).map(|(_, d)| d)
    }
}
```

(If an `impl ShellState` block already exists, add the two methods inside it instead of creating a second block.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui edit_diff_`
Expected: both tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): bounded in-memory edit-diff cache in shell state"
```

---

### Task 4: `AgentUpdate::EditDiff` + fork at dispatch + bin handler

**Files:**
- Modify: `crates/zoid/src/agent.rs:228-290` (add `AgentUpdate` variant) and `:2016-2033` (fork at the dispatch site)
- Modify: `crates/zoid/src/main.rs` (handle the new variant near the `SubagentStarted` arm at ~:2859)

**Interfaces:**
- Consumes: `zoid_tools::FileDiff`/`DiffLine`/`DiffKind` (Task 1–2); `zoid_tui::state::{RenderDiff, RenderDiffLine, RenderDiffKind}` + `ShellState::push_edit_diff` (Task 3).
- Produces: `AgentUpdate::EditDiff { id: String, diff: zoid_tools::FileDiff }`; a `fn map_render_diff(zoid_tools::FileDiff) -> zoid_tui::state::RenderDiff` in `main.rs`.
- **Atomicity:** the `AgentUpdate` enum is matched exhaustively in `main.rs`; the variant and its handler MUST land in the same task or the bin won't compile.

- [ ] **Step 1: Add the variant** (in `crates/zoid/src/agent.rs`, in `pub enum AgentUpdate`, after `SubagentStarted { id: String, task: String },` at :243)

```rust
    /// An ephemeral, UI-only diff for an edit/write tool call. Carries the
    /// computed `FileDiff` to the TUI's in-memory cache; never persisted and
    /// never sent to the model. Keyed by tool-call id.
    EditDiff {
        id: String,
        diff: zoid_tools::FileDiff,
    },
```

- [ ] **Step 2: Fork at the dispatch site** (in `crates/zoid/src/agent.rs`, the Local-tool arm around :2000-2033)

Change `let out = tokio::select! {` (line ~2000) to `let mut out = tokio::select! {`, then between the `tool_fail_msg` line (:2018) and the `emit(` call (:2019) insert the fork:

```rust
                    let tool_ok = !out.is_error;
                    let tool_fail_msg = out.is_error.then(|| out.text.clone());
                    // Ephemeral UI-only diff (edit/write). Sent BEFORE the emit
                    // (which moves tc.id/out.text). Never persisted; the
                    // ToolResult event below still stores only out.text.
                    if let Some(diff) = out.diff.take() {
                        let _ = ui
                            .send(AgentUpdate::EditDiff { id: tc.id.clone(), diff })
                            .await;
                    }
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: out.text,
                            is_error: out.is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
```

(Only two changes vs. today: `let mut out`, and the `if let Some(diff) = out.diff.take()` block. `tool_ok`/`tool_fail_msg` and the `emit` are unchanged.)

- [ ] **Step 3: Add the mapping + handler in the bin** (in `crates/zoid/src/main.rs`)

Add a free function near the other `AgentUpdate` handling helpers (module scope):

```rust
/// Map the tool-side `FileDiff` into the TUI's render mirror (keeps zoid-tui
/// free of a zoid-tools dependency; mirrors SubagentStarted → SubagentRow).
fn map_render_diff(d: zoid_tools::FileDiff) -> zoid_tui::state::RenderDiff {
    use zoid_tui::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    RenderDiff {
        path: d.path,
        added: d.added,
        removed: d.removed,
        truncated_by: d.truncated_by,
        lines: d
            .lines
            .into_iter()
            .map(|l| RenderDiffLine {
                old_no: l.old_no,
                new_no: l.new_no,
                kind: match l.kind {
                    zoid_tools::DiffKind::Ctx => RenderDiffKind::Ctx,
                    zoid_tools::DiffKind::Add => RenderDiffKind::Add,
                    zoid_tools::DiffKind::Del => RenderDiffKind::Del,
                },
                text: l.text,
            })
            .collect(),
    }
}
```

In the `match update { … }` block over `AgentUpdate` (the `SubagentStarted` arm is at ~:2859), add:

```rust
                    AgentUpdate::EditDiff { id, diff } => {
                        app.shell.push_edit_diff(id, map_render_diff(diff));
                    }
```

- [ ] **Step 4: Write a test for the mapping** (append to `crates/zoid/src/main.rs` tests module)

```rust
    #[test]
    fn map_render_diff_preserves_counts_and_kinds() {
        let fd = zoid_tools::FileDiff {
            path: "f.rs".into(),
            added: 3,
            removed: 1,
            truncated_by: 2,
            lines: vec![
                zoid_tools::DiffLine { old_no: Some(1), new_no: Some(1), kind: zoid_tools::DiffKind::Ctx, text: "a".into() },
                zoid_tools::DiffLine { old_no: None, new_no: Some(2), kind: zoid_tools::DiffKind::Add, text: "b".into() },
                zoid_tools::DiffLine { old_no: Some(3), new_no: None, kind: zoid_tools::DiffKind::Del, text: "c".into() },
            ],
        };
        let r = map_render_diff(fd);
        assert_eq!((r.added, r.removed, r.truncated_by), (3, 1, 2));
        assert_eq!(r.lines.len(), 3);
        assert!(matches!(r.lines[1].kind, zoid_tui::state::RenderDiffKind::Add));
        assert!(matches!(r.lines[2].kind, zoid_tui::state::RenderDiffKind::Del));
    }
```

- [ ] **Step 5: Build the workspace + run the mapping test**

Run: `cargo build --workspace 2>&1 | grep -E "^error" | head; echo done`
Expected: no `error` lines (the exhaustive `AgentUpdate` match now handles `EditDiff`).

Run: `cargo test -p zoid --bin zoid map_render_diff_preserves`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(agent): forward ephemeral edit diffs to the TUI cache"
```

---

### Task 5: Render the counts line + inline last-K snippet (`zoid-tui/src/chat.rs`)

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (`conversation_lines` :41, `build_conversation` signature + the `ChatMsg::ToolResult` arm at :224-252)
- Modify: `crates/zoid/src/main.rs` + `crates/zoid-tui/src/render.rs` (call sites that pass args into `conversation_lines`/`build_conversation`)

**Interfaces:**
- Consumes: `ShellState.edit_diffs` / `edit_diff` (Task 3).
- Produces: `build_conversation` and `conversation_lines` gain a parameter `edit_diffs: &[(String, RenderDiff)]` and an `inline_k: usize`. The `ChatMsg::ToolResult` arm shows `· +A −B` counts (when the id is cached) and, for the last `inline_k` cached edit/write results, an inline capped snippet.
- Constant: `pub const DEFAULT_INLINE_K: usize = 5;` in `chat.rs`.

- [ ] **Step 1: Write the failing test** (append to `crates/zoid-tui/src/chat.rs` tests module)

```rust
    #[test]
    fn tool_result_renders_counts_and_inline_snippet_for_cached_edit() {
        use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
        use zoid_core::projection::ChatMsg;

        let msgs = vec![ChatMsg::ToolResult {
            id: "tc1".into(),
            name: "edit".into(),
            output: "edited f.rs (1 change(s))".into(),
            is_error: false,
            compacted: false,
            ts: 0,
        }];
        let diff = RenderDiff {
            path: "f.rs".into(),
            added: 2,
            removed: 1,
            truncated_by: 0,
            lines: vec![
                RenderDiffLine { old_no: Some(2), new_no: None, kind: RenderDiffKind::Del, text: "b".into() },
                RenderDiffLine { old_no: None, new_no: Some(2), kind: RenderDiffKind::Add, text: "B".into() },
            ],
        };
        let cache = vec![("tc1".to_string(), diff)];
        let lines = conversation_lines_with_diffs(&msgs, false, false, 0, 80, None, &cache, 5);
        let text: String = lines.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.as_ref()).collect();
        assert!(text.contains("+2"), "shows added count");
        assert!(text.contains("-1") || text.contains("−1"), "shows removed count");
        assert!(text.contains("B"), "shows the added line inline");
        assert!(text.contains('b'), "shows the removed line inline");
    }

    #[test]
    fn cached_edit_beyond_k_shows_counts_only_no_snippet() {
        use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
        use zoid_core::projection::ChatMsg;

        let mk_res = |id: &str| ChatMsg::ToolResult {
            id: id.into(), name: "edit".into(), output: "edited".into(),
            is_error: false, compacted: false, ts: 0,
        };
        let mk_diff = |marker: &str| RenderDiff {
            path: "f".into(), added: 1, removed: 0, truncated_by: 0,
            lines: vec![RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: marker.into() }],
        };
        // Two edits; K=1 → only the LAST is inline.
        let msgs = vec![mk_res("old"), mk_res("new")];
        let cache = vec![("old".to_string(), mk_diff("OLDLINE")), ("new".to_string(), mk_diff("NEWLINE"))];
        let lines = conversation_lines_with_diffs(&msgs, false, false, 0, 80, None, &cache, 1);
        let text: String = lines.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.as_ref()).collect();
        assert!(text.contains("NEWLINE"), "last edit is inline");
        assert!(!text.contains("OLDLINE"), "older edit is counts-only, no snippet");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui tool_result_renders_counts cached_edit_beyond_k 2>&1 | tail -20`
Expected: FAIL — `conversation_lines_with_diffs` undefined.

- [ ] **Step 3: Thread the cache through the builder**

In `crates/zoid-tui/src/chat.rs`, add the constant near the top of the module:

```rust
/// Default number of most-recent edit/write results shown with an inline diff.
pub const DEFAULT_INLINE_K: usize = 5;
```

Add a new public wrapper alongside `conversation_lines` (:41) and keep the old one delegating (so existing callers/tests that don't have a cache still compile):

```rust
/// Like `conversation_lines`, but with the ephemeral edit-diff cache and the
/// inline-K window. `conversation_lines` delegates here with an empty cache.
#[allow(clippy::too_many_arguments)]
pub fn conversation_lines_with_diffs(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
    edit_diffs: &[(String, crate::state::RenderDiff)],
    inline_k: usize,
) -> Vec<Line<'static>> {
    let mut hits = Vec::new();
    build_conversation(
        msgs, streaming, caret_on, tz_offset_secs, width,
        &mut hits, &mut Vec::new(), &mut Vec::new(), question,
        edit_diffs, inline_k,
    )
}
```

Change the existing `conversation_lines` body to delegate:

```rust
pub fn conversation_lines(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    conversation_lines_with_diffs(
        msgs, streaming, caret_on, tz_offset_secs, width, question, &[], 0,
    )
}
```

Add the two parameters to `build_conversation`'s signature (find `fn build_conversation(`), appending `edit_diffs: &[(String, crate::state::RenderDiff)], inline_k: usize,`. Update any OTHER internal callers of `build_conversation` (e.g. the detail path) to pass `edit_diffs, inline_k` through, or `&[], 0` where a cache isn't available.

- [ ] **Step 4: Compute the inline set + render** (in `build_conversation`)

Just before the main `for`/`match` over `msgs`, precompute which ids get an inline snippet (the last `inline_k` edit/write ToolResults that have a cached diff, in document order):

```rust
    // Ids of the last `inline_k` edit/write results that have a cached diff.
    let inline_ids: std::collections::HashSet<&str> = {
        let mut cached: Vec<&str> = Vec::new();
        for m in msgs {
            if let ChatMsg::ToolResult { id, name, is_error: false, .. } = m {
                if (name == "edit" || name == "write")
                    && edit_diffs.iter().any(|(k, _)| k == id)
                {
                    cached.push(id.as_str());
                }
            }
        }
        let start = cached.len().saturating_sub(inline_k);
        cached[start..].iter().copied().collect()
    };
    let find_diff = |id: &str| edit_diffs.iter().find(|(k, _)| k == id).map(|(_, d)| d);
```

Replace the `ChatMsg::ToolResult` arm (:224-252) so it destructures `id` and appends counts + optional snippet:

```rust
            ChatMsg::ToolResult {
                id,
                name,
                output,
                is_error,
                compacted,
                ..
            } => {
                let (mark, mark_color) = if *is_error {
                    (glyph::WARNING, color::ERROR)
                } else {
                    (glyph::PASS, color::OK)
                };
                let mut spans = vec![Span::styled(
                    format!("  {mark} "),
                    Style::new().fg(mark_color),
                )];
                if *compacted {
                    spans.push(Span::styled(
                        format!("{} compacted ", glyph::COMPACT),
                        Style::new().fg(color::DIM),
                    ));
                }
                spans.push(Span::styled(name.clone(), Style::new().fg(color::DIM)));
                // Ephemeral edit/write diff, if cached: counts on the line …
                let diff = (!*is_error).then(|| find_diff(id.as_str())).flatten();
                if let Some(d) = diff {
                    spans.push(Span::styled(
                        format!(" · +{} ", d.added),
                        Style::new().fg(color::ADDED),
                    ));
                    spans.push(Span::styled(
                        format!("−{}", d.removed),
                        Style::new().fg(color::REMOVED),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!(" → {}", first_line(output)),
                        Style::new().fg(color::DIM),
                    ));
                }
                lines.push(Line::from(spans));

                // … and an inline snippet for the last-K cached edits.
                if let Some(d) = diff {
                    if inline_ids.contains(id.as_str()) {
                        for dl in &d.lines {
                            let (sign, col) = match dl.kind {
                                crate::state::RenderDiffKind::Add => ("+", color::ADDED),
                                crate::state::RenderDiffKind::Del => ("−", color::REMOVED),
                                crate::state::RenderDiffKind::Ctx => (" ", color::DIM),
                            };
                            let no = dl.new_no.or(dl.old_no).unwrap_or(0);
                            lines.push(Line::from(vec![
                                Span::styled(format!("      {no:>5} "), Style::new().fg(color::DIM)),
                                Span::styled(format!("{sign} {}", dl.text), Style::new().fg(col)),
                            ]));
                        }
                        if d.truncated_by > 0 {
                            lines.push(Line::from(vec![Span::styled(
                                format!("      …+{} more", d.truncated_by),
                                Style::new().fg(color::DIM),
                            )]));
                        }
                    }
                }
            }
```

- [ ] **Step 5: Run the rendering tests**

Run: `cargo test -p zoid-tui tool_result_renders_counts cached_edit_beyond_k`
Expected: both PASS.

- [ ] **Step 6: Wire the real call site** (in `crates/zoid/src/main.rs` / `crates/zoid-tui/src/render.rs`)

Find where `conversation_lines(` is called to render the live chat (search `conversation_lines(`), and switch that call to `conversation_lines_with_diffs(…, &app.shell.edit_diffs, zoid_tui::chat::DEFAULT_INLINE_K)`. (Leave the modal/`render_shell` path on the plain `conversation_lines` — it can pass an empty cache; only the primary chat view needs the inline diffs.)

- [ ] **Step 7: Build the workspace**

Run: `cargo build --workspace 2>&1 | grep -E "^error" | head; echo done`
Expected: no `error` lines.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid/src/main.rs crates/zoid-tui/src/render.rs
git commit -m "feat(tui): render +N -M counts and inline last-K edit diffs"
```

---

### Task 6: Config `[ui]` — `edit_diff` on/off + `edit_diff_inline` (K)

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (mirror the `EconomyConfig`/`reassert_interval_tokens` 6-site pattern for a new `UiConfig`)
- Modify: `crates/zoid/src/main.rs` (the chat call site from Task 5 uses the config values)

**Interfaces:**
- Consumes: `Config.ui` (new).
- Produces: `pub struct UiConfig { pub edit_diff: bool, pub edit_diff_inline: u32 }` with defaults `true` / `5`; wired through `Config`, `PartialConfig`/`PartialUi`, `Provenance`, and the merge. The chat call site passes `if cfg.ui.edit_diff { cfg.ui.edit_diff_inline as usize } else { 0 }` as `inline_k` (0 disables inline; counts still show while cached — or gate counts too if `edit_diff == false`, see Step 4).

- [ ] **Step 1: Write the failing test** (append to `crates/zoid-core/src/config.rs` tests module)

The config is layered via the free function `merge(layers: &[(Source, PartialConfig)]) -> (Config, Provenance)`, and partials are built from TOML via `parse_toml` — serde deserializes each section, so a new `[ui]` table needs no parser change, only a `PartialUi` field on `PartialConfig`. Mirror the existing `merge_unions_source_dirs_across_layers` test style (config.rs:532):

```rust
    #[test]
    fn ui_config_defaults_edit_diff_on_and_k_five() {
        let c = UiConfig::default();
        assert!(c.edit_diff, "edit diffs ship enabled");
        assert_eq!(c.edit_diff_inline, 5);
    }

    #[test]
    fn merge_applies_ui_overrides() {
        let (p, _) = parse_toml("[ui]\nedit_diff = false\nedit_diff_inline = 2").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert!(!cfg.ui.edit_diff);
        assert_eq!(cfg.ui.edit_diff_inline, 2);
    }

    #[test]
    fn ui_defaults_when_section_absent() {
        let (p, _) = parse_toml("[economy]\nrecent_n = 3").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert!(cfg.ui.edit_diff, "absent [ui] → default on");
        assert_eq!(cfg.ui.edit_diff_inline, 5);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core ui_config_defaults partial_ui_overrides 2>&1 | tail -20`
Expected: FAIL — `UiConfig`/`Config.ui`/`PartialConfig.ui` undefined.

- [ ] **Step 3: Add `UiConfig` following the `EconomyConfig` pattern** (in `crates/zoid-core/src/config.rs`)

Mirror every site where `EconomyConfig`/`reassert_interval_tokens` appears (struct + Default at :77/:103, `Config.economy` at :32, `Provenance` at :211, `PartialEconomy`/`PartialConfig.economy` at :226/:278, provenance-init at :312, merge block at :355). Concretely add:

```rust
// Near EconomyConfig:
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConfig {
    /// Master switch for the ephemeral edit/write diff snippets.
    pub edit_diff: bool,
    /// How many most-recent edits show an inline snippet (0 = counts only).
    pub edit_diff_inline: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { edit_diff: true, edit_diff_inline: 5 }
    }
}
```

- Add `pub ui: UiConfig,` to `Config` (beside `pub economy: EconomyConfig,` at :32) and `ui: UiConfig::default(),` to `Config`'s `Default`.
- Add a `PartialUi` beside `PartialEconomy`, deriving EXACTLY like the sibling partials (e.g. `PartialApproval` at :263) so serde parses `[ui]`:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialUi {
    pub edit_diff: Option<bool>,
    pub edit_diff_inline: Option<u32>,
}
```

  and add `pub ui: PartialUi,` to `PartialConfig` (:271-285), beside `pub economy: PartialEconomy,`.
- Add `pub ui_edit_diff: Source,` and `pub ui_edit_diff_inline: Source,` to `Provenance` and init them `Source::Default` in its constructor (beside `reassert_interval_tokens: Source::Default,` at :312).
- In the merge fn (beside :355), add:

```rust
        if let Some(v) = p.ui.edit_diff {
            cfg.ui.edit_diff = v;
            prov.ui_edit_diff = *src;
        }
        if let Some(v) = p.ui.edit_diff_inline {
            cfg.ui.edit_diff_inline = v;
            prov.ui_edit_diff_inline = *src;
        }
```

- [ ] **Step 4: Gate rendering on the config** (in `crates/zoid/src/main.rs`)

At the Task 5 chat call site, compute `inline_k` from config and pass it:

```rust
    let inline_k = if app.config.ui.edit_diff {
        app.config.ui.edit_diff_inline as usize
    } else {
        0
    };
```

and pass `inline_k` to `conversation_lines_with_diffs`. When `edit_diff` is `false`, also skip populating the cache so counts don't show either — in the `AgentUpdate::EditDiff` handler (Task 4), guard:

```rust
                    AgentUpdate::EditDiff { id, diff } => {
                        if app.config.ui.edit_diff {
                            app.shell.push_edit_diff(id, map_render_diff(diff));
                        }
                    }
```

- [ ] **Step 5: Run tests + workspace build**

Run: `cargo test -p zoid-core ui_config_defaults partial_ui_overrides && cargo build --workspace 2>&1 | grep -E "^error" | head; echo done`
Expected: config tests PASS; no `error` lines.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/config.rs crates/zoid/src/main.rs
git commit -m "feat(config): [ui] edit_diff toggle + edit_diff_inline (K)"
```

---

## Notes for the implementer

- **Subagent edits.** A subagent running `edit`/`write` also emits `EditDiff` on the shared `ui` channel, so a few subagent diffs may land in the cache. They never render: subagent `ToolResult`s live on the subagent branch and are folded to a `Delegated` card, never appearing as a main-conversation `ChatMsg::ToolResult`. The bounded cache evicts them harmlessly. No special-casing needed in v1.
- **`−` vs `-`.** The counts/sign use the Unicode minus `−` (U+2212) to match the mockups; tests accept either `-` or `−`.
- **Detail zoom** (`detail_lines`, chat.rs:701) is unchanged — inline diffs are a normal-zoom affordance only (per spec non-goals).
