# EventLog: cheap-clone + clearable event log (#6a + #6b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the per-turn event-log snapshot O(n) refcount bumps instead of O(total bytes) (#6a), and free the raw `ToolResult.output` of compacted results from RAM (#6b), without changing rendered output, recall, readmit, subagent file context, or token accounting.

**Architecture:** Introduce an `EventLog(Vec<Arc<Event>>)` newtype in the `zoid` **lib**. `snapshot()` clones the outer `Vec` (refcount bumps only) for the per-turn seed; `clear_tool_output(id)` swaps a single event's `Arc` slot for one with an empty body. zoid-core's projection functions migrate from `&[Event]` to `impl IntoIterator<Item = &Event>` so they can be fed either an `EventLog::iter()` or (unchanged) a `&Vec<Event>` — keeping the core pure and every existing call site compiling.

**Tech Stack:** Rust workspace (crates: `zoid-core` pure, `zoid` effectful lib+bin). `std::sync::Arc`, `ulid::Ulid`. Test runner: `cargo test`.

## Global Constraints

Every task's requirements implicitly include this section. Values copied verbatim from `docs/superpowers/specs/2026-07-04-eventlog-clone-clear-design.md`.

- **zoid-core stays pure** — `EventLog` and all `Arc` sharing live in the `zoid` crate. The core keeps a borrowed-`&Event` (iterator) projection API; **no `Arc` dependency** enters zoid-core.
- **Clear only compacted bodies** — never uncompacted evicted bodies (a readmitted uncompacted turn is rendered from its raw body).
- **Do not alter on-disk store contents, recall, or readmit behavior** — raw bodies stay persisted in SQLite; recall reads from SQLite.
- **Every reader of a compacted body must be redirected to the summary before clearing.** Compaction covers **both** `ToolResult` and **File** items (`compaction.rs:151`), so the redirect set is: `conversation` (already summary-aware by id), `context_window`'s override, eviction's per-turn accounting, **and `file_contents`** (subagent file inlining). A compacted turn/file is accounted at `estimate_tokens(summary)` (matching `context.rs:315`) — **not** the raw body, **not** zero, **not** `original_tokens`.
- **Commit messages: NO `Co-Authored-By` / co-author trailer** (user rule).
- **Final gate:** `cargo test` (workspace) green; introduce **no new** clippy/fmt warnings in feature-touched code. The repo is not clippy/fmt-clean at baseline — the bar is "no new issues in files this plan touches."
- **Line refs** in this plan are against `f50191b` (the commit the branch starts from). If a number has drifted, match on the quoted code, not the number.

---

## File Structure

- **Create:** `crates/zoid/src/eventlog.rs` — the `EventLog` newtype + its unit tests. Declared `pub mod eventlog;` in `crates/zoid/src/lib.rs` (lib, so `agent.rs`/`subagent.rs` can import it).
- **Modify:** `crates/zoid/src/lib.rs` — add `pub mod eventlog;`.
- **Modify:** `crates/zoid/src/main.rs` — change `App.events` type, both seed sites, projection call sites, the `AgentUpdate::Appended` handler (#6b trigger), the resume-load path.
- **Modify:** `crates/zoid/src/agent.rs` — the turn's working-set type + 5 helper signatures + the push site + the local `estimate` closure + core call sites.
- **Modify:** `crates/zoid/src/subagent.rs` — the context-events parameter type + the `file_contents` call site.
- **Modify (zoid-core):** `crates/zoid-core/src/projection.rs`, `context.rs`, `eviction.rs`, `economy.rs`, `tasks.rs`, `compaction.rs` — signature migration (Task 2) and compacted-File correctness (Task 3) and eviction accounting (Task 4).

**Module placement (verified):** the `zoid` package has both a lib (`src/lib.rs`, crate name `zoid`) and a bin (`src/main.rs`) — separate crates. `agent.rs` and `subagent.rs` are **lib** modules (`lib.rs:4-10`). Since a lib module cannot import from the bin, `EventLog` lives in the **lib**: `pub mod eventlog;` in `lib.rs`. Reference it as **`crate::eventlog::EventLog` from lib modules** (`agent.rs`, `subagent.rs`) and as **`zoid::eventlog::EventLog` from the bin** (`main.rs`, which sees the lib as an external crate — cf. `use zoid::agent::…` at `main.rs:25`). Because lib and bin share the crate name, the two spellings name the *same* type, so `App.events: zoid::eventlog::EventLog` interoperates with `run_agent_turn(events: crate::eventlog::EventLog)`.

---

## The zoid-core migration recipe (used by Task 2)

Every migrated function follows this recipe. It relies on two facts:

1. `&Vec<Event>` and `&[Event]` already satisfy `impl IntoIterator<Item = &Event>`, so **existing call sites and existing zoid-core tests keep compiling unchanged**.
2. Iterating a `Vec<&Event>` (or the `IntoIterator` directly) by value yields `&Event` (identical to iterating `&[Event]`); iterating `&Vec<&Event>` / `&[&Event]` yields `&&Event`, and Rust auto-derefs field access (`e.kind`, `e.id`, `e.ts`, `e.branch`, `&e.kind`, non-binding `matches!`) through the extra reference — so field-access-only bodies need **no edits**.

**Recipe:**
- Change the parameter `events: &[Event]` → `events: impl IntoIterator<Item = &'a Event>` and add `<'a>` to the function's generics. (Other parameters keep their types.)
- **Single-pass function** — `events` is consumed by exactly one `for e in events { … }` loop (or one iterator chain) and used nowhere else: **do not** add a collect prelude. `for e in events` iterates the `IntoIterator` directly and yields `&Event`. (Applies to `evicted_ids`, `token_ledger`, `churn_timeline`. `evicted_ids` is hot — called inside `conversation`, `context_window_with`, `plan_evictions` — so skipping the throwaway `Vec` matters. NOTE: `eviction_breadcrumb` is NOT single-pass — it calls `evicted_ids(events)` then loops again, so it is multi-use; see below.)
- **`.rev()` / other `DoubleEndedIterator` needs** (`tasks`): a generic `IntoIterator` is not double-ended, so collect first: `let events: Vec<&Event> = events.into_iter().collect();` then `events.into_iter()…rev()` (Vec's iterator is double-ended).
- **Multi-use function** — `events` is used more than once (a sub-call plus a loop, or two loops: `conversation`, `context_window_with`, `plan_evictions`, `file_contents`, `plan_compactions`): add `let events: Vec<&Event> = events.into_iter().collect();`, then `let visible: &[&Event] = &events;`, feed any sub-call `events.iter().copied()` (yields `&Event`), and iterate `for e in visible` (yields `&&Event`; field access auto-derefs).

Do **not** hand-edit function bodies beyond what the recipe specifies. After applying it, `cargo test -p zoid-core` must be green and `cargo build` (workspace) must still succeed with all call sites unchanged.

---

## Task 1: `EventLog` newtype + unit tests

**Files:**
- Create: `crates/zoid/src/eventlog.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod eventlog;` alongside the other `pub mod` declarations at lines 4-10)
- Test: inline `#[cfg(test)] mod tests` in `crates/zoid/src/eventlog.rs`

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

Create `crates/zoid/src/eventlog.rs` with the test module (implementation stubs follow in Step 3):

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

`arcs()` is a `#[cfg(test)]`-only accessor (Step 3) so tests can assert on `Arc` identity without exposing internals.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib eventlog 2>&1 | head -40`
Expected: FAIL — the `impl EventLog` block does not exist yet (compile errors: `no function or associated item named 'new'`, etc.).

- [ ] **Step 3: Implement `EventLog`**

Insert between the struct definition and the `#[cfg(test)]` module:

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

Then add to `crates/zoid/src/lib.rs`, alongside the existing `pub mod` lines (4-10):

```rust
pub mod eventlog;
```

(`Event` fields are all `pub` — `event.rs:124-132` — so the clone-then-mutate form compiles.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib eventlog 2>&1 | tail -20`
Expected: PASS — all 6 tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/eventlog.rs crates/zoid/src/lib.rs
git commit -m "feat(eventlog): Arc-backed EventLog with cheap snapshot + clearable bodies"
```

---

## Task 2: Migrate zoid-core consumers to the iterator bound

**Files (11 functions + one private helper):**
- Modify: `crates/zoid-core/src/projection.rs:57` (`conversation`)
- Modify: `crates/zoid-core/src/context.rs:165` (`context_window`), `:173` (`context_window_with`), `:356` (`file_contents`)
- Modify: `crates/zoid-core/src/eviction.rs:50` (`evicted_ids`), `:68` (`eviction_breadcrumb`), `:250` (`plan_evictions`) and its private helper `group_turns` (`:198`)
- Modify: `crates/zoid-core/src/economy.rs:19` (`token_ledger`), `:76` (`churn_timeline`)
- Modify: `crates/zoid-core/src/tasks.rs:49` (`tasks`)
- Modify: `crates/zoid-core/src/compaction.rs:52` (`plan_compactions`)
- Test: existing `#[cfg(test)] mod tests` in each file (unchanged — they call with `&vec`, still satisfying the bound).

**Interfaces:**
- Produces: all 11 functions accept `impl IntoIterator<Item = &'a Event>` instead of `&[Event]`. Later tasks feed them `EventLog::iter()`. This task is a **pure mechanical migration — no behavior change** (the compacted-body correctness fixes are Task 3, the eviction accounting fix is Task 4).

Apply **The zoid-core migration recipe** (top of this plan). Per-function classification:

- [ ] **Step 1: Single-pass functions (no collect prelude)**

`token_ledger` (`economy.rs:19`), `churn_timeline` (`economy.rs:76`), `evicted_ids` (`eviction.rs:50`): each is one `for e in events` loop (or one chain) using `events` once. Change signature only. (`eviction_breadcrumb` at `eviction.rs:68` is multi-use — migrate it with the collect-form in Step 2.)

```rust
pub fn token_ledger<'a>(events: impl IntoIterator<Item = &'a Event>) -> TokenLedger {
    // body unchanged: `for e in events { … }` now iterates the IntoIterator, yields &Event
}
```

Same shape for `churn_timeline`, `evicted_ids` — signature change, body untouched.

`tasks` (`tasks.rs:49`) uses `.rev()`, so collect first (double-ended):

```rust
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

- [ ] **Step 2: Multi-use functions (collect + `visible: &[&Event]`)**

`conversation` (`projection.rs:57`) — sub-call + two loops. Replace lines 57-59:

```rust
pub fn conversation<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<ChatMsg> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let evicted = crate::eviction::evicted_ids(events.iter().copied());
```

Rest unchanged (`for e in visible` yields `&&Event`; field access auto-derefs).

`context_window` (`context.rs:165`) delegates:

```rust
pub fn context_window<'a>(events: impl IntoIterator<Item = &'a Event>) -> ContextWindow {
    context_window_with(events, ContextOverhead::default())
}
```

`context_window_with` (`context.rs:173`) — sub-call + one loop. Replace its first two body lines:

```rust
pub fn context_window_with<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    overhead: ContextOverhead,
) -> ContextWindow {
    let events: Vec<&Event> = events.into_iter().collect();
    let evicted = crate::eviction::evicted_ids(events.iter().copied());
    let visible: &[&Event] = &events;
    // rest unchanged (single `for e in visible` fold loop)
```

`file_contents` (`context.rs:356`) — one loop; migrate signature + collect prelude now (the summary-substitution behavior lands in Task 3):

```rust
pub fn file_contents<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashMap<String, String> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    // rest unchanged (single `for e in visible` loop); `.get(id)` on &&Event auto-derefs
```

`plan_compactions` (`compaction.rs:52`) — inspect the body; it calls `context_window_with(events, …)` internally. Migrate the signature, collect, and feed the internal call `events.iter().copied()`:

```rust
pub fn plan_compactions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &ContextPolicy,
    real_input_tokens: Option<u64>,
    calibration_ratio: Option<f64>,
    overhead: &ContextOverhead,
) -> CompactionPlan {
    let events: Vec<&Event> = events.into_iter().collect();
    // internal context_window_with(events, …) → context_window_with(events.iter().copied(), …)
    // any other &events / events loop → visible: &[&Event] form
```

- [ ] **Step 3: `plan_evictions` + `group_turns`**

`plan_evictions` (`eviction.rs:250`) collects and passes a `&[&Event]` to `group_turns`:

```rust
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
    // ... the group_turns call becomes: group_turns(&events, &evicted, policy.recent_n);
    // and any evicted_ids(events) → evicted_ids(events.iter().copied())
```

`group_turns` (`eviction.rs:198`) signature: `fn group_turns(events: &[Event], …)` → `fn group_turns(events: &[&Event], …)`. Body unchanged (`for e in events` yields `&&Event`; `&e.kind`, `e.id`, `e.branch`, and the non-binding `matches!` at `eviction.rs:219` all auto-deref). (The summary-accounting override lands in **Task 4** — do not add it here.)

- [ ] **Step 4: Build + test**

Run: `cargo test -p zoid-core 2>&1 | tail -30`
Expected: PASS — all existing zoid-core tests green (no test edits; `&vec` satisfies the bound).

Run: `cargo build 2>&1 | tail -20`
Expected: SUCCESS — the bin still compiles (its call sites pass `&app.events`, still `&Vec<Event>`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src
git commit -m "refactor(core): projection/compaction fns take impl IntoIterator<Item=&Event>"
```

---

## Task 3: Compacted-File correctness — redirect `file_contents` and `context_window` to the summary

**Why (Gilfoyle CRITICAL + IMPORTANT):** Compaction compacts **File** items, not only non-file tool results (`compaction.rs:151`; File items resolve path→tool-call-id and emit `ToolResultCompacted { id: tool_call_id }` at `compaction.rs:158-172`). Two readers key on item *kind* and therefore miss compacted files:
1. `file_contents` (`context.rs:356`, called by `build_subagent_request` at `subagent.rs:54`) reads raw `ToolResult.output`. Once #6b clears a compacted file read's body, a subagent gets an **empty file**. It must substitute the summary.
2. `context_window`'s override (`context.rs:310-317`) only matches `ItemKind::ToolResult`, so a compacted **file**'s window tokens are never redirected to the summary — after #6b they read `estimate_tokens("") = 0`, under-reading the window. It must also override File items (using the existing `call_path: tool-id→path` map at `context.rs:179`).

**Files:**
- Modify: `crates/zoid-core/src/context.rs` — `file_contents` (`:356`) and the `ToolResultCompacted` arm of `context_window_with` (`:310-317`)
- Test: `#[cfg(test)] mod tests` in `crates/zoid-core/src/context.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `crates/zoid-core/src/context.rs`. Match helper construction to the existing tests in that module (they already build `read_file` ToolCall/ToolResult pairs — reuse their pattern; the sketch below shows intent):

```rust
#[test]
fn file_contents_substitutes_summary_for_compacted_file() {
    use crate::event::{Event, EventKind};
    use ulid::Ulid;
    // read_file call + its result, then a compaction marker for that call id.
    let call = Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
        id: "call-1".into(),
        name: "read_file".into(),
        args: serde_json::json!({ "path": "/src/x.rs" }).to_string(),
    });
    let res = Event::new(Ulid::new(), None, 0, EventKind::ToolResult {
        id: "call-1".into(), name: "read_file".into(),
        output: "FULL FILE BODY".into(), is_error: false,
    });
    let comp = Event::new(Ulid::new(), None, 0, EventKind::ToolResultCompacted {
        id: "call-1".into(), summary: "file summary".into(), original_tokens: 500,
    });
    let evs = vec![call, res, comp];
    let map = file_contents(evs.iter());
    assert_eq!(map.get("file:/src/x.rs").map(String::as_str), Some("file summary"),
        "compacted file must inline its summary, never the raw/cleared body");
}

#[test]
fn context_window_overrides_compacted_file_tokens() {
    use crate::event::{Event, EventKind};
    use crate::economy::estimate_tokens;
    use ulid::Ulid;
    let call = Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
        id: "call-2".into(), name: "read_file".into(),
        args: serde_json::json!({ "path": "/src/y.rs" }).to_string(),
    });
    let res = Event::new(Ulid::new(), None, 0, EventKind::ToolResult {
        id: "call-2".into(), name: "read_file".into(),
        output: "x".repeat(3000), is_error: false,
    });
    let summary = "y summary".to_string();
    let comp = Event::new(Ulid::new(), None, 0, EventKind::ToolResultCompacted {
        id: "call-2".into(), summary: summary.clone(), original_tokens: 1000,
    });
    let evs = vec![call, res, comp];
    let w = context_window(evs.iter());
    let file_item = w.items.iter().find(|i| i.key == "file:/src/y.rs").expect("file item present");
    assert_eq!(file_item.tokens, estimate_tokens(&summary),
        "compacted file item weighs its summary, not the raw body or 0");
    assert!(file_item.compacted);
}
```

Confirm the `read_file` tool name and the `args` path key against `tool_path` (`context.rs`) — use whatever key `tool_path` extracts (the existing File-item tests in this module are the reference).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p zoid-core file_contents_substitutes_summary_for_compacted_file context_window_overrides_compacted_file_tokens 2>&1 | tail -25`
Expected: FAIL — `file_contents` returns `"FULL FILE BODY"`; the File item's tokens are the raw estimate, not the summary.

- [ ] **Step 3: Fix `file_contents`**

Build a compacted map (mirror `projection.rs:63-68`) and substitute in the `ToolResult` arm:

```rust
pub fn file_contents<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashMap<String, String> {
    let events: Vec<&Event> = events.into_iter().collect();
    let compacted: HashMap<&str, &str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, summary, .. } => Some((id.as_str(), summary.as_str())),
            _ => None,
        })
        .collect();
    let mut call_path: HashMap<String, String> = HashMap::new();
    let mut out: HashMap<String, String> = HashMap::new();
    for e in &events {
        match &e.kind {
            EventKind::ToolCall { id, args, .. } => {
                if let Some(p) = tool_path(args) {
                    call_path.insert(id.clone(), p);
                }
            }
            EventKind::ToolResult { id, output, is_error: false, .. } => {
                if let Some(p) = call_path.get(id) {
                    let body = match compacted.get(id.as_str()) {
                        Some(sum) => (*sum).to_string(),
                        None => output.clone(),
                    };
                    out.insert(format!("file:{p}"), body); // latest wins
                }
            }
            _ => {}
        }
    }
    out
}
```

- [ ] **Step 4: Fix the `context_window` override for File items**

In `context_window_with`, extend the `ToolResultCompacted` arm (`context.rs:310-317`) to also match a File item via the `call_path` map:

```rust
            EventKind::ToolResultCompacted { id, summary, .. } => {
                let file_key = call_path.get(id).map(|p| format!("file:{p}"));
                if let Some(it) = items.iter_mut().find(|i| {
                    (i.kind == ItemKind::ToolResult && tool_id_of(&i.key) == Some(id.as_str()))
                        || file_key.as_deref() == Some(i.key.as_str())
                }) {
                    it.tokens = crate::economy::estimate_tokens(summary);
                    it.compacted = true;
                }
            }
```

(`call_path` is populated before the compaction marker is reached, since compaction events always follow the file read in the log.)

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p zoid-core 2>&1 | tail -25`
Expected: PASS — both new tests green and no existing context/compaction test regresses. (If a snapshot-style test asserted a compacted file's raw window tokens, it was asserting the pre-existing over-count; update it to `estimate_tokens(summary)` and note it in the commit.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/context.rs
git commit -m "fix(context): redirect compacted FILE reads to summary (file_contents + window)"
```

---

## Task 4: Eviction summary-accounting fix (compacted turns weigh their summary)

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` — `group_turns` (`:198`, post-Task-2 signature `&[&Event]`)
- Test: `#[cfg(test)] mod tests` in `crates/zoid-core/src/eviction.rs`

**Why:** `ToolResultCompacted` is `is_inert` (`eviction.rs:174`), so a compacted turn's entire ranking weight flows through its `ToolResult` at `event_tokens` (`eviction.rs:189`). Today that reads the **raw** body — over-counting vs. what the request carries (the summary, per `context.rs:315`), and would read ~0 once #6b clears the body. Accounting by summary is correct in both cases. (Eviction matches on the event's inner id, which covers both tool-result and file compactions.)

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/zoid-core/src/eviction.rs`. **The fixture is a lone `ToolResult` + its compaction marker — no `UserMessage`** — so the `ToolResult` opens the turn via the `turns.is_empty()` branch (`eviction.rs:220`) and the turn weight is exactly `estimate_tokens(summary)` (a `UserMessage("hi")` would add `estimate_tokens("hi") = 1`, per `economy.rs` `div_ceil`):

```rust
#[test]
fn compacted_turn_weighs_summary_not_raw_or_zero() {
    use crate::economy::estimate_tokens;
    use crate::event::{Event, EventKind};
    use std::collections::HashSet;

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

    let events: Vec<&Event> = vec![&tr, &comp];
    let turns = group_turns(&events, &HashSet::new(), 0);

    assert_eq!(turns.len(), 1);
    // Weighed by the summary the request carries — not 0 (cleared body) and not
    // 4242 (the pre-compaction original_tokens).
    assert_eq!(turns[0].token_estimate, estimate_tokens(&summary));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-core compacted_turn_weighs_summary_not_raw_or_zero 2>&1 | tail -20`
Expected: FAIL — `token_estimate` is `0` (cleared body → `event_tokens` ~0), not `estimate_tokens(&summary)`.

- [ ] **Step 3: Implement the override in `group_turns`**

Before the main `for e in events` loop, build a tool-id → summary map (mirroring `projection.rs:63-68` / `context.rs:315`):

```rust
    let compacted: std::collections::HashMap<&str, &str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, summary, .. } => Some((id.as_str(), summary.as_str())),
            _ => None,
        })
        .collect();
```

Then replace `t.token_estimate += event_tokens(&e.kind);` with:

```rust
        let tokens = match &e.kind {
            EventKind::ToolResult { id, .. } if compacted.contains_key(id.as_str()) => {
                crate::economy::estimate_tokens(compacted[id.as_str()])
            }
            _ => event_tokens(&e.kind),
        };
        t.token_estimate += tokens;
```

Note: after Task 2, `e` is `&&Event` inside `group_turns`; `&e.kind` and `id.as_str()` auto-deref.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-core compacted_turn_weighs_summary_not_raw_or_zero 2>&1 | tail -20`
Expected: PASS.

Run: `cargo test -p zoid-core 2>&1 | tail -20`
Expected: PASS — no eviction test regresses. (If a test asserted a compacted turn's raw-body weight, update it to `estimate_tokens(summary)` and note it.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "fix(eviction): account compacted turns by summary size, not raw body"
```

---

## Task 5: Flip `App.events` to `EventLog` + wire the cheap snapshot (#6a)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App.events` (`:916`), init sites, `App::record` (`:1002`), `ProjectionCache::refresh` (`:753`), projection call sites (`:311, 437, 757-761, 2119-2146`), seed sites (`:2802, 2850`), resume-load (`:2185`)
- Modify: `crates/zoid/src/agent.rs` — turn working set (`:44, 108`), 5 helper sigs (`:907, 943, 1002, 1070, 1104`), push (`:1116`), `build_request` (`:172`) + its internal `eviction_breadcrumb`/`conversation` calls (`:178, 185`), `plan_compactions` calls (`:964, 1037`), the `estimate` closure (`:1016`) + its callers (`:1028, 1045, 1053`), other projection calls (`:335, 404-405, 685`)
- Modify: `crates/zoid/src/subagent.rs` — `context_events` param (`:104`), the helper (`:46`), `context_window`/`file_contents` calls (`:52, 54`)
- Test: existing workspace tests (must stay green).

**Interfaces:**
- Consumes: `EventLog` (Task 1) and the iterator-bound core fns (Tasks 2-4).
- Produces: `App.events: EventLog`; the turn's owned working set is `EventLog`; the seed is `app.events.snapshot()`.

**Decision — minimize churn:** `EventLog::push(Event)` mirrors `Vec::push(Event)`, so the ~25 `&mut events` call sites in the turn loop stay **byte-for-byte identical**; only helper *signatures*, *bindings*, the *push*, the *closure*, and *projection* call sites change.

- [ ] **Step 1: `App.events` and App-side sites (`main.rs`)**

- `main.rs:916`: `events: Vec<Event>,` → `events: zoid::eventlog::EventLog,`
- Init sites (search `events: Vec::new()` — the real `App { … }` constructor and any test at e.g. `main.rs:3291`): `Vec::new()` → `zoid::eventlog::EventLog::new()`.
- `App::record` (`main.rs:1002`): `self.events.push(ev);` unchanged.
- `ProjectionCache::refresh` (`main.rs:753`): `fn refresh(&mut self, events: &[Event]) -> bool` → `fn refresh(&mut self, events: &zoid::eventlog::EventLog) -> bool`. Keep `Some(events.len())`. Each projection call takes `events.iter()`:
  ```rust
      self.msgs = conversation(events.iter());
      self.window = zoid_core::context::context_window(events.iter());
      self.churn = zoid_core::economy::churn_timeline(events.iter());
      self.tasks = zoid_core::tasks::tasks(events.iter());
      let ledger = zoid_core::economy::token_ledger(events.iter());
  ```
  Its caller passes `&app.events` (already `&EventLog`).
- `main.rs:311`: `token_ledger(&app.events)` → `token_ledger(app.events.iter())`.
- `main.rs:437, 2119, 2126, 2136, 2146`: `conversation(&app.events)` → `conversation(app.events.iter())`.

- [ ] **Step 2: Seed sites (#6a)**

- `main.rs:2850` (spawn_turn) and `main.rs:2802` (subagent): `let seed = app.events.clone();` → `let seed = app.events.snapshot();`

- [ ] **Step 3: Turn working set + helpers + closure (`agent.rs`)**

- `run_agent_turn` (`agent.rs:44`) + cancellable variant (`agent.rs:108`): `events: Vec<Event>` → `events: crate::eventlog::EventLog`.
- 5 helper sigs (`agent.rs:907, 943, 1002, 1070, 1104`): `events: &mut Vec<Event>` → `events: &mut crate::eventlog::EventLog`.
- Push (`agent.rs:1116`): `events.push(ev.clone());` unchanged. The ~25 `&mut events` call sites unchanged.
- `build_request` (`agent.rs:172`): param `events: &[Event]` → `events: &crate::eventlog::EventLog`. Inside: `eviction_breadcrumb(events)` (`:178`) → `eviction_breadcrumb(events.iter())`; `conversation(events)` (`:185`) → `conversation(events.iter())`. Its tests (`agent.rs:1137, 1167, 1228`) build `&[]`/`Vec<Event>` — update to `EventLog::from_vec(vec![...])` and pass `&log`; empty case `&EventLog::new()`.
- `agent.rs:335`: `build_request(&events, …)` unchanged (`events` is `EventLog`, param `&EventLog`).
- `plan_compactions` calls (`agent.rs:964, 1037`): `plan_compactions(events, …)` → `plan_compactions(events.iter(), …)`.
- The `estimate` closure (`agent.rs:1016`): `let estimate = |events: &[Event]| -> u64 {` → `let estimate = |events: &crate::eventlog::EventLog| -> u64 {`, and inside `context_window_with(events, overhead.clone())` → `context_window_with(events.iter(), overhead.clone())`. Its callers `estimate(events)` (`agent.rs:1028, 1045, 1053`) where `events` is `&mut EventLog` reborrow to `&EventLog` automatically; if the borrow checker complains, write `estimate(&*events)`.
- `agent.rs:404`: `context_window_with(&events, overhead.clone())` → `context_window_with(events.iter(), overhead.clone())`.
- `agent.rs:405, 1051, 1060`: `plan_evictions(&events, …)` → `plan_evictions(events.iter(), …)`.
- `agent.rs:685`: `evicted_ids(&events)` → `evicted_ids(events.iter())`.
- Test-only projection calls (`agent.rs:1323, 1393, 1409, 1471`) passing `&out`/`&seed` that are `Vec<Event>`: leave as-is unless the variable became `EventLog`, then use `.iter()`.

- [ ] **Step 4: `subagent.rs`**

- `run_subagent` (`subagent.rs:104`): `context_events: &[Event]` → `context_events: &crate::eventlog::EventLog`. Caller (`main.rs` `run_subagent(&task, &seed, …)`) passes `&seed` = `&EventLog` ✓.
- Helper `build_subagent_request` (`subagent.rs:46`): `events: &[Event]` → `events: &crate::eventlog::EventLog`. Inside: `context_window(events)` (`:52`) → `context_window(events.iter())`; `file_contents(events)` (`:54`) → `file_contents(events.iter())`.
- `conversation(&branch_events)` (`subagent.rs:179`): `branch_events` is a local `Vec<Event>` → leave as-is.

- [ ] **Step 5: Resume-load path**

`main.rs:2185`: `app.events = loaded;` (`loaded: Vec<Event>`) → `app.events = zoid::eventlog::EventLog::from_vec(loaded);` (the `clear_compacted_bodies()` call is added in Task 6). Route any other `app.events =` / initial constructor through `EventLog::new()` / `from_vec(...)`.

- [ ] **Step 6: Build + test**

Run: `cargo build 2>&1 | tail -30`
Expected: SUCCESS. Fix any remaining mismatch by the rule: owned `Vec<Event>`/`&[Event]` source → `&vec`; `EventLog` source → `.iter()` (or `&log` where the param is `&EventLog`).

Run: `cargo test 2>&1 | tail -30`
Expected: PASS — whole workspace green. #6a is live: the seed is a refcount-only snapshot.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src
git commit -m "perf(eventlog): App.events + turn working set use EventLog; seed via snapshot (#6a)"
```

---

## Task 6: #6b live trigger — clear compacted bodies (UI loop + resume)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `AgentUpdate::Appended` handler (`:1505-1528`), resume-load (`:2185`)
- Test: render-regression guard in `crates/zoid/src/eventlog.rs` `mod tests`.

**Interfaces:**
- Consumes: `EventLog::clear_tool_output(&str)` + `clear_compacted_bodies()` (Task 1); the compacted-body redirects (Tasks 3-4) guarantee readers/accounting stay correct after clearing.
- Produces: compacted `ToolResult`/File bodies emptied on `ToolResultCompacted` arrival and on resume.

- [ ] **Step 1: Write the render-safety guard**

Add to `crates/zoid/src/eventlog.rs` `mod tests`:

```rust
    #[test]
    fn cleared_compacted_body_still_renders_summary() {
        use zoid_core::projection::{conversation, ChatMsg};
        let mut log = EventLog::new();
        log.push(tool_result("call-9", "HUGE RAW OUTPUT that must never render"));
        log.push(compacted("call-9", "tiny summary"));
        log.clear_tool_output("call-9"); // simulate the #6b trigger
        let msgs = conversation(log.iter());
        let rendered = msgs.iter().find_map(|m| match m {
            ChatMsg::ToolResult { id, output, .. } if id == "call-9" => Some(output.clone()),
            _ => None,
        });
        assert_eq!(rendered.as_deref(), Some("tiny summary"), "summary renders; cleared raw body never does");
    }
```

(Confirm `ChatMsg::ToolResult` field names against `projection.rs`. This is a regression guard — it passes because `conversation` emits the summary by id regardless of body.)

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p zoid --lib eventlog::tests::cleared_compacted_body_still_renders_summary 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Wire the #6b trigger in the `Appended` handler**

In the `AgentUpdate::Appended(ev) =>` arm of `main.rs`, capture the compacted id before `*ev` is moved into `push`, then clear after. Preserve all existing `apply_streaming`/cache-invalidation lines:

```rust
                    AgentUpdate::Appended(ev) => {
                        // ... existing DelegationResult / ToolResult / apply_streaming logic ...

                        // #6b: when a compaction marker arrives, free the raw body
                        // of the ToolResult it summarizes. Safe: request/render carry
                        // the summary (projection.rs), file_contents & window are
                        // redirected (Task 3), eviction weighs the summary (Task 4),
                        // recall reads SQLite. Capture the id before `*ev` is moved.
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

- [ ] **Step 4: Wire the resume clear**

At `main.rs:2185`, after constructing the `EventLog`:

```rust
                app.events = zoid::eventlog::EventLog::from_vec(loaded);
                app.events.clear_compacted_bodies();
```

- [ ] **Step 5: Build + test**

Run: `cargo build 2>&1 | tail -20`
Expected: SUCCESS.

Run: `cargo test 2>&1 | tail -30`
Expected: PASS — whole workspace green. #6b is live: compacted bodies freed on arrival + on resume.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src
git commit -m "perf(eventlog): free compacted ToolResult/file bodies on compaction + resume (#6b)"
```

---

## Final Verification

- [ ] `cargo test` (workspace) green.
- [ ] `cargo clippy 2>&1` introduces no new warnings in touched files (repo not clippy-clean at baseline).
- [ ] `cargo fmt --check` shows no new violations in touched files.
- [ ] Manual sanity: seed sites use `.snapshot()`; the `Appended` handler clears on `ToolResultCompacted`; resume calls `clear_compacted_bodies()`; `file_contents` + `context_window` redirect compacted **files** to summary; `group_turns` accounts compacted turns by `estimate_tokens(summary)`.
- [ ] Then use **superpowers:finishing-a-development-branch** to complete the work.

## Self-Review notes (author)

- **Spec coverage:** #6a (snapshot, Task 5) ✔; #6b clear-on-compaction (Task 6) ✔; #6b resume clear (Task 6) ✔; eviction summary-accounting (Task 4) ✔; **compacted-File correctness — `file_contents` + `context_window` (Task 3)** ✔ (spec safety-argument correction: compaction covers File items, so two extra readers exist); `conversation` renders summary by id, no change (Task 6 guard) ✔; FTS/recall read SQLite (no change) ✔; non-goals (no cold-paging / windowed resume) respected ✔.
- **Core surface (corrected after review):** the real surface is **11** functions (`conversation`, `context_window`, `context_window_with`, `file_contents`, `plan_evictions`, `evicted_ids`, `eviction_breadcrumb`, `plan_compactions`, `token_ledger`, `churn_timeline`, `tasks`) + the private `group_turns` + the local `estimate` closure — not 8. Task 2 migrates all 11 mechanically; because `&Vec<Event>` satisfies the new bound, existing call sites and tests compile unchanged.
- **Allocation note:** single-pass fns (`evicted_ids`, `token_ledger`, `churn_timeline`) skip the collect prelude and iterate the `IntoIterator` directly — `evicted_ids` is hot (called inside `conversation`/`context_window_with`/`plan_evictions`), so the throwaway `Vec` is avoided there. (`eviction_breadcrumb` is multi-use — corrected after Task 2 review — and takes the collect-form.)
- **Test correctness:** Task 4's fixture is a lone `ToolResult` + marker (no `UserMessage`), so the turn weight is exactly `estimate_tokens(summary)` — `estimate_tokens("hi")` would be `1`, not `0`, under `div_ceil(3)`.
- **Type consistency:** `EventLog::{push, iter, snapshot, clear_tool_output, clear_compacted_bodies, from_vec, new}` used identically across Tasks 1/5/6. `group_turns(&[&Event], …)` (Task 2) is what Task 4 edits.
- **Module placement (verified):** lib target exists (`Cargo.toml [lib] name=zoid`), `agent`/`subagent` are lib modules; `EventLog` in `lib.rs`, referenced `crate::eventlog` in lib, `zoid::eventlog` in bin.
