# TODO — deferred work

## Empty-state guidance for new vs. returning users (DONE)

Implemented in `crates/zoid-tui/src/onboarding.rs` + `crates/zoid/src/main.rs`.
See `docs/superpowers/specs/2026-07-06-empty-state-guidance-design.md`.

## Tool-call approvals (DONE)

Implemented across `crates/zoid-tools/src/approval.rs` (BlacklistGate +
shlex matcher), `crates/zoid/src/agent.rs` (Gate::Prompt arm), and config/CLI
wiring. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

## Delete old sessions from the session picker

Currently the session picker only lets you resume or browse past sessions —
there's no way to delete old/stale ones. Add a delete action (e.g. a keybinding
like `d` or a prompt confirm) to remove session entries from the picker and
clean up the underlying session storage.

## Reduce "thinking" output shown in normal zoom

When in normal (non-expanded) zoom, too much intermediate "thinking" text is
rendered, making the UI noisy. Trim or collapse the thinking content shown at
normal zoom so the main agent's actual messages and tool calls stay visible
without being drowned out by reasoning prose.

## Subagents not working correctly

Several issues with subagent execution and display:

- **Jumpy UI while running** — the subagent appears to write to the main output
  stream while it's still executing, causing the TUI to jump/flicker instead
  of showing a stable progress indicator.
- **Result doesn't land for main agent review** — after the subagent finishes,
  its result is not being surfaced back to the main agent so it can act on /
  review the output. The DelegationResult event seems to not arrive or not be
  consumed properly.
- **Running out of tool iterations** — subagents are exhausting their tool-iteration
  limit (currently ~1k) before completing, which shouldn't happen for reasonable
  tasks. Investigate whether iterations are being burned on retry loops,
  internal overhead, or a miscounted limit.

## Tool call rendering truncated too short in the UI

When a tool is invoked, the rendered tool-call line is truncated way too early,
losing important context. For example, a `shell` call shows only
`shell(command: cd /home/gomanjoe/source/zoid…)` and the result line shows only
`✓ shell →    Compiling zoid-core v0.5.0 (/home/go…` — both cut off after barely
one path/filename. Same for `update_tasks`, `edit`, etc.

The truncation seems overly aggressive — likely the available width isn't being
used, or the truncation threshold is set too low. Tool-call lines and their
result previews should use more of the column width before truncating (ideally
showing the full command/arguments up to the actual panel width, with a `peek`
action for the rest).