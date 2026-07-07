# zoid — Inline Question Cards · Design

**Date:** 2026-07-06
**Status:** Self-reviewed, pending user review
**Slice:** Follow-on to Slice 4 (URL import wizard). Unifies the two human-input surfaces (`ask_user` + `apply_mode_mapping` approval gate) into one inline-card mechanism.
**Author:** gomanjoe (with Claude)

> **Spec set.** This continues the mode/skill direction:
> - **Slice 4 — URL import wizard** → `2026-07-05-url-import-wizard-design.md`. **Merged** `0a0e981`.
> - **This doc** — inline question cards (replaces the modal overlay for both `ask_user` and the wizard's approval gate).

---

## 1. Overview

Today zoid has two surfaces that pause for human input:

1. **`ask_user`** — the model calls the `ask_user` tool; the agent loop intercepts `ToolKind::Interactive` by name, raises `AgentUpdate::AskUser { question, choices, reply }`, parks on a oneshot, and the bin raises a **modal overlay** (`Overlay::Question`) capturing keyboard input. The question text lives in transient `QuestionState` — it is **not** persisted in the event log. On reply, a `ToolResult` carries the answer back to the model.

2. **`apply_mode_mapping`** (the wizard's approval gate) — the model calls `apply_mode_mapping`; the agent loop intercepts `ToolKind::Approving`, raises `AgentUpdate::ModeMappingApproval { mapping, summary, reply }`, parks, and the bin raises the same modal overlay with a longer summary. Same reply path.

Both surfaces use the same overlay, the same `QuestionState`, the same reply channel — but they ride **two separate `AgentUpdate` variants**, **two separate `ToolKind` tags**, and **two separate dispatch arms** in the agent loop. The question content is not in the event log; the overlay is the only record that a question happened.

**This design replaces the modal overlay with an inline card rendered in the conversation body, and unifies the two surfaces into one mechanism.**

### Why inline

The modal overlay is the wrong shape for a conversation:

- **It hides the question.** The card floats over the scrollback; the user can't see the model's reasoning that preceded the question without dismissing the overlay.
- **It doesn't persist.** Once answered, the question disappears. The event log has a `ToolResult` ("Skip") but not the question ("How should I handle the error?"). Reopening a session shows a tool result with no context.
- **It's a separate code path.** Two `AgentUpdate` variants, two `ToolKind` tags, two dispatch arms, one overlay renderer. The wizard duplicates the `ask_user` machinery for no reason — both surfaces are "the model asks, the human answers."

The inline card fixes all three: the question renders at its natural position in the conversation (between the assistant turn and the tool result), it persists in the event log as a `QuestionAsked`/`QuestionAnswered` pair, and the two surfaces collapse to one dispatch arm.

### In one sentence

Replace the modal question overlay with an inline card rendered in the conversation body, unify `ask_user` and `apply_mode_mapping`'s approval gate into one event-driven mechanism (`QuestionAsked`/`QuestionAnswered` + a typed `QuestionKind`), and delete `ToolKind::Approving` + `AgentUpdate::ModeMappingApproval` + `Overlay::Question`.

---

## 2. North star (inherited)

**zoid is a thin, stable host; the runtime below the waist is unchanged.** This slice is entirely above the waist: it touches the event schema (additive), the agent loop's dispatch (unification), the TUI render layer (inline card), and the input routing (soft-capture). No provider-facing change, no on-disk format change, no runtime change.

---

## 3. Scope

### In scope
- New `EventKind::QuestionAsked` / `EventKind::QuestionAnswered` variants (additive to the schema).
- New `QuestionKind` enum (`Ask` | `ModeMapping { mapping }`).
- Unify the `ask_user` and `apply_mode_mapping` dispatch arms in `run_agent_turn`.
- Inline card rendering in `build_conversation` (new `ChatMsg::Question` variant + `render_question_card`).
- Soft-capture input routing while a question is open (typing = answer, ↑↓ = pick, Enter = submit, Esc = cancel).
- Delete `ToolKind::Approving`, `AgentUpdate::ModeMappingApproval`, `Overlay::Question`, `render_question` overlay.
- Body-cache invalidation on cursor movement.
- Exhaustive-match updates (`projection.rs`, `compaction.rs`).
- Tests (unit + integration) + old-session load compatibility.

### Out of scope
- Provider-facing tool schema changes (the `ask_user` / `apply_mode_mapping` tool schemas are unchanged; only the client-side `kind` tag moves).
- On-disk session format migration (old sessions load as-before; no script).
- Runtime behavior (modes, skills, loader — untouched).
- Multiple concurrent questions (one question open at a time, same as today).
- Richer question kinds (e.g. file pickers, multi-select) — `QuestionKind` is extensible but only `Ask` + `ModeMapping` ship now.

---

## 4. Event model

### 4.1 New `EventKind` variants

In `crates/zoid-core/src/event.rs`:

```rust
/// A question the model asked the user via `ask_user` (or `apply_mode_mapping`'s
/// approval gate). Rendered as an inline card in the conversation. Paired with
/// a `QuestionAnswered` carrying the same `id`.
QuestionAsked {
    id: String,           // the tool-call id (matches the ToolCall.id)
    kind: QuestionKind,
    question: String,     // the question text (or the full mapping review for the wizard)
    choices: Vec<String>, // empty = free-text only (no pick list)
},

/// The user's answer to a `QuestionAsked`. `id` matches the question. The
/// card collapses to a one-line summary after this lands.
QuestionAnswered {
    id: String,
    answer: String,        // the chosen choice, the free-text reply, or "cancelled"
},
```

The `id` is the tool-call id (same as `ToolResult.id`). This lets `build_conversation` pair a `QuestionAsked` with its `QuestionAnswered` and the matching `ToolResult` by the same key.

The `question` field carries:
- For `QuestionKind::Ask`: the `question` arg from the `ask_user` tool call.
- For `QuestionKind::ModeMapping`: the output of `detailed_approval_summary(&mapping)` — the full multiline mapping review (already newline-aware via `22f2b15`).

The `choices` field carries:
- For `Ask`: the `choices` arg from `ask_user` (may be empty → free-text only).
- For `ModeMapping`: `["Approve", "Reject", "Adjust"]`.

### 4.2 `QuestionKind`

```rust
/// What kind of question the card represents. Drives rendering + the bin's
/// side-effect on answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionKind {
    /// A plain `ask_user` question (free-text or pick-list).
    Ask,
    /// The wizard's mode-mapping approval. The `mapping` rides here so the bin
    /// can materialize on "Approve" without re-parsing it from anywhere.
    ModeMapping { mapping: Box<ModeMapping> },
}
```

`Box<ModeMapping>` because `ModeMapping` is non-Copy and the event is `Clone` (events are cloned into projections; the box keeps the clone cheap and the enum size bounded).

`QuestionKind` lives in `crates/zoid-core/src/event.rs` (alongside `EventKind`) or a new `crates/zoid-core/src/question_kind.rs` — either is fine; it's a small type. Recommendation: same file as `EventKind` to keep the schema in one place.

### 4.3 What the model sees

The model **does not** see `QuestionAsked` / `QuestionAnswered`. Those are UI/persistence concerns. The model-facing contract is still `ToolResult`: the user's answer is the tool's output, same as today. The model calls `ask_user`, the tool returns `"Skip"` (or `"Approve"`, or the free-text reply). The new events are purely for the human-facing conversation view + session persistence.

This keeps the provider path unchanged: no tool schema change, no new tool, no provider-side awareness of the card mechanism.

---

## 5. Agent loop

### 5.1 Today's two arms

The `match kind` block in `run_agent_turn` (`crates/zoid/src/agent.rs`) has two relevant arms today:

- `ToolKind::Interactive if tc.name == "ask_user"` → parse question/choices from `tc.args`, raise `AgentUpdate::AskUser { question, choices, reply }`, park on a oneshot, emit `ToolResult` with the answer.
- `ToolKind::Approving if tc.name == "apply_mode_mapping"` → parse mapping, raise `AgentUpdate::ModeMappingApproval { mapping, summary, reply }`, park, emit `ToolResult`.

### 5.2 Collapsed to one arm

Both collapse to **one arm** keyed on `ToolKind::Interactive`:

```rust
Some(ToolKind::Interactive) if tc.name == "ask_user" || tc.name == "apply_mode_mapping" => {
    let (kind, question, choices) = if tc.name == "ask_user" {
        let question = tc.args.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let choices = tc.args.get("choices").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        (QuestionKind::Ask, question, choices)
    } else {
        let mapping = parse_mapping_args(&tc.args).map_err(|reason| /* emit error ToolResult, continue */)?;
        let summary = detailed_approval_summary(&mapping);
        let choices = vec!["Approve".into(), "Reject".into(), "Adjust".into()];
        (QuestionKind::ModeMapping { mapping: Box::new(mapping) }, summary, choices)
    };
    // Emit QuestionAsked BEFORE parking — so the card renders inline immediately.
    emit(&session, &mut events, ui, &config.branch,
        EventKind::QuestionAsked { id: tc.id.clone(), kind, question: question.clone(), choices: choices.clone() },
        session_id, now).await?;
    let (rtx, rrx) = oneshot::channel::<Answer>();
    ui.send(AgentUpdate::AskUser { question, choices, reply: rtx }).await;
    let ans = rrx.await;
    let output = match ans {
        Ok(Answer::Choice(s) | Answer::FreeText(s)) => s,
        Ok(Answer::LetYouDecide) => "[let you decide]".into(),
        Err(_) => "cancelled".into(),
    };
    // Emit QuestionAnswered + the ToolResult (the answer is the tool result too).
    emit(&session, &mut events, ui, &config.branch,
        EventKind::QuestionAnswered { id: tc.id.clone(), answer: output.clone() },
        session_id, now).await?;
    emit(&session, &mut events, ui, &config.branch,
        EventKind::ToolResult { id: tc.id, name: tc.name, output, is_error: output == "cancelled" },
        session_id, now).await?;
}
```

### 5.3 Key changes from today

- **`QuestionAsked` is emitted before parking** — the card renders immediately. The unanswered state is in the event log, not transient UI state. This is the load-bearing change: the question is persisted the instant it's asked.
- **`QuestionAnswered` is emitted on reply** — the card collapses.
- **`ToolResult` is still emitted** — the answer is the tool result fed back to the model. The provider sees the user's answer as the tool's output, same as today.
- **`apply_mode_mapping` is `ToolKind::Interactive` now**, not `Approving`. The loop intercepts it by name alongside `ask_user`.
- **`AgentUpdate::ModeMappingApproval` is deleted.** The wizard rides `AgentUpdate::AskUser`. The `mapping` rides the `QuestionAsked` event (in `QuestionKind::ModeMapping`), not the `AgentUpdate`. The bin reads the mapping from the latest unanswered `QuestionAsked` event in `app.events` on answer — it's already in the log.

### 5.4 The bin's side effect (wizard materialize)

The bin's UI handler (`main.rs`) receives `AgentUpdate::AskUser`, raises the question state on `ShellState`, and parks. When the user answers, `answer_question` runs:

- For `kind == QuestionKind::Ask` → no-op (just sends the reply).
- For `kind == QuestionKind::ModeMapping` and answer == "Approve" → run the materializer + reload + clear wizard (the same logic as today, keyed off the `QuestionKind` from the event log instead of the deleted `ModeMappingApproval`).
- For `kind == QuestionKind::ModeMapping` and answer == "Reject" → send the reply, no materialize.
- For `kind == QuestionKind::ModeMapping` and answer == "Adjust" → send the reply, the model gets "Adjust" as the tool result and is expected to re-propose.

The bin reads `kind` from the latest unanswered `QuestionAsked` event in `app.events` (find by `id` matching the `AgentUpdate::AskUser`'s reply channel, or just the last unanswered `QuestionAsked` in the log — there's only ever one open at a time).

---

## 6. Render layer

### 6.1 Where the card lives

`build_conversation` (`crates/zoid-tui/src/chat.rs:75`) walks a projected `&[ChatMsg]` slice (produced by `zoid_core::projection::conversation` from `app.events`) and renders `Vec<Line<'static>>` into `App.body_cache`. Today it handles `User` / `Assistant` / `ToolResult` / `Delegated`. We add one more render target: the **question card**. (Note: `ChatMsg` is defined in `crates/zoid-core/src/projection.rs:20`, imported into `chat.rs` — the new `Question` variant is added there, not in `chat.rs`.)

The card renders at the position where `QuestionAsked` appears in the event log — inline between the assistant turn that called the tool and the `ToolResult` that carries the answer. This is the natural reading order: the question sits where the model asked it.

### 6.2 New `ChatMsg` variant

In `crates/zoid-core/src/projection.rs` (where `ChatMsg` lives, `projection.rs:20`):

```rust
enum ChatMsg {
    User,
    Assistant,
    ToolResult,
    Delegated,
    Question(QuestionCardState),  // NEW
}

/// What the card renders as. The projection decides Open vs Answered from the
/// event log; `build_conversation` fills the live cursor in for Open cards
/// (from `ShellState.question`, which the projection can't see).
enum QuestionCardState {
    /// No matching `QuestionAnswered` yet — the card is live (captures input).
    /// `selected`/`free_text` are placeholder defaults from the projection;
    /// `build_conversation` overwrites them with `ShellState.question`'s live
    /// cursor before rendering.
    Open { selected: usize, free_text: String },
    /// `QuestionAnswered` has landed — the card is collapsed to a one-line summary.
    Answered { answer: String },
}
```

`Question` is rendered by `build_conversation` from a pair of events: a `QuestionAsked` and its (optional) matching `QuestionAnswered`. The `Open` state's `selected` / `free_text` come from `ShellState.question` (the live highlight state) — not from the event log (the log only stores the question text + choices; the highlight cursor is transient UI state).

### 6.3 How the projection folds the card

The folding from `EventKind` to `ChatMsg` happens in `zoid_core::projection::conversation` (the function that produces the `&[ChatMsg]` slice `build_conversation` consumes). Walking the event log left-to-right, when it hits `EventKind::QuestionAsked { id, kind, question, choices }`:

1. **Look ahead** for a later `EventKind::QuestionAnswered { id: same }`.
   - Found → render `Question(Answered { answer })`.
   - Not found → this is the open card. Render `Question(Open { selected, free_text })` where `selected`/`free_text` come from `ShellState.question` (the live state). If `ShellState.question` is `None` (no question live — defensive default), use `selected = 0`, `free_text = ""`.

2. Emit the card's lines, then continue the walk. The matching `QuestionAnswered` is consumed here (don't render it as a separate `ChatMsg` — it's folded into the card).

3. The `ToolResult` for the same `id` is **suppressed** (see §6.6) — the card is the human-facing record; `ToolResult` stays in the log for projections/compaction/replay but is hidden from the conversation view.

### 6.4 Card rendering

A helper `render_question_card` produces `Vec<Line<'static>>`:

**Open state** (`QuestionKind::Ask`, free-text + 2 choices):
```
┌─ Question ──────────────────────────────────────────────
│ How should I handle the error?
│
│   ○ Retry
│   ● Skip        ← highlighted (↑↓ to move, Enter to pick)
│
│ Type your answer, or pick above. Enter to submit.
└─────────────────────────────────────────────────────────
```

**Open state** (`QuestionKind::ModeMapping`, the approval gate):
```
┌─ Mode mapping — review ─────────────────────────────────
│ skill: brainstorming
│   → skills/brainstorming/SKILL.md
│   → skills/brainstorming/references/*.md (3 files)
│ skill: writing-plans
│   → skills/writing-plans/SKILL.md
│   → skills/writing-plans/references/*.md (2 files)
│
│   ○ Approve
│   ● Reject
│   ○ Adjust
│
│ ↑↓ to move, Enter to approve. Esc to cancel.
└─────────────────────────────────────────────────────────
```

The `question` field for `ModeMapping` carries the `detailed_approval_summary` output (already newline-aware via the fix in `22f2b15`). The card splits on `\n` and prefixes each line with `│ `.

**Answered state** (collapsed, both kinds):
```
┌─ Question ──────────────────────────────────────────────
│ How should I handle the error?
└ ► Skip
```

or for the wizard:
```
┌─ Mode mapping — review ─────────────────────────────────
│ (14 files across 4 skills — summary above collapsed)
└ ► Approve
```

The answered card shows the question title + the answer on the last line. The full mapping detail is no longer re-rendered once answered (keeps the scrollback readable).

### 6.5 `ShellState.question` — non-modal state

`QuestionState` (`crates/zoid-tui/src/question.rs:17`) stays, but its role changes: it's **no longer modal**. It holds the live `selected` / `free_text` / `mode` while a question is open, and `build_conversation` reads from it to render the open card's cursor. When no question is open, it's `None`.

- `ShellState.question: Option<QuestionState>` — the live highlight state for the currently-open card, or `None`.
- Set when `AgentUpdate::AskUser` arrives (the bin calls `state.question = Some(...)`).
- Cleared when the user answers (the bin calls `answer_question`, which sends the reply and sets `state.question = None`).

The card's **appearance** (open vs answered) comes from the event log; the card's **cursor** (which choice is highlighted, what's typed so far) comes from `ShellState.question`. This split keeps the log as the source of truth for "what was asked/answered" while letting the cursor move without touching the log.

### 6.6 Input routing (soft-capture)

Today `Overlay::Question` is a modal overlay that captures keys in `route.rs`. With the card inline, there's no overlay — but the open card still needs to capture input. We use **soft-capture**:

While `ShellState.question.is_some()`:
- Typing (printable chars, backspace) → appends to `state.question.free_text` (not to the message textarea).
- ↑↓ → moves `state.question.selected`.
- Enter → submits: if `free_text` non-empty → `Answer::FreeText(free_text)`; else → `Answer::Choice(choices[selected])`.
- Esc → `Answer::FreeText("cancelled")` (or a dedicated `Answer::Cancelled` — same effect).
- Other keys (e.g. `/`, `:`) → swallowed while a question is open (the user is answering a question, not typing a command).

This is a branch at the top of `route.rs`'s key handler: `if let Some(q) = &state.question { return route_question(q, key); }`. The message textarea is **not** focused while a question is open — typing goes to the card. This matches the "card captures input" decision.

**Rejected alternative: hard-capture via overlay.** Keep an `Overlay::Question` variant that routes keys to the card but renders nothing (the card is in the body). Functionally identical but reuses the existing overlay routing — more machinery for no gain.

### 6.7 Body cache invalidation

`App.body_cache` is rebuilt when the event log grows. The card adds two triggers:
- `QuestionAsked` / `QuestionAnswered` events → log grew → cache rebuilt (already covered).
- Cursor moves (`selected`/`free_text` change) → **not** an event-log change, so the cache wouldn't rebuild. We need a second trigger: any mutation to `ShellState.question` marks `body_cache = None` (rebuild on next render).

This is a one-liner in the bin: every place that mutates `state.question` (set on `AskUser`, clear on answer, mutate on ↑↓/typing) also sets `app.body_cache = None`. Cheap and correct.

### 6.8 Suppress the `ToolResult` line

When the card is answered, the `ToolResult` event for the same `id` lands right after `QuestionAnswered`. Today `build_conversation` renders it as a `ToolResult` line (e.g. `↳ ask_user: "Skip"`). With the card already showing `└ ► Skip`, the `ToolResult` line is redundant.

**Decision: suppress `ToolResult` lines whose `id` matches a `QuestionAsked`.** The card is the human-facing record; `ToolResult` stays in the log (for projections/compaction/replay) but is hidden from the conversation view. Keeps the scrollback clean — one card, not a card + a duplicate tool line.

Implementation: `build_conversation` collects the set of `id`s that have a `QuestionAsked` event upstream, and skips `ToolResult` events whose `id` is in that set.

---

## 7. Deletion plan

Once the inline card + `QuestionAsked`/`QuestionAnswered` land, these become dead code:

### `zoid-tools/src/lib.rs`
- Delete `ToolKind::Approving` variant.
- `apply_mode_mapping` tool registration changes `kind: ToolKind::Approving` → `kind: ToolKind::Interactive`.

### `zoid/src/agent.rs`
- Delete `AgentUpdate::ModeMappingApproval { mapping, summary, reply }` variant.
- Delete the `Some(ToolKind::Approving) if tc.name == "apply_mode_mapping"` dispatch arm.
- The `ask_user` arm's `ToolKind::Interactive if tc.name == "ask_user"` guard expands to `tc.name == "ask_user" || tc.name == "apply_mode_mapping"` (§5.2).

### `zoid-tui/src/state.rs`
- Delete `Overlay::Question` variant (no overlay is raised for questions anymore).
- Remove the `Overlay::Question` arm from any `match Overlay` sites (render, route).

### `zoid-tui/src/render.rs`
- Delete `render_question` (the overlay renderer) at `render.rs:1086`.
- Remove the `Overlay::Question` arm from `render_overlay`'s match.

### `zoid-tui/src/route.rs`
- Delete the `Overlay::Question` arm from `route_overlay`.
- Add the soft-capture branch at the top of the main key handler (§6.6).

### `zoid/src/main.rs`
- Delete `App.pending_mode_mapping: Option<ModeMapping>` (the mapping now rides the `QuestionAsked` event in `app.events`; the bin reads it from there on answer).
- The `answer_question` path for the wizard reads `kind` from the latest unanswered `QuestionAsked` in `app.events` instead of `pending_mode_mapping`.

### Deletion order (to keep the tree compiling at each step)
1. Add `EventKind::QuestionAsked`/`QuestionAnswered` + `QuestionKind` (additive, compiles).
2. Add `Question` `ChatMsg` variant + `render_question_card` (additive, compiles).
3. Switch the agent loop to emit `QuestionAsked`/`QuestionAnswered` + unify the two arms (still emits the old `AgentUpdate`s alongside, temporarily — both paths live).
4. Switch the bin to read `kind` from the event log; route wizard answers via `AskUser` reply.
5. Switch `apply_mode_mapping` to `ToolKind::Interactive`.
6. **Now delete**: `ToolKind::Approving`, `AgentUpdate::ModeMappingApproval`, the old `Approving` arm, `Overlay::Question`, `render_question`, `pending_mode_mapping`. All call sites gone — safe deletion.
7. Add the soft-capture branch in `route.rs` (replaces the deleted overlay routing).

Steps 1–2 are additive. Step 3 is the bridge (emits both old + new). Steps 4–5 migrate consumers. Step 6 deletes. Step 7 wires the new input path. Each step compiles.

---

## 8. Testing + migration

### 8.1 Exhaustive match arms (compile-enforced)

Two sites match `EventKind` exhaustively and will fail to compile without new arms:

- `crates/zoid-core/src/projection.rs` — add `QuestionAsked`/`QuestionAnswered` arms to the exhaustive `match EventKind` (lines ~100–171, no wildcard; a new variant is a compile error). The projection folds these into `ChatMsg::Question` (§6.2–6.3). Arms are not no-ops — they produce the `QuestionCardState`.
- `crates/zoid-core/src/compaction.rs` — its three `match &e.kind` blocks (lines ~78, ~105, ~128) all use `_ => None` / `_ => {}` **wildcard arms**, so they are not exhaustive and will not force new arms. That said, the new variants should be **explicitly preserved** (copy through) by adding named arms anyway — otherwise a `QuestionAsked`/`QuestionAnswered` pair could be silently dropped or miscompacted by the wildcard. Recommendation: add explicit `QuestionAsked | QuestionAnswered => Some(e.clone())` (or equivalent) to the preserve-block so the card survives compaction and old conversations still show their Q&A inline. The wildcard stays for forward-compat with future variants.

### 8.2 Old session migration

Sessions persisted before this change have `ToolResult` events for `ask_user`/`apply_mode_mapping` but no `QuestionAsked`/`QuestionAnswered`. Loading an old session:

- `build_conversation` walks the log. For `ask_user`'s `ToolResult`, it renders the tool-result line as today (the old behavior). No card — the question text was never persisted (it lived in transient `QuestionState`), so there's nothing to render as a card.
- This is the correct degradation: old sessions look the same as they do today; new sessions get cards. No migration script needed.

The one wrinkle: old sessions that have a `ToolResult` for `apply_mode_mapping` *do* have the mapping in the tool args (not in the result) — but we don't render tool args in `build_conversation` today, so there's nothing to recover. Old sessions show `↳ apply_mode_mapping: "Approve"` as a tool line. Fine.

### 8.3 Tests

**Unit (zoid-core):**
- `EventKind::QuestionAsked`/`QuestionAnswered` round-trip through serde (schema compat).
- `QuestionKind::Ask` / `QuestionKind::ModeMapping { mapping }` serde round-trip (the `ModeMapping` serde is already tested in `wizard.rs`).
- `classify_update` (if it touches `QuestionKind`) — likely unaffected; it's about update classification, not question kinds.

**Unit (zoid-tui):**
- `build_conversation` with a log containing `QuestionAsked` only → renders an `Open` card with default cursor.
- `build_conversation` with `QuestionAsked` + matching `QuestionAnswered` → renders an `Answered` card, suppresses the matching `ToolResult` line.
- `build_conversation` with `QuestionAsked` + `QuestionAnswered` + a *different* `ToolResult` → the unrelated `ToolResult` still renders.
- `render_question_card` for `Ask` (free-text only, choices only, both).
- `render_question_card` for `ModeMapping` (long mapping, multiline `question` field).
- `render_question_card` `Answered` state (both kinds).
- `route_question`: ↑↓ moves `selected`, wrapping at bounds; printable → `free_text`; backspace; Enter with empty `free_text` → `Choice`; Enter with non-empty → `FreeText`; Esc → cancelled.

**Integration (zoid):**
- `ask_user` end-to-end: agent calls `ask_user`, card appears inline, user types + Enter, answer lands as `ToolResult`, agent continues. Assert `QuestionAsked` + `QuestionAnswered` + `ToolResult` in the event log.
- `apply_mode_mapping` end-to-end: agent calls `apply_mode_mapping`, card appears with mapping review, user picks Approve, materializer runs, mode reloads, `ToolResult` carries "Approve". Assert events + that the mode dir exists.
- Cancel path: Esc on an open card → `QuestionAnswered { answer: "cancelled" }`, `ToolResult { is_error: true }`, agent sees the cancellation.
- Old-session load: a pre-change event log renders without panicking (no card, tool-result line as before).

**Smoke (manual, per existing runbook):**
- Tier-2 plumbing: already PASS for the wizard; rerun after the refactor to confirm no regression.
- Tier-2 behavioral: still PENDING; the inline card doesn't change the provider path, so this remains independent.

### 8.4 Migration risk

Low. The changes are additive to the event schema (new variants, no removed ones), so old sessions load. The deletions are internal (enum variants, render functions) with no on-disk footprint. The one behavior change — `apply_mode_mapping` moving from `Approving` to `Interactive` — is invisible to the provider (the tool's schema doesn't change; only the client-side `kind` tag moves). No provider-facing breaking change.

---

## 9. Open questions

None at present. Decisions captured:
- Inline card for both `ask_user` and `apply_mode_mapping` (one mechanism, not two).
- New `EventKind::QuestionAsked`/`QuestionAnswered` (explicit, self-documenting, natural unanswered-card state).
- Typed `QuestionKind` enum (`Ask` | `ModeMapping { mapping }`).
- Card captures input (typing = answer, not message to model).
- Card renders in body cache (approach A — rebuild on cursor move is human-paced, same cost as a turn render).
- Delete `ToolKind::Approving` and `AgentUpdate::ModeMappingApproval` entirely — wizard rides `QuestionAsked`/`QuestionAnswered`.
- Materializer runs as side effect of `QuestionAnswered` handler when `kind == ModeMapping` and answer == "Approve".
- `ask_user`'s `ToolKind::Interactive` stays; `apply_mode_mapping` changes from `Approving` to `Interactive`.
- Soft-capture input routing (no overlay; branch at top of `route.rs`).
- Suppress `ToolResult` lines whose `id` matches a `QuestionAsked`.

---

## 10. File impact summary

| File | Change |
|---|---|
| `crates/zoid-core/src/event.rs` | + `QuestionAsked` / `QuestionAnswered` variants, + `QuestionKind` enum |
| `crates/zoid-core/src/projection.rs` | + exhaustive-match arms (compile-enforced; fold into `ChatMsg::Question`) |
| `crates/zoid-core/src/compaction.rs` | + explicit preserve arms (not compile-enforced; wildcards exist — but added so the card survives compaction) |
| `crates/zoid-tools/src/lib.rs` | − `ToolKind::Approving`; `apply_mode_mapping` → `Interactive` |
| `crates/zoid/src/agent.rs` | − `AgentUpdate::ModeMappingApproval`; − `Approving` arm; unify `ask_user` + `apply_mode_mapping` arm; emit `QuestionAsked`/`QuestionAnswered` |
| `crates/zoid/src/main.rs` | − `pending_mode_mapping`; `answer_question` reads `kind` from event log; wizard materialize keyed off `QuestionKind` |
| `crates/zoid/src/mode_wizard.rs` | `ApplyModeMappingTool` `kind` → `Interactive` |
| `crates/zoid-tui/src/chat.rs` | + `ChatMsg::Question` handling in `build_conversation` (variant lives in `projection.rs`), + `render_question_card`, suppress matching `ToolResult` |
| `crates/zoid-tui/src/question.rs` | `QuestionState` stays (non-modal live state) |
| `crates/zoid-tui/src/render.rs` | − `render_question` overlay, − `Overlay::Question` arm |
| `crates/zoid-tui/src/state.rs` | − `Overlay::Question` variant |
| `crates/zoid-tui/src/route.rs` | − `Overlay::Question` arm; + soft-capture branch + `route_question` |