# EventLog: cheap-clone + clearable event log (#6a + #6b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the per-turn event-log snapshot O(n) refcount bumps instead of O(total bytes) (#6a), and free the raw `ToolResult.output` of compacted results from RAM (#6b), without changing rendered output, recall, readmit, or token accounting.

**Architecture:** Introduce an `EventLog(Vec<Arc<Event>>)` newtype in the `zoid` bin. `snapshot()` clones the outer `Vec` (refcount bumps only) for the per-turn seed; `clear_tool_output(id)` swaps a single event's `Arc` slot for one with an empty body. zoid-core's projection functions migrate from `&[Event]` to `impl IntoIterator<Item = &Event>` so they can be fed either an `EventLog::iter()` or (unchanged) a `&Vec<Event>` — keeping the core pure and every existing call site compiling.

**Tech Stack:** Rust workspace (crates: `zoid-core` pure, `zoid` effectful bin). `std::sync::Arc`, `ulid::Ulid`. Test runner: `cargo test`.

## Global Constraints

Every task's requirements implicitly include this section. Values copied verbatim from `docs/superpowers/specs/2026-07-04-eventlog-clone-clear-design.md`.

- **zoid-core stays pure** — `EventLog` and all `Arc` sharing live in the `zoid` bin. The core keeps a borrowed-`&Event` (iterator) projection API; **no `Arc` dependency** enters zoid-core.
- **Clear only compacted bodies** — never uncompacted evicted bodies (a readmitted uncompacted turn is rendered from its raw body).
- **Do not alter on-disk store contents, recall, or readmit behavior** — raw bodies stay persisted in SQLite; recall reads from SQLite.
- **`plan_evictions` must account a compacted turn by its in-context size** = `estimate_tokens(summary)` (matching `context.rs:315`). **Not** the raw body, **not** zero, and **not** the pre-compaction `original_tokens` field.
- **Commit messages: NO `Co-Authored-By` / co-author trailer** (user rule).
- **Final gate:** `cargo test` (workspace) green; introduce **no new** clippy/fmt warnings in feature-touched code. The repo is not clippy/fmt-clean at baseline — the bar is "no new issues in files this plan touches."
- **Line refs** in this plan are against `f50191b` (the commit the branch starts from). If a number has drifted, match on the quoted code, not the number.

---

## File Structure

- **Create:** `crates/zoid/src/eventlog.rs` — the `EventLog` newtype + its unit tests. One responsibility: own the in-memory log and its cheap-clone / clear-body operations. Declared as `pub mod eventlog;` in `crates/zoid/src/lib.rs` (lib, so `agent.rs`/`subagent.rs` can import it).
- **Modify:** `crates/zoid/src/lib.rs` — add `pub mod eventlog;`.
- **Modify:** `crates/zoid/src/main.rs` — change `App.events` type, both seed sites, projection call sites, the `AgentUpdate::Appended` handler (#6b trigger), the resume-load path.
- **Modify:** `crates/zoid/src/agent.rs` — the turn's working-set type + 5 helper signatures + the push site.
- **Modify:** `crates/zoid/src/subagent.rs` — the context-events parameter type.
- **Modify (zoid-core, signature migration only):** `crates/zoid-core/src/projection.rs`, `context.rs`, `eviction.rs`, `economy.rs`, `tasks.rs`.

---

## The zoid-core migration recipe (used by Task 2)

Every migrated function follows this exact mechanical recipe. It relies on two facts:

1. `&Vec<Event>` and `&[Event]` already satisfy `impl IntoIterator<Item = &Event>`, so **existing call sites and existing zoid-core tests keep compiling unchanged**.
2. Iterating a `Vec<&Event>` by value yields `&Event` (identical to iterating `&[Event]`); iterating `&Vec<&Event>` / `&[&Event]` yields `&&Event`, and Rust auto-derefs field access (`e.kind`, `e.id`, `e.ts`, `e.branch`, `&e.kind`) through the extra reference — so field-access-only bodies need **no edits**.

**Recipe:**
- Change the parameter `events: &[Event]` → `events: impl IntoIterator<Item = &'a Event>` and add `<'a>` to the function's generics. (Other parameters keep their types.)
- Add as the first line: `let events: Vec<&Event> = events.into_iter().collect();`
- **Single consume-loop** (`for e in events { … }` and `events` is not used again): leave the loop as-is; it now yields `&Event`.
- **Function uses `events` more than once** (a sub-call plus a loop, or two loops): after the collect, add `let visible: &[&Event] = &events;`, feed any sub-call `events.iter().copied()` (yields `&Event`), and iterate `for e in visible` (yields `&&Event`; field access auto-derefs).
- **`.rev()` / other `DoubleEndedIterator` needs:** iterate the collected `Vec` (`events.into_iter()…`), which is double-ended.

Do **not** hand-edit function bodies beyond what the recipe specifies. After applying it, `cargo test -p zoid-core` must be green and `cargo build` (workspace) must still succeed with all call sites unchanged.

---

## Task 1: `EventLog` newtype + unit tests

**Files:**
- Create: `crates/zoid/src/eventlog.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod eventlog;` alongside the other `pub mod` declarations at lines 4-10)
- Test: inline `#[cfg(test)] mod tests` in `crates/zoid/src/eventlog.rs`

**Module placement (decided):** the `zoid` package has both a lib (`src/lib.rs`, crate name `zoid`) and a bin (`src/main.rs`) — separate crates. `agent.rs` and `subagent.rs` are **lib** modules (`pub mod agent;` in `lib.rs`). Since a lib module cannot import from the bin, `EventLog` lives in the **lib**: declare `pub mod eventlog;` in `lib.rs`. Reference it as **`crate::eventlog::EventLog` from lib modules** (`agent.rs`, `subagent.rs`) and as **`zoid::eventlog::EventLog` from the bin** (`main.rs`, which sees the lib as an external crate — cf. `use zoid::agent::…` at `main.rs:25`).

**Interfaces:**
- Consumes: `zoid_core::event::{Event, EventKind}`, `ulid::Ulid`, `std::sync::Arc`.
- Produces (later tasks rely on these exact signatures):
  - `pub struct EventLog` (wraps `Vec<Arc<Event>>`)
  - `pub fn new() -> EventLog` + `impl Default`
  - `pub fn from_vec(events: Vec<Event>) -> EventLog`
  - `pub fn push(&mut self, e: Event)`
  - `pub fn iter(&self) -> impl Iterator<Item = &Event> + '_`
  - `pub fn len(&self) -> usize` + `pub fn is_empty(&self) -> bool`
  - `pub fn snapshot(&self) -> EventLog`
  - `pub fn clear_tool_output(&mut self, tool_id: &str)`
  - `pub fn clear_compacted_bodies(&mut self)`

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid/src/eventlog.rs` with the test module (implementation stubs to follow in Step 3):

```rust
//! The in-memory event log. Wraps `Vec<Arc<Event>>` so that (a) handing a turn
//! its snapshot is O(n) refcount bumps, not O(total bytes) of body copies (#6a);
//! and (b) an individual tool-result body can be swapped out in place — replace
//! the `Arc` slot — without disturbing snapshots already handed to in-flight
//! turns (they hold the old, immutable `Arc`) (#6b).

use std::sync::Arc;

use zoid_core::event::{Event, EventKind};

#[derive(Debug, Clone, Default)]
pub struct EventLog(Vec<Arc<Event>>);

// (implementation added in Step 3)

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

    fn tool_result(tool_id: &str, output: &str) -> Event {
        Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResult {
                id: tool_id.to_string(),
                name: "bash".to_string(),
                output: output.to_string(),
                is_error: false,
            },
        )
    }

    fn compacted(tool_id: &str, summary: &str) -> Event {
        Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResultCompacted {
                id: tool_id.to_string(),
                summary: summary.to_string(),
                original_tokens: 999,
            },
        )
    }

    #[test]
    fn push_iter_len() {
        let mut log = EventLog::new();
        assert!(log.is_empty());
        log.push(tool_result("t1", "hello"));
        log.push(tool_result("t2", "world"));
        assert_eq!(log.len(), 2);
        let outputs: Vec<&str> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(outputs, vec!["hello", "world"]);
    }

    #[test]
    fn snapshot_shares_without_copying_bodies() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "a big body"));
        let snap = log.snapshot();
        // #6a: snapshot shares the same Arc allocations (no deep copy).
        assert_eq!(log.len(), snap.len());
        for (a, b) in log.arcs().iter().zip(snap.arcs().iter()) {
            assert!(Arc::ptr_eq(a, b), "snapshot must share the Arc, not clone the Event");
            assert_eq!(Arc::strong_count(a), 2, "one refcount bump per shared event");
        }
    }

    #[test]
    fn clear_tool_output_empties_only_the_target() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "KEEP THIS"));
        log.push(tool_result("t2", "CLEAR THIS"));
        log.clear_tool_output("t2");
        let bodies: Vec<&str> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bodies, vec!["KEEP THIS", ""], "only t2's body is emptied");
    }

    #[test]
    fn clear_tool_output_is_noop_for_absent_or_non_toolresult() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "body"));
        log.push(compacted("t1", "sum")); // a ToolResultCompacted, not a ToolResult
        let before: Vec<Arc<Event>> = log.arcs().to_vec();
        log.clear_tool_output("does-not-exist");
        // Absent id → nothing changes (same Arc allocations).
        for (a, b) in before.iter().zip(log.arcs().iter()) {
            assert!(Arc::ptr_eq(a, b));
        }
    }

    #[test]
    fn snapshot_is_unaffected_by_later_clear() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "ORIGINAL"));
        let snap = log.snapshot();
        log.clear_tool_output("t1");
        // The in-flight snapshot still sees the original body (immutable share).
        let snap_body = snap.iter().find_map(|e| match &e.kind {
            EventKind::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        });
        assert_eq!(snap_body.as_deref(), Some("ORIGINAL"));
    }

    #[test]
    fn clear_compacted_bodies_clears_exactly_the_compacted() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "COMPACTED BODY"));
        log.push(tool_result("t2", "LIVE BODY"));
        log.push(compacted("t1", "tiny summary")); // marks t1 compacted
        log.clear_compacted_bodies();
        let bodies: Vec<(&str, &str)> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { id, output, .. } => Some((id.as_str(), output.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(bodies, vec![("t1", ""), ("t2", "LIVE BODY")]);
    }
}
```

Note: `arcs()` is a `#[cfg(test)]`-only accessor added in Step 3 so tests can assert on `Arc` identity without exposing internals in the public API.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib eventlog 2>&1 | head -40`
Expected: FAIL — the `impl EventLog` block does not exist yet (compile errors: `no function or associated item named 'new'`, etc.).

- [ ] **Step 3: Implement `EventLog`**

Insert the implementation between the struct definition and the `#[cfg(test)]` module in `crates/zoid/src/eventlog.rs`:

```rust
impl EventLog {
    pub fn new() -> Self {
        EventLog(Vec::new())
    }

    /// Build a log from owned events (e.g. a session snapshot loaded on resume).
    pub fn from_vec(events: Vec<Event>) -> Self {
        EventLog(events.into_iter().map(Arc::new).collect())
    }

    pub fn push(&mut self, e: Event) {
        self.0.push(Arc::new(e));
    }

    pub fn iter(&self) -> impl Iterator<Item = &Event> + '_ {
        self.0.iter().map(|a| a.as_ref())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// #6a: a per-turn snapshot. Clones the outer `Vec` only — each element is
    /// an `Arc` refcount bump, never an `Event` body copy.
    pub fn snapshot(&self) -> EventLog {
        EventLog(self.0.clone())
    }

    /// #6b: replace the `ToolResult` whose inner tool-call `id` == `tool_id`
    /// with an `Arc` whose `output` is empty. No-op if `tool_id` is absent or
    /// the matched event is not a `ToolResult`. Snapshots already handed out
    /// hold the old `Arc` and are unaffected.
    pub fn clear_tool_output(&mut self, tool_id: &str) {
        for slot in self.0.iter_mut() {
            if let EventKind::ToolResult { id, name, is_error, .. } = &slot.kind {
                if id == tool_id {
                    let cleared = Event {
                        kind: EventKind::ToolResult {
                            id: id.clone(),
                            name: name.clone(),
                            output: String::new(),
                            is_error: *is_error,
                        },
                        ..(**slot).clone()
                    };
                    *slot = Arc::new(cleared);
                    return;
                }
            }
        }
    }

    /// #6b resume path: clear the body of every `ToolResult` that has a matching
    /// `ToolResultCompacted` in this log. Keeps reopening a long session from
    /// re-inflating RAM to the pre-#6b footprint.
    pub fn clear_compacted_bodies(&mut self) {
        let compacted: Vec<String> = self
            .0
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResultCompacted { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        for id in compacted {
            self.clear_tool_output(&id);
        }
    }

    #[cfg(test)]
    fn arcs(&self) -> &[Arc<Event>] {
        &self.0
    }
}
```

Then add the module declaration to `crates/zoid/src/lib.rs`, alongside the existing `pub mod` lines (4-10):

```rust
pub mod eventlog;
```

Confirm the `Event` struct exposes public fields `kind`, `id`, `ts`, `branch`, `session_id`, `tokens` (the `..(**slot).clone()` struct-update and `Event { kind, .. }` construction require the fields be constructible from the bin). If `Event` cannot be struct-literal-constructed from outside zoid-core, replace the `clear_tool_output` body with a clone-then-mutate form:

```rust
    pub fn clear_tool_output(&mut self, tool_id: &str) {
        for slot in self.0.iter_mut() {
            let is_match = matches!(&slot.kind, EventKind::ToolResult { id, .. } if id == tool_id);
            if is_match {
                let mut ev = (**slot).clone();
                if let EventKind::ToolResult { output, .. } = &mut ev.kind {
                    output.clear();
                }
                *slot = Arc::new(ev);
                return;
            }
        }
    }
```

(The clone-then-mutate form only needs `Event: Clone` and public `kind`, which the codebase already relies on — prefer it if there is any doubt about field visibility.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib eventlog 2>&1 | tail -20`
Expected: PASS — all 6 tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/eventlog.rs crates/zoid/src/lib.rs
git commit -m "feat(eventlog): Arc-backed EventLog with cheap snapshot + clearable bodies"
```

---

## Task 2: Migrate zoid-core projections to the iterator bound

**Files:**
- Modify: `crates/zoid-core/src/projection.rs:57` (`conversation`)
- Modify: `crates/zoid-core/src/context.rs:165` (`context_window`), `context.rs:173` (`context_window_with`)
- Modify: `crates/zoid-core/src/eviction.rs:50` (`evicted_ids`), `eviction.rs:250` (`plan_evictions`)
- Modify: `crates/zoid-core/src/economy.rs:19` (`token_ledger`), `economy.rs:76` (`churn_timeline`)
- Modify: `crates/zoid-core/src/tasks.rs:49` (`tasks`)
- Test: existing `#[cfg(test)] mod tests` in each file (unchanged — they must still pass)

**Interfaces:**
- Consumes: nothing new.
- Produces: the eight functions above now accept `impl IntoIterator<Item = &'a Event>` instead of `&[Event]`. All existing call sites (which pass `&vec` or `&app.events`) keep compiling because `&Vec<Event>` satisfies the bound. Later tasks feed them `EventLog::iter()`.

Apply **The zoid-core migration recipe** (top of this plan) to each function. The specific per-function shape:

- [ ] **Step 1: Migrate the single-consume-loop functions**

`token_ledger` (`economy.rs:19`), `churn_timeline` (`economy.rs:76`): each is one `for e in events` loop that uses `events` only once. Change the signature and add the collect prelude; leave the loop body untouched.

```rust
// economy.rs — token_ledger
pub fn token_ledger<'a>(events: impl IntoIterator<Item = &'a Event>) -> TokenLedger {
    let events: Vec<&Event> = events.into_iter().collect();
    let mut l = TokenLedger::default();
    for e in events {
        // ... unchanged body ...
    }
    // ... unchanged ...
}
```

```rust
// economy.rs — churn_timeline
pub fn churn_timeline<'a>(events: impl IntoIterator<Item = &'a Event>) -> ChurnTimeline {
    let events: Vec<&Event> = events.into_iter().collect();
    // ... unchanged body (single `for e in events` loop) ...
}
```

`tasks` (`tasks.rs:49`) uses `.iter().rev()`, so collect then iterate the `Vec` (double-ended):

```rust
// tasks.rs — tasks
pub fn tasks<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<TaskItem> {
    let events: Vec<&Event> = events.into_iter().collect();
    events
        .into_iter()
        .filter(|e| e.branch == crate::event::BranchId::default())
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Tasks { items } => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
```

- [ ] **Step 2: Migrate the sub-call + loop functions**

`evicted_ids` (`eviction.rs:50`) is a single consume-loop → same as Step 1's first form:

```rust
// eviction.rs — evicted_ids
pub fn evicted_ids<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashSet<Ulid> {
    let events: Vec<&Event> = events.into_iter().collect();
    let mut set = HashSet::new();
    for e in events {
        // ... unchanged body ...
    }
    set
}
```

`conversation` (`projection.rs:57`) uses `events` for a sub-call (`evicted_ids`) **and** two loops, so use the `visible: &[&Event]` form. Replace lines 57-59:

```rust
pub fn conversation<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<ChatMsg> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let evicted = crate::eviction::evicted_ids(events.iter().copied());
```

The rest of `conversation` is unchanged: `for e in visible` now yields `&&Event`, and every access (`&e.kind`, `e.id`, `e.ts`, `e.branch`) auto-derefs.

`context_window` (`context.rs:165`) delegates — just change its signature and pass through:

```rust
pub fn context_window<'a>(events: impl IntoIterator<Item = &'a Event>) -> ContextWindow {
    context_window_with(events, ContextOverhead::default())
}
```

`context_window_with` (`context.rs:173`) uses a sub-call (`evicted_ids`) + one loop. Replace its first two body lines (`let visible: &[Event] = events;` / `let evicted = crate::eviction::evicted_ids(events);`):

```rust
pub fn context_window_with<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    overhead: ContextOverhead,
) -> ContextWindow {
    let events: Vec<&Event> = events.into_iter().collect();
    let evicted = crate::eviction::evicted_ids(events.iter().copied());
    let visible: &[&Event] = &events;
    // ... unchanged body (single `for e in visible` fold loop) ...
```

- [ ] **Step 3: Migrate `plan_evictions` + its private helper `group_turns`**

`plan_evictions` (`eviction.rs:250`) passes events to `group_turns`. Change `plan_evictions` to the bound, collect, and pass a `&[&Event]` slice to `group_turns`; change `group_turns` (`eviction.rs:198`) to accept `&[&Event]`.

```rust
// eviction.rs — plan_evictions
pub fn plan_evictions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
) -> EvictionPlan {
    if !policy.enabled {
        return EvictionPlan::default();
    }
    let events: Vec<&Event> = events.into_iter().collect();
    // ... unchanged until the group_turns call, which becomes:
    //     let turns = group_turns(&events, &evicted, policy.recent_n);
    // (pass &events — a &[&Event] — instead of the old &[Event])
```

```rust
// eviction.rs — group_turns signature
fn group_turns(events: &[&Event], evicted: &HashSet<Ulid>, recent_n: usize) -> Vec<TurnView> {
```

Inside `group_turns`, `for e in events` now yields `&&Event`; all field accesses (`e.branch`, `&e.kind`, `e.id`) auto-deref, so the body is otherwise unchanged. (The summary-accounting fix lands here in **Task 3** — do not add it yet.)

Also check `evicted_ids` calls inside `plan_evictions`/`eviction.rs` and any other in-crate caller of the migrated fns: pass `.iter().copied()` when the source is now a `Vec<&Event>`, or `&vec` when it is still an owned `Vec<Event>`.

- [ ] **Step 4: Build and run the zoid-core test suite**

Run: `cargo test -p zoid-core 2>&1 | tail -30`
Expected: PASS — all existing zoid-core tests green (they call with `&vec`, which satisfies the new bound; no test edits needed).

Run: `cargo build 2>&1 | tail -20`
Expected: SUCCESS — the bin still compiles (its call sites pass `&app.events`, still `&Vec<Event>`).

If the build flags an unmigrated `&[Event]` consumer that the bin feeds (e.g. `eviction_breadcrumb` at `eviction.rs:68` or `file_contents` at `context.rs:356`), apply the same recipe to it and note it in the commit. If it is only ever called with an owned `&[Event]` internally, leave it.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src
git commit -m "refactor(core): projection fns take impl IntoIterator<Item=&Event>"
```

---

## Task 3: Eviction summary-accounting fix (compacted turns weigh their summary)

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` — `group_turns` (`eviction.rs:198`, post-Task-2 signature `&[&Event]`)
- Test: `#[cfg(test)] mod tests` in `crates/zoid-core/src/eviction.rs`

**Interfaces:**
- Consumes: `group_turns(&[&Event], &HashSet<Ulid>, usize) -> Vec<TurnView>` from Task 2; `crate::economy::estimate_tokens`.
- Produces: `group_turns` now accounts a `ToolResult` whose tool-id has a matching `ToolResultCompacted` by `estimate_tokens(summary)` instead of `event_tokens(&e.kind)`.

**Why:** `ToolResultCompacted` is `is_inert` (`eviction.rs:174`), so a compacted turn's entire ranking weight flows through its `ToolResult` at `event_tokens` (`eviction.rs:189`). Today that reads the **raw** body — over-counting vs. what the request carries (the summary, per `context.rs:315`), and would read ~0 once #6b clears the body. Accounting by summary is correct in both cases.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/zoid-core/src/eviction.rs`:

```rust
#[test]
fn compacted_turn_weighs_summary_not_raw_or_zero() {
    use crate::economy::estimate_tokens;
    use crate::event::{Event, EventKind};
    use std::collections::HashSet;

    let user = Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() });
    // A ToolResult whose body has ALREADY been cleared by #6b (output empty),
    // plus its compaction marker carrying the summary the request actually holds.
    let tr = Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::ToolResult { id: "call-1".into(), name: "bash".into(), output: String::new(), is_error: false },
    );
    let summary = "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".to_string();
    let comp = Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::ToolResultCompacted { id: "call-1".into(), summary: summary.clone(), original_tokens: 4242 },
    );

    let events: Vec<&Event> = vec![&user, &tr, &comp];
    let turns = group_turns(&events, &HashSet::new(), 0);

    assert_eq!(turns.len(), 1);
    // Weighed by the summary the request carries — not 0 (cleared body) and not
    // 4242 (the pre-compaction original_tokens).
    assert_eq!(turns[0].token_estimate, estimate_tokens(&summary));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-core compacted_turn_weighs_summary_not_raw_or_zero 2>&1 | tail -20`
Expected: FAIL — `token_estimate` is `0` (cleared body → `event_tokens` returns ~0), not `estimate_tokens(&summary)`.

- [ ] **Step 3: Implement the summary-accounting override in `group_turns`**

In `group_turns`, before the main `for e in events` loop, build a tool-id → summary map (mirroring `projection.rs:63-68` and `context.rs:315`):

```rust
    // A tool-result whose tool-id has a later ToolResultCompacted is accounted
    // by its summary size — the number actually present in the request
    // (matches context.rs:315). ToolResultCompacted itself is is_inert.
    let compacted: std::collections::HashMap<&str, &str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, summary, .. } => Some((id.as_str(), summary.as_str())),
            _ => None,
        })
        .collect();
```

Then replace the per-event accumulation line `t.token_estimate += event_tokens(&e.kind);` with:

```rust
        let tokens = match &e.kind {
            EventKind::ToolResult { id, .. } if compacted.contains_key(id.as_str()) => {
                crate::economy::estimate_tokens(compacted[id.as_str()])
            }
            _ => event_tokens(&e.kind),
        };
        t.token_estimate += tokens;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid-core compacted_turn_weighs_summary_not_raw_or_zero 2>&1 | tail -20`
Expected: PASS.

Run: `cargo test -p zoid-core 2>&1 | tail -20`
Expected: PASS — no existing eviction test regresses. (If a pre-existing test asserted a compacted turn's raw-body weight, it was asserting the latent over-count; update it to `estimate_tokens(summary)` and note this in the commit.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "fix(eviction): account compacted turns by summary size, not raw body"
```

---

## Task 4: Flip `App.events` to `EventLog` + wire the cheap snapshot (#6a)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App.events` field (`main.rs:916`), `App::record` (`main.rs:1002`), `ProjectionCache::refresh` (`main.rs:753`), projection call sites (`main.rs:311, 437, 757-761, 2119-2146`), the two seed sites (`main.rs:2802, 2850`), the resume-load path (`main.rs:2185`), the `Appended` handler push (`main.rs:1528`)
- Modify: `crates/zoid/src/agent.rs` — turn working-set type (`agent.rs:44, 108`), 5 helper signatures (`agent.rs:907, 943, 1002, 1070, 1104`), the push site (`agent.rs:1116`), `build_request` (`agent.rs:172`), projection call sites (`agent.rs:185, 335, 404-405, 685`)
- Modify: `crates/zoid/src/subagent.rs` — `context_events` parameter (`subagent.rs:104`) and its helper (`subagent.rs:46`)
- Test: existing workspace tests (must stay green); the agent.rs turn tests exercise the working-set flip.

**Interfaces:**
- Consumes: `EventLog` (Task 1) and the iterator-bound core fns (Task 2).
- Produces: `App.events: EventLog`; the turn's owned working set is `EventLog`; the seed handed to each turn/subagent is `app.events.snapshot()`.

**Decision — minimize churn:** Make `EventLog` the drop-in replacement for `Vec<Event>` in agent.rs. Because `EventLog::push(Event)` has the same shape as `Vec::push(Event)`, the ~25 `&mut events` call sites in the turn loop stay **byte-for-byte identical**; only the 5 helper *signatures*, the working-set *bindings*, the *push* site, and the *projection* call sites change.

- [ ] **Step 1: Change `App.events` and the App-side sites**

In `crates/zoid/src/main.rs`:

- `main.rs:916`: `events: Vec<Event>,` → `events: zoid::eventlog::EventLog,`
- Initialization sites (search for `events: Vec::new()` — e.g. `main.rs:3291` in tests, and the real `App { … }` constructor): `Vec::new()` → `zoid::eventlog::EventLog::new()`.
- `App::record` (`main.rs:1002`): `self.events.push(ev);` stays unchanged (EventLog::push takes `Event`).
- `ProjectionCache::refresh` signature (`main.rs:753`): `fn refresh(&mut self, events: &[Event]) -> bool` → `fn refresh(&mut self, events: &zoid::eventlog::EventLog) -> bool`. Inside, the length check `Some(events.len())` stays (EventLog::len exists); each projection call takes `events.iter()`:
  ```rust
      self.msgs = conversation(events.iter());
      self.window = zoid_core::context::context_window(events.iter());
      self.churn = zoid_core::economy::churn_timeline(events.iter());
      self.tasks = zoid_core::tasks::tasks(events.iter());
      let ledger = zoid_core::economy::token_ledger(events.iter());
  ```
  Update `refresh`'s caller to pass `&app.events` (already a `&EventLog`).
- `main.rs:311`: `token_ledger(&app.events)` → `token_ledger(app.events.iter())`.
- `main.rs:437`: `conversation(&app.events)` → `conversation(app.events.iter())`.
- `main.rs:2119, 2126, 2136, 2146`: `conversation(&app.events)` → `conversation(app.events.iter())`.

- [ ] **Step 2: Wire the #6a seed sites**

- `main.rs:2850` (spawn_turn): `let seed = app.events.clone();` → `let seed = app.events.snapshot();`
- `main.rs:2802` (subagent): `let seed = app.events.clone();` → `let seed = app.events.snapshot();`

- [ ] **Step 3: Flip the turn's working set in `agent.rs`**

- `run_agent_turn` param (`agent.rs:44`) and the cancellable variant (`agent.rs:108`): `events: Vec<Event>` → `events: crate::eventlog::EventLog`. (`agent.rs` is a lib module, so it names the sibling lib module as `crate::eventlog` — `EventLog` was placed in `lib.rs` in Task 1 precisely so both `agent.rs` and the bin can reach it.)
- The 5 helper signatures (`agent.rs:907, 943, 1002, 1070, 1104`): `events: &mut Vec<Event>` → `events: &mut crate::eventlog::EventLog`.
- The push site (`agent.rs:1116`): `events.push(ev.clone());` stays unchanged (EventLog::push takes `Event`).
- The ~25 `&mut events` call sites: unchanged.
- Projection call sites:
  - `build_request` (`agent.rs:172`): change its `events: &[Event]` param to `events: &crate::eventlog::EventLog`, and inside (`agent.rs:185`) `conversation(events)` → `conversation(events.iter())`. Its tests (`agent.rs:1137, 1167, 1228`) build `&[]` / `Vec<Event>`; update them to build an `EventLog` (e.g. `EventLog::from_vec(vec![...])`) and pass `&log`. For the empty case, `&EventLog::new()`.
  - `agent.rs:335`: `build_request(&events, …)` stays `build_request(&events, …)` (events is now `EventLog`, param is `&EventLog`).
  - `agent.rs:404`: `context_window_with(&events, overhead.clone())` → `context_window_with(events.iter(), overhead.clone())`.
  - `agent.rs:405, 1051, 1060`: `plan_evictions(&events, …)` → `plan_evictions(events.iter(), …)`.
  - `agent.rs:685`: `evicted_ids(&events)` → `evicted_ids(events.iter())`.
  - Test-only projection calls (`agent.rs:1323, 1393, 1409, 1471`) that pass `&out`/`&seed` where those are `Vec<Event>`: leave as-is (`&Vec<Event>` still satisfies the bound) unless the variable became an `EventLog`, in which case use `.iter()`.

- [ ] **Step 4: Flip `subagent.rs`**

- `run_subagent` (`subagent.rs:104`): `context_events: &[Event]` → `context_events: &crate::eventlog::EventLog`. Its caller (`main.rs:2810` region: `run_subagent(&task, &seed, …)`) passes `&seed` where `seed = app.events.snapshot()` (an `EventLog`) — so `&seed` is `&EventLog` ✓.
- The subagent helper (`subagent.rs:46`): `events: &[Event]` → `events: &crate::eventlog::EventLog`, and `context_window(events)` (`subagent.rs:52`) → `context_window(events.iter())`.
- `conversation(&branch_events)` (`subagent.rs:179`): `branch_events` is a locally built `Vec<Event>` → leave as `conversation(&branch_events)` (`&Vec<Event>` satisfies the bound).

- [ ] **Step 5: Resume-load path**

`main.rs:2185`: `app.events = loaded;` where `loaded: Vec<Event>` from `snapshot_session`. Change to build an `EventLog`:

```rust
                app.events = zoid::eventlog::EventLog::from_vec(loaded);
```

(The `clear_compacted_bodies()` call is added in Task 5 — for now just construct the `EventLog`.)

Also check for other assignments to `app.events` or session-open construction (search `app.events =` and the initial `App { … events:` constructor) and route each through `EventLog::new()` / `EventLog::from_vec(...)`.

- [ ] **Step 6: Build and test the workspace**

Run: `cargo build 2>&1 | tail -30`
Expected: SUCCESS. Fix any remaining call-site type mismatches by the rule: owned `Vec<Event>`/`&[Event]` source → pass `&vec`; `EventLog` source → pass `.iter()` (or `&log` where the param is `&EventLog`).

Run: `cargo test 2>&1 | tail -30`
Expected: PASS — whole workspace green. #6a is now live: the seed is a refcount-only snapshot.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src crates/zoid/src/lib.rs
git commit -m "perf(eventlog): App.events + turn working set use EventLog; seed via snapshot (#6a)"
```

---

## Task 5: #6b live trigger — clear compacted bodies (UI loop + resume)

**Files:**
- Modify: `crates/zoid/src/main.rs` — the `AgentUpdate::Appended` handler (`main.rs:1505-1529`), the resume-load path (`main.rs:2185`)
- Test: a new `#[cfg(test)] mod` test in `crates/zoid/src/eventlog.rs` for the resume clear (already covered by Task 1's `clear_compacted_bodies` test); a render-regression assertion via `conversation`.

**Interfaces:**
- Consumes: `EventLog::clear_tool_output(&str)` and `EventLog::clear_compacted_bodies()` (Task 1); the summary-accounting fix (Task 3) guarantees eviction stays correct after clearing.
- Produces: compacted `ToolResult` bodies are emptied in the hot log at the moment their `ToolResultCompacted` arrives, and on session resume.

- [ ] **Step 1: Write the failing render-regression test**

Add to `crates/zoid/src/eventlog.rs` `mod tests` (it already imports `conversation`? if not, import it):

```rust
    #[test]
    fn cleared_compacted_body_still_renders_summary() {
        use zoid_core::projection::{conversation, ChatMsg};
        let mut log = EventLog::new();
        log.push(tool_result("call-9", "HUGE RAW OUTPUT that must never render"));
        log.push(compacted("call-9", "tiny summary"));
        // Simulate the #6b trigger.
        log.clear_tool_output("call-9");
        let msgs = conversation(log.iter());
        let rendered = msgs.iter().find_map(|m| match m {
            ChatMsg::ToolResult { id, output, .. } if id == "call-9" => Some(output.clone()),
            _ => None,
        });
        assert_eq!(rendered.as_deref(), Some("tiny summary"), "summary renders; cleared raw body never does");
    }
```

(Confirm `ChatMsg` variant field names against `crates/zoid-core/src/projection.rs`; the `conversation` fold emits the summary for a compacted id regardless of the raw body, so this passes once the trigger clears the body — and would pass even without clearing, proving clearing is render-safe.)

- [ ] **Step 2: Run the test to verify it passes (render-safety is inherent)**

Run: `cargo test -p zoid --lib eventlog::tests::cleared_compacted_body_still_renders_summary 2>&1 | tail -20`
Expected: PASS — this asserts the safety invariant (summary always renders). It is a guard, not a red-then-green; keep it as a regression guard.

- [ ] **Step 3: Wire the #6b trigger in the `Appended` handler**

In `crates/zoid/src/main.rs`, in the `AgentUpdate::Appended(ev) =>` arm, **after** `app.events.push(*ev);` (`main.rs:1528`), add the clear. Because `*ev` was moved into `push`, capture the tool-id before the push:

```rust
                    AgentUpdate::Appended(ev) => {
                        // ... existing DelegationResult / ToolResult / apply_streaming logic ...

                        // #6b: when a compaction marker arrives, free the raw
                        // body of the ToolResult it summarizes. Safe: the
                        // request/render always carry the summary (projection.rs),
                        // recall reads SQLite, and eviction weighs the summary
                        // (Task 3). Capture the id before `*ev` is moved.
                        let compacted_id = match &ev.kind {
                            EventKind::ToolResultCompacted { id, .. } => Some(id.clone()),
                            _ => None,
                        };
                        app.events.push(*ev);
                        if let Some(id) = compacted_id {
                            app.events.clear_tool_output(&id);
                        }
                    }
```

Preserve the existing `apply_streaming` / cache-invalidation lines that currently precede `app.events.push(*ev);` — only add the `compacted_id` capture and the post-push clear.

- [ ] **Step 4: Wire the resume clear**

In the resume-load path (`main.rs:2185`), after constructing the `EventLog`, clear compacted bodies so reopening a long session does not re-inflate RAM:

```rust
                app.events = zoid::eventlog::EventLog::from_vec(loaded);
                app.events.clear_compacted_bodies();
```

- [ ] **Step 5: Build and test the workspace**

Run: `cargo build 2>&1 | tail -20`
Expected: SUCCESS.

Run: `cargo test 2>&1 | tail -30`
Expected: PASS — whole workspace green. #6b is now live: compacted bodies are freed on arrival and on resume.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src
git commit -m "perf(eventlog): free compacted ToolResult bodies on compaction + resume (#6b)"
```

---

## Final Verification

- [ ] `cargo test` (workspace) green.
- [ ] `cargo clippy 2>&1` introduces no new warnings in touched files (compare against baseline; the repo is not clippy-clean).
- [ ] `cargo fmt --check` shows no new violations in touched files.
- [ ] Manual sanity: the two seed sites use `.snapshot()`; the `Appended` handler clears on `ToolResultCompacted`; the resume path calls `clear_compacted_bodies()`; `eviction.rs` `group_turns` accounts compacted turns by `estimate_tokens(summary)`.
- [ ] Then use **superpowers:finishing-a-development-branch** to complete the work.

## Self-Review notes (author)

- **Spec coverage:** #6a (snapshot, Task 4) ✔; #6b clear-on-compaction (Task 5) ✔; #6b resume clear (Task 5) ✔; eviction summary-accounting redirect (Task 3) ✔; context.rs correctness already holds (override at `:315`, no change needed — noted, no task, per spec §"Reader redirects") ✔; projection renders summary (no change, verified by Task 5 render guard) ✔; FTS/recall read SQLite (no change) ✔; non-goals (no cold-paging / windowed resume) respected — no such task ✔.
- **Scope correction vs. spec:** the spec's "contained ripple (~3 core functions)" undercounts. The real core surface is **8** functions (`conversation`, `context_window`, `context_window_with`, `plan_evictions`, `evicted_ids`, `token_ledger`, `churn_timeline`, `tasks`) plus the private `group_turns`. Task 2 handles all of them with one mechanical recipe; because `&Vec<Event>` satisfies the new bound, existing call sites and tests compile unchanged, keeping the ripple low-risk.
- **Type consistency:** `EventLog::push(Event)`, `iter() -> impl Iterator<Item=&Event>`, `snapshot() -> EventLog`, `clear_tool_output(&str)`, `clear_compacted_bodies()`, `from_vec(Vec<Event>)` — names used identically across Tasks 1, 4, 5. `group_turns(&[&Event], …)` signature from Task 2 is what Task 3 edits.
- **Module placement (resolved):** verified the `zoid` package has both a lib (`src/lib.rs`) and bin (`src/main.rs`); `agent.rs`/`subagent.rs` are lib modules. `EventLog` is declared `pub mod eventlog;` in `lib.rs` from the start (Task 1), referenced as `crate::eventlog::…` in lib modules and `zoid::eventlog::…` in the bin. No mid-plan relocation needed.
