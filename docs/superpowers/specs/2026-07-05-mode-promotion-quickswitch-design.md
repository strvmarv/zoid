# zoid — Mode Promotion + Quick-Switch (Slice 3) · Design

**Date:** 2026-07-05
**Status:** Approved design, gilfoyle-reviewed (C1/I1–I3/M1–M4 folded in), ready for implementation plan
**Slice:** 3 of the mode/skill seam — the on-disk **mode runtime** (discovery, scoping, **ambient prompt overlay**, switch, error-safety, persistence, hot-reload). Network-free.
**Author:** strvmarv (with Claude)

> **Spec set.** This continues the mode/skill direction:
> - **Slice 0 — runtime spike** → `2026-07-03-mode-skill-runtime-spike-design.md` (the two-layer prompt seam: an ambient `AgentProfile` drives the turn; `invoke_skill` returns a skill body as a tool result). **Merged** `c79a1ed`; smoke PASS on `glm-5.2:cloud`.
> - **Slice 2 — SKILL.md importer** → `2026-07-04-skill-md-importer-design.md` (scan `*/SKILL.md` → `Skill`s into the registry; import-only). **Merged** `6cc9a4d`.
> - **Slice 3 — this doc** (modes + Shift+Tab).
> - **Slice 4 — URL import wizard** → *follow-on spec, not built here* (§12.3).

---

## 1. Overview

A **mode** is a **named agent that owns a scoped set of skills**. The user switches between modes with **Shift+Tab**; the active mode determines which skills the model can see and pull. `Chat` is the default, non-removable mode — a bare coding agent that owns **no scoped skills**, so switching to a heavy mode like Superpowers never leaks its methodology skills back into Chat. (Chat's menu is still the *global* tier — the built-ins plus any `[skills] source_dirs` imports — so "owns no scoped skills" means uncluttered *by modes*, not necessarily empty.)

This slice replaces the current UI `Mode` enum (`Chat`/`Build`, where `Build` is a vestigial placeholder) with a real, extensible **mode registry** built from on-disk mode folders. Everything the user experiences as a "mode" is, internally, an `AgentProfile` plus a scoped skill set. No autonomous-loop or Build behavior is introduced — a "mode" is a behavior/skill scope, not a workflow engine.

The slice is deliberately **network-free and deterministic**: it reads only canonical on-disk files. The messy work of turning someone else's loosely-structured skill set into canonical files is the URL-import wizard's job (Slice 4, §12.3), which materializes files this runtime already understands and then calls `reload()`.

---

## 2. North star (governs every decision here)

**zoid is a thin, stable host; the ecosystem moves fast around it.** Modes and skills are **drop-in extensions** the user adds without waiting on a zoid release: drop a folder into `~/.config/zoid/modes/`, or (Slice 4) paste a URL. The core stays small and does not try to own the value the market (skills, model vendors) is producing.

Design rule for this slice: **natural extensibility first, seams over implementations.** Where a capability is not needed yet (a mode's own system-prompt/tools/model, the overlay picker, URL import), we build the *seam* — plumb and store the data, defer the behavior — rather than the full mechanism.

---

## 3. The minimum data contract (the hourglass waist)

External skill sets are heterogeneous — arbitrary authors, inconsistent shapes. The system copes with that via an **hourglass**: a **strict, tiny canonical contract** in the middle, LLM normalization at the import boundary (Slice 4) above it, and a deterministic runtime below it. This slice **owns and formalizes the contract** because the contract is the target the Slice-4 LLM maps onto — a vague contract makes that mapping unreliable; a crisp one makes it validatable.

**Canonical `SKILL.md`** (already enforced by `parse_skill_md`, `zoid-core/src/skill.rs`):
- **Required:** `---`-fenced frontmatter with a non-empty `name:` scalar.
- **Optional:** `description:` scalar (single-line; one matching pair of surrounding quotes stripped).
- **Body:** everything after the first closing `---`, verbatim.

**Canonical `mode.md`** (new; parsed by the *same* `parse_skill_md` — a `mode.md` is structurally a `SKILL.md` playing a different role):
- **Required:** frontmatter `name:` — the mode's canonical display name.
- **Optional:** `description:` — one-line summary shown in the switch UI.
- **Optional, HONORED (§5, §8):** the **body** → the mode's **ambient system-prompt overlay**. For a `Ready` mode, `system_prompt = <base profile's prompt> + "\n\n" + body` (empty body ⇒ just the base, i.e. behaves like Chat). The base is the bin's `default_profile()` — the composition happens bin-side because `SYSTEM_PROMPT` is bin-only (§5). This overlay is what makes switching to a mode *do* something — e.g. Superpowers' `mode.md` body carries the `using-superpowers` loader text so the model knows to drive the skill menu.
- **Optional, SEAMED (§12.1):** `tools:` allow-list and `model:` override. Parsed and stored on the mode's `AgentProfile`; **not applied to the turn in this slice.**

**A mode folder** = a directory containing a `mode.md` plus zero or more `*/SKILL.md` subfolders (its scoped skills). Anything that does not meet this contract is either skipped-with-warning (a bad skill) or loaded as `Broken` (a bad mode, §9) — never trusted, never fatal.

> **The runtime never normalizes.** Loose/legacy shapes are Slice 4's problem; below the waist we only ever read canonical files.

---

## 4. Vocabulary & invariants

- **User-facing surface is "mode"; internal model is an `AgentProfile` + a scoped `SkillRegistry`.** The Shift+Tab UI says "mode," never "agent."
- **`Chat` is the floor.** It always exists, is always index 0, cannot be removed, owns no skills, and is the fallback whenever the active mode vanishes on reload.
- **Two skill tiers.** *Global* skills (existing `[skills] source_dirs` + convention `skills/` dirs + the built-ins) are visible in **every** mode. *Mode-scoped* skills are visible **only** when their mode is active.
- **Effective menu = globals + active-mode skills**, with a **mode-scoped skill shadowing** a global of the same name while that mode is active (local-overrides-global).
- **Modes are discovered on disk, declared zoid-side.** Which skills belong to which mode is a property of the mode folder the user controls — never a flag in an upstream `SKILL.md` frontmatter (zoid does not own those files; an upstream update must not be able to reshape the user's modes).

---

## 5. Architecture

Two registries, one active pointer:

- **Global `SkillRegistry`** (`zoid-core/src/skill.rs`, unchanged) — built-ins + globally-imported skills. This is today's registry; it becomes the *global tier*.
- **New `ModeRegistry`** (`zoid-core/src/mode.rs`) — an ordered `Vec<Mode>` with an `active: usize`. `Mode` is a total sum type:

```rust
// zoid-core/src/mode.rs
pub enum Mode {
    Ready { profile: AgentProfile, skills: SkillRegistry },
    Broken { name: String, error: String },
}
impl Mode {
    pub fn name(&self) -> &str;            // Ready→profile.name, Broken→name
    pub fn description(&self) -> &str;     // Ready→profile.description, Broken→"" (or the error headline)
    pub fn is_broken(&self) -> bool;
}

pub struct ModeRegistry { modes: Vec<Mode>, active: usize }
impl ModeRegistry {
    pub fn new(modes: Vec<Mode>) -> Self;  // caller guarantees modes[0] == Chat; active = 0
    pub fn active(&self) -> &Mode;         // never panics
    pub fn active_name(&self) -> &str;
    pub fn cycle_next(&mut self);          // (active + 1) % len — wraps; Shift+Tab
    pub fn set_active(&mut self, name: &str) -> bool;  // :mode <name> / restore
    pub fn names(&self) -> Vec<String>;
}
```

`ModeRegistry` **subsumes the active-pointer role** that Slice 0 gave `AgentProfileRegistry`. The `AgentProfile` **struct** is reused verbatim as the identity carried inside `Mode::Ready`. `AgentProfileRegistry` itself (`agent_profile.rs`) can be **fully deleted** — verified: the Chat-delegation subagent path constructs `AgentProfile::builtin()` directly (`main.rs:3010`, `subagent.rs:236`) and never touches the registry, and the app path's only use is the active pointer that `ModeRegistry` now owns. Do not preserve it "just in case."

**Menu & invoke_skill scoping.** A pure helper builds the effective view for the active mode:

```rust
// zoid-core/src/mode.rs — pure, unit-tested
pub fn effective_skills(global: &SkillRegistry, active: &Mode) -> SkillRegistry;
//   Ready  → seed mode skills, then push_unique(each global)  ⇒ mode shadows global (see note)
//   Broken → global only
```

> **Shadowing detail.** "Mode shadows global" means the *mode's* copy wins. Since `push_unique` is first-wins, the builder seeds the mode's skills **first**, then folds in globals — so a same-named global is rejected and the mode copy stays. `Chat` (owns no skills) ⇒ globals only.

The turn already reads `app.profiles.active()` + `app.skills.menu()` at `main.rs:3055` and builds `chat_turn_config_with(profile, menu)`. This slice changes that call site to read the **active mode** and build a **per-turn snapshot** of the effective view (details below): the turn's `system` = the active mode's `AgentProfile.system_prompt` **plus** the effective scoped menu; **`invoke_skill` resolves against the same snapshot.** So one Shift+Tab swaps *both* prompt layers at once — the ambient overlay and the visible/pullable skills — and switching back to `Chat` restores the pure `SYSTEM_PROMPT` with globals only. This reuses Slice 0's `chat_turn_config_with` unchanged; the only new input is "which mode is active."

**Where the overlay is composed (crate boundary — decided).** `SYSTEM_PROMPT` is a **bin-only** constant (`agent.rs:27`); `zoid-core` cannot see it. Therefore **the bin composes the overlay**, not core. `zoid-core`'s `Mode`/`ModeRegistry`/`effective_skills` are pure value-holders: `Mode::chat(base: AgentProfile)` and the mode-importer both take a **base `AgentProfile` as a parameter** and store `system_prompt = base.system_prompt + "\n\n" + mode.md body` (empty body ⇒ just the base). The bin seeds the registry from `default_profile()` (`agent.rs:36`) — exactly as it already seeds `AgentProfileRegistry` at `main.rs:1244`. Consequence: the overlay-composition assertion is a **bin test**, not a core test (§13).

**The `invoke_skill` snapshot (decided — resolves risks 1 & 3).** Slice 0 wired `InvokeSkillTool { skills: Arc<SkillRegistry> }` — an immutable snapshot resolved synchronously (`invoke_skill.rs:17`), and the tools vec is built **once** at App construction (`main.rs:1243`) then cloned into each `tokio::spawn`ed turn (`main.rs:3049`). The menu, however, is **already** recomputed per turn (`main.rs:3056`). We make the resolver consistent with the menu **by construction**: `spawn_turn` builds the effective `SkillRegistry` for the active mode **once at turn start** and binds a **fresh `InvokeSkillTool` (Arc of that snapshot)** for that turn. `InvokeSkillTool`'s field stays `Arc<SkillRegistry>` (no interior mutability, no shared-mutable state, no `RwLock`/`ArcSwap`). A mid-turn mode switch or `:mode reload` **cannot** touch an in-flight turn — it takes effect on the **next** turn. (Cosmetic consequence, stated deliberately: the mode chip may lead the resolver by at most one turn if switched mid-stream; acceptable.)

**The Superpowers split (worked example).** `using-superpowers` — the loader skill whose whole job is *"reach for skills"* — is authored as the **`mode.md` body** (ambient, always-on while the mode is active), **not** a menu entry. The other ~13 methodology skills (`brainstorming`, `writing-plans`, `test-driven-development`, …) are the **scoped menu**, pulled transiently via `invoke_skill`. The two prompt layers from Slice 0 map exactly: ambient loader + transient bodies.

---

## 6. Components & files

| Unit | File | Responsibility |
|---|---|---|
| `Mode`, `ModeRegistry`, `effective_skills` | **new** `crates/zoid-core/src/mode.rs` | Pure value-holders + menu scoping. `Mode::chat(base: AgentProfile)` **takes the base profile as a param** (core can't see the bin's `SYSTEM_PROMPT`, §5); `effective_skills` builds the scoped view. No FS/network, no `SYSTEM_PROMPT`. |
| `ModesConfig` / `PartialModes` | `crates/zoid-core/src/config.rs` | `[modes] source_dirs = [...]` mirroring `SkillsConfig` (`config.rs:8`); union-merge across layers (`config.rs:234` pattern). |
| Mode importer + `reload` | **new** `crates/zoid/src/mode_import.rs` | Effectful: resolve mode dirs (convention + config), scan each subfolder for `mode.md` + sibling `*/SKILL.md`, compose each `Ready` mode's `system_prompt` from the passed base profile + `mode.md` body (§5), build `Vec<Mode>` (Chat first) → `build_mode_registry(base, dirs) -> ModeRegistry`. Mirrors `skill_import.rs`. |
| App wiring + per-turn snapshot | `crates/zoid/src/main.rs` (~1014, 1243, 1244, 3049, 3056) | `App` gains `modes: ModeRegistry` (replacing the `profiles` field); global `skills` stays. `spawn_turn` builds the effective `SkillRegistry` snapshot for `modes.active()` **once at turn start** and binds a fresh `InvokeSkillTool` to it (§5), instead of the once-at-construction tools vec (`main.rs:1243`). |
| Switch action + payload type | `crates/zoid-tui/src/{state,route,command,palette}.rs` | Delete `enum Mode`/`toggle_mode`. **`Command::SwitchMode(Mode)` → `SwitchMode(String)` + a new `CycleMode`** (`command.rs:8`); `parse_command(":mode <name>")` carries a `String`; `all_items(mode: Mode, …)` (`palette.rs:56`) takes the mirrored active-mode name. `BackTab` → `CycleMode`; `:mode reload`; palette "Switch mode ▸" group (replaces "Switch to Build"). |
| `ShellState` mode mirror | `crates/zoid-tui/src/state.rs` | The pure renderer can't reach the bin's `ModeRegistry`, so mirror it like `provider`/`model`/`companion_on` (`state.rs:175-196`): add `active_mode: String`, `active_mode_broken: bool`, and `mode_names: Vec<String>` (for the palette group). The bin pushes these on switch/reload. |
| Mode chip + broken card | `crates/zoid-tui/src/render.rs` | Chip reads the mirrored `active_mode` (⚠ when `active_mode_broken`); delete `render_build_placeholder` + Chat/Build render branches; a `Broken`-mode conversation area renders the crafted error card (§9). |
| Per-session active mode + **migration** | `crates/zoid-core/src/store.rs` (+ `session.rs` actor, `sessions.rs` projection) | **All SQL lives in `store.rs`.** Add the schema migration (§11), an `active_mode`-aware write, and a `get_active_mode(id)` read. `session.rs` threads a `set_active_mode` message; `sessions.rs` is untouched unless the column joins `SessionRow`. |

---

## 7. Discovery & configuration

Symmetric with the Slice-2 skill importer.

- **Global skills (unchanged):** `[skills] source_dirs` + convention `<cfg>/skills`, `<cwd>/.zoid/skills` + built-ins.
- **Modes:** convention dirs `~/.config/zoid/modes/` and `./.zoid/modes/`, **plus** `[modes] source_dirs = [...]` (tilde-expanded like skills). Each **immediate subfolder** of a modes-dir is a candidate mode:
  - has a readable `mode.md` that parses → `Mode::Ready`; its sibling `*/SKILL.md` are imported as that mode's scoped skills (skill-level errors skip-and-warn, Slice-2 behavior).
  - has a `mode.md` that is missing/unreadable/malformed → `Mode::Broken { name: <folder name>, error }` (§9).
  - has no `mode.md` → **ignored** (it is not a mode; may just be a skills dir).
- **Ordering:** `Chat` is always constructed first (index 0). Discovered modes follow in a stable order (convention dirs, then config `source_dirs`, each dir's entries sorted by folder name) so the Shift+Tab cycle is deterministic and snapshot-testable.
- **Name collisions across mode folders:** first-wins by mode name (same rule as skills), later duplicates skipped-with-warning.

```
~/.config/zoid/modes/
  superpowers/                 ← folder name = fallback label only
    mode.md                    ← name: Superpowers   (canonical)
    brainstorming/SKILL.md     ← scope = Mode("Superpowers")
    writing-plans/SKILL.md
```

```toml
# global — every mode sees these
[skills]
source_dirs = ["~/.config/zoid/skills"]

# extra mode roots beyond the two convention dirs
[modes]
source_dirs = ["~/dev/zoid-modes"]
```

---

## 8. Switch UX & enum retirement

- **`Shift+Tab` (`BackTab`)** — already routed to `Action::SwitchMode` (`route.rs:174`); rebind to `ModeRegistry::cycle_next()` (wraps, discovery order, Chat first). **No overlay picker** this slice (deferred, §12.2). One keypress = next mode.
- **A switch swaps the ambient prompt, too.** Because a `Ready` mode carries its `mode.md` body as a system-prompt overlay (§5), one Shift+Tab changes both the visible/pullable skills **and** the ambient instructions in a single keypress; returning to `Chat` restores the pure `SYSTEM_PROMPT`. No separate action.
- **Mode chip — bottom-left status bar.** Shows the active mode's name; a `Broken` mode renders with a `⚠` prefix. (Replaces the `Chat`/`Build` chip branch, `render.rs:277`.)
- **`:mode <name>`** — direct switch (`set_active`); **`:mode reload`** — hot-reload (§10). Replaces `:chat`/`:build` (`command.rs:33`).
- **Palette "Switch mode ▸"** group — lists modes (Chat first), each row `name — description` (⚠ for broken); selecting one switches. Replaces the single "Switch to Build" item (`palette.rs:59`).
- **Retire `enum Mode { Chat, Build }`** (`state.rs:6`): delete the enum, `toggle_mode` (`state.rs:372`), `render_build_placeholder` (`render.rs:173`), the Chat/Build match arms in `render.rs`/`route.rs`/`command.rs`/`palette.rs`, and the `Esc`-from-Build hatch (`route.rs:180` — nothing to escape now). Rendering becomes single-surface; `Chat` is simply mode 0. **Not purely mechanical:** because modes now have arbitrary on-disk names, the `Command`/`Action` payload changes from the typed `Mode` enum to a `String` (`SwitchMode(String)` + `CycleMode`, §6) — a data-contract change across the pure boundary, so the mirrored `ShellState` fields (§6) and the palette's `all_items` signature move with it.

---

## 9. Error handling & graceful degradation

The loader is **total** — it never panics and never silently drops a mode. Two granularities:

| Failure | Behavior |
|---|---|
| **Mode-level** — no/unreadable/malformed `mode.md`, unreadable folder | `Mode::Broken { name: <folder name>, error: <reason+path> }`. Still occupies a cycle slot (visible), chip shows `⚠ <name>`. **Switching to it activates neither skills nor overlay** — the turn `system` stays `SYSTEM_PROMPT` and the conversation area renders a **crafted error card** (mode name, folder path, reason, line if available, and a hint to fix + `:mode reload`). Base agent + global skills keep working; the effective menu falls back to globals only. |
| **Skill-level** — a `SKILL.md` inside a healthy mode fails to parse/read | Skip-and-warn that one skill (existing `import_skills` behavior); the mode still loads `Ready` with its good skills and carries a **warning count** surfaced in the switch UI / chip tooltip. |
| **Active mode missing after reload** (folder deleted) | Fall back to `Chat` (index 0); no error state left dangling. |
| **Duplicate mode name** | First-wins; later folder skipped-with-warning. |

Principle (inherited from Slices 0/2): a bad input **produces a value, never aborts** — startup and every reload always yield a usable `ModeRegistry` with at least `Chat`.

---

## 10. Hot reload (no restart)

`ModeRegistry` is **reloadable at runtime**. `:mode reload` (and the palette "Reload modes" action) re-runs discovery, rebuilds the registry, and **preserves the active mode by name** (`set_active(previous_name)`; falls back to `Chat` if it no longer exists). A `Broken` mode you just fixed becomes `Ready`; a folder you deleted drops out; a folder you added appears — all without a restart.

This is the single seam both on-ramps use: dropping a folder into a convention dir **and** the Slice-4 URL importer both end in "write canonical files, then `reload()`." The importer never touches registry internals.

---

## 11. Persistence — active mode is per-session

The active mode is **session state**. Sessions are already keyed per-repo (`root_path`) and auto-loaded most-recent-first for the current repo, so storing the active mode on the session yields **per-repo stickiness for free**: resume a repo → land back in the mode you left.

- **Storage:** the `sessions` table gains a nullable **`active_mode TEXT`** column. Written on every switch; read on resume and passed to `ModeRegistry::set_active` (falling back to `Chat` if the stored mode no longer resolves — e.g. its folder was removed).
- **Default:** a new session starts in `Chat`.
- **Migration (this is the codebase's FIRST schema migration — name the mechanism).** The schema is created with `CREATE TABLE IF NOT EXISTS sessions (…)` (`store.rs:42`) and there is **no migration framework, no `user_version`, no `ALTER TABLE`** anywhere in `store.rs`. For an existing DB the table already exists, so adding the column to the `CREATE TABLE` body is a **silent no-op** — the column never appears and the first `SELECT/INSERT active_mode` throws at runtime. SQLite has no `ADD COLUMN IF NOT EXISTS`, so on `EventStore::open` we **probe `pragma table_info(sessions)`** (or catch the duplicate-column error) and, if absent, run `ALTER TABLE sessions ADD COLUMN active_mode TEXT`. Idempotent, additive, non-destructive; existing rows read `NULL ⇒ Chat`. This establishes the pattern every future column follows.
- **Read path:** a dedicated `get_active_mode(session_id) -> Option<String>` query (keeps `SessionRow`/`session_list` untouched — less churn than threading the column through the projection).
- **Restore onto a `Broken` mode.** `set_active` matches a `Broken` slot (it occupies a named cycle position, §9), so resuming a session whose stored mode is now broken lands directly on that mode's **error card** — intended ("you left it broken; here's why"). Only a *vanished* mode falls back to `Chat`.

---

## 12. Seams & deferred work

### 12.1 A mode's own identity — split
The **system-prompt overlay is HONORED this slice** (§3, §5, §8): it is the mechanism that makes a mode functional, it reuses Slice 0's `chat_turn_config_with` directly, and it is required for the Superpowers case. What remains **SEAMED** (parsed + stored on the `AgentProfile`, not yet applied) is the mode's **`tools` allow-list** and **`model` override** — these need real per-mode tool filtering at `spawn_turn` (à la `subagent.rs:117`) and model routing, which aren't needed for the near-term modes. Wiring point for the deferred half: `main.rs:3055`.

### 12.2 Overlay picker (deferred)
Shift+Tab cycles blindly this slice. A `Alt+P`-style overlay picker (arrowable list, descriptions, skill counts) is added "if it earns its place" once modes outnumber a comfortable cycle.

### 12.3 URL import wizard — **Slice 4, separate spec**
Paste a URL (e.g. `github.com/obra/superpowers/tree/main/skills`) → the agent fetches & scans it → proposes a **mapping** onto the §3 contract (not just a file list): inferred mode name, the `SKILL.md`s found, generated descriptions where missing, files skipped — → the user approves the **mapping** → validate against the contract → **materialize canonical files** into a mode folder + generate `mode.md` → `reload()` → mode is live, no restart. The LLM leaning lives entirely here, at the boundary, under human approval. Open questions for that spec: GitHub tree vs. contents/raw-URL resolution, private-repo auth, non-GitHub sources, re-import/update semantics.

---

## 13. Testing

**Pure (`zoid-core`, no FS/network):**
- `effective_skills`: `Chat` ⇒ globals only; `Ready` mode ⇒ globals + mode skills; **mode shadows global** of the same name; `Broken` ⇒ globals only.
- `ModeRegistry`: `cycle_next` wraps in order with Chat first; `set_active` hit/miss; `active()`/`names()` on a mix of Ready + Broken; empty-discovery ⇒ just `Chat`.
- `mode.md` parsing via `parse_skill_md`: name→canonical, missing name ⇒ Broken input, description optional.
- **Overlay composition on an arbitrary base (pure — no `SYSTEM_PROMPT`):** `Mode::chat(base)` ⇒ `system_prompt == base.system_prompt`; a `Ready` mode with body `b` built on `base` ⇒ `== base.system_prompt + "\n\n" + b`; empty body ⇒ `== base.system_prompt`.

**Effectful (`zoid` bin, temp dirs):**
- `mode_import`: a folder with `mode.md` + skills ⇒ `Ready` with scoped skills; malformed `mode.md` ⇒ `Broken` named by folder; no `mode.md` ⇒ ignored; a bad `SKILL.md` inside a good mode ⇒ mode `Ready`, warning counted; missing dir ⇒ skipped, never panics.
- **Overlay + turn wiring (uses real `SYSTEM_PROMPT`/`default_profile()`):** the turn `system` under a mode **contains** the overlay and under `Chat` **does not** (the "drops on switch back" invariant); the `"## Available skills…"` menu header lands **after** the overlay text (guards against a future `chat_turn_config_with` reorder, `agent.rs:64`).
- **Per-turn snapshot:** a scoped skill resolvable via `invoke_skill` while its mode is active is **unresolvable** after switching away (proves the snapshot is bound per turn, §5).
- Seamed fields (`tools`, `model`) captured onto the `AgentProfile` but asserted **unused** by the turn config this slice.
- `reload` preserves active-by-name; falls back to `Chat` when the active mode disappears.
- **Migration + persistence:** opening an **old-shape DB** (no `active_mode` column) runs the `ALTER TABLE` and resumes into `Chat` (no throw); switch writes `active_mode`; resume restores it; stored-but-vanished mode ⇒ `Chat`; stored-but-`Broken` mode ⇒ lands on the error card (§11).

**TUI (`TestBackend`/`insta` snapshots):**
- Shift+Tab cycle order/wrap; mode chip (Ready name; ⚠ broken); the crafted **broken-mode error card**; palette "Switch mode ▸" group. Each snapshot asserts the Chat/Build chrome is gone (single-surface render).

---

## 14. Out of scope

- Honoring the mode's `tools`/`model` fields (§12.1 — the *overlay* IS honored this slice), the overlay picker (§12.2), and the entire URL import wizard (§12.3, Slice 4).
- Any autonomous-loop / Build-mode behavior — a mode is a skill scope, not a workflow engine.
- Changes to the Chat-delegation subagent path or `invoke_skill`'s chaining semantics (unchanged from Slice 0 beyond the scoping of *which* skills resolve).
- Network, GitHub, or auth of any kind.

---

## 15. Risks

1. **`invoke_skill` scope — DECIDED (not open), see §5.** The resolver must match the active mode's effective skills, which change at runtime. Resolution: **per-turn snapshot** — `spawn_turn` binds a fresh `InvokeSkillTool(Arc<snapshot>)` at turn start, consistent with the already-per-turn menu. `InvokeSkillTool` keeps its immutable `Arc<SkillRegistry>` field; no shared-mutable state, no lock. Test: a scoped skill is unresolvable after switching away.
2. **Reload/switch vs. a live turn — DISSOLVED by the snapshot (§5).** Because each turn owns its snapshot, a mid-turn `reload()`/switch cannot mutate an in-flight turn; it applies on the next turn. The only residue is cosmetic (the chip may lead the resolver by one turn) and is stated as acceptable. No locking needed.
3. **Enum-retirement is a data-contract change, not just deletion (§6, §8).** Beyond the ~5 `zoid-tui` sites, the `Command`/`Action` payload moves from the `Mode` enum to `String` (`SwitchMode(String)` + `CycleMode`) and the renderer needs mirrored `ShellState` fields. Mitigate: land the payload/mirror change first, then repoint; rewrite `backtab_switches_mode`/palette tests to assert cycle behavior; snapshots catch chrome regressions.
4. **First-ever schema migration (§11).** `store.rs` has no migration machinery, and a `CREATE TABLE IF NOT EXISTS` edit is a silent no-op on existing DBs. Mitigate: probe `pragma table_info` then `ALTER TABLE … ADD COLUMN` on `open`; a test opens an old-shape DB and resumes into `Chat` without throwing. This sets the precedent for future columns — get it right once.
5. **`AgentProfile` reuse creates a subtle coupling.** `Mode::chat(base)` and the importer both depend on the bin passing `default_profile()`; if a future refactor changes `default_profile`'s prompt, every mode's overlay base shifts. Acceptable (it *should* track the base agent), but noted so it's a deliberate dependency, not an accident.
