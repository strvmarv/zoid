# Width-Aware Tool Call & Result Truncation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hardcoded 30-char (args) and 40-char (result) truncation limits with width-aware budgets: `min(available_text_width - fixed_overhead, 120)`, truncating the whole args/result string as a unit.

**Architecture:** Three functions change in `chat.rs`: `scalar` (no longer truncates), `arg_summary` (truncates the joined string to a budget), `first_line` (truncates to a budget). The `build_conversation` call sites for tool-call lines and result lines compute the budget from `ctx.width` and pass it through. The `Delegated` arm's `first_line` call also gets a budget. A `display_width` helper is added for measuring tool/result names.

**Tech Stack:** Rust, ratatui, `unicode_width::UnicodeWidthStr` (already imported in `chat.rs`), `zoid-tui` crate.

## Global Constraints

- `truncate(s: &str, max: usize) -> String` is in `crate::text` and already imported in `chat.rs`. It handles the ellipsis and display-width logic. Do not modify it.
- `UnicodeWidthStr::width` is already imported in `chat.rs` (line 14).
- `ctx.width` in `RenderCtx` is the conversation text width (already padding-adjusted via `conv_text_width`).
- The `⏎ peek` hint is left as-is — it's a non-functional visual hint, out of scope.
- The diff preview path (edit/write results showing `+N −N`) is unchanged — only the `first_line` fallback path gets the width budget.
- The cap is `min(budget, 120)`.

---

### Task 1: Add `display_width` helper

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (add helper near the other private functions, ~line 1069)
- Test: `crates/zoid-tui/src/chat.rs` (test in the existing test module)

**Interfaces:**
- Produces: `fn display_width(s: &str) -> usize` — returns the display width of a string using `UnicodeWidthStr::width`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn display_width_measures_correctly() {
        assert_eq!(display_width("shell"), 5);
        assert_eq!(display_width("update_tasks"), 12);
        assert_eq!(display_width(""), 0);
        // Wide char (fullwidth) counts as 2 columns.
        assert_eq!(display_width("中"), 2);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib chat::tests::display_width_measures_correctly`
Expected: FAIL — `display_width` not defined.

- [ ] **Step 3: Add the helper**

Add near the other private functions (e.g. just before `fn arg_summary` at line ~1069):

```rust
/// Display width of a string (column count, handling wide glyphs).
/// Used to compute the fixed overhead of a tool-call/result line so the
/// args/preview budget can be derived from the available text width.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib chat::tests::display_width_measures_correctly`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "refactor: add display_width helper for width-aware truncation"
```

---

### Task 2: Remove truncation from `scalar`, add width budget to `arg_summary`

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:1069-1086` (`arg_summary` and `scalar`)
- Test: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Produces: `fn arg_summary(args_json: &str, max_width: usize) -> String` — builds the joined key-value string, then truncates the whole thing to `max_width`.
- Produces: `fn scalar(v: &serde_json::Value) -> String` — returns the full string (no truncation).

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
    #[test]
    fn scalar_returns_full_string_no_truncation() {
        let long = "a".repeat(100);
        assert_eq!(scalar(&serde_json::Value::String(long.clone())), long);
        assert_eq!(
            scalar(&serde_json::json!(42)),
            "42"
        );
    }

    #[test]
    fn arg_summary_short_string_large_budget_no_truncation() {
        let json = r#"{"command": "ls -la"}"#;
        let result = arg_summary(json, 120);
        assert_eq!(result, "command: ls -la");
    }

    #[test]
    fn arg_summary_long_string_budget_60_truncates() {
        let long = "a".repeat(200);
        let json = format!(r#"{{"command": "{long}"}}"#);
        let result = arg_summary(&json, 60);
        // truncate produces at most `max` display columns. For ASCII content,
        // that's 59 chars + 1 ellipsis glyph (… = 1 display col, 3 bytes).
        assert!(UnicodeWidthStr::width(result.as_str()) <= 60,
            "result must fit in 60 display cols: got {}", UnicodeWidthStr::width(result.as_str()));
        assert!(result.starts_with("command: a"), "must start with the key and value: {result}");
        assert!(result.ends_with('…'), "must end with ellipsis: {result}");
    }

    #[test]
    fn arg_summary_multi_arg_truncates_as_unit() {
        let json = r#"{"path": "src/main.rs", "old": "fn foo() { return 1; }", "new": "fn foo() { return 2; }"}"#;
        let result = arg_summary(json, 40);
        // The whole joined string is truncated as a unit to 40.
        assert!(result.starts_with("path: src/main.rs"), "first arg visible: {result}");
        // The later args are cut off.
        assert!(!result.contains("return 2"), "later args truncated: {result}");
        assert!(result.ends_with('…'), "must end with ellipsis: {result}");
    }

    #[test]
    fn arg_summary_budget_zero_returns_empty() {
        let json = r#"{"command": "ls"}"#;
        let result = arg_summary(json, 0);
        assert_eq!(result, "");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib chat::tests::scalar_returns_full_string_no_truncation chat::tests::arg_summary`
Expected: FAIL — `scalar` still truncates to 30, `arg_summary` takes one arg.

- [ ] **Step 3: Modify `scalar` to remove truncation**

Replace the `scalar` function (line ~1081):

```rust
// BEFORE:
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 30),
        other => truncate(&other.to_string(), 30),
    }
}

// AFTER:
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
```

- [ ] **Step 4: Modify `arg_summary` to accept and use `max_width`**

Replace the `arg_summary` function (line ~1069):

```rust
// BEFORE:
fn arg_summary(args_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    }
}

// AFTER:
fn arg_summary(args_json: &str, max_width: usize) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let inner = match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    };
    truncate(&inner, max_width)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib chat::tests::scalar_returns_full_string_no_truncation chat::tests::arg_summary`
Expected: PASS

- [ ] **Step 6: Fix the call site in `build_conversation`**

In the `ChatMsg::Assistant` arm, the tool-call rendering (line ~279), change:

```rust
// BEFORE:
                for tc in tool_calls {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {} ", glyph::EDIT),
                            Style::new().fg(color::CHAT_ACCENT),
                        ),
                        Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
                        Span::styled(
                            format!("({})", arg_summary(&tc.args)),
                            Style::new().fg(color::DIM),
                        ),
                        Span::styled(
                            format!(" {} peek", glyph::RETURN),
                            Style::new().fg(color::DIM),
                        ),
                    ]));
                }

// AFTER:
                for tc in tool_calls {
                    let name_w = display_width(&tc.name);
                    let args_budget = ctx.width.saturating_sub(15 + name_w).min(120);
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {} ", glyph::EDIT),
                            Style::new().fg(color::CHAT_ACCENT),
                        ),
                        Span::styled(tc.name.clone(), Style::new().fg(color::TXT).bold()),
                        Span::styled(
                            format!("({})", arg_summary(&tc.args, args_budget)),
                            Style::new().fg(color::DIM),
                        ),
                        Span::styled(
                            format!(" {} peek", glyph::RETURN),
                            Style::new().fg(color::DIM),
                        ),
                    ]));
                }
```

Note: `15` = `  ● ` (4) + `(` (1) + `) ⏎ peek` (10). The `name_w` is the display width of the tool name. The `let` bindings go before the `Line::from(vec![...])` call.

- [ ] **Step 7: Run the full test suite to check for compile errors**

Run: `cargo test -p zoid-tui`
Expected: PASS — all tests pass with the new signature.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat: width-aware arg_summary truncation (cap min(available, 120))"
```

---

### Task 3: Add width budget to `first_line` and update all call sites

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:1088-1090` (`first_line` function)
- Modify: `crates/zoid-tui/src/chat.rs:326` (ToolResult `first_line` call)
- Modify: `crates/zoid-tui/src/chat.rs:365` (Delegated `first_line` call)
- Test: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Produces: `fn first_line(s: &str, max_width: usize) -> String` — takes the first line and truncates to `max_width`.

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
    #[test]
    fn first_line_short_output_large_budget_no_truncation() {
        let result = first_line("hello world", 120);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn first_line_long_output_budget_80_truncates() {
        let long = "a".repeat(200);
        let result = first_line(&long, 80);
        assert!(UnicodeWidthStr::width(result.as_str()) <= 80,
            "result must fit in 80 display cols: got {}", UnicodeWidthStr::width(result.as_str()));
        assert!(result.ends_with('…'), "must end with ellipsis: {result}");
    }

    #[test]
    fn first_line_multiline_takes_only_first_line() {
        let result = first_line("first line\nsecond line", 120);
        assert_eq!(result, "first line");
    }

    #[test]
    fn first_line_empty_returns_empty() {
        let result = first_line("", 120);
        assert_eq!(result, "");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib chat::tests::first_line`
Expected: FAIL — `first_line` takes one arg, not two.

- [ ] **Step 3: Modify `first_line`**

Replace the `first_line` function (line ~1088):

```rust
// BEFORE:
fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 40)
}

// AFTER:
fn first_line(s: &str, max_width: usize) -> String {
    truncate(s.lines().next().unwrap_or(""), max_width)
}
```

- [ ] **Step 4: Update the ToolResult call site**

In the `ChatMsg::ToolResult` arm (line ~326), change:

```rust
// BEFORE:
                    } else {
                        spans.push(Span::styled(
                            format!(" → {}", first_line(output)),
                            Style::new().fg(color::DIM),
                        ));
                    }

// AFTER:
                    } else {
                        let name_w = display_width(name);
                        let mut overhead = 7 + name_w;
                        if *compacted {
                            overhead += 12; // approximate width of "{glyph} compacted "
                        }
                        let result_budget = ctx.width.saturating_sub(overhead).min(120);
                        spans.push(Span::styled(
                            format!(" → {}", first_line(output, result_budget)),
                            Style::new().fg(color::DIM),
                        ));
                    }
```

Note: `7` = `  ✓ ` (4) + ` → ` (3). The `12` is an approximation of the `compacted` prefix width (`{glyph} compacted `). The spec allows this approximation since the budget is a soft cap.

- [ ] **Step 5: Update the Delegated call site**

In the `ChatMsg::Delegated` arm (line ~365), change:

```rust
// BEFORE:
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok {
                    (glyph::PASS, color::OK)
                } else {
                    (glyph::WARNING, color::ERROR)
                };
                lines.push(Line::from(vec![
                    // Purple label with the card background = the collapsed chip.
                    Span::styled(
                        format!("{} delegated · {}", glyph::COLLAPSED, first_line(summary)),
                        Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG),
                    ),
                    Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                    Span::styled(
                        format!("{} peek", glyph::RETURN),
                        Style::new().fg(color::DIM),
                    ),
                ]));
            }

// AFTER:
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok {
                    (glyph::PASS, color::OK)
                } else {
                    (glyph::WARNING, color::ERROR)
                };
                let delegated_prefix_w = 1 + display_width(" delegated · ");
                let summary_budget = ctx.width.saturating_sub(delegated_prefix_w).min(120);
                lines.push(Line::from(vec![
                    // Purple label with the card background = the collapsed chip.
                    Span::styled(
                        format!("{} delegated · {}", glyph::COLLAPSED, first_line(summary, summary_budget)),
                        Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG),
                    ),
                    Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                    Span::styled(
                        format!("{} peek", glyph::RETURN),
                        Style::new().fg(color::DIM),
                    ),
                ]));
            }
```

Note: `glyph::COLLAPSED` is a `char` (1 display column), so the prefix width is `1 + display_width(" delegated · ")`. The `let` bindings go before the `Line::from(vec![...])` call.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib chat::tests::first_line`
Expected: PASS

- [ ] **Step 7: Run the full test suite**

Run: `cargo test -p zoid-tui`
Expected: PASS — no regressions. Existing tests that called `first_line` with one arg have been updated at the call sites.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat: width-aware first_line truncation (cap min(available, 120))"
```

---

### Task 4: Integration-level verification tests

**Files:**
- Test: `crates/zoid-tui/src/chat.rs`

**Interfaces:**
- Consumes: `build_conversation` / `conversation_view` (the render path that now uses width-aware budgets)

- [ ] **Step 1: Write integration tests**

Add the `join_spans` helper (if not already present) and these tests to the test module:

```rust
    fn join_spans(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>()
    }

    #[test]
    fn tool_call_line_wide_terminal_shows_full_short_command() {
        // At width 111, a short shell command should NOT be truncated.
        let msgs = vec![ChatMsg::Assistant {
            thinking: None,
            text: String::new(),
            tool_calls: vec![ToolCallRef {
                id: "tc1".into(),
                name: "shell".into(),
                args: r#"{"command": "cd /home/gomanjoe/source/zoid && cargo build"}"#.into(),
            }],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 111, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("cd /home/gomanjoe/source/zoid && cargo build"),
            "short command must be fully visible at width 111: {joined}"
        );
        assert!(
            !joined.contains('…'),
            "no truncation when command fits in budget: {joined}"
        );
    }

    #[test]
    fn tool_call_line_narrow_terminal_truncates_long_command() {
        // At width 40, a long command should be truncated.
        let long_cmd = "a".repeat(200);
        let msgs = vec![ChatMsg::Assistant {
            thinking: None,
            text: String::new(),
            tool_calls: vec![ToolCallRef {
                id: "tc1".into(),
                name: "shell".into(),
                args: format!(r#"{{"command": "{long_cmd}}}"#),
            }],
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 40, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains('…'),
            "long command must be truncated at narrow width: {joined}"
        );
    }

    #[test]
    fn result_line_wide_terminal_shows_full_output() {
        // At width 111, a short result should NOT be truncated.
        let msgs = vec![ChatMsg::ToolResult {
            id: "tc1".into(),
            name: "shell".into(),
            output: "   Compiling zoid-core v0.5.0 (/home/gomanjoe/source/zoid)".into(),
            is_error: false,
            compacted: false,
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 111, None);
        let joined = join_spans(&lines);
        assert!(
            joined.contains("Compiling zoid-core v0.5.0 (/home/gomanjoe/source/zoid)"),
            "short result must be fully visible at width 111: {joined}"
        );
    }

    #[test]
    fn result_line_capped_at_120_on_very_wide_terminal() {
        // At width 200, the budget should be capped at 120, not the full ~193.
        let long_output = "b".repeat(200);
        let msgs = vec![ChatMsg::ToolResult {
            id: "tc1".into(),
            name: "shell".into(),
            output: long_output,
            is_error: false,
            compacted: false,
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 200, None);
        let joined = join_spans(&lines);
        // The output should be truncated — 200 chars won't fit in a 120 cap.
        assert!(
            joined.contains('…'),
            "very long output must be truncated even at width 200 (cap 120): {joined}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib chat::tests::tool_call_line chat::tests::result_line`
Expected: PASS

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p zoid-tui`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "test: integration tests for width-aware tool call and result truncation"
```