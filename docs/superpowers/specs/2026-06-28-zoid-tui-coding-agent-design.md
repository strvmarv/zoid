# zoid — Cross-Platform TUI Coding Agent · Design

**Date:** 2026-06-28
**Status:** Approved design (brainstorming complete) — ready for implementation planning
**Author:** strvmarv (with Claude)
**Language decision:** **Rust** — chosen after a build spike comparing Rust vs .NET 10 on the design's riskiest axes (see `spikes/RESULTS.md`).

---

## 1. Overview

**zoid** is a cross-platform, terminal-native coding agent, built from scratch in **Rust**, distributed as a single self-contained native binary (~6 MB, validated by spike). It is an open-source product, not a one-off tool.

Its thesis is that current TUI coding agents are all variations on "chat log + sidebar." zoid instead treats **the conversation as a database** (an event-sourced log) and the UI as a set of queries over it, which unlocks interaction paradigms that chat-first tools can't cheaply copy: semantic zoom, object-first actions, a live token economy, async delegates, and a native, triggerable **plan→execute→verify workflow loop**.

The whole interface is **modal** (like vim/helix) with **two isolated modes**: **Chat** (conversation + *manual* implementation — you drive) and **Build** (the *entire* autonomous loop as a stepped pipeline). Entering Build is the act of consent to autonomy; Chat is the manual escape hatch. No mode mixes the two behaviors.

It runs on an **autonomy contract** (§6.0): the human engages at two bookends — *before* (Build's brainstorm → spec → plan, approved) and *after* (Build's finalize step) — and the agent works **autonomously in between**, interrupting only on a genuine **blocker**. zoid faithfully embodies obra's **superpowers** 7-phase loop (brainstorm → worktree → plan → subagent-driven execution w/ TDD → per-task review → final review → finish); see §7.

> **Vision vs v1.** This document describes the full vision. The shippable **v1 cut** is marked inline with **`[V1]`** (in v1) and **`[POST-V1]`** (deferred). Section 12 collects the v1 scope in one place.

---

## 2. Goals & Non-Goals

### Goals
- A coding agent that feels **at home on large, high-resolution terminals** without abusing the space (protect reading ergonomics; spend extra space on parallel state, not long lines).
- **Distribution as a single native binary** per platform — no runtime to install.
- **Workflow-native**: the brainstorm→plan→execute→verify loop (à la obra's *superpowers*) is a first-class, triggerable capability, not a habit the user has to bring.
- **Context as a first-class, user-managed resource**, measured in tokens, with real-time manual and automated control.
- **Extensible**: providers, tools, and workflows/recipes are pluggable artifacts — including **in-process sandboxed WASM plugins** (the spike proved `wasmtime` links trivially in Rust).
- **Autonomous between bookends**: once the plan is approved, the agent executes to completion without per-step engagement, escalating only on a blocker (§6.0).
- **Design fidelity**: the built TUI must match the canonical mockups in `docs/ux/`, enforced by snapshot tests (§16).
- **Maintainable for years** by a Rust contributor base; architecture chosen to keep borrow-checker friction low (message-passing over an append-only log).

### Non-Goals
- Not an IDE; no LSP-grade editing surface in v1 (we shell out to the user's editor when needed).
- Not a GUI/web app. Terminal only.
- No inline image/graphics-protocol rendering in v1 (no Sixel/Kitty) — designed-for via a render-backend seam (§3), shipped later (Ⓡ1).
- No multiplayer/collaboration in v1.
- No billing/dollar accounting — the economy is denominated in **tokens** (see §8).
- **No per-step approval dial.** zoid is autonomous-only between bookends; the sole interrupt is a blocker (§6.0). Dangerous outward-facing actions are blockers, not approval prompts.

---

## 3. Technology Decisions

| Decision | Choice (crate) | Rationale |
|---|---|---|
| Language | **Rust** (edition 2021) | Spike-validated: a true single ~6 MB binary (incl. WASM engine), ~10 ms cold start; best-in-class bespoke TUI rendering substrate; in-process WASM plugins; the most stable TUI ecosystem. Coding agents are I/O-bound, so the choice is about distribution + rendering ceiling + extensibility, not raw speed. |
| Distribution | **Single static binary** per target (linux-x64 musl, win-x64, osx-arm64, …) via cargo + GitHub Actions | Genuinely one file (spike: 6.2 MB), no runtime, no native sidecars — the casual-install story. |
| TUI engine | **`ratatui` + `crossterm`** | Immediate-mode cell buffer: we render whichever projection we want each frame, which is exactly what semantic zoom ① and custom surfaces (canvas ②) want. Most mature, most stable TUI stack. |
| App-framework layer (ours) | thin layer over ratatui: **focus ring, input/key routing, mouse hit-testing, pane/drawer manager** | The known cost of immediate-mode: ratatui gives a buffer, not focus/mouse. The spike confirmed this is ~modest hand-written code; we own it as a small internal module. Consider `tui-textarea` for the multi-line input and `tui-input` where useful. |
| Async runtime | **`tokio`** | Streaming + concurrent subagent fleet/delegates; integrates with `crossterm`'s `EventStream` via `tokio::select!` (spike pattern). |
| LLM transport | **`reqwest`** + SSE (`eventsource-stream`/`reqwest-eventsource`) + **`serde`/`serde_json`** | Streaming + tool-calling; serde is the de-facto serialization layer. |
| Persistence | Append-only event log; **SQLite via `rusqlite`** | Durable, queryable; supports projections, branching, resume. |
| Code intelligence | **`tree-sitter`** (+ grammars), diffs via **`similar`**, highlight via tree-sitter/`syntect` | Code-aware **symbol selection** for object-first verbs ④ (spike: found `fn` byte-range cleanly); precise diffs for the diff drawer. |
| Git / worktrees | **`git2`** (libgit2) | Per-task worktree isolation for the subagent fleet/delegates. |
| WASM plugins | **`wasmtime`** (in-process, sandboxed) | Spike: trivial to embed and statically linked into the single binary. Enables language-agnostic, capability-secured plugins (see §10). |
| Rendering backend | internal **`image-or-ASCII` trait** (ASCII v1; `ratatui-image` Kitty/Sixel/iTerm2 later) | Abstracts "draw a chart/diagram/graph" so inline raster graphics (Ⓡ1) drop in post-v1 without a rewrite. |
| Motion | internal **frame/transition engine** over ratatui | GC-free redraw enables animated transitions (Ⓡ2), gated by a motion budget + reduced-motion setting. |

**The one accepted cost (from the spike):** immediate-mode means we build the *app-framework floor* ourselves — focus ring, key routing, mouse hit-testing, pane/drawer manager. This is a bounded, well-understood internal module (the spike hand-rolled focus + mouse select without trouble), and it buys the rendering ceiling that motivated choosing Rust.

**Build-time note:** release builds with LTO are slow (spike: ~2m with `wasmtime`). Use fast dev builds for iteration; reserve full-LTO for release artifacts; consider `lto = "thin"` and workspace splitting to keep incremental builds quick.

---

## 4. Architecture

zoid is built in layers, each understandable and testable in isolation:

```
┌─────────────────────────────────────────────────────────────┐
│ Presentation: Modes (Chat · Build · Review) ── ratatui+crossterm│
│ mode = main-area layout + registered rail drawers + keymap     │
│ (over shared state) · design tokens (glyphs/colors) · motion   │
├─────────────────────────────────────────────────────────────┤
│ Projections: pure folds over the event log                    │
│ (transcript, context window, branch DAG, workflow board,      │
│  token ledger, churn timeline, decisions log, changed-files)  │
├─────────────────────────────────────────────────────────────┤
│ Core: Event-sourced Session                                   │
│ append-only log · heads/branches · fold engine                │
├──────────────┬───────────────────────┬────────────────────────┤
│ Agent Loop   │ Tool Execution        │ Subagent Runtime        │
│ (orchestr. + │ (fs, shell, search;   │ (fleet/delegate executor│
│  self-gates) │  autonomous; blocker- │  + worktree isolation;  │
│              │  gated, not approval) │  self-gate: tdd+critic) │
├──────────────┴───────────────────────┴────────────────────────┤
│ Providers (Anthropic, …) · streaming · tool-calling           │
└─────────────────────────────────────────────────────────────┘
```

### 4.1 Event-sourced core (the spine)
The session is an **append-only log of immutable events**. Visible state is a **pure fold (projection)** over that log. This single decision makes the novel features cheap:

- **Branching** = a new head pointing at an earlier event.
- **Undo / time-travel** = move a head backward; re-fold.
- **Multi-agent / delegates** = multiple heads over a shared or copied log.
- **Context window** = itself a projection (an editable one — see §8).
- **Workflow** = a sub-log of task/phase/gate events; its board is a projection.

Events are small, append-only, and serialized via `serde` (JSON, or a compact binary format like `bincode`/`postcard` if profiling warrants). The store is SQLite via `rusqlite` (log table + indices for fast projection and branch queries).

**Concurrency shape (Rust-specific):** the core owns the log behind a single writer; readers get immutable snapshots; subagents/delegates communicate via channels (`tokio::sync`) rather than shared mutable state. This actor/message-passing shape keeps borrow-checker friction low and makes the concurrent fleet safe by construction.

### 4.2 Modal state machine
The top level is a **state machine over the shared event log**. A *mode* is **`(main-area layout, registered rail drawers, keymap, projection set)`**. Switching modes never copies state — it swaps the active surface. This:
- dissolves keymap collisions (each mode owns its keys),
- makes "where does X render" a non-question (it renders in its mode's main area or a rail drawer),
- gives a clean extensibility seam: a new capability is "a new mode, or a tool/drawer registered into a mode" — a future Canvas/Review mode or a WASM plugin can register its own rail drawers without touching the core.

v1 modes: **Chat · Build** (§6). Build is a *stepped pipeline* (brainstorm → spec → plan → execute → final review → finalize); "Review/finalize" is Build's last step, not a separate mode.

### 4.3 The rail (per-mode drawers)
A reusable right-rail component hosts **stackable, collapsible drawers**; **each mode owns its own drawer set** (modes are isolated — the rail is a shared *component*, not shared *contents*). Chat: context-economy ⑤ / files / branch / palette. Build: economy ⑤ / changed-files-tree / steering. Panes are for what you watch continuously; rail drawers are for what you consult contextually.

### 4.4 Subagent runtime
A reusable executor that runs an agent turn in isolation: its own branch/head, optional **git worktree** for filesystem isolation, reporting results back as events. **Each subagent receives a precisely-constructed context — never the session history** (superpowers principle): the orchestrator assembles exactly what a task needs (its plan task + relevant code), which is why ⑤ context economy is the *orchestrator's core job*, not just a UI (§8). Per-task verification is a **review pipeline** (TDD → spec-compliance review → code-quality review → fix subagent), not a single critic; it auto-retries and escalates a **blocker** rather than prompting (§6.0, §7). Used by the Build fleet **`[V1]`**, delegates **`[V1]`**, and parallel race **`[POST-V1]`**.

---

## 5. Data Model (events & projections)

**Event (illustrative):**
```
Event {
  id: ulid               // monotonic, sortable
  parent: ulid?          // enables the DAG / branches
  branch: BranchId
  ts: long               // injected (no ambient clock in pure code)
  type: enum             // UserMessage | ModelDelta | ToolCall | ToolResult
                         // | ContextMutation | WorkflowStarted | TaskStateChanged
                         // | SelfGateResult | Decision | BlockerRaised
                         // | BlockerResolved | Merged | ...
  payload: <type-specific, serde-serialized enum variant>
  tokens: TokenStat?     // in/out/cached token counts where applicable
}
```

**Key projections (all pure functions of the log + a head):**
- `Transcript(head)` — ordered turns; supports **semantic zoom** ① by summarizing at variable granularity.
- `ContextWindow(head)` — current items + token cost + usage heat (⑤a).
- `ChurnTimeline(head)` — per-turn token deltas; flags re-sent items (⑤c).
- `BranchDAG()` — the graph of heads (powers time-travel; the **`[POST-V1]`** Canvas mode ②).
- `WorkflowBoard(workflowId)` — phases/tasks/self-gates/agents (Build overview).
- `ChangedFiles(workflowId)` — files×tasks tree with diff stats + `⚠` overlap flags (Build rail drawer B).
- `DecisionsLog(workflowId)` — filter to `Decision`/`BlockerResolved` events; the audit surface in Review.
- `TokenLedger(scope)` — the economy (§8).

---

## 6. Interaction Model & Modes

### 6.0 The autonomy contract `[V1]` (foundational)
The human engages at **two bookends**; the agent is **autonomous in between**.

1. **Before — brainstorm → spec → plan** (the *opening steps of Build mode*, §6.2). Two artifacts, two approvals: a high-level **spec** (intent: goal, approach, **required acceptance criteria**, non-goals — checked against your goals), then a detailed **plan** the agent derives by **reading the actual codebase** (a task DAG grounded in real modules, each spec criterion mapped to a task's self-gate, with risks/assumptions surfaced). This is the high-engagement bookend. Both are plain markdown (`spec.md` / `plan.md`) — the source of truth, à la superpowers (brainstorming → writing-plans).

> **Modes are isolated.** **Chat** = conversation + *manual* implementation only (you drive; no spec/plan/fleet/autonomy). **Build** = the *entire* autonomous loop, entered at its first step and stepping through brainstorm → spec → plan → execute → finalize. Entering Build is the act of consent to autonomy; Chat is the manual escape hatch. No mode mixes the two behaviors.
2. **Execute** (Build, autonomous). The fleet runs to completion: each task **self-gates** (tdd + a critic-subagent review) and **auto-retries/fixes** on failure. No per-step prompts. The user may *watch* (Build is a monitor) but is not expected to.
3. **Blocker → escalate** (the only mid-execution interrupt). On a genuine blocker the agent **pauses that task, fires a notification, and presents the decision**; independent tasks keep running; the user answers; work resumes.
4. **After — finalize** (Review). Review the finished branch + the **decisions log**, then merge / PR / send-back / discard.

**Autonomous-only.** There is **no per-step approval dial**. The single interrupt is a blocker.

**Blocker taxonomy** (escalate, don't guess): unresolvable intent ambiguity; **outward-facing/irreversible actions** (force-push to main, prod writes, deleting external data, spending money); repeated self-gate failure after N retries; a missing capability/credential/dependency; an unsafe-to-auto-resolve conflict.

**Why autonomous-only is safe (deliberate stance, not an oversight):** all execution happens in **isolated git worktrees** and nothing reaches `main` until the finalize bookend, so a bad autonomous decision is **recoverable by discarding the branch** (worst case: wasted tokens, not damaged main). The genuinely dangerous actions *escape the sandbox* and are therefore blockers by definition. Recoverability is structural, so no approval prompts are needed.

**Notifications `[V1]`:** because the user walks away, zoid must pull them back via **four channels** — (1) a **persistent in-app badge** (`⛔ N blocked` in title + status bar, stays until resolved — a missed ping is never fatal), (2) terminal bell, (3) OS notification, (4) a configurable **`notify-cmd` hook** (run any command → ntfy/Slack/phone for headless/SSH/away). Fires on **blocker · completion · budget-leash trip**.

**Blocker presentation by type:** *intent ambiguity* → pick `[1]/[2]`/describe; *outward-facing/irreversible consent* → `approve once / deny / show command`, labeled "blocker by definition — never auto-done regardless of settings"; *repeated self-gate failure* → retry-with-hint / skip / take-over / abandon; *missing capability/credential* → provide / skip. While one task is blocked, **independent tasks keep running**; the follow-stream explains *why* it escalated; answering resumes.

### 6.1 Chat mode `[V1]` — conversation + manual implementation
The default, calm surface. **Manual only**: you drive, the agent assists, edits happen one at a time. No spec, no plan, no fleet, no autonomy. Accent color **blue**.
- **Centered, readable conversation column** (bounded measure, ~80–100 cols) even on ultra-wide terminals — long lines are an ergonomics bug.
- **Light rail (§4.3)**: **context economy ⑤**, files, branch, palette.
- Tool actions render **inline** with `→ peek` affordances (each is an individual, manual action — no autonomous loop).
- **Semantic zoom ①**: `Ctrl-scroll`/keys change the *altitude* of the transcript — turns collapse to one-line summaries + activity glyphs (zoom out) or bloom to full prose/diffs (zoom in); the transition **animates** (Ⓡ2).
- **Object-first verbs ④**: select an object (error, file, diff hunk, **tree-sitter symbol**, test) → a menu of *agent verbs scoped to it*. Inverts prose-first chat for the common case.
- **Active Build suggestion:** on detecting multi-step work the agent offers "switch to Build?" (clear yes/no; never switches without consent). Manual switch via `F2`/`:build`. **The switch is continuous** — the existing conversation carries into Build's brainstorm step (no re-explaining; you just keep talking and the pipeline begins).

### 6.2 Build mode `[V1]` — the autonomous loop (a stepped pipeline)
Build *is* the superpowers 7-phase loop (§7), entered at its first step and stepping through. Accent color **amber**. The two human approvals (spec, plan) are at the front; everything after is autonomous until finalize. Each step has its own layout:

**Steps & layouts**
- **(a) Brainstorm** — chat-like; the conversation that carried in from Chat continues here, now goal-directed toward a spec (Socratic clarifying questions, approach proposal).
- **(b) Spec** *(approval)* — an inline **spec card** (goal, approach, **required acceptance criteria**, non-goals); checked against your goals. `⏎` approve.
- **(c) Worktree + baseline** — auto: create isolated branch/worktree, establish a clean test baseline.
- **(d) Plan** *(approval + pre-flight)* — agent **reads the codebase**, drafts an inline **plan card**: a task DAG grounded in real modules (bite-sized TDD tasks, deps + parallelism), each acceptance criterion mapped to a task's gate, risks/assumptions surfaced; a **pre-flight scan** checks the plan against its global constraints. `⏎` approve.
- **(e) Execute** *(autonomous monitor)* — the **2-pane + rail** working surface. **You may watch; you don't operate it.** Canonical mock `docs/ux/build-mode.html`:
  - **① Overview** (*status*) — phases, tasks with progress gauges, per-task review-pipeline status (`tdd ✓ · spec ✓ · quality ✓ · merged`), fleet sparkline; a blocker shows `⛔ needs you`.
  - **② Follow-stream** (*reasoning*) — opt-in stream at a chosen **altitude**: orchestrator by default, `f`/select-task to drill into a worker's raw stream.
  - **Rail**: **④ economy** `^E`, **B changed-files tree** `^K`, **D steering** `^G`.
  - **Keymap**: `Tab` focus · `^Z` zoom pane/drawer · `j/k` select task · `f` follow · `^E/^K/^G` drawers · `esc`/`:chat`.
  - **Responsive**: 2-pane+rail → fewer panes / tabbed single-pane (min widths: stream ≥~50, rail ≥~28).
- **(f) Final review** — a distinct **broad whole-branch review** (separate from per-task reviews); findings by severity; criticals loop back to fixes.
- **(g) Finalize** *(bookend 2)* — the finish surface. **Left:** summary + the **autonomous-decisions log** (the judgment calls made while you were away — "chose token-bucket because…", each `⏎`-navigable to its diff; the **primary trust mechanism**). **Right:** changed-files tree + diff preview (Ⓡ3). **Actions:** `[a] merge → main` · `[p] open PR` · `[c] request changes → new loop` · `[d] discard branch`. Verifies tests, then cleans up the worktree.

A finished Build collapses to a **summary card** back in Chat (re-expandable, ① zoom). The shared event log means switching Build⇄Chat never loses state.

### 6.3 Mode transitions `[V1]`
**Chat → Build** is *continuous*: the conversation carries straight into Build's brainstorm step (no re-explaining). **Build → Chat** (`esc`/`:chat`) drops to manual; Build keeps running underneath and a finished Build leaves a re-expandable **summary card** in Chat (① zoom). The shared event log means no state is lost across switches. Entering Build is the autonomy consent; returning to Chat is manual control.

### 6.4 Rendering, motion & code intelligence (Rust-enabled) `[V1: Ⓡ2–4; POST-V1: Ⓡ1]`
- **Ⓡ2 Motion `[V1]`** — GC-free immediate-mode redraw enables animated transitions: semantic-zoom fold/unfold, drawer slides, mode transitions, live fleet motion, smooth streaming caret. Governed by a **motion budget + reduced-motion setting**.
- **Ⓡ3 tree-sitter rendering `[V1]`** — real syntax highlighting, **structural folding** (collapse function bodies in diffs), code breadcrumbs, accurate **symbol selection** (④), and code-aware semantic zoom (collapse to signatures).
- **Ⓡ4 live data viz `[V1]`** — ratatui sparkline/gauge widgets: animated token meters, per-agent throughput sparklines, real-time churn — the economy ⑤ and Build fleet as a glanceable dashboard.
- **Ⓡ1 inline raster graphics `[POST-V1]`** — `ratatui-image` (Kitty/Sixel/iTerm2): churn timeline ⑤c, branch map ②, plan graphs as real images, with **ASCII fallback**. Enabled by the render-backend seam (§3) without a rewrite.

### 6.5 Command palette & command line `[V1]`
- **`^P`** — a fuzzy, **mode-aware** action launcher, grouped (mode · branch ⎇ · navigate · context ⑤ · settings · recipes[post-v1]); each row shows its **keybind** (teaches its own shortcuts); ranks by recency + match. Surfaces global verbs (switch mode, fork, settings) plus current-mode actions.
- **`:`** — a vim-style **direct command line** for power users (`:build`, `:chat`, `:fork`, `:model …`, `:q`).
- Mode switch: `:build`/`:chat` or `F2`. Canonical mock: `docs/ux/palette.html`.

### 6.6 Future modes `[POST-V1]`
New capabilities slot in as peer modes (with their own layout + rail drawers) without disturbing the v1 pair:
- **Canvas mode** ② — the branch DAG as a 2-D map; enter a node to time-travel.

---

## 7. The Workflow Engine (Build's brain)

zoid's Build mode faithfully implements obra **superpowers'** 7-phase loop `[V1: full loop]`:

1. **Brainstorm** → `spec.md` (Socratic; required acceptance criteria).
2. **Worktree + baseline** → isolated branch/worktree, clean test baseline.
3. **Plan** → `plan.md` (`docs/superpowers/plans/`): bite-sized **TDD tasks** (write-failing-test → run-fails → minimal-impl → run-passes → commit), exact file paths, global constraints, dependency DAG; a **pre-flight scan** validates the plan vs its constraints before task 1.
4. **Subagent-driven execution** → per task: a **fresh implementer** subagent with **constructed context** (never session history) implements via **TDD**, then a **two-stage review** — *spec-compliance* then *code-quality* — and a **fix subagent** loops on Critical/Important findings before the task is marked done. Independent tasks run in parallel (own worktrees). **Continuous** — no human between tasks.
5. **Final broad review** → a whole-branch review distinct from per-task reviews; criticals loop back.
6. **Finalize** → verify tests, decisions log, merge/PR/discard, worktree cleanup.

Implemented as a **staircase** (additive, not separate builds):
- **L1 — Runtime `[V1]`:** subagent executor + worktree isolation + inbox + the constructed-context assembler. Also powers async **delegates ⑧c**.
- **L2 — Scheduler `[V1]`:** turns `plan.md` into a task DAG, dispatches the per-task implementer→spec-review→quality-review→fix pipeline, runs the final broad review, drives the Build UI. Autonomous; failures auto-retry; only a blocker escalates (§6.0).
- **L3 — Interpreter `[POST-V1, data-ready in v1]`:** **recipes** as first-class authorable artifacts (declarative phase/gate sequences, like superpowers skills). The built-in 7-phase loop becomes "one recipe executing." Users author their own.

> **Critical architectural constraint for v1:** model the **phase/task/gate/review structure as data** from day one, so L3 is "add an interpreter over existing primitives," **not a rewrite.**

---

## 8. The Economy Subsystem (context + agents, in tokens)

A single subsystem underlies **⑤ context** and **⑧ agents**: both are *token-spending entities* reporting into one **TokenLedger**. Everything is denominated in **tokens** — never dollars — to stay model-agnostic (no per-model price tables to track/drift).

- **⑤a Cost-value ledger `[V1]`:** every context item shows **tokens + usage heat** (how often the model actually referenced it). "Cold" deadweight is one-keystroke evictable. Manual control: **pin / evict**. Automation: **auto-evict cold**, **compact at threshold**.
- **⑤c Churn timeline `[V1]`:** per-turn token deltas (what entered/left), flagging the #1 silent cost — files re-sent every turn — and nudging toward **pin / prompt-caching**.
- **Token governor `[V1, optional ceiling]`:** an optional per-task **token** ceiling with warn / auto-compact / pause policy. (No dollar governor; no model auto-routing in v1 — **`[POST-V1]`**.)
- **Agent leash `[V1 partial]`:** delegates report token spend into the same ledger; a global policy can pause delegates when budget is low. Full per-agent roster/autonomy UI is **`[POST-V1]`**.

- **Orchestrator context construction `[V1]`:** in Build, ⑤ is not just *the user's* view — it's the mechanism by which the **orchestrator assembles each subagent's constructed context** (plan task + relevant code, never session history; §4.4, §7). Context curation is the orchestrator's core job; the same ledger/heat machinery scores what each implementer should see.

**Headline:** the user gets **real-time, manual + automated visibility and control** over their own context window — and the same machinery curates the fleet's.

---

## 9. Providers, Tools, Safety

- **Providers `[V1: Anthropic]`:** a provider interface (streaming, tool-calling). Anthropic first; the interface keeps others addable. Use the latest Claude models by default.
- **Tools `[V1]`:** file read/write/edit, shell exec, code search, test runner. Tools run **autonomously inside the worktree sandbox** (no routine per-call approval). Tool calls/results are events.
- **Safety (autonomous-only model, §6.0):** the worktree sandbox + the finalize bookend make in-sandbox actions recoverable, so they need no prompts. **Outward-facing/irreversible actions** that escape the sandbox (force-push to main, prod/network writes, deleting external data, spending money) are **blockers** — the agent escalates rather than executing unilaterally. All actions and escalations are events (auditable, replayable) and surface in the Review decisions log.

---

## 10. Extensibility

Rust enables a stronger extensibility story than the abandoned .NET/AOT plan (which was forced out-of-process). Three layered seams:
- **Recipes/workflows `[POST-V1]`:** declarative files (yaml/markdown), interpreted (L3). No code execution required to author a workflow.
- **WASM plugins `[POST-V1]`:** tools/providers/recipes as **in-process, sandboxed, capability-secured WASM modules** via `wasmtime` (spike-validated as trivial to embed and statically linked). Language-agnostic — authors compile from any language to WASM. Near-native speed, no subprocess overhead, OS-isolated by the WASM sandbox + an explicit capability/host-function surface.
- **Out-of-process (MCP-style) `[POST-V1]`:** also supported for heavier or already-existing external tools/servers.
- **`[V1]`:** ship a fixed, curated set of providers/tools/recipes compiled in; design the trait interfaces (and the WASM host-function boundary) now so the plugin surface is an add-on, not a refactor.

---

## 11. Error Handling & Resilience

- **Streaming:** partial model output is persisted incrementally as `ModelDelta` events; a dropped connection leaves a resumable, consistent log.
- **Crash/resume:** because state = fold over an append-only log, restart replays to the last head. No separate save format.
- **Subagent/delegate failure:** isolated in its branch/worktree; failure is an event, surfaced in the board, never corrupts main.
- **Self-gate failures (Build):** a failed tdd/critic gate triggers **autonomous retry/fix**; after N retries it **escalates as a blocker** (§6.0) rather than silently proceeding or merging.
- **Blocker escalation:** pauses the affected task, fires a notification (bell + OS), and presents the decision; independent tasks keep running; the answer is an event; work resumes from the log.
- **Worktree cleanup:** worktrees are created per task and removed on completion/abandonment; orphans are reclaimable.

---

## 12. v1 Scope (the cut, collected)

**In v1:**
- Rust single static binary for linux-x64 musl (primary), win-x64, osx-arm64.
- Event-sourced core + SQLite (`rusqlite`) log + projection engine + resume.
- **Autonomy contract** (§6.0): autonomous-only execution; blocker escalation; bell + OS **notifications**.
- **Two isolated modes** (`ratatui` + `crossterm` + our thin focus/mouse/pane+rail app-framework layer) + persistent mode indicator; per-mode rails (§4.3); the **design-tokens** module (§16).
- **Chat mode (manual):** centered column, light rail (⑤ context / files / branch / palette), inline tool actions, **① semantic zoom**, **④ object-first verbs**, active Build suggestion (continuous switch).
- **⑤ context economy (a + c), tokens only:** ledger + heat + pin/evict + auto-evict/compact; churn timeline; optional token ceiling; **doubles as the orchestrator's per-subagent context constructor** (§8).
- **Build mode — full 7-phase loop (L1 + L2):** stepped pipeline brainstorm → spec ✓ → worktree/baseline → plan ✓ (read code + pre-flight) → execute → final review → finalize. Execute = subagent-driven fleet with **constructed per-task context**, **TDD**, **two-stage per-task review (spec + quality) + fix-loop**, worktree isolation, parallelism; 2-pane (Overview · Follow-stream) + rail (④ / B changed-files / D steering). Finalize = decisions log + changed-files/diff + merge/PR/request-changes/discard + worktree cleanup. Collapsed summary card back in Chat.
- **Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz.**
- Anthropic provider (streaming + tool-calling); core tool set (fs/shell/search/test), sandbox-autonomous.
- **UX fidelity** (§16): design-tokens module + `ratatui::TestBackend`+`insta` snapshot tests bound to `docs/ux/`.

**Deferred (`[POST-V1]`):**
- L3 recipe/workflow engine (but task/gate/phase modeled as data in v1 to enable it).
- ③ Parallel branch race; ② Canvas mode.
- Full ⑧ roster/autonomy UI + event-triggered automations; model auto-routing.
- Out-of-process tool/provider/recipe plugins (MCP-style); WASM plugins; additional providers.
- **Ⓡ1 inline graphics** (Sixel/Kitty/iTerm2) — render-backend seam built in v1, spectacle later.

---

## 13. Testing Strategy

- **Core (pure):** the event log + projections are pure functions → exhaustive unit + **property tests (`proptest`)** (fold determinism, branch/undo correctness, churn/ledger math). Highest-value tests; no I/O.
- **Agent loop:** test against a **fake provider** that replays scripted SSE streams + tool-call sequences — deterministic, fast, offline.
- **Tool execution:** sandboxed temp dirs; assert approvals are required and recorded.
- **Subagent/worktree runtime:** integration tests creating real temp git repos/worktrees (`git2`); verify isolation + cleanup.
- **Build scheduler:** simulate a plan DAG with fake tasks; assert dependency ordering, gate blocking, and that a failed gate halts completion.
- **TUI / UX fidelity:** `ratatui`'s `TestBackend` renders each canonical screen with fixture data; `insta` snapshots assert it matches an approved buffer **built to match the `docs/ux/` mockup** (§16). Drift becomes a reviewed diff in every PR. Focus/keymap/mouse-hit-testing tested as pure logic over synthetic events.
- **Autonomy/blocker:** assert the agent escalates the blocker taxonomy (esp. outward-facing actions) rather than executing; assert self-gate failure → retry → escalate; assert independent tasks continue while one is blocked.
- **Not snapshot-coverable:** Ⓡ2 motion (separate reduced-motion correctness tests + manual/gif review) and Ⓡ1 graphics (per-terminal visual diff).
- **TDD is the default workflow** for implementation (consistent with the methodology zoid itself embodies).

---

## 14. Open Questions / Risks

1. **App-framework floor over ratatui** — focus ring, key routing, mouse hit-testing, and the pane/drawer manager are ours to build (immediate-mode gives a buffer, not these). Spike showed it's tractable; risk is scope creep. Mitigate: build it as one small, well-tested internal module early; lean on `tui-textarea`/`tui-input` for the input box.
2. **Borrow-checker friction on shared session state** — a mutable UI tree + concurrent agents could fight the checker. Mitigate (already in §4.1): single-writer log + immutable read snapshots + channel message-passing (actor shape), not shared `&mut`.
3. **Release build time** — full-LTO release with `wasmtime` is slow (~2m in spike). Mitigate: fast dev builds, `lto = "thin"`, workspace split, CI-only full-LTO artifacts.
4. **Semantic-zoom summaries** — cheap deterministic summaries (structural: turn headline + glyphs) vs model-generated summaries (costly). v1: structural; model summaries `[POST-V1]`.
5. **Worktree-per-task cost** at higher fan-out — measure; cap concurrency.
6. **Autonomy trust & blocker recall** — autonomous-only relies on the agent *correctly* classifying blockers (esp. outward-facing actions) and on notifications actually reaching a user who walked away. Mitigate: conservative blocker taxonomy biased to escalate; the worktree+finalize recoverability backstop; redundant notification channels (bell + OS); the decisions log for after-the-fact audit. Revisit if real use shows under-escalation.
7. **Notification reliability** across terminals/OSes/SSH — bell may be suppressed, OS notifications may be unavailable headless. Mitigate: multiple channels + a persistent in-app blocked-state indicator.
8. **Naming** — confirm "zoid" as the product/binary name; mode names "Chat"/"Build"/"Review".

---

## 15. Glossary

**Signature features**
- **① Semantic zoom** — altitude control collapsing/expanding the transcript by meaning (animated, Ⓡ2).
- **③ Parallel race** *(secondary/post-v1)* — fork a prompt ×N, run side-by-side, keep winner.
- **④ Object-first verbs** — select an object (incl. tree-sitter symbols) → agent verbs scoped to it.
- **⑤ Context economy (a+c, tokens)** — ledger+heat (a) and churn timeline (c); real-time manual+auto context management.
- **⑧c Delegates** — async subagents on their own branch/worktree, reporting to an inbox; the runtime under Build.
- **Ⓡ1–4** — Rust-enabled rendering: Ⓡ1 inline graphics (post-v1), Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz.

**Concepts**
- **Autonomy contract** — engage at two bookends (spec+plan, finalize); autonomous in between; blocker-only interrupt (§6.0).
- **Modes (isolated)** — **Chat** = conversation + manual implementation; **Build** = the entire autonomous 7-phase loop as a stepped pipeline. v1 has exactly these two; finalize is Build's last step, not a separate mode.
- **Bookend** — the two human-engagement points: Build's spec+plan approvals (front) and Build's finalize step (back).
- **Spec / Plan** — the two bookend-1 artifacts: `spec.md` (intent + acceptance criteria) and `plan.md` (code-grounded task DAG); markdown source of truth.
- **Per-task review pipeline** — TDD → spec-compliance review → code-quality review → fix subagent; runs autonomously per task (replaces the earlier single "critic").
- **Final broad review** — whole-branch review distinct from per-task reviews, before finalize.
- **Constructed context** — the precisely-assembled context each subagent gets (never session history); built via ⑤ (§4.4, §8).
- **Blocker** — the sole mid-execution interrupt; escalates a decision to the human (§6.0 taxonomy).
- **Decisions log** — projection of decision-events; autonomous choices surfaced at finalize for audit/trust.
- **Rail** — the per-mode drawer host (§4.3).

---

## 16. Visual Language & UX Fidelity

The built TUI must match the canonical mockups in **`docs/ux/`** (see `docs/ux/README.md`). Fidelity is a pipeline ending in automated enforcement:

1. **Reference** — `docs/ux/*.html` (visual source of truth).
2. **Contract** — this spec (layouts in §6; min-widths/keymaps/responsive rules; drawer registry) + the **visual-language table** in `docs/ux/README.md` (glyphs, mode accent colors, status + syntax palettes).
3. **Design-tokens module** — a single Rust module holds all glyphs, colors, spacing, and layout constants; **every view renders from it** (one source of truth, so the visual language stays uniform and a token change propagates everywhere).
4. **Enforcement** — `ratatui::TestBackend` + `insta` snapshot tests per canonical screen; the first snapshot is built to match the mockup, and later drift surfaces as a reviewed diff in every PR. This is the *same self-gate machinery* as Build mode — a snapshot mismatch fails the task.
5. **Acceptance** — each TUI plan task's definition-of-done cites its `docs/ux/` mockup + snapshot test.

**Visual language (authoritative):** glyphs `● ✓ ◐ ☐ ⠿ ⎇ ⚠ ▸▾ ⛔ ▲ › ▌`; mode accents Chat=blue / Build=amber / Review=green; status ok/warn/error/branch; tree-sitter syntax palette. Full table in `docs/ux/README.md`.

**Limits:** snapshots cover structure/content/layout (①③④, panes, rail, glyphs). Ⓡ2 motion and Ⓡ1 graphics are **not** snapshot-coverable — verified separately (§13).
