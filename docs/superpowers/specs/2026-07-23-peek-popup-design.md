# Peek Popup — Design

> **Status:** DESIGN (brainstorming, 2026-07-23). Ready for `writing-plans`.

---

## 1. Goal & scope

The `⏎ peek` hint appears on tool-call lines and delegated-result chips at Normal
zoom, but **no action is wired to it** — clicking the line copies a code block
(if one is there) or does nothing; there is no keybinding. The hint is a promise
with no payoff.

This spec makes peek real: **clicking a tool-call line or delegated chip opens a
lightweight popup overlay** centered over the conversation pane, showing the full
untruncated tool call args and result output (or the subagent summary for
delegated results). The popup caps at 65% of the conversation height with
internal scrolling. Dismissed by Esc or click-away.

**In scope:**
- A `PeekHit` collected during `build_conversation` (same side-output pattern as
  `CodeHit` / `QuestionChoiceHit`) that maps a rendered line to a tool call or
  delegated result.
- A `PeekState` on `ShellState` + a `peek` rect on `ShellLayout`.
- A `render_peek_overlay` function that draws a bordered, scrollable popup
  centered over the conversation.
- Routing: `ConversationClick` checks peek hits and opens the popup; Esc and
  click-away dismiss it; mouse wheel inside the popup scrolls its content.
- A new `Action::DismissPeek` variant.

**Out of scope:**
- Keyboard activation of peek (e.g. pressing Enter on a tool-call line). The hint
  says `⏎ peek` but the v1 interaction is click-only. A keybinding can be added
  later if desired.
- Editing tool call args or re-running a tool from the popup. Read-only.
- Peek for non-tool, non-delegated lines (user messages, assistant prose). Those
  have no `⏎ peek` hint and are not affected.

---

## 2. Data available at click time

`handle_conversation_click` already calls `conversation(app.events.iter())` →
`Vec<ChatMsg>`. The relevant variants:

```rust
ChatMsg::Assistant { tool_calls: Vec<ToolCallRef>, .. }   // tool call args
ChatMsg::ToolResult { id, name, output, is_error, compacted, .. }  // result
ChatMsg::Delegated { summary, ok }                         // subagent result
```

`ToolCallRef` carries `id`, `name`, `args` (raw JSON string). `ToolResult` is
matched to the call by `id`. Everything the popup needs is in `msgs` — no new
data fetching required.

---

## 3. Architecture

### 3.1 Hit-collection layer (`chat.rs`)

A new `PeekHit` type and a `peek_hits()` public function, mirroring `code_hits()`
and `question_choice_hits()`:

```rust
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

`build_conversation` gains a `&mut Vec<PeekHit>` side-output parameter. When
rendering a tool-call line in the `Assistant` arm (the line that gets the
`⏎ peek` hint), push a `PeekHit` with `line = lines.len() - 1` (the line just
pushed) and `PeekKind::ToolCall { id, name, args }`. When rendering a delegated
chip in the `Delegated` arm, push `PeekKind::Delegated { summary, ok }`.

`peek_hits()` runs `build_conversation` with the same `RenderCtx` as
`code_hits()` and returns the collected hits:

```rust
pub fn peek_hits(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<PeekHit> { ... }
```

**Important:** `build_conversation` currently takes three side-output vectors
(`hits`, `msg_starts`, `question_choices`). Adding a fourth (`peek_hits`) is
mechanical. All four are populated in the same single render pass, so line
indices are consistent across hit types — a given row maps to at most one hit
type. A tool-call line is never also a code block line, so there is no collision.

### 3.2 State layer (`state.rs`)

A `PeekState` on `ShellState`, following the `Option<...>` overlay pattern:

```rust
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

`ShellState` gains `pub peek: Option<PeekState>`, initialized to `None` in
`ShellState::new()` (which `Default` delegates to). `ShellState` derives
`PartialEq, Eq`, so `peek` is automatically included in equality comparisons.

**Why not an `Overlay` variant?** Existing overlays are keyboard-driven with
mouse-as-`Noop` (see `route_mouse` line 608). The peek popup needs click-away
dismiss and internal mouse scroll — different mouse semantics. Making it a
separate `Option<PeekState>` avoids conflicting with the overlay mouse guard.

### 3.3 Layout layer (`layout.rs`)

`ShellLayout` gains:

```rust
pub peek: Option<Rect>,
```

In `compute()`, after the `palette` rect is computed, add:

```rust
let peek = if state.peek.is_some() {
    let conv = conversation;
    let max_h = (conv.height as f32 * 0.65).floor() as u16;
    Some(centered(conv, conv.width, max_h))
} else {
    None
};
```

This centers the popup within the conversation area at 65% height, full
conversation width. The `centered` helper (already in `layout.rs:235`) clamps
to the area and positions at the vertical center (1/3 from top). The popup width
is the conversation width (not the text width — the bordered block needs the
full area; the inner margin gives the text padding).

### 3.4 Render layer (`render.rs`)

A new `render_peek_overlay` function, called in `render_chat` after the overlay
block (so it draws on top of everything else):

```rust
fn render_peek_overlay(frame: &mut Frame, state: &ShellState, area: Rect) { ... }
```

The function:
1. `frame.render_widget(Clear, area)` — wipes the conversation behind the popup.
2. Renders a bordered `Block` with title `" peek "` (using `glyph::` + `color::`
   tokens, same as other overlays).
3. Gets `inner = area.inner(Margin { horizontal: 1, vertical: 1 })`.
4. Builds the content lines from `state.peek.as_ref().unwrap().content`:
   - **`ToolCall`**: a header line (`name` in bold), a dim `args:` line with the
     full args JSON (pretty-printed if it parses, raw if not), a blank separator,
     then the output. If `output` is `None`: a dim `"(awaiting result…)"` line.
     If `is_error`: the output is styled in `color::ERROR`. If `compacted`: a dim
     `"(compacted)"` note before the output.
   - **`Delegated`**: the full `summary` text, with an `ok`/`fail` marker.
5. Applies `scroll` offset: `Paragraph::new(lines).scroll((scroll as u16, 0))`.
6. Renders the paragraph into `inner`.

The content lines are built as `Vec<Line<'static>>` with styled spans, the same
as every other renderer in `chat.rs` / `render.rs`.

In `render_chat`, the call site:

```rust
// After the overlay block:
if let Some(p) = layout.peek {
    if state.peek.is_some() {
        render_peek_overlay(frame, state, p);
    }
}
```

### 3.5 Route layer (`route.rs`)

**Key routing (`route_key`):**

Add a peek check **before** the overlay check (step 0.5, after the question card
check at step 0 but before the overlay block at step 1):

```rust
// 0.5. An open peek popup captures Esc and scroll keys.
if let Some(ps) = &state.peek {
    match key.code {
        KeyCode::Esc => return Action::DismissPeek;
        KeyCode::Down | KeyCode::PageDown => return Action::ScrollPeek(1),
        KeyCode::Up | KeyCode::PageUp => return Action::ScrollPeek(-1),
        _ => return Action::Noop,
    }
}
```

While peek is open, all other keys are `Noop` — the popup is modal (same as
overlays). The user dismisses with Esc and then their keys reach the normal
handlers.

**Mouse routing (`route_mouse`):**

Add a peek check **before** the overlay check:

```rust
// An open peek popup captures mouse input: scroll inside its rect scrolls
// the popup; a click anywhere dismisses it.
if state.peek.is_some() {
    return match m.kind {
        MouseEventKind::ScrollDown => Action::ScrollPeek(1),
        MouseEventKind::ScrollUp => Action::ScrollPeek(-1),
        MouseEventKind::Down(MouseButton::Left) => {
            // A click inside the popup area is a no-op (don't dismiss on
            // accidental clicks within the popup). A click outside dismisses.
            // The bin has the layout rect to test against.
            Action::PeekClick(m.row, m.column)
        }
        _ => Action::Noop,
    };
}
```

**Three new action variants:**

```rust
/// Dismiss the peek popup (Esc or click-away).
DismissPeek,
/// Scroll the peek popup content by delta lines (positive = down).
ScrollPeek(i32),
/// A mouse click while the peek popup is open. The bin tests whether (row, col)
/// falls inside the popup rect: if outside, dismiss; if inside, no-op.
PeekClick(u16, u16),
```

### 3.6 Bin layer (`main.rs`)

**`handle_conversation_click` — open peek:**

After the existing `code_hits` check (and the question-choice check), add a
`peek_hits` check:

```rust
let peeks = zoid_tui::chat::peek_hits(&msgs, app.streaming, true, app.tz_offset_secs, width, None);
if let Some(hit) = peeks.into_iter().find(|h| h.line == clicked_line) {
    let content = match hit.kind {
        PeekKind::ToolCall { id, name, args } => {
            // Find the matching ToolResult by id.
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

This comes **after** the code-hit check so that clicking a code block still
copies it — code blocks and tool-call lines never share a row, but the
precedence is explicit.

**`handle_action` — new actions:**

```rust
Action::DismissPeek => {
    app.shell.peek = None;
}
Action::ScrollPeek(delta) => {
    if let Some(ps) = &mut app.shell.peek {
        // Clamp scroll to [0, content_len]. The exact max is computed in the
        // renderer; here we just clamp to a generous upper bound and let the
        // renderer's Paragraph scroll handle overflow gracefully.
        if *delta > 0 {
            ps.scroll = ps.scroll.saturating_add(*delta as usize);
        } else {
            ps.scroll = ps.scroll.saturating_sub((-delta) as usize);
        }
    }
}
Action::PeekClick(row, col) => {
    // Dismiss only if the click is outside the popup rect.
    let area = /* current frame area */;
    let layout = compute(area, &app.shell);
    if let Some(p) = layout.peek {
        if !zoid_tui::layout::in_rect(p, col, row) {
            app.shell.peek = None;
        }
    }
}
```

**Mouse event dispatch (`run` loop):**

The `PeekClick` action needs the layout rect, which `route_mouse` doesn't have.
Two options:
- **(A)** `route_mouse` returns `PeekClick(row, col)` and the bin resolves it
  (using the same pattern as `ConversationClick` which is resolved in the bin).
- **(B)** Pass `layout` into `route_mouse` so it can test the rect directly and
  return `DismissPeek`.

Option A is consistent with how `ConversationClick` is already handled (route
returns the raw click, bin resolves it with layout). Use A.

In the `CEvent::Mouse` handler, `PeekClick` is handled alongside
`ConversationClick` — both need the layout, so both are resolved in the
`Some(Ok(CEvent::Mouse))` arm before `handle_action`.

---

## 4. Interaction summary

| Trigger | Effect |
|---|---|
| Click on a tool-call line (at Normal zoom) | Opens peek popup with full args + result |
| Click on a delegated chip | Opens peek popup with full summary |
| Esc while popup is open | Dismisses popup |
| Click outside popup area | Dismisses popup |
| Click inside popup area | No-op (stays open) |
| Mouse wheel / ↑↓ inside popup | Scrolls popup content |
| Mouse wheel outside popup (while open) | Scrolls popup content (captured) |

The popup is modal: while open, keyboard input is captured (Esc/scroll only),
same as other overlays. This prevents the conversation from scrolling
underneath or the message box from receiving typed characters.

---

## 5. Edge cases

- **Tool call with no result yet (still running):** `output` is `None` → popup
  shows `"(awaiting result…)"`. The popup does not auto-update when the result
  arrives — the user dismisses and re-clicks. (Live-updating would require
  re-rendering on every event; not worth it for a quick peek.)
- **Compacted tool result:** If `compacted` is true, show a `"(compacted)"`
  note. The output is the compacted summary, not the original — that's what's in
  `ChatMsg::ToolResult.output`.
- **Very short output:** The popup still uses the 65% height allocation. This is
  acceptable — the alternative (fit-to-content) would require a two-pass render
  (compute content lines, then size the rect), which adds complexity. The fixed
  max height is simpler and consistent with the existing overlay pattern.
  *Revisit: if the empty space looks bad, a `min(content_height, 65%)` fit can be
  added as a follow-up.*
- **Multiple tool calls in one assistant turn:** Each tool-call line gets its own
  `PeekHit`. Clicking one opens a popup for that specific call. The result is
  matched by `id`.
- **Zoom != Normal:** `handle_conversation_click` returns early if
  `zoom != Zoom::Normal` (existing behavior, line 909). Peek is Normal-zoom only,
  matching where the `⏎ peek` hint is rendered.
- **Overlay is open when peek is clicked:** Can't happen — if an overlay is open,
  mouse clicks are `Noop` (route_mouse line 608), so `ConversationClick` never
  fires. Peek and overlays are mutually exclusive.
- **Peek is open when an overlay-opening key is pressed:** The peek check in
  `route_key` returns `Noop` for all non-Esc/scroll keys, so overlay-opening
  combos (Ctrl+P, etc.) are blocked while peek is open. The user must dismiss
  peek first. This is consistent with how overlays block each other.

---

## 6. Testing

- **`chat.rs` tests:** `peek_hits` returns hits with correct line indices and
  kinds for tool-call lines and delegated chips. No hits for user/assistant-prose
  lines. Multiple tool calls in one turn each get their own hit.
- **`state.rs` tests:** `PeekState` is `None` by default; setting and clearing
  `peek` works; `eq_fast` / `eq_full` include `peek`.
- **`route.rs` tests:** Esc while peek is open → `DismissPeek`. Arrow keys →
  `ScrollPeek`. Click outside → `PeekClick` (bin test: outside rect → dismissed).
  Click inside → `PeekClick` (bin test: inside rect → no-op). Other keys → `Noop`.
- **`layout.rs` tests:** `peek` rect is `None` when `state.peek` is `None`; is
  `Some(centered rect at 65% height)` when `state.peek` is `Some`.
- **`main.rs` tests:** `handle_conversation_click` on a tool-call line sets
  `peek` with correct content (args + matching result). Click on a non-tool line
  does not set peek. `DismissPeek` clears `peek`. `ScrollPeek` adjusts scroll and
  clamps at 0.

---

## 7. File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tui/src/chat.rs` | `PeekHit`, `PeekKind`, `peek_hits()`, side-output in `build_conversation` | Modify |
| `crates/zoid-tui/src/state.rs` | `PeekState`, `PeekContent`, `peek` field on `ShellState` | Modify |
| `crates/zoid-tui/src/layout.rs` | `peek` rect on `ShellLayout`, computed in `compute()` | Modify |
| `crates/zoid-tui/src/render.rs` | `render_peek_overlay`, call site in `render_chat` | Modify |
| `crates/zoid-tui/src/route.rs` | `DismissPeek` / `ScrollPeek` / `PeekClick` actions, peek checks in `route_key` + `route_mouse` | Modify |
| `crates/zoid/src/main.rs` | peek-hit check in `handle_conversation_click`, action handlers for `DismissPeek` / `ScrollPeek` / `PeekClick` | Modify |