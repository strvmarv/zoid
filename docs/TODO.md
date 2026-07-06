# TODO — deferred work

## Empty-state guidance for new vs. returning users (DONE)

Implemented in `crates/zoid-tui/src/onboarding.rs` + `crates/zoid/src/main.rs`.
See `docs/superpowers/specs/2026-07-06-empty-state-guidance-design.md`.

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