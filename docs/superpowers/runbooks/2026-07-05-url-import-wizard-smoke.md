# URL Import Wizard — Go/No-Go Smoke

**Purpose:** Answer the one non-unit-testable question: will the active model
produce a *valid, useful* mapping of a real GitHub skill tree onto the canonical
contract, and can the update flow reconcile upstream changes with local edits?

## Preconditions

- `$GITHUB_TOKEN` set (higher rate limit; optional for public repos).
- Built from the branch carrying Tasks 1-14. `cargo test --workspace` green.
- A scratch repo (the wizard writes to `~/.config/zoid/modes/`, user-global).

## Import smoke

1. Launch zoid in a scratch dir.
2. Run: `:mode import github.com/obra/superpowers/tree/main/skills`
3. When the model calls `propose_mode_mapping` and then `apply_mode_mapping`,
   review the proposal in the conversation + the AskUser overlay.
4. Approve (or Adjust if the mode/skill split is wrong).

### Outcome rubric (import)

- **PASS** — the model proposes `Superpowers` as the mode name, the ~13
  methodology skills as scoped skills, `using-superpowers` as the `mode.md`
  body, and skips the genuinely-irrelevant files (README, license,
  tests-for-upstream). On Approve, `~/.config/zoid/modes/superpowers/`
  materializes, `:mode` shows `Superpowers`, switching to it loads the skills,
  and `invoke_skill("brainstorming")` returns its body.
- **PARTIAL** — proposes a mapping but gets the mode/skill split wrong (e.g.
  `using-superpowers` as a skill instead of the mode body), or skips too much,
  or generates bad frontmatter the materializer rejects more than once.
- **FAIL** — never calls `propose_mode_mapping`, or proposes an empty/trivial
  mapping, or loops without converging.

## Update smoke

1. After a successful import, hand-edit one local skill body (e.g. add a comment
   to `brainstorming/SKILL.md`).
2. Simulate upstream changing two files: edit `~/.config/zoid/modes/superpowers/.zoid-provenance.json`
   to bump two `upstream_sha` values to fake "moved" SHAs, and add a new file
   entry to simulate "upstream added". (Or, if a real upstream ref moved, point
   the sidecar's `ref` at the new ref.)
3. Run: `:mode update Superpowers`
4. Review the model's merged mapping; Approve.

### Outcome rubric (update)

- **PASS** — the model's merged mapping carries the local edit, re-materializes
  the upstream-only-changed file, flags the both-changed one with its pick, and
  the on-disk result matches the approved mapping.
- **PARTIAL** — reconciles structure but drops or clobbers the local edit
   against the model's stated intent.
- **FAIL** — can't produce a coherent merged proposal.

## Decision gate

- **Import PASS + update PASS** → the wizard ships; the on-ramp is real.
- **Import PARTIAL** → prompt-engineering on the `propose_mode_mapping` tool
  description / the seed user message before shipping.
- **Import FAIL** → fall back to deterministic mapping (model-only-for-
  descriptions) with the provenance sidecar still shipping.
- **Update FAIL specifically** → ship import-only this slice, defer update.

## Recorded outcome

- Date run:
- Model / build commit:
- Import verdict (PASS / PARTIAL / FAIL):
- Update verdict (PASS / PARTIAL / FAIL):
- Observed mapping / reconciliation:
- Notes / next action: