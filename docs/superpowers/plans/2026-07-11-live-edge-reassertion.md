# Live-Edge System-Prompt Re-Assertion ("Re-Floor") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every N estimated-appended tokens of context growth, re-inject the full system prompt at the live edge of the provider request to counter instruction-drift in long sessions.

**Architecture:** Central policy in the agent loop decides *whether/what* to re-assert (monotonic, compaction-aware token-distance trigger + a persisted `DirectiveReasserted` marker) and sets a new provider-neutral `CompletionRequest.reassert` field; each provider adapter renders it at the tail in the placement that model family honors. The reminder text is ephemeral (request-only); only a weightless marker persists.

**Tech Stack:** Rust workspace (`zoid-core`, `zoid-provider`, `zoid` bin, `zoid-tui`), `cargo test`, event-sourced session log.

**Design spec:** `docs/superpowers/specs/2026-07-11-live-edge-reassertion-design.md` (read it first).

## Global Constraints

- No co-author trailer in commit messages (repo policy).
- Execution runs on branch `subagent-reliability`, **no worktree**. Before any edit/commit assert `git branch --show-current` == `subagent-reliability`; use absolute paths under `/home/gomanjoe/source/zoid`.
- `estimate_tokens` is `chars/3` (`crates/zoid-core/src/economy.rs:41`). All token math uses it.
- The reminder text is **ephemeral** — never persisted as an event. Only `DirectiveReasserted { at_cumulative: u64 }` persists.
- Subagents and tests pass `reassert_interval = 0` (feature off), consistent with `eviction: disabled()`.
- `reassert = None` MUST produce a request body byte-identical to today (explicit early-return in each adapter).
- Locate edit points **by symbol, not line number** (anchors drift as earlier tasks edit files).
- Run `cargo build --workspace` clean before each commit (every task must compile independently).

---

### Task 1: `DirectiveReasserted` event kind (inert everywhere, incl. the one exhaustive match)

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (EventKind enum, near `Tasks`/`Usage`)
- Modify: `crates/zoid-core/src/eviction.rs` (`is_inert`)
- Modify: `crates/zoid-core/src/projection.rs` (`conversation()` — the ONLY exhaustive `EventKind` match in the workspace; no wildcard, so this MUST get an arm or the crate won't compile)
- Test: inline in `eviction.rs` and `context.rs`

**Interfaces:**
- Produces: `EventKind::DirectiveReasserted { at_cumulative: u64 }` — weightless marker recording the cumulative-appended value at a re-floor fire.

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

- [ ] **Step 3: Add the variant** (`event.rs`, alongside `Tasks`/`Usage`)

```rust
    /// Live-edge re-assertion marker (spec: re-floor). Records the
    /// cumulative-appended token value at the moment a re-floor fired, so the
    /// interval spans the whole session. Weightless: inert for eviction and
    /// context_window; never projected into a ChatMsg.
    DirectiveReasserted {
        at_cumulative: u64,
    },
```

- [ ] **Step 4: Add it to `is_inert`** (`eviction.rs`, extend the `matches!` list)

```rust
            | EventKind::TurnsReadmitted { .. }
            | EventKind::DirectiveReasserted { .. }
```

- [ ] **Step 5: Add the required arm to `conversation()`** (`projection.rs`) — the main fold `match &e.kind` ends with `EventKind::TurnsEvicted { .. } | EventKind::TurnsReadmitted { .. } => {}` and has NO wildcard. Extend that ignore arm:

```rust
            EventKind::TurnsEvicted { .. }
            | EventKind::TurnsReadmitted { .. }
            | EventKind::DirectiveReasserted { .. } => {}
```

- [ ] **Step 6: Add a context_window weightless test** (append to `context.rs` tests module)

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

(`context_window`'s fold has a `_ => {}` arm, so no change is needed there — this test proves it.)

- [ ] **Step 7: Build + run tests**

Run: `cargo build -p zoid-core && cargo test -p zoid-core directive_reasserted_is_inert context_window_ignores_directive_reasserted`
Expected: clean build + PASS. (If `projection.rs` was missed, the build fails with a non-exhaustive `match` error — that is the B1 guard.)

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/eviction.rs crates/zoid-core/src/projection.rs crates/zoid-core/src/context.rs
git commit -m "feat(core): add weightless DirectiveReasserted event kind"
```

---

### Task 2: `cumulative_appended` + `reassertion_due` (monotonic, compaction-aware)

**Files:**
- Create: `crates/zoid-core/src/reassert.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod reassert;`)
- Test: inline in `reassert.rs`

**Interfaces:**
- Consumes: `EventKind::{UserMessage,AssistantMessage,ModelDelta,ToolResult,ToolResultCompacted,DirectiveReasserted}`, `economy::estimate_tokens`.
- Produces:
  - `pub fn cumulative_appended<'a>(events: impl IntoIterator<Item = &'a Event> + Clone) -> u64`
  - `pub fn reassertion_due<'a>(events: impl IntoIterator<Item = &'a Event> + Clone, interval: u64) -> bool`

- [ ] **Step 1: Write the module + failing tests** (`reassert.rs`)

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

    fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
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
        let big = user(&"x".repeat(3000)); // 3000 chars / 3 = 1000 est tokens
        let log = vec![big.clone()];
        assert!(reassertion_due(&log, 1000));
        assert!(!reassertion_due(&log, 1001));
        let mut log2 = log.clone();
        log2.push(ev(EventKind::DirectiveReasserted { at_cumulative: 1000 }));
        assert!(!reassertion_due(&log2, 1000));
        log2.push(user(&"y".repeat(3000)));
        assert!(reassertion_due(&log2, 1000));
    }

    #[test]
    fn monotonic_under_compaction_body_clear() {
        let before = vec![tool("t1", &"z".repeat(3000))];
        assert_eq!(cumulative_appended(&before), 1000);
        // Simulate #6b: body emptied; ToolResultCompacted preserves original_tokens.
        let after = vec![tool("t1", ""), compacted("t1", 1000)];
        assert_eq!(cumulative_appended(&after), 1000,
            "compacted+cleared result must still count at original_tokens (monotonic)");
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

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p zoid-core reassert`
  Expected: FAIL — `module reassert not found`.

- [ ] **Step 3: Wire the module** — add `pub mod reassert;` to `crates/zoid-core/src/lib.rs`.

- [ ] **Step 4: Run to verify pass** — Run: `cargo test -p zoid-core reassert` → PASS (all four).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/reassert.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): monotonic compaction-aware re-floor trigger (cumulative_appended, reassertion_due)"
```

---

### Task 3: `CompletionRequest.reassert` field (all constructors updated)

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs` (`CompletionRequest` struct)
- Modify: every `CompletionRequest { .. }` literal (compiler-driven; includes `openai_compat.rs`, `ollama.rs`, `zai.rs`, `opencode_go.rs`, `anthropic/*` tests, `zoid/src/agent.rs`).

**Interfaces:**
- Produces: `CompletionRequest.reassert: Option<String>`.

- [ ] **Step 1: Add the field** (`lib.rs`, after `thinking`)

```rust
    pub thinking: ThinkingMode,
    /// Live-edge re-assertion text (spec: re-floor). `None` = no reminder this
    /// request (body byte-identical to pre-feature). `Some` = adapters render it
    /// at the tail (per-adapter placement).
    pub reassert: Option<String>,
```

- [ ] **Step 2: Enumerate + fix broken constructors**

Run: `cargo build --workspace 2>&1 | rg "missing field .reassert"`
Add `reassert: None,` to every reported struct-literal site (including `build_request_with_thinking` in `agent.rs`).

- [ ] **Step 3: Verify build + tests**

Run: `cargo build --workspace 2>&1 | rg "missing field" ; cargo test -p zoid-provider`
Expected: no missing-field output; PASS (bodies unchanged — field unused).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-provider/src crates/zoid/src/agent.rs
git commit -m "feat(provider): add neutral CompletionRequest.reassert field (unused)"
```

---

### Task 4: Anthropic rendering — append reminder as a text block on the last message

**Files:**
- Modify: `crates/zoid-provider/src/anthropic/request.rs` (`build`)
- Test: inline in `anthropic/request.rs` (use the module's real `req(messages, tools, system)` helper + `serde_json::to_value(build(&r))`)

**Interfaces:**
- Consumes: `CompletionRequest.reassert`. Uses `MessageContent::{Text(String), Blocks(Vec<ContentBlock>)}` and `ContentBlock::Text { text, cache_control }` (`anthropic/types.rs`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reassert_rides_on_last_message_as_user_role() {
    // req(messages, tools, system): tail is a plain user message (MessageContent::Text).
    let mut r = req(vec![Message::user("do the thing")], vec![], None);
    r.reassert = Some("STANDING REMINDER".to_string());
    let body = serde_json::to_value(build(&r)).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "user", "tail stays user-role (alternation preserved)");
    assert!(serde_json::to_string(&last["content"]).unwrap().contains("STANDING REMINDER"));
    assert_eq!(msgs.len(), 1, "no new trailing message added");
}

#[test]
fn reassert_none_is_byte_identical_anthropic() {
    let r = req(vec![Message::user("hi")], vec![], None);
    let mut r2 = r.clone();
    r2.reassert = None;
    assert_eq!(serde_json::to_value(build(&r)).unwrap(), serde_json::to_value(build(&r2)).unwrap());
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p zoid-provider reassert_rides_on_last_message`
  Expected: FAIL — reminder absent.

- [ ] **Step 3: Implement** — in `build`, AFTER the messages vec is assembled and BEFORE `place_breakpoints(&mut out)`, fold the reminder onto the last message. `MessageContent` is an enum (not a Vec), so handle both variants:

```rust
    if let Some(text) = &req.reassert {
        if let Some(last) = out.messages.last_mut() {
            let block = ContentBlock::Text { text: format!("\n\n{text}"), cache_control: None };
            match &mut last.content {
                MessageContent::Text(s) => {
                    last.content = MessageContent::Blocks(vec![
                        ContentBlock::Text { text: std::mem::take(s), cache_control: None },
                        block,
                    ]);
                }
                MessageContent::Blocks(blocks) => blocks.push(block),
            }
        }
    }
```

(Placing it before `place_breakpoints` lets the 1h breakpoint re-home onto the reminder block — acceptable; the reminder turn's cache write is ephemeral. Import `MessageContent`/`ContentBlock` in scope if not already.)

- [ ] **Step 4: Run to verify pass** — Run: `cargo test -p zoid-provider reassert` → PASS (both).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic/request.rs
git commit -m "feat(provider): render reassert as trailing text block on last message (anthropic)"
```

---

### Task 5: openai-compat rendering — trailing system message

**Files:**
- Modify: `crates/zoid-provider/src/openai_compat.rs` (`request_body`)
- Test: inline (construct `CompletionRequest { .. }` literally, as the module's existing tests do)

**Interfaces:**
- Consumes: `CompletionRequest.reassert`. Entry point: `pub fn request_body(req: &CompletionRequest) -> Value`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reassert_pushes_trailing_system_message_openai() {
    let mut req = CompletionRequest {
        model: "m".into(), system: None, messages: vec![Message::user("hi")],
        max_tokens: 16, tools: vec![], thinking: ThinkingMode::Off, reassert: None,
    };
    req.reassert = Some("STANDING REMINDER".into());
    let body = request_body(&req);
    let last = body["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "system");
    assert_eq!(last["content"], "STANDING REMINDER");
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p zoid-provider reassert_pushes_trailing_system_message_openai`
  Expected: FAIL.

- [ ] **Step 3: Implement** — in `request_body`, after the conversation messages are pushed and before building the final `json!({... "messages": messages ...})`:

```rust
    if let Some(text) = &req.reassert {
        messages.push(json!({ "role": "system", "content": text }));
    }
```

- [ ] **Step 4: Run to verify pass** — Run: `cargo test -p zoid-provider reassert` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/openai_compat.rs
git commit -m "feat(provider): render reassert as trailing system message (openai-compat/zai)"
```

---

### Task 6: Ollama-native rendering — trailing system message

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs` (`request_body`)
- Test: inline

**Interfaces:**
- Consumes: `CompletionRequest.reassert`. Entry point: `pub fn request_body(req: &CompletionRequest) -> Value`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reassert_pushes_trailing_system_message_ollama() {
    let mut req = CompletionRequest {
        model: "m".into(), system: None, messages: vec![Message::user("hi")],
        max_tokens: 16, tools: vec![], thinking: ThinkingMode::Off, reassert: None,
    };
    req.reassert = Some("STANDING REMINDER".into());
    let body = request_body(&req);
    let last = body["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "system");
    assert_eq!(last["content"], "STANDING REMINDER");
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p zoid-provider reassert_pushes_trailing_system_message_ollama`
  Expected: FAIL.

- [ ] **Step 3: Implement** — in `request_body`, after the role-mapping loop pushes conversation messages:

```rust
    if let Some(text) = &req.reassert {
        messages.push(json!({ "role": "system", "content": text }));
    }
```

- [ ] **Step 4: Run to verify pass** — Run: `cargo test -p zoid-provider reassert && cargo test -p zoid-provider` → PASS (all adapters).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): render reassert as trailing system message (ollama-native)"
```

---

### Task 7: Config — `reassert_interval_tokens` (partial + merge + resolved default)

**Files:**
- Modify: `crates/zoid-core/src/config.rs` — `EconomyConfig` (+`Default`), `PartialEconomy`, the economy merge block, and the `Source`/provenance plumbing. **Mirror `compact_threshold_pct` at EVERY site it appears** (grep it first).
- Test: inline in `config.rs`

**Interfaces:**
- Produces: `EconomyConfig.reassert_interval_tokens: u64` (default `100_000`, `0` disables); `PartialEconomy.reassert_interval_tokens: Option<u64>`.

- [ ] **Step 1: Grep the pattern to replicate**

Run: `rg -n "compact_threshold_pct" crates/zoid-core/src/config.rs`
This lists every site (struct, Default, PartialEconomy, merge, Source/provenance). The new field mirrors each.

- [ ] **Step 2: Write the failing tests**

```rust
#[test]
fn economy_default_reassert_interval_is_100k() {
    assert_eq!(EconomyConfig::default().reassert_interval_tokens, 100_000);
}

#[test]
fn parse_reassert_interval_into_partial() {
    let (pc, _unknown) = parse_toml("[economy]\nreassert_interval_tokens = 250000").unwrap();
    assert_eq!(pc.economy.reassert_interval_tokens, Some(250_000));
}
```

- [ ] **Step 3: Run to verify they fail** — Run: `cargo test -p zoid-core reassert_interval`
  Expected: FAIL — no such field.

- [ ] **Step 4: Add to `EconomyConfig`**

```rust
    /// Re-assert the system prompt at the live edge every N estimated-appended
    /// tokens of novel content. 0 disables. Default 100_000. Units: estimate_tokens (chars/3).
    pub reassert_interval_tokens: u64,
```

- [ ] **Step 5: Add to `Default for EconomyConfig`**: `reassert_interval_tokens: 100_000,`

- [ ] **Step 6: Add to `PartialEconomy`**: `pub reassert_interval_tokens: Option<u64>,`

- [ ] **Step 7: Wire the merge + provenance** — at each site the Step-1 grep reported for `compact_threshold_pct` (the `PartialEconomy → EconomyConfig` merge, and any `Source`/provenance struct + assignment), add the matching `reassert_interval_tokens` line, using `unwrap_or(100_000)` (or the crate's default-fallback idiom) in the merge.

- [ ] **Step 8: Run to verify pass + build**

Run: `cargo test -p zoid-core reassert_interval && cargo build -p zoid-core`
Expected: PASS + clean.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [economy].reassert_interval_tokens (default 100k, 0 disables)"
```

---

### Task 8: `wrap_reassertion` + `TurnConfig.reassert_interval` + thread `reassert` through `build_request`

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`wrap_reassertion`; `TurnConfig.reassert_interval` + its manual `Debug`; default `0` in `chat_turn_config_with`; new `reassert` param on `build_request_with_thinking` + `build_request` wrapper)
- Modify: `crates/zoid/src/main.rs` (set `turn_config.reassert_interval = app.economy.reassert_interval_tokens` where the other economy-derived `TurnConfig` fields are set)
- Test: inline in `agent.rs`

**Interfaces:**
- Produces: `pub fn wrap_reassertion(system: &str) -> String`; `TurnConfig.reassert_interval: u64`; `build_request_with_thinking(events, model, tools, system, thinking, reassert: Option<String>)`.

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

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p zoid wrap_reassertion_frames`
  Expected: FAIL — not defined.

- [ ] **Step 3: Implement `wrap_reassertion`** (near `SYSTEM_PROMPT`)

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

- [ ] **Step 4: Add `TurnConfig.reassert_interval`** — add `pub reassert_interval: u64,` to the struct, a `.field("reassert_interval", &self.reassert_interval)` line in the manual `Debug` impl, and `reassert_interval: 0,` in `chat_turn_config_with` (subagents/tests default off).

- [ ] **Step 5: Thread `reassert` through `build_request_with_thinking`** — add `reassert: Option<String>` as the last param; set `reassert` on the `CompletionRequest` literal (replacing the `reassert: None` from Task 3). Update the `build_request` convenience wrapper to accept + forward `None`. Fix all callers (they pass `None` for now; Task 9 changes the loop caller).

- [ ] **Step 6: Set the interval in the bin** — in `main.rs`, next to `turn_config.policy = …` / `turn_config.eviction = …`:

```rust
    turn_config.reassert_interval = app.economy.reassert_interval_tokens;
```

- [ ] **Step 7: Run tests + build** — Run: `cargo test -p zoid wrap_reassertion_frames && cargo build --workspace` → PASS + clean.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(agent): wrap_reassertion + TurnConfig.reassert_interval + build_request reassert param"
```

---

### Task 9: Fire the re-floor in the turn loop (+ AgentUpdate variant & handler, preflight, calibration, observability)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`run_turn_inner` loop; `AgentUpdate` enum; `record_compactions` calibration skip)
- Modify: `crates/zoid/src/main.rs` (`AgentUpdate` match arm — **exhaustive, no wildcard**, so the handler MUST land in THIS task or the bin won't compile)
- Test: inline turn-loop test in `agent.rs` (fake provider)

**Interfaces:**
- Consumes: `reassert::{reassertion_due, cumulative_appended}`, `wrap_reassertion`, `config.reassert_interval`, `ContextOverhead` (derives `Clone`, has `pub system_tokens`).
- Produces: `AgentUpdate::DirectiveReasserted { at_cumulative: u64 }`; a persisted `EventKind::DirectiveReasserted` per fire.

- [ ] **Step 1: Add the UI variant + its bin handler together (B2)**

In `agent.rs` `AgentUpdate`:

```rust
    /// A re-floor fired: the system prompt was re-asserted at the live edge.
    DirectiveReasserted { at_cumulative: u64 },
```

In `main.rs` `AgentUpdate` match (mirror the `CompactionComplete` arm — a lightweight, non-transcript status surface; do NOT set the bottom-bar `status_hint`):

```rust
                    AgentUpdate::DirectiveReasserted { at_cumulative } => {
                        app.reassert_count = app.reassert_count.saturating_add(1);
                        tracing::info!(kind = "reassert", at = at_cumulative, "re-floor surfaced");
                    }
```

Add `reassert_count: u64` (default 0) to the bin's `App` struct next to the other counters.

- [ ] **Step 2: Write the failing test** — drive `run_turn_inner` (or `run_agent_turn`) with a fake provider that (a) records whether the request it received had `reassert.is_some()`, and (b) returns a final text answer. Seed a log whose `cumulative_appended` already exceeds a small `reassert_interval`. Assert the recorded request had `reassert = Some`, and the resulting log contains a `DirectiveReasserted` event. Mirror the existing fake-provider tests in `agent.rs`.

```rust
#[tokio::test]
async fn re_floor_fires_and_persists_marker_on_success() {
    // (Mirror the structure of existing run_turn_inner tests: a recording fake
    // provider, a seed log with > interval estimated-appended tokens, small
    // reassert_interval in TurnConfig. Assert request.reassert.is_some() and a
    // persisted EventKind::DirectiveReasserted.)
}
```

- [ ] **Step 3: Run to verify it fails** — Run: `cargo test -p zoid re_floor_fires_and_persists_marker`
  Expected: FAIL — no reassert injected, no marker.

- [ ] **Step 4: Implement the trigger — compute BEFORE preflight (S2/S3 ordering)** — at the top of `'turn: loop`, replacing the existing `preflight_gate(...)` + `build_request_with_thinking(...)` sequence. Order is mandatory: decide → size → preflight → build.

```rust
        // Decide re-floor FIRST so its ephemeral tokens are in the preflight size.
        let will_reassert = config.reassert_interval > 0
            && zoid_core::reassert::reassertion_due(events.iter(), config.reassert_interval);
        let reassert_text = will_reassert.then(|| wrap_reassertion(&config.system));

        let mut overhead_now = overhead.clone();
        if let Some(t) = &reassert_text {
            overhead_now.system_tokens += zoid_core::economy::estimate_tokens(t);
        }

        preflight_gate(&session, &mut events, ui, config, session_id, now,
                       &*calibration_ratio, &overhead_now).await?;

        let req = build_request_with_thinking(
            &events, &model, &tools, &config.system, config.thinking, reassert_text.clone(),
        );
```

(Pass `&overhead_now` to `preflight_gate` in place of the previous `overhead`. The rest of `preflight_gate`'s call args stay as they are today.)

- [ ] **Step 5: Emit the marker only after a successful stream** — after the streaming inner loop and `let _ = stream_task.await;` (the post-stream point, reached ONLY on clean `Done`/close — the context-length `continue 'turn`, the error `break 'turn`, and the abort `break 'turn` all exit before it), add:

```rust
        if will_reassert {
            let at = zoid_core::reassert::cumulative_appended(events.iter());
            emit(&session, &mut events, ui, &config.branch,
                 EventKind::DirectiveReasserted { at_cumulative: at }, session_id, now).await?;
            let _ = ui.send(AgentUpdate::DirectiveReasserted { at_cumulative: at }).await;
            tracing::info!(kind = "reassert", at, "re-floor fired");
        }
```

- [ ] **Step 6: Skip calibration on re-floor sub-turns (S3)** — thread `will_reassert` into `record_compactions` (add a `skip_calibration: bool` param) and guard the `calibration_ratio` update so it is NOT updated when the request carried the extra ephemeral system copy.

- [ ] **Step 7: Run tests + full build**

Run: `cargo test -p zoid re_floor_fires_and_persists_marker && cargo build --workspace && cargo test -p zoid`
Expected: PASS + clean (bin compiles because the `AgentUpdate` handler landed in Step 1).

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(agent): fire re-floor in turn loop (preflight-accounted, marker-after-send, calibration-safe, observable)"
```

---

### Task 10: End-to-end integration + steady-state liveness guard (B1 regression)

**Files:**
- Test: `crates/zoid/tests/` (new integration test, or extend an existing turn test)

**Interfaces:**
- Consumes: the full stack (config → loop → fake provider).

- [ ] **Step 1: Write the integration test** — a long synthetic session that exceeds `context_target` with eviction AND compaction (body-clear) active, small `reassert_interval`. Assert re-floors keep firing PAST `context_target` (no dormancy) and each fired request carried the reminder. It MUST run the log through `clear_compacted_bodies` (not just eviction) — that is the case that reopened B1.

```rust
#[tokio::test]
async fn re_floor_keeps_firing_in_steady_state_with_compaction() {
    // Build the turn stack with reassert_interval small; feed many turns of
    // large tool output; apply compaction body-clears between turns; count
    // DirectiveReasserted events. assert!(fires >= expected_many,
    // "re-floor must not go dormant past context_target under compaction").
}
```

- [ ] **Step 2: Run to verify** — Run: `cargo test -p zoid re_floor_keeps_firing_in_steady_state`
  Expected: PASS (FAIL first if any wiring gap remains; fix, then pass).

- [ ] **Step 3: Full workspace green**

Run: `cargo test --workspace && cargo clippy --workspace`
Expected: PASS + no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/tests
git commit -m "test(agent): steady-state re-floor liveness under eviction+compaction (B1 guard)"
```

---

## Manual acceptance (post-implementation, not a task)

Unit tests cannot prove drift is reduced or that GLM won't wrap up early. Run a long real zai / glm-5.2 session; watch `tracing` for `kind="reassert"` fires (and/or `app.reassert_count`), confirm the cadence, watch for early-termination regressions, and tune `[economy].reassert_interval_tokens`.
