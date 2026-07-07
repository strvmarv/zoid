# Empty-state guidance for new vs. returning users — design

**Source:** `docs/TODO.md` (item 1: "Empty-state guidance for new vs.
returning users")

**Problem.** A fresh session renders `(no messages yet)` and gives the user no
guidance — no explanation of what the app is, no suggested first actions. It
feels like a dead-end rather than an intentional empty state.

**Signal already available.** At startup (`main.rs`, the
`let sessions = session.list_sessions(...)` block) the bin already
distinguishes new vs. returning users: `sessions.is_empty()` at boot means
"this is a first-time user" (no prior session history for this repo). No auth,
localStorage, or extra persistence needed — session history is already stored
server-side in SQLite.

**Gap.** The empty-state renderer (`build_conversation` in `zoid-tui/src/chat.rs`)
only receives `&[ChatMsg]` — it can't see whether session history exists. The
bin knows at boot but doesn't thread the flag down to the render path.

## Design decisions

### Where the content is rendered: bin intercept

The bin intercepts the empty case in `run()`'s body-building block, before
`BodyCache::refresh` is called. When `app.proj.msgs.is_empty()` (and not at
Overview), the bin builds onboarding/welcome-back lines directly from a new
pure module, bypassing `build_conversation` entirely.

Rationale: `build_conversation` / `conversation_view_indexed` /
`conversation_view` have a wide signature cascade (they're called from
`conversation_lines`, `code_hits`, `question_choice_hits`, `detail_lines`, and
the bin's render path). Threading a `first_time_user` flag through all of them
for a case that only fires when `msgs` is empty is disproportionate. The bin
already controls the body via `BodyCache.body`; intercepting there is ~6 lines
and leaves the pure renderer untouched.

### Where the copy lives: a new onboarding module

A dedicated `zoid-tui/src/onboarding.rs` module holds the copy as `const`s at
the top and exposes one pure function. This keeps UI copy out of the bin,
makes the empty-state content unit-testable in isolation, and leaves room for
future onboarding content (interactivity, multi-step guides, etc.) without a
rewrite.

### Suggested prompts: static text (v1)

The TODO mentions "2-3 example/suggested prompts" for new users. In v1 these
are static text the user reads and types — not clickable, not auto-filling
the input. The module is structured so interactivity can be added later (the
function returns `Vec<Line<'static>>`; a future version could return
hit-tested regions), but that's out of scope here.

## Architecture

### 1. The signal: `first_time_user` on `ShellState`

Add a `first_time_user: bool` field to `ShellState` (`crates/zoid-tui/src/state.rs`).

- **Default:** `false` in `ShellState::new()`. Tests and examples that don't
  set it get the returning-user state (the safer default — onboarding copy
  shouldn't appear in snapshot tests).
- **Set once at boot** in `main.rs`, never changes during a session:

```rust
let first_time_user = sessions.is_empty();
// ... later, in the ShellState init block:
shell.first_time_user = first_time_user;
```

`sessions` is the `Vec` returned by `session.list_sessions(Some(root.clone()))`
at boot — the same call the bin already makes to decide auto-resume vs.
create-new. `sessions.is_empty()` is the "is this a new user" test: no prior
session history for this repo means first-time.

Robust across devices/incognito because session history is keyed per-user via
the session store (SQLite `sessions` table, per-repo).

### 2. The plumbing: bin intercept in `run()`

In the render loop's body-building section — the `if is_overview { ... } else
{ ... }` block in `run()` — add a third branch: when
`app.proj.msgs.is_empty()` (and not at Overview), build the empty-state lines
from the new onboarding module instead of calling `BodyCache::refresh`:

```rust
let cache_hit = if is_overview {
    // ... existing overview path unchanged ...
    None
} else if app.proj.msgs.is_empty() {
    // Empty-state intercept: bypass BodyCache, build onboarding lines directly.
    app.body_cache.body = zoid_tui::onboarding::empty_state_lines(
        app.shell.first_time_user,
        body_w,
    );
    app.body_cache.key = None;   // force full rebuild when msgs become non-empty
    app.body_cache.msg_count = 0;
    None // not a real BodyCache lookup; excluded from the cache-hit ratio
} else {
    // ... existing BodyCache::refresh path unchanged ...
};
```

`BodyCache.body` always holds the lines to paint (whether onboarding or
transcript), so `render_shell` and the scroll/scrollbar math work unchanged.
When the first message arrives, `app.proj.msgs` becomes non-empty, the
`else if` falls through to `BodyCache::refresh` (key is `None` → full
rebuild), and normal rendering takes over seamlessly.

The `cache_hit` value is `None` for empty-state frames (like Overview
frames), so they're excluded from the body-render cache-hit ratio in obs.

### 3. The onboarding module: `zoid-tui/src/onboarding.rs`

A pure, terminal-free module. Copy as `const`s at the top; one public
function.

```rust
//! Empty-state guidance rendered in the conversation pane when a session has
//! no messages. Two flavors: onboarding copy for first-time users, a brief
//! "welcome back" for returning users. Pure — no terminal, no state; the bin
//! calls it and paints the result into `BodyCache.body`.

const NEW_USER_TITLE: &str = "zoid — a coding agent for your terminal";
const NEW_USER_INTRO: &str = "Try one of these to get started:";
const NEW_USER_PROMPTS: &[&str] = &[
    "explain this codebase to me",
    "fix the failing tests",
    "add a feature from docs/TODO.md",
];
const RETURNING_HINT: &str =
    "welcome back — type a message, or :resume to pick up another session";

/// Build the empty-state lines for the conversation pane. `first_time_user`
/// selects onboarding copy (new user) vs. a welcome-back hint (returning user).
/// `width` is the text column width for prose wrapping (same `width` the
/// transcript body is wrapped to). Pure; no terminal or state.
pub fn empty_state_lines(first_time_user: bool, width: usize) -> Vec<Line<'static>> {
    // ...
}
```

Styling uses `tokens::color` (no literals):
- Title line: `color::CHAT_ACCENT`, bold
- Intro / hint lines: `color::DIM`
- Prompt lines: `color::TXT` with the `›` marker in `color::CHAT_ACCENT`
  (reuses `glyph::USER_TURN` for visual consistency with user-turn rows)

Word-wrapping reuses the existing `render::wrap_plain` helper (same one the
question card uses), so long lines wrap cleanly at any width without
panicking.

### 4. Rendering details

**New user** (`first_time_user == true`, `msgs.is_empty()`):

```
  zoid — a coding agent for your terminal

  Try one of these to get started:
    › explain this codebase to me
    › fix the failing tests
    › add a feature from docs/TODO.md
```

- Title line: `color::CHAT_ACCENT`, bold
- "Try one of these…" line: `color::DIM`
- Prompt lines: `color::TXT`, `›` glyph in `color::CHAT_ACCENT`
- All lines indented 2 spaces (matches the existing `(no messages yet)` indent)

**Returning user** (`first_time_user == false`, `msgs.is_empty()`):

```
  welcome back — type a message, or :resume to pick up another session
```

- Single dim line (`color::DIM`), 2-space indent (same visual position as the
  old `(no messages yet)`)

### 5. Edge case: returning user opens a new/empty session

A returning user who runs `:new` (or resumes an old empty session) also hits
the empty state. Since `first_time_user` is computed once at boot from
`sessions.is_empty()`, it stays `false` for the whole run — so a returning
user opening `:new` sees the "welcome back" line, not onboarding. That's the
correct behavior: they already know what zoid is; they don't need onboarding
again.

The `:new` path (`Command::NewSession` in `exec_command`) resets
`app.events`, `app.proj`, and `app.body_cache`. The next frame,
`app.proj.msgs.is_empty()` is true, the intercept fires, and
`first_time_user` (still `false`) selects the returning-user copy. No
additional wiring needed — it falls out naturally.

### 6. Testing

Three unit tests in `onboarding.rs` (pure, no terminal):

1. **`new_user_renders_title_and_prompts`** — asserts the title,
   "Try one of these", and all three prompt strings appear in the output, and
   the title line carries the accent color.
2. **`returning_user_renders_welcome_back_only`** — asserts the welcome-back
   string appears and none of the onboarding prompts do.
3. **`wrap_respects_width`** — with a very narrow width (e.g. 20), long lines
   wrap without panicking (regression guard for `wrap_plain`).

**Existing snapshots are unaffected.** The snapshot tests that show
`(no messages yet)` (`chat_snapshot__empty_chat_frame.snap`, etc.) live in
`zoid-tui` and call `conversation_lines` / `render_chat` directly — they
bypass the bin's intercept, so they keep showing `(no messages yet)`. That's
fine: those test the *renderer's* empty fallback (the `build_conversation`
early-return), not the bin's onboarding intercept. New snapshots cover the
onboarding module's output.

### 7. Scope

| Change | File | Size |
|--------|------|------|
| New module | `crates/zoid-tui/src/onboarding.rs` | ~40-50 lines |
| New field | `crates/zoid-tui/src/state.rs` (`first_time_user: bool` + default) | ~2 lines |
| Module registration | `crates/zoid-tui/src/lib.rs` (`pub mod onboarding;`) | 1 line |
| Bin intercept | `crates/zoid/src/main.rs` (`run()` body-building block) | ~6 lines |
| Unit tests | `crates/zoid-tui/src/onboarding.rs` | ~30 lines |

**No signature changes** to `build_conversation`, `conversation_view`,
`conversation_view_indexed`, `detail_lines`, or `render_shell`.