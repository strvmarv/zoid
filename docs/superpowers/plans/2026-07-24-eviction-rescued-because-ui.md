# Eviction "Rescued Because…" UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the rescue rationale to the user — which turns were kept
because of relevance, their rescue scores, and the goal text that drove the
decision. Today rescue is invisible (tracing-only). This plan adds the data
plumbing (`RescueRationale` on `EvictionPlan` and `TurnsEvicted`), a new
`ChatMsg::Evicted` projection variant, and TUI rendering (chip at Normal zoom,
breakdown at Detail zoom).

**Architecture:** `plan_evictions` records survivor rationale alongside victims.
`emit_eviction` destructures the plan to thread `rescue` into the `TurnsEvicted`
event. The projection emits `ChatMsg::Evicted` (instead of skipping
`TurnsEvicted`). `build_request_with_thinking` filters `ChatMsg::Evicted` out
before `map_msg` (the model never sees eviction chips — the breadcrumb stays
the sole model-side channel). The TUI renders the chip and breakdown.

**Tech Stack:** Rust workspace (`zoid-core` pure; `zoid` binary; `zoid-tui`
terminal UI). `ratatui` for rendering. `serde` for event serialization.
`ulid` for event IDs.

**Spec:** `docs/superpowers/specs/2026-07-24-eviction-rescued-because-ui-design.md`

**Dependency:** The `[eviction] rescue_weight` config branch (item 1) must be
merged first — this plan references `resolve_rescue_weight` at the
`GoalContext` construction site (agent.rs:2797), which was added by that branch.

## Global Constraints

- **`Eq` + `Hash` drops.** `EvictionPlan` (eviction.rs:414) drops `Eq`, retains
  `PartialEq`. `EventKind` (event.rs:69) and `Event` (event.rs:196) both drop
  `Eq` **and `Hash`**, retain `PartialEq`. All verified safe — no `Eq`/`Hash`-
  bound consumers, no hash-map/btree-map keying on these types. `assert_eq!`
  round-trip tests survive on `PartialEq + Debug`.
- **`ChatMsg` retains `Eq`.** All new `ChatMsg::Evicted` fields are
  `Eq`-compatible (`u64`, `Vec<String>`, `Option<RescueSummary>`, `i64`).
  `RescueSummary` and `RescuedTurnSummary` are all `Eq`-compatible (`String`,
  `u32`, `Vec`).
- **Model-request filter.** `build_request_with_thinking` (agent.rs:570) must
  filter `ChatMsg::Evicted` out before `map_msg`. `map_msg` (agent.rs:448) gains
  an inert `Evicted` arm (defense-in-depth — unreachable because the filter
  removes them first).
- **`text.clone()` before the move.** In `preflight_gate`, `text` is moved into
  the `spawn_blocking` closure at agent.rs:2780. The `goal_text` for
  `GoalContext` must be cloned *before* that closure: `let goal_text =
  text.clone();` above the `spawn_blocking` block.
- **Cross-crate discipline.** Every task builds `cargo build --workspace` and
  `cargo test --workspace`.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-core/src/eviction.rs` | `RescueRationale`, `RescuedTurn` structs; `EvictionPlan.rescue` field; `GoalContext.goal_text` field; `plan_evictions` populates `rescue`; drop `Eq` from `EvictionPlan` | Modify |
| `crates/zoid-core/src/event.rs` | `TurnsEvicted` gains `rescue` field; drop `Eq`+`Hash` from `EventKind` and `Event`; 1 test literal | Modify |
| `crates/zoid-core/src/projection.rs` | `ChatMsg::Evicted` variant; `RescueSummary`, `RescuedTurnSummary` structs; `TurnsEvicted` → `ChatMsg::Evicted`; update `conversation_skips_evicted_turns` test; 1 test literal | Modify |
| `crates/zoid/src/agent.rs` | `emit_eviction` destructures plan; `build_request_with_thinking` filters `Evicted`; `map_msg` gains `Evicted` arm; `preflight_gate` clones `goal_text`; 4 `TurnsEvicted` test literals + 1 production literal; `GoalContext` production literal | Modify |
| `crates/zoid-tui/src/chat.rs` | Normal + Detail rendering for `ChatMsg::Evicted` | Modify |
| `crates/zoid-core/src/context.rs` | 1 `TurnsEvicted` test literal | Modify |
| `crates/zoid-core/src/reassert.rs` | 1 `TurnsEvicted` test literal | Modify |

**Task order:** T1 (data model in `eviction.rs`) → T2 (`TurnsEvicted` event +
`Eq`/`Hash` drops) → T3 (`emit_eviction` + `preflight_gate` wiring) → T4
(projection + `ChatMsg::Evicted` + model-request filter) → T5 (TUI rendering).
T1 must come first (breaks `EvictionPlan` literals). T2 breaks `TurnsEvicted`
literals. T3 connects them. T4 breaks `ChatMsg` match arms. T5 adds the UI.
Recommended linear order T1→T2→T3→T4→T5.

---

### Task 1: `RescueRationale` + `GoalContext.goal_text` + `EvictionPlan.rescue`

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs`

**Interfaces:**
- Produces: `pub struct RescueRationale`, `pub struct RescuedTurn`,
  `EvictionPlan.rescue: Option<RescueRationale>`, `GoalContext.goal_text: String`.

- [ ] **Step 1: Add `goal_text` field to `GoalContext`**

At eviction.rs:197, change:

```rust
#[derive(Debug, Default, Clone)]
pub struct GoalContext {
    pub goal: Vec<f32>,
    pub vecs: HashMap<Ulid, Vec<f32>>,
    pub weight: f32,
}
```

to:

```rust
#[derive(Debug, Default, Clone)]
pub struct GoalContext {
    pub goal: Vec<f32>,
    pub vecs: HashMap<Ulid, Vec<f32>>,
    pub weight: f32,
    /// The goal text that drove the rescue decision (for `RescueRationale`).
    /// Empty when rescue is inactive. `Default::default()` ⇒ `String::new()`.
    pub goal_text: String,
}
```

- [ ] **Step 2: Update all `GoalContext { ... }` test literals**

Run: `grep -rn "GoalContext\s*{" crates/ | grep -v "fn \|struct \|Default\|default"`

Add `goal_text: String::new(),` to each test literal. Sites (verified):
- `eviction.rs` ~line 346, 862, 904, 922 (4 test sites in `plan_tests`/`relevance_tests`)

The production site at `agent.rs:2797` is updated in Task 3.

- [ ] **Step 3: Add `RescueRationale` and `RescuedTurn` structs**

Add after `EvictedTurn` (eviction.rs:412), before `EvictionPlan`:

```rust
/// Per-turn rescue rationale for candidates that were *kept* (not evicted).
/// Present only when rescue was active (non-empty goal). Survivors with
/// `rescue_bump == 0.0` are excluded — they were kept by recency, not rescue.
#[derive(Debug, Clone, PartialEq)]
pub struct RescueRationale {
    /// The goal text that drove the rescue decision.
    pub goal_text: String,
    /// The weight used (after clamping, from `resolve_rescue_weight`).
    pub weight: f32,
    /// Candidates that were kept (not evicted) with `rescue_bump > 0.0`.
    pub survivors: Vec<RescuedTurn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RescuedTurn {
    pub ids: Vec<Ulid>,
    pub topic_hint: String,
    pub base_score: f32,
    pub rescue_bump: f32,
    pub keep_score: f32,
}
```

- [ ] **Step 4: Add `rescue` field to `EvictionPlan` and drop `Eq`**

Change eviction.rs:414:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvictionPlan {
    pub turns: Vec<EvictedTurn>,
    pub rescue: Option<RescueRationale>,
}
```

(`Eq` dropped — `RescueRationale` has `f32` fields. `PartialEq` retained.)

- [ ] **Step 5: Populate `rescue` in `plan_evictions`**

After the eviction loop (eviction.rs:582, after the `for &i in &idx` block),
before `plan`, add the survivor computation:

```rust
    // Rescue rationale: survivors are candidates NOT evicted AND with bump > 0.0.
    // Only populated when rescue was active (non-empty goal).
    let rescue = if ctx.goal.is_empty() {
        None
    } else {
        let evicted_set: HashSet<Ulid> = plan.turns.iter()
            .flat_map(|t| t.ids.iter().copied())
            .collect();
        let survivors: Vec<RescuedTurn> = candidates
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                bump[*i] > 0.0
                    && !t.ids.iter().any(|id| evicted_set.contains(id))
            })
            .map(|(i, t)| RescuedTurn {
                ids: t.ids.clone(),
                topic_hint: t.topic_hint.clone(),
                base_score: scorer.score(t, ctx),
                rescue_bump: bump[i],
                keep_score: scorer.score(t, ctx) + bump[i],
            })
            .collect();
        if survivors.is_empty() {
            None
        } else {
            Some(RescueRationale {
                goal_text: ctx.goal_text.clone(),
                weight: ctx.weight,
                survivors,
            })
        }
    };
    plan.rescue = rescue;
```

Note: `scorer.score(t, ctx)` is called twice per survivor (once for `base_score`,
once for `keep_score`). This is fine — `RecencyScorer::score` just returns
`turn.index` (a field read, not a computation). If you prefer, cache it:
`let base = scorer.score(t, ctx);` and use `base` + `base + bump[i]`.

- [ ] **Step 6: Write tests**

Extend `relevant_old_turn_survives_while_newer_offgoal_is_evicted`
(eviction.rs:844). After the existing assertions, add:

```rust
    // Rescue rationale is populated.
    let rescue = rescued.rescue.as_ref().expect("rescue should be Some");
    assert_eq!(rescue.goal_text, ctx.goal_text);
    assert_eq!(rescue.weight, ctx.weight);
    // The rescued turn (id 1) should be in survivors with bump > 0.
    let survivor = rescue.survivors.iter().find(|s| s.ids.contains(&Ulid::from(1u128)));
    assert!(survivor.is_some(), "rescued turn id 1 in survivors");
    assert!(survivor.unwrap().rescue_bump > 0.0, "rescue bump > 0");
```

Add a test for rescue=None when goal is empty:

```rust
#[test]
fn rescue_is_none_when_goal_empty() {
    let events = turns8();
    let plan = plan_evictions(
        &events, &policy(5_000, 2), 8_000, &RecencyScorer, &GoalContext::default(),
    );
    assert!(plan.rescue.is_none(), "empty goal ⇒ no rescue rationale");
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p zoid-core -- rescue_is_none relevant_old_turn`
Expected: PASS.

- [ ] **Step 8: Build the workspace (expect compile errors in agent.rs)**

Run: `cargo build --workspace 2>&1 | grep "^error" | head -5`
Expected: FAIL — `missing field 'goal_text'` at the `GoalContext` production
literal in `agent.rs:2797`. This is expected — Task 3 fixes it.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(zoid-core): RescueRationale + goal_text on GoalContext + EvictionPlan.rescue

Add RescueRationale/RescuedTurn structs. EvictionPlan gains rescue field
(drop Eq — f32 in RescueRationale). GoalContext gains goal_text. plan_evictions
populates rescue with survivors (bump > 0.0, not evicted). Tests: rescue
populated when goal non-empty, None when empty."
```

---

### Task 2: `TurnsEvicted` gains `rescue` + `Eq`/`Hash` drops

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (line 69 derive, line 156 `TurnsEvicted`,
  line 196 `Event` derive, line 444 test literal)
- Modify: `crates/zoid-core/src/context.rs` (line 827 test literal)
- Modify: `crates/zoid-core/src/reassert.rs` (line 98 test literal)
- Modify: `crates/zoid-core/src/projection.rs` (line 761 test literal)
- Modify: `crates/zoid-core/src/eviction.rs` (6 test literals: lines 151, 174, 660, 699, 733, 959)
- Modify: `crates/zoid/src/agent.rs` (4 test literals: lines 3079, 4030, 4245; production: 2879)

- [ ] **Step 1: Add `rescue` field to `TurnsEvicted` and drop `Eq`+`Hash`**

At event.rs:69, change the derive:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
```

(Drop `Eq, Hash` — `RescueRationale` has `f32` fields.)

At event.rs:156, add the `rescue` field:

```rust
    TurnsEvicted {
        ids: Vec<Ulid>,
        reclaimed_tokens: u64,
        marker: EvictionMarker,
        rescue: Option<crate::eviction::RescueRationale>,
    },
```

At event.rs:196, change the `Event` derive:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
```

(Drop `Eq, Hash` — transitive from `EventKind`.)

- [ ] **Step 2: Update all 14 `TurnsEvicted { ... }` struct literals**

Run: `grep -rn "EventKind::TurnsEvicted {" crates/ | grep -v "target/" | grep -v "\.\."`

Add `rescue: None,` to each. The 14 sites are:
- `event.rs:444` (test)
- `context.rs:827` (test)
- `reassert.rs:98` (test)
- `projection.rs:761` (test)
- `eviction.rs:151, 174, 660, 699, 733, 959` (6 test sites)
- `agent.rs:2879` (production — `emit_eviction`; this one gets `rescue` in Task 3)
- `agent.rs:3079, 4030, 4245` (3 test sites)

For the production site at `agent.rs:2879`, add `rescue: None,` for now — Task 3
replaces it with the real value.

- [ ] **Step 3: Add `Serialize, Deserialize` derives to `RescueRationale` and `RescuedTurn`**

In `eviction.rs`, change the derives:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RescueRationale { ... }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RescuedTurn { ... }
```

- [ ] **Step 4: Build and test**

Run: `cargo test -p zoid-core --no-fail-fast 2>&1 | grep "test result:" | tail -3`
Expected: PASS — all existing tests pass (the `Eq`/`Hash` drop is safe, verified).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/context.rs \
       crates/zoid-core/src/reassert.rs crates/zoid-core/src/projection.rs \
       crates/zoid-core/src/eviction.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid-core): TurnsEvicted gains rescue field; drop Eq+Hash from EventKind/Event

Add rescue: Option<RescueRationale> to TurnsEvicted. Drop Eq and Hash
from EventKind (event.rs:69) and Event (event.rs:196) — f32 in
RescueRationale is not Eq/Hash. Add Serialize/Deserialize to
RescueRationale/RescuedTurn. Update all 14 TurnsEvicted literals."
```

---

### Task 3: Wire `emit_eviction` + `preflight_gate` `goal_text`

**Files:**
- Modify: `crates/zoid/src/agent.rs`

- [ ] **Step 1: Destructure `EvictionPlan` in `emit_eviction`**

At agent.rs:2858, change:

```rust
    if plan.turns.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::new();
    let mut reclaimed = 0u64;
    let mut spans = Vec::new();
    for t in plan.turns {
```

to:

```rust
    let EvictionPlan { turns, rescue } = plan;
    if turns.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::new();
    let mut reclaimed = 0u64;
    let mut spans = Vec::new();
    for t in turns {
```

Add `use zoid_core::eviction::EvictionPlan;` at the top of `emit_eviction` (or
qualify the path).

Then update the `TurnsEvicted` event construction (agent.rs:2879) to pass
`rescue`:

```rust
        EventKind::TurnsEvicted {
            ids,
            reclaimed_tokens: reclaimed,
            marker: zoid_core::event::EvictionMarker { spans },
            rescue,
        },
```

- [ ] **Step 2: Clone `goal_text` before the `spawn_blocking` move in `preflight_gate`**

At agent.rs:2770, after the `let text = ...` line and before the `spawn_blocking`
block, add:

```rust
            let goal_text = text.clone();
```

Then at agent.rs:2797, update the `GoalContext` construction:

```rust
                    zoid_core::eviction::GoalContext {
                        goal,
                        vecs,
                        weight: zoid_core::eviction::resolve_rescue_weight(
                            config.eviction.rescue_weight,
                        ),
                        goal_text,
                    }
```

> **Note:** The `weight` line references `resolve_rescue_weight` and
> `config.eviction.rescue_weight` — these exist only if the item-1 branch
> (`[eviction]` config) is merged. If it's not merged yet, use
> `weight: zoid_core::eviction::DEFAULT_RESCUE_WEIGHT,` instead.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid --features local-embed -- preflight 2>&1 | grep "test " | head -10`
Expected: PASS — all 4 preflight tests pass.

- [ ] **Step 4: Build the workspace**

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): emit_eviction passes rescue; preflight_gate threads goal_text

emit_eviction destructures EvictionPlan to extract rescue before
consuming turns. preflight_gate clones goal_text before the
spawn_blocking move closure. GoalContext construction passes
goal_text."
```

---

### Task 4: Projection — `ChatMsg::Evicted` + model-request filter

**Files:**
- Modify: `crates/zoid-core/src/projection.rs`
- Modify: `crates/zoid/src/agent.rs` (`map_msg` + `build_request_with_thinking`)

- [ ] **Step 1: Add `RescueSummary` and `RescuedTurnSummary` structs**

Add near the `ChatMsg` enum (projection.rs:20):

```rust
/// Projection-friendly (Eq-compatible) version of `RescueRationale`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescueSummary {
    pub goal_text: String,
    pub weight: u32,
    pub rescued: Vec<RescuedTurnSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuedTurnSummary {
    pub topic_hint: String,
    pub bump_milli: u32,
}
```

- [ ] **Step 2: Add `ChatMsg::Evicted` variant**

Add to the `ChatMsg` enum (projection.rs:20):

```rust
    /// An eviction wave — chip at Normal zoom, breakdown at Detail.
    /// Filtered out of the model request path (§3.1 of the design spec).
    Evicted {
        reclaimed_tokens: u64,
        evicted_topics: Vec<String>,
        rescue: Option<RescueSummary>,
        ts: i64,
    },
```

- [ ] **Step 3: Emit `ChatMsg::Evicted` in the projection**

In `conversation_for_branch` (or `conversation`), split the
`TurnsEvicted { .. } | TurnsReadmitted { .. } | DirectiveReasserted { .. }`
skip arm (projection.rs:281–286). `TurnsEvicted` gets its own arm; the other
two stay skipped:

```rust
            EventKind::TurnsEvicted { ids: _, reclaimed_tokens, marker, rescue } => {
                let evicted_topics: Vec<String> = marker.spans.iter()
                    .map(|s| s.topic_hint.clone())
                    .collect();
                let rescue = rescue.as_ref().map(|r| RescueSummary {
                    goal_text: r.goal_text.clone(),
                    weight: r.weight.round() as u32,
                    rescued: r.survivors.iter().map(|s| {
                        RescuedTurnSummary {
                            topic_hint: s.topic_hint.clone(),
                            bump_milli: (s.rescue_bump * 1000.0).round() as u32,
                        }
                    }).collect(),
                });
                out.push(ChatMsg::Evicted {
                    reclaimed_tokens: *reclaimed_tokens,
                    evicted_topics,
                    rescue,
                    ts: e.ts,
                });
            }
            EventKind::TurnsReadmitted { .. }
            | EventKind::DirectiveReasserted { .. } => {
                // Metadata marker; not a conversation item.
            }
```

> **Note:** The `flush` call must precede this arm (same as other event kinds
> that produce a `ChatMsg`). Check that the `flush` call at projection.rs:292
> still runs after the loop. The existing `TurnsEvicted` arm is inside the
> `for e in events` loop — the `flush` is after the loop. The new arm pushes
> directly to `out` inside the loop, which is the same pattern as `UserMessage`
> and `AssistantMessage` (they call `flush` before pushing). Add a
> `flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());`
> call before the `out.push(ChatMsg::Evicted { ... })`.

- [ ] **Step 4: Update the `conversation_skips_evicted_turns` test**

At projection.rs:746, the test asserts `msgs.len() == 1`. After the change,
`TurnsEvicted` produces a `ChatMsg::Evicted` row, so the count becomes 2.
Update:

```rust
        let msgs = conversation(&events);
        assert_eq!(msgs.len(), 2); // "new" user message + eviction chip
        assert!(matches!(&msgs[0], ChatMsg::User { text, .. } if text == "new"));
        assert!(matches!(&msgs[1], ChatMsg::Evicted { .. }));
```

Also add `rescue: None,` to the test's `TurnsEvicted` literal at projection.rs:761.

- [ ] **Step 5: Add `Evicted` arm to `map_msg` (agent.rs:448)**

After the `Question` arm (agent.rs:476–521), add:

```rust
        ChatMsg::Evicted { .. } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: String::new(),
            tool_calls: vec![],
            tool_name: None,
            tool_call_id: None,
        },
```

- [ ] **Step 6: Filter `ChatMsg::Evicted` in `build_request_with_thinking` (agent.rs:570)**

Change:

```rust
        messages: zoid_core::projection::conversation_for_branch(events.iter(), active_branch)
            .into_iter()
            .map(map_msg)
            .collect(),
```

to:

```rust
        messages: zoid_core::projection::conversation_for_branch(events.iter(), active_branch)
            .into_iter()
            .filter(|m| !matches!(m, zoid_core::projection::ChatMsg::Evicted { .. }))
            .map(map_msg)
            .collect(),
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p zoid-core -- conversation_skips 2>&1 | tail -5`
Expected: PASS (updated test).

Run: `cargo test -p zoid --features local-embed -- preflight 2>&1 | grep "test " | head -5`
Expected: PASS (eviction events now produce `ChatMsg::Evicted`, but the model
filter strips them).

- [ ] **Step 8: Build the workspace**

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/projection.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid-core): ChatMsg::Evicted variant + model-request filter

Projection emits ChatMsg::Evicted for TurnsEvicted (instead of
skipping). RescueSummary/RescuedTurnSummary are Eq-compatible
projection types (f32 → u32 milli). build_request_with_thinking
filters Evicted before map_msg (model never sees eviction chips).
map_msg gains inert Evicted arm (defense-in-depth). Update
conversation_skips_evicted_turns test."
```

---

### Task 5: TUI rendering — chip + detail breakdown

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs`

- [ ] **Step 1: Add `ChatMsg::Evicted` rendering at Normal zoom**

In `build_conversation` (chat.rs, the `match m` block around line 430), after
the `ChatMsg::Delegated` arm, add:

```rust
            ChatMsg::Evicted { reclaimed_tokens, evicted_topics, rescue, ts: _ } => {
                let count = evicted_topics.len();
                let reclaimed_k = format_tokens(*reclaimed_tokens);
                let mut spans = vec![
                    Span::styled(
                        format!("{} evicted {} turns · {} reclaimed",
                            glyph::COLLAPSED, count, reclaimed_k),
                        Style::new().fg(color::DIM),
                    ),
                ];
                if let Some(r) = rescue {
                    spans.push(Span::styled(
                        format!(" · {} rescued", r.rescued.len()),
                        Style::new().fg(color::OK),
                    ));
                }
                lines.push(Line::from(spans));
            }
```

> **Note:** `format_tokens` may not exist — check if there's a helper for
> formatting token counts (e.g. "3.2k"). If not, inline:
> `let reclaimed_k = if *reclaimed_tokens >= 1000 { format!("{:.1}k", *reclaimed_tokens as f64 / 1000.0) } else { format!("{}", reclaimed_tokens) };`

- [ ] **Step 2: Add `ChatMsg::Evicted` rendering at Detail zoom**

In `conversation_view` (chat.rs, the `Zoom::Detail` match block around line 920),
after the `ChatMsg::Delegated` arm, add:

```rust
            ChatMsg::Evicted { reclaimed_tokens, evicted_topics, rescue, ts: _ } => {
                let count = evicted_topics.len();
                let reclaimed_k = /* same format as Normal */;
                // Header line
                out.push(Line::from(vec![
                    Span::styled(
                        format!("{} evicted {} turns · {} reclaimed",
                            glyph::EXPANDED, count, reclaimed_k),
                        Style::new().fg(color::DIM),
                    ),
                    if let Some(r) = rescue {
                        Span::styled(
                            format!(" · {} rescued", r.rescued.len()),
                            Style::new().fg(color::OK),
                        )
                    } else {
                        Span::raw("")
                    },
                ]));
                // Breakdown
                if let Some(r) = rescue {
                    out.push(Line::from(vec![
                        Span::styled("    goal: ", Style::new().fg(color::DIM)),
                        Span::styled(&r.goal_text, Style::new()),
                    ]));
                    out.push(Line::from(vec![
                        Span::styled("    weight: ", Style::new().fg(color::DIM)),
                        Span::styled(format!("{}", r.weight), Style::new()),
                    ]));
                    out.push(Line::from(Span::styled(
                        "    rescued (kept):", Style::new().fg(color::DIM),
                    )));
                    for s in &r.rescued {
                        let bump = s.bump_milli as f64 / 1000.0;
                        out.push(Line::from(vec![
                            Span::styled("      · ", Style::new().fg(color::DIM)),
                            Span::styled(&s.topic_hint, Style::new()),
                            Span::styled(format!(" (bump +{:.1})", bump), Style::new().fg(color::OK)),
                        ]));
                    }
                }
                out.push(Line::from(Span::styled(
                    "    evicted:", Style::new().fg(color::DIM),
                )));
                for topic in evicted_topics {
                    out.push(Line::from(vec![
                        Span::styled("      · ", Style::new().fg(color::DIM)),
                        Span::styled(topic, Style::new()),
                    ]));
                }
            }
```

- [ ] **Step 3: Add `ChatMsg::Evicted` to the Summary zoom (invisible)**

In `digests()` (zoom.rs or chat.rs), `ChatMsg::Evicted` should produce no
digest line. Check if `digests` has an exhaustive match — if so, add:

```rust
ChatMsg::Evicted { .. } => { /* invisible at Summary zoom */ }
```

- [ ] **Step 4: Run TUI snapshot tests**

Run: `cargo insta test -p zoid-tui 2>&1 | tail -20`
Expected: Some snapshots may change if test seeds include `TurnsEvicted` events.
Review the diffs with `cargo insta review` or accept with
`cargo insta test --accept -p zoid-tui` after confirming the changes are
eviction-chip-only.

- [ ] **Step 5: Run the full release gate**

Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat(tui): eviction chip at Normal zoom + breakdown at Detail

ChatMsg::Evicted renders as a one-line chip at Normal (evicted N turns
· Xk reclaimed · N rescued) and a full indented breakdown at Detail
(goal, weight, rescued turns with bump values, evicted turns with
topic hints). Invisible at Summary zoom."
```

---

## Self-Review

Run after all tasks: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
(AGENTS.md release gate). Confirm:
- `rescue_is_none_when_goal_empty` and the extended
  `relevant_old_turn_survives_while_newer_offgoal_is_evicted` pass (T1).
- All 14 `TurnsEvicted` literals updated; `EventKind`/`Event` `Eq`+`Hash` drop
  safe — round-trip tests pass (T2).
- `preflight_rescues_relevant_old_turn_over_newer_offgoal` still passes; the
  `TurnsEvicted` event carries `rescue: Some(...)` (T3).
- `conversation_skips_evicted_turns` updated to `len == 2`; `map_msg` compiles
  with the `Evicted` arm; the model-request filter strips `Evicted` (T4).
- TUI snapshots accepted; eviction chip renders at Normal, breakdown at Detail
  (T5).
- `EvictionPlan` `Eq` drop safe — `bounded_reach_weight_zero_is_pure_recency`
  (eviction.rs:935) still passes (`assert_eq!` needs `PartialEq + Debug`).