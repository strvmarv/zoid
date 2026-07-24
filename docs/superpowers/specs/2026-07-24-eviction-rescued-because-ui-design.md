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

`EventKind` derives `Serialize, Deserialize` — `RescueRationale` and
`RescuedTurn` must also derive them. `EventKind` does not derive `Eq` (it can't
— `QuestionKind::ModeMapping` carries a `Box<ModeMapping>`), so the `f32`
fields are fine.

### 2.3 `ChatMsg::Evicted` + `RescueSummary` (projection.rs)

```rust
/// An eviction wave — chip at Normal zoom, breakdown at Detail.
Evicted {
    reclaimed_tokens: u64,
    evicted_count: usize,
    rescue: Option<RescueSummary>,
    ts: i64,
},
```

`RescueSummary` is the projection-friendly (Eq-compatible) version of
`RescueRationale` — drops `f32` scores, converts bumps to integer milli-units,
drops `Ulid`s:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescueSummary {
    pub goal_text: String,
    pub weight: u16,           // rounded — display only
    pub rescued: Vec<RescuedTurnSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuedTurnSummary {
    pub topic_hint: String,
    pub bump_milli: u16,       // bump × 1000, rounded (e.g. 8400 = +8.4)
}
```

All fields are `Eq`-compatible, so `ChatMsg` retains its `Eq` derive. The
projection converts `RescueRationale` → `RescueSummary` at projection time:
`weight` rounds to `u16`, each survivor's `rescue_bump` rounds to
`(bump * 1000.0).round() as u16`, `ids` are dropped (topic hints suffice).

---

## 3. Projection change

`conversation_for_branch` (and `conversation`) currently skips `TurnsEvicted`
as a metadata marker (projection.rs:281–286). Instead, emit
`ChatMsg::Evicted`:

```
EventKind::TurnsEvicted { ids, reclaimed_tokens, marker, rescue } => {
    // flush pending text/calls first (same as other event kinds)
    let evicted_count = marker.spans.len();
    let rescue = rescue.as_ref().map(|r| RescueSummary {
        goal_text: r.goal_text.clone(),
        weight: r.weight.round() as u16,
        rescued: r.survivors.iter().filter(|s| s.rescue_bump > 0.0).map(|s| {
            RescuedTurnSummary {
                topic_hint: s.topic_hint.clone(),
                bump_milli: (s.rescue_bump * 1000.0).round() as u16,
            }
        }).collect(),
    });
    out.push(ChatMsg::Evicted {
        reclaimed_tokens: *reclaimed_tokens,
        evicted_count,
        rescue,
        ts: /* the event's timestamp */,
    });
}
```

Survivors with `rescue_bump == 0.0` are filtered out of `rescued` — they were
kept by recency, not by rescue. Only turns with a non-zero bump appear in the
"rescued" list.

`TurnsReadmitted` stays a metadata marker (no `ChatMsg` variant — re-admission
is a recall-driven action, not a curation event the user needs to see inline).

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

The `evicted:` list comes from `marker.spans` (already on `ChatMsg::Evicted` as
`evicted_count` — but the Detail view needs the topic hints, so the `ChatMsg::Evicted`
variant must also carry the evicted topic hints). Add `evicted_topics: Vec<String>`
to `ChatMsg::Evicted`:

```rust
Evicted {
    reclaimed_tokens: u64,
    evicted_count: usize,
    evicted_topics: Vec<String>,      // topic hints of evicted turns
    rescue: Option<RescueSummary>,
    ts: i64,
},
```

All `Eq`-compatible. The projection extracts them from `marker.spans`.

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
  └─ constructs TurnsEvicted { ids, reclaimed_tokens, marker, rescue }
     └─ rescue: plan.rescue (moved before the plan.turns loop consumes turns)
  └─ Note: must extract plan.rescue BEFORE `for t in plan.turns` moves the vec

projection (projection.rs)
  └─ TurnsEvicted → ChatMsg::Evicted
     └─ RescueRationale → RescueSummary (f32 → u16 milli, drop Ulids)
     └─ marker.spans → evicted_topics (Vec<String>)

TUI (chat.rs)
  └─ Normal: chip (one line)
  └─ Detail: chip + breakdown (goal, weight, rescued turns, evicted turns)
  └─ Summary/Overview: invisible
```

---

## 6. Cross-crate impact

- **`eviction.rs` (zoid-core)** — `RescueRationale`, `RescuedTurn` structs;
  `EvictionPlan.rescue` field; `EvictionPlan` drops `Eq`; `plan_evictions`
  populates `rescue`. All existing `EvictionPlan { turns: ... }` literals must
  add `rescue: None` (test helpers, `Default`).
- **`event.rs` (zoid-core)** — `TurnsEvicted` gains `rescue` field; all
  `TurnsEvicted { ... }` literals must add `rescue: None` (emit_eviction in
  agent.rs, test event constructors).
- **`projection.rs` (zoid-core)** — `ChatMsg::Evicted` variant; `RescueSummary`,
  `RescuedTurnSummary` structs; `TurnsEvicted` no longer skipped. All exhaustive
  `match ChatMsg` arms in `zoid-tui` and `zoid-core` must add an `Evicted` arm.
- **`chat.rs` (zoid-tui)** — Normal + Detail rendering for `ChatMsg::Evicted`.
  Snapshot tests with eviction events in the seed will change (new rows appear).
- **`agent.rs` (zoid)** — `emit_eviction` passes `plan.rescue` into the event.
  The `preflight_gate` tracing line stays (log-level visibility is still useful).
- **`goal_text` threading** — `plan_evictions` needs the goal text to populate
  `RescueRationale.goal_text`. Today `goal_text` is called in `preflight_gate`
  (agent.rs:2764), not inside `plan_evictions`. Two options: (a) pass `goal_text`
  as a field on `GoalContext` so `plan_evictions` can read it, or (b) have
  `preflight_gate` attach the rationale after `plan_evictions` returns. Option
  (a) is cleaner — add `pub goal_text: String` to `GoalContext` (already has
  `goal`, `vecs`, `weight`). `GoalContext` is `#[derive(Debug, Default, Clone)]`
  (not `Eq`), so adding a `String` field is fine.
- `cargo build --workspace && cargo test --workspace` after each task.

---

## 7. Testing

### zoid-core (pure)

- **`plan_evictions` populates rescue when goal non-empty:** extend
  `relevant_old_turn_survives_while_newer_offgoal` — assert `plan.rescue.is_some()`,
  the rescued turn (id 1) is in `rescue.survivors` with `rescue_bump > 0`.
- **`plan_evictions` rescue is None when goal empty:** assert on
  `empty_goalcontext_is_byte_identical_to_recency`'s plan.
- **`RescueRationale` carries correct data:** assert `goal_text`, `weight`, and
  specific `base_score` / `rescue_bump` / `keep_score` values for the rescue test.
- **Projection:** `TurnsEvicted` with `rescue: Some(...)` → `ChatMsg::Evicted`
  with `rescue: Some(RescueSummary {...})`. Verify `f32 → u16 milli` conversion.
- **Projection:** `TurnsEvicted` with `rescue: None` → `ChatMsg::Evicted` with
  `rescue: None`.
- **`ChatMsg` still derives `Eq`** — all new fields are `Eq`-compatible.
- **`EvictionPlan` `Eq` drop safe** — `bounded_reach_weight_zero_is_pure_recency`
  uses `assert_eq!` (needs `PartialEq + Debug`, not `Eq`); no `Eq`-bound consumer.

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