# Demand-Paged Context (ACM ceiling) — Design

**Date:** 2026-07-04
**Status:** Design approved (brainstorm); implementation plan to follow (`writing-plans`).
**Supersedes/extends:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md` (§4 short-term arc). ACM-1 (tool-result compaction, shipped/merged) is a component of this design, not a replacement target.

## One-line goal

Hold the **live request** (tokens actually sent to the model each turn) within a configurable band around a setpoint — default setpoint ~384k, operating band ~300–500k on 1M-context models — across **indefinite** sessions, auto-managed, surfaced, and undoable, with **nothing truly forgotten**: evicted history stays queryable.

## Architecture in one paragraph

Split the event history into a **hot working set** (the events projections replay and send to the model) and a **cold tier** (evicted events that remain in sqlite, FTS5-indexed, and are *not* replayed). A hysteresis **eviction controller** keeps the hot set inside the band by first compacting tool results (ACM-1, shipped) and then evicting the oldest *whole turns*, leaving an in-context **breadcrumb marker** so the model knows history exists and how to reach it. A **`recall()` tool** searches the cold tier (BM25 via sqlite FTS5, no embeddings) and re-admits matching turns on demand. The one cold tier serves three consumers at once: it bounds the request, bounds working-set RAM/CPU, and backs recall.

---

## 1. Problem & current state

### 1.1 The regression (both failure modes are real and in the tree)

- **"Removed all history" (old bug).** The pre-diff `conversation()` sliced the event log at the last `TurnsDropped` **timestamp cutoff**. Because the token estimate reflected the full context the model received, `current > threshold` stayed true after each drop, so `TurnsDropped` re-fired and cascaded until a single turn survived — then re-thrashed on the next message.
- **"Under-evicts / can't hold the ceiling" (current state).** The parallel session's uncommitted diff correctly **deleted layer-4 turn-dropping** and sharpened the token estimate (chars/4 → chars/3, counts system + tool-spec overhead, learns a calibration ratio from real provider usage). That stops the thrash — but it leaves the live request with **only one** reduction lever: tool-result compaction. Compaction shrinks only `ToolResult`/`File` bodies to head+footer and skips already-compacted items; once every tool result is summarized, nothing further can shrink while `running` can still exceed the ceiling. Messages and whole turns are never touched.

### 1.2 The ceiling is enforced in the wrong place

`token_ceiling` and heat-based eviction exist in `assemble_context` (`crates/zoid-core/src/assembler.rs`), but its **only caller is the subagent path** (`crates/zoid/src/subagent.rs`). The live request (`build_request` → `conversation`) never calls it. So on the live path, `token_ceiling` is dead weight and there is no mechanism to bound an indefinite session.

### 1.3 Storage / replay findings (why "indefinite" breaks before sqlite does)

- Full log held in memory as `Vec<Event>` (`crates/zoid/src/main.rs:918`); projections replay the whole slice each turn. `context_window`/`estimate_tokens` **re-walk every character of every stored tool output** (`chars/3`) on every structural frame. Dominant cost term is **total tool-output bytes, not turn count**.
- Resume is one unbounded `SELECT * WHERE session_id` (`crates/zoid-core/src/store.rs:105-112`) that materializes and JSON-parses the entire lifetime log into RAM. No windowing.
- Append-only; **compaction never reclaims** — it adds a `ToolResultCompacted` event and keeps the original blob. No retention anywhere.
- **Conclusion:** sqlite disk is the *last* thing to hurt (handles GB of append-only blobs). The *first* bottlenecks are the in-memory per-turn byte-walk and the unbounded resume load — both fixed by bounding the **working set**, which is the same mechanism that bounds the request.

---

## 2. Key decisions (resolved forks)

1. **Ceiling governs tokens sent per turn** (not accumulated log size). Steady-state band lives *around* the setpoint on a 1M-window model.
2. **Model windows are 1M** for claude and glm-5.2. `crates/zoid-provider/src/model.rs` currently lists 200k/256k — **wrong/stale**. Fixing to 1M is **step 0**; all ceiling math scales off it.
3. **Eviction unit = whole turn**, never loose items — preserves `tool_use`/`tool_result` pairing in the message list. (Item-level heat eviction stays confined to the subagent path where it already lives.)
4. **Hysteresis, not edge-trimming.** Cross a **high-water** mark → evict down to a **low-water** mark in one wave. This *is* the operating band.
5. **Explicit evicted-id set, never a timestamp cutoff.** The cutoff was the thrash bug. Eviction records the exact ids removed; the projection honors *those*, making it idempotent.
6. **Protection is structural, not heuristic.** System/`Immutable`, `pinned`, and the most-recent-*N* turns are type-level un-selectable by the controller — not gated on a heat threshold that can be misconfigured.
7. **Graduated levers:** (1) compact tool results [shipped] → (2) evict oldest turns. Compaction runs first each wave; eviction only if still over low-water.
8. **Demand-paged, not lossy:** evicted turns leave a **breadcrumb marker** in-context and are retrievable via `recall()`. The marker is load-bearing — without it, demand-paging silently degrades to amnesia.
9. **Recall needs no embeddings for v1** — sqlite **FTS5 (BM25)**. Embeddings (ACM-2) are a later quality upgrade, not a blocker.
10. **Pure-core / effectful-bin seam preserved.** `zoid-core` gains only pure additions (`ItemKind::Retrieved`, eviction/marker/controller logic). The FTS index and the `recall` tool live in the **bin**, which already owns sqlite and the `Tool` trait.
11. **Auto + surfaced + undoable.** The controller runs every turn without prompting; each eviction renders in the transcript (semantic zoom) and is undoable by re-admitting the ids (append-only ⇒ reversible).
12. **Append-only reversibility retained.** Eviction and recall are new events; original events are never mutated or deleted from the log. (Physical reclamation of cold blobs from the hot `Vec`/resume path is Slice 3, and still never deletes from sqlite.)

---

## 3. Components

### 3.1 Eviction controller (pure, `zoid-core`)

A pure planner analogous to `plan_compactions`:

```
plan_evictions(events, policy, current_tokens) -> EvictionPlan
```

- Operates on the projected hot working set (post-compaction).
- **Trigger:** only when `current_tokens > high_water`.
- **Selection:** oldest evictable **turns** first, accumulating reclaimed tokens until `current_tokens - reclaimed <= low_water`.
- **Evictable turn** = a contiguous message group whose items are all `Normal` protection, not `pinned`, not System/`Immutable`, and **older than the most-recent-*N*-turns window**.
- **Idempotent:** turns already evicted (their ids present in a prior `TurnsEvicted`) are skipped; re-running with no new pressure yields an empty plan.
- **Never empties the window:** the most-recent-*N* turns are structurally excluded, so the plan can leave `current_tokens` above `low_water` (or even above `high_water`) rather than evict protected content. This is correct behavior, surfaced as a warning (see §6).
- Emits `EvictionPlan { turns: Vec<EvictedTurn { ids, token_estimate, topic_hint }> }`.

`topic_hint` is a cheap extractive label (e.g. first user-message line of the turn, truncated) — **no LLM call** — used in the breadcrumb marker.

### 3.2 Events (pure, `zoid-core`)

- `EventKind::TurnsEvicted { ids: Vec<EventId>, reclaimed_tokens: u64, marker: EvictionMarker }` — append-only; original events retained.
- `EvictionMarker { spans: Vec<{ id_range_label, token_estimate, topic_hint }> }` — the data the transcript renders and the model reads.
- `EventKind::TurnsReadmitted { ids: Vec<EventId> }` — the undo / recall re-admission event; projections stop skipping these ids.
- The inert `TurnsDropped` variant is left as-is (backward-compatible deserialization); nothing new emits it.

### 3.3 Projections (pure, `zoid-core`)

- `conversation()` and `context_window()` maintain an **evicted-id set** (folded from `TurnsEvicted` minus `TurnsReadmitted`) and **early-`continue`** on any event whose id is evicted — the same shape as the existing subagent-branch skip. This bounds the request **and** the per-turn byte-walk together.
- `conversation()` injects the **breadcrumb marker** as a synthetic message at the position of each evicted span, so the model sees `[N turns evicted here · ~Xk tokens · topics: … · recall("…") to retrieve]`.
- Retrieved turns (via `recall`) are re-admitted through `TurnsReadmitted` and flow back into the projection normally, tagged `ItemKind::Retrieved` for scoring/UX.

### 3.4 Recall tool (effectful, bin)

- New `recall` tool (`ToolKind::Local`) in the bin: `recall(query: string, limit?: int)`.
- Backed by a sqlite **FTS5** virtual table over evicted event content (built/maintained in the bin's store alongside the `events` table).
- Returns coherent **rendered turns** (not raw `tool_use`/`tool_result` JSON), each with its original event ids.
- Re-admission: recall appends `TurnsReadmitted { ids }` so the turns re-enter the hot set as `Retrieved` items, subject to the controller (a recall can itself age back out).
- Miss → a normal empty/"no matches" tool result (not an error).

### 3.5 Cold-paging (Slice 3, deferrable)

- Stop materializing evicted blobs in the hot `Vec<Event>`; keep evicted ids + marker metadata hot, page full bodies from sqlite only on recall.
- **Windowed resume load:** load the live-window tail into the hot `Vec`; leave older events in sqlite (reachable via recall). Fixes the RAM/resume curve.
- Never deletes from sqlite — cold storage is the recall corpus and the undo backstop.

### 3.6 Config (bin + `zoid-core` `EconomyConfig`)

- **Step 0:** `model.rs` context windows → 1,000,000 for claude and glm-5.2 (conservative default for unknown models unchanged).
- Generalize the single `compact_threshold_pct` into a small ceiling policy: **setpoint / high-water / low-water** expressed as percent-of-ceiling (or absolute token counts), plus **`recent_n`** (protected recent-turn count) and a master enable. Keep back-compat: `compact_threshold_pct = 0` still disables ACM.
- Wire the resolved ceiling into the **live** turn config (today it only reaches the subagent path).

---

## 4. Data flow (one turn)

1. Assemble request from `conversation(events)` (skips evicted ids, injects markers).
2. Estimate `current_tokens` (real provider usage if available, else calibrated estimate).
3. If `current_tokens > high_water`: run **compaction** (ACM-1), re-estimate; if still over, run `plan_evictions` down to `low_water`; append `TurnsEvicted` before re-request.
4. Model may call `recall(query)` → FTS5 → append `TurnsReadmitted` → matching turns re-enter next assembly.
5. Transcript renders compaction (`⧟`) and eviction markers with semantic zoom; user can undo an eviction (re-admit) from the UI.

---

## 5. UX (surfaced + undoable)

- Eviction marker in transcript, consistent with the existing compaction glyph treatment: **Summary** = one-line chip (`⋯ 12 turns paged out · 14k`), **Detail** = per-span breakdown with topic hints and a recall/undo affordance.
- Context/economy drawer shows current live tokens vs band (setpoint/high/low) and count of paged-out turns.
- Undo = re-admit the span's ids (`TurnsReadmitted`); the controller may re-evict on the next wave if still over — surfaced, not silent.

---

## 6. Error handling & edge cases

- **Can't reach low-water without touching protected content** → stop at the protected boundary, keep well-formedness, and surface a warning (the request may exceed the band when recent-*N* alone is large). Never evict protected turns to hit a number.
- **Recall miss** → empty result, not an error.
- **FTS5 unavailable / index build failure** → recall degrades to "unavailable"; eviction still functions (the marker still tells the model history existed). Log loudly; do not crash the turn.
- **Well-formedness** → eviction always removes complete turns; a partial turn (streaming in progress) is never evicted.
- **Calibration** → keep the parallel session's real-usage calibration; eviction decisions use the same `current_tokens` source as compaction.

## 7. Testing strategy

- **Steady-state property test (the missing coverage that let this regress):** simulate a long multi-turn session (hundreds of turns, large tool outputs); assert the live request stays `<= ceiling` **for every turn** and never drops below the recent-*N* protected floor. This "holds the band over time" property has **no test today** — its absence is the root cause the regression shipped.
- Eviction **idempotence / no-thrash:** re-running `plan_evictions` with no new pressure yields an empty plan; a single wave reaches low-water in one pass.
- **Protection invariants:** System/`Immutable`, `pinned`, and recent-*N* turns are never in any `EvictionPlan`.
- **Explicit-id (no cutoff):** an evicted id set removes exactly those turns, and later turns are unaffected.
- **Recall round-trip:** evict a turn, `recall(query)` finds it, `TurnsReadmitted` re-admits it, projection includes it again.
- **Undo restores** the exact evicted content.
- **Marker present** whenever anything is evicted (guards against silent amnesia).
- Reuse ACM-1's discipline: cross-crate field adds to shared types must be built with `--workspace` (a prior slice broke zoid-tui literals when tests were scoped to `-p zoid-core`).

## 8. Slicing & sequencing

- **Slice 1 — bounded turn-eviction + breadcrumb, honored by skip-in-fold.** Holds the ceiling and bounds per-turn CPU. Includes step-0 (1M windows) and the config generalization. Must-have; fixes the reported bug.
- **Slice 2 — `recall()` over cold sqlite (FTS5).** Makes eviction non-lossy. **Built with Slice 1 as one coherent unit** (eviction without recall is lossy; recall without eviction is pointless).
- **Slice 3 — cold-paging + windowed resume.** Fixes the RAM/resume curve; reuses Slice 2's cold tier. Most runway; deferrable.
- **Slice 4 — embeddings upgrade to recall (ACM-2).** Quality only; deferrable indefinitely.

**Implementation plan scope: Slices 1+2.** Slices 3+4 are documented here but out of the first plan.

## 9. Relationship to existing code & in-flight work

- Builds directly on the (now-committed) baseline that removed layer-4 turn-dropping and added token calibration — the correct foundation. The new eviction is the **bounded, explicit-id** replacement for that removed layer.
- Reuses ACM-1 wholesale: compaction stays lever 1; the projection-substitution wire-in point (`conversation()`) is exactly where eviction also intervenes.
- Key anchors: `projection.rs` (conversation fold + skip), `context.rs:184-317` (context_window fold, per-event estimate), `compaction.rs:52` (`plan_compactions`, sibling to `plan_evictions`), `assembler.rs` (existing hysteresis/heat logic to mirror, not reuse directly), `store.rs:16-40` (events schema; FTS5 table added alongside), `main.rs:918`/`1061`/`2093` (hot `Vec`, boot/resume load — Slice 3 windowing target), `agent.rs:163-176` (request build), `crates/zoid-provider/src/model.rs` (step-0 window fix).

## 10. Non-goals / out of scope

- Semantic (embedding) relevance scoring — ACM-2 (Slice 4).
- Physical deletion / vacuuming of cold sqlite rows.
- Changing compaction's tool-result behavior (kept as-is).
- Item-level live eviction (stays turn-level on the live path; item/heat eviction remains subagent-only).
- Multi-model routing / pricing (explicitly tokens-only by design).
