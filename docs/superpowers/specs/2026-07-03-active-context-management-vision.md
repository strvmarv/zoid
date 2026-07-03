# Active Context Management — Vision (short + long term)

> **Status:** Vision / north-star. This document frames *what* active context
> management means for zoid and *why*, plus the short-term buildable slice and
> the long-term arc. It is above implementation: it justifies future phases but
> commits code only to the short-term slice in §4. Terminal step after approval
> is `writing-plans` for that slice — **not** the whole arc.
>
> **Date:** 2026-07-03 · **Supersedes framing in:** the P3 "context economy ⑤"
> notes (the machinery from P3 stands; this reframes its *purpose*).

---

## 1. Definition

> **Active context management is zoid continuously curating the model's working
> set of token-spending items — dropping what stopped mattering, compacting what
> is verbose, and (later) retrieving what is missing — so every request carries
> the *right* context, not merely the *recent* context. It acts on its own, but
> narrates every move where the user can see and undo it.**

"Active" is the load-bearing word. P3 is **passive**: it *observes* the window
(a token ledger, per-item heat, a churn timeline) and changes nothing about what
is actually sent. The vision is **active**: it *shapes* the window that feeds the
live request.

The working set is **not files**. It spans every token-spending kind — system,
messages, tool-results, files (and, later, retrieved knowledge and subagent
summaries). Curation is uniform through one assembler but governed by **per-kind
policy** (§3).

### 1.1 What we optimize for, in order

1. **Quality** — the window holds the *right* things, not merely the recent
   things. A coding agent lives or dies on this.
2. **Cost** — a right-sized window is cheaper. This is a *consequence* of
   quality, not an independent goal; we never minimize cost by dropping
   something the model needed.
3. **Trust / legibility** — the user can always see, and undo, what zoid did to
   their context. This is the presentation layer that makes silent automation
   acceptable.

### 1.2 The anti-decoration test

Every piece of this feature must pass one test: **does something *act* on it, or
does the user *decide* from it?** If neither, it is decoration and does not ship.
This test exists because the current ⑤ drawer failed it — see §2.

---

## 2. Why this reframe (honest critique of what exists)

P3 shipped a real, well-built context economy. It also, today, does nothing to
the live request. The critique that motivates this vision:

1. **Observability with zero agency.** `assemble_context(window, policy)` is a
   fully built, fully tested pure function that **nothing in production calls**.
   `build_request` never touches it. Heat, auto-evict, token ceiling, and the
   compaction threshold change zero bytes of what is actually sent. It is a
   gauge over a process it does not influence.
2. **The core signal measures activity, not value.** `heat_of(refs, recency)`:
   hot = referenced ≥3×, cold = untouched ≥3 turns. A file read once but central
   to the task reads **cold**; a file incidentally grepped 3× reads **hot**. This
   signal is too shallow to *safely* drive auto-eviction — which is precisely why
   deferring the live wiring was the correct call.
3. **The name over-promises.** "Economy" implies tokens + `$` + a value heatmap +
   a budget governor + model routing. What shipped is tokens + heat + a churn
   sparkline: a token/heat *inspector*, while the stated purpose was "balance
   value vs spend."

**What redeems it:** the architecture is right. Because context management is
expressed as **pure projections + a pure assembler over an event log**, it can
grow from passive-projection → active-selection **without a data-model change**,
and `heat` can gain a relevance term additively. The expensive, durable part is
already built. What remains is *connecting* it and *deepening the signal* — both
additive. This vision spends that architecture instead of re-earning it.

---

## 3. Architectural invariants (what never changes across phases)

- **Projections + a pure assembler over the event log.**
  `context_window(events) → ContextWindow`, then
  `assemble_context(window, policy) → ContextSelection`. This is the single
  chokepoint for "what gets sent." All phases route through it.
- **`heat` is a pure scoring function with an additive relevance seam.**
  `heat_of(refs, recency)` → `heat_of(refs, recency, relevance)`. The relevance
  term arrives without reshaping the data model or the assembler.
- **Every mutation is an event, not an in-place edit.** `ContextMutation { op }`
  (the variant already exists, used for pin/evict) is the audit trail that makes
  automation safe and undoable.
- **Two open seams — cut now, built only in their phase:**
  - **`Embedder` trait** (Provider-like): default local `bge-small`,
    `FakeEmbedder` for tests, feature-gated for binary size. Powers the relevance
    term (short-term) and RAG (long-term). Reuses total-recall's embedding stack.
  - **Additive retrieval:** a future `retrieve(query) → Vec<ContextItem>` that
    *adds* to the window. The assembler remains the merge point; subtract-only
    curation must not block additive retrieval later.

### 3.1 Principle — per-kind policy

Context is *all* token-spending kinds, managed through **one** assembler but with
**kind-specific rules**, because evictability, compactability, and
relevance-semantics differ by kind:

| Kind           | Evict?                     | Compact?                          | Relevance signal                                   |
| -------------- | -------------------------- | --------------------------------- | -------------------------------------------------- |
| **System**     | **Never** (structural)     | **Never** (structural)            | n/a — always in                                    |
| **Message**    | Old turns, carefully       | Summarize stale turns             | recency + goal-similarity                          |
| **ToolResult** | Yes, when superseded       | **Prime target** — huge, low-density, safe | low; mostly age / supersession            |
| **File**       | Relevance-driven           | Rarely (edit instead)             | **strongest** — semantic term dominates; content goes stale on edit |

Policy is a **lookup keyed by kind**, not a `match` that fails to compile on a new
variant — new kinds get a sane default (see §3.3).

### 3.2 Principle — protection is orthogonal to kind

`ItemKind` answers *provenance* ("where did this token come from"). **Protection**
answers *criticality* ("how sacred is it"). They are separate axes.

- `ContextItem` gains a `Protection` level: **`Normal | Protected | Immutable`.**
- The three levels:
  - **`Normal`** (default) — fully managed: evictable and compactable per the
    kind's policy and heat.
  - **`Protected`** — kept under normal operation; droppable/compactable only
    under genuine pressure (e.g. the token ceiling would otherwise be breached),
    and never as a routine curation pass. Intended for tool-schemas and
    user-pinned items.
  - **`Immutable`** — never compacted, never evicted, never counted against any
    ceiling. Enforced as a **structural skip in the assembler** — a type-level
    guarantee, not a heat threshold anyone can misconfigure.
- **Guardrails are `System` + `Protection::Immutable`** — *not* a new kind.
  Mixing criticality into the provenance enum would make it rot. Protection is
  reusable: a pinned spec `File` or a critical `Decision` can also be `Immutable`.
- Rationale for the hard rule: **losing a clause of a guardrail is a behavior
  change, not a token saving.** Safety text is never a compaction candidate.

### 3.3 Principle — `ItemKind` is a growth seam

RAG and Build will add kinds — `Retrieved`, `SubagentSummary`, `TaskState`,
`Decision`. The enum grows; the assembler and the zoom-rendering must **not**
change shape when it does. Concretely: per-kind policy is a defaulting lookup, and
rendering dispatches on kind with a fallback, so a new variant compiles and
behaves sanely without touching the core loop.

---

## 4. Short-term vision (the buildable next slice)

Smallest-useful-first, highest-leverage-first. Each step either lets the agent
*act* or lets the user *decide* — never "display more."

0. **Harden per-model `context_window` (precursor).** ACM's ceiling and
   compaction threshold are a *percent of the model's context window*, but
   `model_info().context_window` is currently a string-match stub
   (`contains("claude") → 200k, else 256k`). Replace it with a real per-model
   lookup (known models + a safe conservative default), keeping the existing
   `config.economy.context_ceiling` override. This is the **only** provider/model
   work ACM needs — the model *picker* and *pricing* stay out of scope (the
   picker has zero ACM coupling; pricing belongs to the long-term cost layer).
   Rationale: a wrong (over-high) window makes ACM under-compact and risk real
   overflow on small/local models; this makes the ceiling correct-by-construction.
1. **Compact tool-results.** The biggest, safest token reclaim in a coding
   session: replace a 900-line `grep`/test dump with a dense summary
   (`328 matches across 12 files → …`). Quality up (less noise), cost down,
   almost no risk of dropping something load-bearing. **Needs no embeddings** —
   an age/supersession signal is enough — so it ships value *before* the
   `Embedder` work lands, and de-risks the wire-into-`build_request` path on the
   safest possible mutation.
2. **Relevance-score files** via the `Embedder` seam (Tier-1 local embeddings):
   similarity to the *current goal/turn*, not just touch count. Fixes "central
   file reads cold." Files are where the semantic term pays off most.
3. **Wire the assembler into `build_request`.** The single change that flips
   passive → active. Guarded so it can never drop `Immutable`/pinned items and
   never shrinks below a floor.
4. **Announce every mutation through semantic zoom** (reuses the existing three
   zoom levels and the `ContextMutation` event):
   - **Summary** — folded into the turn digest (`context trimmed −4.1k`).
   - **Normal** — a one-line chip, exactly like a tool call
     (`⑤ compacted 3 tool-results → 1.2k`).
   - **Detail** — full breakdown: which items, why (heat / relevance / age),
     token delta, and how to undo.
5. **Drawer = live working set; transcript = history of how it got there.**
   Together they are the glass-box that makes silent-but-announced automation
   trustworthy.

**Autonomy contract for the short-term slice:** automatic but announced. The
agent acts on its own; every mutation is an event and is visible at the right
zoom altitude and undoable. Propose-and-confirm is reserved for the
destructive/expensive case only — Tier-2 generative compaction (§5).

---

## 5. Long-term vision (the arc — seams only for now)

- **Tier-2 generative compaction.** A frontier-LLM pass that summarizes
  stale-but-not-droppable context (e.g. long `Message` history). Because it
  *spends money and loses fidelity*, this is the **one place propose-and-confirm
  applies** — trust's concrete home. Never applies to `Immutable` items.
- **RAG / additive retrieval.** The `Embedder` index over repo + session history
  lets zoid *pull in* the file/symbol/prior-decision the agent never loaded — the
  quality ceiling, since the model cannot reason about code it never saw. Renders
  as a `ContextMutation` too (`⑤ retrieved auth.rs (relevant to current goal)`).
  Adds the `Retrieved` kind.
- **Decompose the System blob.** Split into guardrails (`Immutable`), tool-schemas
  (`Protected`), and a regenerable env-preamble (`Normal`, refreshed rather than
  preserved) — so the preamble can be refreshed while guardrails stay untouchable.
- **The cost / economy layer.** Real `$`, a budget governor, and **model routing**
  (cheap model for cheap turns). This is where "economy" finally earns its name;
  it is a *policy* over the same assembler. Ships only when it *changes a routing
  decision*, never as a number that is merely displayed.
- **Build orchestrator reuse.** The same `assemble_context` constructs each
  subagent's context. Chat's glass-box was the rehearsal; Build is the payoff —
  one assembler, two consumers.

---

## 6. Tiered cost model (how the signal deepens)

- **Tier 0 — pure code signals** (shipped): recency, refs, churn, token counts.
  Cheap and shallow. Enough for tool-result compaction and the ledger.
- **Tier 1 — local embeddings** (`bge-small`, near-free, runs locally): semantic
  *relevance* scoring + redundancy detection + retrieval-for-construction. The
  "nuance" layer that upgrades `heat_of(refs, recency)` →
  `heat_of(refs, recency, relevance)`.
- **Tier 2 — frontier LLM** (rare, threshold-triggered): generative compaction
  only. Expensive; gated by propose-and-confirm.

P4's tree-sitter symbol signals compound with Tier-1 embeddings for code context.
Heat/score stay pure functions so each term slots in additively.

---

## 7. Success criteria

The vision is working when:

- The assembler feeds `build_request` (the agent *acts* on context).
- `heat` includes a relevance term before relevance-driven eviction is trusted.
- Every mutation is visible at the correct zoom altitude and is undoable.
- `Immutable` items (guardrails) are provably never compacted or evicted —
  enforced structurally, covered by tests.
- Any `$`/routing surface *changes a decision*, not just a display.
- Tool-result compaction demonstrably reclaims tokens with no quality regression
  on a representative session.

---

## 8. Explicitly out of scope (for the short-term slice)

- RAG / additive retrieval (seam only).
- Tier-2 generative compaction (seam only; long-term).
- The `$` / budget-governor / model-routing layer (long-term).
- System-blob decomposition (long-term; System is one `Immutable` item for now).
- Manual user commands to execute/customize curation (deferred, per prior
  decision — automation + observability first).
- The model **picker** (interactive provider/model selection) and **pricing**
  fields — the picker has no ACM coupling; pricing belongs to the long-term cost
  layer. Only the narrow `context_window` hardening (§4 step 0) is in scope.
