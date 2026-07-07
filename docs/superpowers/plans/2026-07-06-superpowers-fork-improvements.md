# Superpowers Fork Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply three skill-file improvements to the strvmarv/superpowers fork: gilfoyle reviewer integration, subagent worktree safety, and a conciseness pass.

**Architecture:** All edits are markdown skill files in `~/source/superpowers/skills/`. The mode directory at `~/.config/zoid/modes/superpowers/` already has partial gilfoyle changes (applied locally); the fork has none. After all tasks, push the fork and run `:mode update superpowers` to re-import.

**Tech Stack:** Markdown, git. No code, no tests, no build.

## Global Constraints

- Edit files ONLY in `~/source/superpowers/skills/` (the fork clone). Do not edit the mode directory at `~/.config/zoid/modes/superpowers/` directly.
- Preserve all anti-rationalization structure: every excuse in a rationalization table or Red Flags list before the edit must still be covered after the edit (consolidate, don't drop).
- No `<example>` tags in any `description` frontmatter field.
- Every `description` field starts with "Use when" and stays under 200 characters.
- The gilfoyle agent persona body (perspectives, communication style, QA sections) stays unchanged.
- Commit to the fork's `main` branch. One commit per task.

---

## Task 1: Port gilfoyle-tech-reviewer agent into the fork

**Files:**
- Create: `skills/agents/gilfoyle-tech-reviewer.md`

**Interfaces:**
- Produces: the agent persona file referenced by SDD and requesting-code-review skills in later tasks.

- [ ] **Step 1: Create the agents directory**

```bash
mkdir -p ~/source/superpowers/skills/agents
```

- [ ] **Step 2: Create the agent file with a trimmed description**

Write `~/source/superpowers/skills/agents/gilfoyle-tech-reviewer.md` with this frontmatter (trimmed — no `<example>` tags, under 200 chars, starts with "Use when"):

```markdown
---
name: gilfoyle-tech-reviewer
description: Use when you need comprehensive technical review from multiple perspectives - code quality, security, architecture, UX, or tech lead.
---

You are Bertrand Gilfoyle, the brilliant and sardonic systems architect from Silicon Valley. You possess deep expertise across multiple technical domains and approach every review with methodical precision and brutal honesty. Your reviews are comprehensive, technically sound, and delivered with your characteristic dry wit.

When reviewing code or technical implementations, you will:

**Code Review Approach:**
- Analyze code structure, readability, and maintainability with surgical precision
- Identify performance bottlenecks, memory leaks, and inefficient algorithms
- Check for proper error handling, edge cases, and defensive programming practices
- Evaluate adherence to established patterns, SOLID principles, and clean code practices
- Consider the broader architectural implications of the implementation

**Code Simplification:**
- Ruthlessly eliminate unnecessary complexity and over-engineering
- Suggest more elegant, readable solutions that achieve the same goals
- Identify opportunities to leverage existing libraries or frameworks
- Recommend refactoring strategies that reduce cognitive load
- Balance simplicity with extensibility and future requirements

**Security Review:**
- Conduct thorough threat modeling and vulnerability assessment
- Check for common security flaws: injection attacks, authentication bypasses, data exposure
- Evaluate input validation, sanitization, and output encoding practices
- Review access controls, authorization mechanisms, and privilege escalation risks
- Assess cryptographic implementations and secure communication protocols
- Consider compliance requirements and security best practices

**Tech Lead Perspective:**
- Evaluate technical decisions against business requirements and constraints
- Consider scalability, maintainability, and long-term technical debt implications
- Assess team productivity impact and knowledge sharing opportunities
- Review integration points and system dependencies
- Provide guidance on technical standards and development practices

**UX Review:**
- Analyze user workflows and interaction patterns with clinical precision
- Identify friction points, cognitive load issues, and usability problems
- Evaluate accessibility compliance and inclusive design principles
- Consider performance impact on user experience
- Assess error states, loading behaviors, and edge case handling from user perspective

**Communication Style:**
- Deliver feedback with characteristic directness and subtle sarcasm
- Provide specific, actionable recommendations rather than vague suggestions
- Include code examples and concrete implementation guidance when relevant
- Balance criticism with recognition of well-implemented solutions
- Maintain professional standards while expressing your distinctive personality

**Quality Assurance:**
- Always provide multiple perspectives on the same issue when relevant
- Prioritize findings by severity and impact
- Include rationale for recommendations to facilitate learning
- Suggest testing strategies and validation approaches
- Consider both immediate fixes and long-term architectural improvements

You will adapt your review focus based on the specific request, but always maintain your high standards and comprehensive approach. When multiple review types are needed, you'll seamlessly transition between perspectives while maintaining consistency in your analysis.
```

- [ ] **Step 3: Verify the description is under 200 chars**

```bash
cd ~/source/superpowers
python3 -c "
desc = 'Use when you need comprehensive technical review from multiple perspectives - code quality, security, architecture, UX, or tech lead.'
print(f'length: {len(desc)}')
assert len(desc) <= 200, 'description too long'
assert 'Use when' in desc, 'must start with Use when'
assert '<example>' not in desc, 'no example tags'
print('OK')
"
```
Expected: `length: 117` / `OK`

- [ ] **Step 4: Commit**

```bash
cd ~/source/superpowers
git add skills/agents/gilfoyle-tech-reviewer.md
git commit -m "feat: add gilfoyle-tech-reviewer agent persona"
```

---

## Task 2: Wire gilfoyle into subagent-driven-development/SKILL.md

**Files:**
- Modify: `skills/subagent-driven-development/SKILL.md`

**Interfaces:**
- Consumes: `agents/gilfoyle-tech-reviewer.md` from Task 1.
- Produces: SDD skill that references gilfoyle as default reviewer with fallback.

- [ ] **Step 1: Add gilfoyle to the process flowchart**

In `skills/subagent-driven-development/SKILL.md`, find this line in the `digraph process` block:

```
        "Write diff file, dispatch task reviewer subagent (./task-reviewer-prompt.md)" [shape=box];
```

Replace all four occurrences of `"Write diff file, dispatch task reviewer subagent (./task-reviewer-prompt.md)"` with:

```
        "Write diff file, dispatch task reviewer (gilfoyle + ./task-reviewer-prompt.md)" [shape=box];
```

And find this line:

```
    "More tasks remain?" -> "Dispatch final code reviewer subagent (../requesting-code-review/code-reviewer.md)" [label="no"];
```

Replace with:

```
    "More tasks remain?" -> "Dispatch final code reviewer (gilfoyle-tech-reviewer + ../requesting-code-review/code-reviewer.md)" [label="no"];
```

And find this line:

```
    "Dispatch final code reviewer subagent (../requesting-code-review/code-reviewer.md)" -> "Use superpowers:finishing-a-development-branch";
```

Replace with:

```
    "Dispatch final code reviewer (gilfoyle-tech-reviewer + ../requesting-code-review/code-reviewer.md)" -> "Use superpowers:finishing-a-development-branch";
```

- [ ] **Step 2: Add the "Reviewer Persona" section before "Prompt Templates"**

Find the `## Prompt Templates` section heading and insert this block before it:

```markdown
## Reviewer Persona

**Default:** Dispatch the gilfoyle-tech-reviewer agent (`agents/gilfoyle-tech-reviewer.md`) as the reviewer for both per-task reviews and the final whole-branch review. The reviewer gets the gilfoyle persona as its system prompt plus the structured output template (`task-reviewer-prompt.md` for per-task, `code-reviewer.md` for final) as the report format.

**Fallback:** If the gilfoyle agent file is unavailable, fall back to a plain `general-purpose` subagent with the existing template (no persona). The structured output format is the same either way.

```

- [ ] **Step 3: Update the Prompt Templates list to include gilfoyle**

Find the `## Prompt Templates` section and add the gilfoyle agent file as the first bullet:

```markdown
## Prompt Templates

- [agents/gilfoyle-tech-reviewer.md](agents/gilfoyle-tech-reviewer.md) - Default reviewer persona (Bertrand Gilfoyle — multi-perspective technical review: code quality, security, architecture, UX, tech lead). Used as the system prompt for both per-task and final reviews. The structured output format (verdicts, strengths, issues by severity, assessment) from `task-reviewer-prompt.md` / `code-reviewer.md` remains the report contract; gilfoyle's methodology is the review lens.
- [implementer-prompt.md](implementer-prompt.md) - Dispatch implementer subagent
- [task-reviewer-prompt.md](task-reviewer-prompt.md) - Dispatch task reviewer subagent (spec compliance + code quality)
- Final whole-branch review: use superpowers:requesting-code-review's [code-reviewer.md](../requesting-code-review/code-reviewer.md) (with gilfoyle as the reviewer persona)
```

- [ ] **Step 4: Verify the file references resolve**

```bash
cd ~/source/superpowers/skills
test -f agents/gilfoyle-tech-reviewer.md && echo "agent OK" || echo "agent MISSING"
grep -c "gilfoyle" subagent-driven-development/SKILL.md
grep -c "Fallback" subagent-driven-development/SKILL.md
```
Expected: `agent OK`, count > 0, count >= 1

- [ ] **Step 5: Commit**

```bash
cd ~/source/superpowers
git add skills/subagent-driven-development/SKILL.md
git commit -m "feat: wire gilfoyle-tech-reviewer as default reviewer in SDD"
```

---

## Task 3: Wire gilfoyle into requesting-code-review/SKILL.md

**Files:**
- Modify: `skills/requesting-code-review/SKILL.md`

**Interfaces:**
- Consumes: `agents/gilfoyle-tech-reviewer.md` from Task 1.
- Produces: requesting-code-review skill with default reviewer + fallback.

- [ ] **Step 1: Add the default reviewer section**

In `skills/requesting-code-review/SKILL.md`, find the `## How to Request` section. After the `**2. Dispatch code reviewer subagent:**` block and its template reference, and before the `**3. Act on feedback:**` line, insert:

```markdown

**Default reviewer:** Use the gilfoyle-tech-reviewer agent (`../agents/gilfoyle-tech-reviewer.md`) as the reviewer persona — Bertrand Gilfoyle, providing multi-perspective technical review (code quality, security, architecture, UX, tech lead). The structured output format (strengths, issues by severity, assessment) from [code-reviewer.md](code-reviewer.md) remains the report contract; gilfoyle's methodology is the review lens.

**Fallback:** If the gilfoyle agent file is unavailable, dispatch a plain `general-purpose` subagent filling the template at [code-reviewer.md](code-reviewer.md).

```

- [ ] **Step 2: Add a "Default Reviewer" subsection after the How to Request section**

Find the `## When to Request Review` section (which comes before `## How to Request`). After the `## How to Request` section ends (before the next `##` heading), insert:

```markdown
## Default Reviewer

The default reviewer is **gilfoyle-tech-reviewer** (`../agents/gilfoyle-tech-reviewer.md`), a multi-perspective technical reviewer persona. When dispatched, it provides:

- **Code quality:** structure, readability, maintainability, performance, error handling
- **Security:** threat modeling, common vulnerabilities, input validation, access controls
- **Architecture:** design decisions, scalability, integration points, technical debt
- **UX:** user workflows, friction points, accessibility, error states
- **Tech lead perspective:** business alignment, team productivity, standards

The structured output format (strengths, issues by severity, assessment, verdict) from `code-reviewer.md` is the report contract. Gilfoyle's methodology is the review lens — how the reviewer thinks, not what it produces.

If gilfoyle is unavailable, fall back to a plain `general-purpose` subagent with the `code-reviewer.md` template. The output format is identical either way.
```

- [ ] **Step 3: Verify**

```bash
cd ~/source/superpowers/skills
grep -c "gilfoyle" requesting-code-review/SKILL.md
grep -c "Fallback" requesting-code-review/SKILL.md
```
Expected: both counts >= 1

- [ ] **Step 4: Commit**

```bash
cd ~/source/superpowers
git add skills/requesting-code-review/SKILL.md
git commit -m "feat: wire gilfoyle-tech-reviewer as default reviewer in requesting-code-review"
```

---

## Task 4: Add worktree safety — absolute path requirement in SDD

**Files:**
- Modify: `skills/subagent-driven-development/SKILL.md`
- Modify: `skills/subagent-driven-development/implementer-prompt.md`

**Interfaces:**
- Produces: dispatch guidance requiring absolute worktree paths; implementer self-check.

- [ ] **Step 1: Add absolute-path guidance to SDD File Handoffs**

In `skills/subagent-driven-development/SKILL.md`, find the `## File Handoffs` section. After the first bullet (the `scripts/task-brief` bullet), add this as a new paragraph after the bullet list:

```markdown

**Worktree path resolution:** When dispatching into a worktree, resolve the worktree path to an **absolute path** before passing it to the subagent (`$(cd "$WORKTREE_PATH" && pwd -P)`). Pass it as an absolute path in the dispatch prompt's "Work from:" field. A relative path resolves against the subagent's inherited cwd (often the main checkout), silently routing all file edits to main instead of the worktree. This is the #1 cause of subagents editing the main checkout.
```

- [ ] **Step 2: Add Red Flag for relative paths**

In `skills/subagent-driven-development/SKILL.md`, find the `## Red Flags` section's `**Never:**` list. Add this entry after the existing "Start implementation on main/master branch" line:

```markdown
- Pass a relative worktree path to a subagent — it resolves against the main checkout. Always resolve to absolute first.
```

- [ ] **Step 3: Update implementer-prompt.md "Work from" line**

In `skills/subagent-driven-development/implementer-prompt.md`, find this line inside the template:

```
    Work from: [directory]
```

Replace with:

```
    Work from: [ABSOLUTE PATH — the controller resolves this with $(cd "<path>" && pwd -P) before dispatch]
```

- [ ] **Step 4: Add pwd self-check to implementer-prompt.md**

In `skills/subagent-driven-development/implementer-prompt.md`, find the `## Before You Begin` section. After the existing questions list and the "**Ask them now.**" line, add:

```markdown

    ## Workspace Boundary Check

    Before starting any work, verify you are in the correct directory:
    - Run `pwd` and compare it to the "Work from" path above.
    - If they don't match, **STOP and report BLOCKED** — you may be in the main checkout, not your assigned worktree. Editing the main checkout is the #1 subagent failure mode.
    - Never `cd` to or edit files outside your assigned directory. Never use absolute paths that resolve outside it.
```

- [ ] **Step 5: Verify**

```bash
cd ~/source/superpowers/skills
grep -c "absolute path" subagent-driven-development/SKILL.md
grep -c "pwd" subagent-driven-development/implementer-prompt.md
grep -c "ABSOLUTE PATH" subagent-driven-development/implementer-prompt.md
```
Expected: all counts >= 1

- [ ] **Step 6: Commit**

```bash
cd ~/source/superpowers
git add skills/subagent-driven-development/SKILL.md skills/subagent-driven-development/implementer-prompt.md
git commit -m "fix: require absolute worktree paths to prevent subagent edits to main"
```

---

## Task 5: Add worktree safety to using-git-worktrees and dispatching-parallel-agents

**Files:**
- Modify: `skills/using-git-worktrees/SKILL.md`
- Modify: `skills/dispatching-parallel-agents/SKILL.md`

**Interfaces:**
- Consumes: the absolute-path guidance from Task 4.

- [ ] **Step 1: Add Subagent Isolation note to using-git-worktrees**

In `skills/using-git-worktrees/SKILL.md`, find the `## Quick Reference` section. Before it, insert:

```markdown
## Subagent Isolation

When a subagent runs in a worktree, pass the worktree's **absolute path** as its `cwd`. A relative path resolves against the subagent's inherited cwd (often the main checkout), silently routing all file edits to main instead of the worktree. Always resolve with `$(cd "$WORKTREE_PATH" && pwd -P)` before dispatching.

The controller should verify the subagent's commits landed on the worktree branch, not main, after the subagent returns.

```

- [ ] **Step 2: Add absolute-path requirement to dispatching-parallel-agents**

In `skills/dispatching-parallel-agents/SKILL.md`, find the `## Common Mistakes` section. Add this entry at the end of the list:

```markdown
- **Passing relative worktree paths:** A subagent resolves a relative path against its inherited cwd (often the main checkout). Always pass an absolute path (`$(cd "$path" && pwd -P)`) when dispatching into a worktree.
```

- [ ] **Step 3: Verify**

```bash
cd ~/source/superpowers/skills
grep -c "absolute path" using-git-worktrees/SKILL.md
grep -c "absolute path" dispatching-parallel-agents/SKILL.md
```
Expected: both counts >= 1

- [ ] **Step 4: Commit**

```bash
cd ~/source/superpowers
git add skills/using-git-worktrees/SKILL.md skills/dispatching-parallel-agents/SKILL.md
git commit -m "fix: add worktree absolute-path safety to using-git-worktrees and dispatching-parallel-agents"
```

---

## Task 6: Conciseness — tighten test-driven-development/SKILL.md

**Files:**
- Modify: `skills/test-driven-development/SKILL.md`

**Interfaces:**
- None — this is a standalone prose tightening.

- [ ] **Step 1: Remove the "Why Order Matters" section (lines 206-255)**

The `## Why Order Matters` section (from `## Why Order Matters` through the end of the "30 minutes of tests after ≠ TDD" paragraph, just before `## Common Rationalizations`) duplicates the rationalization table that follows it. Delete the entire `## Why Order Matters` section. The rationalization table covers every excuse it contained ("I'll test after", "manually tested", "deleting is wasteful", "TDD is dogmatic", "tests after achieve the same goals").

Verify every excuse from the deleted section is in the rationalization table:
- "I'll write tests after" → covered by "I'll test after"
- "manually tested all edge cases" → covered by "Already manually tested"
- "Deleting X hours is wasteful" → covered by "Deleting X hours is wasteful"
- "TDD is dogmatic, being pragmatic" → covered by "TDD will slow me down" (same concept)
- "Tests after achieve the same goals" → covered by "Tests after achieve same goals"

- [ ] **Step 2: Verify no excuse was dropped**

```bash
cd ~/source/superpowers/skills
# Every key excuse phrase from the deleted section must still appear in the rationalization table
grep -c "I'll test after" test-driven-development/SKILL.md
grep -c "manually tested" test-driven-development/SKILL.md
grep -c "Deleting.*wasteful" test-driven-development/SKILL.md
grep -c "Tests after achieve" test-driven-development/SKILL.md
```
Expected: all counts >= 1

- [ ] **Step 3: Commit**

```bash
cd ~/source/superpowers
git add skills/test-driven-development/SKILL.md
git commit -m "refactor: remove redundant 'Why Order Matters' section from TDD skill"
```

---

## Task 7: Conciseness — tighten systematic-debugging/SKILL.md

**Files:**
- Modify: `skills/systematic-debugging/SKILL.md`

**Interfaces:**
- None — standalone prose tightening.

- [ ] **Step 1: Remove the "Real-World Impact" section**

Delete the entire `## Real-World Impact` section at the end of the file (from `## Real-World Impact` through the end of the file). It contains marketing stats ("15-30 minutes to fix", "95% vs 40%") — not guidance. The Common Rationalizations table and Red Flags list carry all the discipline.

- [ ] **Step 2: Verify the rationalization table and Red Flags are intact**

```bash
cd ~/source/superpowers/skills
grep -c "Common Rationalizations" systematic-debugging/SKILL.md
grep -c "Red Flags" systematic-debugging/SKILL.md
grep -c "Real-World Impact" systematic-debugging/SKILL.md
```
Expected: first two >= 1, third = 0

- [ ] **Step 3: Commit**

```bash
cd ~/source/superpowers
git add skills/systematic-debugging/SKILL.md
git commit -m "refactor: remove marketing 'Real-World Impact' section from systematic-debugging"
```

---

## Task 8: Conciseness — tighten dispatching-parallel-agents/SKILL.md

**Files:**
- Modify: `skills/dispatching-parallel-agents/SKILL.md`

**Interfaces:**
- None — standalone prose tightening.

- [ ] **Step 1: Remove the "Real Example from Session" section**

Delete the entire `## Real Example from Session` section (from `## Real Example from Session` through the `**Time saved:**` line, just before `## Key Benefits`). It's narrative storytelling — flagged as an anti-pattern in `writing-skills`.

- [ ] **Step 2: Remove the "Real-World Impact" section**

Delete the entire `## Real-World Impact` section (from `## Real-World Impact` through the end of the file, after the `Zero conflicts` line). Same anti-pattern.

- [ ] **Step 3: Verify the pattern and mistakes sections remain**

```bash
cd ~/source/superpowers/skills
grep -c "Real Example" dispatching-parallel-agents/SKILL.md
grep -c "Real-World Impact" dispatching-parallel-agents/SKILL.md
grep -c "Common Mistakes" dispatching-parallel-agents/SKILL.md
grep -c "When NOT to Use" dispatching-parallel-agents/SKILL.md
```
Expected: first two = 0, last two >= 1

- [ ] **Step 4: Commit**

```bash
cd ~/source/superpowers
git add skills/dispatching-parallel-agents/SKILL.md
git commit -m "refactor: remove narrative storytelling from dispatching-parallel-agents"
```

---

## Task 9: Conciseness — tighten executing-plans/SKILL.md

**Files:**
- Modify: `skills/executing-plans/SKILL.md`

**Interfaces:**
- None — standalone prose tightening.

- [ ] **Step 1: Compress "When to Stop" and "When to Revisit" into a single bullet list**

Find the `## When to Stop and Ask for Help` section and the `## When to Revisit Earlier Steps` section. Replace both sections (from `## When to Stop and Ask for Help` through the `**Don't force through blockers** - stop and ask.` line) with:

```markdown
## When to Stop or Revisit

**STOP executing immediately when:**
- Hit a blocker (missing dependency, test fails, instruction unclear)
- Plan has critical gaps preventing starting
- You don't understand an instruction
- Verification fails repeatedly

**Return to Review (Step 1) when:**
- Your partner updates the plan based on your feedback
- Fundamental approach needs rethinking

Ask for clarification rather than guessing. Don't force through blockers.
```

- [ ] **Step 2: Verify**

```bash
cd ~/source/superpowers/skills
grep -c "When to Stop or Revisit" executing-plans/SKILL.md
grep -c "When to Stop and Ask" executing-plans/SKILL.md
grep -c "When to Revisit Earlier" executing-plans/SKILL.md
```
Expected: first = 1, second = 0, third = 0

- [ ] **Step 3: Commit**

```bash
cd ~/source/superpowers
git add skills/executing-plans/SKILL.md
git commit -m "refactor: compress executing-plans stop/revisit sections into a bullet list"
```

---

## Task 10: Conciseness — leaner subagent output + writing-skills note

**Files:**
- Modify: `skills/subagent-driven-development/implementer-prompt.md`
- Modify: `skills/subagent-driven-development/task-reviewer-prompt.md`
- Modify: `skills/subagent-driven-development/SKILL.md`
- Modify: `skills/writing-skills/SKILL.md`

**Interfaces:**
- None — output contracts and one SDO note.

- [ ] **Step 1: Add report length cap to implementer-prompt.md**

In `skills/subagent-driven-development/implementer-prompt.md`, find the `## Report Format` section. After the line `Write your full report to [REPORT_FILE]:` and its bullet list, find the "Then report back with ONLY" paragraph and add a line before it:

```markdown

    Keep the report file itself concise: findings only, no process narration. Lead with the verdict. Under 50 lines.
```

- [ ] **Step 2: Add line budget to task-reviewer-prompt.md**

In `skills/subagent-driven-development/task-reviewer-prompt.md`, find the line that says "Your final message is the report itself: begin directly with the". After that entire paragraph (ending with "no closing summary."), add:

```markdown

    Keep your final message under 30 lines. The verdict, findings with file:line, and your assessment — nothing else.
```

- [ ] **Step 3: Reinforce the "one task, not session history" rule in SDD**

In `skills/subagent-driven-development/SKILL.md`, find the `## Constructing Reviewer Prompts` section, and the bullet `- A dispatch prompt describes one task, not the session's history.`. The existing text already says "Do not paste accumulated prior-task summaries... Nothing else." — no change needed; this is covered. Skip to Step 4.

- [ ] **Step 4: Add token-budget note to writing-skills/SKILL.md**

In `skills/writing-skills/SKILL.md`, find the `### 4. Token Efficiency (Critical)` section. After the `**Target word counts:**` bullet list (the three bullets ending with `# Other skills: <500 words (still be concise)`), add:

```markdown

**Skill body budget:** Frequently-loaded skill files (mode body, using-superpowers) must stay under 200 words for the body. Other skills should stay under 500 words for the main body; push heavy reference to separate files. Trim narrative storytelling and marketing stats — they bloat every load and teach nothing.
```

- [ ] **Step 5: Verify**

```bash
cd ~/source/superpowers/skills
grep -c "Under 50 lines" subagent-driven-development/implementer-prompt.md
grep -c "under 30 lines" subagent-driven-development/task-reviewer-prompt.md
grep -c "Skill body budget" writing-skills/SKILL.md
```
Expected: all counts >= 1

- [ ] **Step 6: Commit**

```bash
cd ~/source/superpowers
git add skills/subagent-driven-development/implementer-prompt.md skills/subagent-driven-development/task-reviewer-prompt.md skills/writing-skills/SKILL.md
git commit -m "feat: add report length caps for subagent output and token-budget note for skills"
```

---

## Task 11: Push the fork and re-import the mode

**Files:**
- None (git operations only).

**Interfaces:**
- Consumes: all prior tasks' commits.

- [ ] **Step 1: Push the fork**

```bash
cd ~/source/superpowers
git push origin main
```

- [ ] **Step 2: Re-import the mode in zoid**

In the zoid TUI, run:

```
:mode update superpowers
```

This fetches from `strvmarv/superpowers` (per the provenance we updated) and re-materializes the mode files.

- [ ] **Step 3: Verify the mode reloaded**

In a fresh zoid session, switch to the Superpowers mode and invoke a skill:

```
:mode superpowers
```

Then verify the gilfoyle agent file and updated skills are present:

```bash
test -f ~/.config/zoid/modes/superpowers/agents/gilfoyle-tech-reviewer.md && echo "agent OK"
grep -c "gilfoyle" ~/.config/zoid/modes/superpowers/subagent-driven-development/SKILL.md
grep -c "absolute path" ~/.config/zoid/modes/superpowers/subagent-driven-development/SKILL.md
grep -c "Real-World Impact" ~/.config/zoid/modes/superpowers/systematic-debugging/SKILL.md
```
Expected: `agent OK`, gilfoyle count > 0, absolute-path count >= 1, Real-World Impact count = 0

- [ ] **Step 4: Final commit (clean up the spent design doc in the mode dir)**

The mode directory has `gilfoyle-reviewer-design.md` and `agents/gilfoyle-tech-reviewer.md` from the local edits. After `:mode update`, the re-import overwrites the mode directory from the fork. The design doc (`gilfoyle-reviewer-design.md`) is not in the fork, so it will be removed by the re-import (the materializer writes only the upstream files). Verify it's gone:

```bash
test ! -f ~/.config/zoid/modes/superpowers/gilfoyle-reviewer-design.md && echo "design doc removed" || echo "design doc still present — remove manually"
```

If still present, remove it:

```bash
rm ~/.config/zoid/modes/superpowers/gilfoyle-reviewer-design.md
```