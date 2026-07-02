# Chat-Mode UX & Capabilities — Design Spec

**Date:** 2026-07-02
**Status:** Approved for planning
**Scope:** zoid Chat mode. Four cohesive enhancements to existing chat UX and agent capabilities, delivered as one spec in four independently-shippable phases.
**Supersedes/extends:** `docs/superpowers/specs/2026-06-30-zoid-chat-mode-design.md` (chat-mode v1). Builds on the shared base in `-zoid-core-architecture.md`.

---

## Goal

Make the agent loop **visible, testable, and interactive** without leaving the event-sourced grain of the system: surface in-flight tool activity, let the model ask the user structured questions mid-turn, let the model publish a live task list to the rail, and make list navigation wrap. Leave clean seams for tool approval and dynamic tool registration without implementing them yet.

## Architecture (the backbone)

Everything except the navigation fix rides the **existing tool loop** (`crates/zoid/src/agent.rs`, `run_agent_turn` / `run_turn_inner`). Today all five registered tools are one kind — run synchronously via `spawn_blocking`, return text. This spec grows the tool model into a small taxonomy:

| Kind | Runs how | Returns | Examples |
|------|----------|---------|----------|
| **Local** (exists) | `spawn_blocking`, does I/O | tool output text | `read_file`, `write_file`, `edit_file`, `search`, `shell` |
| **Emitting** (new) | validates args, appends an `EventKind`, no I/O | short ack string | `update_tasks` |
| **Interactive** (new) | suspends the loop, prompts the UI over the `AgentUpdate` bus, awaits an answer | the user's answer as tool result | `ask_user` |

The two new kinds fill **seams** introduced in Phase 2 (`ToolKind`, `ToolGate`). The interactive "park and await the UI" mechanism is the same primitive a future tool-approval gate will use — approve/deny is just an `ask_user` with two choices consulted by `ToolGate`.

## Global Constraints

Copied verbatim from binding project rules; every task inherits these.

- **§16 token purity:** No literal glyphs or hex color values in rendered UI code outside `crates/zoid-tui/src/tokens.rs`. Comments and tests are exempt. New status/indicator glyphs MUST be added as named tokens in `tokens.rs` and referenced by name. The middle-dot `·` is ordinary punctuation and may be used inline.
- **Event log is faithful:** The core event layer records what the model emitted, verbatim. Validation/enforcement/policy live in the tool layer (edge) or prompt guidance, never by mutating or rejecting recorded truth. (`EventKind` stays `Eq`; tool args are kept as raw JSON strings where they already are.)
- **Provider:** Ollama Cloud native `/api/chat` (NDJSON, Bearer `$OLLAMA_API_KEY`, default `glm-5.2:cloud`). Tool calling uses OpenAI/Ollama `tools` + `tool_calls` shape, never Anthropic `tool_use`.
- **Secrets** never land in committed files.
- **Commits:** no `Co-Authored-By`/co-author trailer.
- **Tests:** each phase ships with tests; no test asserts nothing; no snapshot committed without inspection.

---

## Phase 1 — Wrap-around list navigation (④)

### What
Every list overlay's selection cursor wraps end-to-end (opencode-style): moving down past the last row lands on row 0; moving up from row 0 lands on the last row.

### Design
The single shared movement primitive is `nav()` in `crates/zoid-tui/src/palette.rs` (currently clamps: `next.clamp(0, len-1)`). Change it to wrap:

```rust
/// Move `cur` by `delta` within `0..len`, wrapping at both ends.
/// Returns `cur` unchanged when `len == 0`.
pub fn nav(cur: i64, delta: i64, len: usize) -> i64 {
    if len == 0 { return cur; }
    (cur + delta).rem_euclid(len as i64)
}
```

All overlays already route Up/Down through `nav()` (palette, objects, verbs, sessions, and both config axes — field and section), so this one change makes all five wrap uniformly.

### Deliberately unchanged
- `Zoom::zoom_in`/`zoom_out` saturate at the ends (altitudes are a bounded ladder, not a ring). Untouched.
- The Tab **focus ring** (`focus_next`) already wraps. Untouched.

### Testing
- `palette.rs`: replace `nav_clamps` with `nav_wraps` — assert `nav(2,1,3)==0`, `nav(0,-1,3)==2`, `nav(0,-1,0)==0` (empty guard), `nav(1,1,3)==2` (interior unchanged).
- Existing config-section/field movement tests updated for wrap-at-ends expectations.

---

## Phase 2 — Tool visibility, seams & testkit (①)

Four pieces. Only the first two are user-visible; the last two are seams/infrastructure.

### 2.1 In-flight tool indicator (visibility)
Today a tool is invisible between its `ToolCall` and `ToolResult` events — a slow `shell` looks like a hang. Add a transient "active tool" to `ShellState` (e.g. `active_tool: Option<String>`), set when the agent loop dispatches a tool and cleared when its result arrives. Render a spinner line in the conversation tail / status area using the existing `tokens::glyph::RUNNING` (`◐`) glyph, e.g. `◐ running · shell …`.

Driven off signals already crossing the agent→UI `mpsc::Sender<AgentUpdate>` bus: add `AgentUpdate::ToolStarted { name }` emitted immediately before dispatch; the existing tool-result path clears it. No new channel.

### 2.2 `ToolGate` seam (always-allow)
A gate consulted in `agent.rs` immediately before each tool dispatch:

```rust
pub enum Gate { Allow, Deny(String) }
pub trait ToolGate: Send + Sync {
    fn check(&self, call: &ToolCall) -> Gate;
}
pub struct AllowAll;
impl ToolGate for AllowAll { fn check(&self, _: &ToolCall) -> Gate { Gate::Allow } }
```

v1 wires `AllowAll`. `Gate::Deny(reason)` short-circuits dispatch and feeds `reason` back as the tool result (loop continues). This is the exact insertion point where Phase 4's `ask_user` later powers interactive approve/deny — no behavior now, just the seam.

### 2.3 `ToolKind` seam (extensibility)
`Tool` grows a kind declaration; dispatch matches on it.

```rust
pub enum ToolKind { Local, Emitting, Interactive }
// on trait Tool:
fn kind(&self) -> ToolKind { ToolKind::Local } // default: existing tools unchanged
```

This is both how Phases 3–4 distinguish emitting/interactive tools and the declared spot where config/plugin-defined tools would later register. **No dynamic loading in v1** — the compiled-in `registry()` stays; only the enum + dispatch match arm are added.

### 2.4 `zoid-testkit` crate (shippable test harness)
New workspace member `crates/zoid-testkit` (added to root `Cargo.toml` `members`). Depends **downward** only on `zoid-core` + `zoid-provider` (never `zoid-tools`/`zoid` — keeps the dependency graph honest). Provides:

- `ScriptedProvider` — implements the provider trait, yields a caller-supplied script of `ProviderEvent`s (including `ToolCall`s and terminal events). Promotes the ad-hoc scripted provider currently inlined in `crates/zoid/tests/agent_loop.rs` into a documented public API.
- Assertion helpers to drive `run_agent_turn` and inspect the resulting event log / filesystem effects.
- **Auto-responder** for interactive tools (Phase 4 dependency): a test hook that answers `AskUser` requests with a scripted answer via the `oneshot`, so `ask_user` is exercisable headlessly.

Documented (`//!` crate docs + at least one `examples/` or doctest) so external agent builders test the loop with no live model.

### Testing
- `zoid-testkit` self-tests: `ScriptedProvider` yields events in order; auto-responder answers an `AskUser`.
- `agent.rs`: `ToolGate::Deny` short-circuits and feeds the reason back; `AllowAll` passes through. `AgentUpdate::ToolStarted` is emitted before and cleared after dispatch.
- Migrate `agent_loop.rs`'s inline scripted provider to `zoid-testkit` (behavior-preserving; existing assertions stay green).

---

## Phase 3 — Task widget (③)

Event-sourced, rail-only. The model publishes a live task list; it renders in a new rail drawer and rehydrates on resume; it is **not** inlined into the conversation transcript.

### 3.1 Data model (`zoid-core`)
```rust
pub enum TaskStatus { Pending, Active, Done }
pub struct TaskItem { pub text: String, pub status: TaskStatus }
// new EventKind variant:
EventKind::Tasks { items: Vec<TaskItem> }
```
**Full-snapshot semantics:** each `update_tasks` call carries the entire current list; last-write-wins. Round-trips through the event log like every other `EventKind` (stays `Eq`; `Serialize`/`Deserialize`).

### 3.2 Projection (`zoid-core`)
```rust
/// Latest task list, or empty if the model never published one.
pub fn tasks(events: &[Event]) -> Vec<TaskItem>;
```
Returns the `items` of the most recent `Tasks` event. Subagent-branch events filtered out, consistent with `conversation()`.

### 3.3 Tool `update_tasks` (`zoid-tools`, `ToolKind::Emitting`)
- Spec (JSON Schema): `{ tasks: [{ text: string, status: "pending"|"active"|"done" }] }`.
- `run`: parse+validate the array, request the event append, return a one-line ack (`"3 tasks · 1 active"`). No I/O.
- **Cardinality is not enforced** (faithful log). The tool *description* encourages "at most one Active task at a time" as prompt guidance.
- Emitting tools need to append an event as their effect. The dispatch path routes `ToolKind::Emitting` results to an `EventKind` append (the `Tasks` event) in addition to the normal `ToolResult` ack — defined in `agent.rs` alongside the kind match from 2.3.

### 3.4 Rail drawer (`zoid-tui`)
- New `DrawerId::Tasks`, appended **after** `Context` in `ShellState::new` (renders at the bottom of the rail; the manual stacking loop in `layout.rs` flows it with no `Constraint` edit).
- `TASKS_BODY_ROWS` constant + a `drawer_body_rows` arm.
- `render_tasks_body`: one line per task — a status glyph + truncated text. **Token glyphs** (reuse existing): `Pending → tokens::glyph::PENDING (☐)`, `Active → tokens::glyph::RUNNING (◐)`, `Done → tokens::glyph::PASS (✓)`. Active line accented, Done line dimmed. Empty state: a dim `no tasks` line (always visible per design).

### Testing
- `zoom.rs`/`projection.rs`: `tasks()` returns latest snapshot; last-write-wins across two `Tasks` events; empty when none.
- `event.rs`: `Tasks` round-trips through serialize/deserialize.
- `zoid-tools`: `update_tasks` validates good/bad args, unknown status is an error output (not a panic), ack string format.
- `zoid-tui` snapshot: rail with a 3-task list (one of each status) + empty state.
- Integration (via `zoid-testkit`): scripted `update_tasks` tool call produces a `Tasks` event and the projection reflects it.

---

## Phase 4 — `ask_user` interactive tool + question overlay (②)

The model asks a structured question mid-turn; the loop suspends until the user answers.

### 4.1 Tool `ask_user` (`zoid-tools`, `ToolKind::Interactive`)
- Spec (JSON Schema): `{ question: string, choices?: string[] }`.
- `choices` empty/absent → **free-text** mode.
- `choices` present → **pick-list** mode.

### 4.2 Interactive runtime (`zoid` bin)
New `AgentUpdate` variant carrying the return path:
```rust
AgentUpdate::AskUser {
    question: String,
    choices: Vec<String>,
    reply: tokio::sync::oneshot::Sender<Answer>,
}
pub enum Answer { Choice(String), FreeText(String), LetYouDecide }
```
Flow:
1. Agent loop hits the `ask_user` call → sends `AgentUpdate::AskUser { …, reply }` on the existing `ui_tx` and awaits the `oneshot`.
2. Main event loop raises `Overlay::Question`, stashing the `reply` sender in `ShellState`.
3. User answers → main loop sends the `Answer` down the `oneshot`.
4. Loop resumes; the answer string becomes the `ask_user` `ToolResult`; model continues.

Because the call and answer are already `ToolCall`/`ToolResult` events, the Q&A is **logged and rehydrates for free** — no extra persistence. `LetYouDecide` serializes to a sentinel result string (e.g. `"[let you decide]"`).

### 4.3 Question overlay (`zoid-tui`)
- New `Overlay::Question` variant + `QuestionState { question, choices, selected, free_text: String, mode }`.
- **Pick-list mode:** renders the model's `choices` followed by two synthetic trailing entries — `Other… (type my own)` and `— let you decide —`. Selecting `Other…` switches the overlay into free-text entry; selecting `— let you decide —` returns `Answer::LetYouDecide`. Selection movement uses the Phase-1 wrapping `nav()`.
- **Free-text mode** (no choices, or after choosing `Other…`): a text input; `⏎` submits `Answer::FreeText`; an **empty submit** returns `Answer::LetYouDecide`.
- `route_question_key` handles the keys; `render_question` draws a centered card (reuse `layout::centered`, consistent with the settings card).

### 4.4 Escape semantics
- **Esc = hard-abort the turn** via the existing streaming-interrupt path. Reaching for Esc means "we're drifting, I'm taking the wheel." No answer is sent; the turn stops and control returns to the user. (The oneshot sender is dropped; the loop's await treats a dropped sender as an abort signal and unwinds cleanly.)
- **Deciding for yourself is a positive choice** (`— let you decide —` / empty submit), never a cancel. Every outcome is intentional; no keystroke carries two meanings.

### Testing
- `zoid-tools`: `ask_user` spec shape; free-text vs pick-list selection by presence of `choices`.
- `zoid-tui`: `route_question_key` — arrowing wraps through choices + the two synthetic entries; `Other…` flips to free-text; empty submit → `LetYouDecide`; Esc path signals abort. Snapshot of both overlay modes.
- Integration (via `zoid-testkit` auto-responder): scripted `ask_user` call → auto-answer via `oneshot` → answer appears as the `ToolResult` and the loop continues. A dropped-sender case asserts clean abort.

---

## Build order & dependencies

1. **Phase 1** (wrap nav) — isolated; ship first.
2. **Phase 2** (visibility + `ToolGate` + `ToolKind` + `zoid-testkit`) — establishes the seams and the harness Phases 3–4 rely on.
3. **Phase 3** (task widget) — first `ToolKind::Emitting` consumer; new `EventKind` + projection + rail drawer.
4. **Phase 4** (`ask_user`) — first `ToolKind::Interactive` consumer and first non-trivial future `ToolGate` use; reuses Phase-1 wrapping in the overlay and Phase-2's testkit auto-responder.

Each phase is independently testable and mergeable. Phases 3 and 4 both depend on Phase 2's seams; Phase 4's tests depend on Phase 2's testkit.

## Out of scope (seams only, no implementation)

- **Tool approval** — the `ToolGate` seam exists; no interactive approval policy ships.
- **Dynamic/plugin tool registration** — the `ToolKind` declaration point exists; the compiled-in `registry()` is unchanged.
- **Task interactivity** — the rail task widget is read-only display; the user cannot edit the model's tasks.
- **Q&A special transcript rendering** — `ask_user` call/answer render via the existing tool-call/result path; no bespoke Q&A chip in v1.
