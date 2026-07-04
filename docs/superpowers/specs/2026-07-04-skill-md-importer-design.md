# SKILL.md Source Adapter / Importer — Design

**Date:** 2026-07-04
**Status:** Approved design, ready for implementation plan
**Slice:** Source adapter / importer — the second slice of the "mode/skill seam", on
top of the merged Slice 0 foundation + runtime spike (`SkillRegistry`,
`AgentProfileRegistry`, `invoke_skill`; smoke = **PASS**, merged to main at `c79a1ed`).

## Goal

Populate zoid's `SkillRegistry` from disk. Scan directories of `SKILL.md` files
(e.g. obra's Superpowers corpus) into `Skill`s so the model can pull real skill
bodies via the already-shipped `invoke_skill` tool, with each imported skill's
bundled sibling files (prompt templates, scripts, references) reachable from the
returned body.

This slice makes the runtime spike's proven engine useful: instead of two
hand-written spike skills, `invoke_skill` can now drive a real, user-supplied
skill library.

## Why this slice (and why this shape)

Slice 0 proved a small model will drive `invoke_skill` + chaining. The only thing
standing between that proof and a usable feature is **a supply of real skills**.
This slice is that supply line — pure parsing + a filesystem walk, no behavioral
unknowns. Its entire risk is mechanical, so it is fully unit-testable with no
real-model smoke gate (unlike Slice 0).

The slice is deliberately kept to *import only*. Promotion (marking imported
skills as switchable **modes** / `AgentProfile`s) and the **Shift+Tab** switch UX
are the next slice. Building the supply of invocable skills first keeps this
slice small and lets the mode UX be designed against a registry that is already
populated from real files.

## Decisions locked in brainstorming

1. **An imported `SKILL.md` becomes a `Skill`, always.** It lands in the
   `SkillRegistry` and is invocable via `invoke_skill`. Promotion to an
   `AgentProfile` (mode) is a later projection over the same data — **not built
   here**. One parse path; every imported file is `invoke_skill`-able.
2. **Frontmatter is parsed by a hand-rolled minimal parser** in `zoid-core`
   (pure, no new dependency), reading the two scalar fields we need (`name`,
   `description`). Single-line scalar values only; YAML block scalars are out of
   scope (not used by the corpus). Matches zoid's dependency-minimal ethos
   (hand-rolled CLI, no clap).
3. **Sibling files are reached via a `base_dir` anchor, not body rewriting.**
   `Skill` carries `base_dir: Option<PathBuf>`; `invoke_skill` appends a single
   resolved line so the model reads siblings with the existing `read_file` tool
   using absolute paths. Siblings load lazily — only if the model reads them.
4. **Source directories:** convention dirs (`~/.config/zoid/skills/`,
   `./.zoid/skills/`) are auto-scanned when present, **plus** a
   `[skills] source_dirs = [...]` config key. Lets the user point at any path,
   including the Superpowers plugin cache.
5. **Promotion is deferred** to the Shift+Tab slice. The `AgentProfileRegistry`
   stays at just `default`.

## Architecture

The codebase has a consistent seam: **pure transformation in `zoid-core`,
effectful IO in the bin** (mirrors `config.rs`, which parses/merges TOML strings
while the bin reads the files). The importer honors it:

- **Pure, in core:** `parse_skill_md(&str) -> Result<ParsedSkill, String>` — no
  filesystem, no knowledge of where the text came from. Unit-testable with plain
  string inputs.
- **Effectful, in bin:** `import_skills(dirs) -> Vec<Skill>` — walks the
  filesystem, reads files, expands `~`/env, assigns each `Skill`'s absolute
  `base_dir`, and calls the pure parser per file.

```
┌─ zoid-core (pure) ─────────────────────────────────────┐
│ Skill { name, description, body, base_dir: Option<..> } │
│ parse_skill_md(text) -> Result<ParsedSkill, String>     │
│ SkillRegistry::push_unique(Skill) -> bool  (dedup)      │
│ Config.skills.source_dirs (+ merge union)               │
└─────────────────────────────────────────────────────────┘
┌─ zoid bin (IO) ────────────────────────────────────────┐
│ import_skills(dirs) : walk */SKILL.md, parse, set       │
│   base_dir=<abs skill dir>, skip+warn on error          │
│ convention dirs + ~/env expansion computed here         │
│ App wiring: builtin() then push_unique(each import)     │
│ invoke_skill: append base_dir anchor to returned body   │
└─────────────────────────────────────────────────────────┘
```

## Components & files

| Unit | File | Responsibility |
|---|---|---|
| `Skill.base_dir` | `crates/zoid-core/src/skill.rs` (extend) | `Skill` gains `base_dir: Option<PathBuf>`. The two `builtin()` spike skills set `base_dir: None`. |
| `parse_skill_md` | `crates/zoid-core/src/skill.rs` (extend) | Pure. Split on the `---` fences; from the frontmatter block read `name:` and `description:` scalar lines (strip a matching pair of surrounding `"`/`'`); `body` = text after the closing fence, verbatim. Returns `ParsedSkill { name, description, body }`. `Err` if there is no frontmatter block or `name` is missing/empty. |
| `SkillRegistry::push_unique` | `crates/zoid-core/src/skill.rs` (extend) | `push_unique(&mut self, Skill) -> bool`. Appends unless a skill with that `name` already exists; on collision returns `false` and does not mutate (first-wins). |
| `[skills] source_dirs` | `crates/zoid-core/src/config.rs` (extend) | `Config.skills: SkillsConfig { source_dirs: Vec<String> }` (default empty). `PartialConfig.skills: PartialSkills { source_dirs: Option<Vec<String>> }`. `merge` **unions** `source_dirs` across layers (append, then dedup preserving first-seen order). Not added to `Provenance` — `source_dirs` is a list, not a scalar shown in the config overlay this slice. |
| `import_skills` | **new** `crates/zoid/src/skill_import.rs` (bin) | `import_skills(dirs: &[PathBuf]) -> Vec<Skill>`. For each dir: read immediate child dirs, and for each containing a `SKILL.md`, read + `parse_skill_md` + set `base_dir = Some(<child dir, absolute>)`. Skip unreadable dirs and malformed files with a logged warning; never panic. Returns skills in scan order (dir order, then entry order). |
| anchor injection | `crates/zoid/src/invoke_skill.rs` (extend) | On a resolved skill, if `base_dir` is `Some(d)`, append `"\n\n---\nSkill files are in: {d}/"` to the returned body. `None` → body unchanged (regression-safe for builtins). |
| convention dirs + expansion | `crates/zoid/src/skill_import.rs` or `main.rs` (bin) | Compute `~/.config/zoid/skills` and `./.zoid/skills`; include each only if it exists. Expand `~` and env vars in configured `source_dirs`. Produce the final `Vec<PathBuf>` passed to `import_skills` (convention dirs first, then configured). |
| wiring | `crates/zoid/src/main.rs` (App construction) | Where the registry is built today (`SkillRegistry::builtin()`), instead: `let mut reg = SkillRegistry::builtin(); for s in import_skills(&dirs) { reg.push_unique(s); }`. Then `Arc::new(reg)` as before. |

### Key signatures (contracts between units)

```rust
// zoid-core/src/skill.rs (extend)
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub base_dir: Option<std::path::PathBuf>,   // NEW; None for builtins
}

pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Pure: extract name/description from `---` frontmatter and the body.
/// Err if there is no frontmatter block or `name` is missing/empty.
pub fn parse_skill_md(text: &str) -> Result<ParsedSkill, String>;

impl SkillRegistry {
    /// Append unless a skill with this name already exists (first-wins).
    pub fn push_unique(&mut self, skill: Skill) -> bool;
}

// zoid-core/src/config.rs (extend)
pub struct SkillsConfig { pub source_dirs: Vec<String> }        // on Config
pub struct PartialSkills { pub source_dirs: Option<Vec<String>> } // on PartialConfig
// merge(): union source_dirs across layers (append + dedup, first-seen order)

// zoid/src/skill_import.rs (new, bin)
pub fn import_skills(dirs: &[std::path::PathBuf]) -> Vec<zoid_core::skill::Skill>;
```

## Data flow (startup)

```
1. Config loaded (bin)                → cfg.skills.source_dirs : Vec<String>
2. dirs = [~/.config/zoid/skills, ./.zoid/skills]   (each only if it exists)
        ++ expand(~/env, source_dirs)                (bin owns expansion)
3. imported = import_skills(&dirs)     → Vec<Skill>  (base_dir set, absolute)
4. let mut reg = SkillRegistry::builtin();
   for s in imported { reg.push_unique(s); }         (builtins protected)
5. Arc::new(reg) → App
   → menu()        now lists imported skills alongside the builtins
   → invoke_skill  resolves imported names; returned body carries the
                   "Skill files are in: <abs>/" anchor
   → model can read_file the siblings by absolute path, on demand
```

Notes:

- **No new runtime machinery.** The `invoke_skill` tool, the tool-call/tool-result
  loop, and the menu-in-system-prompt all already exist (Slice 0). This slice only
  changes *what fills the registry* and adds one anchor line to the tool result.
- **Import is one-shot at startup.** No hot-reload / directory watch this slice.
- **The menu grows with the corpus.** Every imported skill's `name: description`
  appears in the active mode's system-prompt menu, so the model can see and invoke
  it. (Menu-size / context-budget tuning for large corpora is an explicit
  later-slice concern — see Out of scope.)

## Error handling & degradation

Mirrors Slice 0's principle: **a bad input is skipped, never fatal.** Startup must
succeed even with a misconfigured or partially-broken skill directory.

| Failure | Behavior |
|---|---|
| A `source_dir` is missing / unreadable | Skip that dir, warn, continue. |
| A child dir has no `SKILL.md` | Silently ignored (not every subdir is a skill). |
| `SKILL.md` has no frontmatter block, or `name` is missing/empty | Skip that skill, warn, continue. |
| Name collision (two imports, or import vs builtin) | **First-wins.** Builtins and earlier-scanned dirs are protected; the later duplicate is skipped with a warning. |
| No skills found anywhere | Registry = builtins only — exactly today's behavior. No crash. |
| Unreadable individual file (permissions, non-UTF-8) | Skip that skill, warn, continue. |

`parse_skill_md` returns `Result<_, String>` (a human-readable reason) rather than
panicking; the bin logs the reason with the offending path and moves on.

## Testing

Entirely unit-testable; no Tier-2 real-model smoke (the slice has no behavioral
unknown).

### Core (pure, no filesystem)

- `parse_skill_md` happy path: `name`, `description`, and `body` extracted from a
  representative `SKILL.md`, including a double-quoted `description` containing
  embedded colons and commas (the real Superpowers `brainstorming` shape).
- `parse_skill_md` preserves the body verbatim (leading/trailing content,
  multiple `---` inside the body do not re-split).
- `parse_skill_md` errors: no frontmatter block → `Err`; frontmatter present but
  `name` missing/empty → `Err`.
- `SkillRegistry::push_unique`: appends a new name (returns `true`); a duplicate
  name returns `false` and leaves the registry unchanged; `menu()`/`names()`
  include a successfully pushed skill.
- `builtin()` skills carry `base_dir: None` (regression guard for the anchor).

### Bin (tempdir fixtures)

- `import_skills` over a temp tree with two valid skill dirs and one malformed
  (`SKILL.md` with no frontmatter) returns exactly the two valid skills, each with
  the correct **absolute** `base_dir`; the malformed one is skipped.
- `import_skills` over a non-existent dir returns empty (no panic).
- `invoke_skill` result: appends `"Skill files are in: <abs>/"` when `base_dir` is
  `Some`; appends nothing when `None`.

### Config

- `parse_toml` accepts `[skills] source_dirs = ["a", "b"]` into `PartialSkills`.
- `merge` unions `source_dirs` across two layers (user + project) into the
  deduped union, first-seen order.

## Out of scope for this slice

- Promotion of imported skills into `AgentProfile`s / modes, and the
  `[[mode]]` config layer.
- Shift+Tab mode quick-switch overlay, active-mode status line, mode persistence.
- Tool-name aliasing for "ghost" tools referenced by imported skill bodies (a
  skill may reference tools zoid does not have; this slice imports the text as-is
  and does not remap tool names).
- Recursive/nested skill discovery beyond immediate `*/SKILL.md`; a `source_dir`
  that *is itself* a single skill dir (containing `SKILL.md` directly rather than
  in children).
- Hot-reload / filesystem watching; re-import on config change without restart.
- Context-budget / menu-size tuning for very large corpora (the menu currently
  lists every imported skill).
- Provenance tracking / config-overlay display for `source_dirs`.
- Any change to the `Mode` UI enum (`state.rs`, Chat/Build) — unrelated to
  modes-as-agents.
