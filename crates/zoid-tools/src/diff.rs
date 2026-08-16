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
    // flattened and capped to `line_cap`. CRITICAL: keep iterating ALL changes
    // to count them; only the PUSH is guarded by the cap. (Do NOT `break` when
    // the cap is hit — that would stop counting and make `truncated_by` wrong.)
    let mut lines: Vec<DiffLine> = Vec::new();
    let mut total_changed_or_ctx = 0u32;
    for group in diff.grouped_ops(CONTEXT_RADIUS).iter() {
        for op in group {
            for ch in diff.iter_changes(op) {
                let (kind, old_no, new_no) = match ch.tag() {
                    ChangeTag::Insert => {
                        (DiffKind::Add, None, ch.new_index().map(|i| i as u32 + 1))
                    }
                    ChangeTag::Delete => {
                        (DiffKind::Del, ch.old_index().map(|i| i as u32 + 1), None)
                    }
                    ChangeTag::Equal => (
                        DiffKind::Ctx,
                        ch.old_index().map(|i| i as u32 + 1),
                        ch.new_index().map(|i| i as u32 + 1),
                    ),
                };
                total_changed_or_ctx += 1;
                if lines.len() < line_cap {
                    // `ch.value()` keeps the trailing newline; strip it for display.
                    let text = ch
                        .value()
                        .strip_suffix('\n')
                        .unwrap_or(ch.value())
                        .to_string();
                    lines.push(DiffLine {
                        old_no,
                        new_no,
                        kind,
                        text,
                    });
                }
                // No `else break`: we must keep counting past the cap so
                // `truncated_by` reflects EVERY dropped line, not just the first.
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
        assert!(d
            .lines
            .iter()
            .any(|l| l.kind == DiffKind::Add && l.text == "B"));
        assert!(d
            .lines
            .iter()
            .any(|l| l.kind == DiffKind::Del && l.text == "b"));
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
        assert!(d
            .lines
            .iter()
            .any(|l| l.kind == DiffKind::Ctx && l.text == "b"));
        assert!(d
            .lines
            .iter()
            .any(|l| l.kind == DiffKind::Ctx && l.text == "d"));
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
