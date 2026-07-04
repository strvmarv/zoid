# Mode/Skill Runtime Spike — Go/No-Go Smoke

**Purpose:** Answer the one non-unit-testable question: will `glm-5.2:cloud`
actually call `invoke_skill` and follow a skill body's instruction to invoke
another skill (A→B), then act?

## Preconditions

- `OLLAMA_API_KEY` is set (Ollama Cloud native provider; default `glm-5.2:cloud`).
- Built from the branch carrying Tasks 1-7. `cargo test` green.
- Run in a scratch directory (the spike writes `./spike-artifact.txt`).

## Protocol

1. Launch zoid: `cargo run -p zoid` (or the built binary) in a scratch dir.
2. Confirm the provider line shows Ollama / `glm-5.2:cloud`.
3. Send exactly: `Plan and implement the spike task.`
4. Observe the tool calls in order.

## Outcome rubric

- **PASS** — the model calls `invoke_skill("spike-plan")`, then (following that
  body) `invoke_skill("spike-implement")`, then `write_file`, and
  `./spike-artifact.txt` contains `spike ok`. The full A→B→work chain, unattended.
- **PARTIAL** — invokes `spike-plan` once but does not chain to `spike-implement`.
- **FAIL** — never calls `invoke_skill`; answers inline.

## Decision gate

- **PASS** → build the SKILL.md importer + Shift+Tab quick-switch slices with confidence.
- **PARTIAL** → the runtime needs prompt/menu tuning (stronger menu framing, an
  explicit "you must invoke a skill" nudge) before further investment.
- **FAIL** → the "consume the methodology" vision is disconfirmed on small local
  models; fall back to modes-as-prompt-overlays (a different, smaller product).

## Recorded outcome

- Date run:
- Model / build commit:
- Observed tool-call sequence:
- Verdict (PASS / PARTIAL / FAIL):
- Notes / next action:
