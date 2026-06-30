# zoid — Cross-Platform TUI Coding Agent · Design

**Date:** 2026-06-28 · **Revised:** 2026-06-29 (phased roadmap + sequential-subagent reframe)
**Status:** Approved design (brainstorming complete) — iterating toward implementation via the phased roadmap (§12)
**Author:** strvmarv (with Claude)
**Language decision:** **Rust** — chosen after a build spike comparing Rust vs .NET 10 on the design's riskiest axes (see `spikes/RESULTS.md`).

---

## 1. Overview

**zoid** is a cross-platform, terminal-native coding agent, built from scratch in **Rust**, distributed as a single self-contained native binary (~6 MB, validated by spike). It is an open-source product, not a one-off tool.

Its thesis is that current TUI coding agents are all variations on "chat log + sidebar." zoid instead treats **the conversation as a database** (an event-sourced log) and the UI as a set of queries over it, which unlocks interaction paradigms that chat-first tools can't cheaply copy: semantic zoom, object-first actions, a live token economy, orchestrated subagents, and a native, triggerable **plan→execute→verify workflow loop**.

The whole interface is **modal** (like vim/helix) with **two isolated modes**: **Chat** (conversation + *manual* implementation — you drive) and **Build** (the *entire* autonomous loop as a stepped pipeline). Entering Build is the act of consent to autonomy; Chat is the manual escape hatch. No mode mixes the two behaviors.

It runs on an **autonomy contract** (§6.0): the human engages at two bookends — *before* (Build's brainstorm → spec → plan, approved) and *after* (Build's finalize step) — and the agent works **autonomously in between**, interrupting only on a genuine **blocker**. zoid faithfully embodies obra's **superpowers** 7-phase loop (brainstorm → worktree → plan → subagent-driven execution w/ TDD → per-task review → final review → finish); see §7.

> **Vision vs roadmap.** This document describes the full vision. Inline tags map to the development roadmap (§12): **`[V1]`** = within the **P0–P9** phases; **`[POST-V1]`** = deferred beyond them. **Nothing here is a release** — the phases are reviewable development checkpoints, refined until the vision lands. §12 has the phase-by-phase breakdown.

---

## 2. Goals & Non-Goals

### Goals
- A coding agent that feels **at home on large, high-resolution terminals** without abusing the space (protect reading ergonomics; spend extra space on parallel state, not long lines).
- **Distribution as a single native binary** per platform — no runtime to install.
- **Workflow-native**: the brainstorm→plan→execute→verify loop (à la obra's *superpowers*) is a first-class, triggerable capability, not a habit the user has to bring.
- **Context as a first-class, user-managed resource**, measured in tokens, with real-time manual and automated control.
- **Extensible by design**: the provider, tool, and workflow/recipe interfaces — and the **in-process sandboxed WASM** host boundary (the spike proved `wasmtime` links trivially in Rust) — are defined early so plugins are an add-on, not a refactor. The plugin surface itself ships post-roadmap (§12); v1 compiles in a curated set.
- **Autonomous between bookends**: once the plan is approved, the agent executes to completion without per-step engagement, escalating only on a blocker (§6.0).
- **Design fidelity**: the built TUI must match the canonical mockups in `docs/ux/`, enforced by snapshot tests (§16).
- **Maintainable for years** by a Rust contributor base; architecture chosen to keep borrow-checker friction low (message-passing over an append-only log).

### Non-Goals
- Not an IDE; no LSP-grade editing surface in v1 (we shell out to the user's editor when needed). Tree-sitter (Ⓡ3) gives read-side code intelligence — highlighting, folding, symbol selection — not live LSP diagnostics or refactors.
- Not a GUI/web app. Terminal only.
- No inline image/graphics-protocol rendering in v1 (no Sixel/Kitty) — designed-for via a render-backend seam (§3), shipped later (Ⓡ1).
- No multiplayer/collaboration in v1.
- No billing/dollar accounting — the economy is denominated in **tokens** (see §8).
- **No per-step approval dial.** zoid is autonomous-only between bookends; the sole interrupt is a blocker (§6.0). Dangerous outward-facing actions are blockers, not approval prompts.

---

## 3. Technology Decisions

| Decision | Choice (crate) | Rationale |
|---|---|---|
| Language | **Rust** (edition 2021) | Spike-validated: a true single static binary (spike ~6 MB incl. the WASM engine that v1 defers, so v1 is smaller), ~10 ms cold start; best-in-class bespoke TUI rendering substrate; in-process WASM plugins; the most stable TUI ecosystem. Coding agents are I/O-bound, so the choice is about distribution + rendering ceiling + extensibility, not raw speed. |
| Distribution | **Single static binary** per target (linux-x64 musl, win-x64, osx-arm64, …) via cargo + GitHub Actions | Genuinely one file (spike: 6.2 MB), no runtime, no native sidecars — the casual-install story. |
| TUI engine | **`ratatui` + `crossterm`** | Immediate-mode cell buffer: we render whichever projection we want each frame, which is exactly what semantic zoom ① and custom surfaces (canvas ②) want. Most mature, most stable TUI stack. |
| App-framework layer (ours) | thin layer over ratatui: **focus ring, input/key routing, mouse hit-testing, pane/drawer manager** | The known cost of immediate-mode: ratatui gives a buffer, not focus/mouse. The spike confirmed this is ~modest hand-written code; we own it as a small internal module. Consider `tui-textarea` for the multi-line input and `tui-input` where useful. |
| Async runtime | **`tokio`** | Streaming + the agent loop + sequential subagent execution; integrates with `crossterm`'s `EventStream` via `tokio::select!` (spike pattern). |
| LLM transport | **`reqwest`** + SSE (`eventsource-stream`/`reqwest-eventsource`) + **`serde`/`serde_json`** | Streaming + tool-calling; serde is the de-facto serialization layer. |
| Persistence | Append-only event log; **SQLite via `rusqlite`** | Durable, queryable; supports projections, branching, resume. `rusqlite` is synchronous, integrated via the single-writer actor (§4.1): one task owns the connection and serializes appends while readers use immutable snapshots — so blocking SQLite never blocks the async runtime. |
| Code intelligence | **`tree-sitter`** (+ grammars) & **`tree-sitter-highlight`**; diffs via **`similar`** | One parse tree drives **symbol selection** (④; spike found `fn` byte-range cleanly), structural folding, and highlighting (capture names map to the design-tokens palette §16). `syntect` only as an optional fallback for languages lacking a grammar. `similar` gives precise diffs for the diff drawer. |
| Git / worktrees | **`git2`** (libgit2) | Per-task worktree isolation for sequentially-dispatched subagents. |
| WASM host boundary | **trait/interface in v1; `wasmtime` added at the plugin phase** | Plugins are post-roadmap (§12), so v1 defines only the in-process sandboxed host boundary and compiles in a curated tool set. The spike proved `wasmtime` embeds and statically links cleanly, so deferring the crate carries no design risk — and keeps it out of the v1 build (smaller binary, faster LTO). Enables language-agnostic, capability-secured plugins later (§10). |
| Rendering backend | internal **`image-or-ASCII` trait** (ASCII v1; `ratatui-image` Kitty/Sixel/iTerm2 later) | Abstracts "draw a chart/diagram/graph" so inline raster graphics (Ⓡ1) drop in post-v1 without a rewrite. |
| Motion | internal **frame/transition engine** over ratatui | GC-free redraw enables animated transitions (Ⓡ2), gated by a motion budget + reduced-motion setting. |

**The one accepted cost (from the spike):** immediate-mode means we build the *app-framework floor* ourselves — focus ring, key routing, mouse hit-testing, pane/drawer manager. This is a bounded, well-understood internal module (the spike hand-rolled focus + mouse select without trouble), and it buys the rendering ceiling that motivated choosing Rust.

**Build-time note:** release builds with LTO are slow (spike: ~2m, much of it `wasmtime`, which v1 defers — §3). Use fast dev builds for iteration; reserve full-LTO for release artifacts; consider `lto = "thin"` and workspace splitting to keep incremental builds quick.

---

## 4. Architecture

zoid is built in layers, each understandable and testable in isolation:

```
┌─────────────────────────────────────────────────────────────┐
│ Presentation: Modes (Chat · Build) ── ratatui+crossterm         │
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
│ (orchestr. + │ (fs, shell, search;   │ (one subagent at a time;│
│  self-gates) │  autonomous; blocker- │  + worktree isolation;  │
│              │  gated, not approval) │  self-gate: tdd+critic) │
├──────────────┴───────────────────────┴────────────────────────┤
│ Providers (Anthropic, …) · streaming · tool-calling           │
└─────────────────────────────────────────────────────────────┘
```

### 4.1 Event-sourced core (the spine)
The session is an **append-only log of immutable events**. Visible state is a **pure fold (projection)** over that log. This single decision makes the novel features cheap:

- **Branching** *(post-roadmap)* = a new head pointing at an earlier event.
- **Undo / time-travel** *(post-roadmap)* = move a head backward; re-fold.
- **Multi-agent / delegates** *(post-roadmap)* = multiple heads over a shared or copied log. *(v1 uses the orchestrator + one active subagent head; concurrent heads is the deferred parallel case.)*
- **Context window** = itself a projection (an editable one — see §8).
- **Workflow** = a sub-log of task/phase/gate events; its board is a projection.

**Phasing:** the core lands in **P0** (log + fold + `Transcript` projection + resume). The head-manipulation features above — branching, undo, time-travel, concurrent heads — are *enabled* by this design but **deferred post-roadmap** (§12); v1 appends and folds a **single linear head** per session. The context-window and workflow projections are in-roadmap (P3, P6–P9).

Events are small, append-only, and serialized via `serde` (JSON, or a compact binary format like `bincode`/`postcard` if profiling warrants). The store is SQLite via `rusqlite` (log table + indices for fast projection and branch queries).

**Concurrency shape (Rust-specific):** the core owns the log behind a single writer; readers get immutable snapshots; subagents/delegates communicate via channels (`tokio::sync`) rather than shared mutable state. This actor/message-passing shape keeps borrow-checker friction low and makes the concurrent components (UI loop, agent loop, the active subagent) safe by construction.

### 4.2 Modal state machine
The top level is a **state machine over the shared event log**. A *mode* is **`(main-area layout, registered rail drawers, keymap, projection set)`**. Switching modes never copies state — it swaps the active surface. This:
- dissolves keymap collisions (each mode owns its keys),
- makes "where does X render" a non-question (it renders in its mode's main area or a rail drawer),
- gives a clean extensibility seam — **modes are zoid's one *native* extension surface** (§10); capability the ecosystem already standardizes (skills, agents, commands, MCP tools) is *adopted* rather than reinvented, while a new *interaction* surface is "a new mode, or a tool/drawer registered into a mode." A future Canvas mode or WASM plugin can register its own rail drawers without touching the core.

**Modes as an extension point `[V1: seam; POST-V1: user-authored]`.** Beyond the surface tuple, a mode carries a **policy**: what happens *between user turns* — act, or yield to the human. (Chat = the human is the clock: read → propose → you approve → one edit → stop. Build = the agent self-advances through its pipeline, yielding only at checkpoints. Everything else — rail affordances, prompt, tool allow-list, autonomy — is downstream of that one decision.) Concretely:
- Mode identity is an **open `ModeId`** (newtype, consts for built-ins), *not* a closed enum. In the event-sourced core this buys forward-compatibility: a `ModeChanged { to: ModeId }` event written under a since-removed custom mode still **replays** — an unresolved id falls back to Chat.
- A `ModeRegistry` holds `impl Mode` entries. **Chat is the floor**: the registry is constructed with it, it cannot be deregistered, and it is the fallback whenever a `ModeId` fails to resolve or a mode errors at runtime.
- **Chat and Build are both native `impl Mode`** — Build is authored as the *first* mode behind the trait, never a hardcoded branch. Proving the seam with two built-ins is what keeps it honest.
- **User-authored modes `[POST-V1]`** are *declarative* `ModePolicy` descriptors (autonomy level, optional phase sequence, tool allow-list, prompt overlay) run by one generic `DeclarativeMode` impl — no per-mode code, no plugin code path. The descriptor shape is designed now; the file loader is deferred to demand. Scripted/WASM mode *logic* is out of scope (a WASM plugin may still register tools/drawers — §10 — just not a mode's decision function).

v1 modes: **Chat · Build** (§6), both behind the `Mode` seam above. Build is a *stepped pipeline* (brainstorm → spec → plan → execute → final review → finalize); "Review/finalize" is Build's last step, not a separate mode.

### 4.3 The rail (per-mode drawers)
A reusable right-rail component hosts **stackable, collapsible drawers**; **each mode owns its own drawer set** (modes are isolated — the rail is a shared *component*, not shared *contents*). Chat: context-economy ⑤ / files / branch / palette. Build: economy ⑤ / changed-files-tree / steering. Panes are for what you watch continuously; rail drawers are for what you consult contextually.

### 4.4 Subagent runtime
A reusable executor that runs an agent turn in isolation: its own branch/head, optional **git worktree** for filesystem isolation, reporting results back as events. **Each subagent receives a precisely-constructed context — never the session history** (superpowers principle): the orchestrator assembles exactly what a task needs (its plan task + relevant code), which is why ⑤ context economy is the *orchestrator's core job*, not just a UI (§8). Per-task verification is a **review pipeline** (TDD → spec-compliance review → code-quality review → fix subagent), not a single critic; it auto-retries and escalates a **blocker** rather than prompting (§6.0, §7). **The default execution model is orchestrator + subagent:** a long-lived **orchestrator** owns the conversation and the process; discrete units of **linear work** are handed to fresh subagents **one at a time**. It's the same pattern in both modes — from **P5**, Chat delegates a discrete, non-trivial unit to a single subagent (trivial edits stay inline); Build's loop automates the same dispatch across a whole plan. **v1 runs one subagent at a time — no parallel fleet;** parallel fan-out (③) and async background delegates (⑧c) are deferred (§12). Workers are parameterized by an **`AgentProfile`** (system prompt + skill overlays + tool allow-list + model) **shaped to mirror the adopted `.claude/agents` file schema** (§10), so an existing ecosystem agent loads as a profile rather than requiring bespoke code; v1 ships one built-in profile, and the named registry crystallizes when Build's review loop needs differentiated workers (implementer→reviewer→fix, P7).

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
                         // | Approval | SelfGateResult | Decision | BlockerRaised
                         // | BlockerResolved | Merged | ...
  payload: <type-specific, serde-serialized enum variant>
  tokens: TokenStat?     // in/out/cached token counts where applicable
}
```

`Approval` records the two bookend-1 sign-offs (spec, plan) — the highest-signal events in a run, replayed into the finalize decisions log ("you approved plan v2 at 14:05").

**Schema note:** `parent` and `branch` are **intentionally retained** even though branching/undo/time-travel are deferred (§4.1, §12) — the schema encodes the full vision so post-roadmap branching is *added behavior, not a migration*. v1 always writes a single linear branch.

**Key projections (all pure functions of the log + a head):**
- `Transcript(head)` **(P0)** — ordered turns; supports **semantic zoom** ① by summarizing at variable granularity (zoom itself lands P4).
- `ContextWindow(head)` **(P3)** — current items + token cost + usage heat (⑤a).
- `ChurnTimeline(head)` **(P3)** — per-turn token deltas; flags re-sent items (⑤c).
- `TokenLedger(scope)` **(P3)** — the economy (§8).
- `WorkflowBoard(workflowId)` **(P6–P8)** — phases/tasks/self-gates/subagent + blocker state (Build overview).
- `ChangedFiles(workflowId)` **(P7)** — files×tasks tree with diff stats; files touched by more than one task are flagged `⚠` for review (Build rail drawer B).
- `DecisionsLog(workflowId)` **(P9)** — filter to `Approval`/`Decision`/`BlockerResolved` events; the audit surface in Build's finalize step.
- `BranchDAG()` *(post-roadmap)* — the graph of heads; powers undo/fork/time-travel and the Canvas mode ② (all deferred — §12).

---

## 6. Interaction Model & Modes

### 6.0 The autonomy contract `[V1]` (foundational)
The human engages at **two bookends**; the agent is **autonomous in between**.

1. **Before — brainstorm → spec → plan** (the *opening steps of Build mode*, §6.2). Two artifacts, two approvals: a high-level **spec** (intent: goal, approach, **required acceptance criteria**, non-goals — checked against your goals), then a detailed **plan** the agent derives by **reading the actual codebase** (a task DAG grounded in real modules, each spec criterion mapped to a task's self-gate, with risks/assumptions surfaced). This is the high-engagement bookend. Both are plain markdown (`spec.md` / `plan.md`) — the source of truth, à la superpowers (brainstorming → writing-plans).

> **Modes are isolated.** **Chat** = conversation + *manual* implementation only (you drive; no spec/plan/subagent-loop/autonomy). **Build** = the *entire* autonomous loop, entered at its first step and stepping through brainstorm → spec → plan → execute → finalize. Entering Build is the act of consent to autonomy; Chat is the manual escape hatch. No mode mixes the two behaviors.
2. **Execute** (Build, autonomous). The orchestrator drives the plan to completion, **dispatching one subagent per task in sequence**: each task **self-gates** (TDD + review subagents) and **auto-retries/fixes** on failure. No per-step prompts. The user may *watch* (Build is a monitor) but is not expected to.
3. **Blocker → escalate** (the only mid-execution interrupt). On a genuine blocker the agent **pauses execution, fires a notification, and presents the decision**; the user answers; work resumes from the log.
4. **After — finalize** (Build's last step). Review the finished branch + the **decisions log**, then merge / PR / send-back / discard.

**Autonomous-only.** There is **no per-step approval dial**. The single interrupt is a blocker.

**Blocker taxonomy** (escalate, don't guess): unresolvable intent ambiguity; **outward-facing/irreversible actions** (force-push to main, prod writes, deleting external data, spending money); repeated self-gate failure after N retries; a missing capability/credential/dependency; an unsafe-to-auto-resolve conflict.

**Why autonomous-only is safe (deliberate stance, not an oversight):** all execution happens in **isolated git worktrees** and nothing reaches `main` until the finalize bookend, so a bad autonomous decision is **recoverable by discarding the branch** (worst case: wasted tokens, not damaged main). The genuinely dangerous actions *escape the sandbox* and are therefore blockers by definition. Recoverability is structural, so no approval prompts are needed.

**Notifications `[V1]`:** because the user walks away, zoid must pull them back via **four channels** — (1) a **persistent in-app badge** (`⛔ N blocked` in title + status bar, stays until resolved — a missed ping is never fatal), (2) terminal bell, (3) OS notification, (4) a configurable **`notify-cmd` hook** (run any command → ntfy/Slack/phone for headless/SSH/away). Fires on **blocker · completion · budget-leash trip**.

**Blocker presentation by type:** *intent ambiguity* → pick `[1]/[2]`/describe; *outward-facing/irreversible consent* → `approve once / deny / show command`, labeled "blocker by definition — never auto-done regardless of settings"; *repeated self-gate failure* → retry-with-hint / skip / take-over / abandon; *missing capability/credential* → provide / skip. The follow-stream explains *why* it escalated; answering resumes execution.

### 6.1 Chat mode `[V1]` — conversation + human-driven implementation
The default, calm surface. **Human-driven, turn by turn** — no spec, no plan, no autonomous multi-task loop. The agent assists one unit of work at a time: trivial edits land **inline**; a discrete, non-trivial unit is **delegated to a single subagent** with a constructed context (from **P5** on — earlier phases are inline-only), its result folding back into the conversation. Accent color **blue**.
- **Centered, readable conversation column** (bounded measure, ~80–100 cols) even on ultra-wide terminals — long lines are an ergonomics bug.
- **Light rail (§4.3)**: **context economy ⑤**, files, branch, palette.
- Tool actions render **inline** with `→ peek` affordances; a delegated subagent's result folds back as a collapsible card (① zoom). Each is human-initiated — no autonomous loop.
- **Semantic zoom ①**: `Ctrl-scroll`/keys change the *altitude* of the transcript — turns collapse to one-line summaries + activity glyphs (zoom out) or bloom to full prose/diffs (zoom in); the transition **animates** (Ⓡ2).
- **Object-first verbs ④**: select an object (error, file, diff hunk, **tree-sitter symbol**, test) → a menu of *agent verbs scoped to it*. Inverts prose-first chat for the common case.
- **Active Build suggestion:** on detecting multi-step work the agent offers "switch to Build?" (clear yes/no; never switches without consent). Manual switch via **`⇧Tab`** (toggle mode) / `:build`. **The switch is continuous** — the existing conversation carries into Build's brainstorm step (no re-explaining; you just keep talking and the pipeline begins).

### 6.2 Build mode `[V1]` — the autonomous loop (a stepped pipeline)
Build *is* the superpowers 7-phase loop (§7), entered at its first step and stepping through. Accent color **amber**. The two human approvals (spec, plan) are at the front; everything after is autonomous until finalize. Each step has its own layout:

**Steps & layouts**
- **(a) Brainstorm** — chat-like; the conversation that carried in from Chat continues here, now goal-directed toward a spec (Socratic clarifying questions, approach proposal).
- **(b) Spec** *(approval)* — an inline **spec card** (goal, approach, **required acceptance criteria**, non-goals); checked against your goals. `⏎` approve.
- **(c) Worktree + baseline** — auto: create isolated branch/worktree, establish a clean test baseline.
- **(d) Plan** *(approval + pre-flight)* — agent **reads the codebase**, drafts an inline **plan card**: a task DAG grounded in real modules (bite-sized TDD tasks, dependency-ordered), each acceptance criterion mapped to a task's gate, risks/assumptions surfaced; a **pre-flight scan** checks the plan against its global constraints. `⏎` approve.
- **(e) Execute** *(autonomous monitor)* — the **2-pane + rail** working surface. **You may watch; you don't operate it.** Canonical mock `docs/ux/build-mode.html`:
  - **① Overview** (*status*) — phases, tasks with progress gauges, per-task review-pipeline status (`tdd ✓ · spec ✓ · quality ✓ · merged`), a progress sparkline (tasks completed over time); a blocker shows `⛔ needs you`.
  - **② Follow-stream** (*reasoning*) — opt-in stream at a chosen **altitude**: orchestrator by default, `f`/select-task to drill into the active subagent's raw stream.
  - **Rail**: **⑤ economy** `^E`, **B changed-files tree** `^K`, **D steering** `^G`.
  - **Keymap**: `Tab` cycle focus (forward) · **`⇧Tab` switch mode** · `^Z` zoom pane/drawer · `j/k` select task · `f` follow · `^E/^K/^G` drawers · `esc`/`:chat`.
  - **Responsive**: 2-pane+rail → fewer panes / tabbed single-pane (min widths: stream ≥~50, rail ≥~28).
- **(f) Final review** — a distinct **broad whole-branch review** (separate from per-task reviews); findings by severity; criticals loop back to fixes.
- **(g) Finalize** *(bookend 2)* — the finish surface. **Left:** summary + the **autonomous-decisions log** (the judgment calls made while you were away — "chose token-bucket because…", each `⏎`-navigable to its diff; the **primary trust mechanism**). **Right:** changed-files tree + diff preview (Ⓡ3). **Actions:** `[a] merge → main` · `[p] open PR` · `[c] request changes → new loop` · `[d] discard branch`. Verifies tests, then cleans up the worktree.

A finished Build collapses to a **summary card** back in Chat (re-expandable, ① zoom). The shared event log means switching Build⇄Chat never loses state.

### 6.3 Mode transitions `[V1]`
**Chat → Build** (`⇧Tab` / `:build`) is *continuous*: the conversation carries straight into Build's brainstorm step (no re-explaining). **Build → Chat** (`⇧Tab` / `esc` / `:chat`) drops to manual; Build keeps running underneath and a finished Build leaves a re-expandable **summary card** in Chat (① zoom). The shared event log means no state is lost across switches. Entering Build is the autonomy consent; returning to Chat is manual control.

### 6.4 Rendering, motion & code intelligence (Rust-enabled) `[V1: Ⓡ2–4; POST-V1: Ⓡ1]`
- **Ⓡ2 Motion `[V1]`** — GC-free immediate-mode redraw enables animated transitions: semantic-zoom fold/unfold, drawer slides, mode transitions, live subagent/progress motion, smooth streaming caret. Governed by a **motion budget + reduced-motion setting**.
- **Ⓡ3 tree-sitter rendering `[V1]`** — real syntax highlighting, **structural folding** (collapse function bodies in diffs), code breadcrumbs, accurate **symbol selection** (④), and code-aware semantic zoom (collapse to signatures).
- **Ⓡ4 live data viz `[V1]`** — ratatui sparkline/gauge widgets: animated token meters, per-agent throughput sparklines, real-time churn — the economy ⑤ and Build progress as a glanceable dashboard.
- **Ⓡ1 inline raster graphics `[POST-V1]`** — `ratatui-image` (Kitty/Sixel/iTerm2): churn timeline ⑤c, branch map ②, plan graphs as real images, with **ASCII fallback**. Enabled by the render-backend seam (§3) without a rewrite.

### 6.5 Command palette & command line `[V1]`
- **`^P`** — a fuzzy, **mode-aware** action launcher, grouped (mode · navigate · context ⑤ · settings · branch ⎇[post-v1] · recipes[post-v1]); each row shows its **keybind** (teaches its own shortcuts); ranks by recency + match. Surfaces global verbs (switch mode, settings) plus current-mode actions.
- **`:`** — a vim-style **direct command line** for power users (`:build`, `:chat`, `:fork`, `:model …`, `:q`).
- Mode switch: **`⇧Tab`** (toggle Chat⇄Build) · `:build`/`:chat`. Canonical mock: `docs/ux/palette.html`.

### 6.6 Future modes `[POST-V1]`
New capabilities slot in as peer modes (with their own layout + rail drawers) behind the `Mode` seam (§4.2), without disturbing the v1 pair:
- **Canvas mode** ② — the branch DAG as a 2-D map; enter a node to time-travel.
- **User-authored modes** — declarative `ModePolicy` descriptors (no code) run by the generic `DeclarativeMode` impl; Chat stays the non-removable fallback. The descriptor shape is fixed in v1 (§4.2); only the loader is deferred.

---

## 7. The Workflow Engine (Build's brain)

zoid's Build mode faithfully implements obra **superpowers'** 7-phase loop `[V1: full loop]`:

1. **Brainstorm** → `spec.md` (Socratic; required acceptance criteria).
2. **Worktree + baseline** → isolated branch/worktree, clean test baseline.
3. **Plan** → `plan.md` (`docs/superpowers/plans/`): bite-sized **TDD tasks**, exact file paths, global constraints, dependency DAG; a **pre-flight scan** validates the plan vs its constraints before task 1.
4. **Subagent-driven execution** → per task: a **fresh implementer** subagent with **constructed context** (never session history) implements the task, then a **two-stage review** — *spec-compliance* then *code-quality* — and a **fix subagent** loops on Critical/Important findings before the task is marked done. The orchestrator runs tasks **sequentially — one subagent at a time**, each in its own worktree (parallel fan-out deferred, §12). **Continuous** — no human between tasks.
5. **TDD** → the inner loop of every task: write-failing-test → run-fails → minimal-impl → run-passes → commit.
6. **Final broad review** → a whole-branch review distinct from per-task reviews; criticals loop back.
7. **Finalize** → verify tests, decisions log, merge/PR/discard, worktree cleanup.

Implemented as a **staircase** (additive, not separate builds):
- **L1 — Runtime (P5):** the orchestrator + a **sequential** subagent executor + worktree isolation + the constructed-context assembler. (Async background **delegates ⑧c** reuse this runtime but are deferred — §12.)
- **L2 — Scheduler (P6–P9):** turns `plan.md` into a task DAG and walks it **in dependency order, one task at a time**, dispatching the per-task implementer→spec-review→quality-review→fix pipeline, escalating blockers, then running the final broad review and driving the Build UI through finalize. Autonomous; failures auto-retry; only a blocker escalates (§6.0).
- **L3 — Interpreter (post-roadmap; data-ready from P6):** **recipes** as first-class authorable artifacts (declarative phase/gate sequences, like superpowers skills). The built-in 7-phase loop becomes "one recipe executing." Users author their own.

> **Critical architectural constraint for v1:** model the **phase/task/gate/review structure as data** from day one, so L3 is "add an interpreter over existing primitives," **not a rewrite.**

---

## 8. The Economy Subsystem (context + agents, in tokens)

A single subsystem underlies **⑤ context** and **⑧ agents**: both are *token-spending entities* reporting into one **TokenLedger**. Everything is denominated in **tokens** — never dollars — to stay model-agnostic (no per-model price tables to track/drift).

- **⑤a Cost-value ledger `[V1]`:** every context item shows **tokens + usage heat**. Heat is a **heuristic** — true per-item attention isn't exposed by provider APIs — derived from observable signals: references in the model's output, tool calls that target the item (re-read / edit), and prompt-cache accounting (cache-read vs creation tokens). "Cold" deadweight (low heat, high tokens) is one-keystroke evictable. Manual control: **pin / evict**. Automation: **auto-evict cold**, **compact at threshold** (never auto-evicts pinned items).
- **⑤c Churn timeline `[V1]`:** per-turn token deltas (what entered/left), flagging the #1 silent cost — files re-sent every turn — and nudging toward **pin / prompt-caching**.
- **Token governor `[V1, optional ceiling]`:** an optional per-task **token** ceiling with warn / auto-compact / pause policy. (No dollar governor; no model auto-routing in v1 — **`[POST-V1]`**.)
- **Subagent leash `[V1]`:** each dispatched subagent reports its token spend into the same ledger; a budget policy can pause before dispatching the next subagent when budget is low. (Full multi-agent roster/autonomy UI follows parallelism — **`[POST-V1]`**, §12.)

- **Orchestrator context construction `[V1]`:** in Build, ⑤ is not just *the user's* view — it's the mechanism by which the **orchestrator assembles each subagent's constructed context** (plan task + relevant code, never session history; §4.4, §7). Context curation is the orchestrator's core job; the same ledger/heat machinery scores what each implementer should see.

**Headline:** the user gets **real-time, manual + automated visibility and control** over their own context window — and the same machinery curates each subagent's.

---

## 9. Providers, Tools, Safety

- **Providers `[V1: Anthropic]`:** a provider interface (streaming, tool-calling). Anthropic first; the interface keeps others addable. Use the latest Claude models by default.
- **Tools `[V1]`:** file read/write/edit, shell exec, code search, test runner. **Two execution contexts:** in **Chat**, tools run in the **working directory** — human-driven and visible; in **Build**, tools run **autonomously inside per-task worktree sandboxes** (no routine per-call approval). Tool calls/results are events in both.
- **Safety — two mechanisms for two modes (§6.0):**
  - **Chat** is safe by **human-in-the-loop**: you see every action as it happens and drive each turn, so cwd execution needs no sandbox and no blocker machinery — anything outward-facing is simply something you're present to approve.
  - **Build** is safe by **isolation + the finalize bookend**: work happens in worktrees and nothing reaches `main` until you finalize, so in-sandbox actions are recoverable and need no prompts. **Outward-facing/irreversible actions** that escape the sandbox (force-push to main, prod/network writes, deleting external data, spending money) are **blockers** — escalated, not executed unilaterally.
  - All actions and escalations are events (auditable, replayable) and surface in the finalize decisions log.
  - **Permission review `[POST-V1]`:** the v1 safety story above is *deterministic* (human-in-the-loop in Chat; isolation + blockers in Build) — no model reviews actions. A future **tier 1.5** local semantic risk gate (reuse the in-process embedder/reranker to flag paraphrased/obfuscated dangers a rule-matcher misses), and a further tier 2 generative judge, are designed-for behind a `PermissionReviewer` seam but **deferred** (§12). Lower priority than the roadmap phases.

---

## 10. Extensibility

**Principle — adopt, don't invent.** zoid is a *host* for the agentic primitives the ecosystem has already standardized as files; it invents only the one thing the ecosystem has *not* standardized — the modal TUI surface (§4.2). Everything else is parsed, not reimagined. Adoption is **provider-neutral**: skills/agents/commands are markdown + frontmatter (prompt assembly, not an API) and MCP is a model-agnostic protocol, so the whole surface works against the Ollama/GLM stack (§9), not just Anthropic. **Target the Claude-style conventions** (the most widely authored, provider-neutral dialect). Layered seams, in priority order:

- **Adopted ecosystem entities `[V1: shapes; POST-V1: loaders]`:**
  - **Skills** — `SKILL.md`-style instruction files (frontmatter name/description + body); injected as prompt overlays, composed into agents and into a mode's phases.
  - **Agents (subagents)** — `.claude/agents/*.md`-style profiles (name, description, tools, model + system-prompt body); loaded into the **subagent runtime** (§4.4) as an `AgentProfile`. This *is* the "grouping of skills" container — and because it already exists as a file format, zoid reads it rather than defining a new one.
  - **Prompts / commands** — `.claude/commands/*.md`-style parameterized templates, surfaced through the command palette `^P` / `:` line (§4.2).

  The internal structs are **shaped to mirror these formats now** (so `AgentProfile` ≈ the agent file schema); the file **loaders are built on demand** (first real need: Build's differentiated workers, P7), not up front.
- **Modes `[V1: seam; POST-V1: user-authored]`:** zoid's **one native** extension concept — the ecosystem has no "mode" primitive. Built-ins (Chat, Build) ship as native `impl Mode` behind a `ModeRegistry`; user-authored modes arrive as declarative `ModePolicy` descriptors run by one generic interpreter — no code execution. Chat is the non-removable fallback. A mode is a *native orchestration populated by the adopted entities above*: it dispatches agents (files) that reference skills (files); scripted/WASM mode *logic* stays out of scope.
- **MCP — the plugin protocol for external tools `[POST-V1]`:** external/already-existing tools and servers arrive over **MCP** (model-agnostic, the de-facto plugin standard). This is the primary answer to "use other tools as plugins."
- **WASM plugins `[POST-V1 — deferred, not forgotten]`:** in-process, sandboxed, capability-secured `wasmtime` modules for the rarer case of a *native* in-process tool/provider wanting near-native speed and OS-level isolation without a subprocess. The spike validated `wasmtime` embeds and statically links cleanly, so the host-function boundary is designed now and the crate is added at the plugin phase — kept out of the v1 build (smaller binary, faster LTO, §3). **Lower priority than MCP**, but explicitly retained.
- **`[V1]`:** ship a fixed, curated set of providers/tools compiled in; design the trait interfaces (and the MCP + WASM host boundaries) now so the plugin surface is an add-on, not a refactor.

### 10.1 Configuration `[V1: minimal — formalizes what exists; POST-V1: full surface]`

v1 codifies only the configuration that **already exists in code** plus a precedence model; the broader surface is enumerated as deferred decisions (below) so nothing is designed by accident.

- **Two namespaces, one principle (§10's "adopt, don't invent").** *zoid-native* config lives in zoid's own namespace; *adopted ecosystem entities* are read from their conventional Claude-style locations.
  - **zoid-native:** `~/.config/zoid/config.toml` (user global) and `./.zoid/config.toml` (project) — TOML (Rust-idiomatic). The session DB already lives at `./.zoid/session.db`.
  - **adopted entities `[POST-V1 loaders]`:** `.claude/agents/*.md`, skills, `.claude/commands/*.md`, and MCP server definitions (`.mcp.json`-style) — read from the ecosystem's locations (§10), not redefined.
- **Precedence (low → high):** compiled defaults → user global → project `./.zoid/config.toml` → local gitignored `./.zoid/config.local.toml` → `ZOID_*` environment → CLI flags.
- **Current knobs (the whole v1 surface):** `OLLAMA_API_KEY` / `ANTHROPIC_API_KEY` (provider select **+ secret**), `ZOID_MODEL` (model), `ZOID_DB` (session DB path), `ZOID_REDUCED_MOTION` (motion §6.4). Provider `base_url` is currently hardcoded per provider.
- **Secrets rule:** API keys are **never** read from committed config — environment, the gitignored `config.local.toml`, or (later) an OS keyring only.
- **Surfacing:** settings appear **read-only** in the `^P` palette's *settings* group (§6.5); editing is file-first — **no in-TUI settings editor in v1**.

**`[POST-V1]` — configuration decisions to review & decide before they're built** (deferred, but the scope is recorded so it isn't designed piecemeal):
- **Format finality:** TOML for zoid-native vs JSON for `settings.json`-parity with the adopted dialect; whether to read a Claude-style `settings.json` directly.
- **Secrets handling:** OS keyring integration (cf. the Billy pattern) vs env-only; per-provider credential resolution; redaction in logs/events.
- **Entity-discovery paths:** exact search order and precedence for adopted `.claude/`-style agents/skills/commands across user vs project scope; enable/disable lists; project-trust (don't auto-run agents/MCP from an untrusted cloned repo).
- **Provider/model config:** `base_url` override (local Ollama vs Cloud), per-mode or per-agent model selection, model auto-routing, request parameters (temperature, max-tokens).
- **Per-subsystem policy as config:** economy ⑤ token ceilings + auto-evict/compact thresholds (§8); permission rules + path scopes (the deferred tier-1/1.5 gate, §9/§12); `notify-cmd` + notification channel toggles (§6.0).
- **User-authored modes:** the `ModePolicy` descriptor load path + discovery (§4.2, §6.6).
- **Validation & UX:** schema validation + actionable errors on malformed config; config hot-reload vs restart; an in-TUI settings editor; a `zoid config` CLI; first-run/onboarding to capture provider + key.

---

## 11. Error Handling & Resilience

- **Streaming:** partial model output is persisted incrementally as `ModelDelta` events; a dropped connection leaves a resumable, consistent log.
- **Crash/resume:** because state = fold over an append-only log, restart replays to the last head. No separate save format.
- **Subagent failure:** isolated in its worktree; failure is an event, surfaced in the board, never corrupts main.
- **Self-gate failures (Build):** a failed tdd/critic gate triggers **autonomous retry/fix**; after N retries it **escalates as a blocker** (§6.0) rather than silently proceeding or merging.
- **Blocker escalation:** pauses execution, fires a notification (bell + OS), and presents the decision; the answer is an event; work resumes from the log.
- **Worktree cleanup:** worktrees are created per task and removed on completion/abandonment; orphans are reclaimable.

---

## 12. Development Roadmap (phased)

The build proceeds as a sequence of **vertical slices**. Each phase (a) compiles and runs, (b) is reviewable on its own, and (c) can reshape the phases after it. This is a **development sequence, not a release plan** — **nothing ships** until the full vision is realized; every phase is a checkpoint the author reviews and refines.

**Ordering rationale (full-Chat-first):** Chat is realized completely before the Build loop, because (1) the economy ⑤ (P3) is the very substrate Build's per-subagent context-construction needs (§4.4, §8), and (2) the signature Chat interactions (P4) de-risk the rendering/motion/tree-sitter stack on a calm surface *before* it has to also serve the autonomous loop. P5 introduces the orchestrator + **sequential** subagent runtime (one subagent at a time — no fleet), which P6–P9 then automate into the full superpowers loop.

| Phase | Lands | Runnable end-state |
|---|---|---|
| **P0 · Spine & skeleton** | Cargo workspace; **design-tokens** module; event log (`rusqlite` append-only) + fold engine + `Transcript` projection; **fake provider**; bare `ratatui` shell (boot/render/quit); the `proptest` (core) + `TestBackend`/`insta` (shell) harness | `zoid` boots to an empty Chat frame, persists & replays a session. Event-sourced architecture **and** the fidelity pipeline proven end-to-end on trivial data. |
| **P1 · Chat MVP** | Anthropic provider (SSE streaming + tool-calling); the agent loop; core tools (fs read/write/edit, shell, code search) operating in cwd; inline tool rendering with `→ peek`; real multi-line input (`tui-textarea`) | A usable **manual chat coding agent**. |
| **P2 · Modal shell** | App-framework floor (focus ring, key routing, mouse hit-testing, pane/drawer manager); the rail component; command palette `^P` + command line `:`; files & branch drawers; persistent mode indicator | Chat as a real modal TUI driven by keyboard + mouse. |
| **P3 · Context economy ⑤** | `TokenLedger` + `ContextWindow`(heat) + `ChurnTimeline` projections; pin/evict; auto-evict-cold + compact-at-threshold; optional token ceiling; economy rail drawer with **Ⓡ4** dataviz (gauges/sparklines); the **constructed-context assembler** primitive | Live manual + automated context control — and the context-construction substrate P5 reuses. |
| **P4 · Signature Chat** | **Ⓡ2** motion engine (motion budget + reduced-motion); **① semantic zoom** (structural summaries); **Ⓡ3** tree-sitter (highlight, structural fold, symbol selection); **④ object-first verbs** | The "beyond chat-log" Chat experience. |
| **P5 · Orchestrator + subagent runtime (L1)** | Subagent executor; `git2` **worktree isolation**; the constructed-context assembler (from P3) wired to subagent dispatch; **one subagent at a time**; available from Chat (delegate a discrete task) | Dispatch an isolated subagent for a unit of work and fold its result back. |
| **P6 · Build front-half (L2a)** | **Mode seam first** (open `ModeId` + `trait Mode` + `ModeRegistry`; refactor Chat behind it; §4.2) — *then* Build as the **first** `impl Mode`; Build mode as a stepped-pipeline shell; **continuous Chat→Build** switch; brainstorm → spec card ✓ → worktree+baseline → plan card ✓ (read-code + pre-flight); **phase/task/gate modeled as data** | Drive Build to an approved plan (execution stubbed / dry-run). |
| **P7 · Build execution — happy path (L2b)** | Scheduler walks the task DAG **in dependency order, one task at a time**; per-task implementer→spec-review→quality-review→fix loop w/ **TDD**; self-gates + auto-retry; the 2-pane (Overview · Follow-stream) + rail execute surface | A plan with no blockers executes end-to-end autonomously; you watch it. |
| **P8 · Blockers & notifications (L2c)** | Blocker detection/classification/escalation (esp. outward-facing actions), pause→resume; the 4 notification channels + persistent badge | A plan that hits ambiguity or an outward-facing action pauses, notifies, and resumes on your answer. |
| **P9 · Finalize bookend** | Final broad whole-branch review; finalize surface (decisions log + changed-files/diff + merge / PR / request-changes / discard) + worktree cleanup + tests verify; collapsed summary card back in Chat | The **full autonomy contract**, brainstorm → finalize. |

**Cross-cutting (every phase):** TDD is the default workflow; every new screen ships its design-tokens + a `TestBackend`/`insta` snapshot bound to `docs/ux/` (§16); each phase is a reviewable checkpoint, and what it teaches is allowed to revise the phases after it.

**Deferred beyond the roadmap (`[POST-V1]` — the vision's later spectacle):**
- **L3** recipe/workflow interpreter (data-ready from P6 — phase/task/gate already modeled as data).
- **Parallelism:** parallel subagent fan-out · ③ parallel branch race · cross-worktree conflict radar · per-agent roster/leash UI.
- **Async background delegates ⑧c** (inbox) — reuse the P5 runtime.
- **② Canvas mode** (branch-DAG map) — with **undo** (move head), **fork** (new head), and **time-travel** to an arbitrary turn (all event-log-native, surfaced here).
- **Ⓡ1 inline raster graphics** (Sixel/Kitty/iTerm2) — render-backend seam built earlier, spectacle later.
- **Extensibility loaders (§10):** Claude-style **skill / agent / command** file loaders · **MCP** external-tool plugins (the primary plugin path) · **WASM** in-process plugins (deferred, not forgotten) · additional providers · model auto-routing.
- **Permission tier 1.5 — local semantic risk scoring** (§9): an LLM-reviewed permission gate that reuses the in-process local embedder/reranker (e.g. bge-small) to score a *proposed* tool call against danger-exemplars + natural-language deny-policies, escalating Allow→Ask/Deny on paraphrased/obfuscated dangers that static rules (tier 1) miss. Designed-for behind a `PermissionReviewer` seam; **no generative model, no provider tokens**. Verdicts are recorded as events so deterministic replay reads the verdict, never re-runs the model. A generative judge (small local instruct model *or* the configured remote provider, tier 2) is a further deferral.

---

## 13. Testing Strategy

- **Core (pure):** the event log + projections are pure functions → exhaustive unit + **property tests (`proptest`)** (fold determinism, branch/undo correctness, churn/ledger math). Highest-value tests; no I/O.
- **Agent loop:** test against a **fake provider** that replays scripted SSE streams + tool-call sequences — deterministic, fast, offline.
- **Tool execution:** sandboxed temp dirs; assert tool calls/results are recorded as events, and that **outward-facing actions escalate (don't execute)**.
- **Subagent/worktree runtime:** integration tests creating real temp git repos/worktrees (`git2`); verify isolation + cleanup.
- **Build scheduler:** simulate a plan DAG with fake tasks; assert **sequential** dependency ordering (topological, one task at a time), gate blocking, and that a failed gate halts completion.
- **TUI / UX fidelity:** `ratatui`'s `TestBackend` renders each canonical screen with fixture data; `insta` snapshots assert it matches an approved buffer **built to match the `docs/ux/` mockup** (§16). Drift becomes a reviewed diff in every PR. Focus/keymap/mouse-hit-testing tested as pure logic over synthetic events.
- **Autonomy/blocker:** assert the agent escalates the blocker taxonomy (esp. outward-facing actions) rather than executing; assert self-gate failure → retry → escalate; assert a blocker pauses execution and that answering resumes from the log.
- **Not snapshot-coverable:** Ⓡ2 motion (separate reduced-motion correctness tests + manual/gif review) and Ⓡ1 graphics (per-terminal visual diff).
- **TDD is the default workflow** for implementation (consistent with the methodology zoid itself embodies).

---

## 14. Open Questions / Risks

1. **App-framework floor over ratatui** — focus ring, key routing, mouse hit-testing, and the pane/drawer manager are ours to build (immediate-mode gives a buffer, not these). Spike showed it's tractable; risk is scope creep. Mitigate: build it as one small, well-tested internal module early; lean on `tui-textarea`/`tui-input` for the input box.
2. **Borrow-checker friction on shared session state** — a mutable UI tree + concurrent agents could fight the checker. Mitigate (already in §4.1): single-writer log + immutable read snapshots + channel message-passing (actor shape), not shared `&mut`.
3. **Release build time** — full-LTO release builds are slow (~2m in spike, much of it `wasmtime`). Since `wasmtime` is deferred out of the v1 build (§3), v1 LTO is lighter; further mitigate with fast dev builds, `lto = "thin"`, workspace split, and CI-only full-LTO artifacts. Re-measure when the plugin phase reintroduces `wasmtime`.
4. **Semantic-zoom summaries** — cheap deterministic summaries (structural: turn headline + glyphs) vs model-generated summaries (costly). v1: structural; model summaries `[POST-V1]`.
5. **Sequential-execution latency & worktree churn** — running one subagent at a time trades throughput for simplicity and safety; a large plan takes longer wall-clock, and each task creates/tears down a worktree. Acceptable for v1 (the user has walked away). Parallel fan-out is a deferred optimization (§12); the phase/task model is built so it slots in without a rewrite.
6. **Autonomy trust & blocker recall** — autonomous-only relies on the agent *correctly* classifying blockers (esp. outward-facing actions) and on notifications actually reaching a user who walked away. Mitigate: conservative blocker taxonomy biased to escalate; the worktree+finalize recoverability backstop; redundant notification channels (bell + OS); the decisions log for after-the-fact audit. Revisit if real use shows under-escalation.
7. **Notification reliability** across terminals/OSes/SSH — bell may be suppressed, OS notifications may be unavailable headless. Mitigate: multiple channels + a persistent in-app blocked-state indicator.
8. **Naming** — confirm "zoid" as the product/binary name; mode names "Chat"/"Build" (finalize is Build's last step, not a separate mode).
9. **Context-heat fidelity (⑤a)** — "usage heat" is a heuristic (output references, tool targeting, prompt-cache accounting), not true attention (unavailable via API). A weak heuristic makes "cold" misleading and risks bad auto-evictions. Mitigate: conservative auto-evict (suggest, or only evict clearly-cold + unpinned), pin always overrides, tune the heuristic against real sessions; never auto-evict pinned items.
10. **Fake-provider drift** — the agent-loop tests replay scripted SSE against a fake provider (§13), which can drift from real Anthropic behavior (new event types, tool-call framing, error shapes), leaving tests green while the live client breaks. Mitigate: periodic contract tests against the live API; version the fake against a captured real transcript.
11. **Configuration surface** — only the *minimal* surface is specified (§10.1: existing `ZOID_*`/key env vars + precedence + the `.zoid/`-vs-`.claude/` split). The **full surface is unresolved** and listed in §10.1's `[POST-V1]` block: format finality (TOML vs `settings.json`-parity), secrets/keyring, entity-discovery paths + project-trust, per-mode/agent provider+model config, policy-as-config (economy ceilings, permission rules, notifications), `ModePolicy` load path, validation/hot-reload/in-TUI editor/onboarding. Decide each before building it — don't let config accrete piecemeal.

---

## 15. Glossary

**Signature features**
- **① Semantic zoom** — altitude control collapsing/expanding the transcript by meaning (animated, Ⓡ2).
- **③ Parallel race** *(post-roadmap)* — fork a prompt ×N, run side-by-side, keep winner.
- **④ Object-first verbs** — select an object (incl. tree-sitter symbols) → agent verbs scoped to it.
- **⑤ Context economy (a+c, tokens)** — ledger+heat (a) and churn timeline (c); real-time manual+auto context management.
- **⑧c Delegates** *(post-roadmap)* — async background subagents reporting to an inbox; reuse the orchestrator/subagent runtime.
- **Ⓡ1–4** — Rust-enabled rendering: Ⓡ1 inline graphics (post-v1), Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz.

**Concepts**
- **Autonomy contract** — engage at two bookends (spec+plan, finalize); autonomous in between; blocker-only interrupt (§6.0).
- **Modes (isolated)** — **Chat** = conversation + human-driven implementation (delegates a discrete unit to a single subagent from P5; trivial edits inline); **Build** = the entire autonomous 7-phase loop as a stepped pipeline. v1 has exactly these two; finalize is Build's last step, not a separate mode.
- **Bookend** — the two human-engagement points: Build's spec+plan approvals (front) and Build's finalize step (back).
- **Spec / Plan** — the two bookend-1 artifacts: `spec.md` (intent + acceptance criteria) and `plan.md` (code-grounded task DAG); markdown source of truth.
- **Per-task review pipeline** — TDD → spec-compliance review → code-quality review → fix subagent; runs autonomously per task (replaces the earlier single "critic").
- **Final broad review** — whole-branch review distinct from per-task reviews, before finalize.
- **Constructed context** — the precisely-assembled context each subagent gets (never session history); built via ⑤ (§4.4, §8).
- **Orchestrator** — the long-lived agent that owns the conversation and the process; dispatches subagents and manages gates/blockers.
- **Subagent** — a fresh agent given a constructed context to do one unit of **linear work** in its own worktree; **one at a time in v1** (no fleet).
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

**Visual language (authoritative):** glyphs `● ✓ ◐ ☐ ⠿ ⎇ ⚠ ▸▾ ⛔ ▲ › ▌`; mode accents Chat=blue / Build=amber (finalize uses a green accent within Build); status ok/warn/error/branch; tree-sitter syntax palette. Full table in `docs/ux/README.md`.

**Limits:** snapshots cover structure/content/layout (①③④, panes, rail, glyphs). Ⓡ2 motion and Ⓡ1 graphics are **not** snapshot-coverable — verified separately (§13).
