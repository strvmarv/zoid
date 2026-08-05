# Subagent Dispatch Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three mechanical backstops to `dispatch_subagent` that a pure-prompt-text fix (2026-07-27, already merged and proven insufficient by the 2026-08-04 incident) couldn't hold: reject duplicate dispatches, cap runaway self-narration after a dispatch, and add an explicit English-language directive.

**Architecture:** All changes live in the existing `crates/zoid/src/agent.rs` — no new files. A duplicate-dispatch check is added to the existing `dispatch_subagent` handling arm, before its existing pool-capacity check. A single turn-scoped `bool` (`dispatched_this_turn`) latches `true` the moment a dispatch succeeds and stays latched for the remainder of that `run_turn_inner` call. This latch is the narration gate: it is **position-independent** by construction (it does not consult event ordering at all), so it is immune to the compaction/eviction bookkeeping events and out-of-order tool-call batching that defeated a "check the last event on the branch" heuristic. The latch is correct as a one-way latch (never un-set within a turn) because a subagent's `DelegationResult` is emitted by a **detached** `tokio::spawn` (`spawn_subagent.rs:165-166`) into the **main loop's** `app.events` (`main.rs:3487`), never into the running turn's local `events` log (`run_turn_inner` owns `events: EventLog` by value, `agent.rs:805`, with no reload-from-session step between sub-turns) — so the resolution always arrives in a **fresh** `run_turn_inner` call (started by `spawn_turn` after `plan_delegation_wake`, `main.rs:3497-3499`), which begins with `dispatched_this_turn = false`. No per-turn pruning is possible or needed. The narration cap reuses `run_turn_inner`'s existing `aborted`/`stream_task.abort()` cleanup path (built for Esc/Ctrl-C) rather than inventing new cancellation plumbing — it adds one new trigger condition and one new bit of state (`narration_capped`) to tell the two apart when deciding whether to emit a marker. The cap is guarded by a per-sub-turn `tool_call_seen_this_sub_turn` flag so a compliant ack + dispatch + brief follow-up in the same sub-turn is never falsely capped. `SYSTEM_PROMPT` gets one appended sentence, following the exact pattern of the 2026-07-27 fix.

**Tech Stack:** Rust, `zoid` + `zoid-core` crates, existing test harness (`#[tokio::test]`, `SequencedProvider`, `chat_turn_config()`).

**Design doc:** `docs/superpowers/specs/2026-08-04-subagent-dispatch-hardening-design.md`
**Bug diagnosis:** `docs/bugs/subagent-dispatch-language-drift.md`

> **Note on the design doc:** The design doc (`...-design.md`) originally specified `awaiting_dispatch: Vec<String>` plus a `dispatch_is_resolved` pure helper that prunes the list as `DelegationResult` events "appear in the log." A code review (gilfoyle, 2026-08-04) verified against source that this pruning is dead code: the running turn's local `events` log never receives `DelegationResult` within the same `run_turn_inner` call (it arrives via the main loop into `app.events`, which is a different `EventLog` instance). This plan replaces that mechanism with the simpler, correct `dispatched_this_turn: bool` latch described above. The design doc should be updated to match; this plan is the source of truth.

## Global Constraints

- All three changes are unconditional — no config flags, no gating behind a feature switch.
- The dedup guard keys on `(agent, task)` (both fields of `SubagentHandle`), never `task` alone — an identical task dispatched to a *different* agent profile is a legitimate parallel-review pattern and must not be rejected. The registry stores the **resolved** agent name at insert (`agent.rs:1742` stores `resolved_agent_name.clone()`), and the dedup compares against the current call's `resolved_agent_name` — both sides are in the same resolved-name space. This invariant (registry stores resolved names) must be preserved by any future change to the insert site.
- The narration cap must never affect a turn's response to a fresh `UserMessage` — verified (`main.rs:3553-3578`) that a message typed mid-turn is queued (`app.pending_message`) and only converted into a `UserMessage` event + a **new** `run_turn_inner` call once the current one fully returns. `dispatched_this_turn` is scoped to a single `run_turn_inner` call (declared before `'turn: loop`), so a fresh call always starts with it `false` — no extra carve-out needed, but Task 3's tests must prove this rather than assume it.
- The narration cap must also not falsely trip on a sub-turn that already did the right thing (dispatched then briefly narrated): the trip is guarded by `!tool_call_seen_this_sub_turn`, so once any `ProviderEvent::ToolCall` arrives in a sub-turn the cap cannot fire for the rest of that sub-turn. Task 3's tests must prove this (the `TextDelta → ToolCall → TextDelta` path) rather than assume it.
- `dispatch_subagent`'s existing "queued" (pool-at-capacity) path is untouched — the dedup check runs *before* it, so a duplicate is rejected even when the pool has headroom (the exact condition that let the incident's duplicates both run).
- `outcome` (the `&'static str` used only in `run_turn_inner`'s final `tracing::info!` line, `agent.rs:2676-2683`) gets one new value (`"narration_capped"`) — nothing else reads it, so this is purely additive.
- No new files. No refactoring beyond what each task's tests require.

---

### Task 1: Duplicate-dispatch guard

**Files:**
- Modify: `crates/zoid/src/agent.rs:1644-1650` (insert new check between the profile-resolution `match` and the existing pool-capacity check)
- Test: `crates/zoid/src/agent.rs` (new tests near `dispatch_two_subagents_second_is_rejected`, `agent.rs:5061`)

**Interfaces:**
- Consumes: `task: String` and `resolved_agent_name: String` (both already bound earlier in the `dispatch_subagent` arm, at `agent.rs:1587-1592` and `agent.rs:1617-1644` respectively), `config.in_flight: Option<Arc<Mutex<HashMap<String, SubagentHandle>>>>`, `SubagentHandle { task: String, agent: String, .. }` (`agent.rs:105-118`)
- Produces: an early-`continue` path in the tool-call loop that emits a non-error `ToolResult` naming the existing duplicate's ID

- [ ] **Step 1: Write the failing tests**

In `crates/zoid/src/agent.rs`, add two new tests immediately after `dispatch_two_subagents_second_is_rejected` (after line 5164):

```rust
    #[tokio::test]
    async fn dispatch_subagent_rejects_exact_duplicate_agent_and_task() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch two identical tasks".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review the diff", "agent": "gilfoyle"}),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review the diff", "agent": "gilfoyle"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("both handled".into()),
                ProviderEvent::Done,
            ],
        ]));

        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            reg.clone(),
            zoid_tools::KillSlot::new(),
        ));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let mut config = chat_turn_config();
        config.agents = Some(reg);
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        // Plenty of pool headroom (default max_concurrent = 3) — this must be
        // rejected by the DEDUP check, not the pool-capacity check, matching
        // the incident (both duplicates had headroom).

        let out = run_agent_turn(
            config,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        let results: Vec<(String, bool)> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { name, output, is_error, .. }
                    if name == "dispatch_subagent" =>
                {
                    Some((output.clone(), *is_error))
                }
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2, "two dispatch tool results");
        assert!(!results[0].1, "first dispatch succeeds: {:?}", results[0]);
        assert!(
            !results[1].1,
            "duplicate rejection is not an error result: {:?}",
            results[1]
        );
        assert!(
            results[1].0.contains("already running as sub-"),
            "second result must name the duplicate: {:?}",
            results[1]
        );
        assert!(
            !results[1].0.starts_with("subagent queued"),
            "must be rejected by the dedup check, not the pool-capacity check: {:?}",
            results[1]
        );
    }

    #[tokio::test]
    async fn dispatch_subagent_allows_same_task_different_agent() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "two independent reviews".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review the diff", "agent": "gilfoyle"}),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review the diff", "agent": "delegate"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("both handled".into()),
                ProviderEvent::Done,
            ],
        ]));

        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            reg.clone(),
            zoid_tools::KillSlot::new(),
        ));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let mut config = chat_turn_config();
        config.agents = Some(reg);
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        // Provide a spare script so the detached subagent (which shares the
        // SequencedProvider) can pop one without stealing the parent's
        // continuation — see "Shared SequencedProvider" note in Task 2.
        // (Not strictly required for this assertion, which only checks the
        // first sub-turn's tool results, but kept for consistency.)

        let out = run_agent_turn(
            config,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        let errored: Vec<_> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { name, output, is_error, .. }
                    if name == "dispatch_subagent" && *is_error =>
                {
                    Some(output.clone())
                }
                _ => None,
            })
            .collect();
        assert!(
            errored.is_empty(),
            "identical task to a DIFFERENT agent must not be rejected: {errored:?}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid --lib dispatch_subagent_rejects_exact_duplicate_agent_and_task`
Expected: FAIL — `assertion failed: results[1].0.contains("already running as sub-")` (the check doesn't exist yet, so both dispatches currently succeed)

Run: `cargo test -p zoid --lib dispatch_subagent_allows_same_task_different_agent`
Expected: PASS already (this one documents current behavior — confirms the guard you're about to add won't need to special-case it; keep it as a regression test)

- [ ] **Step 3: Add the dedup check**

In `crates/zoid/src/agent.rs`, insert this new block between the closing `};` of the profile-resolution `match` (line 1644) and the existing pool-capacity-check comment (line 1645):

```rust
                    // Duplicate-dispatch guard: the 2026-08-04 incident dispatched
                    // the identical task twice, 270ms apart, and the pool-capacity
                    // check alone didn't catch it (both had headroom). Keyed on
                    // (agent, task), not task alone — an identical task dispatched
                    // to a DIFFERENT agent profile (e.g. two independent reviewer
                    // opinions on the same diff) is legitimate and must not be
                    // rejected. Both sides use the RESOLVED agent name: the
                    // registry stores `resolved_agent_name` at insert
                    // (agent.rs:1742), so this comparison is in a single name
                    // space. `task.trim()` normalizes only the comparison; the
                    // stored handle keeps the raw task (so list_subagents shows
                    // what the model actually sent).
                    if let Some(set) = &config.in_flight {
                        let dup_id = set
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(_, h)| {
                                h.agent == resolved_agent_name && h.task.trim() == task.trim()
                            })
                            .map(|(id, _)| id.clone());
                        if let Some(dup_id) = dup_id {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: format!(
                                        "dispatch_subagent: an identical task is already \
                                         running as {dup_id} — do not dispatch a duplicate, \
                                         wait for its DelegationResult."
                                    ),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            continue;
                        }
                    }
```

> **Lock scope note:** `set.lock().unwrap()` is held only across the synchronous `iter`/`find`/`map` — no `.await` while locked. The `emit(...).await?` runs only after the guard drops the lock (the `if let Some(dup_id) = dup_id` block owns the `String`, not the guard). Correct.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid --lib dispatch_subagent_rejects_exact_duplicate_agent_and_task`
Expected: PASS

Run: `cargo test -p zoid --lib dispatch_subagent_allows_same_task_different_agent`
Expected: PASS

Run: `cargo test -p zoid --lib dispatch_two_subagents_second_is_rejected` (existing test — must not regress)
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): reject duplicate (agent, task) dispatch_subagent calls"
```

---

### Task 2: Turn-scoped dispatch latch (`dispatched_this_turn`) — position-independent, immune to compaction/eviction and batching order

**Files:**
- Modify: `crates/zoid/src/agent.rs` (new turn-scoped `bool` in `run_turn_inner`, `agent.rs:836`; set it in the `dispatch_subagent` handling arm, `agent.rs:1745`)
- Test: `crates/zoid/src/agent.rs` (new `SequencedProvider`-based tests proving the latch is position-independent)

**Interfaces:**
- Produces: `dispatched_this_turn: bool` (turn-scoped in `run_turn_inner`, declared before `'turn: loop` alongside `iterations`, `context_retries`, `outcome`, `turn_produced_content`, `cwd_for_exec`) — Task 3 consumes it as the narration gate. Set `true` in the `dispatch_subagent` arm, unconditionally (even when `config.in_flight` is `None`, since the latch is separate from the concurrency pool/`list_subagents` registry and must gate narration regardless).

**Why a latch, not a prunable list:** A `DelegationResult` is emitted by a **detached** `tokio::spawn` (`spawn_subagent.rs:165-166`) that `session.append`s the event and sends `AgentUpdate::Appended` to the **main loop** (`main.rs:3403-3487`), which pushes it into `app.events`. The running turn's local `events: EventLog` (`agent.rs:805`, owned by value) never receives it — there is no reload-from-session step between sub-turns. So within one `run_turn_inner` call, a dispatch made this turn can never be observed as "resolved." The resolution always arrives in a **fresh** `run_turn_inner` call (started by `spawn_turn` after `plan_delegation_wake`, `main.rs:3497-3499`), which begins with `dispatched_this_turn = false`. A one-way latch is therefore both necessary and sufficient: it gates exactly the sub-turns after the dispatch within the same turn, and never a subsequent turn.

> **Shared `SequencedProvider` note (read before writing Task 2/3 tests):** `SequencedProvider` is a shared `VecDeque` behind a `std::sync::Mutex` (`agent.rs:3835-3860`). `dispatch_subagent` clones the provider into a detached `tokio::spawn` (`spawn_subagent.rs:69`), and the subagent's `run_agent_turn_cancellable` calls `provider.stream()` — popping the **next** script off the **same** shared queue. Tests that need the parent to retrieve a specific post-dispatch script must provide a **spare copy** of that script so the subagent can pop one without stealing the parent's. Every narration test below follows this pattern: `[dispatch_script, <continuation_script>, <continuation_script>]`. On the current-thread `#[tokio::test]` runtime the subagent runs only when the parent yields (e.g. at `emit().await`), so at most one subagent script is consumed; two copies guarantee the parent always retrieves one. Without this, the parent's second `stream()` can return empty (`unwrap_or_default()`), the cap never trips, and the test flakes.

- [ ] **Step 1: Declare the latch**

In `crates/zoid/src/agent.rs`, inside `run_turn_inner` (starts at `agent.rs:799`), declare the state before `'turn: loop {` (currently at `agent.rs:838`) — add immediately above it, alongside the other turn-scoped locals (`iterations`, `context_retries`, `outcome`, `turn_produced_content`, `cwd_for_exec`):

```rust
    // Latched true the moment this turn dispatches a subagent, and never
    // un-set within this run_turn_inner call. Gates post-dispatch free-text
    // narration (Task 3). Position-independent by construction — it does not
    // consult event ordering, so it is immune to preflight_gate's
    // compaction/eviction bookkeeping events (which can append a
    // ToolResultCompacted/TurnsEvicted event after the dispatch's ToolResult)
    // and to dispatch_subagent being batched with an unrelated tool call that
    // executes after it. A DelegationResult re-enters via a FRESH
    // run_turn_inner call (spawn_turn after plan_delegation_wake,
    // main.rs:3497-3499), which starts with this false — verified that a
    // mid-turn UserMessage is queued (main.rs:3553-3578), not injected into the
    // running call, so a fresh call's response is never gated.
    let mut dispatched_this_turn = false;
```

- [ ] **Step 2: Set the latch in the dispatch arm**

In the `dispatch_subagent` handling arm, right after the existing registry-insert block (`agent.rs:1733-1745`: `if let Some(reg) = &config.in_flight { reg.lock().unwrap().insert(sub_id.clone(), SubagentHandle { .. }); }`), add — unconditionally, not inside that `if let`, since the latch must apply even when `config.in_flight` is `None`:

```rust
                    dispatched_this_turn = true;
```

- [ ] **Step 3: Write the compaction regression test**

This proves the latch is position-independent: eviction bookkeeping events landing between the dispatch and the following sub-turn do not un-gate it (trivially true, since the latch ignores events entirely — but the test pins it as a regression guard). Add near the other `run_turn_inner`-level tests (after Task 1's new tests):

```rust
    #[tokio::test]
    async fn narration_gate_survives_compaction_after_dispatch() {
        // Proves the latch is position-independent: preflight_gate's eviction
        // path can append a TurnsEvicted event AFTER the dispatch's ToolResult
        // — a positional "is the last branch event a dispatch ToolResult" gate
        // would be defeated by this. The latch ignores event ordering, so it
        // stays on. Force eviction (same technique as
        // preflight_gate_evicts_before_send, agent.rs:3459) in the same window
        // as an unresolved dispatch and confirm the FOLLOWING sub-turn is still
        // gated (a long, un-capped run would leave the fabricated text
        // un-truncated in the output). NOTE: preflight_gate early-returns when
        // eviction is disabled (agent.rs:2888), so this test MUST set
        // cfg.eviction — without it no TurnsEvicted event is ever appended and
        // the scenario is unreachable.
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let big = "x".repeat(3000);
        let mut seed = Vec::new();
        for i in 0..8u128 {
            seed.push(Event::new(
                Ulid::from(i * 2 + 1),
                None,
                (i * 2 + 1) as i64,
                EventKind::UserMessage { text: big.clone() },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: "ok".into() },
            ));
        }
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let long_narration = "y".repeat(1000); // ~333 estimated tokens, well past a 60-token budget

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review", "agent": "gilfoyle"}),
                }),
                ProviderEvent::Done,
            ],
            // Spare copy: the detached subagent shares the SequencedProvider
            // and may pop one of these; two copies guarantee the parent
            // retrieves the other for its gated sub-turn.
            vec![
                ProviderEvent::TextDelta(long_narration.clone()),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(long_narration.clone()),
                ProviderEvent::Done,
            ],
        ]));

        let mut cfg = chat_turn_config();
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 3_000,
            band_headroom_pct: 20,
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
        cfg.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            cfg,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            out.iter().any(|e| matches!(e.kind, EventKind::TurnsEvicted { .. })),
            "sanity: eviction must actually have fired for this test to prove anything"
        );
        // Strengthened (gilfoyle M3): the TurnsEvicted must land AFTER the
        // dispatch ToolResult in event order, proving the interleaving the
        // test is named for — not just that eviction fired somewhere.
        let dispatch_idx = out
            .iter()
            .position(|e| matches!(
                &e.kind,
                EventKind::ToolResult { name, .. } if name == "dispatch_subagent"
            ))
            .expect("dispatch ToolResult must be present");
        let evict_idx = out
            .iter()
            .position(|e| matches!(e.kind, EventKind::TurnsEvicted { .. }))
            .expect("TurnsEvicted must be present");
        assert!(
            evict_idx > dispatch_idx,
            "TurnsEvicted (idx {evict_idx}) must land AFTER the dispatch ToolResult \
             (idx {dispatch_idx}) to exercise the compaction-interleaving scenario"
        );
        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text == &long_narration
            )),
            "the full un-capped narration must NOT appear — the latch must have \
             stayed on despite the TurnsEvicted event landing between the dispatch \
             and this sub-turn"
        );
    }
```

- [ ] **Step 4: Write the batching-order regression test**

```rust
    #[tokio::test]
    async fn narration_gate_fires_when_dispatch_is_not_the_last_batched_call() {
        // Proves the latch is position-independent in a second way:
        // dispatch_subagent batched with an unrelated tool call, executed
        // first — its ToolResult isn't the last event even without any
        // compaction. The latch is set the moment the dispatch arm runs,
        // regardless of what executes after it in the same batch.
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch then read".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let long_narration = "y".repeat(1000);

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review", "agent": "gilfoyle"}),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "r1".into(),
                    name: "read".into(),
                    args: json!({"path": "Cargo.toml"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(long_narration.clone()),
                ProviderEvent::Done,
            ],
            // Spare copy for the detached subagent (see shared-provider note).
            vec![
                ProviderEvent::TextDelta(long_narration.clone()),
                ProviderEvent::Done,
            ],
        ]));

        let mut config = chat_turn_config();
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            config,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text == &long_narration
            )),
            "must be gated even though dispatch_subagent's ToolResult wasn't the \
             last event in the batch (the 'read' call's ToolResult came after it)"
        );
    }
```

**Note:** these two tests will not fully pass until Task 3 wires the actual budget-tripping logic — right now `dispatched_this_turn` is set but nothing consumes it yet. That's expected: run them now to confirm they fail for the *right* reason (the long narration DOES appear, because nothing caps it yet), then re-run them at the end of Task 3 as that task's integration proof. Do not skip Step 6 below.

- [ ] **Step 5: Run the new tests and confirm the expected (pre-Task-3) failure**

Run: `cargo test -p zoid --lib narration_gate_survives_compaction_after_dispatch narration_gate_fires_when_dispatch_is_not_the_last_batched_call`
Expected: both FAIL on the "must NOT appear" assertions (the long narration is present, unmodified) — confirms the test setup correctly exercises the scenario and isn't vacuously passing.

> The `TurnsEvicted`-after-dispatch ordering assertion in the compaction test may pass or fail at this stage depending on timing; what matters is that the "must NOT appear" assertion fails (narration present). If the ordering assertion fails first, that's acceptable pre-Task-3 — re-run after Task 3.

- [ ] **Step 6: Run the full existing test suite to confirm no regressions from the latch wiring itself**

Run: `cargo test -p zoid --lib`
Expected: all PASS except the two new tests from Step 4 (still red, expected — Task 3 turns them green)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): add turn-scoped dispatch latch (position-independent narration gate)"
```

---

### Task 3: Narration budget cap — trip on runaway post-dispatch text, guarded against false positives

**Files:**
- Modify: `crates/zoid/src/agent.rs` (streaming loop inside `run_turn_inner`, the `aborted` cleanup block)
- Test: `crates/zoid/src/agent.rs` (new `run_turn_inner`-level tests; the two tests from Task 2 turn green here)

**Interfaces:**
- Consumes: `dispatched_this_turn: bool` (Task 2), `zoid_core::economy::estimate_tokens(s: &str) -> u64`, `WARN_GLYPH` (`agent.rs:24`)
- Produces: `DISPATCH_NARRATION_BUDGET_TOKENS: u64` constant; a `narration_capped: bool` local; a `narration_tokens: u64` per-sub-turn counter; a `tool_call_seen_this_sub_turn: bool` per-sub-turn flag; one new `outcome` value (`"narration_capped"`)

- [ ] **Step 1: Add the budget constant**

In `crates/zoid/src/agent.rs`, near `MAX_TOOL_ITERATIONS` (`agent.rs:334`), add:

```rust
/// Free-text budget (estimated tokens) for a sub-turn immediately following an
/// unresolved dispatch_subagent — the exact moment the model is told to "end
/// your turn now" and free-form narration risks running away into a
/// fabricated/hallucinated prediction of the subagent's result instead of
/// stopping (2026-08-04 incident: 300 chunks / 77s before the real result).
/// This bounds cost and blast radius; it does NOT guarantee zero contamination
/// of context — see docs/superpowers/specs/2026-08-04-subagent-dispatch-hardening-design.md.
/// Only counted when `dispatched_this_turn` is latched AND no ToolCall has
/// arrived yet in this sub-turn (so a compliant ack + dispatch + brief
/// follow-up is never falsely capped).
const DISPATCH_NARRATION_BUDGET_TOKENS: u64 = 60;
```

- [ ] **Step 2: Add the per-sub-turn locals**

In `run_turn_inner`, alongside the existing `let mut aborted = false;` (`agent.rs:891`, which is **inside** the `'turn: loop` and thus per-sub-turn), add:

```rust
        let mut narration_capped = false;
        let mut narration_tokens: u64 = 0;
        let mut tool_call_seen_this_sub_turn = false;
```

> These are declared inside `'turn: loop`, so they reset to their initial values at the start of every sub-turn (each iteration of `'turn`). This is deliberate: the budget is per-sub-turn, not cumulative across sub-turns — text counted in sub-turn 1 does not carry over to sub-turn 2. `narration_capped` is per-sub-turn but only read in the `aborted` cleanup block of the same sub-turn, so resetting it is safe.

- [ ] **Step 3: Set `tool_call_seen` in the `ToolCall` arm**

In `crates/zoid/src/agent.rs`, in the `ProviderEvent::ToolCall` arm (`agent.rs:922-939`), add `tool_call_seen_this_sub_turn = true;` at the top of the arm (before or after `emit` — order doesn't matter, only that it's set within this sub-turn before the trip check could fire on a later `TextDelta`). Insert as the first statement in the arm:

```rust
                ProviderEvent::ToolCall(mut tc) => {
                    tool_call_seen_this_sub_turn = true;
                    tc.id = ensure_tool_call_id(std::mem::take(&mut tc.id));
                    turn_produced_content = true;
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            args: tc.args.to_string(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    pending.push(tc);
                }
```

- [ ] **Step 4: Trip the cap in the `TextDelta` arm**

In `crates/zoid/src/agent.rs`, replace the `ProviderEvent::TextDelta` arm (`agent.rs:909-921`):

Current:
```rust
                ProviderEvent::TextDelta(s) => {
                    turn_produced_content = true;
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ModelDelta { text: s },
                        session_id,
                        now,
                    )
                    .await?;
                }
```

New:
```rust
                ProviderEvent::TextDelta(s) => {
                    turn_produced_content = true;
                    if dispatched_this_turn && !tool_call_seen_this_sub_turn {
                        narration_tokens += zoid_core::economy::estimate_tokens(&s);
                    }
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ModelDelta { text: s },
                        session_id,
                        now,
                    )
                    .await?;
                    if dispatched_this_turn
                        && !tool_call_seen_this_sub_turn
                        && narration_tokens > DISPATCH_NARRATION_BUDGET_TOKENS
                    {
                        narration_capped = true;
                        aborted = true;
                        break;
                    }
                }
```

> The `!tool_call_seen_this_sub_turn` guard is the false-positive fix (gilfoyle B2): once any `ProviderEvent::ToolCall` has arrived this sub-turn, the cap cannot trip for the rest of that sub-turn — so a legit `TextDelta(ack) → ToolCall(dispatch) → TextDelta(brief follow-up)` is never capped. Text emitted before the ToolCall in the same sub-turn still counts toward the budget, but cannot trip until the ToolCall would have arrived; if the ToolCall arrives, the guard flips and the sub-turn is safe. A sub-turn with only `TextDelta` (the runaway-narration case) has no ToolCall and trips normally.

- [ ] **Step 5: Emit the marker and distinguish the outcome in the `aborted` cleanup block**

In `crates/zoid/src/agent.rs`, replace the tail of the `if aborted { ... }` block (`agent.rs:1025-1052`) — specifically the final two lines (`outcome = "aborted"; break 'turn;`) that follow the `pending.drain(..)` loop:

Current:
```rust
            outcome = "aborted";
            break 'turn;
```//└ this is at the end of the `if aborted { ... }` block starting agent.rs:1025

New:
```rust
            if narration_capped {
                emit(
                    &session,
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::AssistantMessage {
                        text: format!(
                            "{WARN_GLYPH} turn ended early — continued narrating past a \
                             subagent dispatch instead of waiting; discarded speculative \
                             text."
                        ),
                    },
                    session_id,
                    now,
                )
                .await?;
                outcome = "narration_capped";
            } else {
                outcome = "aborted";
            }
            break 'turn;
```

> The marker emits after `stream_task.abort()` and the `pending.drain(..)` loop (the existing code in the `if aborted` block runs first). For a real HTTP provider, `stream_task.abort()` cancels the in-flight request. For `SequencedProvider` in tests, `abort()` drops the spawned future; any already-queued `TextDelta` that crossed the channel before the `break` is unread and dropped (the inner `loop` has exited). Either way, text after the trip is not persisted — but note the test assertion validates the `break` happened, not that `abort()` performed real cancellation (a no-op `abort()` would still pass it). The production path is correct; the test's coverage of cancellation mechanics is limited by the test double.

- [ ] **Step 6: Write the tests**

Add near the Task 2 tests:

```rust
    #[tokio::test]
    async fn narration_cap_trips_and_discards_speculative_text() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch a review".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        // Well past the 60-token (~180 char) budget.
        let runaway = "the reviewer said this and that and more and more ".repeat(10);

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review", "agent": "gilfoyle"}),
                }),
                ProviderEvent::Done,
            ],
            // Spare copy for the detached subagent (see shared-provider note).
            vec![
                ProviderEvent::TextDelta(runaway.clone()),
                ProviderEvent::TextDelta("more text that should never be reached".into()),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(runaway.clone()),
                ProviderEvent::TextDelta("more text that should never be reached".into()),
                ProviderEvent::Done,
            ],
        ]));

        let mut config = chat_turn_config();
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            config,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text.contains("should never be reached")
            )),
            "text emitted after the trip must never be persisted"
        );
        assert!(
            out.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text } if text.contains("turn ended early")
            )),
            "the warning marker must be persisted so the transcript shows why the turn ended"
        );
    }

    #[tokio::test]
    async fn narration_under_budget_completes_normally() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch a review".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let reply = "Dispatched, waiting for review.";

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "review", "agent": "gilfoyle"}),
                }),
                ProviderEvent::Done,
            ],
            // Spare copy for the detached subagent.
            vec![
                ProviderEvent::TextDelta(reply.into()),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(reply.into()),
                ProviderEvent::Done,
            ],
        ]));

        let mut config = chat_turn_config();
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            config,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text == reply
            )),
            "a short, compliant post-dispatch turn must pass through untouched"
        );
        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text } if text.contains("turn ended early")
            )),
            "no cap marker when the budget wasn't crossed"
        );
        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ToolResult { is_error, .. } if *is_error
            )),
            "turn must have completed normally — no error ToolResult (gilfoyle m5: \
             assert completion, not just marker absence)"
        );
    }

    #[tokio::test]
    async fn narration_gate_does_not_block_immediate_second_dispatch() {
        // A gated sub-turn whose FIRST action is a ToolCall (e.g. dispatching
        // the next task) with no preceding narration must proceed completely
        // normally — the budget only counts free-text, never tool calls.
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch task 4, then task 5".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task 4", "agent": "delegate"}),
                }),
                ProviderEvent::Done,
            ],
            // Sub-turn 2: first thing is a ToolCall, zero preceding text.
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task 5", "agent": "delegate"}),
                }),
                ProviderEvent::Done,
            ],
            // Spare copies for the two detached subagents (one per dispatch).
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task 5", "agent": "delegate"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task 5", "agent": "delegate"}),
                }),
                ProviderEvent::Done,
            ],
        ]));

        let mut config = chat_turn_config();
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            config,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        let dispatch_results: Vec<_> = out
            .iter()
            .filter(|e| matches!(
                &e.kind,
                EventKind::ToolResult { name, is_error, .. }
                    if name == "dispatch_subagent" && !*is_error
            ))
            .collect();
        assert_eq!(
            dispatch_results.len(),
            2,
            "both dispatches must succeed — the second must not be blocked by \
             the narration gate just because it landed in a gated sub-turn"
        );
        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text } if text.contains("turn ended early")
            )),
            "no cap marker — the gated sub-turn never emitted any free text to trip on"
        );
    }

    #[tokio::test]
    async fn narration_cap_does_not_false_trip_after_a_dispatch_in_the_same_sub_turn() {
        // The false-positive the gilfoyle B2 review caught: a compliant
        // sub-turn that acks briefly, then dispatches (a ToolCall), then
        // narrates a bit more — the cumulative TextDelta tokens would cross the
        // budget, but because a ToolCall arrived this sub-turn the cap must
        // NOT trip. Without the `!tool_call_seen_this_sub_turn` guard this
        // would falsely cap a correct turn.
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "ack then dispatch then narrate".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        // Sub-turn 1: short ack (under budget) + dispatch. The dispatch latches
        // the gate AND sets tool_call_seen_this_sub_turn for sub-turn 1.
        // Sub-turn 2: long-ish narration that CUMULATIVELY would exceed 60
        // tokens, but sub-turn 2 is a fresh sub-turn (tool_call_seen reset) —
        // UNLESS a ToolCall arrives in sub-turn 2. To exercise the guard in a
        // single gated sub-turn, we pack ack + ToolCall + more-text into ONE
        // sub-turn (sub-turn 2): the ack text accumulates, then a ToolCall
        // arrives (tool_call_seen := true), then more text that would push
        // over budget — but the guard prevents the trip.
        let ack = "Dispatching the next review now. ".repeat(3); // ~33 tokens
        let more = " and then I will wait for the result to come back. ".repeat(3); // ~33 tokens
        // ack (~33) + more (~33) = ~66 > 60, but a ToolCall sits between them.

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "first review", "agent": "gilfoyle"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(ack.clone()),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "second review", "agent": "delegate"}),
                }),
                ProviderEvent::TextDelta(more.clone()),
                ProviderEvent::Done,
            ],
            // Spare copies for the detached subagents.
            vec![
                ProviderEvent::TextDelta(ack.clone()),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "second review", "agent": "delegate"}),
                }),
                ProviderEvent::TextDelta(more.clone()),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta(ack.clone()),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "second review", "agent": "delegate"}),
                }),
                ProviderEvent::TextDelta(more.clone()),
                ProviderEvent::Done,
            ],
        ]));

        let mut config = chat_turn_config();
        config.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let out = run_agent_turn(
            config,
            provider,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            out.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text == &more
            )),
            "the post-ToolCall narration must be preserved — the cap must not \
             trip once a ToolCall arrived in this sub-turn"
        );
        assert!(
            !out.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text } if text.contains("turn ended early")
            )),
            "no cap marker — a ToolCall in this sub-turn disables the trip"
        );
    }

    #[tokio::test]
    async fn narration_gate_never_caps_a_response_to_a_fresh_user_message() {
        // Regression test for the core UX constraint: a subagent dispatched in
        // an EARLIER, separate run_agent_turn call must never suppress a full
        // response in a NEW call answering a fresh user message, even though
        // that earlier dispatch is still unresolved in the persisted event log.
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();

        // First call: dispatch a subagent, leave it unresolved.
        let seed1 = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "dispatch a review".into() },
        )];
        for e in &seed1 {
            session.append(e.clone()).await.unwrap();
        }
        let provider1 = std::sync::Arc::new(SequencedProvider::new(vec![vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "d1".into(),
                name: "dispatch_subagent".into(),
                args: json!({"task": "review", "agent": "gilfoyle"}),
            }),
            ProviderEvent::Done,
        ]]));
        // Fresh in_flight for call 1 (gilfoyle m1: don't carry stale registry
        // state across the two turns — production clears it on DelegationResult,
        // which doesn't run in this test, so use a fresh map per call).
        let mut config1 = chat_turn_config();
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        config1.agents = Some(reg.clone());
        config1.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx1, mut rx1) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx1.recv().await.is_some() {} });
        let mut all_events = run_agent_turn(
            config1.clone(),
            provider1,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                reg.clone(),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session.clone(),
            crate::eventlog::EventLog::from_vec(seed1),
            "m".into(),
            tx1,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();
        assert!(
            !all_events.iter().any(|e| matches!(e.kind, EventKind::DelegationResult { .. })),
            "sanity: the dispatch must still be unresolved going into the second call"
        );

        // Second call: a BRAND NEW user message, long reply, no ToolCall at all
        // — must complete in full, uncapped, despite the still-unresolved
        // dispatch sitting in the persisted history.
        let long_reply = "here is a very long, perfectly legitimate answer ".repeat(10);
        all_events.push(Event::new(
            Ulid::from(99u128),
            None,
            99,
            EventKind::UserMessage { text: "actually, tell me something else entirely".into() },
        ));
        let provider2 = std::sync::Arc::new(SequencedProvider::new(vec![vec![
            ProviderEvent::TextDelta(long_reply.clone()),
            ProviderEvent::Done,
        ]]));
        // Fresh in_flight for call 2 (no subagent spawned this call, but keep
        // it explicit to mirror production isolation).
        let mut config2 = chat_turn_config();
        config2.agents = Some(reg);
        config2.in_flight = Some(std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (tx2, mut rx2) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx2.recv().await.is_some() {} });
        let out2 = run_agent_turn(
            config2,
            provider2,
            std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                config2.agents.as_ref().unwrap().clone(),
                zoid_tools::KillSlot::new(),
            )),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            all_events,
            "m".into(),
            tx2,
            Ulid::from(1u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        assert!(
            out2.iter().any(|e| matches!(
                &e.kind,
                EventKind::ModelDelta { text } if text == &long_reply
            )),
            "a response to a fresh user message must never be capped, regardless \
             of an unresolved dispatch elsewhere in history"
        );
        assert!(
            !out2.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text } if text.contains("turn ended early")
            )),
            "no cap marker — this call's dispatched_this_turn starts false"
        );
    }
```

- [ ] **Step 7: Run all narration-cap tests to verify they now pass**

Run:
```bash
cargo test -p zoid --lib \
  narration_cap_trips_and_discards_speculative_text \
  narration_under_budget_completes_normally \
  narration_gate_does_not_block_immediate_second_dispatch \
  narration_cap_does_not_false_trip_after_a_dispatch_in_the_same_sub_turn \
  narration_gate_never_caps_a_response_to_a_fresh_user_message \
  narration_gate_survives_compaction_after_dispatch \
  narration_gate_fires_when_dispatch_is_not_the_last_batched_call
```
Expected: all seven PASS (the last two are Task 2's tests, now green)

- [ ] **Step 8: Run the full existing test suite**

Run: `cargo test -p zoid --lib`
Expected: PASS — no regressions, in particular `dispatch_subagent_returns_id_as_tool_result`, `dispatch_two_subagents_second_is_rejected`, and any test exercising Esc/Ctrl-C cancellation (confirms the `aborted`/`narration_capped` split didn't disturb the existing external-cancel path).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): cap runaway post-dispatch narration, guarded against false positives"
```

---

### Task 4: English-language directive in `SYSTEM_PROMPT`

**Files:**
- Modify: `crates/zoid/src/agent.rs:36-48` (`SYSTEM_PROMPT` constant)
- Test: `crates/zoid/src/agent.rs` (new unit test, same style as the existing `system_prompt_reinforces_no_poll`)

**Interfaces:**
- Consumes: nothing
- Produces: the updated `SYSTEM_PROMPT` string that `wrap_reassertion` re-states periodically and `default_profile()` uses as the Chat-mode system prompt

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/agent.rs`, in the `#[cfg(test)] mod tests` block (this module has `use super::*;`, so `SYSTEM_PROMPT` is in scope), add:

```rust
    #[test]
    fn system_prompt_directs_english_regardless_of_source_language() {
        assert!(
            SYSTEM_PROMPT.contains("Always respond in English"),
            "SYSTEM_PROMPT must direct English regardless of file/tool-output/\
             subagent-summary language, so wrap_reassertion periodically \
             reinforces it: {SYSTEM_PROMPT}"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid --lib system_prompt_directs_english_regardless_of_source_language`
Expected: FAIL — `SYSTEM_PROMPT must direct English...`

- [ ] **Step 3: Append the sentence to `SYSTEM_PROMPT`**

In `crates/zoid/src/agent.rs`, modify the `SYSTEM_PROMPT` constant (`agent.rs:36-48`). Current end of the string:

```rust
     exactly one wake — never schedule duplicate wakes for the same event, and \
     cancel a pending wake before scheduling a replacement.";
```

New:

```rust
     exactly one wake — never schedule duplicate wakes for the same event, and \
     cancel a pending wake before scheduling a replacement. Always respond in \
     English, regardless of what language any file, tool output, subagent \
     summary, or prior turn contains.";
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid --lib system_prompt_directs_english_regardless_of_source_language`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): direct English regardless of source-content language in SYSTEM_PROMPT"
```

---

### Task 5: Full build, clippy, and full-suite verification

**Files:**
- No modifications — verification only

**Interfaces:**
- Consumes: all four prior tasks
- Produces: confirmation the workspace compiles clean and all tests pass

- [ ] **Step 1: Build the workspace**

Run: `cargo build -p zoid`
Expected: PASS (no compile errors)

- [ ] **Step 2: Run every test added by this plan, by name, once more together**

Run:
```bash
cargo test -p zoid --lib \
  dispatch_subagent_rejects_exact_duplicate_agent_and_task \
  dispatch_subagent_allows_same_task_different_agent \
  narration_gate_survives_compaction_after_dispatch \
  narration_gate_fires_when_dispatch_is_not_the_last_batched_call \
  narration_cap_trips_and_discards_speculative_text \
  narration_under_budget_completes_normally \
  narration_gate_does_not_block_immediate_second_dispatch \
  narration_cap_does_not_false_trip_after_a_dispatch_in_the_same_sub_turn \
  narration_gate_never_caps_a_response_to_a_fresh_user_message \
  system_prompt_directs_english_regardless_of_source_language
```
Expected: all PASS (note: `dispatch_is_resolved_true_only_with_matching_delegation_result` is no longer present — the helper was removed; see the architecture note)

- [ ] **Step 3: Run clippy on the touched crate**

Run: `cargo clippy -p zoid -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Run the full `zoid` test suite to catch any regressions**

Run: `cargo test -p zoid --lib`
Expected: PASS — in particular re-confirm `dispatch_subagent_returns_id_as_tool_result`, `dispatch_two_subagents_second_is_rejected`, `dispatch_with_unknown_agent_emits_error_listing_available`, `preflight_gate_evicts_before_send`, and `system_prompt_reinforces_no_poll` (the July 27 fix's own test — must still pass unmodified).

- [ ] **Step 5: Commit if any fixups were needed**

If Steps 1-4 required any fixup edits, commit them:

```bash
git add -A
git commit -m "fix: post-verification fixups for subagent dispatch hardening"
```

If no fixups were needed, this step is a no-op — do not create an empty commit.