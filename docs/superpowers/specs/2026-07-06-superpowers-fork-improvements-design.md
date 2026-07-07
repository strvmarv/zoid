# Superpowers Fork Improvements

**Date:** 2026-07-06

## Goal

Three targeted improvements to the strvmarv/superpowers fork: (1) finish
wiring the gilfoyle-tech-reviewer persona as the default reviewer, (2)
prevent subagents dispatched into worktrees from editing the main checkout,
and (3) a conciseness pass that tightens skill prose and makes subagent output
leaner. All changes are skill-file-only (no Rust/zoid runtime changes).

## Scope

**In scope:** the skill files under `~/.config/zoid/modes/superpowers/` (the
imported mode that now tracks the strvmarv/superpowers fork):

- `agents/gilfoyle-tech-reviewer.md`
- `subagent-driven-development/SKILL.md`
- `subagent-driven-development/implementer-prompt.md`
- `subagent-driven-development/task-reviewer-prompt.md`
- `requesting-code-review/SKILL.md`
- `dispatching-parallel-agents/SKILL.md`
- `using-git-worktrees/SKILL.md`
- `test-driven-development/SKILL.md`
- `systematic-debugging/SKILL.md`
- `executing-plans/SKILL.md`
- `writing-skills/SKILL.md` (one note)

**Out of scope:** zoid Rust runtime changes, the mode import mechanism, the
`:mode update` provenance flow (already done), new skills, the brainstorming
visual companion server scripts.

## Improvement 1: Gilfoyle Reviewer Integration

### Problem

The gilfoyle-tech-reviewer agent persona exists and is partially wired into
the SDD and requesting-code-review skills, but:

- The agent's `description` frontmatter is bloated with `<example>` tags (~400
  characters of examples). This description loads into every context where the
  agent is advertised — pure token waste.
- The fallback path (when the agent file doesn't exist on a fresh install
  without the fork) is specified in the design doc but not verified in the
  actual skill text.
- A standalone design doc (`gilfoyle-reviewer-design.md`) lingers as a one-off
  planning artifact.

### Changes

1. **Trim the agent `description`** to a concise trigger-only description,
   following the SDO rules in `writing-skills/SKILL.md`:
   - No `<example>` tags, no workflow summary.
   - Format: "Use when [triggering conditions]."
   - Keep the persona body (the review perspectives, communication style) as-is.

2. **Verify the fallback path** in `subagent-driven-development/SKILL.md` and
   `requesting-code-review/SKILL.md`: when gilfoyle is unavailable (agent file
   absent), the reviewer falls back to a plain `general-purpose` subagent with
   the existing structured template. Add explicit fallback language if missing.

3. **Remove `gilfoyle-reviewer-design.md`** — it's a spent planning artifact,
   not a permanent reference. The integration lives in the skills themselves.

### Acceptance Criteria

- The agent `description` is under 200 characters, contains no `<example>`
  tags, and starts with "Use when."
- Both SDD and requesting-code-review skills explicitly state the fallback
  when the gilfoyle agent file is absent.
- `gilfoyle-reviewer-design.md` is deleted.
- The persona body is unchanged (still multi-perspective, still Gilfoyle).

## Improvement 2: Subagent Worktree Safety

### Problem

A subagent dispatched into a worktree edits the main checkout. Root cause: the
controller passes a **relative** worktree path to the subagent. The subagent
resolves that relative path against its inherited cwd (often the main
checkout), silently routing all file edits to main instead of the worktree.
This happens in both zoid and Claude Code.

### Changes

1. **`subagent-driven-development/SKILL.md`** — add to the dispatch guidance
   (File Handoffs section): the controller MUST resolve the worktree path to an
   absolute path (`$(cd "$WORKTREE_PATH" && pwd -P)`) before passing it to the
   subagent, and pass it as an absolute path in the dispatch prompt's "Work
   from:" field. Add a Red Flag: "Never pass a relative worktree path to a
   subagent — it resolves against the main checkout."

2. **`subagent-driven-development/implementer-prompt.md`** — the template's
   "Work from: [directory]" line becomes "Work from: [ABSOLUTE PATH]". Add a
   self-check step before the implementer begins: "Before starting, run `pwd`
   and verify you are in the directory named above. If not, STOP and report
   BLOCKED — you may be in the main checkout."

3. **`using-git-worktrees/SKILL.md`** — add a "Subagent Isolation" note: when a
   subagent runs in a worktree, pass the worktree's absolute path. A relative
   path resolves against the subagent's inherited cwd (often the main
   checkout), silently routing edits to main.

4. **`dispatching-parallel-agents/SKILL.md`** — same absolute-path requirement
   for parallel dispatch, since it has the same failure mode.

### Acceptance Criteria

- The SDD skill explicitly requires absolute worktree paths in dispatch and
  lists the relative-path failure as a Red Flag.
- The implementer prompt template has a `pwd` self-check before starting.
- `using-git-worktrees` and `dispatching-parallel-agents` both warn about the
  relative-path issue.
- No Rust changes.

## Improvement 3: Conciseness Pass

### Problem

Skill files are verbose in two ways: (A) the files themselves carry redundant
prose that bloats every load, and (B) subagent output (implementer reports,
reviewer reports) runs long with process narration and accumulated
prior-task summaries.

### Changes

#### A) Tighten skill files

1. **`agents/gilfoyle-tech-reviewer.md`** — trim `description` (covered by
   Improvement 1).

2. **`test-driven-development/SKILL.md`** — the rationalization table and the
   "Why Order Matters" section repeat the same excuses 3-4× across sections.
   Keep one rationalization table (the most comprehensive one), drop the
   redundant rest. Cut the "Why This Matters" narrative paragraphs that
   re-explain what the table already says.

3. **`systematic-debugging/SKILL.md`** — same pattern: the rationalization
   table, "Common Rationalizations," and the Red Flags list overlap heavily.
   Consolidate to one table + one Red Flags list. Cut the "Real-World Impact"
   stats sections (they're marketing, not guidance).

4. **`dispatching-parallel-agents/SKILL.md`** — cut the "Real Example from
   Session" and "Real-World Impact" sections (narrative storytelling, flagged
   as an anti-pattern in `writing-skills`). Keep the pattern and the agent
   prompt structure.

5. **`executing-plans/SKILL.md`** — trim the "When to Stop" and "When to
   Revisit" sections; they're verbose restatements of "stop when blocked, don't
   guess." Compress to a bullet list.

6. **`writing-skills/SKILL.md`** — add one note to the SDO Token Efficiency
   section: "Frequently-loaded skill files (mode body, using-superpowers) must
   stay under 200 words for the body. Other skills should stay under 500 words
   for the main body; push heavy reference to separate files."

#### B) Leaner subagent output

1. **`subagent-driven-development/implementer-prompt.md`** — add to the report
   contract: "Reports: findings only, no process narration. Lead with the
   verdict. Under 15 lines." (The template already says "under 15 lines" for
   the summary; extend it to cover the report file too.)

2. **`subagent-driven-development/task-reviewer-prompt.md`** — the template
   already says "no preamble, no process narration, no closing summary." Add an
   explicit line budget: "Your final message is the report itself, under 30
   lines."

3. **`subagent-driven-development/SKILL.md`** — reinforce the existing "Do not
   paste accumulated prior-task summaries into later dispatches" rule with a
   one-line reminder in the File Handoffs section: "A dispatch describes one
   task, not the session's history."

### Acceptance Criteria

- TDD and systematic-debugging each have exactly one rationalization table and
  one Red Flags list (no duplicates).
- `dispatching-parallel-agents` has no "Real Example" or "Real-World Impact"
  sections.
- The implementer prompt template caps report length.
- The task-reviewer prompt template has an explicit line budget.
- No skill loses its anti-rationalization structure (the remaining table/list
  covers every excuse the duplicates covered).
- `writing-skills` has the one-line token-budget note.

## Implementation Notes

- All edits target the mode files at `~/.config/zoid/modes/superpowers/`.
  After editing, `:mode reload` picks up changes without a restart.
- The mode directory is a materialized snapshot (not a git clone). To publish
  to the fork: clone `strvmarv/superpowers`, apply the same edits to the
  corresponding `skills/` files, commit, push. Then `:mode update superpowers`
  re-imports from the fork. For local testing, edit the mode files directly
  first, then mirror the changes to the fork clone.

## Non-Goals

- No new skills.
- No changes to the brainstorming visual companion server.
- No Rust/zoid runtime changes.
- No restructuring of the skill directory layout.
- No changes to the mode import/provenance mechanism (already pointed at the
  fork).