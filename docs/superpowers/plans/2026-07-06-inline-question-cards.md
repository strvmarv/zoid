# Inline Question Cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the modal `Overlay::Question` with an inline card rendered in the conversation body, and unify `ask_user` and `apply_mode_mapping`'s approval gate into one event-driven mechanism (`EventKind::QuestionAsked`/`QuestionAnswered` + a typed `QuestionKind`). The card renders at the position where the question was asked, persists in the event log, and captures keyboard input while open (typing = answer, ↑↓ = pick, Enter = submit, Esc = cancel). `ToolKind::Approving`, `AgentUpdate::ModeMappingApproval`, `Overlay::Question`, and `render_question` are deleted.

**Architecture:** New `EventKind` variants (`QuestionAsked`/`QuestionAnswered`) + a `QuestionKind` enum (`Ask` | `ModeMapping { mapping }`) live in `zoid-core/src/event.rs`. The projection (`zoid-core/src/projection.rs`) folds these into a new `ChatMsg::Question(QuestionCardState)` variant — `Open` (no matching `QuestionAnswered` yet) or `Answered` (collapsed). `build_conversation` in `zoid-tui/src/chat.rs` renders the card inline from the `ChatMsg::Question` item, reading the live cursor (`selected`/`free_text`) from `ShellState.question` (non-modal state, no overlay). Input routing adds a soft-capture branch at the top of `route_key`: while `ShellState.question.is_some()`, keys go to the card, not the message textarea. The agent loop collapses the two dispatch arms (`ask_user` + `apply_mode_mapping`) into one `ToolKind::Interactive` arm that emits `QuestionAsked` → parks → emits `QuestionAnswered` + `ToolResult`. The bin reads `QuestionKind` from the latest unanswered `QuestionAsked` in the event log to decide whether to run the materializer on "Approve".

**Tech Stack:** Rust 2021 workspace. `zoid-core` (pure: `event.rs`, `projection.rs`, `compaction.rs`), `zoid-tools` (`ToolKind`), `zoid` bin+lib (`agent.rs`, `main.rs`, `mode_wizard.rs`), `zoid-tui` (`chat.rs`, `question.rs`, `state.rs`, `render.rs`, `route.rs`). Tests via `cargo test`. No new deps.

## Global Constraints

- **`zoid-core` stays pure** — no `std::fs`, no network, no process. `QuestionKind` + the new `EventKind` variants are pure types with serde.
- **Additive schema first, then bridge, then migrate, then delete.** The plan follows the 7-step deletion order from the spec §7 so the tree compiles at every task. No big-bang refactor.
- **The model never sees `QuestionAsked`/`QuestionAnswered`.** Those are UI/persistence concerns. The model-facing contract stays `ToolResult` (the answer is the tool's output). No tool schema changes, no provider-facing breaking change.
- **Old sessions load unchanged.** A pre-change event log has no `QuestionAsked`/`QuestionAnswered` and renders as today (tool-result line, no card). No migration script.
- **One question open at a time.** Same as today. The soft-capture branch assumes `ShellState.question.is_some()` means exactly one card is live.
- **No `Co-Authored-By` / co-author trailer** in commit messages (repo rule).
- **No inline comments unless asked** (repo rule).
- **Per task:** `cargo test --workspace` green, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo fmt --all` clean. TDD (failing test first). Commit at the end of each task.

---

## File Structure

**Modified:**
- `crates/zoid-core/src/event.rs` — + `QuestionKind` enum, + `EventKind::QuestionAsked` / `EventKind::QuestionAnswered` variants.
- `crates/zoid-core/src/projection.rs` — + `ChatMsg::Question(QuestionCardState)` variant, + `QuestionCardState` enum, fold `QuestionAsked`/`QuestionAnswered` into `ChatMsg::Question` in `conversation()`, suppress `ToolResult` lines whose `id` matches a `QuestionAsked`.
- `crates/zoid-core/src/compaction.rs` — + explicit preserve arms for `QuestionAsked`/`QuestionAnswered` (not compile-enforced — wildcards exist — but added so the card survives compaction).
- `crates/zoid-tools/src/lib.rs` — − `ToolKind::Approving` variant.
- `crates/zoid/src/agent.rs` — − `AgentUpdate::ModeMappingApproval`; − `ToolKind::Approving` dispatch arm; unify `ask_user` + `apply_mode_mapping` into one `ToolKind::Interactive` arm that emits `QuestionAsked`/`QuestionAnswered`; `map_msg` gets a `ChatMsg::Question` arm.
- `crates/zoid/src/mode_wizard.rs` — `ApplyModeMappingTool::kind()` returns `ToolKind::Interactive` (was `Approving`); update the doc comment.
- `crates/zoid/src/main.rs` — − `pending_mode_mapping`; `AskUser` UI handler reads `QuestionKind` from the latest unanswered `QuestionAsked` in `app.events`; `answer_question` materializes on "Approve" when `kind == ModeMapping`; − `ModeMappingApproval` UI handler arm; remove `Overlay::Question` raises; body-cache invalidation on `ShellState.question` mutation.
- `crates/zoid-tui/src/chat.rs` — + `render_question_card` helper; `build_conversation` handles `ChatMsg::Question` (renders the card inline, reading the live cursor via a callback).
- `crates/zoid-tui/src/state.rs` — − `Overlay::Question` variant; `ShellState.question` stays (non-modal live state).
- `crates/zoid-tui/src/render.rs` — − `render_question` function; − `Overlay::Question` arm in the overlay render match.
- `crates/zoid-tui/src/route.rs` — − `Overlay::Question` arm in `route_key`; − `Overlay::Question` short-circuit in `route_mouse`; + soft-capture branch at the top of `route_key` (while `state.question.is_some()`, route to `route_question_key`).

**Dependency order:** T1 (QuestionKind + EventKind variants, additive) → T2 (projection: ChatMsg::Question + fold, additive) → T3 (compaction preserve arms, additive) → T4 (agent loop: emit QuestionAsked/QuestionAnswered, bridge — still emits old AgentUpdates) → T5 (bin: read kind from event log, bridge) → T6 (mode_wizard: ToolKind::Interactive) → T7 (delete: ToolKind::Approving, AgentUpdate::ModeMappingApproval, Overlay::Question, render_question, pending_mode_mapping) → T8 (route: soft-capture + delete overlay arms) → T9 (chat: render_question_card inline) → T10 (integration tests).

---

## Task 1: `QuestionKind` + `EventKind` variants (additive)

**Files:**
- Modify: `crates/zoid-core/src/event.rs`

**Interfaces:**
- Produces: `QuestionKind` enum, `EventKind::QuestionAsked`, `EventKind::QuestionAnswered`.

- [ ] **Step 1: Write the failing serde round-trip tests**

In `crates/zoid-core/src/event.rs`, append to the `#[cfg(test)] mod tests` block (after the last test, before the closing `}`):

```rust
    #[test]
    fn question_ask_round_trips() {
        use crate::wizard::ModeMapping;
        let k = EventKind::QuestionAsked {
            id: "call_1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }

    #[test]
    fn question_ask_mode_mapping_round_trips() {
        use crate::wizard::{MappingEntry, ModeMapping};
        let mapping = ModeMapping {
            mode_name: "brainstorm".into(),
            mode_description: "brainstorming mode".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "skills/brainstorming/SKILL.md".into(),
                source: "upstream/SKILL.md".into(),
                summary: "the skill".into(),
            }],
        };
        let k = EventKind::QuestionAsked {
            id: "call_2".into(),
            kind: QuestionKind::ModeMapping {
                mapping: Box::new(mapping.clone()),
            },
            question: "review the mapping".into(),
            choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
        // Confirm the mapping survived the box + serde round-trip.
        match back.kind {
            EventKind::QuestionAsked {
                kind: QuestionKind::ModeMapping { mapping: m },
                ..
            } => assert_eq!(*m, mapping),
            _ => panic!("expected ModeMapping kind"),
        }
    }

    #[test]
    fn question_answered_round_trips() {
        let k = EventKind::QuestionAnswered {
            id: "call_1".into(),
            answer: "Skip".into(),
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core event::tests::question_ask_round_trips event::tests::question_ask_mode_mapping_round_trips event::tests::question_answered_round_trips`
Expected: FAIL — `QuestionKind` / `QuestionAsked` / `QuestionAnswered` not found.

- [ ] **Step 3: Add `QuestionKind` and the new `EventKind` variants**

In `crates/zoid-core/src/event.rs`, add `QuestionKind` after the `EvictionMarker` struct (line 39, before `pub enum EventKind`):

```rust
/// What kind of question an inline card represents. Drives rendering + the
/// bin's side-effect on answer (materialize for `ModeMapping` + "Approve").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionKind {
    /// A plain `ask_user` question (free-text or pick-list).
    Ask,
    /// The wizard's mode-mapping approval. The `mapping` rides here so the bin
    /// can materialize on "Approve" without re-parsing it from anywhere.
    ModeMapping {
        mapping: Box<crate::wizard::ModeMapping>,
    },
}
```

Then add the two new variants to `EventKind` (insert after `TurnsReadmitted { .. }` at line 119, before the closing `}` of the enum):

```rust
    /// A question the model asked the user via `ask_user` (or `apply_mode_mapping`'s
    /// approval gate). Rendered as an inline card in the conversation. Paired
    /// with a `QuestionAnswered` carrying the same `id`.
    QuestionAsked {
        id: String,
        kind: QuestionKind,
        question: String,
        choices: Vec<String>,
    },
    /// The user's answer to a `QuestionAsked`. `id` matches the question. The
    /// card collapses to a one-line summary after this lands.
    QuestionAnswered {
        id: String,
        answer: String,
    },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core event::tests`
Expected: PASS — all event tests green, including the three new ones.

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo clippy -p zoid-core --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid-core`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/event.rs
git commit -m "feat(core): add QuestionKind + QuestionAsked/QuestionAnswered events"
```

---

## Task 2: Projection folds the card (additive)

**Files:**
- Modify: `crates/zoid-core/src/projection.rs`

**Interfaces:**
- Produces: `ChatMsg::Question(QuestionCardState)`, `QuestionCardState` enum.
- `conversation()` now folds `QuestionAsked` (+ matching `QuestionAnswered`) into `ChatMsg::Question` and suppresses `ToolResult` lines whose `id` matches a `QuestionAsked`.

- [ ] **Step 1: Write the failing projection tests**

In `crates/zoid-core/src/projection.rs`, append to the `#[cfg(test)] mod tests` block:

```rust
    use crate::event::QuestionKind;
    use crate::wizard::{MappingEntry, ModeMapping};

    fn q_asked(id: u128, qid: &str, question: &str, choices: &[&str]) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::QuestionAsked {
                id: qid.into(),
                kind: QuestionKind::Ask,
                question: question.into(),
                choices: choices.iter().map(|s| s.to_string()).collect(),
            },
        )
    }

    fn q_answered(id: u128, qid: &str, answer: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::QuestionAnswered {
                id: qid.into(),
                answer: answer.into(),
            },
        )
    }

    #[test]
    fn question_asked_alone_folds_to_open_card() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry or skip?", &["Retry", "Skip"]),
        ];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question { state: QuestionCardState::Open { .. }, .. })
        ));
    }

    #[test]
    fn question_asked_then_answered_folds_to_answered_card() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry or skip?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
        ];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question { state: QuestionCardState::Answered { answer }, .. }) if answer == "Skip"
        ));
    }

    #[test]
    fn tool_result_matching_question_asked_is_suppressed() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
            Event::new(
                Ulid::from(4u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "c1".into(),
                    name: "ask_user".into(),
                    output: "Skip".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        // No ToolResult ChatMsg whose id == "c1" survives — the card owns it.
        assert!(
            !conv.iter().any(|m| matches!(m, ChatMsg::ToolResult { id, .. } if id == "c1")),
            "ToolResult matching a QuestionAsked must be suppressed"
        );
    }

    #[test]
    fn unrelated_tool_result_still_renders() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
            Event::new(
                Ulid::from(4u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "c2".into(),
                    name: "read_file".into(),
                    output: "data".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        assert!(
            conv.iter().any(|m| matches!(m, ChatMsg::ToolResult { id, .. } if id == "c2")),
            "unrelated ToolResult must still render"
        );
    }

    #[test]
    fn mode_mapping_question_folds_to_open_card() {
        let mapping = ModeMapping {
            mode_name: "brainstorm".into(),
            mode_description: "".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "skills/brainstorming/SKILL.md".into(),
                source: "up/SKILL.md".into(),
                summary: "skill".into(),
            }],
        };
        let events = vec![Event::new(
            Ulid::from(1u128),
            None,
            0,
            EventKind::QuestionAsked {
                id: "c1".into(),
                kind: QuestionKind::ModeMapping {
                    mapping: Box::new(mapping),
                },
                question: "review".into(),
                choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
            },
        )];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question { state: QuestionCardState::Open { .. }, .. })
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core projection::tests`
Expected: FAIL — `ChatMsg::Question` / `QuestionCardState` not found.

- [ ] **Step 3: Add `ChatMsg::Question` + `QuestionCardState` and fold the events**

In `crates/zoid-core/src/projection.rs`, add the `Question` variant to `ChatMsg` (after `Delegated`, before the closing `}`):

```rust
    /// An inline question card (from `EventKind::QuestionAsked` + optional
    /// matching `QuestionAnswered`). `Open` means no answer yet — the card is
    /// live and captures input; `build_conversation` fills the cursor from
    /// `ShellState.question` at render time. `Answered` means the card has
    /// collapsed to a one-line summary.
    Question {
        id: String,
        kind: crate::event::QuestionKind,
        question: String,
        choices: Vec<String>,
        state: QuestionCardState,
        ts: i64,
    },
```

Add the `QuestionCardState` enum after `ChatMsg` (before `pub fn conversation`):

```rust
/// What the card renders as. The projection decides Open vs Answered from the
/// event log; `build_conversation` overwrites the `Open` cursor with
/// `ShellState.question`'s live values before rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionCardState {
    /// No matching `QuestionAnswered` yet — the card is live (captures input).
    /// `selected`/`free_text` are placeholder defaults from the projection;
    /// `build_conversation` overwrites them with `ShellState.question`'s live
    /// cursor before rendering.
    Open { selected: usize, free_text: String },
    /// `QuestionAnswered` has landed — the card is collapsed to a one-line summary.
    Answered { answer: String },
}
```

Now modify `conversation()` to fold `QuestionAsked`/`QuestionAnswered` and suppress matching `ToolResult`s. Replace the function body (lines 57–175) with:

```rust
pub fn conversation<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<ChatMsg> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let evicted = crate::eviction::evicted_ids(events.iter().copied());

    // ACM-1: a tool-result whose id has a later ToolResultCompacted is emitted
    // as its summary (last write wins), both to the live request and the view.
    let mut compacted: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for e in visible {
        if let EventKind::ToolResultCompacted { id, summary, .. } = &e.kind {
            compacted.insert(id.as_str(), summary.as_str());
        }
    }

    // Pair QuestionAsked → QuestionAnswered by id, and record which tool-result
    // ids belong to a question (so they can be suppressed from the view — the
    // card is the human-facing record; the ToolResult stays in the log for the
    // model/projections/compaction but is hidden from the conversation).
    let mut answered: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut question_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in visible {
        match &e.kind {
            EventKind::QuestionAsked { id, .. } => {
                question_ids.insert(id.as_str());
            }
            EventKind::QuestionAnswered { id, answer } => {
                answered.insert(id.as_str(), answer.as_str());
            }
            _ => {}
        }
    }

    let mut out: Vec<ChatMsg> = Vec::new();
    let mut text: Option<String> = None;
    let mut calls: Vec<ToolCallRef> = Vec::new();
    // ts of the first event that contributed to the in-progress assistant turn.
    let mut turn_ts: Option<i64> = None;

    fn flush(
        text: &mut Option<String>,
        calls: &mut Vec<ToolCallRef>,
        turn_ts: &mut Option<i64>,
        out: &mut Vec<ChatMsg>,
    ) {
        if text.is_some() || !calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: text.take().unwrap_or_default(),
                tool_calls: std::mem::take(calls),
                ts: turn_ts.take().unwrap_or(0),
            });
        }
        *turn_ts = None;
    }

    for e in visible {
        if evicted.contains(&e.id) {
            continue;
        }
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            EventKind::UserMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::User {
                    text: t.clone(),
                    ts: e.ts,
                });
            }
            EventKind::AssistantMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::Assistant {
                    text: t.clone(),
                    tool_calls: Vec::new(),
                    ts: e.ts,
                });
            }
            EventKind::ModelDelta { text: t } => {
                turn_ts.get_or_insert(e.ts);
                text.get_or_insert_with(String::new).push_str(t);
            }
            EventKind::ToolCall { id, name, args } => {
                turn_ts.get_or_insert(e.ts);
                calls.push(ToolCallRef {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
            }
            EventKind::ToolResult {
                id,
                name,
                output,
                is_error,
            } => {
                // Suppress the tool-result line when a QuestionAsked owns this
                // id — the card is the human-facing record.
                if question_ids.contains(id.as_str()) {
                    // The assistant turn that made the call(s) still ends here.
                    flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                    continue;
                }
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                let (output, was_compacted) = match compacted.get(id.as_str()) {
                    Some(sum) => ((*sum).to_string(), true),
                    None => (output.clone(), false),
                };
                out.push(ChatMsg::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    output,
                    is_error: *is_error,
                    compacted: was_compacted,
                    ts: e.ts,
                });
            }
            EventKind::QuestionAsked {
                id,
                kind,
                question,
                choices,
            } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                let state = match answered.get(id.as_str()) {
                    Some(ans) => QuestionCardState::Answered {
                        answer: (*ans).to_string(),
                    },
                    None => QuestionCardState::Open {
                        selected: 0,
                        free_text: String::new(),
                    },
                };
                out.push(ChatMsg::Question {
                    id: id.clone(),
                    kind: kind.clone(),
                    question: question.clone(),
                    choices: choices.clone(),
                    state,
                    ts: e.ts,
                });
            }
            EventKind::QuestionAnswered { .. } => {
                // Folded into the matching QuestionAsked card above; not a
                // standalone conversation item.
            }
            EventKind::DelegationResult { summary, ok, .. } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::Delegated {
                    summary: summary.clone(),
                    ok: *ok,
                });
            }
            EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. } => {
                // Economy bookkeeping; folded elsewhere, not a raw conversation item.
            }
            EventKind::Tasks { .. } => {
                // Rail-only snapshot; never inlined into the conversation transcript.
            }
            EventKind::TurnsDropped { .. } => {
                // Metadata marker; not a conversation item.
            }
            EventKind::TurnsEvicted { .. } | EventKind::TurnsReadmitted { .. } => {
                // Metadata marker; not a conversation item. (Out of scope: rendering
                // the in-context breadcrumb / recall filtering is a later slice.)
            }
        }
    }
    flush(&mut text, &mut calls, &mut turn_ts, &mut out);
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core projection::tests`
Expected: PASS — all projection tests green, including the five new ones.

- [ ] **Step 5: Run clippy + fmt + full workspace test**

Run: `cargo clippy -p zoid-core --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid-core`
Run: `cargo test --workspace`
Expected: the workspace tests will FAIL to compile in `zoid/src/agent.rs` (`map_msg` doesn't handle `ChatMsg::Question`) and `zoid-tui/src/chat.rs` (`build_conversation` doesn't handle `ChatMsg::Question`). That's expected — those arms are added in Tasks 4 and 9. For now, only `cargo test -p zoid-core` must be green. Note this in the commit message.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/projection.rs
git commit -m "feat(core): fold QuestionAsked/QuestionAnswered into ChatMsg::Question

Adds the Question variant + QuestionCardState and the fold logic in
conversation(). Suppresses ToolResult lines whose id matches a QuestionAsked.
Workspace compile breaks downstream (agent.rs map_msg, chat.rs build_conversation)
— fixed in Tasks 4 and 9. zoid-core tests pass."
```

---

## Task 3: Compaction preserve arms (additive)

**Files:**
- Modify: `crates/zoid-core/src/compaction.rs`

**Why:** `compaction.rs`'s three `match &e.kind` blocks (lines ~78, ~105, ~128) use `_ =>` wildcards, so the new variants aren't compile-enforced. But without explicit arms, a `QuestionAsked`/`QuestionAnswered` pair could be silently dropped or miscompacted by the wildcard. We add explicit preserve arms so the card survives compaction and old conversations still show their Q&A inline.

- [ ] **Step 1: Read the three match sites to find the exact wildcard arms**

Run: `cargo doc -p zoid-core --no-deps` is not needed. Instead, open `crates/zoid-core/src/compaction.rs` and locate the three `match &e.kind` blocks (around lines 78, 105, 128). Each ends with a `_ => None` or `_ => {}` wildcard. Identify which block is the "preserve" decision (returns `Some` to keep an event, `None` to drop it).

- [ ] **Step 2: Add explicit preserve arms to the "preserve" match**

In the block that decides whether to preserve an event (the one returning `Option<&Event>` or similar — the first `match &e.kind` at ~line 78 that collects ids/decides retention), add explicit arms so `QuestionAsked`/`QuestionAnswered` are preserved:

```rust
            EventKind::QuestionAsked { .. } | EventKind::QuestionAnswered { .. } => {
                // The card is a paired record; preserve both halves so the
                // conversation view still renders the Q&A inline after compaction.
            }
```

Place the new arm immediately before the `_ => {}` (or `_ => None`) wildcard in the preserve-decision block. The arm body is empty if the surrounding block's pattern is "match-and-collect"; if the block returns `Some(e)` for preserved events, the arm must return `Some(e)` instead. Read the surrounding arms to match the exact shape.

- [ ] **Step 3: Verify the other two match blocks don't need arms**

The other two `match &e.kind` blocks (at ~105 and ~128) handle `ToolResult`-specific compaction (token estimation, id correlation). `QuestionAsked`/`QuestionAnswered` carry no tool-result body to compact, so they correctly fall through to the wildcard. No change needed there. Verify by reading the blocks.

- [ ] **Step 4: Run the compaction tests**

Run: `cargo test -p zoid-core compaction::tests`
Expected: PASS — no regressions.

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo clippy -p zoid-core --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid-core`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/compaction.rs
git commit -m "feat(core): preserve QuestionAsked/QuestionAnswered through compaction

Explicit arms in the preserve-decision match so the card pair survives
compaction and old conversations still render their Q&A inline. The other
two match blocks (token estimation, id correlation) correctly fall through
to the wildcard — questions carry no tool-result body to compact."
```

---

## Task 4: Agent loop emits QuestionAsked/QuestionAnswered (bridge)

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Why:** Bridge step — the agent loop now emits `QuestionAsked`/`QuestionAnswered` alongside the existing `AgentUpdate`s. The old `ModeMappingApproval` arm and the `AskUser` arm both gain `QuestionAsked`/`QuestionAnswered` emission. `map_msg` gets a `ChatMsg::Question` arm so the workspace compiles. This task does NOT delete anything — both old and new paths live.

- [ ] **Step 1: Add the `ChatMsg::Question` arm to `map_msg`**

In `crates/zoid/src/agent.rs`, in `fn map_msg` (lines 153–183), add an arm for `ChatMsg::Question` before the closing `}`:

```rust
        ChatMsg::Question { id, question, choices, state, .. } => {
            // The card is a UI/persistence concern — never sent to the model.
            // The model sees the answer as the ToolResult for the matching id.
            // Return an inert assistant message so the provider request stays
            // well-formed (the ToolResult for the same id is what the model
            // actually reads).
            let _ = (id, question, choices, state);
            Message {
                role: zoid_provider::MsgRole::Assistant,
                content: String::new(),
                tool_calls: vec![],
                tool_name: None,
                tool_call_id: None,
            }
        }
```

This is a placeholder — the card is never inlined into the provider request. The matching `ToolResult` (still emitted by the loop, suppressed only in the conversation *view*) is what the model reads. Returning an empty assistant message is harmless because the `ToolResult` for the same `id` carries the actual content.

- [ ] **Step 2: Emit `QuestionAsked` in the `ask_user` arm (before parking)**

In `crates/zoid/src/agent.rs`, in the `Some(zoid_tools::ToolKind::Interactive) if tc.name == "ask_user"` arm (starts at line 885), insert the `QuestionAsked` emission right after `choices` is parsed and before the `oneshot::channel` line. The arm currently reads (lines 885–901):

```rust
                Some(zoid_tools::ToolKind::Interactive) if tc.name == "ask_user" => {
                    let question = tc
                        .args
                        .get("question")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let choices = tc
                        .args
                        .get("choices")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.as_str().map(String::from))
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_default();
                    let (rtx, rrx) = oneshot::channel::<Answer>();
```

Insert after the `let choices = ...` block and before `let (rtx, rrx) = oneshot::channel::<Answer>();`:

```rust
                    // Emit QuestionAsked so the card renders inline immediately.
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAsked {
                            id: tc.id.clone(),
                            kind: zoid_core::event::QuestionKind::Ask,
                            question: question.clone(),
                            choices: choices.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
```

- [ ] **Step 3: Emit `QuestionAnswered` in the `ask_user` arm (on reply)**

In the same arm, after `let ans = rrx.await;` and before the existing `emit(... EventKind::ToolResult ...)` calls, insert the `QuestionAnswered` emission. The arm currently has two reply paths (Ok(ans) → emit ToolResult; Err(_) → emit "[user aborted]" ToolResult). For the `Ok` path, insert before the `emit(... ToolResult ...)`:

```rust
                    let output = match ans {
                        Ok(Answer::Choice(s) | Answer::FreeText(s)) => s,
                        Ok(Answer::LetYouDecide) => "[let you decide]".to_string(),
                        Err(_) => "[user aborted]".to_string(),
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAnswered {
                            id: tc.id.clone(),
                            answer: output.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    let is_error = output == "[user aborted]";
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output,
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = !is_error,
                        "tool executed"
                    );
```

This replaces the existing `match ans { Ok(ans) => { ... } Err(_) => { ... } }` block (lines 917–989) with a single unified path. The old Err-path's "drain remaining batched tool calls" loop (lines 970–989) must be preserved — keep it after the emit, guarded on `is_error`:

```rust
                    if is_error {
                        // Drain any remaining batched tool calls so none is left
                        // without a matching ToolResult (the provider's tool-call
                        // protocol requires every call to be answered before the
                        // next request).
                        for rest in pending_iter.by_ref() {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: rest.id,
                                    name: rest.name,
                                    output: "[skipped: turn aborted]".to_string(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                    }
                    break;
```

(The original used `break` to end the turn after draining; preserve that.)

- [ ] **Step 4: Emit `QuestionAsked`/`QuestionAnswered` in the `apply_mode_mapping` arm**

In the `Some(zoid_tools::ToolKind::Approving) if tc.name == "apply_mode_mapping"` arm (lines 820–884), insert `QuestionAsked` after the mapping is parsed and `QuestionAnswered` after the reply. The arm currently:

```rust
                Some(zoid_tools::ToolKind::Approving) if tc.name == "apply_mode_mapping" => {
                    let mapping = match crate::mode_wizard::parse_mapping_args(&tc.args) { ... };
                    let summary = crate::mode_wizard::approval_summary(&mapping);
                    let (rtx, rrx) = oneshot::channel::<String>();
                    let sent = ui.send(AgentUpdate::ModeMappingApproval { ... }).await;
                    if sent.is_err() { continue; }
                    let ans = rrx.await;
                    let output = match ans { Ok(d) => d, Err(_) => "approval cancelled".into() };
                    let is_error = ...;
                    emit(... EventKind::ToolResult { ... }).await?;
                }
```

Insert after `let summary = ...` (keep `summary` for the `AgentUpdate::ModeMappingApproval` — the bin still uses it during the bridge) and before `let (rtx, rrx) = ...`:

```rust
                    let detail = crate::mode_wizard::detailed_approval_summary(&mapping);
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAsked {
                            id: tc.id.clone(),
                            kind: zoid_core::event::QuestionKind::ModeMapping {
                                mapping: Box::new(mapping.clone()),
                            },
                            question: detail,
                            choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
                        },
                        session_id,
                        now,
                    )
                    .await?;
```

Then after `let ans = rrx.await;` and before the `ToolResult` emit, insert:

```rust
                    let output = match ans {
                        Ok(d) => d,
                        Err(_) => "approval cancelled".to_string(),
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAnswered {
                            id: tc.id.clone(),
                            answer: output.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    let is_error = output == "Reject" || output.starts_with("approval cancelled");
```

Keep the existing `emit(... EventKind::ToolResult { ... })` that follows (it now runs after `QuestionAnswered`).

- [ ] **Step 5: Verify the workspace compiles**

Run: `cargo build --workspace`
Expected: PASS — the bridge emits both old (`AgentUpdate::AskUser` / `ModeMappingApproval`) and new (`QuestionAsked`/`QuestionAnswered`) events. The bin still handles the old `AgentUpdate`s (Task 5 switches the bin to read from the event log, then Task 7 deletes the old paths).

- [ ] **Step 6: Run the agent tests**

Run: `cargo test -p zoid`
Expected: PASS — existing agent tests green. The new emissions are additive; no test asserts their absence.

- [ ] **Step 7: Run clippy + fmt**

Run: `cargo clippy -p zoid --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): emit QuestionAsked/QuestionAnswered (bridge)

The ask_user and apply_mode_mapping arms now emit QuestionAsked before
parking and QuestionAnswered on reply, alongside the existing AgentUpdate
emissions. map_msg handles ChatMsg::Question (inert — the card is never
sent to the model; the matching ToolResult carries the answer). This is
the bridge step: both old and new paths live. Tasks 5-7 migrate consumers
and delete the old paths."
```

---

## Task 5: Bin reads `QuestionKind` from the event log (bridge)

**Files:**
- Modify: `crates/zoid/src/main.rs`

**Why:** The bin's `AskUser` UI handler now reads `QuestionKind` from the latest unanswered `QuestionAsked` in `app.events` to decide whether to run the materializer on "Approve". `answer_question` materializes when `kind == ModeMapping` and answer == "Approve". The old `ModeMappingApproval` handler arm stays (deleted in Task 7). `pending_mode_mapping` stays (deleted in Task 7) but is no longer the source of truth for the mapping — the event log is.

- [ ] **Step 1: Add a helper to find the latest unanswered `QuestionAsked`**

In `crates/zoid/src/main.rs`, add a helper function near `answer_question` (before `fn answer_question` at line 2971):

```rust
/// Find the latest unanswered `QuestionAsked` in the event log. Returns the
/// `QuestionKind` (which carries the `ModeMapping` for wizard approvals) or
/// `None` if no question is open. Used by `answer_question` to decide whether
/// to run the materializer on "Approve".
fn latest_open_question(events: &[zoid_core::event::Event]) -> Option<&zoid_core::event::QuestionKind> {
    let mut asked: Option<&zoid_core::event::QuestionKind> = None;
    let mut asked_id: Option<&str> = None;
    for e in events {
        match &e.kind {
            zoid_core::event::EventKind::QuestionAsked { id, kind, .. } => {
                asked = Some(kind);
                asked_id = Some(id.as_str());
            }
            zoid_core::event::EventKind::QuestionAnswered { id, .. } => {
                if asked_id == Some(id.as_str()) {
                    asked = None;
                    asked_id = None;
                }
            }
            _ => {}
        }
    }
    asked
}
```

- [ ] **Step 2: Rewrite `answer_question` to read `kind` from the event log**

Replace `fn answer_question` (lines 2971–3045) with:

```rust
/// Send the user's answer down the `ask_user` reply channel and close the
/// question state. For a `ModeMapping` question answered "Approve", run the
/// materializer + reload + clear the wizard (same logic as the old
/// `ModeMappingApproval` path, now keyed off the `QuestionKind` from the
/// latest unanswered `QuestionAsked` in the event log). A no-op if the
/// channel was already consumed/dropped (e.g. a double-fire race).
fn answer_question(app: &mut App, ans: zoid::agent::Answer) {
    let kind = latest_open_question(&app.events).cloned();
    let is_wizard = matches!(
        kind,
        Some(zoid_core::event::QuestionKind::ModeMapping { .. })
    );

    // Resolve the answer to a string for the reply channel + QuestionAnswered
    // event (the agent loop emits QuestionAnswered from its side; the bin's
    // reply channel carries the same value).
    let answer_str = match &ans {
        zoid::agent::Answer::Choice(s) | zoid::agent::Answer::FreeText(s) => s.clone(),
        zoid::agent::Answer::LetYouDecide => "[let you decide]".into(),
    };

    if is_wizard {
        if let Some(zoid_core::event::QuestionKind::ModeMapping { mapping }) = kind {
            match ans {
                zoid::agent::Answer::Choice(c) if c == "Approve" => {
                    let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
                    let dest = cfg_dir
                        .join("modes")
                        .join(zoid::mode_wizard::slugify(&mapping.mode_name));
                    let scan = app
                        .wizard
                        .as_ref()
                        .expect("wizard open during approval")
                        .scan
                        .clone();
                    let fetched_at = chrono::Utc::now().to_rfc3339();
                    match zoid::mode_wizard::materialize(&mapping, &scan, &dest, &fetched_at) {
                        Ok(_) => {
                            let prev = app.modes.active_name().to_string();
                            app.modes = zoid::mode_import::build_mode_registry(
                                &app.base_profile,
                                &app.mode_dirs,
                            );
                            app.modes.set_active(&prev);
                            sync_mode_mirror(app);
                            app.wizard = None;
                            app.shell.status_hint = Some(format!(
                                "imported '{}' — Shift+Tab to it",
                                mapping.mode_name
                            ));
                        }
                        Err(e) => {
                            app.shell.status_hint = Some(format!(
                                "materialize failed: {}. Re-run :mode import to retry.",
                                e.problems.join("; ")
                            ));
                            app.wizard = None;
                            // Send "Reject" so the agent loop records an error result.
                            if let Some(tx) = app.pending_answer.take() {
                                let _ = tx.send(zoid::agent::Answer::Choice("Reject".into()));
                            }
                            app.shell.question = None;
                            return;
                        }
                    }
                }
                zoid::agent::Answer::Choice(c) if c == "Reject" => {
                    app.wizard = None;
                    app.shell.status_hint = Some("import cancelled".into());
                }
                zoid::agent::Answer::Choice(_) | zoid::agent::Answer::FreeText(_) => {
                    // "Adjust" or free-text: the model gets the text and re-proposes.
                    let text = match ans {
                        zoid::agent::Answer::Choice(s) | zoid::agent::Answer::FreeText(s) => s,
                        zoid::agent::Answer::LetYouDecide => "[let you decide]".into(),
                    };
                    let ts = now_ms();
                    let ev = zoid_core::event::Event::new(
                        ulid::Ulid::new(),
                        None,
                        ts,
                        zoid_core::event::EventKind::UserMessage { text },
                    );
                    app.pending_adjust = Some(ev);
                }
                zoid::agent::Answer::LetYouDecide => {
                    // Treat as Approve for the wizard (matches the old behavior).
                }
            }
        }
    }

    if let Some(tx) = app.pending_answer.take() {
        let _ = tx.send(ans);
    }
    app.shell.question = None;
}
```

Note: this removes the `pending_mode_mapping` read path. The old `pending_mode_mapping` field and the `ModeMappingApproval` UI handler arm (lines 1856–1866) are still present but now unused — they're deleted in Task 7. For the bridge to work, the `AskUser` handler (lines 1748–1761) must NOT raise `Overlay::Question` (the card is inline now), but that change is in Task 7 too. For now, keep the overlay raise — it's harmless because Task 7 removes it. The key bridge behavior: `answer_question` reads `kind` from the event log, not `pending_mode_mapping`.

Wait — the bridge needs the `AskUser` handler to stop raising the overlay so the inline card is visible. But Task 7 deletes the overlay. To keep the bridge compiling and behaving, this task only changes `answer_question`'s source of truth. The overlay still raises (redundant with the inline card) until Task 7 removes it. This is acceptable for a bridge step.

- [ ] **Step 3: Verify the workspace compiles**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 4: Run the bin tests**

Run: `cargo test -p zoid`
Expected: PASS — the `App` literal in tests still has `pending_mode_mapping: None` (deleted in Task 7). The new `answer_question` doesn't touch `pending_mode_mapping`.

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo clippy -p zoid --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid`
Expected: clean. If clippy warns about `answer_str` being unused (the agent loop emits `QuestionAnswered` from its side), prefix with `let _ = answer_str;` or remove the binding if unused.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): answer_question reads QuestionKind from event log (bridge)

The bin's answer_question now reads the QuestionKind from the latest
unanswered QuestionAsked in app.events (via latest_open_question) to decide
whether to run the materializer on 'Approve'. The old pending_mode_mapping
field and ModeMappingApproval handler arm are still present but unused —
deleted in Task 7. The overlay still raises (redundant with the inline card)
until Task 7 removes it."
```

---

## Task 6: `apply_mode_mapping` → `ToolKind::Interactive`

**Files:**
- Modify: `crates/zoid/src/mode_wizard.rs`

**Why:** `apply_mode_mapping` is no longer `Approving` — it rides the unified `Interactive` arm (intercepted by name alongside `ask_user`). This is the migration step: change the `kind()` return value and update the doc comment. `ToolKind::Approving` is deleted in Task 7 (after this task, nothing returns it).

- [ ] **Step 1: Change `ApplyModeMappingTool::kind()`**

In `crates/zoid/src/mode_wizard.rs`, at line 451–453, change:

```rust
    fn kind(&self) -> ToolKind {
        ToolKind::Approving
    }
```

to:

```rust
    fn kind(&self) -> ToolKind {
        ToolKind::Interactive
    }
```

- [ ] **Step 2: Update the doc comment on `ApplyModeMappingTool`**

At lines 412–414, change:

```rust
/// The `apply_mode_mapping` tool: an `Approving` tool the agent loop intercepts
/// by name. The loop parses the model's `ModeMapping` from the args, validates
/// it, and raises `AgentUpdate::ModeMappingApproval`. `run()` is never called.
```

to:

```rust
/// The `apply_mode_mapping` tool: an `Interactive` tool the agent loop
/// intercepts by name (alongside `ask_user`). The loop parses the model's
/// `ModeMapping` from the args, emits a `QuestionAsked` (kind = ModeMapping),
/// and parks for the user's answer. `run()` is never called.
```

- [ ] **Step 3: Verify the workspace compiles**

Run: `cargo build --workspace`
Expected: FAIL — the `Some(zoid_tools::ToolKind::Approving) if tc.name == "apply_mode_mapping"` arm in `agent.rs` is now unreachable (the tool returns `Interactive`). This is expected; Task 7 deletes that arm and unifies the dispatch. For now, the arm stays but never fires. The `Interactive` arm in `agent.rs` doesn't yet handle `apply_mode_mapping` (only `ask_user`) — so the tool would fall through to the generic `Local`/`run()` path and return an error.

To keep the workspace functional during the bridge, temporarily extend the `Interactive` arm's guard in `agent.rs` (line 885) from:

```rust
                Some(zoid_tools::ToolKind::Interactive) if tc.name == "ask_user" => {
```

to:

```rust
                Some(zoid_tools::ToolKind::Interactive)
                    if tc.name == "ask_user" || tc.name == "apply_mode_mapping" =>
                {
```

And inside the arm, add the `apply_mode_mapping` branch (the `else` branch of the `if tc.name == "ask_user"` split). After parsing `question`/`choices` for `ask_user`, add:

```rust
                    let (question, choices) = if tc.name == "ask_user" {
                        let question = tc
                            .args
                            .get("question")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let choices = tc
                            .args
                            .get("choices")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|c| c.as_str().map(String::from))
                                    .collect::<Vec<String>>()
                            })
                            .unwrap_or_default();
                        (question, choices)
                    } else {
                        // apply_mode_mapping
                        let mapping = match crate::mode_wizard::parse_mapping_args(&tc.args) {
                            Ok(m) => m,
                            Err(reason) => {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: tc.id.clone(),
                                        name: tc.name.clone(),
                                        output: format!(
                                            "apply_mode_mapping: {reason}. Re-propose with valid args."
                                        ),
                                        is_error: true,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                                continue;
                            }
                        };
                        let detail = crate::mode_wizard::detailed_approval_summary(&mapping);
                        let choices = vec!["Approve".into(), "Reject".into(), "Adjust".into()];
                        // Emit QuestionAsked with the ModeMapping kind.
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::QuestionAsked {
                                id: tc.id.clone(),
                                kind: zoid_core::event::QuestionKind::ModeMapping {
                                    mapping: Box::new(mapping),
                                },
                                question: detail.clone(),
                                choices: choices.clone(),
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        (detail, choices)
                    };
```

This unifies the arm. The `QuestionAsked` emission for `ask_user` (from Task 4) stays; the `apply_mode_mapping` branch adds its own. Then the existing parking + `QuestionAnswered` + `ToolResult` emission (from Task 4) handles both.

**Important:** This makes the old `ToolKind::Approving` arm (lines 820–884) dead code — it never fires because `apply_mode_mapping` now returns `Interactive`. Leave the dead arm for Task 7 to delete (keeps the diff focused).

- [ ] **Step 4: Run the bin tests**

Run: `cargo test -p zoid`
Expected: PASS.

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo clippy -p zoid --all-targets --all-features -- -D warnings`
Run: `cargo fmt -p zoid`
Expected: clippy may warn about the unreachable `Approving` arm. If so, add `#[allow(dead_code)]` to the arm temporarily, or delete the arm now (Task 7 deletes it anyway — moving the deletion here is fine). Prefer deleting the arm now to avoid the warning:

Delete lines 820–884 (the `Some(zoid_tools::ToolKind::Approving) if tc.name == "apply_mode_mapping"` arm) entirely. The unified `Interactive` arm now handles `apply_mode_mapping`.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs crates/zoid/src/agent.rs
git commit -m "feat(wizard): apply_mode_mapping is now ToolKind::Interactive

The tool rides the unified Interactive arm (intercepted by name alongside
ask_user). The old Approving arm in agent.rs is deleted (dead code once
the kind changes). The Interactive arm now branches on tc.name to build
the QuestionAsked payload (Ask vs ModeMapping)."
```

---

## Task 7: Delete the old paths (`ToolKind::Approving`, `AgentUpdate::ModeMappingApproval`, `Overlay::Question`, `render_question`, `pending_mode_mapping`)

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs`
- Modify: `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/src/main.rs`
- Modify: `crates/zoid-tui/src/state.rs`
- Modify: `crates/zoid-tui/src/render.rs`

**Why:** Now that the unified `Interactive` arm handles both surfaces and the bin reads `kind` from the event log, the old paths are dead. Delete them.

- [ ] **Step 1: Delete `ToolKind::Approving`**

In `crates/zoid-tools/src/lib.rs`, delete lines 52–56 (the `Approving` variant + its doc comment):

```rust
    /// A tool that requires user approval before its effect lands (e.g.
    /// `apply_mode_mapping`). The agent loop intercepts it by name, raises a
    /// UI approval prompt, and parks until the user answers. `run()` is never
    /// called; the loop emits the tool result from the approval outcome.
    Approving,
```

- [ ] **Step 2: Delete `AgentUpdate::ModeMappingApproval`**

In `crates/zoid/src/agent.rs`, delete lines 119–128 (the `ModeMappingApproval` variant + its doc comment):

```rust
    /// The model proposed a mode mapping via `apply_mode_mapping`; the loop
    /// validated it and is parking for user approval. `reply` receives the
    /// user's decision: "Approve" (materialize), "Reject" (cancel), or
    /// free-text (adjust — re-propose). The bin, not the loop, runs the
    /// materializer on "Approve".
    ModeMappingApproval {
        mapping: zoid_core::wizard::ModeMapping,
        summary: String,
        reply: oneshot::Sender<String>,
    },
```

- [ ] **Step 3: Delete the `ModeMappingApproval` UI handler arm in `main.rs`**

In `crates/zoid/src/main.rs`, delete lines 1856–1866 (the `AgentUpdate::ModeMappingApproval { ... }` match arm):

```rust
                    AgentUpdate::ModeMappingApproval { mapping, summary, reply } => {
                        let detail = zoid::mode_wizard::detailed_approval_summary(&mapping);
                        let _ = summary;
                        app.pending_mode_mapping = Some((mapping, reply));
                        app.shell.question =
                            Some(zoid_tui::question::QuestionState::new(
                                detail,
                                vec!["Approve".into(), "Reject".into(), "Adjust".into()],
                            ));
                        app.shell.overlay = zoid_tui::state::Overlay::Question;
                    }
```

- [ ] **Step 4: Delete `pending_mode_mapping` from `App`**

In `crates/zoid/src/main.rs`, delete the field declaration at line 1049:

```rust
    pending_mode_mapping: Option<(
```

and the following lines through the closing `),`. Read the exact lines around 1049 to get the full type, then delete the field + its trailing comma.

Then delete all initializers of `pending_mode_mapping`:
- Line 1296: `pending_mode_mapping: None,` (in `App::new` or similar)
- Line 3985: `pending_mode_mapping: None,` (in the test `App` literal)

Search for all `pending_mode_mapping` references and confirm none remain:

```bash
rg pending_mode_mapping crates/zoid/src/main.rs
```

Expected: no matches.

- [ ] **Step 5: Remove `Overlay::Question` raises from the `AskUser` handler**

In `crates/zoid/src/main.rs`, in the `AgentUpdate::AskUser { ... }` handler (lines 1748–1761), remove the overlay raise. The handler now only sets `ShellState.question` (the inline card's live state) — no overlay. Change:

```rust
                    AgentUpdate::AskUser {
                        question,
                        choices,
                        reply,
                    } => {
                        tracing::debug!(
                            "main: AskUser received, raising Question overlay (choices={})",
                            choices.len()
                        );
                        app.shell.question =
                            Some(zoid_tui::question::QuestionState::new(question, choices));
                        app.shell.overlay = zoid_tui::state::Overlay::Question;
                        app.pending_answer = Some(reply);
                    }
```

to:

```rust
                    AgentUpdate::AskUser {
                        question,
                        choices,
                        reply,
                    } => {
                        tracing::debug!(
                            "main: AskUser received, opening inline card (choices={})",
                            choices.len()
                        );
                        app.shell.question =
                            Some(zoid_tui::question::QuestionState::new(question, choices));
                        app.pending_answer = Some(reply);
                        app.body_cache = None;
                    }
```

(The `body_cache = None` forces a rebuild so the open card renders immediately.)

Also update the `TurnComplete`/`pending_answer = None` line at ~1739 and ~2250 to clear the overlay only if needed — actually, those lines clear `pending_answer` on turn end; they no longer need to touch `Overlay::Question` (it's gone). Check lines 1739 and 2250 and remove any `Overlay::Question` references there.

- [ ] **Step 6: Delete `Overlay::Question` from `state.rs`**

In `crates/zoid-tui/src/state.rs`, delete line 55:

```rust
    Question,
```

- [ ] **Step 7: Delete `render_question` and its call site from `render.rs`**

In `crates/zoid-tui/src/render.rs`:
- Delete lines 214–217 (the `Overlay::Question` arm in the overlay render match):

```rust
    } else if state.overlay == Overlay::Question {
        if let Some(q) = &state.question {
            render_question(frame, frame.area(), q);
        }
    } else if state.overlay == Overlay::ProviderSwitch {
```

becomes:

```rust
    } else if state.overlay == Overlay::ProviderSwitch {
```

- Delete the `render_question` function (lines 1084–1170) and the `QUESTION_HINT` const (line 1084). The whole block from `const QUESTION_HINT` through the closing `}` of `render_question` goes.

- Remove `use crate::question::QuestionMode;` inside `render_question` if it was a local import (it was — line 1088). Since the function is deleted, the import goes with it.

- [ ] **Step 8: Delete the `Overlay::Question` arms from `route.rs`**

In `crates/zoid-tui/src/route.rs`:
- Delete lines 131–136 (the `Overlay::Question` arm in `route_key`):

```rust
        Overlay::Question => {
            return match &state.question {
                Some(q) => crate::question::route_question_key(q, key),
                None => Action::Noop,
            };
        }
```

- Delete lines 395–401 (the `Overlay::Question` short-circuit in `route_mouse`):

```rust
    if state.overlay == Overlay::Question {
        return match m.kind {
            MouseEventKind::ScrollDown => Action::QuestionMove(1),
            MouseEventKind::ScrollUp => Action::QuestionMove(-1),
            _ => Action::Noop,
        };
    }
```

(The soft-capture branch added in Task 8 handles question input routing without the overlay.)

- [ ] **Step 9: Verify the workspace compiles**

Run: `cargo build --workspace`
Expected: FAIL — `route.rs` still references `Overlay::Question` in the test at line 926 (`s.overlay = Overlay::Question;`). Delete or update that test. Search for all `Overlay::Question` references:

```bash
rg "Overlay::Question" crates/
```

Delete/fix every remaining reference. The test at `route.rs:926` likely sets up a question-overlay scenario — replace it with a test that sets `state.question = Some(...)` instead (the soft-capture path).

- [ ] **Step 10: Run the full workspace tests**

Run: `cargo test --workspace`
Expected: PASS — all tests green. The `route.rs` test that used `Overlay::Question` is updated or deleted.

- [ ] **Step 11: Run clippy + fmt**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Run: `cargo fmt --all`
Expected: clean.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tools/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid-tui/src/state.rs crates/zoid-tui/src/render.rs crates/zoid-tui/src/route.rs
git commit -m "refactor: delete ToolKind::Approving, AgentUpdate::ModeMappingApproval, Overlay::Question

The unified Interactive arm (Task 6) and event-log-driven answer_question
(Task 5) replace the old paths. Deleted:
- ToolKind::Approving (zoid-tools)
- AgentUpdate::ModeMappingApproval (agent.rs)
- Overlay::Question variant (state.rs)
- render_question + QUESTION_HINT (render.rs)
- pending_mode_mapping field (main.rs)
- ModeMappingApproval UI handler arm (main.rs)
- Overlay::Question arms in route_key + route_mouse (route.rs)
- Overlay::Question raise in AskUser handler (main.rs; card is inline)"
```

---

## Task 8: Soft-capture input routing (replaces the overlay routing)

**Files:**
- Modify: `crates/zoid-tui/src/route.rs`
- Modify: `crates/zoid-tui/src/question.rs` (if `route_question_key` needs adjustment for the inline card)

**Why:** With no overlay, the open card still needs to capture keyboard input. Add a soft-capture branch at the top of `route_key`: while `state.question.is_some()`, route to `route_question_key`. The message textarea is not focused while a question is open — typing goes to the card.

- [ ] **Step 1: Add the soft-capture branch to `route_key`**

In `crates/zoid-tui/src/route.rs`, at the top of `pub fn route_key` (line 122), before the `// 1. Overlays capture keys first.` comment, add:

```rust
pub fn route_key(state: &ShellState, key: KeyEvent) -> Action {
    // 0. An open inline question card captures input (soft-capture): while
    // state.question is Some, typing goes to the card's free-text buffer,
    // arrows move the highlight, Enter submits, Esc cancels. The message
    // textarea is not focused during a question.
    if let Some(q) = &state.question {
        return crate::question::route_question_key(q, key);
    }

    // 1. Overlays capture keys first.
    match state.overlay {
```

- [ ] **Step 2: Add the soft-capture to `route_mouse`**

In `route_mouse` (line 389), add the same guard at the top, before the deleted `Overlay::Question` short-circuit (which is gone now):

```rust
pub fn route_mouse(state: &ShellState, layout: &ShellLayout, m: MouseEvent) -> Action {
    // An open inline question card captures scroll (navigate choices); other
    // mouse input is ignored while a question is pending.
    if state.question.is_some() {
        return match m.kind {
            MouseEventKind::ScrollDown => Action::QuestionMove(1),
            MouseEventKind::ScrollUp => Action::QuestionMove(-1),
            _ => Action::Noop,
        };
    }
    // Overlays are keyboard-driven. ...
```

- [ ] **Step 3: Verify `route_question_key` works for the inline card**

`route_question_key` (in `question.rs`) already routes ↑↓/Enter/Esc/backspace/char for both `Pick` and `FreeText` modes. It returns `Action::QuestionMove` / `QuestionSelect` / `QuestionChar` / `QuestionBackspace` / `QuestionAbort` — all handled in `main.rs` (lines 2808–2851). No change needed unless the Enter semantics change (the spec says: if `free_text` non-empty → `FreeText`, else `Choice`). Check `QuestionState::resolved()` (question.rs:59) — it already does this for `FreeText` mode. For `Pick` mode, `resolved()` returns `Choice`/`EnterFreeText`/`LetYouDecide`. The spec's Enter rule (text if present else highlighted choice) requires a tweak: in `Pick` mode, if `free_text` is non-empty, Enter should submit `FreeText(free_text)`, not the highlighted choice.

Update `QuestionState::resolved()` in `crates/zoid-tui/src/question.rs` (line 59) to:

```rust
    pub fn resolved(&self) -> QuestionOutcome {
        match self.mode {
            QuestionMode::FreeText => {
                if self.free_text.is_empty() {
                    QuestionOutcome::LetYouDecide
                } else {
                    QuestionOutcome::FreeText(self.free_text.clone())
                }
            }
            QuestionMode::Pick => {
                // If the user typed free text while in pick mode, submit that
                // (Enter = text if present, else highlighted choice).
                if !self.free_text.is_empty() {
                    return QuestionOutcome::FreeText(self.free_text.clone());
                }
                let rows = self.rows();
                let idx = self.selected.min(rows.len() - 1);
                if idx == rows.len() - 1 {
                    QuestionOutcome::LetYouDecide
                } else if idx == rows.len() - 2 {
                    QuestionOutcome::EnterFreeText
                } else {
                    QuestionOutcome::Choice(rows[idx].clone())
                }
            }
        }
    }
```

- [ ] **Step 4: Allow typing in `Pick` mode**

`route_question_key` (question.rs:84) currently only accepts `Char`/`Backspace` in `FreeText` mode. For the inline card, typing in `Pick` mode should append to `free_text` (so the user can type a custom answer even when choices are shown). Update `route_question_key`:

```rust
pub fn route_question_key(state: &QuestionState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => Action::QuestionMove(-1),
        KeyCode::Down => Action::QuestionMove(1),
        KeyCode::Enter => Action::QuestionSelect,
        KeyCode::Esc => Action::QuestionAbort,
        KeyCode::Backspace => Action::QuestionBackspace,
        KeyCode::Char(c) => Action::QuestionChar(c),
        _ => Action::Noop,
    }
}
```

This unifies the routing: ↑↓ always move, Enter always selects, Esc always aborts, typing always appends, backspace always pops. The mode (`Pick` vs `FreeText`) only affects rendering, not routing. Remove the `match state.mode` wrapper.

- [ ] **Step 5: Update the `question.rs` tests**

The existing tests in `question.rs` (lines 103–165) assert `route_question_key` behavior per-mode. Update them to match the unified routing. For example, `esc_routes_to_abort_in_both_modes` still passes (Esc → abort in both). But tests that assert `Pick` mode ignores `Char` must be updated — now `Char` always appends. Rewrite the tests to assert the unified behavior. Add a new test:

```rust
    #[test]
    fn pick_mode_typing_appends_to_free_text() {
        let q = QuestionState::new("db?", vec!["pg".into(), "sqlite".into()]);
        assert_eq!(
            route_question_key(&q, k(KeyCode::Char('x'))),
            Action::QuestionChar('x')
        );
    }

    #[test]
    fn pick_mode_enter_with_free_text_submits_free_text() {
        let mut q = QuestionState::new("db?", vec!["pg".into()]);
        q.free_text = "custom".into();
        assert_eq!(q.resolved(), QuestionOutcome::FreeText("custom".into()));
    }
```

- [ ] **Step 6: Run the route + question tests**

Run: `cargo test -p zoid-tui`
Expected: PASS.

- [ ] **Step 7: Run clippy + fmt + full workspace**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Run: `cargo fmt --all`
Run: `cargo test --workspace`
Expected: clean + green.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid-tui/src/question.rs
git commit -m "feat(tui): soft-capture input routing for inline question cards

While ShellState.question is Some, route_key and route_mouse route to the
card (typing appends to free_text, arrows move, Enter submits, Esc cancels)
instead of the message textarea. route_question_key is unified across Pick
and FreeText modes; resolved() now returns FreeText when free_text is
non-empty even in Pick mode (Enter = text if present, else highlighted choice)."
```

---

## Task 9: Inline card rendering in `build_conversation`

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`

**Why:** `build_conversation` must render `ChatMsg::Question` as an inline card. The card reads the live cursor (`selected`/`free_text`) from `ShellState.question` — but `build_conversation` is pure (takes `&[ChatMsg]`, no `ShellState`). We thread an optional `&QuestionState` through the call chain so the open card's cursor is live.

- [ ] **Step 1: Thread `QuestionState` through the conversation builders**

The signature change cascades through `conversation_lines`, `code_hits`, `build_conversation`, `conversation_view`, `conversation_view_indexed`, `detail_lines`. Add an optional `question: Option<&QuestionState>` parameter to each. The callers (the bin's render path) pass `app.shell.question.as_ref()`.

In `crates/zoid-tui/src/chat.rs`, update the public fns:

```rust
pub fn conversation_lines(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    let mut hits = Vec::new();
    build_conversation(
        msgs,
        streaming,
        caret_on,
        tz_offset_secs,
        width,
        &mut hits,
        &mut Vec::new(),
        question,
    )
}

pub fn code_hits(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<CodeHit> {
    let mut hits = Vec::new();
    build_conversation(
        msgs,
        streaming,
        caret_on,
        tz_offset_secs,
        width,
        &mut hits,
        &mut Vec::new(),
        question,
    );
    hits
}

fn build_conversation(
    msgs: &[ChatMsg],
    streaming: bool,
    caret_on: bool,
    tz_offset_secs: i32,
    width: usize,
    hits: &mut Vec<CodeHit>,
    msg_starts: &mut Vec<usize>,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
```

And `conversation_view_indexed`:

```rust
pub fn conversation_view_indexed(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> (Vec<Line<'static>>, Vec<usize>) {
```

And `conversation_view`:

```rust
pub fn conversation_view(
    msgs: &[ChatMsg],
    view: &ChatView,
    streaming: bool,
    width: usize,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
    conversation_view_indexed(msgs, view, streaming, width, question).0
}
```

And `detail_lines`:

```rust
fn detail_lines(
    msgs: &[ChatMsg],
    tz_offset_secs: i32,
    width: usize,
    msg_starts: &mut Vec<usize>,
    question: Option<&crate::question::QuestionState>,
) -> Vec<Line<'static>> {
```

Inside `conversation_view_indexed`, pass `question` through to `build_conversation` and `detail_lines`.

- [ ] **Step 2: Add the `ChatMsg::Question` arm to `build_conversation`**

In `build_conversation` (the `for (i, m) in msgs.iter().enumerate()` loop), add the `Question` arm after `Delegated`:

```rust
            ChatMsg::Question {
                id: _,
                kind,
                question: qtext,
                choices,
                state,
                ts: _,
            } => {
                blank_between_turns(&mut lines);
                let (selected, free_text) = match state {
                    zoid_core::projection::QuestionCardState::Open { selected, free_text } => {
                        // Overwrite the projection's placeholder cursor with the
                        // live cursor from ShellState.question (if present).
                        if let Some(q) = question {
                            (q.selected, q.free_text.clone())
                        } else {
                            (*selected, free_text.clone())
                        }
                    }
                    zoid_core::projection::QuestionCardState::Answered { .. } => {
                        (0, String::new())
                    }
                };
                let card = render_question_card(kind, qtext, choices, state, selected, &free_text, width);
                lines.extend(card);
            }
```

- [ ] **Step 3: Implement `render_question_card`**

Add the helper function in `chat.rs` (after `build_conversation`):

```rust
/// Render an inline question card as a block of lines. Open state shows the
/// question + choices (with the live highlight) + a hint line; Answered state
/// collapses to the question title + the answer on the last line. `width` is
/// the conversation column width; the card wraps to fit.
fn render_question_card(
    kind: &zoid_core::event::QuestionKind,
    question: &str,
    choices: &[String],
    state: &zoid_core::projection::QuestionCardState,
    selected: usize,
    free_text: &str,
    width: usize,
) -> Vec<Line<'static>> {
    use zoid_core::event::QuestionKind;
    use zoid_core::projection::QuestionCardState;

    let title = match kind {
        QuestionKind::Ask => " Question ",
        QuestionKind::ModeMapping { .. } => " Mode mapping — review ",
    };
    let content_w = width.saturating_sub(4).max(20);

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Top border with title.
    lines.push(card_border_top(title, content_w + 2));
    // Question body (split on newlines, prefix each line with "│ ").
    for para in question.split('\n') {
        if para.is_empty() {
            lines.push(Line::from(Span::styled(
                "│ ".to_string(),
                Style::new().fg(color::DIM),
            )));
        } else {
            for l in wrap_plain(para, content_w) {
                lines.push(Line::from(Span::styled(
                    format!("│ {l}"),
                    Style::new().fg(color::TXT),
                )));
            }
        }
    }
    lines.push(Line::from(Span::styled(
        "│ ".to_string(),
        Style::new().fg(color::DIM),
    )));

    match state {
        QuestionCardState::Open { .. } => {
            // Choices with highlight.
            for (i, c) in choices.iter().enumerate() {
                let marker = if i == selected { "●" } else { "○" };
                let style = if i == selected {
                    Style::new().fg(color::TXT).bg(color::SEL_BG)
                } else {
                    Style::new().fg(color::TXT)
                };
                lines.push(Line::from(Span::styled(
                    format!("│   {marker} {}", c),
                    style,
                )));
            }
            // Free-text echo (if the user typed anything).
            if !free_text.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("│   {}{}", free_text, glyph::CARET),
                    Style::new().fg(color::TXT),
                )));
            }
            lines.push(Line::from(Span::styled(
                "│ Type your answer, or pick above. Enter to submit · Esc to cancel."
                    .to_string(),
                Style::new().fg(color::DIM),
            )));
            // Bottom border.
            lines.push(card_border_bottom(content_w + 2));
        }
        QuestionCardState::Answered { answer } => {
            // Collapsed: just the answer on the last line.
            lines.push(Line::from(Span::styled(
                format!("└ ► {}", answer),
                Style::new().fg(color::TXT),
            )));
        }
    }
    lines
}

fn card_border_top(title: &str, width: usize) -> Line<'static> {
    let inner = width.saturating_sub(title.chars().count() + 2);
    let right = "─".repeat(inner);
    Line::from(vec![
        Span::styled(format!("┌─{title}"), Style::new().fg(color::DIM)),
        Span::styled(right, Style::new().fg(color::DIM)),
    ])
}

fn card_border_bottom(width: usize) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::new().fg(color::DIM),
    ))
}
```

Note: `wrap_plain` lives in `render.rs` — it's `pub(crate)` or needs to be made `pub(crate)`. Check its visibility. If it's private to `render.rs`, either make it `pub(crate)` or move/duplicate a small wrap helper in `chat.rs`. Prefer making it `pub(crate) fn wrap_plain` in `render.rs` and importing it in `chat.rs`.

- [ ] **Step 4: Update all callers of the changed signatures**

Search for callers of `conversation_lines`, `code_hits`, `conversation_view`, `conversation_view_indexed` across the workspace and pass the new `question` argument. The main caller is the bin's render path in `main.rs` — pass `app.shell.question.as_ref()`. Tests in `chat.rs` pass `None`.

```bash
rg "conversation_lines|code_hits|conversation_view|conversation_view_indexed" crates/
```

Update every call site. For tests, pass `None`.

- [ ] **Step 5: Write a unit test for the card rendering**

In `chat.rs` tests, add:

```rust
    #[test]
    fn open_question_card_renders_choices_and_highlight() {
        use zoid_core::event::QuestionKind;
        use zoid_core::projection::QuestionCardState;
        let msgs = vec![ChatMsg::Question {
            id: "c1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
            state: QuestionCardState::Open {
                selected: 1,
                free_text: String::new(),
            },
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(joined.contains("retry or skip?"), "question text rendered");
        assert!(joined.contains("○ Retry"), "first choice rendered unselected");
        assert!(joined.contains("● Skip"), "second choice rendered selected");
    }

    #[test]
    fn answered_question_card_collapses_to_answer_line() {
        use zoid_core::event::QuestionKind;
        use zoid_core::projection::QuestionCardState;
        let msgs = vec![ChatMsg::Question {
            id: "c1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
            state: QuestionCardState::Answered {
                answer: "Skip".into(),
            },
            ts: 0,
        }];
        let lines = conversation_lines(&msgs, false, true, 0, 80, None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(joined.contains("└ ► Skip"), "answered card shows the answer");
        assert!(
            !joined.contains("○ Retry"),
            "answered card does not re-render choices"
        );
    }
```

(If `QuestionCard` isn't the right import name — it's `QuestionCardState` — fix the import. The variant is `ChatMsg::Question { state: QuestionCardState, .. }`.)

- [ ] **Step 6: Run the chat tests**

Run: `cargo test -p zoid-tui chat::tests`
Expected: PASS.

- [ ] **Step 7: Run clippy + fmt + full workspace**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Run: `cargo fmt --all`
Run: `cargo test --workspace`
Expected: clean + green.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(tui): render inline question cards in build_conversation

ChatMsg::Question renders as an inline card: open state shows the question
+ choices with the live highlight (read from ShellState.question via the
new optional parameter) + a hint line; answered state collapses to the
answer line. wrap_plain is now pub(crate) so chat.rs can use it. All
callers of conversation_lines/conversation_view/code_hits updated to
thread the question state; tests pass None."
```

---

## Task 10: Integration tests + body-cache invalidation

**Files:**
- Modify: `crates/zoid/src/main.rs` (body-cache invalidation on `ShellState.question` mutation)
- Create: `crates/zoid/tests/inline_question_card.rs`

**Why:** End-to-end coverage: `ask_user` → card appears inline → user answers → `QuestionAnswered` + `ToolResult` in the log. `apply_mode_mapping` → card with mapping review → Approve → materializer runs. Cancel path (Esc). Plus the body-cache invalidation on every `ShellState.question` mutation so the cursor moves render immediately.

- [ ] **Step 1: Body-cache invalidation on question mutation**

In `crates/zoid/src/main.rs`, every place that mutates `app.shell.question` must set `app.body_cache = None`. The sites are:
- `AskUser` handler (already done in Task 7 — `app.body_cache = None` added).
- `Action::QuestionMove` (line 2808) — after `q.selected = ...`, add `app.body_cache = None;`.
- `Action::QuestionChar` (line 2814) — after `q.free_text.push(c);`, add `app.body_cache = None;`.
- `Action::QuestionBackspace` (line 2819) — after `q.free_text.pop();`, add `app.body_cache = None;`.
- `answer_question` (end) — after `app.shell.question = None;`, add `app.body_cache = None;`.
- `Action::QuestionAbort` (line 2845) — after `app.shell.question = None;`, add `app.body_cache = None;`.

For the `QuestionMove`/`QuestionChar`/`QuestionBackspace` arms, the mutation is inside `if let Some(q) = &mut app.shell.question { ... }` — add `app.body_cache = None;` after the `if let` block (the borrow ends, so `app` is available).

- [ ] **Step 2: Write the `ask_user` integration test**

Create `crates/zoid/tests/inline_question_card.rs`:

```rust
//! Integration: the inline question card for `ask_user` and `apply_mode_mapping`.
//! Asserts QuestionAsked/QuestionAnswered land in the event log and the card
//! renders inline (not as an overlay).

use zoid::agent::{AgentUpdate, Answer};
use zoid_core::event::{EventKind, QuestionKind};

// A minimal scripted test: drive one turn that calls ask_user, assert the
// QuestionAsked event is in the log before the reply, send the reply, assert
// QuestionAnswered + ToolResult land. This mirrors the existing
// mode_import_wiring.rs pattern (scripted provider + injected tools).
//
// NOTE: the exact harness depends on zoid's test helpers (ScriptedProvider,
// App literal). Follow the pattern in crates/zoid/tests/mode_import_wiring.rs.
// If a full App harness is too heavy, write a focused agent-loop test that
// drives run_agent_turn with a scripted provider emitting an ask_user
// ToolCall, and asserts the events emitted on the session.

#[test]
fn ask_user_emits_question_asked_then_answered() {
    // TODO: drive run_agent_turn with a ScriptedProvider that emits a
    // ToolCall { name: "ask_user", args: { question, choices } }.
    // Assert: events contain QuestionAsked { kind: Ask, ... } before the
    // reply is sent.
    // Send Answer::Choice("Skip") on the reply channel.
    // Assert: events contain QuestionAnswered { answer: "Skip" } and
    // ToolResult { id, name: "ask_user", output: "Skip" }.
    // This test follows the mode_import_wiring.rs harness shape.
}

#[test]
fn apply_mode_mapping_emits_question_asked_with_mapping() {
    // TODO: drive run_agent_turn with a ScriptedProvider that emits a
    // ToolCall { name: "apply_mode_mapping", args: { mode_name, entries } }.
    // Assert: events contain QuestionAsked { kind: ModeMapping { mapping }, ... }.
    // Send Answer::Choice("Approve") on the reply channel.
    // Assert: events contain QuestionAnswered { answer: "Approve" } and
    // ToolResult { id, name: "apply_mode_mapping", output: "Approve" }.
}

#[test]
fn cancel_path_emits_cancelled_answer() {
    // TODO: drive run_agent_turn with an ask_user ToolCall.
    // Drop the reply channel (simulating Esc).
    // Assert: events contain QuestionAnswered { answer: "[user aborted]" } and
    // ToolResult { is_error: true }.
}
```

Then implement the three test bodies following the `mode_import_wiring.rs` harness pattern. If the existing harness doesn't expose `run_agent_turn` directly for integration tests, write them as `zoid`-internal tests in `crates/zoid/src/agent.rs`'s `#[cfg(test)]` block instead (same crate, can call private fns). Prefer the integration-test file if the harness supports it; otherwise move to `agent.rs` tests.

- [ ] **Step 3: Run the integration tests**

Run: `cargo test -p zoid --test inline_question_card`
(Or `cargo test -p zoid agent::tests` if the tests live in `agent.rs`.)
Expected: PASS.

- [ ] **Step 4: Run the full workspace + clippy + fmt**

Run: `cargo test --workspace`
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Run: `cargo fmt --all`
Expected: clean + green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid/tests/inline_question_card.rs
git commit -m "test: inline question card integration tests + body-cache invalidation

Integration tests assert QuestionAsked/QuestionAnswered/ToolResult land in
the event log for ask_user, apply_mode_mapping (Approve), and the cancel
path. Body-cache invalidation on every ShellState.question mutation so the
card cursor re-renders immediately."
```

---

## Verification checklist

After all tasks, verify:

- [ ] `cargo test --workspace` green.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- [ ] `cargo fmt --all` clean (run `cargo fmt --all -- --check` to verify).
- [ ] No `Co-Authored-By` trailer: `git log --format='%b' HEAD~10..HEAD | rg -i 'co-authored'` returns nothing.
- [ ] No `Overlay::Question` references: `rg "Overlay::Question" crates/` returns nothing.
- [ ] No `ToolKind::Approving` references: `rg "ToolKind::Approving" crates/` returns nothing.
- [ ] No `AgentUpdate::ModeMappingApproval` references: `rg "ModeMappingApproval" crates/` returns nothing.
- [ ] No `pending_mode_mapping` references: `rg "pending_mode_mapping" crates/` returns nothing.
- [ ] No `render_question` references (except the new `render_question_card`): `rg "fn render_question\b" crates/` returns nothing.
- [ ] Manual smoke: run `zoid`, trigger an `ask_user` (or `:mode import` → `apply_mode_mapping`), confirm the card renders inline, typing captures, ↑↓ moves, Enter submits, the card collapses on answer, and the conversation scrollback shows the card (not a tool-result line).