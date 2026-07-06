# TODO — deferred work

## Empty-state guidance for new vs. returning users

**Problem.** A fresh session renders `(no messages yet)` (`chat.rs:87`, inside
`build_conversation`) and gives the user no guidance — no explanation of what
the app is, no suggested first actions. It feels like a dead-end rather than an
intentional empty state.

**Signal already available.** At startup (`main.rs:1164`) the bin already
distinguishes the two groups:

```rust
let sessions = session.list_sessions(Some(root.clone())).await.unwrap_or_default();
let (session_id, ...) = if let Some(s) = sessions.first() {
    // resume — returning user (prior session history exists)
    ...
} else {
    // create new — first-time user (zero history)
    ...
};
```

So `sessions.is_empty()` at boot is the "is this a new user" test. No auth,
localStorage, or extra persistence needed — session history across all
sessions is already stored server-side (SQLite `sessions` table) and fetched
here. Robust across devices/incognito because it's keyed per-user via the
session store.

**Gap.** The empty-state renderer (`build_conversation`, pure, in `zoid-tui`)
only receives `&[ChatMsg]` — it can't see whether session history exists. The
bin knows at boot but doesn't thread the flag down to the render path.

**Proposed approach (small, ~30-50 lines once copy is settled):**

1. Capture the flag at startup: `let first_time_user = sessions.is_empty();`
   and store it on `ShellState` (set once, never changes during a session).
2. Thread it to the render path: add `first_time_user: bool` to `ShellState`.
   The bin's body-building path already intercepts the empty case before
   `render_shell` — when `app.proj.msgs.is_empty()`, build onboarding lines
   (new user) vs. a "welcome back / start a new conversation" line (returning
   user) instead of falling through to `build_conversation`'s generic
   placeholder.
3. Render the actual empty-state content (the design decision, not the
   plumbing):
   - **New user:** a 1-line "what this is" + 2-3 example/suggested prompts.
   - **Returning user:** "welcome back" + maybe a hint to resume (`:resume`).

**Subtlety.** The current empty state also fires for a *returning* user who
opens a brand-new session (`:new`) or resumes an old empty one. So
`first_time_user` (computed once at boot from `sessions.is_empty()`) is the
right signal for *onboarding*, but the returning-user empty state should show
regardless of how they got there.

**Deferred because:** the plumbing is trivial; the real work is settling the
copy/design for each group. Pick that up when ready to implement.

## Tool-call approvals (dangerous-action blacklist + YOLO mode)

**Design:** [`APPROVALS.md`](./APPROVALS.md) — full design captured there;
this entry is a pointer.

**In brief.** The `ToolGate` seam already exists and is tested
(`crates/zoid-tools/src/lib.rs`; deny path covered by
`crates/zoid/tests/agent_loop.rs`), but only `AllowAll` ships. The work:

- Add `Gate::Prompt { question, choices }` so the sync `check` can request an
  interactive approval; the agent loop reuses the existing `ask_user`
  park-and-await overlay (no new UI).
- Ship a `BlacklistGate` with builtin dangerous-`shell` pattern defaults
  (destructive `rm`, force-push incl. `--force-with-lease`, non-GET `curl`,
  `sudo`, system/prod mutation, deploys) plus user-config additions/exemptions.
  Best-effort tokenizer matching, fail-safe toward prompting.
- Tiering: reads never prompt; `write_file`/`edit_file` allow by default;
  `shell` blacklist-gated.
- Subagents (headless) get the same blacklist as auto-deny instead of a prompt.
- YOLO mode: `AllowAll` selectable via `approval.yolo = true` or `--yolo`;
  never the default; `ask_user` is unaffected.

**Deferred because:** the design is settled; implementation is the next step.