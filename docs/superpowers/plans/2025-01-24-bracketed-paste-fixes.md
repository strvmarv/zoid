# Bracketed Paste + Two Small Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix multi-line paste, tool indicator truncation, and subagent model override 404 in one pass.

**Architecture:** Three independent fixes touching three different files. No shared state between them; order doesn't matter except all three are committed before the plan is done.

**Tech Stack:** Rust, crossterm 0.29, ratatui, zoid-tools

**Spec:** `docs/superpowers/specs/2025-01-24-bracketed-paste-design.md`

## Global Constraints

- crossterm 0.29 (already a dependency; `EnableBracketedPaste`, `DisableBracketedPaste`, `Event::Paste` all in scope)
- Best-effort terminal escapes (same `let _ =` / `execute!` pattern as the kitty keyboard flags)
- Subagents must always inherit the session's provider + model — no LLM-chosen overrides

---

### Task 1: Bracketed Paste

**Files:**
- Modify: `crates/zoid/src/main.rs` (imports, startup, teardown, event loop)

**Interfaces:**
- Consumes: `crossterm::event::{EnableBracketedPaste, DisableBracketedPaste}` (already in the `crossterm::event` module; add to the existing `use` list)
- Produces: none (self-contained)

- [ ] **Step 1: Add imports**

In `crates/zoid/src/main.rs`, add `EnableBracketedPaste` and `DisableBracketedPaste` to the existing crossterm event imports at the top of the file:

```rust
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
        EnableMouseCapture, Event as CEvent, EventStream,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
```

- [ ] **Step 2: Enable at startup**

Find the line (~line 1422):

```rust
execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
```

Replace with:

```rust
execute!(
    out,
    EnterAlternateScreen,
    EnableMouseCapture,
    EnableBracketedPaste
)?;
```

- [ ] **Step 3: Disable on exit**

Find the cleanup block (~line 1450):

```rust
let _ = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    LeaveAlternateScreen
);
```

Replace with:

```rust
let _ = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    DisableBracketedPaste,
    LeaveAlternateScreen
);
```

- [ ] **Step 4: Handle Event::Paste in the event loop**

Find the `term_events.next()` match block (~line 1797). Before the `Some(Ok(_))` catch-all arm, add a paste arm:

```rust
Some(Ok(CEvent::Paste(text))) => {
    app.textarea.insert_text(text);
}
```

The arm sits between the `CEvent::Mouse` arm and the `Some(Ok(_))` catch-all:

```rust
Some(Ok(CEvent::Mouse(me))) => {
    // ... existing mouse handling ...
}
Some(Ok(CEvent::Paste(text))) => {
    app.textarea.insert_text(text);
}
Some(Ok(_)) => { /* resize: redraw next loop */ }
```

- [ ] **Step 5: Build and test**

Run: `cargo test -p zoid 2>&1 | grep -E "test result|FAILED"`
Expected: all tests pass (existing tests don't cover paste — this is mechanical wiring).

Run: `cargo clippy -p zoid 2>&1 | grep error`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: bracketed paste — multi-line paste as a single message"
```

---

### Task 2: Tool Indicator Dynamic Width

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (the `render_status` function, ~line 360-395)

**Interfaces:**
- Consumes: `right_start` (already computed), `spans` vec (already built up to the tool insertion point)
- Produces: none (pure rendering change)

- [ ] **Step 1: Remove the fixed TOOL_SLOT constant and its pad/truncate block**

Find (~line 364-394) the `const TOOL_SLOT` declaration and the entire block that pads or truncates `tool_text` to it:

```rust
const TOOL_SLOT: usize = 12; // "◐ shell …" + padding
```

...and the block:

```rust
let tool_text = {
    let tw = tool_text.width();
    if tw < TOOL_SLOT {
        format!("{}{}", tool_text, " ".repeat(TOOL_SLOT - tw))
    } else if tw > TOOL_SLOT {
        let mut chars: Vec<char> = tool_text.chars().collect();
        while chars.iter().collect::<String>().width() > TOOL_SLOT && chars.len() > 2 {
            chars.remove(1);
        }
        chars.into_iter().collect::<String>()
    } else {
        tool_text
    }
};
let tool_w = tool_text.width();
```

Delete both of those. The `tool_w` variable is unused after this change (clippy already warned about it).

- [ ] **Step 2: Add dynamic cap before the tool span is pushed**

After the 4-space gap span is pushed (the line `spans.push(Span::styled(" ".repeat(4), Style::new()));` that precedes the tool), compute the available width and truncate `tool_text` to it:

```rust
// Fixed 4-space gap between working and tool (symmetric with compact gap).
spans.push(Span::styled(" ".repeat(4), Style::new()));
// Tool indicator right of "working" (4-space gap), always present.
// The tool indicator is on the outside (right of "working"), so it has
// room to grow toward the right edge. Cap it to the available space
// rather than a fixed 12-char slot, so long tool names expand instead of
// truncating against the glyph.
let consumed_before_tool: usize = spans.iter().map(|s| s.content.width()).sum();
let tool_cap = right_start
    .saturating_sub(consumed_before_tool)
    .saturating_sub(1) // 1-char padding before the zoom hint
    .max(8); // floor: keep the glyph + a few chars even on a narrow screen
let tool_text = {
    let tw = tool_text.width();
    if tw > tool_cap {
        let mut chars: Vec<char> = tool_text.chars().collect();
        while chars.iter().collect::<String>().width() > tool_cap && chars.len() > 2 {
            chars.remove(1); // trim after the glyph
        }
        chars.into_iter().collect::<String>()
    } else {
        tool_text
    }
};
spans.push(Span::styled(tool_text, Style::new().fg(tool_fg)));
```

Note: the original code pushed `tool_text.clone()` — change to just `tool_text` (no longer needs cloning since the cap happens inline).

- [ ] **Step 3: Build and test**

Run: `cargo test -p zoid-tui 2>&1 | grep -E "test result|FAILED"`
Expected: all tests pass except the pre-existing `compaction_segment_visible_when_compacting` failure (unrelated).

Run: `cargo clippy -p zoid-tui 2>&1 | grep error`
Expected: no errors (the `tool_w` unused variable warning goes away).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "fix: tool indicator expands into available space instead of truncating at 12 chars"
```

---

### Task 3: Remove Subagent Model Override

**Files:**
- Modify: `crates/zoid-tools/src/subagent_dispatch.rs` (tool spec)
- Modify: `crates/zoid/src/agent.rs` (remove model_override extraction)
- Test: `crates/zoid-tools/src/subagent_dispatch.rs` (update test assertion)

**Interfaces:**
- Consumes: `model` parameter of `run_agent_turn_cancellable` (already in scope)
- Produces: `spawn_subagent` always receives the session model (no override path)

- [ ] **Step 1: Update the test to assert no model property**

In `crates/zoid-tools/src/subagent_dispatch.rs`, find the test `dispatch_subagent_spec_and_kind` (~line 44). Replace the assertion that checks `model` is an object:

```rust
assert!(params["properties"]["model"].is_object());
```

with:

```rust
assert!(
    params["properties"].get("model").is_none(),
    "model must not be in the dispatch_subagent spec — subagents inherit the session model"
);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tools dispatch_subagent_spec_and_kind 2>&1 | tail -10`
Expected: FAIL — the spec still has the `model` property.

- [ ] **Step 3: Remove the model property from the tool spec**

In `crates/zoid-tools/src/subagent_dispatch.rs`, find the `parameters` json in the `spec()` method (~line 19). Remove the `model` line:

Before:
```rust
"worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false },
"model": { "type": "string", "description": "Model override; omit to inherit the session model" }
```

After:
```rust
"worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false }
```

(Note: remove the trailing comma after the `worktree` line too, since `model` was the last property.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tools dispatch_subagent_spec_and_kind 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Remove model_override extraction in agent.rs**

In `crates/zoid/src/agent.rs`, find the `dispatch_subagent` handler (~line 882). Remove the `model_override` extraction:

```rust
let model_override = tc
    .args
    .get("model")
    .and_then(|v| v.as_str())
    .map(String::from);
```

And change the `spawn_subagent` call (~line 921) from:

```rust
model_override.unwrap_or_else(|| model.clone()),
```

to:

```rust
model.clone(),
```

- [ ] **Step 6: Build and test**

Run: `cargo test -p zoid-tools -p zoid 2>&1 | grep -E "test result|FAILED"`
Expected: all tests pass.

Run: `cargo clippy -p zoid -p zoid-tools 2>&1 | grep error`
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/subagent_dispatch.rs crates/zoid/src/agent.rs
git commit -m "fix: remove dispatch_subagent model param — subagents inherit session model"
```

---

### Task 4: Final Verification

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED"`
Expected: all pass except the pre-existing `compaction_segment_visible_when_compacting` failure.

- [ ] **Step 2: Release build**

Run: `cargo build --release -p zoid 2>&1 | tail -3`
Expected: `Finished` with no errors.

- [ ] **Step 3: Clippy clean**

Run: `cargo clippy --workspace 2>&1 | grep error`
Expected: no errors.