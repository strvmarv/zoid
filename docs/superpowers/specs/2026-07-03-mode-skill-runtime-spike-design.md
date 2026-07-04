# Mode / Skill Runtime Spike — Design

**Date:** 2026-07-03
**Status:** Approved design, ready for implementation plan
**Slice:** Foundation (Slice 0) + runtime spike — the first slice of the larger "mode/skill seam"

## Goal

Prove the riskiest assumption behind zoid's mode/skill direction: **can a small local
model (`glm-5.2:cloud`) actually drive a skill graph** — call an `invoke_skill` tool and
follow a skill body's instruction to invoke another skill — inside zoid's existing agent
turn loop? Ship the minimal foundation that makes that question answerable, and record the
answer as an explicit go/no-go gate for the rest of the direction.

## Why this slice first

The full vision (below) has three slices. We deliberately build the runtime spike first
because the single unknown that could *invalidate the entire vision* is behavioral, not
architectural: if a small model won't reliably drive `invoke_skill` + chaining, then a
"mode" can only ever be a static prompt overlay and "consume the Superpowers methodology"
is disconfirmed. A shipped Shift+Tab switch would tell us nothing about that risk. Spiking
the runtime retires the scary risk early (classic "spike the highest-risk unknown first").

The Shift+Tab quick-switch is cheap, orthogonal, and never wasted work, so it slots in as a
later slice with no loss.

## The larger picture (context — NOT built in this slice)

The user-facing feature is **modes**: a curated set of behaviors the user switches between
with **Shift+Tab**, mirroring the existing Alt+P provider/model quick-switch. Under the hood
a mode is an agent/skill (`AgentProfile`). The full direction decomposes into:

- **Slice 0 — Foundation:** an `AgentProfile` registry with an active pointer; the chat turn
  reads the *active profile* (system prompt + tool allow-list) instead of a hard-coded const.
- **Runtime spike (this slice, on top of Slice 0):** an `invoke_skill` tool + a skill *menu*
  in the active prompt + 2 hand-written built-in skills that chain A→B. Proves the engine
  runs and a small model can drive it.
- **Later — Source adapter / importer:** scan a directory of `SKILL.md` files (e.g. obra's
  Superpowers) into `AgentProfile`s; resolve bundled sibling files; the *promotion* step that
  marks selected skills as top-level modes.
- **Later — Mode quick-switch UX:** the Shift+Tab overlay (clone of the Alt+P provider-switch)
  cycling only the *promoted* modes; active-mode status line; persistence.
- **Later — Skill runtime hardening:** tool-name aliasing for "ghost" tools referenced by
  imported skills, subagent bridge, context-budget tuning.

### Vocabulary and invariants that bind the whole direction

- User-facing surface is **"mode"**; internal model is an **agent/skill** (`AgentProfile`).
  Internal ≠ surface. The Shift+Tab overlay is labeled "Modes", not "Agents".
- **Modes are curated, not the whole corpus.** A mode is a skill/agent *explicitly promoted*
  to mode status. Most skills never appear in the Shift+Tab cycle — they are substrate the
  runtime can `invoke_skill` into.
- **Promotion is declared zoid-side, never upstream.** Which skills are promoted lives in
  zoid config (a `[[mode]]` array in `~/.config/zoid/config.toml` or a `.zoid/modes.toml`),
  *never* as a flag in a `SKILL.md` frontmatter — zoid does not own those files and an
  upstream update would clobber the flag. (Applies to the later importer slice; recorded here
  so the foundation does not preclude it.)

## Architecture

Two layers of the prompt, not one:

- A **mode** is *ambient*: it sets the turn's **system prompt** and tool allow-list and
  persists across every turn until switched. That is Slice 0 — `chat_turn_config()` reads the
  **active `AgentProfile`** instead of the `SYSTEM_PROMPT` const.
- An **`invoke_skill`** call is *transient*: it is a **tool** whose *result* is the skill's
  body text, injected into the conversation as a `Message::tool` — exactly how Claude Code's
  Skill tool works. The model treats a returned skill body like a file it read.
- They **compose**: the ambient mode prompt carries a **skill menu** (`name: description`
  lines). The model calls `invoke_skill("…")` to pull a full body on demand; **chaining is
  just the model calling it again**. No overlay stack, no per-turn system-prompt surgery for
  skills — the menu lives in the system prompt, full bodies live in tool results and scroll
  with the conversation.

Consequence: the "runtime" is one tool plus the tool-call/tool-result loop that already
exists in `run_agent_turn`. The only genuinely new machinery in Slice 0 is "the active
profile drives the turn instead of a const."

```
┌─ Slice 0: Foundation ──────────────────────────────────┐
│ AgentProfileRegistry (in-memory): profiles + active ptr │
│ chat_turn_config(profile, menu) reads active profile's  │
│   system_prompt + tool allow-list (was: SYSTEM_PROMPT)  │
│ Active profile's system prompt gets the skill MENU       │
└─────────────────────────────────────────────────────────┘
┌─ Runtime spike (on top) ───────────────────────────────┐
│ SkillRegistry: 2 hand-written built-in skills (A & B)   │
│ invoke_skill(name) tool → returns skill body as a       │
│   Message::tool result                                  │
│ Skill A ("spike-plan") body ends by instructing         │
│   invoke_skill("spike-implement")  ← chaining proof     │
└─────────────────────────────────────────────────────────┘
```

## Components & files

Layering note: `zoid-tools` depends only on `zoid-provider`, **not** `zoid-core`. To avoid a
new `tools → core` crate edge, the **bin (`zoid` crate) is the composition root** — it already
depends on both crates and wires `tools: Arc::new(zoid_tools::registry())` at `main.rs:875`.
The `invoke_skill` tool is *implemented in the bin* against the public `zoid_tools::Tool`
trait, holding the skill registry. `TurnConfig` has no `tools` field; tools are passed to
`run_agent_turn` separately, and the subagent path already filters tools by an `AgentProfile`
allow-list (`subagent.rs:117`) — the foundation reuses that proven pattern at `spawn_turn`.

| Unit | File | Responsibility |
|---|---|---|
| `Skill`, `SkillRegistry` | **new** `crates/zoid-core/src/skill.rs` | Domain model: `Skill { name, description, body }`; `builtin()` returns the 2 spike skills; `get(name)`, `menu()`, `names()`. Lives in core beside `AgentProfile`. |
| `AgentProfileRegistry` | extend `crates/zoid-core/src/agent_profile.rs` | Holds profiles + active pointer: `builtin()`, `active()`, `by_name()`, `set_active(name)`. Built-in `"default"` profile = today's `SYSTEM_PROMPT` + all-tools allow-list (zero regression). |
| `chat_turn_config` refactor | `crates/zoid/src/agent.rs:44` | New signature `chat_turn_config(profile: &AgentProfile, skill_menu: &str)`. `system` = `profile.system_prompt` + rendered skill menu; preserves `cwd/branch/policy`. |
| tool filtering | `crates/zoid/src/main.rs` `spawn_turn` (~2395) | Filter `app.tools` by `active_profile.tools` allow-list before the turn — mirroring `subagent.rs:117`. Default profile allows all. |
| `InvokeSkillTool` | **new** `crates/zoid/src/invoke_skill.rs` | `impl zoid_tools::Tool`. `call(name)` → looks up body in the injected `Arc<SkillRegistry>`, returns it as the tool-result string; unknown name → error result listing `names()`. |
| wiring | `crates/zoid/src/main.rs:875` (App construction) | App gains `profiles: AgentProfileRegistry` + `skills: Arc<SkillRegistry>`; the `invoke_skill` tool is appended to the tools vec here. |

### Key signatures (contracts between units)

```rust
// zoid-core/src/skill.rs
pub struct Skill { pub name: String, pub description: String, pub body: String }
pub struct SkillRegistry { /* Vec<Skill> */ }
impl SkillRegistry {
    pub fn builtin() -> Self;                 // "spike-plan" (A) + "spike-implement" (B)
    pub fn get(&self, name: &str) -> Option<&Skill>;
    pub fn menu(&self) -> String;             // "- spike-plan: …\n- spike-implement: …"
    pub fn names(&self) -> Vec<String>;
}

// zoid-core/src/agent_profile.rs (extend)
pub struct AgentProfileRegistry { /* Vec<AgentProfile>, active: usize */ }
impl AgentProfileRegistry {
    pub fn builtin() -> Self;                 // ["default"] — today's SYSTEM_PROMPT + all tools
    pub fn active(&self) -> &AgentProfile;    // never None; falls back to "default"
    pub fn by_name(&self, name: &str) -> Option<&AgentProfile>;
    pub fn set_active(&mut self, name: &str) -> bool;
}

// zoid/src/agent.rs
pub fn chat_turn_config(profile: &AgentProfile, skill_menu: &str) -> TurnConfig;
```

### The 2 built-in spike skills

- **`spike-plan`** (A) — a short body describing a trivial task, ending with an explicit
  instruction: *"Now call `invoke_skill(\"spike-implement\")` to continue."*
- **`spike-implement`** (B) — a trivial terminal skill: *"Write the one-line file the plan
  described,"* referencing only `write_file`.

Both bodies reference **only tools that exist in zoid** (`invoke_skill`, `write_file`) so the
spike measures "can the model drive the graph," not "can it cope with ghost tools." The A→B
handoff is the entire chaining proof.

## Data flow (one live turn)

User is in the default mode and types *"plan and implement the spike task."*

```
1. spawn_turn (main.rs)
   ├─ profile = app.profiles.active()              // "default"
   ├─ menu   = app.skills.menu()                   // "- spike-plan: …\n- spike-implement: …"
   ├─ cfg    = chat_turn_config(profile, &menu)
   │            system = profile.system_prompt
   │                   + "\n\n## Available skills — call invoke_skill(name):\n" + menu
   └─ tools  = app.tools filtered by profile.tools  // default ⇒ all + invoke_skill

2. run_agent_turn → provider.stream(req)           // req.system carries the menu
   Model → ToolCall{ name:"invoke_skill", args:{name:"spike-plan"} }

3. InvokeSkillTool.call({name:"spike-plan"})
   └─ skills.get("spike-plan").body → returned as the tool RESULT (Message::tool)

4. Round 2: model has spike-plan's body in context; its body says to invoke spike-implement.
   Model → ToolCall{ name:"invoke_skill", args:{name:"spike-implement"} }
        ↑ CHAINING PROOF — model followed one skill's instruction to invoke another

5. InvokeSkillTool.call({name:"spike-implement"}) → body returned as tool result.

6. Round 3: model follows spike-implement's body → ToolCall{ name:"write_file", … }
   (real on-disk write) → ToolResult → final text → TurnComplete.
```

Notes:

- **No new loop machinery.** `invoke_skill` rides the existing tool-call/tool-result cycle.
  A skill body is text arriving as a `Message::tool`.
- **The menu is the only system-prompt change.** The model can't invoke what it can't see, so
  the menu (names + descriptions) must be in `system`. Full bodies never sit in the system
  prompt — they arrive on demand and scroll with the conversation (the context-cost win: pay
  for a body only when it is pulled).
- **Idempotency:** invoking the same skill twice just returns the body twice — no state, no
  guard needed.

## Error handling & degradation

| Failure | Behavior |
|---|---|
| Unknown skill name (`invoke_skill("nope")`) | Tool returns an **error result** (not a crash): `"unknown skill 'nope'. Available: spike-plan, spike-implement."` Model self-corrects next round. |
| Model never calls `invoke_skill` | Turn completes as a plain chat answer. No error — this is the **go/no-go FAIL signal** (§Testing). |
| Runaway chaining (A→B→A→…) | The existing `MAX_TOOL_ITERATIONS = 50` leash (`agent.rs`) force-ends the turn. invoke_skill calls count as tool rounds; no new guard. |
| Empty/garbled args | Tool validates `args.name` is a non-empty string; missing → error result listing the menu. |
| Profile/registry misconfig (active name not found) | `AgentProfileRegistry::active()` falls back to the `"default"` built-in — the app is never left with no active profile. |

Principle: **every failure returns a tool result, never an `Err` that aborts the turn** —
mirroring the existing provider convention (`ProviderEvent::Error` over `Err` where possible,
`lib.rs:114`). For a spike whose point is observing model behavior, keeping the loop alive
through errors is essential.

## Testing & the go/no-go protocol

Two tiers, because this slice's real question is behavioral and cannot be unit-tested.

### Tier 1 — deterministic wiring tests (CI, `FakeProvider`, no network)

- `SkillRegistry::builtin()` contains both skills; `get()` hits and misses; `menu()` renders
  `name: description` lines for every skill; `names()` is complete.
- `chat_turn_config(profile, menu)` embeds the menu in `system` and preserves
  `cwd/branch/policy`; the `"default"` profile's `system` still starts with the current
  `SYSTEM_PROMPT` (regression guard).
- `InvokeSkillTool.call` returns the exact body for a known name; returns the
  error-with-menu string for an unknown name; rejects empty/missing `name`.
- **Chaining wiring:** a `FakeProvider` scripted to emit `ToolCall(invoke_skill "spike-plan")`
  causes the loop to append `spike-plan`'s body as a `Message::tool` and re-invoke the
  provider. (Proves the plumbing carries a body back into the loop — not that a real model
  chooses to chain.)
- Tool allow-list filtering at `spawn_turn`: a restricted profile drops disallowed tools; the
  default profile keeps all tools plus `invoke_skill`.

### Tier 2 — the real-model go/no-go smoke (manual, documented, `glm-5.2:cloud`)

> Fresh session → default mode → prompt: *"Plan and implement the spike task."*
>
> - **PASS** = the model calls `invoke_skill("spike-plan")`, then (following that body)
>   `invoke_skill("spike-implement")`, then `write_file` — the full A→B→work chain, unattended.
> - **PARTIAL** = invokes once but does not chain.
> - **FAIL** = never invokes; answers inline.

### Decision gate (recorded outcome of this slice)

- **PASS** → build the importer and Shift+Tab slices with confidence.
- **PARTIAL** → the runtime needs prompt-engineering / menu-tuning before more investment.
- **FAIL** → the "consume the methodology" vision is disconfirmed on small local models; fall
  back to modes-as-prompt-overlays (still useful, but a different product).

Capturing this branch now is the whole reason we spiked first.

## Out of scope for this slice

- Shift+Tab quick-switch overlay and the promoted-mode tier.
- `SKILL.md` importer / source adapter and the promotion config layer.
- Tool-name aliasing for "ghost" tools referenced by imported skills.
- Subagent bridge and context-budget tuning.
- Any change to the `Mode` UI enum (`state.rs:5`, Chat/Build) — modes-as-agents are a separate
  concept from that rendering enum and do not touch it in this slice.
