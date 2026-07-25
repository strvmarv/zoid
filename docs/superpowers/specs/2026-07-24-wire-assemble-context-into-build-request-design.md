# Wire `assemble_context` into `build_request` — Design

> **Status:** DESIGN (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** `docs/superpowers/specs/2026-07-24-acm-followups-roadmap.md` (item 3).
> **Parent vision:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md`
> (§4.4/§8 — the assembler as the single chokepoint for "what gets sent").

---

## 1. Goal & scope

Make `assemble_context` the input to `build_request` — the assembler becomes
the decision layer for what messages reach the model. Today `build_request`
reads the post-gate event log via `conversation_for_branch → ChatMsg → map_msg`.
After this change, it reads the assembler's `ContextSelection` and maps the
*included* items to provider messages.

**In scope:**
- `ContextItem` gains `chatmsg_index: Option<usize>` — a back-reference to a
  parallel `Vec<ChatMsg>` produced alongside the `ContextWindow`.
- A new `context_window_and_messages` function that produces both the
  `ContextWindow` and the `Vec<ChatMsg>` in one pass, with each `ContextItem`
  tagged with its `ChatMsg` index.
- `build_request_with_thinking` consumes the assembler's `ContextSelection`
  instead of calling `conversation_for_branch` directly. It collects the unique
  `chatmsg_index` values from included items and maps those `ChatMsg`s through
  the existing `map_msg`.
- A regression guard: when the assembler's policy is a no-op (no ceiling, no
  auto-evict, no compaction), the new path produces byte-identical `Message[]`
  to the old `conversation_for_branch → filter(Evicted) → map_msg` path.

**Out of scope:**
- Removing the gate (`preflight_gate`). The gate still runs, still mutates the
  event log (eviction + compaction), and still emits `TurnsEvicted` events.
  The assembler operates on the *post-gate* log. The gate is not replaced —
  `build_request` just reads through the assembler instead of through the
  projection directly.
- Per-kind policy (item 5), cost routing (item 9), additive retrieval (item 7).
  These compose on top of the assembler in future slices.
- Subagent path convergence (the subagent already uses `assemble_context`, but
  for a different purpose — task-prompt construction, not conversation
  selection). Unifying the two paths is a later slice.
- The `ChatMsg::Evicted` chip rendering (item 2, shipped). Unchanged — the
  projection still emits `ChatMsg::Evicted` for the TUI; `build_request` just
  no longer calls the projection directly.

---

## 2. Data model

### 2.1 `ContextItem` gains `chatmsg_index` (context.rs)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub key: String,
    pub label: String,
    pub kind: ItemKind,
    pub tokens: u64,
    pub heat: Heat,
    pub pinned: bool,
    pub evicted: bool,
    pub compacted: bool,
    /// Index into the parallel `Vec<ChatMsg>` produced by
    /// `context_window_and_messages`. `None` for `System` overhead and `File`
    /// items (they don't produce provider messages). `Some(i)` for `Message`
    /// and `ToolResult` items, pointing at the corresponding `ChatMsg`.
    pub chatmsg_index: Option<usize>,
}
```

`Option<usize>` is `Eq`, so `ContextItem` retains its `Eq` derive. The economy
view doesn't read `chatmsg_index` — it's a no-op for existing consumers.

### 2.2 `context_window_and_messages` (context.rs)

A new function that produces both the `ContextWindow` and the `Vec<ChatMsg>`
in one pass:

```rust
/// Like `context_window_with`, but also produces the parallel `Vec<ChatMsg>`
/// (the conversation projection) and tags each `ContextItem` with its
/// `chatmsg_index` into that vector. `build_request` uses this to resolve
/// the assembler's selection back to provider messages.
///
/// The `Vec<ChatMsg>` is identical to what `conversation_for_branch` would
/// produce from the same events (regression guard: byte-identical).
pub fn context_window_and_messages<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    overhead: ContextOverhead,
    active_branch: &crate::event::BranchId,
) -> (ContextWindow, Vec<ChatMsg>) {
    // ... single pass over events, producing both items and ChatMsgs ...
}
```

The existing `context_window` and `context_window_with` functions are unchanged
— they delegate to the new function and discard the `Vec<ChatMsg>`. Existing
callers (economy view, subagent path) are unaffected.

The `chatmsg_index` is set during the single pass:
- `UserMessage` → `ContextItem { kind: Message, chatmsg_index: Some(i) }` where
  `i` is the index of the `ChatMsg::User` just pushed.
- `ModelDelta` flush → `ContextItem { kind: Message, chatmsg_index: Some(i) }`
  where `i` is the index of the `ChatMsg::Assistant` just flushed.
- `AssistantMessage` → same as `ModelDelta` flush.
- `ToolResult` → `ContextItem { kind: ToolResult, chatmsg_index: Some(i) }`
  where `i` is the index of the `ChatMsg::ToolResult` just pushed.
- `System` overhead → `chatmsg_index: None`.
- `File` items (from tool-result path inference) → `chatmsg_index: None`.

### 2.3 Alignment between `ContextItem` grouping and `ChatMsg` grouping

Both `context_window` and `conversation_for_branch` flush `ModelDelta` runs at
the same boundaries: `UserMessage`, `AssistantMessage`, `ToolResult`. Both fold
`ToolCall` events into the preceding assistant message. Both produce one
`ToolResult` item per `ToolResult` event. The groupings are aligned — one
`ContextItem` (with `chatmsg_index: Some(i)`) maps to exactly one `ChatMsg`.

The one subtlety: `context_window` uses `upsert` (re-encountering the same key
updates tokens and increments refs), while the projection always pushes a new
`ChatMsg`. But `upsert` only re-encounters keys for `ToolResult` events that
re-read the same file — and each `ToolResult` event produces its own
`ChatMsg::ToolResult` in the projection. The `chatmsg_index` on a re-upserted
`ContextItem` is updated to point at the latest `ChatMsg::ToolResult` (last
write wins, matching the projection's "last write wins" for compacted
summaries).

---

## 3. `build_request_with_thinking` changes (agent.rs)

### 3.1 New flow

```rust
pub fn build_request_with_thinking(
    events: &crate::eventlog::EventLog,
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
    thinking: ThinkingMode,
    reassert: Option<String>,
    active_branch: &zoid_core::event::BranchId,
) -> CompletionRequest {
    let system = match zoid_core::eviction::eviction_breadcrumb(events.iter()) {
        Some(bc) => format!("{system}\n\n{bc}"),
        None => system.to_string(),
    };
    let max_tokens = /* unchanged */;

    // NEW: build the context window + parallel ChatMsgs, then assemble.
    let overhead = ContextOverhead {
        system_tokens: estimate_tokens(&system),
        tools_tokens: tool_specs_tokens(tools),
    };
    let (window, chatmsgs) = zoid_core::context::context_window_and_messages(
        events.iter(),
        overhead,
        active_branch,
    );
    let policy = ContextPolicy::default(); // no-op for now — gate handles eviction
    let selection = assemble_context(&window, &policy);

    // Collect unique chatmsg indices from included items, in order.
    let mut seen = std::collections::HashSet::new();
    let mut messages: Vec<Message> = Vec::new();
    for item in &selection.included {
        if let Some(idx) = item.chatmsg_index {
            if seen.insert(idx) && idx < chatmsgs.len() {
                messages.push(map_msg(chatmsgs[idx].clone()));
            }
        }
    }

    CompletionRequest {
        model: model.to_string(),
        system: Some(system),
        messages,
        max_tokens,
        tools: tool_specs(tools),
        thinking,
        reassert,
    }
}
```

### 3.2 What stays the same

- The `system` prompt + breadcrumb: unchanged.
- `max_tokens`, `tools`, `thinking`, `reassert`: unchanged.
- `map_msg`: unchanged — it maps `ChatMsg → Message` exactly as today.
- The `ChatMsg::Evicted` filter is no longer needed in `build_request` — the
  assembler excludes evicted items (they have `evicted: true`), so their
  `chatmsg_index` never appears in `selection.included`. But the `ChatMsg::Evicted`
  variant still exists in the `Vec<ChatMsg>` (the projection produces it); it
  just never gets selected because the `ContextItem` for an eviction event has
  `chatmsg_index: None` (eviction events don't produce conversation items).

Wait — `ChatMsg::Evicted` is produced by `TurnsEvicted` events in the projection.
But `context_window` skips evicted events (line 190-192: `if
evicted.contains(&e.id) { continue; }`). And `TurnsEvicted` events themselves
are not user/assistant/tool events — they're metadata markers. So
`context_window` never creates a `ContextItem` for `TurnsEvicted` events, and
the parallel `Vec<ChatMsg>` should NOT include `ChatMsg::Evicted` entries
(those are TUI-only). The parallel `Vec<ChatMsg>` is the *model-facing*
projection — it should skip `TurnsEvicted` the same way the old
`filter(|m| !matches!(m, ChatMsg::Evicted { .. }))` did.

This means `context_window_and_messages` produces a `Vec<ChatMsg>` that is
identical to `conversation_for_branch` **minus** `ChatMsg::Evicted` entries.
The regression guard must account for this: the old path was
`conversation_for_branch → filter(Evicted) → map_msg`; the new path produces
the filtered list directly.

### 3.3 The `ContextPolicy` for the main chat path

For this slice, the policy is `ContextPolicy::default()` — a no-op. The gate
already handles eviction (events are marked `evicted: true` in the
`ContextWindow`), and the assembler's `auto_evict_cold: true` (the default)
excludes cold items when a ceiling is set. But with `token_ceiling: None`
(the default), the assembler includes everything that isn't pinned/evicted —
which is exactly the post-gate log.

Future slices will set `token_ceiling` and `auto_evict_cold` from the gate's
band calculations, moving the eviction decision into the assembler. But that's
out of scope here.

---

## 4. The regression guard

The critical invariant: the new path produces byte-identical `Message[]` to the
old path when the assembler is a no-op.

**Old path:**
```
events → conversation_for_branch(active_branch) → filter(!Evicted) → map_msg → Message[]
```

**New path:**
```
events → context_window_and_messages(active_branch) → assemble_context(default policy) → included items → chatmsg_index → map_msg → Message[]
```

**Test:** build a diverse event log (user messages, assistant messages with
tool calls, tool results, compacted tool results, evicted turns, questions).
Build requests both ways. Assert `messages_new == messages_old`.

The test must cover:
- Plain user/assistant alternation.
- Tool calls + tool results (the `ChatMsg::Assistant` with `tool_calls` +
  `ChatMsg::ToolResult` pairing).
- Compacted tool results (the `compacted` flag replaces content).
- Evicted turns (excluded from both paths).
- `ChatMsg::Evicted` entries (excluded from both paths — old via filter, new
  via `chatmsg_index: None`).
- Questions (the `ChatMsg::Question` variant — included in both).
- Subagent branches (excluded from both — `context_window` skips non-main
  branches, `conversation_for_branch` skips non-active branches).

---

## 5. Cross-crate impact

- **`context.rs` (zoid-core)** — `ContextItem` gains `chatmsg_index:
  Option<usize>`. New `context_window_and_messages` function. All
  `ContextItem { ... }` literals in tests must add `chatmsg_index: None`.
  `context_window` and `context_window_with` delegate to the new function.
- **`assembler.rs` (zoid-core)** — `assemble_context` is unchanged. The
  `ContextPolicy::default()` no-op already includes all non-evicted items.
  The `chatmsg_index` field is carried through transparently (the assembler
  clones `ContextItem`s, so the field is preserved).
- **`agent.rs` (zoid)** — `build_request_with_thinking` rewritten to use
  `context_window_and_messages` + `assemble_context` instead of
  `conversation_for_branch`. The `map_msg` filter for `ChatMsg::Evicted` is
  removed (no longer needed — the assembler excludes evicted items). `map_msg`
  itself is unchanged. The `Evicted` arm in `map_msg` stays (defense-in-depth,
  unreachable).
- **`projection.rs` (zoid-core)** — `conversation_for_branch` is unchanged.
  It's still used by the TUI for rendering. `build_request` no longer calls it,
  but the TUI does.
- `cargo build --workspace && cargo test --workspace` after each task.

---

## 6. Testing

### zoid-core (pure)

- **`context_window_and_messages` produces aligned output:** verify that each
  `ContextItem` with `chatmsg_index: Some(i)` maps to a `ChatMsg` at index `i`
  in the parallel vector. Verify the `Vec<ChatMsg>` is identical to
  `conversation_for_branch` output (minus `ChatMsg::Evicted`).
- **`chatmsg_index` is `None` for System and File items:** verify the overhead
  item and file items have `chatmsg_index: None`.
- **Assembler carries `chatmsg_index` through:** verify
  `assemble_context`'s `included` items retain their `chatmsg_index` values.

### zoid (integration)

- **Regression guard:** build a diverse event log, build requests both ways
  (old `conversation_for_branch → filter → map_msg` vs new
  `context_window_and_messages → assemble → map_msg`), assert
  `messages_new == messages_old`.
- **Existing preflight tests pass:** `preflight_rescues_relevant_old_turn_over_newer_offgoal`,
  `preflight_without_embedder_evicts_the_old_turn`,
  `preflight_rescue_weight_zero_is_pure_recency` — all still pass (the gate
  is unchanged; `build_request` reads the post-gate log through the assembler).
- **Existing `map_msg` tests pass:** the `map_msg` function is unchanged.