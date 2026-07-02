# Archive — 2026-07-02

Artifacts for **Build mode**, which was deferred throughout v1 and never
implemented (the running app ships only a `render_build_placeholder` stub).
Archived here because the next chapter is being redesigned from scratch — the
original Build-as-7-phase-pipeline direction is superseded by a new design.

Everything else from v1 shipped and remains live under
`docs/superpowers/specs/` and `docs/superpowers/plans/`.

## Contents

- `specs/2026-06-30-zoid-build-mode-design.md` — the deferred Build-mode design
  (2-pane execute view, stepped pipeline, blocker escalation, finalize step).
- `ux/build-pipeline.html` — Build as a stepped 7-phase pipeline.
- `ux/build-mode.html` — Build execute step (Overview · Follow-stream + rail).
- `ux/finalize-and-decisions.html` — autonomy contract, blocker escalation, finalize.
- `ux/blocker-notifications.html` — blocker types + notification channels.
- `ux/_superseded-build-quad.html` — earlier 2×2 Build layout (already superseded pre-archive).
- `ux/_superseded-chat-scenarios.html` — early Chat scenes (already superseded pre-archive).

These remain valid references if the new Build design wants to borrow ideas;
they are simply out of the active spec/plan/UX set.
