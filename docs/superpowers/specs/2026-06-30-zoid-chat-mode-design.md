# zoid — Chat Mode · Design

**Date:** 2026-06-30 (extracted from the 2026-06-28 combined design; refined 2026-06-30)
**Status:** Approved design — the v1 near-term surface, layered on the shared core architecture.
**Author:** strvmarv (with Claude)
**Language decision:** **Rust** (edition 2021) — see the core doc.

> **Spec set.** This is one of three documents. It assumes the **core-architecture** doc and covers **only Chat** — the mode-specific surface, interactions, economy detail, session management, and single-subagent delegation. Everything cross-cutting (event spine, mode seam, subagent *runtime*, data model, extensibility, testing harness, visual language) lives in the core doc and is **referenced, not restated**.
> - **Core architecture** → `2026-06-30-zoid-core-architecture.md` (the shared substrate — read it first).
> - **Chat mode** (this doc) → `2026-06-30-zoid-chat-mode-design.md` — conversation + manual implementation + single-subagent delegation (the v1 near-term work).
> - **Build mode** → `2026-06-30-zoid-build-mode-design.md` (**deferred**: the autonomous 7-phase loop that *automates* Chat's shared subagent runtime across a whole plan).
>
> Where this doc says "see core §X" it means the core-architecture doc. Nothing here is a release — see the core roadmap (§9 there) and Chat's slice of it (§9 here).
>
> **Implementation state.** Much of Chat (P0–P4) is already built; where this refinement records a target that differs from what currently ships, it is marked *(current: …)* so the spec is both intent **and** ground truth.

---

## 1. Overview

**Chat is the default, calm surface** and the **non-removable mode floor**: the `ModeRegistry` is constructed with it, it cannot be deregistered, and it is the fallback whenever a `ModeId` fails to resolve or a mode errors at runtime (see core §4.2). Everything else in zoid is reached from, or falls back to, Chat.

Chat is **human-driven, turn by turn** — no spec, no plan, no autonomous multi-task loop. You read → propose → approve → one unit of work happens → stop. The agent assists **one unit of work at a time**: trivial edits land **inline**; a discrete, non-trivial unit is **delegated to a single subagent** (from **P5** on — earlier phases are inline-only), whose result folds back into the conversation. Accent color **blue** (see core §13 for the visual language and the design-tokens rule).

Chat rides the shared spine without re-explaining it: the transcript, context window, and churn timeline are all **projections over the event log** (core §4.1, §5); a **session** is one bounded, resumable partition of that log (§7 here); switching to Build never copies state, it swaps the surface (core §4.2). This doc describes what Chat *does* on top of that substrate.

---

## 2. Chat mode interaction model `[V1]`

The default, calm surface. Human-driven, turn by turn — no spec, no plan, no autonomous multi-task loop. Canonical mock: `docs/ux/chat-mode.html`.

- **Centered, readable conversation column** — a bounded measure (**~80–100 cols**) even on ultra-wide terminals. Long lines are an ergonomics bug; extra horizontal space is spent on the rail and parallel state, never on stretching prose.
- **Light rail (core §4.3)** — Chat's own drawer set, detailed in §2.1. Panes are for what you watch continuously; rail drawers are for what you consult contextually. *(The palette is a `^P`/`:` overlay launcher, §4 here — **not** a rail drawer.)*
- **Inline tool actions** — file read/write/edit, shell exec, code search, and the test runner render **inline** in the conversation with `→ peek` affordances to expand the full call/result. Each is **human-initiated** — there is no autonomous loop; you see and drive every action.
- **Delegated result cards** — a delegated subagent's result folds back into the conversation as a collapsible card (① zoom), inline with the turn that requested it (§6 here).
- **Timestamped rows** — each user/assistant message row is prefixed with a **dim 24-hour `HH:MM`** (local) timestamp; tool-result and wrapped continuation sub-rows stay bare. *(Already implemented; a folded assistant turn takes its first contributing event's time.)*
- **Active Build suggestion `[deferred with Build]`** — when Build ships, Chat will detect multi-step work and offer "switch to Build?" (clear yes/no; **never** switches without consent), the switch being a mode-seam concern (core §4.2) and the carry-over owned by the Build spec. **Build is deferred**, so this affordance and the `⇧Tab → Build` hint are **not surfaced in the Chat UI for now** (§2.2).

### 2.1 Rail drawers

Chat's rail hosts, **top to bottom** (the rail component itself is core §4.3):

| Drawer | Default | Contents |
|---|---|---|
| **repo** | expanded | **repo name · branch · worktree**, plus a **working-tree changes** line — green `+added` / red `-removed` line counts + changed-file count, reflecting the repo's current diff state. The git identity, at the **top** of the rail. The **only** place branch is shown (§2.2). |
| **session** | expanded | current session **name · model/provider · duration · token usage · context size · cwd** (§7). The at-a-glance "where am I / which model / what has this cost" widget. The **cwd** (working directory) is **truncated to fit, never wrapped** — paths can be long. |
| **context ⑤** | expanded | the economy drawer — ledger + heat + churn (§5), with Ⓡ4 dataviz (§3.4). |

- **No shortcut keys.** Rail drawers carry **no keybind labels and no keybind glyphs** in the UI for now — the drawer header shows only its title (and expand/collapse chevron). *(current: drawers render a `keybind` field — `^5`/`^F`/`^B`, `state.rs:111–115`, `render.rs:148–151` — to be removed.)* Drawers are still toggfeatle via the palette (§4); dedicated keybinds may return later.
- Order/expansion above is easily reordered; it reflects "identity + cost, top to bottom." There is **no files drawer** in the rail — file browsing lives in the palette (**Open file…**, §4).

### 2.2 Chrome: mode indicator, repo & status bar

zoid currently renders the mode name, branch, and hints in several places; this consolidates them to **one home each** (removing duplicates observed in the running TUI).

- **Mode indicator — bottom-left only.** The accent-colored mode chip (` CHAT `) lives in the **bottom status bar** and nowhere else. *(current: also rendered top-left in the title bar, `render.rs:82,87` — remove the top-left one.)*
- **Branch/repo — rail only.** Branch appears **only** in the repo drawer (§2.1). *(current: rendered in four places — top title bar `render.rs:88`, bottom status bar `render.rs:119`, the branch drawer, and the legacy test-only renderer — keep only the rail; remove from the top bar and the status bar.)*
- **Top title bar (minimal)** — shows the **app name** and a **live activity indicator** (`● idle` when waiting for you; `⠿`/`● running` while the agent streams or a tool runs). It does **not** carry the session name, model/provider, or context tokens — those all live in the **session rail widget** (§2.1, §7). *(current: the title bar also shows session name / model / `n/200k` tokens — move model+provider and tokens into the session widget and drop the session name from the bar.)*
- **Bottom status bar** — mode chip (left) + a **minimal** hint string: `zoom <level> · ^P palette`. **Hidden:** `⇧Tab → Build` (Build is deferred, §2) and `^C quit` (quit is a palette action / `:q`, §4). *(current: `render.rs:116–123` shows branch, `⇧Tab → Build`, and `^C quit` — trim to zoom + palette.)*
- **Quit** — removed from the status-bar hint; reachable via the palette (§4) and `:q`.
- **Message box** — the input is a **bordered box titled `message`**; **⏎ submits**, **Alt+⏎ inserts a newline** (`route.rs:112`); no hint row is rendered. *(current: the box shows the tui-textarea default cursor-line **underline** — a known fix, §9.)*

---

## 3. Signature Chat interactions `[V1]`

These are the "beyond chat-log" interactions that land on the calm Chat surface first (roadmap **P4**), de-risking the rendering/motion/tree-sitter stack before it also has to serve the autonomous loop. The rendering *infrastructure* — the motion engine, the tree-sitter parse, the dataviz widgets — is core substrate (core §3, and the Ⓡ glyphs below); this section describes how each **manifests on the Chat surface**. Canonical mock: `docs/ux/chat-mode.html` (and `docs/ux/rust-unlocks.html` for the Ⓡ set).

### 3.1 ① Semantic zoom
`Ctrl-scroll` / keys change the **altitude** of the transcript — turns **collapse to one-line summaries + activity glyphs** (zoom out) or **bloom to full prose/diffs** (zoom in). The transition **animates (Ⓡ2)**. Code-aware zoom **collapses to signatures** via tree-sitter (Ⓡ3) — function bodies fold to their signature line at higher altitude. Concretely there are three named altitudes — **summary · normal · detail** — shown in the status bar; `Alt+=`/`Alt+-` change altitude from any focus (terminals reserve `Ctrl`+/-/= for font zoom).

Summaries in v1 are **structural** (deterministic: turn headline + activity glyphs), not model-generated — a cost decision, see §12 (risks). Model-generated summaries are `[POST-V1]`.

### 3.2 ④ Object-first verbs
Select an **object** — an error, a file, a diff hunk, a **tree-sitter symbol**, a test — and get a menu of **agent verbs scoped to it**. This inverts prose-first chat for the common case: instead of describing what to act on, you point at it and choose the action. Symbol-level selection is powered by tree-sitter (Ⓡ3; see §3.3).

### 3.3 Ⓡ3 tree-sitter rendering (as it serves Chat)
One parse tree drives Chat's read-side code intelligence (core §3 owns the infrastructure; here is how it shows up in Chat):
- **Syntax highlighting** — capture names map to the design-tokens syntax palette (core §13).
- **Structural folding** — collapse function bodies in diffs and code blocks.
- **Symbol selection** — accurate `fn`/symbol byte-ranges make ④ object-first verbs precise at the symbol level.
- **Code-aware semantic zoom** — collapse-to-signatures feeds ① (§3.1).

### 3.4 Ⓡ4 live data viz (as it serves the economy drawer)
ratatui sparkline/gauge widgets render the economy ⑤ as a **glanceable dashboard** inside the rail drawer: animated token meters, usage-heat visualization, and the churn timeline (§5 here). This is Chat's only v1 consumer of Ⓡ4; Build's progress dashboards reuse the same widgets (Build spec).

### 3.5 Message rendering: markdown & code blocks `[V1]`
Assistant and user message bodies are **rendered**, not shown raw. *(current: message bodies render as a single plain `color::TXT` span with wrapping — `chat.rs:34,46` — no markdown, no code highlighting; only Detail-zoom **tool-result** bodies are highlighted, `chat.rs:127–165`. This is the gap to close.)*

- **Markdown** — headings, **bold**/*italic*, inline `code`, lists, blockquotes, and links render to styled ratatui spans (mapped to design tokens, core §13). Recommended parser: **`pulldown-cmark`** (small, pure-Rust, no runtime) → spans; cap nesting depth and fall back to plain text on parse issues. Wrapping (`Wrap { trim: false }`) is preserved.
- **Fenced code blocks** — ```` ```lang ```` fences are **syntax-highlighted** by the existing Ⓡ3 highlighter (`highlight_lines`), language resolved from the fence info string (falling back to plain-text for unknown/unspecified languages, and to no-highlight for prose). This reuses the P4 highlighter already used for tool-result code — it is a **wiring** task, not new infrastructure.
- **Interaction with ① zoom** — at *summary* altitude, rendered markdown collapses to the structural headline; at *normal*/*detail* it renders in full. Markdown/code rendering is orthogonal to the zoom altitude machinery.

Markdown is a Chat concern (message presentation); it is **not** a Build feature. Roadmap: **P4** (§9).

---

## 4. Command palette & command line `[V1]`

This spec **owns** the palette/command-line detail: it first lands in Chat (roadmap **P2**); Build references it. Canonical mock: `docs/ux/palette.html`.

- **`^P` — fuzzy, mode-aware action launcher.** Grouped (**session** · mode · navigate · context ⑤ · settings · branch ⎇`[POST-V1]` · recipes`[POST-V1]`); **each row shows its keybind** where one exists (the palette teaches its own shortcuts); ranks by recency + match quality. Surfaces global verbs (new/resume session, **quit**, settings) plus current-mode actions. It is an **overlay**, not a rail drawer (§2 here).
- **Session group** (§7) — **New session**, **Resume session ▸** (lists sessions, current repo first), **Rename session**. This is the primary session-management surface.
- **Quit** — a palette action (the status bar no longer hints `^C quit`, §2.2); also `:q`.
- **Rail drawers** — since drawers carry no keybinds for now (§2.1), the palette's *navigate* group is how you toggle **session / context ⑤ / repo / files**.
- **`:` — vim-style direct command line** for power users (`:chat`, `:new`, `:rename`, `:model …`, `:q`; `:build` is inert while Build is deferred).
- **Settings surfacing** — settings appear **read-only** in the palette's *settings* group (core §7.1); editing is file-first, no in-TUI editor in v1 (DB-backed in-app settings are a deferred direction, core §7.1).

---

## 5. The Economy subsystem ⑤ `[V1]`

The core doc **defers the full user-facing economy detail here** (core §5 defines the shared `ContextWindow` / `ChurnTimeline` / `TokenLedger` projections but points to this section for behavior). Everything is denominated in **tokens — never dollars** — to stay model-agnostic (no per-model price tables to track/drift).

The economy is a single subsystem: context items and subagents are both **token-spending entities** reporting into **one `TokenLedger`**. In Chat the headline is that **the user gets real-time, manual + automated visibility and control over their own context window** — and the *same machinery* constructs a delegated subagent's context (§5.4, §6). Its scope is the **current session** (§7): a session's bounded log is what makes the ledger and heat meaningful rather than an ever-growing aggregate.

### 5.1 ⑤a Cost-value ledger
Every context item shows **tokens + usage heat**.
- **Usage heat is a heuristic** — true per-item attention isn't exposed by provider APIs. It is derived from observable signals: references in the model's output, tool calls that target the item (re-read / edit), and prompt-cache accounting (cache-read vs cache-creation tokens). "Cold" deadweight = low heat, high tokens.
- **Manual control:** `pin` / `evict` (one keystroke to evict clearly-cold items).
- **Automation:** **auto-evict-cold** and **compact-at-threshold** — but it **never auto-evicts pinned items**; pin always overrides.

### 5.2 ⑤c Churn timeline
Per-turn **token deltas** (what entered / left the window each turn), flagging the #1 silent cost — **files re-sent every turn** — and nudging toward **pin / prompt-caching**. Rendered in the rail via Ⓡ4 (§3.4).

### 5.3 Token governor `[V1, optional ceiling]`
An **optional per-task token ceiling** with a **warn / auto-compact / pause** policy. No dollar governor; no model auto-routing in v1 (both `[POST-V1]`).

> **Multi-subagent leash is a Build concern.** Pausing before dispatching the *next* subagent across a *sequence* (the "subagent leash") belongs to Build's automated loop — see the Build spec. A **single Chat delegation** still reports its token spend into this same ledger (§6), so the ledger already accounts for a hand-dispatched unit; it just isn't gating a sequence in Chat.

### 5.4 The constructed-context assembler
The mechanism that assembles a **subagent's** context. Per the superpowers principle, a subagent **never receives the session history** — the assembler builds exactly what the unit of work needs (**the plan/task unit + relevant code**), scored by the **same ledger/heat machinery** (§5.1) that curates the user's own window. This is why the economy is the *orchestrator's core job*, not just a UI (core §4.4).

The assembler lands as a **primitive in P3** (with the rest of the economy) and is **wired to subagent dispatch in P5** (§6 here). It is a Chat-spec concern because Chat is where a single constructed context is first built and dispatched by hand; Build reuses the identical assembler to construct contexts across many tasks.

---

## 6. Chat's single-subagent delegation `[V1, from P5]`

Chat can hand **one discrete, non-trivial unit of work** to **one isolated subagent**. This is Chat's *use* of the shared subagent runtime (the runtime itself — executor, worktree isolation, `AgentProfile` — is core §4.4); described here is the **Chat UX** of that delegation.

- **Trivial edits stay inline.** Small, obvious changes are made directly in the conversation (the human-in-the-loop path, §8 here) — no worktree, no card.
- **A non-trivial unit gets a constructed context** (§5.4 here) and **runs in an isolated git worktree** (core §4.4). The delegation is **hand-dispatched** — you decide to delegate; there is no autonomous loop deciding for you.
- **The result folds back as a collapsible card** in the conversation (① zoom, §3.1) — expand to inspect, collapse to keep the transcript calm. You judge the result; there is no per-task review pipeline in Chat (that pipeline is Build's — Build spec).
- **One built-in `AgentProfile`** drives the delegation in v1 (system prompt + skill overlays + tool allow-list + model, shaped to mirror the `.claude/agents` schema — core §4.4, §7). The named/differentiated registry crystallizes later when Build needs implementer→reviewer→fix workers.
- **Spend reports into the ledger** (§5.3 here): a Chat delegation's tokens land in the same `TokenLedger` as everything else, within the current session (§7).

> Build (deferred) *automates* this exact runtime across a whole plan — dispatching many units in sequence with a scheduler and per-task review. **That is not a Chat concern**; see the Build spec. Chat delegates one unit, by hand, and folds it back.

---

## 7. Session management & history `[V1]`

A **session** is one bounded, resumable conversation — a partition of the event log by `session_id` (core §5). Sessions give each thread its own **bounded context window**, which is what makes the economy ⑤ meaningful (an unbounded forever-log defeats pin/evict/compact), and they give you **history**: threads you can return to.

- **Storage (core §5).** The **user-global** application database at **`~/.local/share/zoid/zoid.db`** (XDG data dir; **not** in the repo) holds the `events` table + a **`sessions` table** (`id`, `name`, `root_path`, `created_ts`, `last_touched_ts`); every event carries a `session_id`. Because the DB is global, each session records the **`root_path`** (repo/cwd) it belongs to. *(current: single implicit session in an in-repo `./.zoid/session.db`, always resumed — `main.rs:33–40,103–129`; this replaces both the location and the single-session model.)*
- **Auto-load last (default).** On launch, zoid resumes the **most-recently-touched session for the current repo** (`root_path` = the cwd/repo root). If none exists for this repo, it starts a fresh session. History is never lost silently.
- **New session.** Palette **New session** (or `:new`) starts a fresh session — a new `sessions` row, a new bounded log — in the current repo. Use it to start a clean thread (and a clean context budget).
- **Resume / history.** Palette **Resume session ▸** lists sessions **most-recent-first**, default-scoped to the current repo (option to show all repos), each row showing **name · last-touched · token total**. Selecting one resumes it (replay its `session_id` events).
- **Naming.** A session's name is **auto-derived** from its first user message (truncated), falling back to a timestamp; **renameable** via palette **Rename session** (or `:rename`). Name/duration/token-total are computable from the log + `sessions` row (core §5) and surface in the **session rail widget** (§2.1) and the title bar (§2.2).
- **Deletion/archive `[POST-V1 candidate]`** — a palette **Delete session** is a natural follow-on; kept out of the minimal v1 surface for now.

Roadmap: the `sessions` table + `session_id` + palette new/resume + auto-load-last-for-repo land in **P2** (§9), building on P0 persistence.

---

## 8. Providers, tools & Chat safety `[V1]`

- **Providers** — Chat uses the shared provider interface (streaming + tool-calling). Direction is **Ollama Cloud + a GLM model** over the OpenAI-compatible endpoint, Anthropic addable behind the same interface — see core §6 for the transport detail. Chat does not add anything to the provider layer.
- **Tools run in the working directory.** File read/write/edit, shell exec, code search, and the test runner execute in **cwd** — **human-driven and visible** (contrast Build, where tools run inside per-task worktree sandboxes — Build spec). Tool calls/results are events (core §5).
- **Chat is safe by human-in-the-loop.** You **see and drive every action** as it happens, turn by turn. Because you are present for each action, **cwd execution needs no sandbox and no blocker machinery** — anything outward-facing is simply something you are there to approve in the moment. (Build's isolation + blocker safety model is a separate mechanism for a separate mode — Build spec.) All actions are auditable, replayable events.

---

## 9. Chat's slice of the roadmap (P0–P5) `[V1]`

Chat is realized **completely before** the Build loop (core §9 gives the canonical sequence and the full-Chat-first rationale). Below are P0–P5 as **Chat deliverables**. The **mode seam** lands at the **tail** — after P5, before Build — because Chat is the non-removable floor and the seam is the extension thesis (core §4.2 sequencing note). **P6–P9 are Build and are not covered here** (Build spec).

| Phase | Lands (Chat deliverable) | Runnable end-state |
|---|---|---|
| **P0 · Spine & skeleton** | Cargo workspace; design-tokens module; event log + fold engine + `Transcript` projection; fake provider; bare `ratatui` shell; the `proptest` + `TestBackend`/`insta` harness (core §8) | `zoid` boots to an empty Chat frame, persists & replays a session. |
| **P1 · Chat MVP** | Provider (SSE streaming + tool-calling); the agent loop; core tools (fs read/write/edit, shell, code search) in cwd; **inline tool rendering with `→ peek`**; real multi-line input (`tui-textarea`) | A usable **manual chat coding agent**. |
| **P2 · Modal shell** | App-framework floor (focus/keys/mouse/panes); the rail component; **command palette `^P` + command line `:`** (§4); the **repo/session/context** rail drawers (§2.1) + **chrome consolidation** (§2.2); **user-global multi-session store** (`~/.local/share/zoid/zoid.db`: `sessions` table + `session_id`) + **palette new/resume + auto-load-last-for-repo** (§7); persistent mode indicator | Chat as a real modal TUI with sessions & history. |
| **P3 · Context economy ⑤** | `TokenLedger` + `ContextWindow`(heat) + `ChurnTimeline` projections; **pin/evict; auto-evict-cold + compact-at-threshold**; optional token ceiling; economy rail drawer with **Ⓡ4** dataviz; the **constructed-context assembler** primitive (§5) | Live manual + automated context control — and the substrate P5 reuses. |
| **P4 · Signature Chat** | **Ⓡ2** motion engine; **① semantic zoom** (structural summaries); **Ⓡ3** tree-sitter (highlight, structural fold, symbol selection); **④ object-first verbs**; **message rendering — markdown + fenced-code highlighting** (§3.5) | The "beyond chat-log" Chat experience. |
| **P5 · Orchestrator + subagent runtime (L1)** | Subagent executor; `git2` **worktree isolation**; the constructed-context assembler wired to dispatch; **one subagent at a time**; **available from Chat** — delegate a discrete unit (§6) | Dispatch an isolated subagent for a unit of work and fold its result back. |
| **Mode seam** *(tail of Chat)* | Open `ModeId` + `trait Mode` + `ModeRegistry`; refactor Chat behind it (Chat = non-removable floor). **Lands before Build.** | Core §4.2 — the boundary handing off to the Build spec. |

**Cross-cutting (every phase):** TDD default; every new Chat screen ships its design-tokens + a `TestBackend`/`insta` snapshot bound to `docs/ux/` (core §8, §13).

**Known fixes carried in this refinement** (fold into the relevant phase's work): remove the message-input **underline** (tui-textarea's default cursor-line style — set the cursor-line style to plain; `main.rs:122/309/353`); consolidate duplicate mode/branch chrome (§2.2); drop rail drawer keybind labels/glyphs (§2.1); render markdown + highlight fenced code in messages (§3.5).

---

## 10. Testing obligations specific to Chat

The shared harness, snapshot convention (`format!("{:#?}", terminal.backend().buffer())`), and the pure-core/fake-provider/worktree strategies are core §8 — **not restated here**. Chat adds:

- **Fidelity snapshots for the Chat screens** — the Chat conversation frame, the rail drawers (**session / context ⑤ / repo / files**), the `^P` palette (`docs/ux/palette.html`), inline tool `→ peek`, and the delegated result card. Each built to match its `docs/ux/` mockup (`chat-mode.html`, `palette.html`), drift surfacing as a reviewed diff per the core pipeline (core §13). Chrome consolidation (§2.2) is asserted by these snapshots (mode/branch appear once each).
- **Session management** — assert **New session** creates an isolated `session_id` log; **auto-load** picks the most-recently-touched session for the current `root_path`; `SessionList()` ordering (most-recent-first) and repo filtering; rename updates the `sessions` row; a fresh session starts with a fresh (bounded) context window.
- **Message rendering** — assert markdown → styled spans (headings/bold/inline-code/lists), fenced code → highlighted lines by fence language, and that plain prose is unaffected; assert the input box has **no** cursor-line underline.
- **Semantic-zoom structural summaries** — assert the deterministic structural summary (turn headline + activity glyphs) at each altitude (summary/normal/detail), and the code-aware collapse-to-signatures via tree-sitter. (Ⓡ2 motion of the transition is **not** snapshot-coverable — reduced-motion correctness + manual/gif review, core §8.)
- **Object-first verb scoping** — assert the verb menu is correctly scoped to the selected object (error / file / diff hunk / tree-sitter symbol / test).
- **Constructed-context assembler** — assert a delegated subagent's context contains the unit + relevant code and **never the session history** (the superpowers invariant, §5.4 here).

---

## 11. Chat-specific glossary

- **① Semantic zoom** — altitude control (summary/normal/detail) that collapses/expands the transcript by meaning (animated, Ⓡ2); structural summaries in v1, model-generated `[POST-V1]`.
- **④ Object-first verbs** — select an object (error / file / diff hunk / tree-sitter symbol / test) → a menu of agent verbs scoped to it; inverts prose-first chat.
- **⑤ Context economy** — the token-denominated subsystem: cost-value ledger + usage heat (⑤a), churn timeline (⑤c), optional token governor, and the constructed-context assembler. Real-time manual + automated context management, scoped to the current session.
- **Session** — a bounded, resumable conversation = one partition of the event log by `session_id` (core §5), tagged with its repo `root_path`; has a name, duration, and token total; the unit of history and of the context budget.
- **Constructed context (Chat's use)** — the precisely-assembled context a single Chat-delegated subagent receives (the unit of work + relevant code, never session history), built by the economy's assembler (§5.4).
- **Delegation (Chat)** — hand-dispatching **one** discrete, non-trivial unit to **one** isolated subagent whose result folds back as a collapsible card; trivial edits stay inline. (Build automates this across many units — Build spec.)

*(Shared terms — event-sourced core, projection, mode, mode seam, rail, orchestrator, subagent, AgentProfile, Ⓡ1–4 — are in core §11.)*

---

## 12. Chat-specific risks

*(Cross-cutting risks — app-framework floor, borrow-checker friction, build time, fake-provider drift, config surface — are core §12.)*

1. **Semantic-zoom summary cost** — cheap deterministic structural summaries (turn headline + glyphs) vs model-generated summaries (costly, spend tokens per fold). **v1: structural; model-generated `[POST-V1]`.** (Original §14.4.)
2. **Context-heat fidelity (⑤a)** — "usage heat" is a **heuristic** (output references, tool targeting, prompt-cache accounting), not true attention (unavailable via API). A weak heuristic makes "cold" misleading and risks bad auto-evictions. Mitigate: conservative auto-evict (suggest, or evict only clearly-cold + unpinned), **pin always overrides**, tune against real sessions, **never auto-evict pinned items**. (Original §14.9.)
3. **Lost-thread on new session** — starting a new session can hide an older thread the user meant to continue. Mitigate: **auto-load last-for-repo** by default (§7), always-visible **Resume ▸** recents in the palette, and the session rail widget showing which session is active.
4. **Markdown-rendering dependency & complexity** — rendering markdown adds a dependency (`pulldown-cmark`) and span-mapping/wrapping complexity, and risks mis-rendering pathological input. Mitigate: a small, pure-Rust, well-maintained parser; render to spans with capped nesting; fall back to plain text on parse issues; snapshot-test representative documents (§10).
