# Empty-state guidance for new vs. returning users — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the generic `(no messages yet)` empty state with onboarding copy for first-time users and a "welcome back" hint for returning users.

**Architecture:** A new pure `onboarding` module in `zoid-tui` produces the styled lines. The bin captures `first_time_user` from `sessions.is_empty()` at boot, stores it on `ShellState`, and intercepts the empty-conversation case in `run()`'s body-building block — building the onboarding lines directly into `BodyCache.body` instead of calling `BodyCache::refresh`. No signature changes to `build_conversation` or `render_shell`.

**Tech Stack:** Rust, ratatui (`Line`, `Span`, `Style`), `zoid-tui` tokens (`color`, `glyph`), `zoid-tui::render::wrap_plain`.

## Global Constraints

- **Design tokens (spec §16):** every color and glyph comes from `crates/zoid-tui/src/tokens.rs` — never hardcode a color or character literal in render code. Reuse `color::{CHAT_ACCENT, DIM, TXT}` and `glyph::USER_TURN` (`'›'`).
- **`wrap_plain` visibility:** `pub(crate) fn wrap_plain` lives in `crates/zoid-tui/src/render.rs:1218`. It's already `pub(crate)`, so a sibling module can call `crate::render::wrap_plain` directly — no visibility change needed.
- **`ShellState::new()` default:** `first_time_user` defaults to `false` (returning-user state), so snapshot tests and examples that don't set it never show onboarding copy.
- **No signature changes:** `build_conversation`, `conversation_view`, `conversation_view_indexed`, `detail_lines`, and `render_shell` are not modified.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid-tui/src/onboarding.rs` | **New.** Copy consts + one pure `empty_state_lines` function + unit tests. Terminal-free, state-free. |
| `crates/zoid-tui/src/lib.rs` | Register `pub mod onboarding;` |
| `crates/zoid-tui/src/state.rs` | Add `pub first_time_user: bool` field to `ShellState` + `false` default in `new()`. |
| `crates/zoid/src/main.rs` | Capture `first_time_user` at boot; set it on `shell`; add the empty-state intercept branch in `run()`. |

---

## Task 1: Add `first_time_user` field to `ShellState`

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (struct `ShellState` + `new()`)
- Test: `crates/zoid-tui/src/state.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing
- Produces: `ShellState.first_time_user: bool` (defaults `false`)

- [ ] **Step 1: Write the failing test**

Add this test to the existing `#[cfg(test)] mod tests` block in `state.rs` (after the `new_has_no_status_hint` test):

```rust
    #[test]
    fn first_time_user_defaults_false() {
        assert!(
            !ShellState::new().first_time_user,
            "first_time_user must default to false so tests/examples don't show onboarding"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib state::tests::first_time_user_defaults_false`
Expected: FAIL — `no field named first_time_user on type ShellState`

- [ ] **Step 3: Add the field and default**

In the `ShellState` struct definition, add the field after `pub question: Option<crate::question::QuestionState>,`:

```rust
    /// Whether this is a first-time user (no prior session history for this repo
    /// at boot). Set once at boot from `sessions.is_empty()`, never changes
    /// during a session. Drives the empty-state onboarding copy vs. the
    /// "welcome back" hint. Defaults `false` so tests and examples that don't
    /// set it get the returning-user state (no onboarding copy in snapshots).
    pub first_time_user: bool,
```

In `ShellState::new()`, add the default after `question: None,` in the `Self { ... }` block:

```rust
            first_time_user: false,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib state::tests::first_time_user_defaults_false`
Expected: PASS

- [ ] **Step 5: Run the full zoid-tui lib test suite to confirm no regressions**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (all existing tests still pass — the new field has a default)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): add first_time_user field to ShellState

Defaults false so tests and examples get the returning-user state.
Set once at boot from sessions.is_empty() (wired in the next task)."
```

---

## Task 2: Create the `onboarding` module with `empty_state_lines`

**Files:**
- Create: `crates/zoid-tui/src/onboarding.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (register the module)

**Interfaces:**
- Consumes: `crate::tokens::{color, glyph}`, `crate::render::wrap_plain`, `ratatui::text::{Line, Span}`, `ratatui::style::Style`
- Produces: `pub fn empty_state_lines(first_time_user: bool, width: usize) -> Vec<Line<'static>>`

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-tui/src/onboarding.rs` with only the test module (the function doesn't exist yet, so this fails to compile — that's expected for TDD in Rust; we'll add a stub first):

```rust
//! Empty-state guidance rendered in the conversation pane when a session has
//! no messages. Two flavors: onboarding copy for first-time users, a brief
//! "welcome back" for returning users. Pure — no terminal, no state; the bin
//! calls it and paints the result into `BodyCache.body`.

use crate::tokens::{color, glyph};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The title line shown to a first-time user.
const NEW_USER_TITLE: &str = "zoid — a coding agent for your terminal";
/// The intro line above the suggested prompts.
const NEW_USER_INTRO: &str = "Try one of these to get started:";
/// Suggested first prompts for a new user. Static text in v1 (not clickable).
const NEW_USER_PROMPTS: &[&str] = &[
    "explain this codebase to me",
    "fix the failing tests",
    "add a feature from docs/TODO.md",
];
/// The hint shown to a returning user with an empty session.
const RETURNING_HINT: &str =
    "welcome back — type a message, or :resume to pick up another session";

/// Build the empty-state lines for the conversation pane. `first_time_user`
/// selects onboarding copy (new user) vs. a welcome-back hint (returning user).
/// `width` is the text column width for prose wrapping (same `width` the
/// transcript body is wrapped to). Pure; no terminal or state.
pub fn empty_state_lines(first_time_user: bool, width: usize) -> Vec<Line<'static>> {
    if first_time_user {
        new_user_lines(width)
    } else {
        returning_user_lines(width)
    }
}

fn new_user_lines(width: usize) -> Vec<Line<'static>> {
    let indent = "  ";
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title (accent, bold).
    for w in wrap_title(indent, NEW_USER_TITLE, width) {
        lines.push(Line::from(Span::styled(
            w,
            Style::new().fg(color::CHAT_ACCENT).bold(),
        )));
    }

    // Blank separator.
    lines.push(Line::from(""));

    // Intro (dim).
    for w in crate::render::wrap_plain(
        &format!("{indent}{NEW_USER_INTRO}"),
        width,
    ) {
        lines.push(Line::from(Span::styled(w, Style::new().fg(color::DIM))));
    }

    // Prompts: › <prompt> (marker in accent, text in TXT).
    for prompt in NEW_USER_PROMPTS {
        let row = format!("{indent}  {} {}", glyph::USER_TURN, prompt);
        for w in crate::render::wrap_plain(&row, width) {
            lines.push(Line::from(vec![
                Span::styled(
                    // The › marker is the first non-space char; wrap_plain may
                    // break a long prompt across lines, but the marker only
                    // appears on the first row. For wrapped continuations the
                    // whole row is TXT (the marker is embedded in the string).
                    w.clone(),
                    Style::new().fg(color::TXT),
                ),
            ]));
        }
    }

    lines
}

/// Wrap the title line, preserving the 2-space indent on continuation rows.
/// `wrap_plain` breaks on whitespace; we pass the full indented string and let
/// it wrap naturally.
fn wrap_title(indent: &str, title: &str, width: usize) -> Vec<String> {
    let full = format!("{indent}{title}");
    crate::render::wrap_plain(&full, width)
}

fn returning_user_lines(width: usize) -> Vec<Line<'static>> {
    let full = format!("  {RETURNING_HINT}");
    crate::render::wrap_plain(&full, width)
        .into_iter()
        .map(|w| Line::from(Span::styled(w, Style::new().fg(color::DIM))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first-time user sees the title, the intro, and all three suggested
    /// prompts. The title line carries the accent color.
    #[test]
    fn new_user_renders_title_and_prompts() {
        let lines = empty_state_lines(true, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(NEW_USER_TITLE),
            "title must appear: got {joined:?}"
        );
        assert!(
            joined.contains(NEW_USER_INTRO),
            "intro must appear: got {joined:?}"
        );
        for prompt in NEW_USER_PROMPTS {
            assert!(
                joined.contains(prompt),
                "prompt '{prompt}' must appear: got {joined:?}"
            );
        }
        // The title line carries the accent color.
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::CHAT_ACCENT))),
            "at least one line must use the accent color"
        );
    }

    /// A returning user sees only the welcome-back hint; none of the onboarding
    /// prompts appear.
    #[test]
    fn returning_user_renders_welcome_back_only() {
        let lines = empty_state_lines(false, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(RETURNING_HINT),
            "welcome-back hint must appear: got {joined:?}"
        );
        for prompt in NEW_USER_PROMPTS {
            assert!(
                !joined.contains(prompt),
                "onboarding prompt '{prompt}' must NOT appear for returning user: got {joined:?}"
            );
        }
        assert!(
            !joined.contains(NEW_USER_TITLE),
            "onboarding title must NOT appear for returning user: got {joined:?}"
        );
    }

    /// A very narrow width must not panic — `wrap_plain` handles it.
    #[test]
    fn wrap_respects_narrow_width() {
        // Both branches must survive a width of 10 without panicking.
        let _ = empty_state_lines(true, 10);
        let _ = empty_state_lines(false, 10);
        // Width 1 is degenerate but must not panic either.
        let _ = empty_state_lines(true, 1);
        let _ = empty_state_lines(false, 1);
    }

    /// The `›` glyph (USER_TURN) must appear in the new-user output.
    #[test]
    fn new_user_uses_turn_glyph() {
        let lines = empty_state_lines(true, 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            joined.contains(glyph::USER_TURN),
            "the › turn glyph must appear in new-user output"
        );
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/zoid-tui/src/lib.rs`, add `pub mod onboarding;` after `pub mod objects;`:

```rust
pub mod onboarding;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib onboarding`
Expected: PASS (all 4 tests — the module compiles and the function works)

- [ ] **Step 4: Run the full zoid-tui lib test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (no regressions)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/onboarding.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): add onboarding module with empty_state_lines

Pure, terminal-free. New users see a title + 3 suggested prompts;
returning users see a 'welcome back' hint. Copy as consts at the top."
```

---

## Task 3: Wire the bin — capture `first_time_user` at boot and intercept the empty state

**Files:**
- Modify: `crates/zoid/src/main.rs` (boot path + `run()` body-building block)

**Interfaces:**
- Consumes: `zoid_tui::ShellState.first_time_user` (Task 1), `zoid_tui::onboarding::empty_state_lines` (Task 2)
- Produces: the empty-state intercept in `run()`'s body-building block

- [ ] **Step 1: Capture `first_time_user` at boot**

In `crates/zoid/src/main.rs`, find the boot block that computes `sessions`:

```rust
    let sessions = session
        .list_sessions(Some(root.clone()))
        .await
        .unwrap_or_default();
    let (session_id, session_name, session_started_ms) = if let Some(s) = sessions.first() {
```

Add `let first_time_user = sessions.is_empty();` right after the `let sessions = ...` block (before the `let (session_id, ...) = ...` line):

```rust
    let sessions = session
        .list_sessions(Some(root.clone()))
        .await
        .unwrap_or_default();
    let first_time_user = sessions.is_empty();
    let (session_id, session_name, session_started_ms) = if let Some(s) = sessions.first() {
```

- [ ] **Step 2: Set the flag on `shell`**

Find the line `shell.cwd = root.clone();` in the boot setup block. Add `shell.first_time_user = first_time_user;` right after it:

```rust
    shell.cwd = root.clone();
    shell.first_time_user = first_time_user;
```

- [ ] **Step 3: Add the empty-state intercept in `run()`**

Find the `let cache_hit = if is_overview {` block in `run()`. The current code is:

```rust
        let cache_hit = if is_overview {
            let data = build_overview_data(app, obs_state);
            app.overview_body = zoid_tui::overview::overview_lines(&data, body_w);
            None
        } else {
            Some(app.body_cache.refresh(
                BodyKey {
                    zoom,
                    width: body_w,
                    streaming,
                    caret: streaming && caret,
                    tz,
                },
                &app.proj.msgs,
                body_w,
                app.shell.question.as_ref(),
            ))
        };
```

Replace the `else` branch with an `else if` empty-state intercept + `else`:

```rust
        let cache_hit = if is_overview {
            let data = build_overview_data(app, obs_state);
            app.overview_body = zoid_tui::overview::overview_lines(&data, body_w);
            None
        } else if app.proj.msgs.is_empty() {
            // Empty-state intercept: bypass BodyCache, build onboarding/welcome
            // lines directly. When the first message arrives, proj.msgs becomes
            // non-empty and the else branch takes over (key is None → full
            // rebuild). Excluded from the body-render cache-hit ratio (None).
            app.body_cache.body = zoid_tui::onboarding::empty_state_lines(
                app.shell.first_time_user,
                body_w,
            );
            app.body_cache.key = None;
            app.body_cache.msg_count = 0;
            None
        } else {
            Some(app.body_cache.refresh(
                BodyKey {
                    zoom,
                    width: body_w,
                    streaming,
                    caret: streaming && caret,
                    tz,
                },
                &app.proj.msgs,
                body_w,
                app.shell.question.as_ref(),
            ))
        };
```

- [ ] **Step 4: Verify the workspace compiles**

Run: `cargo check -p zoid`
Expected: PASS (compiles with no errors)

- [ ] **Step 5: Run the full test suite to confirm no regressions**

Run: `cargo test --workspace`
Expected: PASS (all existing tests still pass — the intercept only fires when `proj.msgs` is empty, which no existing test exercises via the bin's `run()` path; the renderer tests call `conversation_lines`/`render_chat` directly and bypass this intercept)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): wire empty-state onboarding intercept

Capture first_time_user from sessions.is_empty() at boot, store on
ShellState, and intercept the empty-conversation case in run()'s
body-building block — build onboarding/welcome-back lines directly
into BodyCache.body instead of calling BodyCache::refresh."
```

---

## Task 4: Verify end-to-end and update the TODO

**Files:**
- Modify: `docs/TODO.md` (mark item 1 as done)
- No code changes — this task is verification + doc cleanup.

- [ ] **Step 1: Full workspace test + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS (no warnings, all tests green)

- [ ] **Step 2: Manual smoke check (optional, if a terminal is available)**

Run: `cargo run` in a repo with no prior zoid sessions (a fresh clone or a repo dir with an empty `~/.local/share/zoid/zoid.db`). Observe the onboarding copy in the conversation pane. Then type a message; observe the transcript replaces the onboarding. Run `:new` from a returning-user state; observe the "welcome back" hint.

- [ ] **Step 3: Mark the TODO item as done**

In `docs/TODO.md`, replace the entire "## Empty-state guidance for new vs. returning users" section header with a completed marker. Find the line:

```markdown
## Empty-state guidance for new vs. returning users
```

Replace with:

```markdown
## Empty-state guidance for new vs. returning users (DONE)

Implemented in `crates/zoid-tui/src/onboarding.rs` + `crates/zoid/src/main.rs`.
See `docs/superpowers/specs/2026-07-06-empty-state-guidance-design.md`.
```

- [ ] **Step 4: Commit**

```bash
git add docs/TODO.md
git commit -m "docs: mark empty-state guidance TODO as done"
```