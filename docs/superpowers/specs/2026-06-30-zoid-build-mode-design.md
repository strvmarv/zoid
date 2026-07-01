# zoid — Build Mode · Design (autonomous loop)

**Date:** 2026-06-30 (extracted from the 2026-06-28 combined design)
**Status:** **Deferred — design captured, implementation follows the Chat sequence.** The full Build design is specified here so it is ready when we return to it; it does not begin until the near-term Chat work (P0–P5 + the mode seam) lands.
**Author:** strvmarv (with Claude)

> **Spec set.** This is one of three documents. It layers on top of the shared core and assumes the Chat spec:
> - **Core architecture** → `2026-06-30-zoid-core-architecture.md` (the event-sourced spine, the modal seam, the **shared subagent runtime** §4.4, data model, providers/tools, extensibility, testing, visual language, and the canonical roadmap).
> - **Chat mode** → `2026-06-30-zoid-chat-mode-design.md` (the near-term surface: conversation + manual implementation + **single hand-dispatched subagent** from P5; owns the context **economy** — ledger/heat/churn and the **constructed-context assembler**).
> - **Build mode** → *this doc* (deferred: the autonomous 7-phase loop).
>
> **What this doc does and does not spec.** Build's distinctive contribution is the **scheduler that automates the shared runtime across a whole plan** — one subagent per task, in dependency order — plus the autonomy contract, blockers, notifications, and the finalize bookend. It therefore **references, and does not re-specify**: the event log & mode-seam mechanics (core §4.1–§4.2), the **subagent executor / worktree / constructed-context internals** (core §4.4), the **economy ledger/heat and the assembler** (Chat spec), the design-tokens module, and the test harness (core §8). Where a mechanism is shared, this doc points at its owning section rather than restating it.

---

## 1. Overview

**Build is the *entire* autonomous loop as a stepped pipeline.** Where Chat is human-driven turn by turn, Build steps through **brainstorm → spec → worktree+baseline → plan → execute → final review → finalize**, running autonomously between two human bookends. **Entering Build is the act of consent to autonomy**; Chat is the manual escape hatch. No mode mixes the two behaviors (core §1, §4.2).

Accent color **amber** (the finalize step uses a **green** accent within Build; core §Visual Language). Canonical mockups live under `docs/ux/`: `build-mode.html` (execute surface), `build-pipeline.html` (the stepped pipeline), `blocker-notifications.html` (escalation), `finalize-and-decisions.html` (bookend 2), and `modes.html` (Chat↔Build).

**Build is the first *additional* `impl Mode` behind the mode seam.** The seam itself (open `ModeId` + `trait Mode` + `ModeRegistry`, with Chat as the non-removable floor) is a **core/Chat deliverable that lands before Build** — see core §4.2 and the sequencing note there. Build never introduces the seam; it is simply the first mode authored behind it, which is what keeps the seam honest (core §4.2, two built-ins).

Everything below reuses Chat's subagent runtime and economy. Build's new code is the **automation layer**: the L2 scheduler (§5), the autonomy contract's enforcement (§2), and the finalize bookend (§3g).

---

## 2. The autonomy contract `[V1 vision; deferred to P6–P9]` (foundational)

The human engages at **two bookends**; the agent is **autonomous in between**.

1. **Before — brainstorm → spec → plan** (the *opening steps of Build mode*, §3). Two artifacts, two approvals: a high-level **spec** (intent: goal, approach, **required acceptance criteria**, non-goals — checked against your goals), then a detailed **plan** the agent derives by **reading the actual codebase** (a task DAG grounded in real modules, each spec criterion mapped to a task's self-gate, with risks/assumptions surfaced). This is the high-engagement bookend. Both are plain markdown (`spec.md` / `plan.md`) — the source of truth, à la superpowers (brainstorming → writing-plans).

   > **Modes are isolated.** **Chat** = conversation + *manual* implementation only (you drive; no spec/plan/subagent-loop/autonomy). **Build** = the *entire* autonomous loop, entered at its first step and stepping through brainstorm → spec → plan → execute → finalize. Entering Build is the act of consent to autonomy; Chat is the manual escape hatch.
2. **Execute** (autonomous). The orchestrator drives the plan to completion, **dispatching one subagent per task in sequence** (the shared runtime, core §4.4): each task **self-gates** (TDD + review subagents) and **auto-retries/fixes** on failure. No per-step prompts. The user may *watch* (Build is a monitor) but is not expected to.
3. **Blocker → escalate** (the only mid-execution interrupt). On a genuine blocker the agent **pauses execution, fires a notification, and presents the decision**; the user answers; work resumes from the log.
4. **After — finalize** (Build's last step). Review the finished branch + the **decisions log**, then merge / PR / send-back / discard.

**Autonomous-only.** There is **no per-step approval dial** (core §2 non-goal). The single interrupt is a blocker.

### 2.1 Blocker taxonomy (escalate, don't guess)
- **Unresolvable intent ambiguity** — the task admits multiple defensible readings the spec doesn't settle.
- **Outward-facing / irreversible actions** — force-push to main, prod/network writes, deleting external data, spending money. These *escape the sandbox* and are **blockers by definition**, never executed unilaterally regardless of settings.
- **Repeated self-gate failure after N retries** — a task's review pipeline (§5) can't be made to pass.
- **A missing capability / credential / dependency** — the run needs something the environment doesn't provide.
- **An unsafe-to-auto-resolve conflict** — e.g., a merge/rebase conflict whose resolution isn't mechanical.

### 2.2 Why autonomous-only is safe (deliberate stance, not an oversight)
All execution happens in **isolated git worktrees** (core §4.4) and nothing reaches `main` until the finalize bookend, so a bad autonomous decision is **recoverable by discarding the branch** (worst case: wasted tokens, not damaged main). The genuinely dangerous actions *escape the sandbox* and are therefore blockers by definition. Recoverability is **structural**, so no approval prompts are needed.

### 2.3 Notifications `[V1 vision; P8]`
Because the user walks away, zoid must pull them back via **four channels**:
1. a **persistent in-app badge** (`⛔ N blocked` in the title + status bar, stays until resolved — a missed ping is never fatal),
2. **terminal bell**,
3. **OS notification**,
4. a configurable **`notify-cmd` hook** (run any command → ntfy/Slack/phone for headless/SSH/away).

Fires on **blocker · completion · budget-leash trip** (§7). Channel toggles + `notify-cmd` are deferred config (core §7.1 `[POST-V1]`).

### 2.4 Blocker presentation by type
- **intent ambiguity** → pick `[1]/[2]` / describe;
- **outward-facing / irreversible consent** → `approve once / deny / show command`, labeled **"blocker by definition — never auto-done regardless of settings"**;
- **repeated self-gate failure** → retry-with-hint / skip / take-over / abandon;
- **missing capability / credential** → provide / skip.

The follow-stream (§3e) explains *why* it escalated; answering resumes execution. Canonical mock: `docs/ux/blocker-notifications.html`.

---

## 3. Build mode stepped pipeline `[V1 vision; deferred to P6–P9]`

Build *is* the superpowers 7-phase loop (§5), entered at its first step and stepping through. The two human approvals (spec, plan) are at the front; everything after is autonomous until finalize. Each step has its own layout. Canonical mocks: `docs/ux/build-pipeline.html` (the pipeline) and `docs/ux/build-mode.html` (the execute surface).

**Steps & layouts**

- **(a) Brainstorm** — chat-like; the conversation that carried in from Chat continues here (§4), now goal-directed toward a spec (Socratic clarifying questions, approach proposal).
- **(b) Spec** *(approval — bookend 1)* — an inline **spec card** (goal, approach, **required acceptance criteria**, non-goals); checked against your goals. `⏎` approve.
- **(c) Worktree + baseline** — auto: create isolated branch/worktree (core §4.4), establish a clean test baseline.
- **(d) Plan** *(approval + pre-flight — bookend 1)* — agent **reads the codebase**, drafts an inline **plan card**: a task DAG grounded in real modules (bite-sized TDD tasks, dependency-ordered), each acceptance criterion mapped to a task's gate, risks/assumptions surfaced; a **pre-flight scan** checks the plan against its global constraints before task 1. `⏎` approve.
- **(e) Execute** *(autonomous monitor)* — the **2-pane + rail** working surface. **You may watch; you don't operate it.** Canonical mock `docs/ux/build-mode.html`:
  - **① Overview** (*status*) — phases, tasks with progress gauges, per-task review-pipeline status (`tdd ✓ · spec ✓ · quality ✓ · merged`), a progress sparkline (tasks completed over time, Ⓡ4); a blocker shows `⛔ needs you`.
  - **② Follow-stream** (*reasoning*) — opt-in stream at a chosen **altitude**: orchestrator by default, `f` / select-task to drill into the active subagent's raw stream.
  - **Rail** (per-mode drawer set, core §4.3): **⑤ economy** `^E`, **B changed-files tree** `^K`, **D steering** `^G`.
  - **Keymap**: `Tab` cycle focus (forward) · **`⇧Tab` switch mode** · `^Z` zoom pane/drawer · `j/k` select task · `f` follow · `^E/^K/^G` drawers · `esc` / `:chat`.
  - **Responsive**: 2-pane+rail → fewer panes / tabbed single-pane (**min widths: stream ≥~50, rail ≥~28**).
- **(f) Final review** — a distinct **broad whole-branch review** (separate from per-task reviews); findings by severity; criticals loop back to fixes.
- **(g) Finalize** *(bookend 2)* — the finish surface, canonical mock `docs/ux/finalize-and-decisions.html`. **Left:** summary + the **autonomous-decisions log** (the judgment calls made while you were away — "chose token-bucket because…", each `⏎`-navigable to its diff; the **primary trust mechanism**). **Right:** changed-files tree + diff preview (Ⓡ3). **Actions:** `[a] merge → main` · `[p] open PR` · `[c] request changes → new loop` · `[d] discard branch`. Verifies tests, then cleans up the worktree.

A finished Build collapses to a **summary card** back in Chat (re-expandable, ① zoom). The shared event log means switching Build⇄Chat never loses state (§4).

---

## 4. Mode transitions `[V1 vision]` (spans both modes — owned here)

This section covers Chat↔Build transitions in both directions; the Chat spec merely references it.

**Chat → Build** (`⇧Tab` / `:build`, or by accepting the Chat-side "switch to Build?" suggestion — Chat spec §2, which owns the detect-and-offer behavior and the never-without-consent rule) is *continuous*: the existing conversation **carries straight into Build's brainstorm step** (no re-explaining — you just keep talking and the pipeline begins).

**Build → Chat** (`⇧Tab` / `esc` / `:chat`) drops to **manual**: Build keeps running underneath, and a finished Build leaves a **re-expandable summary card** in Chat (① zoom).

**No state lost.** Because both modes fold the **same event log** (core §4.1), switching never copies or drops state — it swaps the active surface (core §4.2). Entering Build is the autonomy consent; returning to Chat is manual control. Canonical mock: `docs/ux/modes.html`.

---

## 5. The Workflow Engine (Build's brain) `[V1 vision; L2 deferred to P6–P9]`

Build faithfully implements obra **superpowers'** 7-phase loop:

1. **Brainstorm** → `spec.md` (Socratic; required acceptance criteria).
2. **Worktree + baseline** → isolated branch/worktree (core §4.4), clean test baseline.
3. **Plan** → `plan.md` (`docs/superpowers/plans/`): bite-sized **TDD tasks**, exact file paths, global constraints, dependency DAG; a **pre-flight scan** validates the plan vs its constraints before task 1.
4. **Subagent-driven execution** → per task: a **fresh implementer** subagent with **constructed context** (never session history) implements the task, then a **two-stage review** — *spec-compliance* then *code-quality* — and a **fix subagent** loops on Critical/Important findings before the task is marked done. The orchestrator runs tasks **sequentially — one subagent at a time** (core §4.4), each in its own worktree. **Continuous** — no human between tasks.
5. **TDD** → the inner loop of every task: write-failing-test → run-fails → minimal-impl → run-passes → commit.
6. **Final broad review** → a whole-branch review distinct from per-task reviews; criticals loop back.
7. **Finalize** → verify tests, decisions log, merge/PR/discard, worktree cleanup.

### 5.1 The L1/L2/L3 staircase (additive, not separate builds)
- **L1 — Runtime (P5, *Chat's* deliverable):** the orchestrator + a **sequential** subagent executor + worktree isolation + the constructed-context assembler. This is the **shared runtime from core §4.4 / the Chat spec** — Build does **not** re-implement it. (Async background **delegates ⑧c** reuse this runtime but are deferred — core §9.)
- **L2 — Scheduler (P6–P9, *Build's core new work*):** turns `plan.md` into a **task DAG** and walks it **in dependency order, one task at a time**, dispatching the per-task **implementer → spec-review → quality-review → fix** pipeline over the L1 runtime, escalating blockers, then running the **final broad review** and driving the Build UI through finalize. Autonomous; failures auto-retry; only a blocker escalates (§2). **The scheduler is a *driver* over the shared runtime, not a second implementation.**
- **L3 — Interpreter (post-roadmap; data-ready from P6):** **recipes** as first-class authorable artifacts (declarative phase/gate sequences, like superpowers skills). The built-in 7-phase loop becomes "one recipe executing." Users author their own.

> **Critical architectural constraint (from day one):** model the **phase / task / gate / review structure as DATA**, so L3 is "add an interpreter over existing primitives," **not a rewrite.** The workflow/decision event types are already reserved in the core schema (core §5) though only Build emits them.

---

## 6. Build-only projections `[V1 vision; P6–P9]`

Core §5 reserves the workflow/decision event types and names these projections as **Build-owned**; they are defined here. All are pure functions of the log + a head (core §4.1).

- **`WorkflowBoard(workflowId)` (P6–P8)** — folds `WorkflowStarted` / `TaskStateChanged` / `SelfGateResult` / `BlockerRaised` / `BlockerResolved` into the **Build Overview** ①: phases, tasks with progress gauges, per-task review-pipeline status (`tdd ✓ · spec ✓ · quality ✓ · merged`), and blocker state (`⛔ needs you`).
- **`ChangedFiles(workflowId)` (P7)** — a **files × tasks tree** with diff stats. Files touched by **more than one task are flagged `⚠`** for review. Powers Build rail drawer **B** (`^K`) and the finalize changed-files view.
- **`DecisionsLog(workflowId)` (P9)** — **filter to `Approval` / `Decision` / `BlockerResolved` events**; the **finalize audit surface** (§3g) and the run's primary trust mechanism. Replays the two bookend-1 approvals ("you approved plan v2 at 14:05") alongside every autonomous judgment call, each `⏎`-navigable to its diff.

---

## 7. Build economy notes `[V1 vision]`

Build **reuses** the economy subsystem — the `TokenLedger`, usage heat, churn timeline, and the **constructed-context assembler** are all defined in the **Chat spec** (P3). Build adds two automated, at-scale uses:

- **Subagent leash across a *sequence* of dispatches:** each dispatched subagent reports its token spend into the same ledger; a **budget policy pauses before dispatching the *next* subagent** when budget is low (a budget-leash trip fires a notification, §2.3). This is the Chat leash applied across the whole plan rather than a single hand-dispatch. (Full multi-agent roster/autonomy UI follows parallelism — post-roadmap, core §9.)
- **Orchestrator context construction at scale:** in Build, the economy is not just *the user's* view — it is the mechanism by which the orchestrator **assembles each subagent's constructed context** (plan task + relevant code, never session history; core §4.4). Context curation is the orchestrator's core job; the **same ledger/heat machinery** scores what each implementer should see. Build is the **automated, at-scale use of the Chat spec's assembler** — see that spec for the assembler, ledger, and heat themselves.

Denomination is **tokens, never dollars** (core §2, model-agnostic). The **token governor** — an optional per-task token ceiling with warn / auto-compact / pause — is defined in the Chat spec (§5.3) and applies per Build task; no dollar governor, no model auto-routing in v1.

---

## 8. Providers, tools & Build safety `[V1 vision]`

Build uses the shared provider/tool interface (core §6) — this section covers only Build's execution context and safety stance.

- **Tools run autonomously inside per-task worktree sandboxes.** Unlike Chat (tools in the working directory, human-driven and visible), Build's tools execute **without routine per-call approval** inside each task's isolated worktree. Tool calls/results are events in both modes.
- **Build is safe by isolation + the finalize bookend.** Work happens in worktrees; nothing reaches `main` until you finalize, so in-sandbox actions are recoverable and need no prompts (§2.2).
- **Outward-facing / irreversible actions that escape the sandbox are blockers, not executed** — force-push to main, prod/network writes, deleting external data, spending money (§2.1). They are escalated, never performed unilaterally.
- All actions and escalations are **events** (auditable, replayable) and surface in the finalize decisions log (§6). Permission review (a semantic risk gate) is deferred behind the `PermissionReviewer` seam (core §6, §9).

---

## 9. Build resilience `[V1 vision]`

Extends the shared resilience story (core §10, which covers streaming/crash-resume/subagent-failure/orphan-worktree reclaim) with Build's autonomous-loop specifics:

- **Self-gate failure → autonomous retry/fix → escalate after N.** A failed tdd/spec/quality gate triggers **autonomous retry/fix** (the fix subagent loops on Critical/Important findings); after **N retries** it **escalates as a blocker** (§2.1) rather than silently proceeding or merging.
- **Blocker escalation:** **pause execution → notify** (§2.3) → present the decision → **resume from the log** (the answer is an event; execution continues deterministically from where it paused).
- **Worktree cleanup:** worktrees are created per task and removed on completion/abandonment; the finalize step cleans up the run's worktree; orphans are reclaimable (core §10).

---

## 10. Build's slice of the roadmap — P6–P9 `[deferred beyond the Chat sequence]`

Core §9 holds the **canonical** sequence and establishes that **P0–P5 + the mode seam precede this** (Chat is realized completely first; the economy P3 is the substrate Build's context construction reuses; the seam lands at the Chat→Build boundary). The phases below are **Build deliverables** — the captured-but-deferred design. Although the original combined spec tagged Build `[V1]`, in this split Build is **deferred** (post the Chat sequence); the phase content is intact, the framing is deferred.

| Phase | Lands (Build deliverable) | Runnable end-state |
|---|---|---|
| **P6 · Build front-half (L2a)** | Build as the **first additional** `impl Mode` (seam already present); stepped-pipeline shell; **continuous Chat→Build** switch (§4); brainstorm → spec card ✓ → worktree+baseline → plan card ✓ (read-code + pre-flight); **phase/task/gate modeled as data** (§5.1) | Drive Build to an approved plan (execution stubbed / dry-run). |
| **P7 · Build execution — happy path (L2b)** | Scheduler walks the task DAG **in dependency order, one task at a time**; per-task implementer→spec-review→quality-review→fix loop w/ **TDD**; self-gates + auto-retry; the 2-pane (Overview · Follow-stream) + rail execute surface (§3e) | A plan with no blockers executes end-to-end autonomously; you watch it. |
| **P8 · Blockers & notifications (L2c)** | Blocker detection/classification/escalation (esp. outward-facing actions), pause→resume (§2, §9); the **4 notification channels + persistent badge** (§2.3) | A plan that hits ambiguity or an outward-facing action pauses, notifies, and resumes on your answer. |
| **P9 · Finalize bookend** | Final broad whole-branch review; finalize surface (**decisions log** + changed-files/diff + merge / PR / request-changes / discard, §3g) + worktree cleanup + tests verify; collapsed **summary card** back in Chat | The **full autonomy contract**, brainstorm → finalize. |

**Deferred beyond even Build (`[POST-V1]`, core §9):** L3 recipe interpreter (data-ready from P6) · parallelism (fan-out ③, branch race, conflict radar, roster/leash UI) · async background delegates ⑧c · ② Canvas mode · Ⓡ1 inline raster graphics · extensibility loaders · permission tier 1.5/2.

---

## 11. Build testing obligations `[V1 vision]`

Reuses the shared harness (core §8: `proptest` for pure projections, the fake provider for the agent loop, `git2` temp-repo integration tests for the runtime, `TestBackend`/`insta` for TUI fidelity). Build adds:

- **Scheduler DAG ordering:** simulate a plan DAG with fake tasks; assert **sequential dependency ordering** (topological, one task at a time), **gate blocking**, and that a **failed gate halts completion**.
- **Autonomy / blocker escalation:** assert the agent **escalates the blocker taxonomy** (esp. outward-facing actions) rather than executing; assert **self-gate failure → retry → escalate after N**; assert a blocker **pauses execution** and that **answering resumes from the log**.
- **Tool safety:** assert **outward-facing actions escalate (don't execute)** from inside a worktree sandbox.
- **Fidelity:** each Build screen (pipeline steps, execute 2-pane+rail, blocker presentation, finalize) ships a `TestBackend`/`insta` snapshot bound to its `docs/ux/` mock (core §8, §13).

---

## 12. Glossary (Build-specific)

- **Autonomy contract** — engage at two bookends (spec+plan approvals; finalize); autonomous in between; blocker-only interrupt (§2).
- **Bookend** — the two human-engagement points: Build's spec+plan approvals (front) and Build's finalize step (back).
- **Blocker** — the sole mid-execution interrupt; escalates a decision to the human (§2.1 taxonomy).
- **Decisions log** — projection of `Approval`/`Decision`/`BlockerResolved` events; autonomous choices surfaced at finalize for audit/trust (§6, §3g).
- **Per-task review pipeline** — TDD → spec-compliance review → code-quality review → fix subagent; runs autonomously per task (replaces the earlier single "critic").
- **Final broad review** — whole-branch review distinct from per-task reviews, before finalize (§5, step 6).
- **Spec / Plan** — the two bookend-1 artifacts: `spec.md` (intent + acceptance criteria) and `plan.md` (code-grounded task DAG); markdown source of truth.
- **Scheduler (L2)** — Build's core new work: the driver that turns `plan.md` into a task DAG and walks it over the shared runtime (§5.1).

Shared terms (event-sourced core, projection, mode, mode seam, rail, orchestrator, subagent, constructed context, AgentProfile, design tokens, Ⓡ1–4) are defined in core §11.

---

## 13. Build-specific risks

1. **Sequential-execution latency & worktree churn** — running one subagent at a time trades throughput for simplicity and safety; a large plan takes longer wall-clock, and each task creates/tears down a worktree. Acceptable for v1 (the user has walked away). Parallel fan-out is a deferred optimization (core §9); the phase/task model (§5.1) is built so it slots in without a rewrite.
2. **Autonomy trust & blocker recall** — autonomous-only relies on the agent *correctly* classifying blockers (esp. outward-facing actions) and on notifications actually reaching a user who walked away. Mitigate: a conservative blocker taxonomy biased to escalate (§2.1); the worktree+finalize recoverability backstop (§2.2); redundant notification channels (§2.3); the decisions log for after-the-fact audit (§6).
3. **Notification reliability across terminals / OSes / SSH** — the bell may be suppressed, OS notifications may be unavailable headless. Mitigate: multiple channels + the **persistent in-app blocked-state badge** so a missed ping is never fatal (§2.3).
