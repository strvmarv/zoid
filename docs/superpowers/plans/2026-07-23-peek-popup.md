# Peek Popup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `⏎ peek` hint real — clicking a tool-call line or delegated result chip at Normal zoom opens a scrollable popup overlay showing the full untruncated tool call args and result output (or subagent summary).

**Architecture:** A `PeekHit` side-output collected during `build_conversation` (same pattern as `CodeHit` / `QuestionChoiceHit`) maps rendered lines to tool calls / delegated chips. A `PeekState` on `ShellState` (separate from `Overlay` — different mouse semantics) holds the popup content and scroll. A `peek` rect on `ShellLayout` centers the popup at 65% of conversation height. `render_peek_overlay` draws a bordered, scrollable panel. Routing: `ConversationClick` checks peek hits and opens the popup; Esc and click-away dismiss; mouse wheel / arrows scroll the popup content.

**Tech Stack:** Rust, ratatui (`Frame`, `Block`, `Paragraph`, `Clear`, `Rect`, `Line`, `Span`, `Style`, `Borders`, `Margin`, `Layout`, `Constraint`), `zoid-core` (`ChatMsg`, `ToolCallRef`), `zoid-tui` / `zoid` crates.

**Spec:** `docs/superpowers/specs/2026-07-23-peek-popup-design.md`

## Global Constraints

- `ShellState` derives `#[derive(Debug, Clone, PartialEq, Eq)]` at `crates/zoid-tui/src/state.rs:388` — adding a field automatically includes it in equality. No manual `eq_fast`/`eq_full`.
- `ShellState::new()` is at `crates/zoid-tui/src/state.rs:608`; `Default` delegates to `new()` at line 861.
- `build_conversation` is at `crates/zoid-tui/src/chat.rs:165` and takes 5 args today: `msgs`, `ctx`, `hits: &mut Vec<CodeHit>`, `msg_starts: &mut Vec<usize>`, `question_choices: &mut Vec<QuestionChoiceHit>`. The plan adds a 6th: `peek_hits: &mut Vec<PeekHit>`.
- There are exactly 4 callers of `build_conversation`: `conversation_lines_with_diffs` (line 60), `code_hits` (line 91), `question_choice_hits` (line 121), `conversation_view_indexed` (line 740). All must be updated with the new parameter.
- `centered(area, w, h)` is `pub(crate)` at `crates/zoid-tui/src/layout.rs:235` — clamps w/h to area, centers horizontally, positions at 1/3 from top.
- `in_rect(r, col, row)` is `pub` at `crates/zoid-tui/src/layout.rs:98`.
- `ShellLayout` is at `crates/zoid-tui/src/layout.rs:85` and is constructed in exactly 2 places: the "too small" early return (line 108) and the normal return (line 221). Both must add the `peek` field.
- `route_key` is at `crates/zoid-tui/src/route.rs:222`. The question-card check is step 0 (line 227); the overlay block is step 1 (line 232). Peek goes between them.
- `route_mouse` is at `crates/zoid-tui/src/route.rs:598`. The overlay mouse guard is at line 608. Peek goes before it.
- `handle_conversation_click` is at `crates/zoid/src/main.rs:906`. It already calls `conversation(app.events.iter())` to get `Vec<ChatMsg>`.
- `handle_action` is at `crates/zoid/src/main.rs:3620`. The last arm before `Action::Noop => {}` (line 4766) is where new actions go.
- Mouse events are dispatched at `crates/zoid/src/main.rs:2783`. `ConversationClick` is resolved in-place (line 2791) because it needs `layout`. `PeekClick` follows the same pattern.
- `glyph::RETURN` is `⏎` at `crates/zoid-tui/src/tokens.rs:19`.
- Colors come from `crate::tokens::color` (e.g. `color::DIM`, `color::ERROR`, `color::TXT`, `color::CHAT_ACCENT`).
- Glyphs come from `crate::tokens::glyph` (e.g. `glyph::PASS`, `glyph::WARNING`, `glyph::COLLAPSED`).
- Tests for `zoid-tui` run with `cargo test -p zoid-tui --lib`.
- Tests for the `zoid` binary run with `cargo test -p zoid --bin zoid`. `test_app()` is at `crates/zoid/src/main.rs:7291`.
- Cross-crate builds: `cargo build --workspace` and `cargo test --workspace`.
- No co-author trailer in commits (repo `CLAUDE.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tui/src/chat.rs` | `PeekHit`, `PeekKind`, `peek_hits()`, `build_conversation` side-output | Modify |
| `crates/zoid-tui/src/state.rs` | `PeekState`, `PeekContent`, `peek` field on `ShellState` | Modify |
| `crates/zoid-tui/src/layout.rs` | `peek` rect on `ShellLayout`, computed in `compute()` | Modify |
| `crates/zoid-tui/src/render.rs` | `render_peek_overlay`, call site in `render_shell` | Modify |
| `crates/zoid-tui/src/route.rs` | `DismissPeek` / `ScrollPeek` / `PeekClick` actions, peek checks in `route_key` + `route_mouse` | Modify |
| `crates/zoid/src/main.rs` | peek-hit check in `handle_conversation_click`, action handlers, `PeekClick` dispatch | Modify |

**Task dependency:** T1 (chat.rs types + hits) → T2 (state.rs) → T3 (layout.rs) → T4 (render.rs) → T5 (route.rs) → T6 (main.rs). T2 depends on T1 (imports `PeekKind`). T3 depends on T2 (reads `state.peek`). T4 depends on T2+T3 (reads `state.peek`, uses `layout.peek`). T5 depends on T2 (reads `state.peek`). T6 depends on T1+T5 (uses `peek_hits`, handles new actions). Recommended linear order T1…T6. Every task builds `cargo build --workspace` because changes are cross-crate.

---

### Task 1: Hit-collection layer — `PeekHit`, `PeekKind`, `peek_hits()`

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (add types after `QuestionChoiceHit` at ~line 35; add `peek_hits()` after `question_choice_hits()` at ~line 146; add `peek_hits` param to `build_conversation` at line 165; push hits in the `Assistant` tool-call loop at ~line 258 and the `Delegated` arm at ~line 372; update all 4 callers at lines 60, 91, 121, 740)
- Test: `crates/zoid-tui/src/chat.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `ChatMsg`, `ToolCallRef` from `zoid_core::projection`; `RenderCtx` (existing struct in the same file)
- Produces: `pub struct PeekHit { pub line: usize, pub kind: PeekKind }`; `pub enum PeekKind { ToolCall { id, name, args }, Delegated { summary, ok } }`; `pub fn peek_hits(msgs, streaming, caret_on, tz_offset_secs, width, question) -> Vec<PeekHit>`

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `chat.rs`, after the existing tests (before the closing `}` of the module). Use the existing `conversation_lines` helper and the `seeded()` / `view()` helpers already defined in the test module:

```rust
    #[test]
    fn peek_hits_finds_tool_call_line() {
        let msgs = vec![
            ChatMsg::User { text: "hello".into(), ts: 0 },
            ChatMsg::Assistant {
                text: "let me check".into(),
                tool_calls: vec![ToolCallRef {
                    id: "tc1".into(),
                    name: "shell".into(),
                    args: r#"{"command":"ls"}"#.into(),
                }],
                ts: 100,
                thinking: None,
            },
        ];
        let hits = peek_hits(&msgs, false, true, 0, 80, None);
        assert_eq!(hits.len(), 1);
        assert!(matches!(
            &hits[0].kind,
            PeekKind::ToolCall { id, name, .. } if id == "tc1" && name == "shell"
        ));
        // The hit line should correspond to the tool-call rendered line.
        // The assistant text "let me check" is on one line (stamped), then
        // the tool call line follows. So the hit line is >= 1.
        assert!(hits[0].line >= 1);
    }

    #[test]
    fn peek_hits_finds_delegated_chip() {
        let msgs = vec![
            ChatMsg::User { text: "do the thing".into(), ts: 0 },
            ChatMsg::Delegated { summary: "all done".into(), ok: true },
        ];
        let hits = peek_hits(&msgs, false, true, 0, 80, None);
        assert_eq!(hits.len(), 1);
        assert!(matches!(
            &hits[0].kind,
            PeekKind::Delegated { summary, ok } if summary == "all done" && *ok
        ));
    }

    #[test]
    fn peek_hits_empty_for_prose_only() {
        let msgs = vec![
            ChatMsg::User { text: "hello".into(), ts: 0 },
            ChatMsg::Assistant {
                text: "hi there".into(),
                tool_calls: vec![],
                ts: 100,
                thinking: None,
            },
        ];
        let hits = peek_hits(&msgs, false, true, 0, 80, None);
        assert!(hits.is_empty());
    }

    #[test]
    fn peek_hits_multiple_tool_calls_each_get_own_hit() {
        let msgs = vec![
            ChatMsg::Assistant {
                text: String::new(),
                tool_calls: vec![
                    ToolCallRef { id: "a".into(), name: "read".into(), args: "{}".into() },
                    ToolCallRef { id: "b".into(), name: "edit".into(), args: "{}".into() },
                ],
                ts: 100,
                thinking: None,
            },
        ];
        let hits = peek_hits(&msgs, false, true, 0, 80, None);
        assert_eq!(hits.len(), 2);
        assert!(matches!(&hits[0].kind, PeekKind::ToolCall { id, .. } if id == "a"));
        assert!(matches!(&hits[1].kind, PeekKind::ToolCall { id, .. } if id == "b"));
        // Each tool call is on its own line, so hit lines must differ.
        assert_ne!(hits[0].line, hits[1].line);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib -- peek_hits`
Expected: FAIL — `peek_hits` not found, `PeekHit` / `PeekKind` not found.

- [ ] **Step 3: Add `PeekHit` and `PeekKind` types**

Add after the `QuestionChoiceHit` struct (after line 35):

```rust
/// A clickable tool-call line or delegated chip — the `⏎ peek` hint. Maps a
/// rendered line to the data needed to populate a peek popup. Collected as a
/// side-output of `build_conversation`, like `CodeHit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeekHit {
    /// The rendered line index (transcript coordinates, same as CodeHit).
    pub line: usize,
    pub kind: PeekKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeekKind {
    ToolCall {
        id: String,
        name: String,
        args: String,
    },
    Delegated {
        summary: String,
        ok: bool,
    },
}
```

- [ ] **Step 4: Add `peek_hits()` function**

Add after `question_choice_hits()` (after line 146):

```rust
/// The clickable tool-call / delegated-chip map for the same inputs
/// `conversation_lines` renders at Normal altitude. Like `code_hits`, called
/// on demand (on a click), so the extra build cost is paid then.
pub fn peek_hits(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<PeekHit> {
    let mut peeks = Vec::new();
    build_conversation(
        msgs,
        &RenderCtx {
            streaming,
            caret_on,
            tz_offset_secs,
            width,
            question,
            edit_diffs: &[],
            inline_k: 0,
        },
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut peeks,
    );
    peeks
}
```

- [ ] **Step 5: Add `peek_hits` parameter to `build_conversation`**

Change the signature at line 165 from:

```rust
fn build_conversation(
    msgs: &[ChatMsg],
    ctx: &RenderCtx,
    hits: &mut Vec<CodeHit>,
    msg_starts: &mut Vec<usize>,
    question_choices: &mut Vec<QuestionChoiceHit>,
) -> Vec<Line<'static>> {
```

to:

```rust
fn build_conversation(
    msgs: &[ChatMsg],
    ctx: &RenderCtx,
    hits: &mut Vec<CodeHit>,
    msg_starts: &mut Vec<usize>,
    question_choices: &mut Vec<QuestionChoiceHit>,
    peek_hits: &mut Vec<PeekHit>,
) -> Vec<Line<'static>> {
```

- [ ] **Step 6: Update all 4 callers of `build_conversation`**

Each caller needs an extra `&mut Vec::new()` (or `&mut peeks` for `peek_hits`) argument. The callers are:

1. `conversation_lines_with_diffs` (line 60) — add `&mut Vec::new(),` after `&mut Vec::new(),` (the `question_choices` arg):

```rust
    build_conversation(
        msgs,
        &RenderCtx {
            streaming,
            caret_on,
            tz_offset_secs,
            width,
            question,
            edit_diffs,
            inline_k,
        },
        &mut hits,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
    )
```

2. `code_hits` (line 91) — same pattern, add `&mut Vec::new(),` after the last `&mut Vec::new(),`:

```rust
    build_conversation(
        msgs,
        &RenderCtx {
            streaming,
            caret_on,
            tz_offset_secs,
            width,
            question,
            edit_diffs: &[],
            inline_k: 0,
        },
        &mut hits,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
```

3. `question_choice_hits` (line 121) — add `&mut Vec::new(),` after `&mut choices,`:

```rust
    build_conversation(
        msgs,
        &RenderCtx {
            streaming,
            caret_on,
            tz_offset_secs,
            width,
            question,
            edit_diffs: &[],
            inline_k: 0,
        },
        &mut Vec::new(),
        &mut Vec::new(),
        &mut choices,
        &mut Vec::new(),
    );
```

4. `conversation_view_indexed` (line 740) — add `&mut Vec::new(),` after `&mut Vec::new(),` (the `question_choices` arg):

```rust
            build_conversation(
                msgs,
                &RenderCtx {
                    streaming,
                    caret_on: view.caret_on,
                    tz_offset_secs: view.tz_offset_secs,
                    width,
                    question,
                    edit_diffs,
                    inline_k,
                },
                &mut hits,
                &mut starts,
                &mut Vec::new(),
                &mut Vec::new(),
            )
```

- [ ] **Step 7: Push `PeekHit` in the tool-call loop**

In `build_conversation`, inside the `for tc in tool_calls { ... }` loop (around line 258), after the `lines.push(Line::from(vec![...]))` that renders the tool-call line with the `⏎ peek` hint (the push that ends around line 275), add:

```rust
                    peek_hits.push(PeekHit {
                        line: lines.len() - 1,
                        kind: PeekKind::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            args: tc.args.clone(),
                        },
                    });
```

This goes immediately after the `lines.push(...)` for the tool-call line and before the closing `}` of the `for tc in tool_calls` loop.

- [ ] **Step 8: Push `PeekHit` in the `Delegated` arm**

In `build_conversation`, in the `ChatMsg::Delegated { summary, ok }` arm, after the `lines.push(Line::from(vec![...]))` that renders the delegated chip with the `⏎ peek` hint (the push that ends around line 383), add:

```rust
                peek_hits.push(PeekHit {
                    line: lines.len() - 1,
                    kind: PeekKind::Delegated {
                        summary: summary.clone(),
                        ok: *ok,
                    },
                });
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib -- peek_hits`
Expected: PASS — all 4 tests pass.

- [ ] **Step 10: Build workspace to verify no breakage**

Run: `cargo build --workspace`
Expected: PASS — all crates compile.

- [ ] **Step 11: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat(peek): add PeekHit/PeekKind types and peek_hits() collection in build_conversation"
```

---

### Task 2: State layer — `PeekState`, `PeekContent`, `peek` field

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add types before `ShellState` at ~line 387; add `peek` field to `ShellState` after `session_confirm` at line 495; add `peek: None` to `ShellState::new()` after `session_confirm: None` at line 672)
- Test: `crates/zoid-tui/src/state.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: nothing new (pure state types)
- Produces: `pub struct PeekState { pub content: PeekContent, pub scroll: usize }`; `pub enum PeekContent { ToolCall { name, args, output, is_error, compacted }, Delegated { summary, ok } }`; `pub peek: Option<PeekState>` field on `ShellState`

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `state.rs`:

```rust
    #[test]
    fn peek_is_none_by_default() {
        let s = ShellState::new();
        assert!(s.peek.is_none());
    }

    #[test]
    fn peek_set_and_clear() {
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::ToolCall {
                name: "shell".into(),
                args: r#"{"command":"ls"}"#.into(),
                output: Some("file1\nfile2".into()),
                is_error: false,
                compacted: false,
            },
            scroll: 0,
        });
        assert!(s.peek.is_some());
        s.peek = None;
        assert!(s.peek.is_none());
    }

    #[test]
    fn peek_included_in_equality() {
        let mut a = ShellState::new();
        let mut b = ShellState::new();
        assert_eq!(a, b);
        a.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "done".into(), ok: true },
            scroll: 0,
        });
        assert_ne!(a, b);
        b.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "done".into(), ok: true },
            scroll: 0,
        });
        assert_eq!(a, b);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib -- peek_`
Expected: FAIL — `PeekState` / `PeekContent` not found, `peek` field not found.

- [ ] **Step 3: Add `PeekState` and `PeekContent` types**

Add before the `ShellState` struct definition (before line 388):

```rust
/// State for the peek popup — a lightweight overlay showing the full content
/// of a tool call (args + result) or delegated result. Separate from `Overlay`
/// because it has different mouse semantics (click-away dismiss, internal
/// scroll). `None` when no popup is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeekState {
    pub content: PeekContent,
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeekContent {
    ToolCall {
        name: String,
        args: String,
        /// The full result output, if the call has returned. None if still
        /// pending (the tool call was sent but no ToolResult event yet).
        output: Option<String>,
        is_error: bool,
        /// Whether the result was compacted by ACM (shows a "(compacted)" note).
        compacted: bool,
    },
    Delegated {
        summary: String,
        ok: bool,
    },
}
```

- [ ] **Step 4: Add `peek` field to `ShellState`**

Add after the `session_confirm` field (after line 495):

```rust
    /// The peek popup state. `None` when no popup is open. Separate from
    /// `Overlay` — see `PeekState` docs.
    pub peek: Option<PeekState>,
```

- [ ] **Step 5: Add `peek: None` to `ShellState::new()`**

Add after `session_confirm: None,` (after line 672):

```rust
            peek: None,
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib -- peek_`
Expected: PASS — all 3 tests pass.

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(peek): add PeekState/PeekContent types and peek field on ShellState"
```

---

### Task 3: Layout layer — `peek` rect on `ShellLayout`

**Files:**
- Modify: `crates/zoid-tui/src/layout.rs` (add `peek` field to `ShellLayout` after `palette` at line 94; compute `peek` in `compute()` after the `palette` match at ~line 219; add `peek: None` to the "too small" early return at line 117; add `peek` to the normal return at line 230)
- Test: `crates/zoid-tui/src/layout.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `ShellState.peek` from Task 2; `centered()` (existing in same file)
- Produces: `pub peek: Option<Rect>` on `ShellLayout`

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `layout.rs`:

```rust
    #[test]
    fn peek_rect_none_when_peek_closed() {
        let s = ShellState::new();
        let l = compute(area(160, 40), &s);
        assert!(l.peek.is_none());
    }

    #[test]
    fn peek_rect_some_when_peek_open() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        let l = compute(area(160, 40), &s);
        assert!(l.peek.is_some());
        let p = l.peek.unwrap();
        // 65% of a 38-row conversation area (40 - 1 title - 1 status = 38,
        // minus input height; roughly 34-36). Just check it's < conversation
        // height and > 0.
        assert!(p.height > 0);
        assert!(p.height <= l.conversation.height);
        // Centered: x should be at or within the conversation area.
        assert!(p.x >= l.conversation.x);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib -- peek_rect`
Expected: FAIL — `peek` field not found on `ShellLayout`.

- [ ] **Step 3: Add `peek` field to `ShellLayout`**

Add after `pub palette: Option<Rect>,` (after line 94):

```rust
    /// The peek popup rect (centered over the conversation at 65% height).
    /// `None` when no peek popup is open.
    pub peek: Option<Rect>,
```

- [ ] **Step 4: Add `peek: None` to the "too small" early return**

In the early return `ShellLayout { ... }` at line 108, add `peek: None,` after `palette: None,`:

```rust
        return ShellLayout {
            title: Rect { x: area.x, y: area.y, width: area.width, height: 1 },
            body: Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(1) },
            conversation: Rect::default(),
            rail: None,
            drawer_headers: Vec::new(),
            drawer_bodies: Vec::new(),
            input: Rect::default(),
            status: Rect::default(),
            palette: None,
            peek: None,
        };
```

- [ ] **Step 5: Compute `peek` in `compute()` and add to the normal return**

After the `let palette = match state.overlay { ... };` block (after line 219), add:

```rust
    let peek = if state.peek.is_some() {
        let max_h = (conversation.height as f32 * 0.65).floor() as u16;
        Some(centered(conversation, conversation.width, max_h))
    } else {
        None
    };
```

Then in the `ShellLayout { ... }` return at line 221, add `peek,` after `palette,`:

```rust
    ShellLayout {
        title,
        body,
        conversation,
        rail,
        drawer_headers,
        drawer_bodies,
        input,
        status,
        palette,
        peek,
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib -- peek_rect`
Expected: PASS — both tests pass.

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/layout.rs
git commit -m "feat(peek): add peek rect to ShellLayout, centered at 65% conversation height"
```

---

### Task 4: Render layer — `render_peek_overlay`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (add `render_peek_overlay` function; add call site in `render_shell` after the overlay block at ~line 256)
- Test: `crates/zoid-tui/src/render.rs` (new tests in the existing `#[cfg(test)] mod tests` block, if present; otherwise, visual verification via build)

**Interfaces:**
- Consumes: `ShellState.peek` from Task 2; `ShellLayout.peek` from Task 3; `glyph::`, `color::` tokens
- Produces: `fn render_peek_overlay(frame: &mut Frame, state: &ShellState, area: Rect)` (private)

- [ ] **Step 1: Add `render_peek_overlay` function**

Add the function before `render_sessions_overlay` (before line 1098). This is a render-only function with no unit-testable logic beyond compilation (the popup content is straightforward line building). The test in Step 2 verifies the call site wiring.

```rust
fn render_peek_overlay(frame: &mut Frame, state: &ShellState, area: Rect) {
    use crate::state::{PeekContent, PeekState};
    use ratatui::text::Line;

    let Some(ps) = &state.peek else {
        return;
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(" peek ", Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);

    let lines: Vec<Line<'static>> = match &ps.content {
        PeekContent::ToolCall {
            name,
            args,
            output,
            is_error,
            compacted,
        } => {
            let mut out = Vec::new();
            // Header: tool name in bold.
            out.push(Line::from(vec![
                Span::styled(name.clone(), Style::new().fg(color::TXT).bold()),
            ]));
            // Args: pretty-printed if valid JSON, raw otherwise.
            let args_display = if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| args.clone())
            } else {
                args.clone()
            };
            out.push(Line::from(Span::styled(
                "args:",
                Style::new().fg(color::DIM),
            )));
            for line in args_display.lines() {
                out.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::new().fg(color::DIM),
                )));
            }
            out.push(Line::from(""));
            // Output section.
            if *compacted {
                out.push(Line::from(Span::styled(
                    "(compacted)",
                    Style::new().fg(color::DIM),
                )));
            }
            match output {
                Some(text) => {
                    let style = if *is_error {
                        Style::new().fg(color::ERROR)
                    } else {
                        Style::new().fg(color::TXT)
                    };
                    for line in text.lines() {
                        out.push(Line::from(Span::styled(line.to_string(), style)));
                    }
                }
                None => {
                    out.push(Line::from(Span::styled(
                        "(awaiting result…)",
                        Style::new().fg(color::DIM),
                    )));
                }
            }
            out
        }
        PeekContent::Delegated { summary, ok } => {
            let (mark, mark_color) = if *ok {
                (glyph::PASS, color::OK)
            } else {
                (glyph::WARNING, color::ERROR)
            };
            let mut out = Vec::new();
            out.push(Line::from(vec![
                Span::styled(format!("{mark} "), Style::new().fg(mark_color)),
                Span::styled("delegated", Style::new().fg(color::BRANCH).bold()),
            ]));
            out.push(Line::from(""));
            for line in summary.lines() {
                out.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::new().fg(color::TXT),
                )));
            }
            out
        }
    };

    let scroll = ps.scroll as u16;
    frame.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)),
        inner,
    );
}
```

- [ ] **Step 2: Add the call site in `render_shell`**

After the overlay block (after line 256, before `conv_max_scroll` at line 257), add:

```rust
    // Peek popup — drawn last so it sits on top of everything.
    if let Some(p) = layout.peek {
        render_peek_overlay(frame, state, p);
    }
```

- [ ] **Step 3: Build workspace to verify compilation**

Run: `cargo build --workspace`
Expected: PASS — compiles without errors. Check that `serde_json` is available in `zoid-tui` (it's used elsewhere in the crate for `arg_summary`).

- [ ] **Step 4: Verify serde_json availability**

Run: `grep 'serde_json' crates/zoid-tui/Cargo.toml`
Expected: `serde_json` is listed as a dependency. If not, add it:
```bash
# Only if serde_json is NOT in Cargo.toml:
# Add serde_json = "workspace" to [dependencies] in crates/zoid-tui/Cargo.toml
```

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(peek): add render_peek_overlay — bordered scrollable popup for tool calls and delegated results"
```

---

### Task 5: Route layer — actions, `route_key`, `route_mouse`

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (add 3 action variants after `ConversationClick` at ~line 43; add peek check in `route_key` after the question-card check at ~line 229; add peek check in `route_mouse` before the overlay guard at ~line 608)
- Test: `crates/zoid-tui/src/route.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `ShellState.peek` from Task 2
- Produces: `Action::DismissPeek`, `Action::ScrollPeek(i32)`, `Action::PeekClick(u16, u16)`

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `route.rs`. Use the existing test helpers (`ShellState::new()`, `KeyEvent` constructors, `MouseEvent` constructors — see the existing `mouse_click_toggles_drawer_and_focuses` test at line 1227 for the MouseEvent pattern):

```rust
    fn esc_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn down_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
    }

    fn up_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
    }

    fn area160x40() -> Rect {
        Rect { x: 0, y: 0, width: 160, height: 40 }
    }

    #[test]
    fn peek_open_esc_dismisses() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        assert_eq!(route_key(&s, esc_key()), Action::DismissPeek);
    }

    #[test]
    fn peek_open_arrows_scroll() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        assert_eq!(route_key(&s, down_key()), Action::ScrollPeek(1));
        assert_eq!(route_key(&s, up_key()), Action::ScrollPeek(-1));
    }

    #[test]
    fn peek_open_other_keys_are_noop() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        assert_eq!(
            route_key(&s, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            Action::Noop
        );
    }

    #[test]
    fn peek_open_mouse_scroll_scrolls_peek() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        let layout = compute(area160x40(), &s);
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &layout, scroll_down), Action::ScrollPeek(1));
    }

    #[test]
    fn peek_open_mouse_click_returns_peek_click() {
        use crate::state::{PeekContent, PeekState};
        let mut s = ShellState::new();
        s.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "x".into(), ok: true },
            scroll: 0,
        });
        let layout = compute(area160x40(), &s);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        let action = route_mouse(&s, &layout, click);
        assert!(matches!(action, Action::PeekClick(_, _)));
    }

    #[test]
    fn peek_closed_mouse_behaves_normally() {
        let s = ShellState::new();
        let layout = compute(area160x40(), &s);
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(route_mouse(&s, &layout, scroll_down), Action::ScrollConversation(1));
    }
```

Note: the test module in `route.rs` constructs `Rect` inline (see `mouse_click_toggles_drawer_and_focuses` at line 1229). The `area160x40` helper above follows the same pattern. `KeyModifiers::NONE` is the constant used throughout the existing tests.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib -- peek_`
Expected: FAIL — `DismissPeek` / `ScrollPeek` / `PeekClick` not found.

- [ ] **Step 3: Add 3 new action variants**

Add after `ConversationClick(u16),` (after line 43):

```rust
    /// Dismiss the peek popup (Esc or click-away).
    DismissPeek,
    /// Scroll the peek popup content by delta lines (positive = down).
    ScrollPeek(i32),
    /// A mouse click while the peek popup is open. The bin tests whether (row, col)
    /// falls inside the popup rect: if outside, dismiss; if inside, no-op.
    PeekClick(u16, u16),
```

- [ ] **Step 4: Add peek check in `route_key`**

After the question-card check block (after line 229, before the overlay block at line 231), add:

```rust
    // 0.5. An open peek popup captures Esc and scroll keys.
    if state.peek.is_some() {
        match key.code {
            KeyCode::Esc => return Action::DismissPeek,
            KeyCode::Down | KeyCode::PageDown => return Action::ScrollPeek(1),
            KeyCode::Up | KeyCode::PageUp => return Action::ScrollPeek(-1),
            _ => return Action::Noop,
        }
    }
```

- [ ] **Step 5: Add peek check in `route_mouse`**

Before the overlay mouse guard `if state.overlay != Overlay::None {` (before line 608), add:

```rust
    // An open peek popup captures mouse input: scroll scrolls the popup
    // content; a click is resolved by the bin (inside = no-op, outside =
    // dismiss).
    if state.peek.is_some() {
        return match m.kind {
            MouseEventKind::ScrollDown => Action::ScrollPeek(1),
            MouseEventKind::ScrollUp => Action::ScrollPeek(-1),
            MouseEventKind::Down(MouseButton::Left) => Action::PeekClick(m.row, m.column),
            _ => Action::Noop,
        };
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib -- peek_`
Expected: PASS — all 6 tests pass.

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/route.rs
git commit -m "feat(peek): add DismissPeek/ScrollPeek/PeekClick actions and routing in route_key + route_mouse"
```

---

### Task 6: Bin layer — click handling, action handlers, mouse dispatch

**Files:**
- Modify: `crates/zoid/src/main.rs` (add peek-hit check in `handle_conversation_click` after the `code_hits` check at ~line 945; add 3 action handler arms in `handle_action` before `Action::Noop => {}` at line 4766; add `PeekClick` dispatch in the mouse event arm at ~line 2788)
- Test: `crates/zoid/src/main.rs` (new tests in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `peek_hits()` from Task 1; `PeekState` / `PeekContent` from Task 2; `Action::DismissPeek` / `Action::ScrollPeek` / `Action::PeekClick` from Task 5; `in_rect()` from `zoid-tui::layout`; `compute()` from `zoid-tui::layout`
- Produces: wired end-to-end peek functionality

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `main.rs`. Use the existing `test_app()` async helper (line 7291) and `FakeProvider` pattern. These tests verify the click-to-open and action-handling logic:

```rust
    #[tokio::test]
    async fn conversation_click_on_tool_call_opens_peek() {
        use zoid_core::event::{Event, EventKind};
        use zoid_core::projection::ChatMsg;
        use zoid_tui::state::PeekContent;

        let mut app = test_app().await;
        // Inject a tool call + result into the event log.
        let now = 1000i64;
        app.events.push(Event::new(
            ulid::Ulid::new(),
            None,
            now,
            EventKind::ToolCall {
                id: "tc1".into(),
                name: "shell".into(),
                args: r#"{"command":"ls -la"}"#.into(),
            },
        ));
        app.events.push(Event::new(
            ulid::Ulid::new(),
            None,
            now + 1,
            EventKind::ToolResult {
                id: "tc1".into(),
                name: "shell".into(),
                output: "file1\nfile2\nfile3".into(),
                is_error: false,
            },
        ));

        // Build the layout and compute the tool-call line index.
        let area = ratatui::layout::Rect {
            x: 0, y: 0, width: 200, height: 50,
        };
        let layout = zoid_tui::layout::compute(area, &app.shell);
        let width = zoid_tui::layout::conv_text_width(layout.conversation.width) as usize;
        let msgs = conversation(app.events.iter());
        let peeks = zoid_tui::chat::peek_hits(&msgs, false, true, 0, width, None);
        assert!(!peeks.is_empty(), "should have at least one peek hit");

        // Simulate a click on the tool-call line.
        let clicked_line = peeks[0].line;
        let row = layout.conversation.y + clicked_line as u16;
        handle_conversation_click(&mut app, &layout, row);

        // Verify peek state was set.
        assert!(app.shell.peek.is_some());
        let ps = app.shell.peek.as_ref().unwrap();
        assert!(matches!(&ps.content, PeekContent::ToolCall { name, .. } if name == "shell"));
    }

    #[tokio::test]
    async fn dismiss_peek_clears_state() {
        use zoid_tui::state::{PeekContent, PeekState};
        use zoid_tui::route::Action;

        let mut app = test_app().await;
        app.shell.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "done".into(), ok: true },
            scroll: 0,
        });
        handle_action(&mut app, Action::DismissPeek).await.unwrap();
        assert!(app.shell.peek.is_none());
    }

    #[tokio::test]
    async fn scroll_peek_adjusts_scroll() {
        use zoid_tui::state::{PeekContent, PeekState};
        use zoid_tui::route::Action;

        let mut app = test_app().await;
        app.shell.peek = Some(PeekState {
            content: PeekContent::Delegated { summary: "line1\nline2\nline3".into(), ok: true },
            scroll: 0,
        });
        handle_action(&mut app, Action::ScrollPeek(2)).await.unwrap();
        assert_eq!(app.shell.peek.as_ref().unwrap().scroll, 2);
        // Scroll up clamps at 0.
        handle_action(&mut app, Action::ScrollPeek(-5)).await.unwrap();
        assert_eq!(app.shell.peek.as_ref().unwrap().scroll, 0);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --bin zoid -- peek`
Expected: FAIL — `PeekClick` handling not found, peek-hit check not found, action handlers not found.

- [ ] **Step 3: Add peek-hit check in `handle_conversation_click`**

After the `code_hits` check block (after line 951, before the closing `}` of the function at line 952), add:

```rust
    // Peek hits — clicking a tool-call line or delegated chip opens a popup.
    let peeks = zoid_tui::chat::peek_hits(&msgs, app.streaming, true, app.tz_offset_secs, width, None);
    if let Some(hit) = peeks.into_iter().find(|h| h.line == clicked_line) {
        use zoid_tui::state::{PeekContent, PeekState};
        use zoid_tui::chat::PeekKind;
        let content = match hit.kind {
            PeekKind::ToolCall { id, name, args } => {
                let result = msgs.iter().find_map(|m| {
                    if let ChatMsg::ToolResult { id: rid, output, is_error, compacted, .. } = m {
                        if rid == &id { Some((output.clone(), *is_error, *compacted)) } else { None }
                    } else { None }
                });
                PeekContent::ToolCall {
                    name,
                    args,
                    output: result.as_ref().map(|(o, _, _)| o.clone()),
                    is_error: result.map(|(_, e, _)| e).unwrap_or(false),
                    compacted: result.map(|(_, _, c)| c).unwrap_or(false),
                }
            }
            PeekKind::Delegated { summary, ok } => {
                PeekContent::Delegated { summary, ok }
            }
        };
        app.shell.peek = Some(PeekState { content, scroll: 0 });
        return;
    }
```

- [ ] **Step 4: Add action handlers in `handle_action`**

Before `Action::Noop => {}` (before line 4766), add:

```rust
        Action::DismissPeek => {
            app.shell.peek = None;
        }
        Action::ScrollPeek(delta) => {
            if let Some(ps) = &mut app.shell.peek {
                if *delta > 0 {
                    ps.scroll = ps.scroll.saturating_add(*delta as usize);
                } else {
                    ps.scroll = ps.scroll.saturating_sub((-delta) as usize);
                }
            }
        }
```

Note: `PeekClick` is NOT handled here — it's resolved in the mouse event handler (Step 5) because it needs the `layout` rect to test whether the click is inside or outside the popup. This mirrors how `ConversationClick` is resolved in the mouse handler rather than in `handle_action`.

- [ ] **Step 5: Add `PeekClick` dispatch in the mouse event handler**

In the `Some(Ok(CEvent::Mouse(me))) => { ... }` arm (around line 2788), after the `ConversationClick` resolution and before the `action => { handle_action(...) }` fallback, add a `PeekClick` arm:

```rust
                        match route_mouse(&app.shell, &layout, me) {
                            // Resolved here (not in handle_action) because it needs
                            // the conversation rect + wrap width from `layout`.
                            zoid_tui::route::Action::ConversationClick(row) => {
                                handle_conversation_click(app, &layout, row);
                            }
                            zoid_tui::route::Action::PeekClick(row, col) => {
                                if let Some(p) = layout.peek {
                                    if !zoid_tui::layout::in_rect(p, col, row) {
                                        app.shell.peek = None;
                                    }
                                }
                            }
                            action => {
                                if handle_action(app, action).await? {
                                    return Ok(());
                                }
                            }
                        }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid --bin zoid -- peek`
Expected: PASS — all 3 tests pass.

- [ ] **Step 7: Build and test the full workspace**

Run: `cargo build --workspace && cargo test --workspace`
Expected: PASS — all crates compile and all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(peek): wire end-to-end — click tool-call/delegated lines to open popup, Esc/click-away to dismiss"
```