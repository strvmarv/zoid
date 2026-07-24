# Reduce Thinking Output in Normal Zoom — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the per-turn "▾ Thinking…" marker line at Normal zoom with a dim `·thinking` badge appended to the last rendered line of the turn.

**Architecture:** Single-file change in `chat.rs` — remove the thinking marker block from the `ChatMsg::Assistant` arm of `build_conversation`, and append a badge span to `lines.last_mut()` after the turn's text and tool calls are rendered. No new functions, no new types, no changes to the projection or agent layers.

**Tech Stack:** Rust, ratatui (`Line`, `Span`, `Style`), `zoid-tui` crate.

## Global Constraints

- The `truncate` function in `text.rs` is the project's display-width-aware truncation utility — already used by `scalar` and `first_line`.
- Colors come from `crate::tokens::color` (e.g. `color::DIM`).
- `Span<'static>` is the ratatui span type used throughout `chat.rs`.
- Tests use `conversation_lines` / `conversation_view` helpers already defined in `chat.rs` tests.

---

### Task 1: Remove the thinking marker and add the inline badge

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:238-250` (remove thinking marker block) and `crates/zoid-tui/src/chat.rs:287` (add badge after tool-call loop)
- Test: `crates/zoid-tui/src/chat.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `ChatMsg::Assistant { thinking: Option<String>, text: String, tool_calls: Vec<ToolCallRef>, ts: i64 }` from `zoid_core::projection`
- Produces: No new public interfaces. The change is internal to `build_conversation`.

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `chat.rs`, after the existing tests (before the closing `}` of the module). Use the existing `conversation_lines` helper:

```rust
    fn join_spans(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>()
    }

    #[test]
    fn thinking_badge_on_text_only_turn() {
        // A text-only assistant turn with thinking → badge on the last line.
        let msgs = vec![ChatMsg::Assistant {
            thinking: Some("reasoning here".into()),
            text: "Here is the answer.".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("·thinking"),
            "badge must appear when thinking is present: {joined}"
        );
        // No standalone "Thinking…" marker line.
        assert!(
            !joined.contains("Thinking…"),
            "standalone thinking marker must not appear: {joined}"
        );
    }

    #[test]
    fn thinking_badge_on_tool_call_only_turn() {
        // A turn with tool calls but no text + thinking → badge on last tool-call line.
        let msgs = vec![ChatMsg::Assistant {
            thinking: Some("reasoning here".into()),
            text: String::new(),
            tool_calls: vec![ToolCallRef {
                id: "tc1".into(),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
            }],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("·thinking"),
            "badge must appear on a tool-call turn: {joined}"
        );
        assert!(
            !joined.contains("Thinking…"),
            "standalone thinking marker must not appear: {joined}"
        );
    }

    #[test]
    fn thinking_badge_on_text_plus_tool_calls() {
        // A turn with both text and tool calls + thinking → badge on last tool-call line.
        let msgs = vec![ChatMsg::Assistant {
            thinking: Some("reasoning here".into()),
            text: "Let me read the file.".into(),
            tool_calls: vec![ToolCallRef {
                id: "tc1".into(),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
            }],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("·thinking"),
            "badge must appear: {joined}"
        );
        // The badge should be on the tool-call line (the last line of the turn).
        // Search for the tool glyph (●) which only appears on tool-call lines,
        // not in the assistant text "Let me read the file."
        let tool_line = lines.iter().find(|l| {
            l.spans.iter().any(|s| s.content.contains('●'))
        });
        assert!(
            tool_line.is_some_and(|l| {
                l.spans.iter().any(|s| s.content.contains("·thinking"))
            }),
            "badge must be on the tool-call line: {joined}"
        );
    }

    #[test]
    fn no_thinking_no_badge() {
        // A turn with no thinking → no badge anywhere.
        let msgs = vec![ChatMsg::Assistant {
            thinking: None,
            text: "Hello.".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined = join_spans(&lines);
        assert!(
            !joined.contains("·thinking"),
            "no badge when thinking is None: {joined}"
        );
        assert!(
            !joined.contains("Thinking…"),
            "no standalone marker when thinking is None: {joined}"
        );
    }

    #[test]
    fn empty_thinking_string_no_badge() {
        // thinking = Some("") → no badge (empty guard).
        let msgs = vec![ChatMsg::Assistant {
            thinking: Some(String::new()),
            text: "Hello.".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined = join_spans(&lines);
        assert!(
            !joined.contains("·thinking"),
            "no badge for empty thinking string: {joined}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib chat::tests`
Expected: FAIL — the `·thinking` badge doesn't exist yet, and the `Thinking…` marker is still present.

- [ ] **Step 3: Remove the thinking marker block**

In `build_conversation`, inside the `ChatMsg::Assistant { text, tool_calls, ts, thinking }` arm, **remove** the thinking marker block (currently at lines ~238–250):

```rust
// REMOVE THIS ENTIRE BLOCK:
                // Thinking marker (collapsed at Normal zoom).
                if let Some(thinking_text) = thinking {
                    if !thinking_text.is_empty() {
                        blank_between_turns(&mut lines);
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("{} ", glyph::EXPANDED),
                                Style::new().fg(color::DIM),
                            ),
                            Span::styled("Thinking…", Style::new().fg(color::DIM)),
                        ]));
                    }
                }
```

- [ ] **Step 4: Add the badge after the tool-call loop**

In the same `ChatMsg::Assistant` arm, after the `for tc in tool_calls { ... }` loop (currently ending at line ~287), add:

```rust
                // Append a dim "·thinking" badge to the last rendered line of
                // this turn if reasoning was present. Replaces the old
                // standalone "▾ Thinking…" marker line — saves one line per
                // thinking turn.
                let has_thinking = thinking.as_ref().is_some_and(|t| !t.is_empty());
                if has_thinking {
                    if let Some(last) = lines.last_mut() {
                        last.spans.push(Span::styled(
                            " ·thinking".to_string(),
                            Style::new().fg(color::DIM),
                        ));
                    }
                }
```

This must go after the `for tc in tool_calls { ... }` loop and before the closing `}` of the `ChatMsg::Assistant` arm.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib chat::tests`
Expected: PASS — all new thinking-badge tests pass, and existing tests still pass.

- [ ] **Step 6: Verify the full test suite**

Run: `cargo test -p zoid-tui`
Expected: PASS — no regressions.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat: replace thinking marker line with inline ·thinking badge at Normal zoom"
```

---

### Task 2: Verify Detail zoom is unchanged

**Files:**
- Test: `crates/zoid-tui/src/chat.rs` (add a Detail-zoom test)

**Interfaces:**
- Consumes: `detail_lines` function in `chat.rs` (renders the full thinking section at Detail zoom)
- Produces: No code changes — verification only.

- [ ] **Step 1: Write a Detail-zoom thinking test**

Add this test to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn detail_zoom_still_shows_full_thinking_section() {
        // Detail zoom must still render the full thinking text under the
        // "─ Thinking ─" separator — the badge change is Normal-zoom only.
        let msgs = vec![ChatMsg::Assistant {
            thinking: Some("I need to consider the tradeoffs.".into()),
            text: "Here is the answer.".into(),
            tool_calls: vec![],
            ts: 0,
        }];
        let view = ChatView {
            zoom: crate::state::Zoom::Detail,
            caret_on: false,
            reveal: None,
            tz_offset_secs: 0,
        };
        let lines = conversation_view(&msgs, &view, false, 80, None, &[], 0);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("Thinking"),
            "Detail zoom must show the thinking section header: {joined}"
        );
        assert!(
            joined.contains("I need to consider the tradeoffs."),
            "Detail zoom must show the full thinking text: {joined}"
        );
    }
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --lib chat::tests::detail_zoom_still_shows_full_thinking_section`
Expected: PASS — Detail zoom is unchanged by the badge change.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "test: verify Detail zoom thinking section is unchanged by badge change"
```