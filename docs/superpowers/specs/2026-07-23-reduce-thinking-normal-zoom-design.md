# Reduce Thinking Output in Normal Zoom

## Problem

At Normal zoom, every assistant turn that had reasoning gets a dedicated "▾ Thinking…"
marker line (plus a blank separator). Over a session with many turns, these marker
lines accumulate and crowd out the actual messages and tool calls. The marker is
rendered in `chat.rs` `build_conversation` (the `ChatMsg::Assistant` arm, lines 238–250)
as a standalone line with a blank separator before it.

## Design

Replace the dedicated thinking marker line with a **dim `·thinking` badge appended to
the last rendered line of the turn** — whether that line is the last wrapped line of
the assistant's text or the last tool-call line. This signals reasoning happened
without consuming an extra line per turn.

### §1 What changes

**`chat.rs` — `build_conversation`, `ChatMsg::Assistant` arm (Normal zoom path):**

**Remove** the thinking marker block (lines 238–250):

```rust
// REMOVED:
if let Some(thinking_text) = thinking {
    if !thinking_text.is_empty() {
        blank_between_turns(&mut lines);
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", glyph::EXPANDED), Style::new().fg(color::DIM)),
            Span::styled("Thinking…", Style::new().fg(color::DIM)),
        ]));
    }
}
```

**Add** badge logic after the turn's text and tool calls are rendered. After the
existing `for tc in tool_calls { ... }` loop, if `thinking` is `Some` and non-empty:

```rust
// Append a dim badge to the last line of this turn.
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

This grabs `lines.last_mut()` — the last line pushed by the assistant text rendering
or the last tool-call line — and appends the badge span. No extra line is consumed.

If the turn produced no lines at all (empty text + no tool calls — edge case where
the `!shown.is_empty() || tool_calls.is_empty()` guard produced a single empty-prefix
line via `push_message`), the badge still attaches to that line. If `lines` is somehow
empty (shouldn't happen — `push_message` always pushes at least the prefix line), the
`if let Some(last)` guard skips the badge.

### §2 What is not touched

- **The `ModelThinking` event and its ephemeral emission** (`agent.rs`) — unchanged.
  `ThinkingDelta` still accumulates in `thinking_buf`, and `ModelThinking` is still
  emitted only on the final sub-turn (intermediate sub-turns discard thinking).
- **The projection layer** (`projection.rs`) — `ChatMsg::Assistant { thinking, .. }`
  still carries the thinking text. The folding logic is unchanged.
- **Detail zoom** — full thinking text still renders under the "─ Thinking ─"
  separator (`chat.rs` `detail_lines`). No badge in Detail zoom; the thinking is
  fully visible there.
- **Summary/Overview zoom** — neither shows thinking. No change.
- **The status bar** — `thinking_label` still shows the thinking mode (e.g.
  "thinking high") when enabled. Unchanged.
- **`glyph::EXPANDED`** — no longer used for the thinking marker at Normal zoom. The
  constant stays in `tokens.rs` (it may be used elsewhere or in future). No removal.

### §3 Visual example

Before (Normal zoom, thinking enabled):

```
19:51 › Let's fix the auth bug in login.rs

▾ Thinking…

19:51 zoid  I'll look at the login handler first.
  ● read(path: src/auth/login.rs) ⏎ peek
  ✓ read → fn login(req: Request) → Result…

19:52 › Looks wrong — the token check is missing

▾ Thinking…

19:52 zoid  Right — the validate_token call was removed. I'll add it back
  ● edit(path: src/auth/login.rs, old: "fn login(req:…) ⏎ peek
  ✓ edit → +3 −1

19:53 › Commit it

19:53 zoid  Done.
  ● shell(command: git commit -am "fix…) ⏎ peek
  ✓ shell → [main a1b2c3d] fix: restore…
```

After (Normal zoom, thinking enabled):

```
19:51 › Let's fix the auth bug in login.rs

19:51 zoid  I'll look at the login handler first.
  ● read(path: src/auth/login.rs) ⏎ peek ·thinking
  ✓ read → fn login(req: Request) → Result…

19:52 › Looks wrong — the token check is missing

19:52 zoid  Right — the validate_token call was removed. I'll add it back
  ● edit(path: src/auth/login.rs, old: "fn login(req:…) ⏎ peek ·thinking
  ✓ edit → +3 −1

19:53 › Commit it

19:53 zoid  Done.
  ● shell(command: git commit -am "fix…) ⏎ peek
  ✓ shell → [main a1b2c3d] fix: restore…
```

The third turn ("Done.") had no thinking, so no badge. The first two turns had
thinking — the badge appears on the last tool-call line (the last rendered line of
each turn). 4 lines saved (2 marker lines + 2 blank separators).

For a text-only turn with thinking (no tool calls), the badge appears on the last
wrapped line of the assistant message:

```
19:54 › What did the fix do?

19:54 zoid  The validate_token call was removed in a previous
            commit, so any request with an expired session
            cookie would bypass authentication. ·thinking
```

### §4 Edge cases

- **Multi-line text:** The badge appears at the end of the last wrapped line, not the
  first. The user reads the full message, then sees the badge.
- **Tool calls after text:** The badge goes on the last tool-call line, since that is
  rendered after the text. Text and tool calls are part of the same turn.
- **Streaming:** The `thinking` field on `ChatMsg::Assistant` is only populated after
  the turn completes (the projection folds `ModelThinking` into the assistant item).
  During streaming, the badge is not present — it appears once the turn is flushed.
- **Empty thinking string (`Some("")`):** The `is_some_and(|t| !t.is_empty())` guard
  prevents the badge. Same guard as the old marker.
- **Turn with no text and no tool calls + thinking:** `push_message` still pushes a
  line (the prefix with empty body), so `lines.last_mut()` finds it and the badge
  attaches to the prefix line. This matches the old behavior where the marker showed
  even for empty-text turns.
- **Thinking disabled (default):** No `ModelThinking` events are emitted, `thinking`
  is always `None` on `ChatMsg::Assistant`, no badge appears. No behavioral change
  from the user's perspective when thinking is off.

### §5 Testing

**Unit tests in `chat.rs`:**

- A turn with thinking + text → the last line of the assistant text ends with
  `·thinking`.
- A turn with thinking + tool calls but no text → the last tool-call line ends with
  `·thinking`.
- A turn with thinking + text + tool calls → the last tool-call line ends with
  `·thinking` (it is the last rendered line of the turn).
- A turn with no thinking → no `·thinking` badge anywhere.
- A turn with empty thinking string (`Some("")`) → no badge.
- Normal zoom no longer produces a standalone "Thinking…" line (no line containing
  only the `▾` glyph and "Thinking…" text).
- Detail zoom still renders the full thinking section under "─ Thinking ─"
  unchanged.