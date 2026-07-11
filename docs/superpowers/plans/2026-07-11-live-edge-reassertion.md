# Live-Edge System-Prompt Re-Assertion ("Re-Floor") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every N estimated-appended tokens of context growth, re-inject the full system prompt at the live edge of the provider request to counter instruction-drift in long sessions.

**Architecture:** Central policy in the agent loop decides *whether/what* to re-assert (monotonic, compaction-aware token-distance trigger + a persisted `DirectiveReasserted` marker) and sets a new provider-neutral `CompletionRequest.reassert` field; each provider adapter renders it at the tail in the placement that model family honors. The reminder text is ephemeral (request-only); only a weightless marker persists.

**Tech Stack:** Rust workspace (`zoid-core`, `zoid-provider`, `zoid` bin, `zoid-tui`), `cargo test`, event-sourced session log.

**Design spec:** `docs/superpowers/specs/2026-07-11-live-edge-reassertion-design.md` (read it first).

## Global Constraints

- No co-author trailer in commit messages (repo policy).
- `estimate_tokens` is `chars/3` (`crates/zoid-core/src/economy.rs:41`). All token math uses it.
- The reminder text is **ephemeral** — never persisted as an event. Only `DirectiveReasserted { at_cumulative: u64 }` persists.
- Subagents and tests pass `reassert_interval = 0` (feature off), consistent with `eviction: disabled()`.
- `reassert = None` MUST produce a request body byte-identical to today (explicit early-return in each adapter).
- Run `cargo build` and `cargo clippy --workspace` clean before each commit.

---

### Task 1: `DirectiveReasserted` event kind (inert everywhere)

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (EventKind enum, near `Tasks`/`TurnsEvicted`)
- Modify: `crates/zoid-core/src/eviction.rs:220-231` (`is_inert`)
- Test: `crates/zoid-core/src/eviction.rs` (inline `#[cfg(test)]`), `crates/zoid-core/src/context.rs` (inline test)

**Interfaces:**
- Produces: `EventKind::DirectiveReasserted { at_cumulative: u64 }` — a weightless marker recording the cumulative-appended value at a re-floor fire.

- [ ] **Step 1: Write the failing test** (append to `eviction.rs` tests module)

```rust
#[test]
fn directive_reasserted_is_inert() {
    let k = EventKind::DirectiveReasserted { at_cumulative: 123 };
    assert!(is_inert(&k), "re-floor marker must not join evictable turn groups");
    assert_eq!(event_tokens(&k), 0, "marker is weightless");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core directive_reasserted_is_inert`
Expected: FAIL — `no variant named DirectiveReasserted`.

- [ ] **Step 3: Add the variant** (in `event.rs`, alongside the other bookkeeping kinds like `Tasks`/`Usage`)

```rust
    /// Live-edge re-assertion marker (spec: re-floor). Records the
    /// cumulative-appended token value at the moment a re-floor fired, so the
    /// interval spans the whole session. Weightless: inert for eviction and
    /// context_window; never rendered as conversation.
    DirectiveReasserted {
        at_cumulative: u64,
    },
```

- [ ] **Step 4: Add it to `is_inert`** (`eviction.rs:221`)

```rust
    matches!(
        kind,
        EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. }
            | EventKind::Tasks { .. }
            | EventKind::TurnsDropped { .. }
            | EventKind::TurnsEvicted { .. }
            | EventKind::TurnsReadmitted { .. }
            | EventKind::DirectiveReasserted { .. }
    )
```

- [ ] **Step 5: Add a context_window weightless test** (append to `context.rs` tests module)

```rust
#[test]
fn context_window_ignores_directive_reasserted() {
    let base = vec![u("hello world this is content")];
    let mut with_marker = base.clone();
    with_marker.push(ev(EventKind::DirectiveReasserted { at_cumulative: 999 }));
    assert_eq!(
        context_window(&base).total_tokens,
        context_window(&with_marker).total_tokens,
        "re-floor marker must not change the context window total"
    );
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core directive_reasserted_is_inert context_window_ignores_directive_reasserted`
Expected: PASS (the `_ => {}` arm in `context_window` already ignores unknown kinds; no change needed there).

- [ ] **Step 7: Confirm projection ignores it** — verify `conversation()` in `crates/zoid-core/src/projection.rs` has a catch-all `_ => {}` in its event match (it does). No code change; note it in the commit body.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/eviction.rs crates/zoid-core/src/context.rs
git commit -m "feat(core): add weightless DirectiveReasserted event kind"
```

---

### Task 2: `cumulative_appended` + `reassertion_due` (monotonic, compaction-aware)

**Files:**
- Create: `crates/zoid-core/src/reassert.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod reassert;`)
- Test: inline `#[cfg(test)]` in `reassert.rs`

**Interfaces:**
- Consumes: `EventKind::{UserMessage,AssistantMessage,ModelDelta,ToolResult,ToolResultCompacted,DirectiveReasserted}`, `economy::estimate_tokens`.
- Produces:
  - `pub fn cumulative_appended<'a>(events: impl IntoIterator<Item = &'a Event> + Clone) -> u64`
  - `pub fn reassertion_due<'a>(events: impl IntoIterator<Item = &'a Event> + Clone, interval: u64) -> bool`

- [ ] **Step 1: Write the failing tests** (`reassert.rs`)

```rust
//! Live-edge re-assertion policy (spec: re-floor). Pure functions over the
//! event log; monotonic under BOTH eviction (marks-but-keeps) and compaction
//! (#6b empties tool bodies, but ToolResultCompacted preserves original_tokens).

use crate::economy::estimate_tokens;
use crate::event::{Event, EventKind};
use std::collections::HashMap;

pub fn cumulative_appended<'a>(events: impl IntoIterator<Item = &'a Event> + Clone) -> u64 {
    let orig: HashMap<&str, u64> = events
        .clone()
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, original_tokens, .. } => {
                Some((id.as_str(), *original_tokens))
            }
            _ => None,
        })
        .collect();
    events
        .into_iter()
        .map(|e| match &e.kind {
            EventKind::UserMessage { text }
            | EventKind::AssistantMessage { text }
            | EventKind::ModelDelta { text } => estimate_tokens(text),
            EventKind::ToolResult { id, output, .. } => orig
                .get(id.as_str())
                .copied()
                .unwrap_or_else(|| estimate_tokens(output)),
            _ => 0,
        })
        .sum()
}

pub fn reassertion_due<'a>(events: impl IntoIterator<Item = &'a Event> + Clone, interval: u64) -> bool {
    if interval == 0 {
        return false;
    }
    let last = events
        .clone()
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::DirectiveReasserted { at_cumulative } => Some(*at_cumulative),
            _ => None,
        })
        .last()
        .unwrap_or(0);
    cumulative_appended(events).saturating_sub(last) >= interval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, EvictionMarker};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }
    fn user(t: &str) -> Event { ev(EventKind::UserMessage { text: t.into() }) }
    fn tool(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult { id: id.into(), name: "shell".into(), output: out.into(), is_error: false })
    }
    fn compacted(id: &str, original_tokens: u64) -> Event {
        ev(EventKind::ToolResultCompacted { id: id.into(), summary: "sum".into(), original_tokens })
    }

    #[test]
    fn disabled_interval_never_due() {
        assert!(!reassertion_due(&vec![user(&"x".repeat(9000))], 0));
    }

    #[test]
    fn fires_at_threshold_and_marker_resets_baseline() {
        // ~1000 estimated tokens (3000 chars / 3).
        let big = user(&"x".repeat(3000));
        let log = vec![big.clone()];
        assert!(reassertion_due(&log, 1000));
        assert!(!reassertion_due(&log, 1001));
        // Fire recorded at 1000 → needs another 1000 before next fire.
        let mut log2 = log.clone();
        log2.push(ev(EventKind::DirectiveReasserted { at_cumulative: 1000 }));
        assert!(!reassertion_due(&log2, 1000));
        log2.push(user(&"y".repeat(3000)));
        assert!(reassertion_due(&log2, 1000));
    }

    #[test]
    fn monotonic_under_compaction_body_clear() {
        // A tool result contributing ~1000 est tokens, later compacted+cleared.
        let before = vec![tool("t1", &"z".repeat(3000))];
        let full = cumulative_appended(&before);
        assert_eq!(full, 1000);
        // Simulate #6b: body emptied, ToolResultCompacted preserves original_tokens.
        let after = vec![
            tool("t1", ""), // cleared body
            compacted("t1", 1000),
        ];
        assert_eq!(
            cumulative_appended(&after),
            full,
            "compacted+cleared result must still count at original_tokens (monotonic)"
        );
    }

    #[test]
    fn monotonic_under_eviction_marker() {
        let mut log = vec![user(&"a".repeat(3000)), tool("t1", &"b".repeat(3000))];
        let before = cumulative_appended(&log);
        log.push(ev(EventKind::TurnsEvicted {
            ids: vec![log[0].id],
            reclaimed_tokens: 1000,
            marker: EvictionMarker { spans: vec![] },
        }));
        assert_eq!(cumulative_appended(&log), before, "evicted events still counted");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core reassert`
Expected: FAIL — `module reassert not found` until `lib.rs` is wired.

- [ ] **Step 3: Wire the module** — add to `crates/zoid-core/src/lib.rs`:

```rust
pub mod reassert;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core reassert`
Expected: PASS (all four).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/reassert.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): monotonic compaction-aware re-floor trigger (cumulative_appended, reassertion_due)"
```

---

### Task 3: `CompletionRequest.reassert` field (all constructors updated)

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs:214-221` (struct)
- Modify: every `CompletionRequest { .. }` literal: `crates/zoid-provider/src/{openai_compat.rs,ollama.rs,zai.rs}` tests, `crates/zoid-provider/src/anthropic/request.rs` tests, `crates/zoid/src/agent.rs` (`build_request_with_thinking`), and any other call sites surfaced by the compiler.

**Interfaces:**
- Produces: `CompletionRequest.reassert: Option<String>` — fully-wrapped reminder text, or `None`.

- [ ] **Step 1: Add the field** (`lib.rs:220`, after `thinking`)

```rust
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
    pub thinking: ThinkingMode,
    /// Live-edge re-assertion text (spec: re-floor). `None` = no reminder this
    /// request (body byte-identical to pre-feature). `Some` = adapters render it
    /// at the tail (per-adapter placement).
    pub reassert: Option<String>,
}
```

- [ ] **Step 2: Run the build to enumerate broken constructors**

Run: `cargo build -p zoid-provider 2>&1 | rg "missing field .reassert"`
Expected: a list of every struct-literal site. Add `reassert: None,` to each.

- [ ] **Step 3: Update `build_request_with_thinking`** in `agent.rs` to set `reassert: None` for now (threaded properly in Task 8). Locate the `CompletionRequest { .. }` at ~agent.rs:348 and add `reassert: None,`.

- [ ] **Step 4: Run the full build**

Run: `cargo build --workspace 2>&1 | rg "missing field .reassert"`
Expected: no output (all sites fixed).

- [ ] **Step 5: Verify no behavior change**

Run: `cargo test -p zoid-provider`
Expected: PASS (field is unused so far; bodies unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src crates/zoid/src/agent.rs
git commit -m "feat(provider): add neutral CompletionRequest.reassert field (unused)"
```

---

### Task 4: Anthropic rendering — append reminder to last user message

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/request.rs` (body builder)
- Test: inline test in `anthropic/request.rs`

**Interfaces:**
- Consumes: `CompletionRequest.reassert`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reassert_appends_text_block_to_last_user_message() {
    let mut req = sample_request(); // existing test helper; tail is a user message
    req.reassert = Some("STANDING REMINDER".to_string());
    let body = build_body(&req);
    let msgs = body["messages"].as_array().unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "user", "tail stays user-role (alternation preserved)");
    // The reminder rides inside the last user message's content.
    let content = serde_json::to_string(&last["content"]).unwrap();
    assert!(content.contains("STANDING REMINDER"));
    // No new trailing message was added.
    assert_eq!(msgs.len(), sample_request_message_count());
}

#[test]
fn reassert_none_is_byte_identical() {
    let req = sample_request();
    let mut req2 = req.clone();
    req2.reassert = None;
    assert_eq!(build_body(&req), build_body(&req2));
}
```

(If `sample_request`/`build_body` helpers don't exist under those names, adapt to the module's existing test scaffolding — mirror the neighboring `#[test] fn` that asserts body shape.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider reassert_appends_text_block`
Expected: FAIL — reminder not present in body.

- [ ] **Step 3: Implement** — in the Anthropic body builder, after the messages array is assembled and before serialization, if `req.reassert` is `Some(text)`, append a text block to the **last** message's content array:

```rust
if let Some(text) = &req.reassert {
    if let Some(last) = messages.last_mut() {
        // Anthropic content is an array of blocks; push a text block.
        last.content.push(ContentBlock::text(format!("\n\n{text}")));
    }
}
```

(Use the module's actual content-block type/constructor. The tail at build time is always a `User` message per the spec, so this preserves alternation.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider reassert`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/request.rs
git commit -m "feat(provider): render reassert as trailing text block on last user message (anthropic)"
```

---

### Task 5: openai-compat rendering — trailing system message

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs:66-101` (body builder)
- Test: inline

**Interfaces:**
- Consumes: `CompletionRequest.reassert`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reassert_pushes_trailing_system_message() {
    let mut req = CompletionRequest { /* minimal: model, system:None, one user message, ... */ reassert: Some("STANDING REMINDER".into()), ..sample() };
    let body = build_body(&req);
    let msgs = body["messages"].as_array().unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "system");
    assert_eq!(last["content"], "STANDING REMINDER");
}

#[test]
fn reassert_none_body_unchanged_openai() {
    let mut a = sample(); a.reassert = None;
    let b = a.clone();
    assert_eq!(build_body(&a), build_body(&b));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider reassert_pushes_trailing_system_message`
Expected: FAIL.

- [ ] **Step 3: Implement** — after the loop that maps conversation messages (openai_compat.rs ~101), before finalizing the JSON:

```rust
if let Some(text) = &req.reassert {
    messages.push(json!({ "role": "system", "content": text }));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider reassert`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs
git commit -m "feat(provider): render reassert as trailing system message (openai-compat/zai)"
```

---

### Task 6: Ollama-native rendering — trailing system message

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:16-44` (native body builder)
- Test: inline

**Interfaces:**
- Consumes: `CompletionRequest.reassert`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn native_reassert_pushes_trailing_system_message() {
    let mut req = sample_native();
    req.reassert = Some("STANDING REMINDER".into());
    let body = build_native_body(&req);
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.last().unwrap()["role"], "system");
    assert_eq!(msgs.last().unwrap()["content"], "STANDING REMINDER");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider native_reassert_pushes_trailing_system_message`
Expected: FAIL.

- [ ] **Step 3: Implement** — after the role-mapping loop in the native body builder (ollama.rs ~44):

```rust
if let Some(text) = &req.reassert {
    messages.push(json!({ "role": "system", "content": text }));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider reassert`
Expected: PASS. Then `cargo test -p zoid-provider` (all adapters) — PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): render reassert as trailing system message (ollama-native)"
```

---

### Task 7: Config — `reassert_interval_tokens`

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (`EconomyConfig` struct + `Default`; `PartialConfig`/`parse_toml` economy parsing)
- Modify: `crates/zoid/src/main.rs:5299-5307` (resolve into `TurnConfig`; done in Task 8 — here just resolve the value onto `app.economy`)
- Test: inline in `config.rs`

**Interfaces:**
- Produces: `EconomyConfig.reassert_interval_tokens: u64` (default `100_000`, `0` disables).

- [ ] **Step 1: Write the failing test** (`config.rs` tests)

```rust
#[test]
fn economy_default_reassert_interval_is_100k() {
    assert_eq!(EconomyConfig::default().reassert_interval_tokens, 100_000);
}

#[test]
fn parse_reassert_interval_from_toml() {
    let c = parse_toml("[economy]\nreassert_interval_tokens = 250000").unwrap();
    assert_eq!(c.economy.reassert_interval_tokens, 250_000);
}

#[test]
fn parse_reassert_interval_zero_disables() {
    let c = parse_toml("[economy]\nreassert_interval_tokens = 0").unwrap();
    assert_eq!(c.economy.reassert_interval_tokens, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core reassert_interval`
Expected: FAIL — no such field.

- [ ] **Step 3: Add the field** to `EconomyConfig`:

```rust
    /// Re-assert the system prompt at the live edge every N estimated-appended
    /// tokens of novel content. 0 disables. Default 100_000. Units: estimate_tokens (chars/3).
    pub reassert_interval_tokens: u64,
```

- [ ] **Step 4: Add to `Default for EconomyConfig`**:

```rust
            reassert_interval_tokens: 100_000,
```

- [ ] **Step 5: Wire TOML parsing** — mirror how `compact_threshold_pct` is read in `PartialConfig` merge / `parse_toml` (find the `economy` field reads and add `reassert_interval_tokens` with the same pattern; default applied when absent).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core reassert_interval`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [economy].reassert_interval_tokens (default 100k, 0 disables)"
```

---

### Task 8: `wrap_reassertion` + `TurnConfig.reassert_interval` + thread `reassert` through `build_request`

**Files:**
- Modify: `crates/zoid/src/agent.rs` (add `wrap_reassertion`, `TurnConfig.reassert_interval` field + Debug + `chat_turn_config_with` default 0, `build_request_with_thinking` new param)
- Modify: `crates/zoid/src/main.rs:5299` (set `turn_config.reassert_interval = app.economy.reassert_interval_tokens`)
- Test: inline in `agent.rs`

**Interfaces:**
- Consumes: `config.system`.
- Produces:
  - `pub fn wrap_reassertion(system: &str) -> String`
  - `TurnConfig.reassert_interval: u64` (Chat: from config; subagents/tests: `0`)
  - `build_request_with_thinking(events, model, tools, system, thinking, reassert: Option<String>)`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn wrap_reassertion_frames_prompt_as_standing_reminder() {
    let out = wrap_reassertion("BEHAVIORAL RULES");
    assert!(out.contains("BEHAVIORAL RULES"));
    assert!(out.contains("NOT a signal that anything is complete"));
    assert!(out.contains("resume the task"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid wrap_reassertion_frames`
Expected: FAIL — not defined.

- [ ] **Step 3: Implement `wrap_reassertion`** (near `SYSTEM_PROMPT`):

```rust
/// Wrap the system prompt as a standing, tail-injected reminder. The pre/post
/// framing is the only added text; `system` is verbatim (zero drift). The
/// "NOT a signal that anything is complete" clause guards against a mid-loop
/// re-floor being misread as "the task is done now".
pub fn wrap_reassertion(system: &str) -> String {
    format!(
        "[Standing reminder — your operating instructions below are still in \
         effect. This is a periodic re-statement, NOT a change of task and NOT \
         a signal that anything is complete. Do not alter what you are doing in \
         response to seeing this; continue the current work and keep following \
         these instructions:]\n\n{system}\n\n[End of reminder — resume the task in progress.]"
    )
}
```

- [ ] **Step 4: Add `reassert_interval` to `TurnConfig`** — add field `pub reassert_interval: u64,`, add it to the manual `Debug` impl, and set `reassert_interval: 0` in `chat_turn_config_with` (subagents/tests default off).

- [ ] **Step 5: Add the new `build_request_with_thinking` param** — extend the signature with `reassert: Option<String>` and set `reassert` on the `CompletionRequest` literal. Update the thin `build_request` wrapper and all call sites (the eviction-breadcrumb system-append logic stays as-is). Existing callers pass `None`.

- [ ] **Step 6: Set the interval in the bin** — `crates/zoid/src/main.rs` after line 5299:

```rust
    turn_config.reassert_interval = app.economy.reassert_interval_tokens;
```

- [ ] **Step 7: Run tests to verify they pass + build**

Run: `cargo test -p zoid wrap_reassertion_frames && cargo build --workspace`
Expected: PASS + clean build.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(agent): wrap_reassertion + TurnConfig.reassert_interval + build_request reassert param"
```

---

### Task 9: Loop wiring — fire the re-floor (preflight, marker-after-send, calibration skip)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`run_turn_inner` turn loop; `record_compactions` calibration skip; `AgentUpdate` enum)
- Test: inline integration-style test in `agent.rs` (fake provider)

**Interfaces:**
- Consumes: `reassert::reassertion_due`, `reassert::cumulative_appended`, `wrap_reassertion`, `config.reassert_interval`, `ContextOverhead`.
- Produces: `AgentUpdate::DirectiveReasserted { at_cumulative: u64 }`; a persisted `EventKind::DirectiveReasserted` per fire.

- [ ] **Step 1: Add the UI update variant** to `AgentUpdate`:

```rust
    /// A re-floor fired: the system prompt was re-asserted at the live edge.
    DirectiveReasserted { at_cumulative: u64 },
```

- [ ] **Step 2: Write the failing test** — drive `run_turn_inner` (or the cancellable wrapper) with a fake provider over a seed log whose `cumulative_appended` already exceeds the interval; assert (a) the built request carried `reassert = Some(..)`, and (b) a `DirectiveReasserted` event was persisted after a successful (non-error) stream. Use the existing fake-provider test harness in `agent.rs` tests; capture the request via a provider double that records `req.reassert.is_some()`.

```rust
#[tokio::test]
async fn re_floor_fires_and_persists_marker_on_success() {
    // Seed a log with ~interval+ estimated-appended tokens; interval small.
    // Fake provider returns a final text answer (no tool calls), records the
    // request it saw. Assert the request had reassert=Some and a
    // DirectiveReasserted event is in the resulting log.
    // (Mirror the structure of the existing run_turn_inner tests.)
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid re_floor_fires_and_persists_marker`
Expected: FAIL — no reassert injected, no marker.

- [ ] **Step 4: Implement the trigger in the turn loop** — at the top of `'turn: loop`, after `preflight_gate` but before `build_request_with_thinking` (agent.rs ~528-540):

```rust
    let will_reassert = config.reassert_interval > 0
        && zoid_core::reassert::reassertion_due(events.iter(), config.reassert_interval);
    let reassert_text = will_reassert.then(|| wrap_reassertion(&config.system));

    // S2: size the request honestly — the ephemeral reminder is not in the
    // event log, so add its tokens to overhead for THIS turn's preflight.
    let mut overhead_now = overhead.clone();
    if let Some(t) = &reassert_text {
        overhead_now.system_tokens += zoid_core::economy::estimate_tokens(t);
    }
    // (Pass &overhead_now into preflight_gate instead of `overhead`.)

    let req = build_request_with_thinking(
        &events, &model, &tools, &config.system, config.thinking, reassert_text.clone(),
    );
```

- [ ] **Step 5: Emit the marker after a successful stream** — after the streaming inner loop completes normally and `stream_task.await` returns (agent.rs ~701, before/after the usage-recording block), guarded so it does NOT run on the `continue 'turn` (context-length) or `break 'turn` (error) paths (both leave before this point):

```rust
    if will_reassert {
        let at = zoid_core::reassert::cumulative_appended(events.iter());
        emit(&session, &mut events, ui, &config.branch,
             EventKind::DirectiveReasserted { at_cumulative: at }, session_id, now).await?;
        let _ = ui.send(AgentUpdate::DirectiveReasserted { at_cumulative: at }).await;
    }
```

- [ ] **Step 6: Skip calibration on re-floor sub-turns (S3)** — thread `will_reassert` into the `record_compactions` call (or guard the `calibration_ratio` update) so the ratio is not updated when the request carried an extra ephemeral system copy. Add a `skip_calibration: bool` param to `record_compactions` and pass `will_reassert`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p zoid re_floor_fires_and_persists_marker && cargo test -p zoid`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): fire re-floor in turn loop (preflight-accounted, marker-after-send, calibration-safe)"
```

---

### Task 10: TUI observability — transcript marker

**Files:**
- Modify: `crates/zoid/src/main.rs` (handle `AgentUpdate::DirectiveReasserted`)
- Modify: `crates/zoid-tui/src/render.rs` (render `EventKind::DirectiveReasserted` as a subtle system line)
- Test: inline render test in `render.rs`

**Interfaces:**
- Consumes: `EventKind::DirectiveReasserted`, `AgentUpdate::DirectiveReasserted`.

- [ ] **Step 1: Write the failing test** (`render.rs`)

```rust
#[test]
fn directive_reasserted_renders_subtle_marker() {
    let line = render_event(&EventKind::DirectiveReasserted { at_cumulative: 42 });
    assert!(line.contains("re-asserted") || line.contains("↻"));
}
```

(Adapt to the module's actual render entry point — mirror how another bookkeeping event, e.g. a compaction notice, is rendered.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui directive_reasserted_renders`
Expected: FAIL.

- [ ] **Step 3: Implement the render arm** — in the event-to-line match, add:

```rust
    EventKind::DirectiveReasserted { .. } => dim_system_line("↻ re-asserted operating instructions"),
```

(Use the module's existing dim/system styling helper.)

- [ ] **Step 4: Handle the AgentUpdate in the bin** — in `main.rs`'s `AgentUpdate` match, add an arm for `DirectiveReasserted { .. }` (the persisted event drives the transcript; the update can just trigger a redraw / no-op if the event already renders). Ensure the match stays exhaustive.

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p zoid-tui directive_reasserted_renders && cargo build --workspace`
Expected: PASS + clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(tui): show a subtle transcript marker when a re-floor fires"
```

---

### Task 11: End-to-end integration + steady-state liveness guard

**Files:**
- Test: `crates/zoid/tests/` (new integration test file, or extend an existing turn test)

**Interfaces:**
- Consumes: the full stack (config → loop → provider double).

- [ ] **Step 1: Write the integration test**

```rust
// Long synthetic session: many turns of tool output that exceed context_target
// with eviction + compaction (body-clear) active, interval small.
// Assert: DirectiveReasserted fires repeatedly PAST context_target (no dormancy),
// and each fired request carried the reminder. This is the B1 regression guard —
// it must exercise a log that has been through clear_compacted_bodies, not just
// eviction markers.
#[tokio::test]
async fn re_floor_keeps_firing_in_steady_state_with_compaction() {
    // Build the app/turn stack with reassert_interval small, eviction+compaction on.
    // Run N turns feeding large tool outputs; count DirectiveReasserted events.
    // assert!(fires >= expected_many, "re-floor must not go dormant past context_target");
}
```

- [ ] **Step 2: Run test to verify it fails** (before Task 9 wiring it would fail; here it validates the whole path)

Run: `cargo test -p zoid re_floor_keeps_firing_in_steady_state`
Expected: initially FAIL if any wiring gap remains.

- [ ] **Step 3: Fix any gaps surfaced, then verify pass**

Run: `cargo test -p zoid re_floor_keeps_firing_in_steady_state`
Expected: PASS.

- [ ] **Step 4: Full workspace green**

Run: `cargo test --workspace && cargo clippy --workspace`
Expected: PASS + no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/tests
git commit -m "test(agent): steady-state re-floor liveness under eviction+compaction (B1 guard)"
```

---

## Manual acceptance (post-implementation, not a task)

Unit tests cannot prove drift is reduced or that GLM won't wrap up early. Run a long real zai / glm-5.2 session with the transcript re-floor markers visible; confirm the reminder fires at the expected cadence, watch for early-termination regressions, and tune `[economy].reassert_interval_tokens`.
