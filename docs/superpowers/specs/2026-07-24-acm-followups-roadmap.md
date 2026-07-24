# ACM Follow-ups — Roadmap

> **Status:** Roadmap / index. Captures the remaining ACM work after
> Slice-4b (relevance-rescued eviction) shipped. Items 1–3 are the agreed
> next batch; 4–9 are deferred for later discussion.
>
> **Date:** 2026-07-24
> **Parent vision:** `docs/superpowers/specs/2026-07-03-active-context-management-vision.md`
> **Completed:** Slice-1 (wire-in + tool-result compaction), Slice-2/3
> (demand-paged context: band-holding eviction + cold-tier FTS recall),
> Slice-4 v1 (local embeddings + hybrid recall), Slice-4b (relevance-rescued
> eviction). See `docs/CHANGELOG.md` for the engineering record.

---

## Next batch (1–3)

### 1. `[eviction]` config exposure of `RESCUE_WEIGHT`

**What:** `DEFAULT_RESCUE_WEIGHT` is a `const f32 = 12.0` in
`crates/zoid-core/src/eviction.rs:193`. Expose it as a runtime config key
so it can be tuned without a rebuild.

**Why:** The replay eval (Slice-4b Task 7) fixed 12.0 from *one* dogfood
corpus. Different workflows (long research sessions vs. rapid coding)
may want a different rescue reach. The weight is already analytically
bounded (§5 of the 4b design); a config key just makes the knob
turnable.

**Surface:** Add `rescue_weight: Option<f32>` to `EconomyConfig` (or a
new `[eviction]` section — TBD during design). `None` / absent ⇒ fall
back to `DEFAULT_RESCUE_WEIGHT` const. The `preflight_gate`
`GoalContext` construction (`agent.rs:2791`) reads the resolved value
instead of the const.

**Risk:** Low. Pure config plumbing. Property tests (4b Task 5) already
prove any in-range value is safe.

**Status:** Not yet specced. → `brainstorming` → spec → plan.

---

### 2. Eviction-detail "rescued because…" UI

**What:** The eviction detail view shows *what was evicted* (the victim
turns). It does not show *why a turn was kept* — the rescue rationale is
tracing-only today (`agent.rs:2804`).

**Why:** The vision's trust contract (§1.2: "the user can always see
what zoid did to their context") requires legibility. "Why is this old
turn still here when newer ones were dropped?" is the question rescue
raises. Today it's invisible.

**Blocker:** `EvictionPlan` records *victims* (`EvictedTurn`), not
*survivors* or their rescue scores. Surfacing "kept because relevant"
needs the plan (or a companion structure) to carry per-turn
`keep_score` / `rescue_bump` for the candidates that were *not* evicted.
This is a data-model addition to `EvictionPlan`, then a render change in
the TUI detail view.

**Risk:** Medium. The data model is pure and tested; the TUI change is
presentational. No behavior change.

**Status:** Not yet specced. → `brainstorming` → spec → plan.

---

### 3. Wire `assemble_context` into `build_request`

**What:** Today the main chat request path
(`build_request_with_thinking`, `agent.rs:543`) builds messages directly
from `conversation_for_branch(events.iter(), active_branch)` — a flat
projection that skips evicted ids but does *not* route through
`assemble_context`. `preflight_gate` does compaction + eviction as a
separate pre-pass, mutating the event log, and `build_request` just
reads the result. The pure assembler (`assembler.rs:33`) is called only
by `subagent.rs:61` and tests.

The vision (§4 step 3) calls for `assemble_context` to be the **single
chokepoint** for "what gets sent" — flipping ACM from "gate mutates the
log, request reads it" to "assembler decides what fits, request
consumes the selection." This is the architectural keystone.

**Why:** Until the assembler feeds `build_request`, ACM is a gate-side
side-effect, not a selection. The assembler is the place where
per-kind policy (§3.1), protection levels (§3.2), and the token ceiling
all converge. Wiring it in is what makes the rest of the vision
(per-kind policy, additive retrieval, cost routing) composable instead
of ad-hoc.

**Scope TBD (design questions):**
- Does `build_request` consume a `ContextSelection` (assembler output)
  directly, or does the gate still mutate the log and the assembler
  becomes a *view* over the post-gate log?
- The assembler works on `ContextWindow` (items with `Heat`, `pinned`,
  `evicted`); `build_request` works on `Event`s. Is there a projection
  `ContextWindow → messages`, or does the assembler output items that
  `build_request` maps?
- Subagent dispatch already uses `assemble_context` — does main chat
  converge to the same path, or stay separate?
- How does relevance rescue (4b) interact? Today rescue lives inside
  `plan_evictions` (the gate). If the assembler becomes the decider,
  does rescue move into the assembler's sort, or stay in the gate's
  eviction pass that *feeds* the assembler?

**Risk:** High — this is the largest architectural change. It touches
the hot path (`build_request` runs every turn), the gate, and the
subagent path. Must be done incrementally with the regression guard
(eviction byte-identical when assembler is a no-op).

**Status:** Not yet specced. → `brainstorming` → spec → plan. This is
the item that needs the most design work.

---

## Deferred (4–9) — discuss later

### 4. Announce every mutation through semantic zoom (vision §4 step 4)

Partially done: eviction events render in the conversation; compaction
has start/complete UI states. Missing: the full detail-level breakdown
(per-item *why*: heat/relevance/age, token delta, undo action).

### 5. Per-kind policy (vision §3.1)

Today eviction is turn-granularity with `recent_n` protection. The
vision calls for kind-specific rules (System never evicted; Messages by
recency+relevance; ToolResults by supersession; Files by relevance).
Needs `ItemKind`-keyed policy lookup. Depends on #3 (assembler as
chokepoint) to be useful.

### 6. Tier-2 generative compaction (vision §5)

Frontier-LLM summarization of stale-but-not-droppable messages. The one
place propose-and-confirm applies. Expensive; gated. Seam exists (no
impl).

### 7. RAG / additive retrieval (vision §5)

The `Embedder` index over repo + session history to *pull in* context
the agent never loaded. Adds a `Retrieved` kind. The `Embedder` +
`event_embeddings` infrastructure from Slice-4 v1 is the foundation;
this is the *additive* direction (vs. the *subtractive* curation the
eviction work covers).

### 8. Cross-encoder reranker

`Reranker` trait exists (`retrieval.rs:31`), `NoopReranker` placeholder.
Spike-de-risked (`ms-marco-MiniLM-L-6-v2`, 91 MB, ~21 ms/pair). Improves
recall precision; built after RAG (#7) makes recall volume meaningful.

### 9. Cost / budget-governor / model-routing layer (vision §5)

Real `$`, a budget governor, and model routing (cheap model for cheap
turns). A *policy* over the same assembler. Ships only when it *changes
a routing decision*. Also: System-blob decomposition (split into
guardrails `Immutable`, tool-schemas `Protected`, env-preamble `Normal`).

---

## Dependency graph

```
1 (rescue_weight config)        — independent, do first (smallest)
2 (rescue UI)                   — independent of 1 and 3
3 (assemble_context → build_req)— keystone; 5 and 9 depend on it
  ├── 5 (per-kind policy)       — needs assembler as chokepoint
  └── 9 (cost/routing layer)    — policy over the assembler
4 (zoom announcements)          — incremental, anytime
6 (generative compaction)       — independent seam
7 (RAG / additive retrieval)    — builds on Slice-4 v1 embeddings
  └── 8 (cross-encoder reranker)— improves 7's precision
```