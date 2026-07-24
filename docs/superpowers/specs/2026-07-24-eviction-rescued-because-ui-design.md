# Eviction "Rescued Because…" UI — Design

> **Status:** DESIGN APPROVED (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** `docs/superpowers/specs/2026-07-24-acm-followups-roadmap.md` (item 2).
> **Builds on:** `docs/superpowers/specs/2026-07-23-acm-relevance-rescued-eviction-design.md`
> (Slice-4b, shipped — rescue is active but tracing-only).

---

## 1. Goal & scope

Surface the rescue rationale to the user: which turns were kept because of
relevance, what their rescue scores were, and what goal text drove the decision.
Today rescue is invisible — `TurnsEvicted` is skipped in the conversation
projection, and the only artifact is the system-prompt breadcrumb (model-only)
and a `tracing::info!` line (log-only).

**In scope:**
- `EvictionPlan` gains `rescue: Option<RescueRationale>` — survivor data
  (candidates kept, their `base_score` / `rescue_bump` / `keep_score`) plus the
  `goal_text` and `weight` that drove the decision.
- `TurnsEvicted` event gains `rescue: Option<RescueRationale>` — so the rationale
  survives into the event log.
- New `ChatMsg::Evicted` variant — the projection emits it for `TurnsEvicted`
  events (instead of skipping them as metadata markers).
- TUI rendering: a one-line chip at Normal zoom, a full indented breakdown at
  Detail zoom. Invisible at Summary and Overview.

**Out of scope:**
- Summary-zoom eviction markers (a refinement — eviction is invisible at Summary
  this slice, same as today).
- Undo/recall UI for eviction (existing `recall` tool handles this).
- Per-event-ID display in the TUI (topic hints suffice for user-facing rationale).

---

## 2. Data model

### 2.1 `RescueRationale` + `RescuedTurn` (eviction.rs)

`plan_evictions` already computes `bump[i]` and `keep_score` for every
candidate. Survivors are the candidates NOT in `plan.turns`. The rationale
captures them:

```rust
/// Per-turn rescue rationale for candidates that were *kept* (not evicted).
/// Present only when rescue was active (non-empty goal).
#[derive(Debug, Clone, PartialEq)]
pub struct RescueRationale {
    /// The goal text that drove the rescue decision.
    pub goal_text: String,
    /// The weight used (after clamping, from `resolve_rescue_weight`).
    pub weight: f32,
    /// Candidates that were kept (not evicted) with their scores.
    pub survivors: Vec<RescuedTurn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RescuedTurn {
    pub ids: Vec<Ulid>,
    pub topic_hint: String,
    pub base_score: f32,    // recency score (turn.index)
    pub rescue_bump: f32,   // weight · normalized_relevance
    pub keep_score: f32,    // base_score + rescue_bump
}
```

`EvictionPlan` gains the field:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvictionPlan {
    pub turns: Vec<EvictedTurn>,
    pub rescue: Option<RescueRationale>,
}
```

`EvictionPlan` drops `Eq` (retains `PartialEq`) — `RescueRationale` has `f32`
fields. The `bounded_reach_weight_zero_is_pure_recency` test compares
`EvictionPlan`s with `assert_eq!`, which requires `PartialEq + Debug`, not
`Eq`. Verified: no `Eq`-bound consumer of `EvictionPlan` exists.

`plan_evictions` populates `rescue` when `ctx.goal` is non-empty. The survivors
are `candidates` not in `plan.turns`, with their `base_score` (`scorer.score`),
`rescue_bump` (`bump[i]`), and `keep_score` (sum). The `goal_text` comes from
the caller (the `preflight_gate` already has it). `weight` is `ctx.weight`.

### 2.2 `TurnsEvicted` event (event.rs)

```rust
TurnsEvicted {
    ids: Vec<Ulid>,
    reclaimed_tokens: u64,
    marker: EvictionMarker,
    rescue: Option<RescueRationale>,  // NEW
},
```

`EventKind` (event.rs:69) and `Event` (event.rs:196) both derive
`Eq, Hash, Serialize, Deserialize`. Adding `Option<RescueRationale>` (with
`f32` fields, which do **not** impl `Eq` or `Hash`) to the `TurnsEvicted`
variant **breaks the `Eq` and `Hash` derives on both `EventKind` and `Event`**
(transitive). `RescueRationale` and `RescuedTurn` must derive
`Serialize, Deserialize`.

**`Eq` + `Hash` drop blast radius:** `EventKind` and `Event` lose `Eq` and
`Hash`, retain `PartialEq`. Verified safe: no `HashSet<Event>` /
`HashMap<Event, _>` / `BTreeMap`-keyed-on-`Event` usages exist in the
workspace, and no `Event: Eq` or `Event: Hash` trait bounds. The round-trip
tests (`event.rs:258`, `event.rs:303`, `round_trip.rs:39`) use `assert_eq!`,
which requires only `PartialEq + Debug`, not `Eq`. The `Source` enum and
`Provenance` struct (config.rs) derive `Eq + Hash` but are unrelated.

### 2.3 `ChatMsg::Evicted` + `RescueSummary` (projection.rs)

```rust
/// An eviction wave — chip at Normal zoom, breakdown at Detail.
/// Filtered out of the model request path (§3.1).
Evicted {
    reclaimed_tokens: u64,
    evicted_topics: Vec<String>,   // topic hints of evicted turns
    rescue: Option<RescueSummary>,
    ts: i64,
},
```

`evicted_count` is NOT a separate field — it's `evicted_topics.len()`. The
Normal chip uses `evicted_topics.len()` for the count. Carrying both would
invite drift. The projection extracts `evicted_topics` from `marker.spans`.

`RescueSummary` is the projection-friendly (Eq-compatible) version of
`RescueRationale` — drops `f32` scores, converts bumps to integer milli-units,
drops `Ulid`s:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescueSummary {
    pub goal_text: String,
    pub weight: u32,           // rounded — display only
    pub rescued: Vec<RescuedTurnSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuedTurnSummary {
    pub topic_hint: String,
    pub bump_milli: u32,       // bump × 1000, rounded (e.g. 8400 = +8.4)
}
```

All fields are `Eq`-compatible, so `ChatMsg` retains its `Eq` derive.

**`u32` for milli-units, not `u16`:** `bump = weight · normalized` where
`weight ≤ RESCUE_WEIGHT_MAX` (48.0 today, but configurable up to any value
the future `[eviction]` config allows). `bump_milli` as `u16` saturates at
65.535 — fragile if `rescue_weight` is ever set above ~65. Use `u32` for
`bump_milli` and `weight` to foreclose any future config-driven overflow.

The projection converts `RescueRationale` → `RescueSummary` at projection time:
`weight` rounds to `u32`, each survivor's `rescue_bump` rounds to
`(bump * 1000.0).round() as u32`, `ids` are dropped (topic hints suffice).

---

## 3. Projection change

`conversation_for_branch` (and `conversation`) currently skips `TurnsEvicted`
as a metadata marker (projection.rs:281–286). Instead, emit
`ChatMsg::Evicted`:

```
EventKind::TurnsEvicted { ids, reclaimed_tokens, marker, rescue } => {
    // flush pending text/calls first (same as other event kinds)
    let evicted_topics: Vec<String> = marker.spans.iter()
        .map(|s| s.topic_hint.clone()).collect();
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
        ts: /* the event's timestamp */,
    });
}
```

Survivors with `rescue_bump == 0.0` are filtered out **at the `plan_evictions`
source** (not just in the projection) — they were kept by recency, not by
rescue, and including them would bloat the persisted event with off-goal
turns that weren't rescued at all. Only turns with a non-zero bump appear in
`RescueRationale.survivors`. The invariant: `rescue.is_some() <=> 
!ctx.goal.is_empty()`, and `survivors` contains only candidates with
`rescue_bump > 0.0` that were NOT evicted.

**Survivor set computation:** build a `HashSet<Ulid>` of evicted ids from
`plan.turns.iter().flat_map(|t| t.ids.iter().copied())`, then filter
`candidates` by `!t.ids.iter().any(|id| evicted_set.contains(id))` AND
`bump[i] > 0.0`. `group_turns` produces disjoint turns (no id overlap), so the
set membership check is exact.

`TurnsReadmitted` stays a metadata marker (no `ChatMsg` variant — re-admission
is a recall-driven action, not a curation event the user needs to see inline).

### 3.1 Model-request path — filter `ChatMsg::Evicted` out

`conversation_for_branch` feeds two consumers through the same `Vec<ChatMsg>`:
1. **The TUI** — `ProjectionCache.msgs` → `build_conversation` (chat.rs).
2. **The model request** — `build_request_with_thinking` → `map_msg` (agent.rs:447).

`map_msg` is an exhaustive match (agent.rs:448) returning `Message` (not
`Option<Message>`). Adding `ChatMsg::Evicted` breaks it, and mapping it to a
`Message` would inject eviction chips into the model's context window —
breaking the design invariant that the model learns about eviction *only*
through the system-prompt breadcrumb (agent.rs:552), never as inline transcript
rows.

**Fix:** `build_request_with_thinking` (agent.rs:570) filters `ChatMsg::Evicted`
out of the messages before mapping:

```rust
messages: zoid_core::projection::conversation_for_branch(events.iter(), active_branch)
    .into_iter()
    .filter(|m| !matches!(m, ChatMsg::Evicted { .. }))
    .map(map_msg)
    .collect(),
```

`map_msg` still needs an `Evicted` arm (exhaustive match) — it returns an inert
empty assistant message (unreachable in practice because the filter removes
`Evicted` before `map_msg` runs):

```rust
ChatMsg::Evicted { .. } => Message {
    role: zoid_provider::MsgRole::Assistant,
    content: String::new(),
    tool_calls: vec![],
    tool_name: None,
    tool_call_id: None,
},
```

This is defense-in-depth: the filter is the real guard, but `map_msg` must
still compile. The `Question::Approval` arm (agent.rs:493) already uses the
same inert-message pattern for a similar "UI-only, not for the model" case.

---

## 4. TUI rendering

### 4.1 Normal zoom — one-line chip

Following the `Delegated` card pattern (chat.rs:430):

```
  ⑤ evicted 4 turns · 3.2k reclaimed · 2 rescued
```

When `rescue` is `None`:

```
  ⑤ evicted 4 turns · 3.2k reclaimed
```

- `⑤` glyph in `color::DIM` — the context-economy marker (matches existing
  breadcrumb/icon convention).
- `evicted N turns` in `color::WARNING` (amber — context was trimmed).
- `· Xk reclaimed` in `color::DIM`.
- `· N rescued` in `color::OK` (green — positive: turns were protected).

### 4.2 Detail zoom — chip + indented breakdown

```
  ▼ evicted 4 turns · 3.2k reclaimed · 2 rescued
    goal: "implement the relevance rescue scorer"
    weight: 12
    rescued (kept):
      · implement the relevance rescue scorer (bump +8.4)
      · wire it into preflight_gate under pressure (bump +3.2)
    evicted:
      · zulu yankee xray whiskey n3
      · zulu yankee xray whiskey n5
      · zulu yankee xray whiskey n7
      · zulu yankee xray whiskey n9
```

The `evicted:` list comes from `evicted_topics` on `ChatMsg::Evicted` (defined
in §2.3, extracted from `marker.spans` by the projection). The Detail view
iterates `evicted_topics` for the evicted list and `rescue.rescued` for the
rescued list.

### 4.3 Summary and Overview

- **Summary** — eviction chips are invisible. `digests()` groups by `User`
  messages; an eviction isn't a turn. Same as today.
- **Overview** — bypasses the transcript. No change.

---

## 5. Data flow

```
plan_evictions (eviction.rs)
  └─ computes bump[i] + keep_score for every candidate
  └─ EvictionPlan { turns, rescue: Option<RescueRationale> }
     └─ rescue populated when ctx.goal non-empty
     └─ survivors = candidates NOT in plan.turns, with scores

emit_eviction (agent.rs:2851)
  └─ receives EvictionPlan by value
  └─ destructure: let EvictionPlan { turns, rescue } = plan;
     └─ iterate `turns` for ids/spans/reclaimed (no move-order issue)
     └─ pass `rescue` into the TurnsEvicted event
  └─ Note: use destructure at the top, not field-by-field access after a
     consuming loop — avoids the partial-move footgun entirely

projection (projection.rs)
  └─ TurnsEvicted → ChatMsg::Evicted
     └─ RescueRationale → RescueSummary (f32 → u32 milli, drop Ulids)
     └─ marker.spans → evicted_topics (Vec<String>)

build_request_with_thinking (agent.rs:570)
  └─ filters ChatMsg::Evicted out before map_msg (§3.1)

TUI (chat.rs)
  └─ Normal: chip (one line)
  └─ Detail: chip + breakdown (goal, weight, rescued turns, evicted turns)
  └─ Summary/Overview: invisible
```

---

## 6. Cross-crate impact

- **`eviction.rs` (zoid-core)** — `RescueRationale`, `RescuedTurn` structs;
  `EvictionPlan.rescue` field; `EvictionPlan` drops `Eq` (retains `PartialEq`);
  `plan_evictions` populates `rescue` (survivors with `bump > 0.0` only). All
  existing `EvictionPlan { turns: ... }` literals must add `rescue: None`
  (test helpers, `Default`). `GoalContext` gains `pub goal_text: String` field;
  all `GoalContext { ... }` literals must add `goal_text: String::new()` (test
  sites: eviction.rs ~line 346, 904, 922; production site: agent.rs:2797).
  `Default` for `GoalContext` yields `goal_text: String::default()` = `""`,
  consistent with `rescue: None` (empty goal ⇒ no rescue).
- **`event.rs` (zoid-core)** — `TurnsEvicted` gains `rescue` field. `EventKind`
  (line 69) and `Event` (line 196) both drop `Eq` **and `Hash`** (retains
  `PartialEq`). All `TurnsEvicted { ... }` literals must add `rescue: None`
  (emit_eviction in agent.rs, test event constructors in event.rs and agent.rs
  test modules).
- **`projection.rs` (zoid-core)** — `ChatMsg::Evicted` variant; `RescueSummary`,
  `RescuedTurnSummary` structs; `TurnsEvicted` no longer skipped. All exhaustive
  `match ChatMsg` arms in `zoid-tui`, `zoid-core`, **and `zoid` (`map_msg` at
  agent.rs:448)** must add an `Evicted` arm. The `conversation_skips_evicted_turns`
  test (projection.rs:746) must be updated — it currently asserts `msgs.len() == 1`
  because `TurnsEvicted` is skipped; after this change it emits a `ChatMsg::Evicted`
  row, so the assertion becomes `len == 2`.
- **`chat.rs` (zoid-tui)** — Normal + Detail rendering for `ChatMsg::Evicted`.
  Snapshot tests with eviction events in the test seed will change (new rows appear).
- **`agent.rs` (zoid)** — `emit_eviction` destructures `EvictionPlan` to extract
  `rescue` before consuming `turns`. `build_request_with_thinking` (agent.rs:570)
  filters `ChatMsg::Evicted` out of messages before `map_msg`. `map_msg` gains an
  `Evicted` arm (inert empty assistant message, defense-in-depth). The
  `preflight_gate` tracing line stays. The `GoalContext` construction at
  agent.rs:2797 must add `goal_text: text.clone()` (the `text` variable is in
  scope from agent.rs:2770 and still needed for the embed call at agent.rs:2781).
- `cargo build --workspace && cargo test --workspace` after each task.

---

## 7. Testing

### zoid-core (pure)

- **`plan_evictions` populates rescue when goal non-empty:** extend
  `relevant_old_turn_survives_while_newer_offgoal_is_evicted` (eviction.rs:844)
  — assert `plan.rescue.is_some()`, the rescued turn (id 1) is in
  `rescue.survivors` with `rescue_bump > 0`.
- **`plan_evictions` rescue is None when goal empty:** assert on
  `empty_goalcontext_is_byte_identical_to_recency`'s plan.
- **`RescueRationale` carries correct data:** assert `goal_text`, `weight`, and
  specific `base_score` / `rescue_bump` / `keep_score` values for the rescue test.
- **Projection:** `TurnsEvicted` with `rescue: Some(...)` → `ChatMsg::Evicted`
  with `rescue: Some(RescueSummary {...})`. Verify `f32 → u32 milli` conversion.
- **Projection:** `TurnsEvicted` with `rescue: None` → `ChatMsg::Evicted` with
  `rescue: None`.
- **`ChatMsg` still derives `Eq`** — all new fields are `Eq`-compatible.
- **`EvictionPlan` `Eq` drop safe** — `bounded_reach_weight_zero_is_pure_recency`
  (eviction.rs:935) uses `assert_eq!` (needs `PartialEq + Debug`, not `Eq`);
  no `Eq`-bound consumer. Multiple `assert_eq!` sites (eviction.rs:832, :911,
  :935) all survive.
- **`EventKind`/`Event` `Eq` + `Hash` drop safe** — no `HashSet`/`HashMap`/
  `BTreeMap` keyed on `Event`/`EventKind`; no `Eq`/`Hash` trait bounds. Round-trip
  tests (event.rs:258, :303, round_trip.rs:39) use `assert_eq!` (`PartialEq`).
- **`conversation_skips_evicted_turns`** (projection.rs:746) — update assertion
  from `msgs.len() == 1` to `msgs.len() == 2` (the `TurnsEvicted` now produces a
  `ChatMsg::Evicted` row). Verify the second message is `ChatMsg::Evicted`.

### zoid-tui

- **Normal zoom:** `ChatMsg::Evicted` renders a one-line chip. With rescue:
  includes `· N rescued`. Without: omits it.
- **Detail zoom:** chip + indented breakdown with goal, weight, rescued turns
  (bump values formatted as `+N.N`), evicted turns (topic hints).
- **Summary zoom:** `ChatMsg::Evicted` invisible (not in digest).
- **Snapshot tests:** existing snapshots with eviction events in the test seed
  will change — new `ChatMsg::Evicted` rows appear where `TurnsEvicted` was
  previously skipped. Accept these with `cargo insta test --accept`.

### zoid (integration)

- **`preflight_rescues_relevant_old_turn_over_newer_offgoal`** still passes, and
  the resulting `TurnsEvicted` event carries `rescue: Some(...)`.
- **`preflight_without_embedder_evicts_the_old_turn`** still passes, and the
  `TurnsEvicted` event carries `rescue: None`.