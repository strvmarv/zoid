# zoid — Core Architecture · Design (shared base)

**Date:** 2026-06-30 (extracted from the 2026-06-28 combined design)
**Status:** Approved design — the shared substrate both mode specs build on.
**Author:** strvmarv (with Claude)
**Language decision:** **Rust** (edition 2021) — chosen after a build spike (see `spikes/RESULTS.md`).

> **Spec set.** This is one of three documents. This **core** doc holds everything cross-cutting — the event-sourced spine, the modal seam, the shared subagent runtime, data model, extensibility, testing, and visual language. The two mode specs layer on top and reference it:
> - **Chat mode** → `2026-06-30-zoid-chat-mode-design.md` (the v1 near-term surface: conversation + manual implementation + single-subagent delegation).
> - **Build mode** → `2026-06-30-zoid-build-mode-design.md` (deferred: the autonomous 7-phase loop that *automates* Chat's subagent runtime).
>
> Where a section is mode-specific it says so and points to the owning doc. Nothing here is a release — see the roadmap (§9).

---

## 1. Overview

**zoid** is a cross-platform, terminal-native coding agent, built from scratch in **Rust**, distributed as a single self-contained native binary (~6 MB, validated by spike). It is an open-source product, not a one-off tool.

Its thesis: current TUI coding agents are all variations on "chat log + sidebar." zoid instead treats **the conversation as a database** (an event-sourced log) and the UI as a set of queries (projections) over it, which unlocks interaction paradigms chat-first tools can't cheaply copy: semantic zoom, object-first actions, a live token economy, orchestrated subagents, and — later — a native plan→execute→verify workflow loop.

The whole interface is **modal** (like vim/helix) with **isolated modes**. v1 realizes two:
- **Chat** — conversation + *manual* implementation; you drive turn by turn, delegating a discrete unit to a single subagent when it's worth it. (Owning doc: the Chat spec.)
- **Build** — the *entire* autonomous loop as a stepped pipeline; entering it is the act of consent to autonomy. (Owning doc: the Build spec — **deferred beyond the near-term Chat work**.)

Modes never mix behaviors, and they share one spine: the event log below. Switching modes swaps the active surface, never the state.

---

## 2. Goals & Non-Goals

### Goals (product-level)
- A coding agent that feels **at home on large, high-resolution terminals** without abusing the space (protect reading ergonomics; spend extra space on parallel state, not long lines).
- **Distribution as a single native binary** per platform — no runtime to install.
- **Context as a first-class, user-managed resource**, measured in tokens, with real-time manual and automated control (§Economy, detailed in the Chat spec).
- **Extensible by design**: the provider, tool, and mode interfaces — and the in-process sandboxed WASM host boundary — are defined early so plugins are an add-on, not a refactor. The plugin surface itself ships post-roadmap (§7); v1 compiles in a curated set.
- **Workflow-native** (Build): the brainstorm→plan→execute→verify loop is a first-class, triggerable capability. *(Deferred with Build.)*
- **Design fidelity**: the built TUI must match the canonical mockups in `docs/ux/`, enforced by snapshot tests (§8, §Visual Language).
- **Maintainable for years** by a Rust contributor base; architecture chosen to keep borrow-checker friction low (message-passing over an append-only log).

### Non-Goals
- Not an IDE; no LSP-grade editing surface in v1 (we shell out to the user's editor when needed). Tree-sitter gives read-side code intelligence — highlighting, folding, symbol selection — not live LSP diagnostics or refactors.
- Not a GUI/web app. Terminal only.
- No inline image/graphics-protocol rendering in v1 (no Sixel/Kitty) — designed-for via a render-backend seam (§3), shipped later (Ⓡ1).
- No multiplayer/collaboration in v1.
- No billing/dollar accounting — the economy is denominated in **tokens**.
- **No per-step approval dial** (a Build stance): zoid is autonomous-only between bookends; the sole interrupt is a blocker. *(Detailed in the Build spec.)*

---

## 3. Technology Decisions

| Decision | Choice (crate) | Rationale |
|---|---|---|
| Language | **Rust** (edition 2021) | Spike-validated: a true single static binary (~6 MB incl. the WASM engine that v1 defers, so v1 is smaller), ~10 ms cold start; best-in-class bespoke TUI rendering substrate; in-process WASM plugins; the most stable TUI ecosystem. Coding agents are I/O-bound, so the choice is about distribution + rendering ceiling + extensibility, not raw speed. |
| Distribution | **Single static binary** per target (linux-x64 musl, win-x64, osx-arm64, …) via cargo + GitHub Actions | Genuinely one file (spike: 6.2 MB), no runtime, no native sidecars. |
| TUI engine | **`ratatui` + `crossterm`** | Immediate-mode cell buffer: we render whichever projection we want each frame — exactly what semantic zoom ① and custom surfaces want. Most mature, most stable TUI stack. |
| App-framework layer (ours) | thin layer over ratatui: **focus ring, input/key routing, mouse hit-testing, pane/drawer manager** | The known cost of immediate-mode. The spike confirmed this is modest hand-written code; we own it as a small internal module. `tui-textarea` for multi-line input, `tui-input` where useful. |
| Async runtime | **`tokio`** | Streaming + the agent loop + sequential subagent execution; integrates with `crossterm`'s `EventStream` via `tokio::select!`. |
| LLM transport | **`reqwest`** + SSE (`eventsource-stream`/`reqwest-eventsource`) + **`serde`/`serde_json`** | Streaming + tool-calling. |
| Persistence | Append-only event log; **SQLite via `rusqlite`** | Durable, queryable; supports projections, branching, resume. `rusqlite` is synchronous, integrated via the single-writer actor (§4.1): one task owns the connection and serializes appends while readers use immutable snapshots — so blocking SQLite never blocks the async runtime. |
| Code intelligence | **`tree-sitter`** (+ grammars) & **`tree-sitter-highlight`**; diffs via **`similar`** | One parse tree drives symbol selection (④), structural folding, and highlighting (capture names map to the design-tokens palette). `syntect` only as optional fallback. |
| Git / worktrees | **`git2`** (libgit2) | Per-task worktree isolation for dispatched subagents. **Justified from Chat's P5 delegation onward** (a single hand-dispatched subagent already runs in an isolated worktree), then reused by Build's automated loop. |
| WASM host boundary | **trait/interface in v1; `wasmtime` added at the plugin phase** | Plugins are post-roadmap, so v1 defines only the in-process sandboxed host boundary and compiles in a curated tool set. The spike proved `wasmtime` embeds and statically links cleanly, so deferring the crate carries no design risk — and keeps it out of the v1 build (smaller binary, faster LTO). |
| Rendering backend | internal **`image-or-ASCII` trait** (ASCII v1; `ratatui-image` Kitty/Sixel/iTerm2 later) | Abstracts "draw a chart/diagram/graph" so inline raster graphics (Ⓡ1) drop in post-v1 without a rewrite. |
| Motion | internal **frame/transition engine** over ratatui | GC-free redraw enables animated transitions (Ⓡ2), gated by a motion budget + reduced-motion setting. |

**The one accepted cost (from the spike):** immediate-mode means we build the *app-framework floor* ourselves — focus ring, key routing, mouse hit-testing, pane/drawer manager. Bounded, well-understood; it buys the rendering ceiling that motivated choosing Rust.

**Build-time note:** release builds with LTO are slow (spike ~2m, much of it `wasmtime`, which v1 defers). Use fast dev builds for iteration; reserve full-LTO for release artifacts; consider `lto = "thin"` and workspace splitting.

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
│ (orchestr. + │ (fs, shell, search)   │ (one subagent at a time;│
│  self-gates) │                       │  + worktree isolation)  │
├──────────────┴───────────────────────┴────────────────────────┤
│ Providers (Ollama/GLM, Anthropic, …) · streaming · tool-calling│
└─────────────────────────────────────────────────────────────┘
```

### 4.1 Event-sourced core (the spine)
The session is an **append-only log of immutable events**. Visible state is a **pure fold (projection)** over that log. This single decision makes the novel features cheap:

- **Branching** *(post-roadmap)* = a new head pointing at an earlier event.
- **Undo / time-travel** *(post-roadmap)* = move a head backward; re-fold.
- **Multi-agent / delegates** *(post-roadmap)* = multiple heads over a shared or copied log. *(v1 uses the orchestrator + one active subagent head; concurrent heads is the deferred parallel case.)*
- **Context window** = itself a projection (an editable one — see the Chat spec's Economy).
- **Workflow** = a sub-log of task/phase/gate events; its board is a projection. *(Build.)*

**Phasing:** the core lands in **P0** (log + fold + `Transcript` projection + resume). The head-manipulation features above are *enabled* by this design but **deferred post-roadmap** (§9); v1 appends and folds a **single linear head** per session.

Events are small, append-only, serialized via `serde` (JSON, or a compact binary like `bincode`/`postcard` if profiling warrants). The store is SQLite via `rusqlite` (log table + indices).

**Concurrency shape (Rust-specific):** the core owns the log behind a single writer; readers get immutable snapshots; subagents/delegates communicate via channels (`tokio::sync`) rather than shared mutable state. This actor/message-passing shape keeps borrow-checker friction low and makes the concurrent components (UI loop, agent loop, active subagent) safe by construction.

### 4.2 Modal state machine — **the Mode seam (the one native extension point)**
The top level is a **state machine over the shared event log**. A *mode* is **`(main-area layout, registered rail drawers, keymap, projection set)`** plus a **policy**: what happens *between user turns* — act, or yield to the human. (Chat = the human is the clock: read → propose → you approve → one edit → stop. Build = the agent self-advances through its pipeline, yielding only at checkpoints. Everything else — rail affordances, prompt, tool allow-list, autonomy — is downstream of that one decision.)

Switching modes never copies state — it swaps the active surface. This:
- dissolves keymap collisions (each mode owns its keys),
- makes "where does X render" a non-question,
- gives a clean extensibility seam — **modes are zoid's one *native* extension surface** (§7); capability the ecosystem already standardizes (skills, agents, commands, MCP tools) is *adopted* rather than reinvented, while a new *interaction* surface is "a new mode, or a tool/drawer registered into a mode."

**The seam, concretely `[V1: seam; POST-V1: user-authored]`:**
- Mode identity is an **open `ModeId`** (newtype, consts for built-ins), *not* a closed enum. In the event-sourced core this buys forward-compatibility: a `ModeChanged { to: ModeId }` event written under a since-removed custom mode still **replays** — an unresolved id falls back to Chat.
- A `ModeRegistry` holds `impl Mode` entries. **Chat is the floor**: the registry is constructed with it, it cannot be deregistered, and it is the fallback whenever a `ModeId` fails to resolve or a mode errors at runtime.
- **Chat and Build are both native `impl Mode`** — Build is authored as the *first* mode behind the trait, never a hardcoded branch. Proving the seam with two built-ins is what keeps it honest.
- **User-authored modes `[POST-V1]`** are *declarative* `ModePolicy` descriptors (autonomy level, optional phase sequence, tool allow-list, prompt overlay) run by one generic `DeclarativeMode` impl — no per-mode code, no plugin code path. The descriptor shape is designed now; the file loader is deferred. Scripted/WASM mode *logic* is out of scope (a WASM plugin may still register tools/drawers — §7 — just not a mode's decision function).

> **Sequencing note (important):** the mode seam is **not** Build's to introduce. Because Chat is the non-removable floor and the seam is the extension thesis, the seam lands **before** Build — it is a Chat-spec deliverable (P6-front in the combined roadmap, now owned by the Chat sequence's tail). Build is then simply the *first additional* `impl Mode`. See §9.

### 4.3 The rail (per-mode drawers)
A reusable right-rail component hosts **stackable, collapsible drawers**; **each mode owns its own drawer set** (modes are isolated — the rail is a shared *component*, not shared *contents*). Chat: repo / session / context-economy ⑤ (Chat spec §2.1). Build: economy ⑤ / changed-files-tree / steering. Panes are for what you watch continuously; rail drawers are for what you consult contextually.

### 4.4 Subagent runtime (shared; **Chat drives one, Build automates many**)
A reusable executor that runs an agent turn in isolation: its own branch/head, optional **git worktree** for filesystem isolation, reporting results back as events. **Each subagent receives a precisely-constructed context — never the session history** (superpowers principle): the orchestrator assembles exactly what a task needs (its unit of work + relevant code), which is why the **context economy is the orchestrator's core job**, not just a UI (see the Chat spec's Economy section for the constructed-context assembler).

**The default execution model is orchestrator + subagent:** a long-lived **orchestrator** owns the conversation and the process; discrete units of **linear work** are handed to fresh subagents **one at a time**. This is the seam both modes share:
- **Chat (P5)** delegates a *single* discrete, non-trivial unit to *one* subagent by hand (trivial edits stay inline); its result folds back into the conversation.
- **Build (P7+, deferred)** *automates* the same dispatch across a whole plan — the scheduler is a driver over this identical runtime, not a second implementation.

**v1 runs one subagent at a time — no parallel fleet;** parallel fan-out (③) and async background delegates (⑧c) are deferred (§9). Workers are parameterized by an **`AgentProfile`** (system prompt + skill overlays + tool allow-list + model) **shaped to mirror the adopted `.claude/agents` file schema** (§7), so an existing ecosystem agent loads as a profile rather than requiring bespoke code. v1 ships one built-in profile (used by Chat's delegation); the named registry crystallizes when Build's review loop needs differentiated workers (implementer→reviewer→fix).

Per-task verification (a **review pipeline**: TDD → spec-compliance review → code-quality review → fix subagent) is a **Build** concern and lives in the Build spec; Chat's single delegation folds a result back for the human to judge.

---

## 5. Data Model (events & shared projections)

**Event (illustrative):**
```
Event {
  id: ulid               // monotonic, sortable
  parent: ulid?          // enables the DAG / branches (retained even though branching is deferred)
  branch: BranchId
  session_id: ulid       // groups events into one resumable session (see below)
  ts: long               // injected (no ambient clock in pure code)
  type: enum             // UserMessage | ModelDelta | ToolCall | ToolResult
                         // | ContextMutation | WorkflowStarted | TaskStateChanged
                         // | Approval | SelfGateResult | Decision | BlockerRaised
                         // | BlockerResolved | Merged | ...
  payload: <type-specific, serde-serialized enum variant>
  tokens: TokenStat?     // in/out/cached token counts where applicable
}
```

**Schema note:** `parent` and `branch` are **intentionally retained** even though branching/undo/time-travel are deferred (§4.1, §9) — the schema encodes the full vision so post-roadmap branching is *added behavior, not a migration*. v1 always writes a single linear branch. Likewise, the workflow/decision event types are reserved now though only Build emits them.

**Sessions & the application database.** The store lives at **`~/.local/share/zoid/zoid.db`** (XDG data dir, honoring `$XDG_DATA_HOME`; overridable via `ZOID_DB`) — a **single, user-global database**, not a per-repo file, and the **zoid application database** rather than merely an event log. v1 holds the append-only `events` table plus a **`sessions` table** (`id`, `name`, `root_path` — the repo/cwd the session belongs to — `created_ts`, `last_touched_ts`); every event carries a **`session_id`** so the one log partitions into independent, resumable conversations, **each with its own bounded context window** — the precondition for the economy ⑤ to mean anything (an unbounded forever-log defeats pin/evict/compact). Because the DB is user-global, a session records its `root_path`, so zoid auto-resumes the **last session for the current repo** and the palette lists history across (or filtered to) repos. The same DB is the intended home for **usage/metrics tracking** and, later, **DB-backed in-app configuration** (§7.1) — so the schema is designed as a general app store from the start, not retrofitted. `SessionList()` folds the session rows; the Chat spec owns the resume/new-session UX and the session rail widget.

**Shared projections (all pure functions of the log + a head):**
- `Transcript(head)` **(P0)** — ordered turns; supports semantic zoom ① (Chat spec).
- `ContextWindow(head)` **(P3)** — current items + token cost + usage heat (⑤a). *(Detailed in the Chat spec's Economy.)*
- `ChurnTimeline(head)` **(P3)** — per-turn token deltas; flags re-sent items (⑤c).
- `TokenLedger(scope)` **(P3)** — the economy ledger.
- `SessionList()` **(P2)** — sessions (id, name, `root_path`, created/last-touched ts, token totals) folded from the `sessions` table; supports auto-resume-last-for-repo + the palette resume picker + the session rail widget (Chat spec).
- `BranchDAG()` *(post-roadmap)* — the graph of heads; powers undo/fork/time-travel and Canvas mode ②.

**Build-only projections** — `WorkflowBoard`, `ChangedFiles`, `DecisionsLog` — are defined in the Build spec (they fold the workflow/decision event types reserved above).

---

## 6. Providers, Tools & the safety seam

- **Providers:** a provider interface (streaming, tool-calling). **Direction: Ollama Cloud + a GLM model** via the OpenAI-compatible endpoint (`POST https://ollama.com/v1/chat/completions`, Bearer `$OLLAMA_API_KEY`, `stream:true` → OpenAI SSE, `data:[DONE]`), reusing `eventsource-stream`. Tool-calling uses **OpenAI/Ollama `tools`+`tool_calls`**, not Anthropic `tool_use`. Anthropic remains addable behind the same interface. Use the latest models by default.
- **Tools:** file read/write/edit, shell exec, code search, test runner. **Two execution contexts:** in **Chat**, tools run in the **working directory** — human-driven and visible; in **Build**, tools run **autonomously inside per-task worktree sandboxes**. Tool calls/results are events in both.
- **Safety is mode-specific** and detailed in each mode spec: Chat is safe by **human-in-the-loop** (you see and drive every action; cwd execution needs no sandbox); Build is safe by **isolation + the finalize bookend** (worktrees; nothing reaches `main` until finalize; outward-facing actions are blockers). All actions/escalations are events (auditable, replayable).
- **Permission review `[POST-V1]`:** the v1 safety story is *deterministic* (no model reviews actions). A future **tier 1.5** local semantic risk gate and a tier 2 generative judge are designed-for behind a `PermissionReviewer` seam but deferred (§9).

---

## 7. Extensibility

**Principle — adopt, don't invent.** zoid is a *host* for the agentic primitives the ecosystem has already standardized as files; it invents only the one thing the ecosystem has *not* standardized — the modal TUI surface (§4.2). Everything else is parsed, not reimagined. Adoption is **provider-neutral**: skills/agents/commands are markdown + frontmatter (prompt assembly, not an API) and MCP is a model-agnostic protocol, so the whole surface works against the Ollama/GLM stack, not just Anthropic. **Target the Claude-style conventions** (the most widely authored, provider-neutral dialect). Layered seams, in priority order:

- **Adopted ecosystem entities `[V1: shapes; POST-V1: loaders]`:**
  - **Skills** — `SKILL.md`-style instruction files; injected as prompt overlays, composed into agents and into a mode's phases.
  - **Agents (subagents)** — `.claude/agents/*.md`-style profiles (name, description, tools, model + system-prompt body); loaded into the subagent runtime (§4.4) as an `AgentProfile`. This *is* the "grouping of skills" container — read, not redefined.
  - **Prompts / commands** — `.claude/commands/*.md`-style parameterized templates, surfaced through the command palette `^P` / `:` line.

  The internal structs are **shaped to mirror these formats now**; the file **loaders are built on demand** (first real needs: Chat's P5 delegation profile, then Build's differentiated workers).
- **Modes `[V1: seam; POST-V1: user-authored]`:** zoid's **one native** extension concept (§4.2). Built-ins (Chat, Build) ship as native `impl Mode`; user-authored modes arrive as declarative `ModePolicy` descriptors run by one generic interpreter — no code execution. Chat is the non-removable fallback.
- **MCP — the plugin protocol for external tools `[POST-V1]`:** external/existing tools and servers arrive over **MCP** (model-agnostic, the de-facto plugin standard). The primary answer to "use other tools as plugins."
- **WASM plugins `[POST-V1 — deferred, not forgotten]`:** in-process, sandboxed, capability-secured `wasmtime` modules for a *native* in-process tool/provider wanting near-native speed and OS-level isolation without a subprocess. Host-function boundary designed now, crate added at the plugin phase. **Lower priority than MCP.**
- **`[V1]`:** ship a fixed, curated set of providers/tools compiled in; design the trait interfaces (and the MCP + WASM host boundaries) now so the plugin surface is an add-on, not a refactor.

### 7.1 Configuration `[V1: minimal — formalizes what exists; POST-V1: full surface]`

v1 codifies only the configuration that **already exists in code** plus a precedence model; the broader surface is enumerated as deferred decisions so nothing is designed by accident.

> **Status (2026-07-01) — implemented.** §7.1's TOML config + full precedence
> (defaults → user-global → project → local → `ZOID_*` env) and an **encrypted-DB
> secret store** now ship (see `2026-07-01-config-screen-design.md`). This
> **amends the secrets rule below**: API keys may also live **encrypted in
> `zoid.db`** (key file `~/.local/share/zoid/secret.key`, `0600`; env still wins
> on read) — never in any `*.toml`. It also **supersedes the "read-only /
> no in-TUI editor" note**: a full-screen **configuration screen** (palette →
> *Open settings*, or `:config`) now edits config live and writes back to
> user-global (or the repo override via `r`). Model caps come from a basic
> **caps-only model registry** (`zoid-provider::model`). `base_url` override is
> surfaced but not yet applied to provider construction (follow-up).

- **Two namespaces, one principle.** *zoid-native* config lives in zoid's namespace; *adopted ecosystem entities* are read from their conventional Claude-style locations.
  - **zoid-native:** config is `~/.config/zoid/config.toml` (user global) and `./.zoid/config.toml` (project) — TOML. The **application database** is user-global at `~/.local/share/zoid/zoid.db` (XDG data dir, `$XDG_DATA_HOME` honored; **not** in the repo) — the event log + sessions today, usage/metrics and later DB-backed settings tomorrow (§5).
  - **adopted entities `[POST-V1 loaders]`:** `.claude/agents/*.md`, skills, `.claude/commands/*.md`, MCP server definitions (`.mcp.json`-style) — read from the ecosystem's locations, not redefined.
- **Precedence (low → high):** compiled defaults → user global → project `./.zoid/config.toml` → local gitignored `./.zoid/config.local.toml` → `ZOID_*` environment → CLI flags.
- **Current knobs (the whole v1 surface):** `OLLAMA_API_KEY` / `ANTHROPIC_API_KEY` (provider select **+ secret**), `ZOID_MODEL` (model), `ZOID_DB` (application DB path; default `~/.local/share/zoid/zoid.db`), `ZOID_REDUCED_MOTION` (motion). Provider `base_url` is currently hardcoded per provider.
- **Secrets rule:** API keys are **never** read from committed config — environment, the gitignored `config.local.toml`, or (later) an OS keyring only.
- **Surfacing:** settings appear **read-only** in the `^P` palette's *settings* group; editing is file-first — **no in-TUI settings editor in v1**.

**`[POST-V1]` — configuration decisions to review before they're built:** format finality (TOML vs `settings.json`-parity); secrets/keyring; entity-discovery paths + project-trust; provider/model config (`base_url` override, per-mode/agent model, auto-routing, request params); per-subsystem policy-as-config (economy ceilings + thresholds, permission rules, `notify-cmd` + channel toggles); the `ModePolicy` load path; validation/hot-reload/in-TUI editor/`zoid config` CLI/first-run onboarding; **DB-backed in-app configuration & usage tracking** (the user-global `zoid.db` as home for settings + metrics, vs file-only config).

---

## 8. Testing Strategy (shared)

- **Core (pure):** the event log + projections are pure functions → exhaustive unit + **property tests (`proptest`)** (fold determinism, branch/undo correctness, churn/ledger math). Highest-value tests; no I/O.
- **Agent loop:** test against a **fake provider** that replays scripted SSE streams + tool-call sequences — deterministic, fast, offline.
- **Tool execution:** sandboxed temp dirs; assert tool calls/results are recorded as events.
- **Subagent/worktree runtime:** integration tests creating real temp git repos/worktrees (`git2`); verify isolation + cleanup.
- **TUI / UX fidelity:** `ratatui`'s `TestBackend` renders each canonical screen with fixture data; `insta` snapshots assert it matches an approved buffer **built to match the `docs/ux/` mockup** (§Visual Language). Drift becomes a reviewed diff in every PR. Focus/keymap/mouse-hit-testing tested as pure logic over synthetic events.
- **Snapshot convention:** the established standard is `format!("{:#?}", terminal.backend().buffer())` (Buffer Debug, captures style). A plain `.to_string()` snapshot captures glyphs/layout only — use the Debug form where **style/color** correctness matters.
- **Not snapshot-coverable:** Ⓡ2 motion (separate reduced-motion correctness tests + manual/gif review) and Ⓡ1 graphics (per-terminal visual diff).
- **TDD is the default workflow** for implementation.

Mode-specific test obligations (Build scheduler ordering, autonomy/blocker escalation) live in the Build spec.

---

## 9. Development Roadmap (canonical sequence)

The build proceeds as a sequence of **vertical slices**. Each phase (a) compiles and runs, (b) is reviewable on its own, and (c) can reshape the phases after it. This is a **development sequence, not a release plan** — **nothing ships** until the full vision is realized.

**Ordering rationale (full-Chat-first):** Chat is realized completely before the Build loop, because (1) the economy ⑤ (P3) is the very substrate Build's per-subagent context-construction needs, and (2) the signature Chat interactions (P4) de-risk the rendering/motion/tree-sitter stack on a calm surface *before* it has to also serve the autonomous loop. P5 introduces the orchestrator + **sequential** subagent runtime (one at a time — no fleet), and the **mode seam** lands at the Chat→Build boundary; P6–P9 then automate the runtime into the full loop.

| Phase | Lands | Owning spec |
|---|---|---|
| **P0 · Spine & skeleton** | Cargo workspace; design-tokens module; event log (`rusqlite`) + fold engine + `Transcript`; fake provider; bare `ratatui` shell; `proptest` + `TestBackend`/`insta` harness | core → Chat |
| **P1 · Chat MVP** | Provider (SSE streaming + tool-calling); agent loop; core tools (fs/shell/search) in cwd; inline tool rendering with `→ peek`; real multi-line input (`tui-textarea`) | Chat |
| **P2 · Modal shell** | App-framework floor (focus/keys/mouse/panes); the rail component; command palette `^P` + command line `:`; repo/session/context rail drawers; **user-global multi-session store (`~/.local/share/zoid/zoid.db`: `sessions` table + `session_id`) + palette new/resume + auto-load-last-for-repo**; persistent mode indicator | Chat |
| **P3 · Context economy ⑤** | `TokenLedger` + `ContextWindow`(heat) + `ChurnTimeline`; pin/evict; auto-evict-cold + compact-at-threshold; optional token ceiling; economy rail drawer with Ⓡ4 dataviz; the **constructed-context assembler** primitive | Chat |
| **P4 · Signature Chat** | Ⓡ2 motion engine; ① semantic zoom; Ⓡ3 tree-sitter (highlight, structural fold, symbol selection); ④ object-first verbs | Chat |
| **P5 · Orchestrator + subagent runtime (L1)** | Subagent executor; `git2` **worktree isolation**; constructed-context assembler wired to dispatch; **one subagent at a time**; **available from Chat** (delegate a discrete unit) | Chat |
| **Mode seam** | Open `ModeId` + `trait Mode` + `ModeRegistry`; refactor Chat behind it (Chat = non-removable floor) — **lands before Build** | core/Chat boundary |
| **P6 · Build front-half (L2a)** | Build as the **first additional** `impl Mode`; stepped-pipeline shell; continuous Chat→Build switch; brainstorm → spec ✓ → worktree+baseline → plan ✓ (read-code + pre-flight); phase/task/gate modeled as data | Build |
| **P7 · Build execution — happy path (L2b)** | Scheduler walks the task DAG one task at a time; per-task implementer→spec-review→quality-review→fix + TDD; self-gates + auto-retry; 2-pane execute surface | Build |
| **P8 · Blockers & notifications (L2c)** | Blocker detection/classification/escalation; pause→resume; 4 notification channels + persistent badge | Build |
| **P9 · Finalize bookend** | Final broad review; finalize surface (decisions log + diff + merge/PR/request-changes/discard) + worktree cleanup + tests verify; collapsed summary card back in Chat | Build |

**Cross-cutting (every phase):** TDD default; every new screen ships design-tokens + a `TestBackend`/`insta` snapshot bound to `docs/ux/`; each phase is a reviewable checkpoint that may revise the phases after it.

**Deferred beyond the roadmap (`[POST-V1]`):** L3 recipe/workflow interpreter (data-ready from P6) · parallelism (fan-out ③, branch race, conflict radar, roster/leash UI) · async background delegates ⑧c · ② Canvas mode (branch-DAG map, undo, fork, time-travel) · Ⓡ1 inline raster graphics · extensibility loaders (skill/agent/command, MCP, WASM, more providers, model auto-routing) · permission tier 1.5/2.

---

## 10. Error Handling & Resilience (shared)

- **Streaming:** partial model output is persisted incrementally as `ModelDelta` events; a dropped connection leaves a resumable, consistent log.
- **Crash/resume:** because state = fold over an append-only log, restart replays to the last head. No separate save format.
- **Subagent failure:** isolated in its worktree; failure is an event; never corrupts main. Orphan worktrees are reclaimable.

Build-specific resilience (self-gate failure → retry → blocker escalation; blocker pause/resume) lives in the Build spec.

---

## 11. Glossary (shared terms)

- **Event-sourced core** — append-only log of immutable events; visible state is a pure fold (projection) over it.
- **Projection** — a pure function of the log + a head producing a view (Transcript, ContextWindow, ChurnTimeline, TokenLedger, …).
- **Mode (isolated)** — `(layout, rail drawers, keymap, projection set)` + a between-turns policy; behind the `ModeRegistry`. v1 has exactly two: Chat and Build. Chat is the non-removable floor/fallback.
- **Mode seam** — the open `ModeId` + `trait Mode` + `ModeRegistry` extension point; zoid's one native extension surface. Lands before Build.
- **Rail** — the per-mode drawer host (§4.3).
- **Orchestrator** — the long-lived agent that owns the conversation and the process; dispatches subagents.
- **Subagent** — a fresh agent given a **constructed context** (never session history) to do one unit of linear work in its own worktree; **one at a time in v1**.
- **Constructed context** — the precisely-assembled context each subagent gets; built via the economy's assembler (Chat spec).
- **AgentProfile** — system prompt + skill overlays + tool allow-list + model; shaped to mirror the `.claude/agents` schema.
- **Design tokens** — the single module holding all glyphs/colors/spacing/layout constants; every view renders from it (§Visual Language).
- **Ⓡ1–4** — Rust-enabled rendering: Ⓡ1 inline graphics (post-v1), Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz.

Mode-specific terms (autonomy contract, bookend, blocker, decisions log, per-task review pipeline, semantic zoom, object-first verbs, context economy) are defined in their owning mode specs.

---

## 12. Cross-cutting Risks

1. **App-framework floor over ratatui** — focus ring, key routing, mouse hit-testing, pane/drawer manager are ours to build. Mitigate: one small, well-tested internal module early; lean on `tui-textarea`/`tui-input`.
2. **Borrow-checker friction on shared session state** — mitigate (§4.1): single-writer log + immutable read snapshots + channel message-passing (actor shape), not shared `&mut`.
3. **Release build time** — full-LTO is slow (much of it `wasmtime`, deferred from v1). Mitigate: fast dev builds, `lto = "thin"`, workspace split, CI-only full-LTO. Re-measure when the plugin phase reintroduces `wasmtime`.
4. **Fake-provider drift** — scripted-SSE tests can drift from real provider behavior (event types, tool-call framing, error shapes). Mitigate: periodic contract tests against the live API; version the fake against a captured real transcript.
5. **Configuration surface** — only the minimal surface is specified (§7.1); the full surface is unresolved and must be decided before each piece is built, not accreted piecemeal.

Mode-specific risks (semantic-zoom summary cost, context-heat fidelity → Chat spec; sequential-execution latency, autonomy trust, notification reliability → Build spec).

---

## 13. Visual Language & UX Fidelity

The built TUI must match the canonical mockups in **`docs/ux/`** (see `docs/ux/README.md`). Fidelity is a pipeline ending in automated enforcement:

1. **Reference** — `docs/ux/*.html` (visual source of truth).
2. **Contract** — the mode specs (layouts, min-widths/keymaps/responsive rules, drawer registry) + the **visual-language table** in `docs/ux/README.md` (glyphs, mode accent colors, status + syntax palettes).
3. **Design-tokens module** — a single Rust module holds all glyphs, colors, spacing, and layout constants; **every view renders from it** (one source of truth). This is the §16 constraint: **no literal glyphs or raw color hex anywhere outside the tokens module.**
4. **Enforcement** — `TestBackend` + `insta` snapshot tests per canonical screen; the first snapshot is built to match the mockup, later drift surfaces as a reviewed diff. Same self-gate machinery as Build mode.
5. **Acceptance** — each TUI plan task's definition-of-done cites its `docs/ux/` mockup + snapshot test.

**Visual language (authoritative):** glyphs `● ✓ ◐ ☐ ⠿ ⎇ ⚠ ▸▾ ⛔ ▲ › ▌`; mode accents **Chat = blue / Build = amber** (finalize uses a green accent within Build); status ok/warn/error/branch; tree-sitter syntax palette. Full table in `docs/ux/README.md`.

**Limits:** snapshots cover structure/content/layout. Ⓡ2 motion and Ⓡ1 graphics are **not** snapshot-coverable — verified separately (§8).
