# Bracketed Paste Support

## Problem

When a user paste a multi-line chunk of text into the message input, the
terminal sends each newline as a raw `Enter` keypress. The router maps bare
`Enter` → `Action::Submit`, so each line fires a separate submit — the
multi-line chunk never arrives as a single editable message.

## Solution

Enable crossterm's **bracketed paste mode** at startup. The terminal then
wraps pasted content in sentinel escape sequences (`ESC[200~` … `ESC[201~`),
and crossterm delivers the entire chunk as a single `Event::Paste(String)`.
The event loop inserts that string into the textarea in one shot, preserving
newlines. The user edits the full text and hits `Enter` to submit when ready.

## Changes (all in `crates/zoid/src/main.rs`)

### 1. Terminal setup — enable bracketed paste

In `main()`, alongside the existing `EnterAlternateScreen` /
`EnableMouseCapture`:

```rust
execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
```

### 2. Terminal teardown — disable bracketed paste

In the cleanup block, alongside `DisableMouseCapture` / `LeaveAlternateScreen`:

```rust
let _ = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    DisableBracketedPaste,
    LeaveAlternateScreen
);
```

### 3. Event loop — handle `Event::Paste`

Add a `CEvent::Paste` arm before the `Some(Ok(_))` catch-all in the
`term_events.next()` match:

```rust
Some(Ok(CEvent::Paste(text))) => {
    app.textarea.insert_text(text);
}
```

`insert_text` places the cursor at the end of the inserted text (standard
editor behavior). No `Action` routing — paste is a direct textarea mutation,
identical to how `Action::Edit(key)` calls `app.textarea.input(key)`.

### 4. No routing changes

`route_key` is untouched. Bare `Enter` still maps to `Action::Submit`. A
paste never produces individual `CEvent::Key(Enter)` events — the terminal
delivers the whole chunk as one `Event::Paste`, so `route_key` never sees
those keypresses.

## Fallback

A terminal without bracketed paste support doesn't wrap the paste in
sentinels, so crossterm delivers individual `CEvent::Key` events as before —
the current line-by-line behavior. No worse than today; no special handling
needed.

## Testing

No unit test for the paste handler itself (one-liner textarea mutation).
Existing tests verify `Action::Submit` still works and that multi-line text
in the textarea submits as a single message. The terminal setup/teardown
changes are mechanical and best-effort (same `let _ =` pattern as the
keyboard enhancement flags).