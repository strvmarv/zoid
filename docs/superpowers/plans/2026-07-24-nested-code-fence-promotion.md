# Nested Code Fence Promotion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the model's output contains a bare ``` fence that contains a
```lang-tagged fence inside it, `pulldown-cmark` misinterprets the inner
close ``` as the closing fence of the outer block. A preprocessing step
promotes the outer ``` to ```` so the inner block is consumed as content.

**Algorithm:** Scan for ``` opening fences (bare or with a language tag).
When found, scan ahead for pulldown's close (the next bare ``` line — close
fences have no language tag). If between the open and close there's a line
matching ``` followed by a non-empty language tag, the block is nested —
promote the outer open (preserving its language tag) and the NEXT bare ```
after the close (the real outer close) to ````.

This heuristic avoids false positives on two adjacent separate code blocks
(the first block's content has no ```lang line inside it) and handles the
common case (the model uses language tags on inner code blocks). Bare ```
containing bare ``` (no language tag) is ambiguous and left unpromoted —
same behavior as today, not worse.

**Tech Stack:** Rust (`zoid-tui` crate). Pure string manipulation, no
parser changes.

**Spec:** `docs/superpowers/specs/2026-07-24-nested-code-fence-promotion-design.md`

## Global Constraints

- **The preprocessor is pure** — no I/O, no state, deterministic.
- **Idempotent** — after promotion to ````, the outer fence is 4 backticks;
  `is_fence_open(line, 3)` checks for exactly 3, so a second pass is a no-op.
- **No false positives on adjacent separate blocks** — two ``` blocks with
  no ```lang inside the first are not promoted.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tui/src/markdown.rs` | `promote_nested_fences` function + `render_body` integration + tests | Modify |

**Single task — all changes in one file, one commit.**

---

### Task 1: `promote_nested_fences` + `render_body` integration + tests

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`

- [ ] **Step 1: Write tests (TDD — write first, verify they fail)**

Add tests at the end of the test module in `markdown.rs` (after the
existing tests, before the closing `}`):

```rust
    // --- nested fence promotion tests ---

    #[test]
    fn promote_no_nesting_unchanged() {
        let input = "```rust\nlet x = 1;\n```\n";
        let out = super::promote_nested_fences(input);
        assert_eq!(out, input, "no inner fences -> unchanged");
    }

    #[test]
    fn promote_single_nesting() {
        // Outer bare ``` contains a ```rust inner block.
        let input = "```\nhere is a codeblock:\n\n```rust\nlet x = 1;\n```\n\ndone\n```";
        let out = super::promote_nested_fences(input);
        assert!(out.starts_with("````\n"), "outer open promoted to 4 backticks: {out:?}");
        assert!(out.ends_with("````"), "outer close promoted to 4 backticks: {out:?}");
        assert!(out.contains("```rust\nlet x = 1;\n```"), "inner fence preserved: {out:?}");
    }

    #[test]
    fn promote_language_tag_preserved() {
        // Outer ```text containing inner ```rust.
        let input = "```text\n```rust\nlet x = 1;\n```\n```";
        let out = super::promote_nested_fences(input);
        assert!(out.starts_with("````text\n"), "language tag preserved: {out:?}");
        assert!(out.ends_with("````"), "outer close promoted: {out:?}");
    }

    #[test]
    fn promote_two_separate_blocks_not_merged() {
        // Two adjacent separate code blocks — must NOT be promoted.
        let input = "```rust\nlet a = 1;\n```\n\n```rust\nlet b = 2;\n```";
        let out = super::promote_nested_fences(input);
        assert_eq!(out, input, "two separate blocks not promoted");
    }

    #[test]
    fn promote_bare_inside_bare_not_promoted() {
        // Bare ``` containing bare ``` with no lang tag — ambiguous, left alone.
        let input = "```\nsome text\n```\nmore\n```";
        let out = super::promote_nested_fences(input);
        assert_eq!(out, input, "ambiguous bare-in-bare not promoted");
    }

    #[test]
    fn promote_unterminated_fence_unchanged() {
        let input = "```rust\nlet x = 1;\n";  // no closing fence
        let out = super::promote_nested_fences(input);
        assert_eq!(out, input, "unterminated fence -> unchanged");
    }

    #[test]
    fn promote_no_false_positive_on_inline_backticks() {
        // ``` appearing in the middle of a line inside code is NOT a fence.
        let input = "```rust\nlet s = \"some ``` text\";\n```\n";
        let out = super::promote_nested_fences(input);
        assert_eq!(out, input, "inline backticks in code body are not fences");
    }

    #[test]
    fn promote_idempotent() {
        let input = "```\n```rust\nlet x = 1;\n```\n```";
        let once = super::promote_nested_fences(input);
        let twice = super::promote_nested_fences(&once);
        assert_eq!(once, twice, "idempotent");
        // Verify promotion actually happened (non-vacuous).
        assert_ne!(once, input, "promotion must change the output");
    }

    #[test]
    fn promote_renders_nested_block_correctly() {
        // End-to-end: the promoted output should render as a single code
        // block (one CodeHead), not multiple.
        let input = "```\n```rust\nlet x = 1;\n```\n```";
        let promoted = super::promote_nested_fences(input);
        let body = super::render_body(&promoted, 80);
        let code_heads: Vec<_> = body.iter()
            .filter(|b| b.kind == super::BodyKind::CodeHead)
            .collect();
        assert_eq!(code_heads.len(), 1, "one code block, not multiple");
    }
```

Run: `cargo test -p zoid-tui -- promote`
Expected: FAIL (function doesn't exist yet — compile error).

- [ ] **Step 2: Implement `promote_nested_fences`**

Add the function in `markdown.rs`, before `render_body` (around line 56):

```rust
/// Promote ``` fences to ```` when the block they open contains a ```lang
/// inner fence. This prevents `pulldown-cmark` from misinterpreting the
/// inner ``` close as the closing fence of the outer block. Pure,
/// idempotent, O(n). Only promotes when a ```lang line (a fence with a
/// non-empty language tag) appears between the outer open and pulldown's
/// first close — this avoids false positives on adjacent separate blocks.
fn promote_nested_fences(source: &str) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut output: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Is this a ``` opening fence (bare or with a language tag)?
        if let Some(lang) = is_fence_open(line) {
            // Scan ahead for pulldown's close: the next bare ``` line
            // (close fences have no language tag).
            if let Some(close_idx) = find_next_bare_fence(&lines, i + 1) {
                // Does the content between open and close contain a
                // ```lang line (fence with a non-empty language tag)?
                if has_lang_fence_between(&lines, i + 1, close_idx) {
                    // The real outer close is the NEXT bare ``` after
                    // close_idx (close_idx was the inner block's close).
                    if let Some(outer_close) = find_next_bare_fence(&lines, close_idx + 1) {
                        // Promote: outer open (preserve lang tag) and outer close to ````.
                        output.push(promote_fence_line(line, &lang));
                        for j in (i + 1)..outer_close {
                            output.push(lines[j].to_string());
                        }
                        output.push(promote_fence_line(lines[outer_close], ""));
                        i = outer_close + 1;
                        continue;
                    }
                }
            }
        }
        output.push(line.to_string());
        i += 1;
    }
    output.join("\n")
}

/// If the line is a ``` fence opening (exactly 3 backticks after up to 3
/// leading spaces, optionally followed by a language tag), returns the
/// language tag (possibly empty string for bare ```). Returns `None` if
/// the line is not a fence opening.
fn is_fence_open(line: &str) -> Option<String> {
    let leading = line.len() - line.trim_start().len();
    if leading > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let backticks: String = trimmed.chars().take_while(|c| *c == '`').collect();
    if backticks.len() != 3 {
        return None;
    }
    let rest = &trimmed[3..];
    // A fence open: the rest is empty (bare ```) or a language tag
    // (non-empty after trimming trailing whitespace).
    let tag = rest.trim_end();
    if tag.is_empty() || tag.chars().all(|c| !c.is_whitespace()) {
        return Some(tag.to_string());
    }
    None
}

/// Scan from `start` for the first line that is a bare ``` fence
/// (exactly 3 backticks, optional leading/trailing whitespace, no
/// language tag). Returns the index or `None`.
fn find_next_bare_fence(lines: &[&str], start: usize) -> Option<usize> {
    for i in start..lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.len() == 3 && trimmed.chars().all(|c| c == '`') {
            return Some(i);
        }
    }
    None
}

/// True if any line between `start` and `end` (exclusive) is a fence
/// opening with a non-empty language tag: starts with ``` (after up to
/// 3 leading spaces) followed by at least one non-backtick, non-whitespace
/// character.
fn has_lang_fence_between(lines: &[&str], start: usize, end: usize) -> bool {
    (start..end).any(|i| {
        let line = lines[i];
        let leading = line.len() - line.trim_start().len();
        if leading > 3 {
            return false;
        }
        let trimmed = line.trim_start();
        let backticks: String = trimmed.chars().take_while(|c| *c == '`').collect();
        if backticks.len() != 3 {
            return false;
        }
        let rest = trimmed[3..].trim();
        !rest.is_empty()
    })
}

/// Replace ``` (3 backticks) with ```` (4 backticks) on a fence line.
/// Preserves leading whitespace and the language tag (if any).
fn promote_fence_line(line: &str, lang: &str) -> String {
    let leading = &line[..line.len() - line.trim_start().len()];
    if lang.is_empty() {
        format!("{leading}````")
    } else {
        format!("{leading}````{lang}")
    }
}
```

Run: `cargo test -p zoid-tui -- promote`
Expected: PASS (all tests).

- [ ] **Step 3: Integrate into `render_body`**

In `render_body` (line 57), add the preprocessor call before the parser:

```rust
pub fn render_body(source: &str, content_w: usize) -> Vec<BodyLine> {
    let promoted = promote_nested_fences(source);
    let mut b = Builder::default();
    b.content_w = content_w;
    for ev in Parser::new_ext(&promoted, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        b.event(ev);
        if b.bail {
            return plain_lines(source);
        }
    }
    // ... rest unchanged (b.finish()) ...
```

Note: `plain_lines(source)` uses the ORIGINAL source (not `promoted`) so
the bail-to-plain-text fallback doesn't show ```` instead of ```.

Run: `cargo test -p zoid-tui`
Expected: PASS (all existing tests + new promotion tests).

- [ ] **Step 4: Run the full release gate**

Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "fix(tui): promote nested code fences before markdown parsing

When the model's output contains a bare ``` fence with a ```lang-tagged
fence inside it, pulldown-cmark misinterprets the inner close ``` as the
outer close. A preprocessing step in render_body detects this case (bare
``` open containing a ```lang line) and promotes the outer ``` to ````
so the inner block is consumed as content. Pure, idempotent, O(n).
Avoids false positives on adjacent separate blocks (no ```lang inside
the first block). Bail-to-plain-text fallback uses the original source."
```

---

## Self-Review

Run after the task: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
(AGENTS.md release gate). Confirm:
- `promote_nested_fences` tests pass (no nesting, single nesting, language
  tag, two separate blocks, bare-in-bare, unterminated, inline backticks,
  idempotent, end-to-end render).
- Existing markdown tests pass (no regressions — the preprocessor is a
  no-op when there's no nesting).
- `render_body` calls `promote_nested_fences` before `Parser::new_ext`.
- `plain_lines` uses the original `source`, not `promoted`.