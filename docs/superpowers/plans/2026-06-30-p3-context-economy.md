# P3 · Context Economy ⑤ Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the user real-time, manual + automated *visibility* into their context window — a token ledger, per-item heat, and a churn timeline — plus a pure constructed-context assembler primitive that P5 will reuse to build subagent contexts.

**Architecture:** The economy is a set of **pure projections over the event log** (zoid-core): `TokenLedger`, `ChurnTimeline`, and `ContextWindow` (items across *all* token-spending sources — system, messages, tool results, files — each with tokens + heat + pin/evict). A pure **assembler** (`assemble_context`) turns a window + policy into a selection; it is fully tested but **not** wired into the live `build_request` in P3 (that is P5 — this phase is visualize-only). zoid-tui renders the ⑤ drawer from a pure `EconomyView` view-model built from those projections. The agent loop starts recording real token `Usage` so the ledger has live numbers.

**Tech Stack:** Rust 2021, serde, ulid, ratatui 0.29 (`TestBackend`/`insta` snapshots), proptest (core).

## Global Constraints

- **Crates & dep direction:** `zoid-core` (pure, no ratatui), `zoid-provider`, `zoid-tools`, `zoid-tui` (deps core), `zoid` bin. Never introduce a cycle. Projections live in `zoid-core`; view-models and rendering in `zoid-tui`; side effects (event recording) only in the `zoid` bin and `zoid/src/agent.rs`.
- **Visualize-only (P3 scope decision):** Do **not** modify `zoid/src/agent.rs::build_request` to filter context through the assembler. The assembler is exercised by unit tests and the `EconomyView`; live wiring into the model request is **deferred to P5**. (User decision, 2026-06-30.)
- **Auto-evict default ON (P3 scope decision):** `ContextPolicy::default()` sets `auto_evict_cold = true`. This affects the *projected/effective* window and the assembler's output (and thus the drawer), **never** the live request in P3, and **never** evicts a pinned item. (User decision, 2026-06-30.)
- **Design tokens are the single source of truth (spec §16):** no literal special glyphs (heat blocks, sparkline ramp, ⑤, ●, ✕, …) or hex colors outside `crates/zoid-tui/src/tokens.rs`. New visual tokens must also be added to the authoritative table in `docs/ux/README.md`. ASCII punctuation (`[`, `]`, `/`, digits) is exempt.
- **UX testing is mandatory and multi-width:** every task that changes rendering adds/updates `TestBackend`+`insta` snapshots at **both 100×24 and 140×24** (the 100-only blind spot caused the P2 gutter bug), and adds/updates the matching `crates/zoid-tui/examples/preview.rs` scene. All Ⓡ4 dataviz (heat bars, sparklines, gauges, token humanization) are **pure functions with their own unit tests** — never computed inline inside a render fn.
- **TDD, DRY, YAGNI, frequent commits.** Pure core projections get `proptest` invariants in addition to example-based unit tests (spec §14: core is the highest-value test surface).
- **No `Co-Authored-By` / co-author trailer in commits** (user global instruction).
- **Heat is an explicit heuristic** (spec §15 risk 9): document it, keep thresholds as named consts, and make pin always override eviction.
- **Token estimation:** per-item token cost is estimated as `ceil(chars/4)` via `estimate_tokens`; aggregate ledger numbers come from real provider `Usage`.
- Run `cargo test` (workspace) and `cargo clippy --all-targets` clean before every commit. Accept new snapshots with `INSTA_UPDATE=always cargo test -p zoid-tui --test <file>` (cargo-insta is not installed), and review the `.snap` content for fidelity to `docs/ux/` before committing.

---

### Task 1: Economy design tokens (glyph ramps, heat colors)

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs`
- Modify: `docs/ux/README.md` (visual-language table — authoritative source)
- Test: `crates/zoid-tui/src/tokens.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing.
- Produces: `glyph::HEAT_FULL: char`, `glyph::HEAT_SHADE: char`, `glyph::SPARK: [char; 8]`, `glyph::PIN: char`; `color::HEAT_HOT`, `color::HEAT_WARM`, `color::HEAT_COLD: Color`. (Heat colors alias existing status colors so the palette stays uniform.)

- [ ] **Step 1: Add the failing token test**

In `crates/zoid-tui/src/tokens.rs`, inside `mod tests`, add:

```rust
#[test]
fn p3_economy_tokens_present() {
    assert_eq!(glyph::HEAT_FULL, '█');
    assert_eq!(glyph::HEAT_SHADE, '░');
    assert_eq!(glyph::SPARK, ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']);
    assert_eq!(glyph::PIN, '●');
    // Heat colors reuse the status palette (spec §16: uniform language).
    assert_eq!(color::HEAT_HOT, color::OK);
    assert_eq!(color::HEAT_WARM, color::WARN);
    assert_eq!(color::HEAT_COLD, color::DIM);
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p zoid-tui --lib tokens::tests::p3_economy_tokens_present`
Expected: FAIL — `no associated item named HEAT_FULL`.

- [ ] **Step 3: Add the tokens**

In `mod glyph` (after `RECIPE`):

```rust
    pub const HEAT_FULL: char = '█';   // ⑤ heat bar — hot cell (Ⓡ4)
    pub const HEAT_SHADE: char = '░';  // ⑤ heat bar — empty cell (Ⓡ4)
    pub const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']; // churn sparkline ramp
    pub const PIN: char = '●';         // ⑤ pinned-item marker
```

In `mod color` (after `BUILD_BG`):

```rust
    // ⑤ context heat — reuse the status palette so the visual language stays uniform.
    pub const HEAT_HOT: Color = OK;
    pub const HEAT_WARM: Color = WARN;
    pub const HEAT_COLD: Color = DIM;
```

- [ ] **Step 4: Mirror in the authoritative table**

In `docs/ux/README.md`, extend the **Glyphs** line (§ "Visual language") by appending:

```
 · `█`/`░` heat bar (Ⓡ4) · `▁▂▃▄▅▆▇█` sparkline ramp (Ⓡ4) · `●` pinned item.
```

And add a line under **Status:**

```
**Heat (⑤a):** hot = ok green `#3fb950` · warm `#d29922` · cold = dim `#6e7681`.
```

- [ ] **Step 5: Run the test to confirm it passes**

Run: `cargo test -p zoid-tui --lib tokens::tests::p3_economy_tokens_present`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs docs/ux/README.md
git commit -m "feat(tokens): ⑤ heat/sparkline/pin glyphs + heat colors (Ⓡ4)"
```

---

### Task 2: Core event model — `Usage`, `ContextMutation`, `estimate_tokens`

**Files:**
- Modify: `crates/zoid-core/src/event.rs`
- Modify: `crates/zoid-core/src/projection.rs`
- Create: `crates/zoid-core/src/economy.rs` (just `estimate_tokens` + module decl this task; projections added in T3/T4)
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod economy;`)
- Test: inline in each modified file.

**Interfaces:**
- Consumes: existing `Event`, `EventKind`, `TokenStat`.
- Produces:
  - `enum MutationOp { Pin, Unpin, Evict, Restore }` (in `event.rs`, derives `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`).
  - `EventKind::Usage` (unit-like variant; the token numbers live in `Event.tokens`).
  - `EventKind::ContextMutation { item: String, op: MutationOp }`.
  - `economy::estimate_tokens(s: &str) -> u64`.
  - `conversation()` skips both new variants (no visual change).

- [ ] **Step 1: Write the failing tests**

In `crates/zoid-core/src/event.rs` `mod tests`:

```rust
#[test]
fn usage_and_mutation_round_trip() {
    let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    let usage = Event {
        id, parent: None, branch: BranchId::default(), ts: 5,
        kind: EventKind::Usage,
        tokens: Some(TokenStat { input: 100, output: 40, cached: 10 }),
    };
    let mutation = Event::new(id, None, 6, EventKind::ContextMutation {
        item: "file:src/a.rs".into(), op: MutationOp::Pin,
    });
    for ev in [usage, mutation] {
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
    }
}
```

In `crates/zoid-core/src/economy.rs` (new file) `mod tests`:

```rust
#[test]
fn estimate_tokens_is_chars_over_four_rounded_up() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("a"), 1);     // ceil(1/4)
    assert_eq!(estimate_tokens("abcd"), 1);  // 4/4
    assert_eq!(estimate_tokens("abcde"), 2); // ceil(5/4)
    // counts chars, not bytes
    assert_eq!(estimate_tokens("é"), 1);
}
```

In `crates/zoid-core/src/projection.rs` `mod tests`, add (using helpers already in that module — add small constructors if missing):

```rust
#[test]
fn conversation_ignores_usage_and_mutation() {
    let evs = vec![
        user(1, "hi"),
        Event::new(Ulid::from(2u128), None, 0, EventKind::Usage),
        Event::new(Ulid::from(3u128), None, 0, EventKind::ContextMutation {
            item: "file:a".into(), op: crate::event::MutationOp::Evict,
        }),
        delta(4, "yo"),
    ];
    let msgs = conversation(&evs);
    assert_eq!(msgs, vec![
        ChatMsg::User("hi".into()),
        ChatMsg::Assistant { text: "yo".into(), tool_calls: vec![] },
    ]);
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p zoid-core`
Expected: compile error — `Usage`/`ContextMutation`/`MutationOp`/`economy` don't exist.

- [ ] **Step 3: Implement**

In `crates/zoid-core/src/event.rs`, add above `EventKind`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MutationOp {
    Pin,
    Unpin,
    Evict,
    Restore,
}
```

Add to `EventKind` (before the closing brace):

```rust
    /// A turn's token usage. The numbers live in `Event.tokens`; this variant
    /// is the carrier so the economy projections can sum real counts. Ignored
    /// by the conversation projection.
    Usage,
    /// A manual or automatic change to the context window, targeting a
    /// `ContextItem` by its stable `key`.
    ContextMutation { item: String, op: MutationOp },
```

In `crates/zoid-core/src/economy.rs` (new):

```rust
//! The context-economy projections (spec §8): token ledger, churn timeline,
//! and the per-item token estimator. All pure functions of the event log.

/// Estimate the token cost of a string as `ceil(chars / 4)` — the standard
/// rough heuristic (≈4 chars/token). Aggregate ledger numbers use real
/// provider `Usage`; this is for per-item context sizing where the provider
/// gives no breakdown.
pub fn estimate_tokens(s: &str) -> u64 {
    let chars = s.chars().count() as u64;
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    // (estimate_tokens test from Step 1)
}
```

In `crates/zoid-core/src/lib.rs`, add `pub mod economy;` (keep modules alphas/grouped with the others).

In `crates/zoid-core/src/projection.rs`, add the new match arms inside `conversation`'s `for` loop (do **not** flush — these are orthogonal to turn boundaries):

```rust
            EventKind::Usage | EventKind::ContextMutation { .. } => {
                // Economy bookkeeping; not part of the conversation projection.
            }
```

If `projection.rs`'s test helpers lack `delta`/`user`, they already exist (confirmed in the module). Ensure `use crate::event::EventKind;` covers the new variants (it does — glob not needed).

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cargo test -p zoid-core`
Expected: PASS (all existing + 3 new).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/economy.rs crates/zoid-core/src/lib.rs crates/zoid-core/src/projection.rs
git commit -m "feat(core): Usage + ContextMutation events; estimate_tokens; conversation skips them"
```

---

### Task 3: `TokenLedger` projection

**Files:**
- Modify: `crates/zoid-core/src/economy.rs`
- Test: inline `mod tests` + a `proptest!` block.

**Interfaces:**
- Consumes: `Event`, `Event.tokens: Option<TokenStat>`.
- Produces:
  - `struct TokenLedger { pub input: u64, pub output: u64, pub cached: u64, pub total: u64 }` (derives `Debug, Clone, Copy, PartialEq, Eq, Default`).
  - `fn token_ledger(events: &[Event]) -> TokenLedger` — sums `e.tokens` across the whole log. `total = input + output` (cached is a subset of input, reported separately, not double-added).

- [ ] **Step 1: Write the failing tests**

In `economy.rs` `mod tests`:

```rust
use crate::event::{Event, EventKind, TokenStat};
use ulid::Ulid;

fn usage(input: u64, output: u64, cached: u64) -> Event {
    Event {
        id: Ulid::new(), parent: None, branch: Default::default(), ts: 0,
        kind: EventKind::Usage, tokens: Some(TokenStat { input, output, cached }),
    }
}

#[test]
fn ledger_sums_usage_and_ignores_untokened_events() {
    let evs = vec![
        Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() }),
        usage(100, 40, 10),
        usage(50, 20, 5),
    ];
    let l = token_ledger(&evs);
    assert_eq!(l.input, 150);
    assert_eq!(l.output, 60);
    assert_eq!(l.cached, 15);
    assert_eq!(l.total, 210); // input + output, cached not double-counted
}

#[test]
fn ledger_of_empty_log_is_zero() {
    assert_eq!(token_ledger(&[]), TokenLedger::default());
}
```

Add a property test (the module already has access to `proptest` as a dev-dep — confirm `use proptest::prelude::*;` at the top of `mod tests`):

```rust
proptest! {
    #[test]
    fn ledger_total_equals_input_plus_output(stats in proptest::collection::vec((0u64..10_000, 0u64..10_000, 0u64..10_000), 0..50)) {
        let evs: Vec<Event> = stats.iter().map(|&(i,o,c)| usage(i,o,c)).collect();
        let l = token_ledger(&evs);
        prop_assert_eq!(l.total, l.input + l.output);
        prop_assert_eq!(l.input, stats.iter().map(|s| s.0).sum::<u64>());
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core economy`
Expected: FAIL — `TokenLedger` / `token_ledger` undefined.

- [ ] **Step 3: Implement**

In `economy.rs` (module body, above tests):

```rust
use crate::event::Event;

/// Aggregate token spend over a scope of the log (spec §8). `total` is
/// `input + output`; `cached` is the cache-read subset of input, surfaced
/// separately (it is *not* added into `total` again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub total: u64,
}

/// Fold the log into a `TokenLedger` by summing every event's `tokens`.
pub fn token_ledger(events: &[Event]) -> TokenLedger {
    let mut l = TokenLedger::default();
    for e in events {
        if let Some(t) = e.tokens {
            l.input += t.input;
            l.output += t.output;
            l.cached += t.cached;
        }
    }
    l.total = l.input + l.output;
    l
}
```

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-core economy`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/economy.rs
git commit -m "feat(core): TokenLedger projection (sums Event.tokens)"
```

---

### Task 4: `ChurnTimeline` projection

**Files:**
- Modify: `crates/zoid-core/src/economy.rs`
- Test: inline.

**Interfaces:**
- Consumes: `Event`, `EventKind`, `Event.tokens`, `estimate_tokens`.
- Produces:
  - `struct ChurnPoint { pub turn: usize, pub tokens: u64, pub resent_tokens: u64 }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `struct ChurnTimeline { pub points: Vec<ChurnPoint> }` (`Debug, Clone, PartialEq, Eq, Default`).
  - `fn churn_timeline(events: &[Event]) -> ChurnTimeline`.

**Definition:** A *turn* starts at each `UserMessage`. `tokens` = the turn's real usage (sum of `Event.tokens` totals within the turn). `resent_tokens` = the estimated token cost of file paths referenced **in this turn that were also referenced in any earlier turn** (the #1 silent cost — re-sent files; spec §8 ⑤c). File path is taken from a tool call's args (`path`/`file_path`/`file`).

- [ ] **Step 1: Write the failing tests**

```rust
fn umsg(text: &str) -> Event {
    Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: text.into() })
}
fn toolcall_read(id: &str, path: &str) -> Event {
    Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
        id: id.into(), name: "read_file".into(),
        args: format!(r#"{{"path":"{path}"}}"#),
    })
}

#[test]
fn churn_segments_by_user_message_and_flags_resent_files() {
    let evs = vec![
        umsg("turn 1"),
        toolcall_read("c1", "src/a.rs"),
        usage(100, 20, 0),
        umsg("turn 2"),
        toolcall_read("c2", "src/a.rs"), // re-sent file → resent
        toolcall_read("c3", "src/b.rs"), // new file → not resent
        usage(140, 30, 0),
    ];
    let t = churn_timeline(&evs);
    assert_eq!(t.points.len(), 2);
    assert_eq!(t.points[0].turn, 0);
    assert_eq!(t.points[0].tokens, 120);       // 100+20
    assert_eq!(t.points[0].resent_tokens, 0);  // first sight of a.rs
    assert_eq!(t.points[1].turn, 1);
    assert_eq!(t.points[1].tokens, 170);       // 140+30
    // a.rs re-sent: estimate_tokens of its path-based cost is > 0
    assert!(t.points[1].resent_tokens > 0);
}

#[test]
fn churn_empty_when_no_turns() {
    assert_eq!(churn_timeline(&[]), ChurnTimeline::default());
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core economy::tests::churn`
Expected: FAIL — undefined types.

- [ ] **Step 3: Implement**

Add to `economy.rs`:

```rust
use crate::event::EventKind;
use std::collections::HashSet;

/// One turn's churn (spec §8 ⑤c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChurnPoint {
    pub turn: usize,
    pub tokens: u64,
    pub resent_tokens: u64,
}

/// Per-turn token deltas with re-sent-file flagging.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChurnTimeline {
    pub points: Vec<ChurnPoint>,
}

/// Extract a file path from a tool call's JSON args, trying common keys.
pub(crate) fn tool_path(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    for key in ["path", "file_path", "file"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

pub fn churn_timeline(events: &[Event]) -> ChurnTimeline {
    let mut points: Vec<ChurnPoint> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut cur: Option<ChurnPoint> = None;

    for e in events {
        match &e.kind {
            EventKind::UserMessage { .. } => {
                if let Some(p) = cur.take() {
                    points.push(p);
                }
                cur = Some(ChurnPoint { turn: points.len(), tokens: 0, resent_tokens: 0 });
            }
            EventKind::ToolCall { args, .. } => {
                if let (Some(p), Some(path)) = (cur.as_mut(), tool_path(args)) {
                    if seen_paths.contains(&path) {
                        p.resent_tokens += estimate_tokens(&path).max(1);
                    }
                    seen_paths.insert(path);
                }
            }
            _ => {}
        }
        if let (Some(p), Some(t)) = (cur.as_mut(), e.tokens) {
            p.tokens += t.input + t.output;
        }
    }
    if let Some(p) = cur.take() {
        points.push(p);
    }
    ChurnTimeline { points }
}
```

> Note: `resent_tokens` here is a relative magnitude (path-cost proxy), enough to drive the sparkline and the "re-sent" nudge. T5 computes the real per-file token cost; a later phase can join them. Keep `.max(1)` so a re-sent file always registers.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-core economy`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/economy.rs
git commit -m "feat(core): ChurnTimeline projection (per-turn tokens + re-sent files)"
```

---

### Task 5: `ContextWindow` + heat + mutation fold (all item kinds)

**Files:**
- Create: `crates/zoid-core/src/context.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod context;`)
- Test: inline `mod tests` + `proptest!`.

**Interfaces:**
- Consumes: `Event`, `EventKind`, `MutationOp`, `economy::{estimate_tokens, tool_path}`.
- Produces:
  - `enum Heat { Cold, Warm, Hot }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `enum ItemKind { System, Message, ToolResult, File }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `struct ContextItem { pub key: String, pub label: String, pub kind: ItemKind, pub tokens: u64, pub heat: Heat, pub pinned: bool, pub evicted: bool }` (`Debug, Clone, PartialEq, Eq`).
  - `struct ContextWindow { pub items: Vec<ContextItem>, pub total_tokens: u64 }` (`Debug, Clone, PartialEq, Eq, Default`).
  - `fn context_window(events: &[Event]) -> ContextWindow`.
  - consts: `HOT_REFS: u32 = 3`, `WARM_REFS: u32 = 2`, `COLD_RECENCY_TURNS: usize = 3`.

**Model:** Context items span **all token-spending sources**, not just files:
- **Message** items: each `UserMessage` and each collapsed assistant turn (run of `ModelDelta`) → one item; `tokens = estimate_tokens(text)`.
- **File** items: a `ToolResult` whose originating `ToolCall` (matched by `id`) carries a file path → keyed `file:{path}`, latest content wins; `tokens = estimate_tokens(output)`.
- **ToolResult** items: any other `ToolResult` (shell/test/grep output) → keyed `tool:{name}:{id}`.
- (**System** prompt overhead is reflected in the ledger's input tokens; modeling it as a pinned, non-evictable `ItemKind::System` item is a documented post-P3 refinement, since the prompt text lives in the bin, not the log.)

**Heat:** `refs(item)` = number of tool calls targeting that file path (File items) or `1` otherwise; `recency` = turns since the item last appeared. `Hot` if `refs >= HOT_REFS` or last seen in the current turn; `Warm` if `refs >= WARM_REFS` or seen within `COLD_RECENCY_TURNS`; else `Cold`.

**Mutations:** fold `ContextMutation { item, op }` in log order onto matching `key`: `Pin→pinned=true`, `Unpin→pinned=false`, `Evict→evicted=true`, `Restore→evicted=false`. Pin and evict are independent flags; the assembler (T6) resolves precedence (pin wins).

`items` sorted by `tokens` desc, ties broken by `key` asc (stable, deterministic for snapshots).

- [ ] **Step 1: Write the failing tests**

```rust
use crate::context::*;
use crate::event::{Event, EventKind, MutationOp};
use ulid::Ulid;

fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
fn u(t: &str) -> Event { ev(EventKind::UserMessage { text: t.into() }) }
fn call(id: &str, name: &str, path: &str) -> Event {
    ev(EventKind::ToolCall { id: id.into(), name: name.into(), args: format!(r#"{{"path":"{path}"}}"#) })
}
fn result(id: &str, name: &str, out: &str) -> Event {
    ev(EventKind::ToolResult { id: id.into(), name: name.into(), output: out.into(), is_error: false })
}

#[test]
fn window_has_items_across_kinds() {
    let evs = vec![
        u("read the config"),
        call("c1", "read_file", "cfg.toml"),
        result("c1", "read_file", "key = 1\nkey2 = 2\n"),
        call("c2", "shell", "n/a"),                 // no path → not a File
        result("c2", "shell", "lots of shell output here"),
    ];
    let w = context_window(&evs);
    let kinds: Vec<ItemKind> = w.items.iter().map(|i| i.kind).collect();
    assert!(kinds.contains(&ItemKind::Message));
    assert!(kinds.contains(&ItemKind::File));
    assert!(kinds.contains(&ItemKind::ToolResult));
    // File item keyed by path, with positive token estimate.
    let f = w.items.iter().find(|i| i.kind == ItemKind::File).unwrap();
    assert_eq!(f.key, "file:cfg.toml");
    assert!(f.tokens > 0);
    // Sorted by tokens desc.
    for pair in w.items.windows(2) {
        assert!(pair[0].tokens >= pair[1].tokens);
    }
    assert_eq!(w.total_tokens, w.items.iter().map(|i| i.tokens).sum::<u64>());
}

#[test]
fn pin_and_evict_fold_onto_items() {
    let evs = vec![
        u("go"),
        call("c1", "read_file", "a.rs"),
        result("c1", "read_file", "fn main() {}"),
        ev(EventKind::ContextMutation { item: "file:a.rs".into(), op: MutationOp::Pin }),
        ev(EventKind::ContextMutation { item: "file:a.rs".into(), op: MutationOp::Evict }),
    ];
    let w = context_window(&evs);
    let a = w.items.iter().find(|i| i.key == "file:a.rs").unwrap();
    assert!(a.pinned);
    assert!(a.evicted); // both flags set; precedence resolved by the assembler
}

#[test]
fn repeated_reads_make_a_file_hot_single_item() {
    let mut evs = vec![u("go")];
    for i in 0..3 {
        evs.push(call(&format!("c{i}"), "read_file", "hot.rs"));
        evs.push(result(&format!("c{i}"), "read_file", "fn x() {}"));
    }
    let w = context_window(&evs);
    let hot: Vec<_> = w.items.iter().filter(|i| i.key == "file:hot.rs").collect();
    assert_eq!(hot.len(), 1, "reads of one path collapse to one item");
    assert_eq!(hot[0].heat, Heat::Hot);
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core context`
Expected: FAIL — module/types undefined.

- [ ] **Step 3: Implement**

`crates/zoid-core/src/context.rs`:

```rust
//! The `ContextWindow` projection (spec §8 ⑤a): the current context as a list
//! of token-spending items — system, messages, tool results, files — each with
//! a token cost, a heat heuristic, and pin/evict state folded from
//! `ContextMutation` events. Pure.

use crate::economy::{estimate_tokens, tool_path};
use crate::event::{Event, EventKind};
use std::collections::HashMap;

pub const HOT_REFS: u32 = 3;
pub const WARM_REFS: u32 = 2;
pub const COLD_RECENCY_TURNS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heat {
    Cold,
    Warm,
    Hot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    System,
    Message,
    ToolResult,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub key: String,
    pub label: String,
    pub kind: ItemKind,
    pub tokens: u64,
    pub heat: Heat,
    pub pinned: bool,
    pub evicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextWindow {
    pub items: Vec<ContextItem>,
    pub total_tokens: u64,
}

// Internal accumulator while folding.
struct Acc {
    key: String,
    label: String,
    kind: ItemKind,
    tokens: u64,
    refs: u32,
    last_turn: usize,
}

pub fn context_window(events: &[Event]) -> ContextWindow {
    let mut order: Vec<String> = Vec::new(); // first-seen order of keys
    let mut acc: HashMap<String, Acc> = HashMap::new();
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut turn: usize = 0;
    let mut msg_seq: usize = 0;

    let mut upsert = |order: &mut Vec<String>, acc: &mut HashMap<String, Acc>,
                      key: String, label: String, kind: ItemKind, tokens: u64, turn: usize| {
        let e = acc.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Acc { key, label, kind, tokens: 0, refs: 0, last_turn: turn }
        });
        e.tokens = tokens; // latest content wins
        e.refs += 1;
        e.last_turn = turn;
    };

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => {
                turn += 1;
                let key = format!("msg:{msg_seq}");
                msg_seq += 1;
                upsert(&mut order, &mut acc, key, truncate(text, 40), ItemKind::Message, estimate_tokens(text), turn);
            }
            EventKind::AssistantMessage { text } => {
                let key = format!("msg:{msg_seq}");
                msg_seq += 1;
                upsert(&mut order, &mut acc, key, truncate(text, 40), ItemKind::Message, estimate_tokens(text), turn);
            }
            EventKind::ToolCall { id, args, .. } => {
                if let Some(path) = tool_path(args) {
                    call_path.insert(id.clone(), path.clone());
                    // a call targeting a known file counts as a reference (drives heat)
                    let key = format!("file:{path}");
                    if let Some(a) = acc.get_mut(&key) {
                        a.refs += 1;
                        a.last_turn = turn;
                    }
                }
            }
            EventKind::ToolResult { id, name, output, .. } => {
                if let Some(path) = call_path.get(id) {
                    let key = format!("file:{path}");
                    upsert(&mut order, &mut acc, key, path.clone(), ItemKind::File, estimate_tokens(output), turn);
                } else {
                    let key = format!("tool:{name}:{id}");
                    upsert(&mut order, &mut acc, key, name.clone(), ItemKind::ToolResult, estimate_tokens(output), turn);
                }
            }
            _ => {}
        }
    }

    let last_turn_global = turn;
    let mut items: Vec<ContextItem> = order
        .iter()
        .map(|k| {
            let a = &acc[k];
            ContextItem {
                key: a.key.clone(),
                label: a.label.clone(),
                kind: a.kind,
                tokens: a.tokens,
                heat: heat_of(a.refs, a.last_turn, last_turn_global),
                pinned: false,
                evicted: false,
            }
        })
        .collect();

    // Fold mutations (log order; last write wins per flag).
    for e in events {
        if let EventKind::ContextMutation { item, op } = &e.kind {
            if let Some(it) = items.iter_mut().find(|i| &i.key == item) {
                use crate::event::MutationOp::*;
                match op {
                    Pin => it.pinned = true,
                    Unpin => it.pinned = false,
                    Evict => it.evicted = true,
                    Restore => it.evicted = false,
                }
            }
        }
    }

    // Sort by tokens desc, then key asc (deterministic for snapshots).
    items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.key.cmp(&b.key)));
    let total_tokens = items.iter().map(|i| i.tokens).sum();
    ContextWindow { items, total_tokens }
}

fn heat_of(refs: u32, last_turn: usize, current_turn: usize) -> Heat {
    let recency = current_turn.saturating_sub(last_turn);
    if refs >= HOT_REFS || recency == 0 {
        Heat::Hot
    } else if refs >= WARM_REFS || recency <= COLD_RECENCY_TURNS {
        Heat::Warm
    } else {
        Heat::Cold
    }
}

fn truncate(s: &str, max: usize) -> String {
    let one_line = s.lines().next().unwrap_or("");
    if one_line.chars().count() > max {
        let head: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        one_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // (tests from Step 1)
}
```

Make `economy::tool_path` visible to `context`: it is `pub(crate)` (declared in T4) — good. Add `pub mod context;` to `lib.rs`.

> The `refs >= HOT_REFS` test: three reads of `hot.rs` produce 3 `ToolResult` upserts (`refs += 1` each) plus 3 `ToolCall` reference bumps — well over `HOT_REFS`. Also `recency == 0` (seen in the final turn) ⇒ Hot regardless.

- [ ] **Step 4: Add a proptest invariant**

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn total_equals_sum_and_pin_evict_independent(n in 0usize..30) {
        let mut evs = vec![u("seed")];
        for i in 0..n {
            evs.push(call(&format!("c{i}"), "read_file", &format!("f{}.rs", i % 5)));
            evs.push(result(&format!("c{i}"), "read_file", &"x".repeat(i + 1)));
        }
        let w = context_window(&evs);
        prop_assert_eq!(w.total_tokens, w.items.iter().map(|i| i.tokens).sum::<u64>());
        // sorted desc
        for pair in w.items.windows(2) {
            prop_assert!(pair[0].tokens >= pair[1].tokens);
        }
    }
}
```

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid-core context`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/context.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): ContextWindow projection — items across kinds, heat, pin/evict fold"
```

---

### Task 6: Constructed-context assembler primitive

**Files:**
- Create: `crates/zoid-core/src/assembler.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod assembler;`)
- Test: inline.

**Interfaces:**
- Consumes: `context::{ContextWindow, ContextItem, Heat}`.
- Produces:
  - `struct ContextPolicy { pub token_ceiling: Option<u64>, pub auto_evict_cold: bool, pub compact_threshold: Option<u64> }`, `impl Default` with `auto_evict_cold = true`, others `None`.
  - `struct ContextSelection { pub included: Vec<ContextItem>, pub excluded: Vec<ContextItem>, pub tokens: u64, pub compacted: bool }` (`Debug, Clone, PartialEq, Eq, Default`).
  - `fn assemble_context(window: &ContextWindow, policy: &ContextPolicy) -> ContextSelection`.

**Rules (precedence):**
1. **Pinned ⇒ always included** (overrides evict, auto-evict, and budget).
2. Manually `evicted` (and not pinned) ⇒ excluded.
3. If `auto_evict_cold` and item is `Heat::Cold` and not pinned ⇒ excluded.
4. If `compact_threshold = Some(t)` and `window.total_tokens > t` ⇒ set `compacted = true` and force rule 3 on (compaction drops cold first).
5. If `token_ceiling = Some(c)`: keep pinned first, then remaining included items in `window.items` order (already tokens-desc) while cumulative `tokens <= c`; drop the rest to `excluded`. Pinned items are always kept even if they alone exceed `c`.
6. `tokens` = sum of `included`.

> **P3 scope:** this function is **not** called from `build_request`. It is the pure substrate P5 wires into subagent dispatch (and later the live Chat request). Do not modify the agent loop here.

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use crate::context::{ContextItem, ContextWindow, Heat, ItemKind};

fn item(key: &str, tokens: u64, heat: Heat, pinned: bool, evicted: bool) -> ContextItem {
    ContextItem { key: key.into(), label: key.into(), kind: ItemKind::File, tokens, heat, pinned, evicted }
}
fn window(items: Vec<ContextItem>) -> ContextWindow {
    let total = items.iter().map(|i| i.tokens).sum();
    ContextWindow { items, total_tokens: total }
}

#[test]
fn pin_overrides_evict_and_auto_cold() {
    let w = window(vec![
        item("pinned-cold", 100, Heat::Cold, true, true), // pinned wins
        item("cold", 50, Heat::Cold, false, false),       // auto-evicted (default on)
        item("hot", 30, Heat::Hot, false, false),
    ]);
    let s = assemble_context(&w, &ContextPolicy::default());
    let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
    assert!(keys.contains(&"pinned-cold"));
    assert!(keys.contains(&"hot"));
    assert!(!keys.contains(&"cold")); // auto_evict_cold default true
    assert_eq!(s.tokens, 130);
}

#[test]
fn manual_evict_excludes_unless_pinned() {
    let w = window(vec![item("e", 10, Heat::Hot, false, true)]);
    let s = assemble_context(&w, &ContextPolicy { auto_evict_cold: false, ..Default::default() });
    assert!(s.included.is_empty());
    assert_eq!(s.excluded.len(), 1);
}

#[test]
fn ceiling_drops_lowest_priority_keeps_pinned() {
    let w = window(vec![
        item("big-pinned", 1000, Heat::Warm, true, false),
        item("a", 60, Heat::Hot, false, false),
        item("b", 60, Heat::Hot, false, false),
    ]);
    let s = assemble_context(&w, &ContextPolicy { token_ceiling: Some(100), auto_evict_cold: false, ..Default::default() });
    let keys: Vec<&str> = s.included.iter().map(|i| i.key.as_str()).collect();
    assert!(keys.contains(&"big-pinned")); // pinned kept even over ceiling
    assert!(keys.contains(&"a"));          // first non-pinned fits cumulative ≤100
    assert!(!keys.contains(&"b"));         // would exceed
}

#[test]
fn compaction_flag_trips_over_threshold() {
    let w = window(vec![item("cold", 500, Heat::Cold, false, false), item("hot", 10, Heat::Hot, false, false)]);
    let s = assemble_context(&w, &ContextPolicy { compact_threshold: Some(100), auto_evict_cold: false, ..Default::default() });
    assert!(s.compacted);
    assert!(s.included.iter().all(|i| i.key != "cold")); // compaction forced cold-evict
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core assembler`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement**

`crates/zoid-core/src/assembler.rs`:

```rust
//! The constructed-context assembler (spec §4.4/§8): turn a `ContextWindow`
//! plus a `ContextPolicy` into the set of items that *would* be sent. Pure and
//! standalone — P5 wires it into subagent dispatch and the live request; in P3
//! it only feeds the economy view-model. Pin always overrides eviction.

use crate::context::{ContextItem, ContextWindow, Heat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPolicy {
    pub token_ceiling: Option<u64>,
    pub auto_evict_cold: bool,
    pub compact_threshold: Option<u64>,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self { token_ceiling: None, auto_evict_cold: true, compact_threshold: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextSelection {
    pub included: Vec<ContextItem>,
    pub excluded: Vec<ContextItem>,
    pub tokens: u64,
    pub compacted: bool,
}

pub fn assemble_context(window: &ContextWindow, policy: &ContextPolicy) -> ContextSelection {
    let compacted = policy
        .compact_threshold
        .is_some_and(|t| window.total_tokens > t);
    let drop_cold = policy.auto_evict_cold || compacted;

    let mut included: Vec<ContextItem> = Vec::new();
    let mut excluded: Vec<ContextItem> = Vec::new();

    // Pass 1: pin/evict/auto-cold filtering (order preserved = tokens-desc).
    let mut survivors: Vec<ContextItem> = Vec::new();
    for it in &window.items {
        if it.pinned {
            survivors.push(it.clone());
        } else if it.evicted || (drop_cold && it.heat == Heat::Cold) {
            excluded.push(it.clone());
        } else {
            survivors.push(it.clone());
        }
    }

    // Pass 2: token ceiling (pinned always kept; non-pinned fit cumulatively).
    let mut running: u64 = survivors.iter().filter(|i| i.pinned).map(|i| i.tokens).sum();
    for it in survivors {
        if it.pinned {
            included.push(it);
            continue;
        }
        match policy.token_ceiling {
            Some(c) if running + it.tokens > c => excluded.push(it),
            _ => {
                running += it.tokens;
                included.push(it);
            }
        }
    }

    let tokens = included.iter().map(|i| i.tokens).sum();
    ContextSelection { included, excluded, tokens, compacted }
}

#[cfg(test)]
mod tests {
    use super::*;
    // (tests from Step 1)
}
```

Add `pub mod assembler;` to `lib.rs`.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-core assembler`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/assembler.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): constructed-context assembler (policy → selection); P5 substrate"
```

---

### Task 7: Economy view-model (`heat_bar`, `sparkline`, `EconomyView`)

**Files:**
- Create: `crates/zoid-tui/src/economy_view.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod economy_view;` + re-export `EconomyView`)
- Test: inline.

**Interfaces:**
- Consumes: `zoid_core::context::{ContextWindow, ContextItem, Heat, ItemKind}`, `zoid_core::economy::{ChurnTimeline, TokenLedger}`, `zoid_core::assembler::ContextPolicy`, `tokens::{glyph, color}`.
- Produces (all pure):
  - `fn human_tokens(n: u64) -> String` — `4000 → "4k"`, `1_200_000 → "1.2M"`, `<1000 → "123"`.
  - `fn heat_bar(heat: Heat) -> String` — `Hot → "██"`, `Warm → "█░"`, `Cold → "░░"` (built from `glyph::HEAT_FULL`/`HEAT_SHADE`).
  - `fn heat_color(heat: Heat) -> ratatui::style::Color`.
  - `fn sparkline(values: &[u64]) -> String` — maps to `glyph::SPARK` (max→top; empty→`""`; all-zero→all `SPARK[0]`).
  - `struct EconomyRow { pub pinned: bool, pub label: String, pub tokens: String, pub heat: Heat, pub cold: bool }`.
  - `struct EconomyView { pub rows: Vec<EconomyRow>, pub churn: String, pub ledger: String, pub over_ceiling: bool, pub auto_evict_cold: bool, pub selected: usize }`.
  - `impl EconomyView { pub fn build(window: &ContextWindow, churn: &ChurnTimeline, ledger: &TokenLedger, policy: &ContextPolicy, selected: usize) -> Self }`.

`ledger` label: if `policy.token_ceiling = Some(c)` → `"{used}/{c}"` humanized (e.g. `"142k/200k"`), else humanized total. `over_ceiling = ceiling.is_some_and(|c| total > c)`. `rows` mirror `window.items` (already tokens-desc), `cold = heat == Cold`.

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use zoid_core::context::{ContextItem, ContextWindow, Heat, ItemKind};
use zoid_core::economy::{ChurnPoint, ChurnTimeline, TokenLedger};
use zoid_core::assembler::ContextPolicy;

#[test]
fn human_tokens_scales() {
    assert_eq!(human_tokens(0), "0");
    assert_eq!(human_tokens(123), "123");
    assert_eq!(human_tokens(4000), "4k");
    assert_eq!(human_tokens(4500), "4k");        // floor to k
    assert_eq!(human_tokens(1_200_000), "1.2M");
}

#[test]
fn heat_bar_glyphs() {
    assert_eq!(heat_bar(Heat::Hot), "██");
    assert_eq!(heat_bar(Heat::Warm), "█░");
    assert_eq!(heat_bar(Heat::Cold), "░░");
}

#[test]
fn sparkline_maps_range() {
    assert_eq!(sparkline(&[]), "");
    assert_eq!(sparkline(&[0, 0]), "▁▁");
    let s = sparkline(&[1, 4, 8]);
    assert_eq!(s.chars().count(), 3);
    assert_eq!(s.chars().last().unwrap(), '█'); // max maps to top of ramp
}

#[test]
fn build_populates_rows_and_ledger() {
    let w = ContextWindow {
        items: vec![
            ContextItem { key: "file:a.rs".into(), label: "a.rs".into(), kind: ItemKind::File, tokens: 4000, heat: Heat::Hot, pinned: true, evicted: false },
            ContextItem { key: "file:c.sql".into(), label: "c.sql".into(), kind: ItemKind::File, tokens: 5000, heat: Heat::Cold, pinned: false, evicted: false },
        ],
        total_tokens: 9000,
    };
    let churn = ChurnTimeline { points: vec![ChurnPoint { turn: 0, tokens: 10, resent_tokens: 0 }, ChurnPoint { turn: 1, tokens: 80, resent_tokens: 5 }] };
    let ledger = TokenLedger { input: 9000, output: 1000, cached: 0, total: 10_000 };
    let policy = ContextPolicy { token_ceiling: Some(200_000), ..Default::default() };
    let v = EconomyView::build(&w, &churn, &ledger, &policy, 0);
    assert_eq!(v.rows.len(), 2);
    assert!(v.rows[0].pinned);
    assert!(v.rows[1].cold);
    assert_eq!(v.ledger, "10k/200k");
    assert!(!v.over_ceiling);
    assert!(v.auto_evict_cold);
    assert_eq!(v.churn.chars().count(), 2);
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib economy_view`
Expected: FAIL — module undefined.

- [ ] **Step 3: Implement**

`crates/zoid-tui/src/economy_view.rs`:

```rust
//! Pure view-model for the ⑤ economy drawer (Ⓡ4 dataviz). Turns core
//! projections into render-ready strings (heat bars, churn sparkline, token
//! ledger). No `Frame`; unit-tested independently of rendering.

use crate::tokens::{color, glyph};
use ratatui::style::Color;
use zoid_core::assembler::ContextPolicy;
use zoid_core::context::{ContextWindow, Heat};
use zoid_core::economy::{ChurnTimeline, TokenLedger};

/// Compact token count: `4000 → "4k"`, `1_200_000 → "1.2M"`.
pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        format!("{m:.1}M")
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

pub fn heat_bar(heat: Heat) -> String {
    let (a, b) = match heat {
        Heat::Hot => (glyph::HEAT_FULL, glyph::HEAT_FULL),
        Heat::Warm => (glyph::HEAT_FULL, glyph::HEAT_SHADE),
        Heat::Cold => (glyph::HEAT_SHADE, glyph::HEAT_SHADE),
    };
    format!("{a}{b}")
}

pub fn heat_color(heat: Heat) -> Color {
    match heat {
        Heat::Hot => color::HEAT_HOT,
        Heat::Warm => color::HEAT_WARM,
        Heat::Cold => color::HEAT_COLD,
    }
}

/// Map values onto the 8-step sparkline ramp (max → top).
pub fn sparkline(values: &[u64]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| {
            let idx = if max == 0 {
                0
            } else {
                ((v as u128 * (glyph::SPARK.len() as u128 - 1)) / max as u128) as usize
            };
            glyph::SPARK[idx]
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomyRow {
    pub pinned: bool,
    pub label: String,
    pub tokens: String,
    pub heat: Heat,
    pub cold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomyView {
    pub rows: Vec<EconomyRow>,
    pub churn: String,
    pub ledger: String,
    pub over_ceiling: bool,
    pub auto_evict_cold: bool,
    pub selected: usize,
}

impl EconomyView {
    pub fn build(
        window: &ContextWindow,
        churn: &ChurnTimeline,
        ledger: &TokenLedger,
        policy: &ContextPolicy,
        selected: usize,
    ) -> Self {
        let rows = window
            .items
            .iter()
            .map(|i| EconomyRow {
                pinned: i.pinned,
                label: i.label.clone(),
                tokens: human_tokens(i.tokens),
                heat: i.heat,
                cold: i.heat == Heat::Cold,
            })
            .collect();
        let churn_vals: Vec<u64> = churn.points.iter().map(|p| p.tokens).collect();
        let used = ledger.total;
        let ledger_label = match policy.token_ceiling {
            Some(c) => format!("{}/{}", human_tokens(used), human_tokens(c)),
            None => human_tokens(used),
        };
        Self {
            rows,
            churn: sparkline(&churn_vals),
            ledger: ledger_label,
            over_ceiling: policy.token_ceiling.is_some_and(|c| used > c),
            auto_evict_cold: policy.auto_evict_cold,
            selected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // (tests from Step 1)
}
```

In `crates/zoid-tui/src/lib.rs`: add `pub mod economy_view;` and `pub use economy_view::EconomyView;`.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib economy_view`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/economy_view.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): EconomyView view-model — heat bars, churn sparkline, token ledger (Ⓡ4)"
```

---

### Task 8: Data-driven drawer-body rects in layout

**Files:**
- Modify: `crates/zoid-tui/src/layout.rs`
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `state::{DrawerId, ShellState}`.
- Produces:
  - `ShellLayout` gains `pub drawer_bodies: Vec<(DrawerId, Rect)>` (body rect for each *open* drawer).
  - `pub const DRAWER_BODY_ROWS: u16 = 4;` (default) and `pub const ECONOMY_BODY_ROWS: u16 = 6;`.
  - `fn drawer_body_rows(id: DrawerId) -> u16` (Economy → 6, else 4).

This resolves the P2-deferred debt: render no longer computes body rects inline; it reads `layout.drawer_bodies`. The header-stacking math in `compute` must use `drawer_body_rows(id)` for the open-body gap (replacing the inline `let body_rows = if d.open { 4 } else { 0 };`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn open_drawer_gets_a_body_rect_sized_by_kind() {
    let mut s = ShellState::new(); // Economy open by default
    s.toggle_drawer(DrawerId::Files); // Files now open too
    let l = compute(area(100, 30), &s);
    let econ = l.drawer_bodies.iter().find(|(id, _)| *id == DrawerId::Economy).unwrap().1;
    let files = l.drawer_bodies.iter().find(|(id, _)| *id == DrawerId::Files).unwrap().1;
    assert_eq!(econ.height, ECONOMY_BODY_ROWS);
    assert_eq!(files.height, DRAWER_BODY_ROWS);
    // closed drawers have no body
    assert!(l.drawer_bodies.iter().all(|(id, _)| *id != DrawerId::Branch));
    // body sits directly under its header
    let econ_hdr = l.drawer_headers.iter().find(|(id, _)| *id == DrawerId::Economy).unwrap().1;
    assert_eq!(econ.y, econ_hdr.y + 1);
}

#[test]
fn headers_stack_below_taller_economy_body() {
    let s = ShellState::new(); // only Economy open (6-row body)
    let l = compute(area(100, 30), &s);
    let econ_hdr = l.drawer_headers[0].1;
    let files_hdr = l.drawer_headers[1].1;
    // header(1) + ECONOMY_BODY_ROWS + 1 spacer
    assert_eq!(files_hdr.y, econ_hdr.y + 1 + ECONOMY_BODY_ROWS + 1);
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib layout`
Expected: FAIL — `drawer_bodies` / consts undefined.

- [ ] **Step 3: Implement**

Add consts near the top of `layout.rs`:

```rust
/// Default drawer body height in rows (P3).
pub const DRAWER_BODY_ROWS: u16 = 4;
/// The economy ⑤ drawer needs more rows (items + churn + ledger + toggle).
pub const ECONOMY_BODY_ROWS: u16 = 6;

/// Body height for a drawer kind.
pub fn drawer_body_rows(id: DrawerId) -> u16 {
    match id {
        DrawerId::Economy => ECONOMY_BODY_ROWS,
        _ => DRAWER_BODY_ROWS,
    }
}
```

Add field to `ShellLayout`:

```rust
    pub drawer_bodies: Vec<(DrawerId, Rect)>,
```

In `compute`, replace the drawer-header loop body with one that also records body rects and uses `drawer_body_rows`:

```rust
    let mut drawer_headers = Vec::new();
    let mut drawer_bodies = Vec::new();
    if let Some(rr) = rail {
        let inner = Rect { x: rr.x.saturating_add(1), y: rr.y, width: rr.width.saturating_sub(2), height: rr.height };
        let mut y = inner.y;
        for d in &state.drawers {
            if y >= inner.y.saturating_add(inner.height) {
                break;
            }
            drawer_headers.push((d.id, Rect { x: inner.x, y, width: inner.width, height: 1 }));
            let body_rows = if d.open { drawer_body_rows(d.id) } else { 0 };
            if d.open {
                drawer_bodies.push((
                    d.id,
                    Rect { x: inner.x.saturating_add(1), y: y.saturating_add(1), width: inner.width.saturating_sub(1), height: body_rows },
                ));
            }
            y = y.saturating_add(1 + body_rows + 1);
        }
    }
```

Add `drawer_bodies` to the `ShellLayout { … }` constructor at the end of `compute`.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib layout`
Expected: PASS. (The existing `narrow_hides_rail` asserts `drawer_headers.is_empty()`; `drawer_bodies` is also empty there — add `assert!(l.drawer_bodies.is_empty());` to that test for symmetry.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/layout.rs
git commit -m "feat(layout): data-driven drawer-body rects; economy drawer taller (resolves P2 debt)"
```

---

### Task 9: Render the economy drawer + ledger gauge

**Files:**
- Modify: `crates/zoid-tui/src/render.rs`
- Modify: `crates/zoid-tui/examples/preview.rs` (add `economy` scene; thread `EconomyView`)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (thread `EconomyView`; add economy snapshots @100 & @140)
- Test: snapshots.

**Interfaces:**
- `render_shell` gains a parameter: `economy: &EconomyView` (placed after `state`):
  `pub fn render_shell(frame, state: &ShellState, economy: &EconomyView, msgs, input, streaming)`.
- The economy drawer body renders from `economy`; other drawers keep `drawer_body(id, state)`.

**Layout of the economy body (mirrors `docs/ux/chat-mode.html`):**
```
● users.rs      4k   ██
  ctx.rs        3k   █░
  schema.sql    5k   ░░ cold
churn ▁▂▁▃▁
[x] evict cold              142k/200k
```
- Row marker: `glyph::PIN` when `pinned`, else space. Selected row (when rail focused) gets `color::SEL_BG`.
- Heat bar via `heat_bar`, colored via `heat_color`; `cold` rows append ` cold` in `color::HEAT_COLD`.
- Churn line: `"churn "` + `economy.churn`.
- Footer: `"[x] evict cold"` / `"[ ] evict cold"` per `economy.auto_evict_cold`, plus right-aligned `economy.ledger` in `color::WARN` when `over_ceiling`, else `color::DIM`.

- [ ] **Step 1: Update callers so the workspace compiles, then write/extend snapshot tests**

In `crates/zoid-tui/tests/shell_snapshot.rs`, change `draw` to build and pass an `EconomyView`, and add economy scenes. Add at top:

```rust
use zoid_tui::EconomyView;
use zoid_core::context::ContextWindow;
use zoid_core::economy::{ChurnTimeline, TokenLedger};
use zoid_core::assembler::ContextPolicy;
```

Replace `draw` and add a seeded economy:

```rust
fn empty_economy() -> EconomyView {
    EconomyView::build(&ContextWindow::default(), &ChurnTimeline::default(), &TokenLedger::default(), &ContextPolicy::default(), 0)
}

fn draw(state: &ShellState, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    draw_econ(state, &empty_economy(), msgs, w, h)
}

fn draw_econ(state: &ShellState, econ: &EconomyView, msgs: &[ChatMsg], w: u16, h: u16) -> String {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_shell(f, state, econ, msgs, &input, false)).unwrap();
    terminal.backend().to_string()
}

fn seeded_economy() -> EconomyView {
    use zoid_core::context::{ContextItem, Heat, ItemKind};
    let it = |label: &str, tokens, heat, pinned| ContextItem {
        key: format!("file:{label}"), label: label.into(), kind: ItemKind::File, tokens, heat, pinned, evicted: false,
    };
    let w = ContextWindow {
        items: vec![
            it("schema.sql", 5000, Heat::Cold, false),
            it("users.rs", 4000, Heat::Hot, true),
            it("ctx.rs", 3000, Heat::Warm, false),
        ],
        total_tokens: 12000,
    };
    let churn = ChurnTimeline { points: vec![
        zoid_core::economy::ChurnPoint { turn: 0, tokens: 10, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 1, tokens: 30, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 2, tokens: 12, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 3, tokens: 48, resent_tokens: 0 },
        zoid_core::economy::ChurnPoint { turn: 4, tokens: 12, resent_tokens: 0 },
    ] };
    let ledger = TokenLedger { input: 142_000, output: 0, cached: 0, total: 142_000 };
    let policy = ContextPolicy { token_ceiling: Some(200_000), ..Default::default() };
    EconomyView::build(&w, &churn, &ledger, &policy, 0)
}

#[test]
fn economy_drawer_frame() {
    let s = ShellState::new(); // economy open by default
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 100, 24));
}

#[test]
fn economy_drawer_wide_frame() {
    let s = ShellState::new();
    insta::assert_snapshot!(draw_econ(&s, &seeded_economy(), &seeded(), 140, 24));
}
```

- [ ] **Step 2: Update `preview.rs`**

Add `EconomyView` plumbing and an `economy` scene. Change `scene` to also return an `EconomyView` and update the render call:

```rust
use zoid_tui::EconomyView;
// ... in main, build economy per scene:
let economy = if name == "economy" { /* seeded */ seeded_economy() } else { EconomyView::build(&Default::default(), &Default::default(), &Default::default(), &Default::default(), 0) };
terminal.draw(|f| render_shell(f, &state, &economy, &msgs, &input, false)).unwrap();
```

(Copy a small `seeded_economy()` into `preview.rs` mirroring the test's, or factor a shared helper — a local copy is acceptable for an example.)

- [ ] **Step 3: Run snapshots to confirm they fail (compile, then pending)**

Run: `cargo build -p zoid-tui` then `cargo test -p zoid-tui --test shell_snapshot`
Expected: existing snapshots still need the new `render_shell` arg to compile; once compiling, the two new tests are *pending* (no `.snap` yet) → FAIL.

- [ ] **Step 4: Implement the render change**

Change the signature and the economy-drawer rendering in `render.rs`. Update `render_shell`:

```rust
pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    msgs: &[ChatMsg],
    input: &TextArea<'_>,
    streaming: bool,
) {
```

Pass `economy` into `render_rail`, and in `render_rail` render the economy body from the layout body rect:

```rust
        if d.open {
            let body = layout.drawer_bodies.iter().find(|(id, _)| id == &d.id).map(|(_, r)| *r);
            if let Some(rect) = body {
                if d.id == DrawerId::Economy {
                    render_economy_body(frame, economy, rect, state.focus == Focus::Rail);
                } else {
                    frame.render_widget(Paragraph::new(drawer_body(d.id, state)), rect);
                }
            }
        }
```

(Replace the old inline `body_rect` computation. Import `Focus`, `EconomyView`, and `crate::economy_view::{heat_bar, heat_color}`.)

Add the renderer:

```rust
fn render_economy_body(frame: &mut Frame, econ: &EconomyView, area: Rect, rail_focused: bool) {
    use crate::economy_view::{heat_bar, heat_color};
    let mut lines: Vec<Line> = Vec::new();
    let max_rows = area.height.saturating_sub(2) as usize; // leave room for churn + footer
    for (i, r) in econ.rows.iter().take(max_rows).enumerate() {
        let marker = if r.pinned { glyph::PIN } else { ' ' };
        let sel = rail_focused && i == econ.selected;
        let base = if sel { Style::new().bg(color::SEL_BG) } else { Style::new() };
        let mut spans = vec![
            Span::styled(format!("{marker} {:<10} {:>4} ", r.label, r.tokens), base.fg(color::TXT)),
            Span::styled(heat_bar(r.heat), base.fg(heat_color(r.heat))),
        ];
        if r.cold {
            spans.push(Span::styled(" cold", base.fg(color::HEAT_COLD)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(vec![
        Span::styled("churn ", Style::new().fg(color::DIM)),
        Span::styled(econ.churn.clone(), Style::new().fg(color::CHAT_ACCENT)),
    ]));
    let check = if econ.auto_evict_cold { "[x]" } else { "[ ]" };
    let ledger_color = if econ.over_ceiling { color::WARN } else { color::DIM };
    lines.push(Line::from(vec![
        Span::styled(format!("{check} evict cold  "), Style::new().fg(color::DIM)),
        Span::styled(econ.ledger.clone(), Style::new().fg(ledger_color)),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}
```

- [ ] **Step 5: Accept snapshots and verify fidelity**

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Then **read** `crates/zoid-tui/tests/snapshots/shell_snapshot__economy_drawer_frame.snap` and `…__economy_drawer_wide_frame.snap` and confirm against `docs/ux/chat-mode.html` (pinned ●, heat bars, churn sparkline, `[x] evict cold`, ledger). Re-run without the env var to confirm green:
Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS. Also confirm `cargo run -p zoid-tui --example preview -- economy 100 24` looks right.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/examples/preview.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): render ⑤ economy drawer (items/heat/churn/ledger) + economy snapshots @100/@140"
```

---

### Task 10: Manual control — selection, pin/evict keys, palette, commands

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (selection + policy)
- Modify: `crates/zoid-tui/src/command.rs` (new commands)
- Modify: `crates/zoid-tui/src/route.rs` (actions + routing)
- Modify: `crates/zoid-tui/src/palette.rs` (enable economy rows)
- Modify: `crates/zoid/src/main.rs` (record `ContextMutation` events; selection lookup)
- Test: inline unit tests + a palette snapshot.

**Interfaces:**
- `ShellState` gains: `pub economy_selected: usize`, `pub policy: zoid_core::assembler::ContextPolicy` (default), and mutators `economy_move(delta: i32, len: usize)`, `toggle_auto_evict()`.
- `command::Command` gains: `EvictCold`, `SetCeiling(Option<u64>)`. `parse_command` handles `:evict-cold`, `:set ceiling N` / `:set ceiling off`.
- `route::Action` gains: `EconomyMove(i32)`, `PinSelected`, `EvictSelected`, `ToggleAutoEvict`. Routed only when `focus == Focus::Rail` and the economy drawer is open (else fall through to existing behavior). Keys: `↑/k` `↓/j` move; `p` pin; `x` evict; `e` toggle auto-evict-cold.
- `palette::all_items`: the existing `CONTEXT` group rows ("Pin … to context", "Evict cold items") get real `command`s: pin → `None` (selection-driven; keep as a hint row) — instead make **"Evict cold items" → `Some(Command::EvictCold)`** and add **"Toggle auto-evict cold" → `Some(Command::EvictCold)`**? No — keep one: "Evict cold items" → `Command::EvictCold`. Leave per-item pin as a keybind (`p`) since it needs a selection.

**Bin behavior:** `EconomyMove` adjusts `shell.economy_selected` clamped to the current window length (bin computes the window each frame — see T11; for routing, the bin passes the row count). `PinSelected`/`EvictSelected` look up `window.items[selected].key` and `record(EventKind::ContextMutation { item: key, op: Pin|Evict })`. `EvictCold` records an `Evict` mutation for every cold, unpinned item key. `ToggleAutoEvict` flips `shell.policy.auto_evict_cold`. `SetCeiling(c)` sets `shell.policy.token_ceiling`.

- [ ] **Step 1: Write failing unit tests**

`state.rs`:

```rust
#[test]
fn economy_move_clamps() {
    let mut s = ShellState::new();
    s.economy_move(1, 3);
    assert_eq!(s.economy_selected, 1);
    s.economy_move(-5, 3);
    assert_eq!(s.economy_selected, 0);
    s.economy_move(10, 3);
    assert_eq!(s.economy_selected, 2); // len-1
    s.economy_move(1, 0);
    assert_eq!(s.economy_selected, 0); // empty list stays 0
}

#[test]
fn toggle_auto_evict_flips_policy() {
    let mut s = ShellState::new();
    assert!(s.policy.auto_evict_cold); // default on (P3 decision)
    s.toggle_auto_evict();
    assert!(!s.policy.auto_evict_cold);
}
```

`command.rs`:

```rust
#[test]
fn parses_economy_commands() {
    assert_eq!(parse_command(":evict-cold"), Command::EvictCold);
    assert_eq!(parse_command(":set ceiling 200000"), Command::SetCeiling(Some(200_000)));
    assert_eq!(parse_command(":set ceiling off"), Command::SetCeiling(None));
}
```

`route.rs` (rail-focused economy routing):

```rust
#[test]
fn economy_keys_route_when_rail_focused_and_economy_open() {
    let mut s = ShellState::new();
    s.focus = Focus::Rail; // economy open by default
    assert_eq!(route_key(&s, key(KeyCode::Char('p'), KeyModifiers::NONE)), Action::PinSelected);
    assert_eq!(route_key(&s, key(KeyCode::Char('x'), KeyModifiers::NONE)), Action::EvictSelected);
    assert_eq!(route_key(&s, key(KeyCode::Char('j'), KeyModifiers::NONE)), Action::EconomyMove(1));
    assert_eq!(route_key(&s, key(KeyCode::Char('e'), KeyModifiers::NONE)), Action::ToggleAutoEvict);
}
```

(Use the test helpers already present in `route.rs`'s test module — `key(code, mods)`. If absent, add a small one mirroring existing tests.)

`palette.rs`:

```rust
#[test]
fn evict_cold_palette_row_is_runnable() {
    let items = all_items(Mode::Chat);
    let evict = items.iter().find(|i| i.label.contains("Evict cold")).unwrap();
    assert_eq!(evict.command, Some(Command::EvictCold));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib`
Expected: FAIL — new fields/variants/actions undefined.

- [ ] **Step 3: Implement**

`state.rs`: add fields to `ShellState` (`economy_selected: usize`, `policy: ContextPolicy`), initialize in `new()` (`economy_selected: 0, policy: ContextPolicy::default()`), `use zoid_core::assembler::ContextPolicy;`, and:

```rust
    pub fn economy_move(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.economy_selected = 0;
            return;
        }
        let max = (len - 1) as i32;
        let next = (self.economy_selected as i32 + delta).clamp(0, max);
        self.economy_selected = next as usize;
    }

    pub fn toggle_auto_evict(&mut self) {
        self.policy.auto_evict_cold = !self.policy.auto_evict_cold;
    }
```

> Adding `policy: ContextPolicy` requires `ContextPolicy: PartialEq + Eq` (it derives them in T6) so `ShellState`'s derives still hold. Confirm.

`command.rs`: add variants to `Command` and parsing:

```rust
    EvictCold,
    SetCeiling(Option<u64>),
```

In `parse_command`, after stripping the prefix, match the normalized string:

```rust
        "evict-cold" | "evict cold" => Command::EvictCold,
        rest if rest.starts_with("set ceiling") => {
            let arg = rest.trim_start_matches("set ceiling").trim();
            if arg == "off" || arg.is_empty() {
                Command::SetCeiling(None)
            } else {
                arg.parse::<u64>().map(|n| Command::SetCeiling(Some(n))).unwrap_or(Command::Unknown(rest.to_string()))
            }
        }
```

`route.rs`: add `Action` variants (`EconomyMove(i32)`, `PinSelected`, `EvictSelected`, `ToggleAutoEvict`). In `route_key`, in the focus-contextual section, add a branch for `Focus::Rail` when the economy drawer is open (check `state.drawer(DrawerId::Economy).is_some_and(|d| d.open)`), mapping `Up`/`k`→`EconomyMove(-1)`, `Down`/`j`→`EconomyMove(1)`, `p`→`PinSelected`, `x`→`EvictSelected`, `e`→`ToggleAutoEvict`. Keep the global precedence (overlay, `^C`, `^P`, `⇧Tab`, `Tab`) ahead of it.

`palette.rs`: in `all_items`, change the CONTEXT-group "Evict cold items" row's `command` from `None` to `Some(Command::EvictCold)` (keep the others as hint rows).

`crates/zoid/src/main.rs`: in `handle_action`, add arms. Note the bin must know the current window to resolve selection; compute it from `app.events` (cheap):

```rust
        Action::EconomyMove(d) => {
            let len = zoid_core::context::context_window(&app.events).items.len();
            app.shell.economy_move(d, len);
        }
        Action::PinSelected | Action::EvictSelected => {
            let w = zoid_core::context::context_window(&app.events);
            if let Some(it) = w.items.get(app.shell.economy_selected) {
                let op = if matches!(action, Action::PinSelected) { MutationOp::Pin } else { MutationOp::Evict };
                app.record(EventKind::ContextMutation { item: it.key.clone(), op }).await?;
            }
        }
        Action::ToggleAutoEvict => app.shell.toggle_auto_evict(),
```

(Import `zoid_core::event::MutationOp`. `action` is moved into the match — bind the op before matching, or duplicate arms; simplest is two separate arms.) In `exec_command`, add:

```rust
        Command::EvictCold => {
            let w = zoid_core::context::context_window(&app.events);
            for it in w.items.iter().filter(|i| i.heat == zoid_core::context::Heat::Cold && !i.pinned) {
                app.record(EventKind::ContextMutation { item: it.key.clone(), op: MutationOp::Evict }).await?;
            }
            Ok(false)
        }
        Command::SetCeiling(c) => { app.shell.policy.token_ceiling = c; Ok(false) }
```

- [ ] **Step 4: Run unit tests to confirm pass**

Run: `cargo test -p zoid-tui --lib && cargo build -p zoid`
Expected: PASS / clean build.

- [ ] **Step 5: Add a palette snapshot showing the enabled row**

In `shell_snapshot.rs`, the existing `palette_overlay_frame` already renders the palette; update its `.snap` (the "Evict cold" row is no longer dimmed). Re-accept:
Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot palette_overlay_frame` then read the `.snap` to confirm the row now renders in `TXT` (enabled), and re-run without the env var.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/command.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/palette.rs crates/zoid/src/main.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): manual context control — economy selection, pin/evict keys, :evict-cold/:set ceiling, palette"
```

---

### Task 11: Agent loop records `Usage`; bin builds `EconomyView`; integration

**Files:**
- Modify: `crates/zoid/src/agent.rs` (accumulate `Usage` → append `EventKind::Usage` event)
- Modify: `crates/zoid/src/main.rs` (build `EconomyView` each frame; pass to `render_shell`)
- Test: `crates/zoid/tests/economy_integration.rs` (new) + existing agent-loop test stays green.

**Interfaces:**
- Consumes: `ProviderEvent::Usage`, `zoid_core::{context, economy, assembler}`, `EconomyView`.
- Produces: real `EventKind::Usage` events in the log; a live ⑤ drawer.

- [ ] **Step 1: Write the failing integration test**

`crates/zoid/tests/economy_integration.rs`:

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::economy::token_ledger;
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent, Usage};
use ulid::Ulid;

fn now() -> i64 { 0 }

#[tokio::test]
async fn turn_usage_lands_in_ledger() {
    // Fake provider: a little text, then a usage report, then done.
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("hello".into()),
        ProviderEvent::Usage(Usage { input_tokens: 120, output_tokens: 18 }),
        ProviderEvent::Done,
    ]));
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let session = SessionHandle::spawn(tmp.path().to_str().unwrap()).unwrap();
    let seed = vec![Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() })];
    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    run_agent_turn(provider, Arc::new(zoid_tools::registry()), session.clone(), seed, "m".into(), tx, now).await.unwrap();
    while rx.recv().await.is_some() {}

    let events = session.snapshot().await.unwrap();
    let ledger = token_ledger(&events);
    assert_eq!(ledger.input, 120);
    assert_eq!(ledger.output, 18);
    assert_eq!(ledger.total, 138);
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::Usage)));
}
```

(Confirm `tempfile` is a dev-dep of `zoid`; the existing agent-loop test under `crates/zoid/tests/` already creates a session — reuse its pattern for the DB path if `tempfile` is absent.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid --test economy_integration`
Expected: FAIL — ledger is zero (Usage currently dropped).

- [ ] **Step 3: Implement the agent-loop change**

In `crates/zoid/src/agent.rs`, accumulate usage across the sub-turn and append one `Usage` event when the stream ends. Add a `TokenStat` accumulator before the `while prx.recv()` loop, fold `ProviderEvent::Usage` into it (replacing the discard), and after the loop (before tool execution / `break`), emit it:

```rust
        let mut turn_usage = zoid_core::event::TokenStat::default();
        let mut pending: Vec<ToolCall> = Vec::new();
        while let Some(pe) = prx.recv().await {
            match pe {
                // ...
                ProviderEvent::Usage(u) => {
                    turn_usage.input += u.input_tokens;
                    turn_usage.output += u.output_tokens;
                }
                // ...
            }
        }
        let _ = stream_task.await;

        // Record the sub-turn's token usage so the economy ledger is live.
        if turn_usage != zoid_core::event::TokenStat::default() {
            emit_with_tokens(&session, &mut events, ui, EventKind::Usage, Some(turn_usage), now).await?;
        }
```

Add an `emit_with_tokens` helper (generalize `emit`):

```rust
async fn emit_with_tokens(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    kind: EventKind,
    tokens: Option<zoid_core::event::TokenStat>,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind);
    ev.tokens = tokens;
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(ev)).await;
    Ok(())
}
```

(Have the existing `emit` delegate: `emit(...) = emit_with_tokens(..., None, ...)`.) Keep the `Error` arm behavior; the `Usage` arm no longer discards.

- [ ] **Step 4: Wire the bin to render the economy**

In `crates/zoid/src/main.rs`, build the view each frame and pass it:

```rust
        terminal.draw(|f| {
            let msgs = conversation(&app.events);
            let window = zoid_core::context::context_window(&app.events);
            let churn = zoid_core::economy::churn_timeline(&app.events);
            let ledger = zoid_core::economy::token_ledger(&app.events);
            let economy = zoid_tui::EconomyView::build(&window, &churn, &ledger, &app.shell.policy, app.shell.economy_selected);
            render_shell(f, &app.shell, &economy, &msgs, &app.textarea, app.streaming);
        })?;
```

- [ ] **Step 5: Run the suite**

Run: `cargo test -p zoid && cargo test --workspace`
Expected: PASS (integration test green; existing agent-loop tests unaffected — the extra `Usage` event is ignored by `conversation`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/tests/economy_integration.rs
git commit -m "feat(zoid): record turn Usage events; live ⑤ economy drawer from projections"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `cargo run -p zoid-tui --example preview -- economy 100 24` and `-- economy 140 24` match `docs/ux/chat-mode.html`.
- [ ] No literal special glyphs/hex outside `tokens.rs`; `docs/ux/README.md` table updated.
- [ ] Assembler is **not** referenced by `build_request` (grep to confirm — P3 is visualize-only).
- [ ] Snapshots exist at both 100 and 140 for every economy-bearing screen.

## Self-Review notes (author)

- **Spec coverage:** TokenLedger (T3), ContextWindow+heat (T5), ChurnTimeline (T4), pin/evict (T2 events + T5 fold + T10 actions), auto-evict-cold + compact-at-threshold (T6 policy, default-on per decision), optional token ceiling (T6 + T10 `:set ceiling`), economy rail drawer with Ⓡ4 dataviz (T7 view-model + T9 render), constructed-context assembler primitive (T6). Live request wiring intentionally deferred to P5 (user decision) — noted in Global Constraints and T6.
- **Type consistency:** `EconomyView::build(window, churn, ledger, policy, selected)` signature is identical in T7 (def), T9 (snapshot tests + preview), and T11 (bin). `context_window`/`token_ledger`/`churn_timeline` take `&[Event]`. `assemble_context(&ContextWindow, &ContextPolicy)`. `ContextPolicy` derives `Copy + Eq` so `ShellState` derives hold.
- **Heat heuristic** is explicit and conservative (named consts; pin always overrides). Documented as a heuristic per spec §15 risk 9.
- **P2 debt folded in:** T8 moves drawer bodies into `layout::compute` with `DRAWER_BODY_ROWS`.
