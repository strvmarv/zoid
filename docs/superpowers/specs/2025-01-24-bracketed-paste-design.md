# Bracketed Paste + Two Small Fixes

## 1. Bracketed Paste

### Problem

When a user pastes a multi-line chunk of text into the message input, the
terminal sends each newline as a raw `Enter` keypress. The router maps bare
`Enter` → `Action::Submit`, so each line fires a separate submit — the
multi-line chunk never arrives as a single editable message.

### Solution

Enable crossterm's **bracketed paste mode** at startup. The terminal then
wraps pasted content in sentinel escape sequences (`ESC[200~` … `ESC[201~`),
and crossterm delivers the entire chunk as a single `Event::Paste(String)`.
The event loop inserts that string into the textarea in one shot, preserving
newlines. The user edits the full text and hits `Enter` to submit when ready.

### Changes (all in `crates/zoid/src/main.rs`)

**Terminal setup** — In `main()`, alongside the existing
`EnterAlternateScreen` / `EnableMouseCapture`:

```rust
execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
```

**Terminal teardown** — In the cleanup block, alongside
`DisableMouseCapture` / `LeaveAlternateScreen`:

```rust
let _ = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    DisableBracketedPaste,
    LeaveAlternateScreen
);
```

**Event loop** — Add a `CEvent::Paste` arm before the `Some(Ok(_))` catch-all
in the `term_events.next()` match:

```rust
Some(Ok(CEvent::Paste(text))) => {
    app.textarea.insert_text(text);
}
```

`insert_text` places the cursor at the end of the inserted text (standard
editor behavior). No `Action` routing — paste is a direct textarea mutation,
identical to how `Action::Edit(key)` calls `app.textarea.input(key)`.

**No routing changes** — `route_key` is untouched. Bare `Enter` still maps to
`Action::Submit`. A paste never produces individual `CEvent::Key(Enter)`
events — the terminal delivers the whole chunk as one `Event::Paste`, so
`route_key` never sees those keypresses.

**Fallback** — A terminal without bracketed paste support doesn't wrap the
paste in sentinels, so crossterm delivers individual `CEvent::Key` events as
before — the current line-by-line behavior. No worse than today.

## 2. Tool Indicator Expansion (status bar)

### Problem

The tool activity indicator in the status bar (right of "working") is
hard-capped to a fixed `TOOL_SLOT` of 12 columns. A long tool name (e.g.
`"dispatch_subagent"` → `"◐ dispatch_subagent …"`) is **truncated** to fit
12 chars, squishing against the glyph instead of expanding into the open
space to its right (the gap between the tool indicator and the zoom hint).

### Fix (`crates/zoid-tui/src/render.rs`)

The tool indicator sits on the *outside* (right of "working"), so it has room
to grow toward the right edge. Replace the fixed `TOOL_SLOT` truncation with
a **dynamic cap**: compute the available width as `right_start - consumed_so_far
- 1` (the space between the 4-space gap after "working" and the zoom hint,
minus 1-char padding). If the tool text fits in the available width, render
it in full; otherwise truncate from the left (keep the glyph + as much of the
name as fits) to the available width instead of 12.

The idle tool text (`"◐ tool"`) stays as-is — it's already short. Only the
active tool text (long name) benefits from the dynamic cap.

Remove the `TOOL_SLOT` constant and its padding/truncation block. Replace
with:

```rust
// Compute the available width for the tool text: the space between the
// 4-space gap after "working" and the zoom hint, minus 1-char padding.
let consumed_before_tool: usize = spans.iter().map(|s| s.content.width()).sum();
let tool_cap = (right_start.saturating_sub(consumed_before_tool)).saturating_sub(1).max(8);
```

Then truncate the tool text to `tool_cap` (left-truncate, keeping the glyph)
if it exceeds it, instead of padding/truncating to the fixed 12.

## 3. Subagent Model Override Bug

### Problem

The `dispatch_subagent` tool spec advertises a `model` parameter:

```json
"model": { "type": "string", "description": "Model override; omit to inherit the session model" }
```

The LLM fills this in with a model it thinks is appropriate (e.g.
`"claude-sonnet-4-6"`), overriding the session's active model. The subagent
then runs against the session's provider (Ollama) with a model the provider
doesn't serve → HTTP 404.

### Root cause

The `model` parameter invites the LLM to choose a model. It shouldn't —
subagents must always inherit the session's provider + model. The plumbing
(`model_override.unwrap_or_else(|| model.clone())` in `agent.rs`) correctly
falls back to the session model when the parameter is absent, but the LLM
fills it in.

### Fix

**`crates/zoid-tools/src/subagent_dispatch.rs`** — Remove the `model` property
from the `dispatch_subagent` tool spec's `parameters.json`. The tool still
accepts `task` and `worktree`.

**`crates/zoid/src/agent.rs`** — Remove the `model_override` extraction (the
`tc.args.get("model")` block) and pass `model.clone()` directly to
`spawn_subagent`. The `model` parameter is no longer in the spec, so the LLM
won't send it, but removing the extraction also makes the code honest.

**`crates/zoid-tools/src/subagent_dispatch.rs` test** — Update
`dispatch_subagent_spec_and_kind` to assert the `model` property is absent
from the spec.

## Testing

No unit test for the paste handler itself (one-liner textarea mutation).
Existing tests verify `Action::Submit` still works and multi-line text
submits as a single message. The terminal setup/teardown changes are
mechanical and best-effort.

The tool indicator fix is verified by the existing `render::tests` module
(status bar renders without panic). The dynamic cap is a pure calculation
from already-computed values.

The subagent fix is verified by the updated `dispatch_subagent_spec_and_kind`
test asserting no `model` property.