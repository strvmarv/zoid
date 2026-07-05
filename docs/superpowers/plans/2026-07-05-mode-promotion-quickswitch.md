# Mode Promotion + Quick-Switch (Slice 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's hard-coded `Chat`/`Build` UI mode into a real, extensible **mode registry** — named agents that own scoped skill sets, discovered from on-disk `mode.md` folders, cycled with Shift+Tab, with an ambient system-prompt overlay, graceful `Broken`-mode handling, hot reload, and per-session persistence.

**Architecture:** A new **pure `ModeRegistry`** in `zoid-core` (a `Vec<Mode>` where `Mode` is `Ready{profile, skills}` or `Broken{name, error}`, index 0 always `Chat`) plus pure `effective_skills`/`active_turn`/`overlay_prompt` helpers. The **bin** discovers `mode.md` folders (mirroring the Slice-2 skill importer), composes each mode's overlay (`SYSTEM_PROMPT + body`), and — critically — builds a **per-turn snapshot** of the active mode's effective skills so `invoke_skill` never shares mutable state with an in-flight turn. The **TUI** retires the `Mode` enum, mirrors the active mode onto `ShellState`, and cycles via a payload-carrying `Command`.

**Tech Stack:** Rust (edition 2021), `rusqlite` (SQLite), `ratatui`/`insta` (TUI snapshots), `tokio`. Reuses `parse_skill_md` (no YAML crate) and the Slice-2 `skill_import` walker.

## Global Constraints

- **`zoid-core` stays pure** — no `std::fs`, process, network, `git2`, or provider deps. `mode.rs` and the `config.rs` additions must add **zero** dependencies. All filesystem IO lives in the `zoid` bin (`mode_import.rs`).
- **Reuse `parse_skill_md`** for `mode.md` — no new parser, no YAML crate. A `mode.md` is structurally a `SKILL.md`.
- **Loaders are total** — a bad mode folder yields `Mode::Broken`, a bad `SKILL.md` is skipped-with-warning to stderr; **never** `panic!`/`unwrap` on external input, never abort startup. (Mirrors `skill_import.rs`.)
- **First-wins dedup** — earlier modes/skills win name collisions (`SkillRegistry::push_unique` semantics).
- **`Chat` is index 0** and non-removable in every `ModeRegistry`.
- **Per task:** `cargo test --workspace` green, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo fmt --all` clean. TDD (failing test first). Commit at the end of each task. **No `Co-Authored-By` / co-author trailer** in commit messages (repo rule).
- **Seamed, not honored this slice:** a mode's `tools`/`model` and the overlay *picker* are out of scope; the overlay *body* IS honored.

---

## File Structure

**Created:**
- `crates/zoid-core/src/mode.rs` — pure `Mode`, `ModeRegistry`, `effective_skills`, `active_turn`, `overlay_prompt`.
- `crates/zoid/src/mode_import.rs` — effectful mode-folder discovery + `build_mode_registry`.

**Modified:**
- `crates/zoid-core/src/lib.rs` — `pub mod mode;`.
- `crates/zoid-core/src/skill.rs` — add `SkillRegistry::all()` accessor.
- `crates/zoid-core/src/config.rs` — `[modes] source_dirs` (`ModesConfig`/`PartialModes`/merge).
- `crates/zoid-core/src/store.rs` — first schema migration (`active_mode` column) + get/set.
- `crates/zoid-core/src/session.rs` — actor messages `set_active_mode`/`get_active_mode`.
- `crates/zoid-core/src/agent_profile.rs` — **delete** `AgentProfileRegistry` (keep `AgentProfile`).
- `crates/zoid/src/lib.rs` — `pub mod mode_import;`.
- `crates/zoid/src/main.rs` — `App.modes`; per-turn snapshot in `spawn_turn`; switch/cycle/reload/persist wiring.
- `crates/zoid-tui/src/state.rs` — remove `enum Mode`/`mode` field; add mirror fields.
- `crates/zoid-tui/src/command.rs` — `SwitchMode(String)` + `ReloadModes`.
- `crates/zoid-tui/src/route.rs` — `Action::SwitchMode` → `Action::CycleMode`; drop Esc-from-Build.
- `crates/zoid-tui/src/palette.rs` — mode rows from `mode_names`.
- `crates/zoid-tui/src/render.rs` — chip from mirror; broken error card; delete `render_build_placeholder`.
- `crates/zoid-tui/examples/preview.rs` + `crates/zoid-tui/tests/shell_snapshot.rs` — update for the new state shape.

**Dependency order:** T1 → T2 → T3 → T4 → T5 → T6 → T7. T1/T2 are pure-core and independent of each other (T1 first by convention). T3 needs T1. T4 needs T1+T3. T5 needs T1 (for `set_active`). T6 is the cross-crate cutover (needs T1–T5). T7 is fidelity + cleanup.

---

### Task 1: Core mode model (`zoid-core/src/mode.rs`)

**Files:**
- Create: `crates/zoid-core/src/mode.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod mode;`)
- Modify: `crates/zoid-core/src/skill.rs` (add `all()` accessor)
- Test: inline `#[cfg(test)] mod tests` in `mode.rs`

**Interfaces:**
- Consumes: `crate::agent_profile::AgentProfile`, `crate::skill::{Skill, SkillRegistry}`.
- Produces:
  - `enum Mode { Ready { profile: AgentProfile, skills: SkillRegistry }, Broken { name: String, error: String } }`
  - `Mode::chat(base: AgentProfile) -> Mode` (Ready, empty skills)
  - `Mode::name(&self) -> &str`, `Mode::description(&self) -> &str`, `Mode::is_broken(&self) -> bool`
  - `struct ModeRegistry { modes: Vec<Mode>, active: usize }` with `new(Vec<Mode>) -> Self`, `active(&self) -> &Mode`, `active_name(&self) -> &str`, `active_is_broken(&self) -> bool`, `cycle_next(&mut self)`, `set_active(&mut self, &str) -> bool`, `names(&self) -> Vec<String>`
  - `fn overlay_prompt(base_prompt: &str, body: &str) -> String`
  - `fn effective_skills(global: &SkillRegistry, active: &Mode) -> SkillRegistry`
  - `fn active_turn(modes: &ModeRegistry, global: &SkillRegistry, base: &AgentProfile) -> (AgentProfile, SkillRegistry)`
  - `SkillRegistry::all(&self) -> &[Skill]`

- [ ] **Step 1: Add the `SkillRegistry::all()` accessor test (failing)**

In `crates/zoid-core/src/skill.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn all_exposes_every_skill_in_order() {
        let r = SkillRegistry::builtin();
        let names: Vec<&str> = r.all().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["spike-plan", "spike-implement"]);
    }
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p zoid-core all_exposes_every_skill_in_order`
Expected: FAIL — `no method named 'all' found`.

- [ ] **Step 3: Implement `all()`**

In `crates/zoid-core/src/skill.rs`, in `impl SkillRegistry` (next to `names`):

```rust
    /// All skills in registry order (for composing scoped views).
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }
```

- [ ] **Step 4: Verify it passes**

Run: `cargo test -p zoid-core all_exposes_every_skill_in_order`
Expected: PASS.

- [ ] **Step 5: Register the module + write the failing `mode.rs` core tests**

In `crates/zoid-core/src/lib.rs` add (alphabetical among the `pub mod` lines): `pub mod mode;`

Create `crates/zoid-core/src/mode.rs` with the tests first (implementation empty stubs come next step). Write the full file with this test module:

```rust
//! Modes: a named agent that owns a scoped set of skills. Pure value-holders +
//! scoping logic — the effectful discovery of `mode.md` folders lives in the bin
//! (`mode_import.rs`). `Chat` is the non-removable index-0 floor. The ambient
//! system-prompt overlay is composed here (`overlay_prompt`) from a base prompt
//! passed in by the bin, because `SYSTEM_PROMPT` is bin-only.

use crate::agent_profile::AgentProfile;
use crate::skill::SkillRegistry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::Skill;

    fn prof(name: &str, prompt: &str) -> AgentProfile {
        AgentProfile {
            name: name.into(),
            description: format!("{name} desc"),
            system_prompt: prompt.into(),
            tools: vec![],
            model: None,
        }
    }
    fn skill(name: &str) -> Skill {
        Skill { name: name.into(), description: "d".into(), body: format!("body-{name}"), base_dir: None }
    }
    fn ready(name: &str, prompt: &str, skills: Vec<Skill>) -> Mode {
        Mode::Ready { profile: prof(name, prompt), skills: SkillRegistry::new(skills) }
    }

    #[test]
    fn overlay_prompt_appends_body_or_returns_base() {
        assert_eq!(overlay_prompt("BASE", ""), "BASE");
        assert_eq!(overlay_prompt("BASE", "OVER"), "BASE\n\nOVER");
    }

    #[test]
    fn chat_mode_has_base_profile_and_no_skills() {
        let m = Mode::chat(prof("Chat", "BASE"));
        assert_eq!(m.name(), "Chat");
        assert!(!m.is_broken());
        match &m {
            Mode::Ready { profile, skills } => {
                assert_eq!(profile.system_prompt, "BASE");
                assert!(skills.all().is_empty());
            }
            _ => panic!("chat must be Ready"),
        }
    }

    #[test]
    fn broken_mode_reports_name_and_is_broken() {
        let m = Mode::Broken { name: "Bust".into(), error: "boom".into() };
        assert_eq!(m.name(), "Bust");
        assert!(m.is_broken());
    }

    #[test]
    fn effective_skills_ready_puts_mode_first_and_shadows_global() {
        let global = SkillRegistry::new(vec![skill("brainstorming"), skill("y")]);
        let mode = ready("SP", "p", vec![skill("brainstorming"), skill("x")]);
        let eff = effective_skills(&global, &mode);
        // mode's brainstorming + x first, then global y; global brainstorming shadowed.
        assert_eq!(eff.names(), vec!["brainstorming", "x", "y"]);
        assert_eq!(eff.get("brainstorming").unwrap().body, "body-brainstorming"); // the mode copy
    }

    #[test]
    fn effective_skills_broken_is_globals_only() {
        let global = SkillRegistry::new(vec![skill("y")]);
        let broken = Mode::Broken { name: "b".into(), error: "e".into() };
        assert_eq!(effective_skills(&global, &broken).names(), vec!["y"]);
    }

    #[test]
    fn registry_cycles_wraps_and_sets_active_by_name() {
        let mut reg = ModeRegistry::new(vec![
            Mode::chat(prof("Chat", "BASE")),
            ready("SP", "p", vec![]),
        ]);
        assert_eq!(reg.active_name(), "Chat");
        reg.cycle_next();
        assert_eq!(reg.active_name(), "SP");
        reg.cycle_next(); // wraps
        assert_eq!(reg.active_name(), "Chat");
        assert!(reg.set_active("SP"));
        assert_eq!(reg.active_name(), "SP");
        assert!(!reg.set_active("ghost")); // miss, unchanged
        assert_eq!(reg.active_name(), "SP");
        assert_eq!(reg.names(), vec!["Chat", "SP"]);
    }

    #[test]
    fn active_turn_chat_is_base_prompt_and_globals() {
        let base = prof("default", "BASE");
        let global = SkillRegistry::new(vec![skill("y")]);
        let reg = ModeRegistry::new(vec![Mode::chat(base.clone())]);
        let (profile, eff) = active_turn(&reg, &global, &base);
        assert_eq!(profile.system_prompt, "BASE");
        assert_eq!(eff.names(), vec!["y"]);
    }

    #[test]
    fn active_turn_ready_uses_mode_profile_and_scoped_skills() {
        let base = prof("default", "BASE");
        let global = SkillRegistry::new(vec![skill("y")]);
        let mut reg = ModeRegistry::new(vec![
            Mode::chat(base.clone()),
            ready("SP", "BASE\n\nOVER", vec![skill("x")]),
        ]);
        reg.set_active("SP");
        let (profile, eff) = active_turn(&reg, &global, &base);
        assert_eq!(profile.system_prompt, "BASE\n\nOVER"); // overlay present
        assert_eq!(eff.names(), vec!["x", "y"]);
    }

    #[test]
    fn active_turn_broken_falls_back_to_base_and_globals() {
        let base = prof("default", "BASE");
        let global = SkillRegistry::new(vec![skill("y")]);
        let mut reg = ModeRegistry::new(vec![
            Mode::chat(base.clone()),
            Mode::Broken { name: "B".into(), error: "e".into() },
        ]);
        reg.set_active("B");
        let (profile, eff) = active_turn(&reg, &global, &base);
        assert_eq!(profile.system_prompt, "BASE"); // no overlay for broken
        assert_eq!(eff.names(), vec!["y"]);
    }
}
```

- [ ] **Step 6: Run tests, verify they fail to compile (types not defined)**

Run: `cargo test -p zoid-core --lib mode::`
Expected: FAIL — `cannot find type 'Mode'` etc.

- [ ] **Step 7: Implement the module (above the test module)**

Insert into `crates/zoid-core/src/mode.rs`, after the `use` lines and before `#[cfg(test)]`:

```rust
/// One mode: either a fully-loaded agent (`Ready`) or a slot that failed to load
/// (`Broken`) but stays visible in the cycle so the failure is surfaced, never
/// silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Ready { profile: AgentProfile, skills: SkillRegistry },
    Broken { name: String, error: String },
}

impl Mode {
    /// The `Chat` floor: the base coding-agent profile, owning no skills.
    pub fn chat(base: AgentProfile) -> Mode {
        Mode::Ready { profile: base, skills: SkillRegistry::new(vec![]) }
    }
    pub fn name(&self) -> &str {
        match self {
            Mode::Ready { profile, .. } => &profile.name,
            Mode::Broken { name, .. } => name,
        }
    }
    pub fn description(&self) -> &str {
        match self {
            Mode::Ready { profile, .. } => &profile.description,
            Mode::Broken { error, .. } => error,
        }
    }
    pub fn is_broken(&self) -> bool {
        matches!(self, Mode::Broken { .. })
    }
}

/// Compose a mode's ambient system prompt: the base coding-agent prompt plus the
/// `mode.md` body as an overlay. Empty body ⇒ just the base (behaves like Chat).
/// Pure and base-agnostic (the bin passes `SYSTEM_PROMPT`, which core can't see).
pub fn overlay_prompt(base_prompt: &str, body: &str) -> String {
    if body.is_empty() {
        base_prompt.to_string()
    } else {
        format!("{base_prompt}\n\n{body}")
    }
}

/// The skills the model may `invoke_skill` while `active` is the current mode:
/// the active mode's scoped skills (seeded first, so they win name collisions via
/// first-wins `push_unique`) then all globals. `Broken` ⇒ globals only.
pub fn effective_skills(global: &SkillRegistry, active: &Mode) -> SkillRegistry {
    match active {
        Mode::Ready { skills, .. } => {
            let mut reg = SkillRegistry::new(skills.all().to_vec());
            for g in global.all() {
                reg.push_unique(g.clone());
            }
            reg
        }
        Mode::Broken { .. } => SkillRegistry::new(global.all().to_vec()),
    }
}

/// The (profile, effective-skills) a turn should run with, given the active mode.
/// `Ready` ⇒ its own profile (carrying the overlay) + scoped skills; `Broken` ⇒
/// the base profile + globals only (so a broken active mode degrades to Chat-like
/// behavior behind its error card).
pub fn active_turn(
    modes: &ModeRegistry,
    global: &SkillRegistry,
    base: &AgentProfile,
) -> (AgentProfile, SkillRegistry) {
    let active = modes.active();
    match active {
        Mode::Ready { profile, .. } => (profile.clone(), effective_skills(global, active)),
        Mode::Broken { .. } => (base.clone(), effective_skills(global, active)),
    }
}

/// An ordered set of modes with one active. `modes[0]` is `Chat` by construction
/// (the bin guarantees it); `active()` never fails.
#[derive(Debug, Clone)]
pub struct ModeRegistry {
    modes: Vec<Mode>,
    active: usize,
}

impl ModeRegistry {
    /// Build from a non-empty mode list (caller puts `Chat` at index 0). Active = 0.
    pub fn new(modes: Vec<Mode>) -> Self {
        assert!(!modes.is_empty(), "ModeRegistry needs at least Chat");
        Self { modes, active: 0 }
    }
    pub fn active(&self) -> &Mode {
        &self.modes[self.active]
    }
    pub fn active_name(&self) -> &str {
        self.modes[self.active].name()
    }
    pub fn active_is_broken(&self) -> bool {
        self.modes[self.active].is_broken()
    }
    /// Advance to the next mode, wrapping (Shift+Tab).
    pub fn cycle_next(&mut self) {
        self.active = (self.active + 1) % self.modes.len();
    }
    /// Make the named mode active; `false` (unchanged) if none matches.
    pub fn set_active(&mut self, name: &str) -> bool {
        match self.modes.iter().position(|m| m.name() == name) {
            Some(i) => {
                self.active = i;
                true
            }
            None => false,
        }
    }
    pub fn names(&self) -> Vec<String> {
        self.modes.iter().map(|m| m.name().to_string()).collect()
    }
}
```

- [ ] **Step 8: Run the full core suite, verify green**

Run: `cargo test -p zoid-core`
Expected: PASS (new `mode::tests` + existing).

- [ ] **Step 9: Lint + format + commit**

```bash
cargo clippy -p zoid-core --all-targets -- -D warnings
cargo fmt --all
git add crates/zoid-core/src/mode.rs crates/zoid-core/src/lib.rs crates/zoid-core/src/skill.rs
git commit -m "feat(core): pure Mode/ModeRegistry + effective_skills/active_turn/overlay_prompt (mode/skill slice 3)"
```

---

### Task 2: Core config — `[modes] source_dirs`

**Files:**
- Modify: `crates/zoid-core/src/config.rs`
- Test: inline `mod merge_tests` in `config.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `Config.modes: ModesConfig { source_dirs: Vec<String> }`, `PartialConfig.modes: PartialModes { source_dirs: Option<Vec<String>> }`; `merge` unions `modes.source_dirs` exactly like `skills.source_dirs`.

- [ ] **Step 1: Write failing tests**

In `crates/zoid-core/src/config.rs`, in `mod merge_tests`, add:

```rust
    #[test]
    fn parses_modes_source_dirs() {
        let (p, _) = parse_toml("[modes]\nsource_dirs = [\"m1\", \"m2\"]").unwrap();
        assert_eq!(p.modes.source_dirs, Some(vec!["m1".to_string(), "m2".to_string()]));
    }

    #[test]
    fn merge_unions_modes_source_dirs() {
        let (user, _) = parse_toml("[modes]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[modes]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.modes.source_dirs, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p zoid-core parses_modes_source_dirs merge_unions_modes_source_dirs`
Expected: FAIL — `no field 'modes'`.

- [ ] **Step 3: Implement**

In `config.rs`:

1. After `struct SkillsConfig` (line ~12) add:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModesConfig {
    /// Extra directories to scan for `<mode>/mode.md` folders (beyond the two
    /// convention dirs the bin adds). Unioned across config layers.
    pub source_dirs: Vec<String>,
}
```
2. In `struct Config` add field after `pub skills: SkillsConfig,`: `pub modes: ModesConfig,`
3. In `impl Default for Config` add after `skills: SkillsConfig::default(),`: `modes: ModesConfig::default(),`
4. After `struct PartialSkills` (line ~148) add:
```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialModes {
    pub source_dirs: Option<Vec<String>>,
}
```
5. In `struct PartialConfig` add after `pub skills: PartialSkills,`: `pub modes: PartialModes,`
6. In `merge`, after the `p.skills.source_dirs` union block (line ~234–240) add:
```rust
        if let Some(dirs) = &p.modes.source_dirs {
            for d in dirs {
                if !cfg.modes.source_dirs.contains(d) {
                    cfg.modes.source_dirs.push(d.clone());
                }
            }
        }
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p zoid-core parses_modes_source_dirs merge_unions_modes_source_dirs`
Expected: PASS. Also run `cargo test -p zoid-core` (the `empty_layer_changes_nothing` test compares against `Config::default()` — the new field defaults so it still holds).

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p zoid-core --all-targets -- -D warnings && cargo fmt --all
git add crates/zoid-core/src/config.rs
git commit -m "feat(core): [modes] source_dirs config (union-merged, mirrors [skills])"
```

---

### Task 3: Bin mode importer (`zoid/src/mode_import.rs`)

**Files:**
- Create: `crates/zoid/src/mode_import.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod mode_import;`)
- Test: inline `#[cfg(test)] mod tests` (temp dirs via `tempfile`, already a dev-dep — see `skill_import.rs` tests)

**Interfaces:**
- Consumes: `zoid_core::mode::{Mode, ModeRegistry, overlay_prompt}`, `zoid_core::agent_profile::AgentProfile`, `zoid_core::skill::{parse_skill_md, Skill, SkillRegistry}`, `crate::skill_import::import_skills`.
- Produces:
  - `fn resolve_mode_dirs(source_dirs: &[String], user_cfg_dir: &Path, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf>`
  - `fn build_mode_registry(base: &AgentProfile, dirs: &[PathBuf]) -> ModeRegistry`

- [ ] **Step 1: Register module + write failing tests**

In `crates/zoid/src/lib.rs` add: `pub mod mode_import;`

Create `crates/zoid/src/mode_import.rs` with the test module (impl next step):

```rust
//! Filesystem source adapter for modes — the effectful half (the pure model is
//! `zoid_core::mode`). Walks convention + configured dirs, and for each subfolder
//! with a `mode.md` builds a `Mode::Ready` (its `*/SKILL.md` become the mode's
//! scoped skills) or, on a parse failure, a `Mode::Broken` named by the folder.
//! Bad inputs are skipped/degraded, never fatal — mirroring `skill_import.rs`.

use std::path::{Path, PathBuf};

use zoid_core::agent_profile::AgentProfile;
use zoid_core::mode::{overlay_prompt, Mode, ModeRegistry};
use zoid_core::skill::{parse_skill_md, SkillRegistry};

use crate::skill_import::import_skills;

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AgentProfile {
        AgentProfile {
            name: "default".into(),
            description: "base".into(),
            system_prompt: "BASE".into(),
            tools: vec![],
            model: None,
        }
    }

    fn write(dir: &Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_mode_dirs(
            &["~/m".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/modes"),
                PathBuf::from("/proj/.zoid/modes"),
                PathBuf::from("/home/u/m"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn chat_is_always_index_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert_eq!(reg.names().first().map(String::as_str), Some("default")); // Chat = base profile name
    }

    #[test]
    fn ready_mode_composes_overlay_and_scopes_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "superpowers/mode.md", "---\nname: Superpowers\ndescription: sp\n---\nUSE SKILLS\n");
        write(root, "superpowers/brainstorming/SKILL.md", "---\nname: brainstorming\ndescription: d\n---\nBODY\n");
        let reg = build_mode_registry(&base(), &[root.to_path_buf()]);
        assert_eq!(reg.names(), vec!["default".to_string(), "Superpowers".to_string()]);
        match &reg_get(&reg, "Superpowers") {
            Mode::Ready { profile, skills } => {
                assert_eq!(profile.system_prompt, "BASE\n\nUSE SKILLS\n"); // overlay = base + body
                assert_eq!(skills.names(), vec!["brainstorming".to_string()]);
            }
            _ => panic!("Superpowers must be Ready"),
        }
    }

    #[test]
    fn malformed_mode_md_is_broken_named_by_folder() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "busted/mode.md", "no frontmatter here\n");
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert!(matches!(reg_get(&reg, "busted"), Mode::Broken { .. }));
    }

    #[test]
    fn folder_without_mode_md_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "just-skills/x/SKILL.md", "---\nname: x\ndescription: d\n---\nb\n");
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert_eq!(reg.names(), vec!["default".to_string()]); // only Chat
    }

    #[test]
    fn bad_skill_inside_good_mode_keeps_mode_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "m/mode.md", "---\nname: M\ndescription: d\n---\n\n");
        write(root, "m/good/SKILL.md", "---\nname: good\ndescription: d\n---\nb\n");
        write(root, "m/bad/SKILL.md", "no frontmatter\n");
        let reg = build_mode_registry(&base(), &[root.to_path_buf()]);
        match reg_get(&reg, "M") {
            Mode::Ready { skills, .. } => assert_eq!(skills.names(), vec!["good".to_string()]),
            _ => panic!("M must be Ready"),
        }
    }

    #[test]
    fn missing_dir_is_skipped_without_panic() {
        let reg = build_mode_registry(&base(), &[PathBuf::from("/nonexistent/zoid/modes/xyz")]);
        assert_eq!(reg.names(), vec!["default".to_string()]);
    }

    // Test helper: fetch a mode by name (the registry has no public getter).
    fn reg_get<'a>(reg: &'a ModeRegistry, name: &str) -> &'a Mode {
        // names() gives order; re-walk via set_active + active is awkward, so we
        // expose the modes through a clone+cycle. Simpler: cycle to it.
        let mut r = reg.clone();
        assert!(r.set_active(name), "mode {name} not found");
        // active() borrows r (local); return an equivalent from the original by
        // matching index.
        let idx = reg.names().iter().position(|n| n == name).unwrap();
        // Safe: names() order matches modes order.
        REG_INDEX.with(|_| {});
        &modes_of(reg)[idx]
    }

    // NOTE: `ModeRegistry` keeps `modes` private; for tests we add a
    // `pub(crate) fn modes(&self) -> &[Mode]` in Step 3 and use it here instead of
    // the placeholder below.
    fn modes_of(reg: &ModeRegistry) -> &[Mode] {
        reg.modes_for_test()
    }
    thread_local! { static REG_INDEX: () = (); }
}
```

> **Simplify the test helper:** the placeholder above is intentionally ugly to flag a real need. Replace Steps 1's `reg_get`/`modes_of`/`REG_INDEX` with a single clean accessor. In `zoid-core/src/mode.rs` (Task 1 file) add a test-only accessor:
> ```rust
> impl ModeRegistry {
>     /// Read-only view of all modes, in order. For importer/bin tests.
>     pub fn modes(&self) -> &[Mode] { &self.modes }
> }
> ```
> Then in this test file use `reg.modes().iter().find(|m| m.name()==name).unwrap()`. Use this clean form; delete the placeholder helpers.

- [ ] **Step 2: Add `ModeRegistry::modes()` accessor to Task-1 file, rewrite the test helper**

In `crates/zoid-core/src/mode.rs`, add the public `modes(&self) -> &[Mode]` accessor shown above (it's generally useful — the bin needs it to sync `mode_names` and to look up the active `Broken` error). Replace the placeholder test helpers in `mode_import.rs` with:

```rust
    fn reg_get<'a>(reg: &'a ModeRegistry, name: &str) -> &'a Mode {
        reg.modes().iter().find(|m| m.name() == name).unwrap()
    }
```

(Delete `modes_of`, `REG_INDEX`, and the `thread_local!`.)

- [ ] **Step 3: Run, verify fail**

Run: `cargo test -p zoid --lib mode_import`
Expected: FAIL — `resolve_mode_dirs`/`build_mode_registry` not found.

- [ ] **Step 4: Implement the importer**

Insert into `mode_import.rs`, before `#[cfg(test)]`:

```rust
/// Ordered dirs to scan: the two convention dirs (`<cfg>/modes`, `<cwd>/.zoid/modes`)
/// then configured `source_dirs` (leading `~`/`~/` expanded). Pure path arithmetic.
pub fn resolve_mode_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![user_cfg_dir.join("modes"), cwd.join(".zoid").join("modes")];
    for s in source_dirs {
        dirs.push(expand_tilde(s, home));
    }
    dirs
}

fn expand_tilde(s: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if s == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = s.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Build the mode registry: `Chat` (from `base`) at index 0, then one mode per
/// `<dir>/<name>/mode.md`. A folder without `mode.md` is ignored; a malformed
/// `mode.md` becomes `Mode::Broken` named by its folder. Scoped skills come from
/// the folder's `*/SKILL.md` (reusing the skill importer). First-wins by mode
/// name across dirs. Never panics.
pub fn build_mode_registry(base: &AgentProfile, dirs: &[PathBuf]) -> ModeRegistry {
    let mut modes = vec![Mode::chat(base.clone())];
    let mut seen: Vec<String> = vec![base.name.clone()];
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping modes dir {}: {e}", dir.display());
                continue;
            }
        };
        // Sort by folder name for deterministic cycle order.
        let mut folders: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        folders.sort();
        for folder in folders {
            let manifest = folder.join("mode.md");
            if !manifest.is_file() {
                continue; // not a mode
            }
            let folder_name = folder
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("<mode>")
                .to_string();
            let mode = load_mode(base, &folder, &manifest, &folder_name);
            let name = mode.name().to_string();
            if seen.iter().any(|n| n == &name) {
                eprintln!("zoid: skipping duplicate mode '{name}' at {}", folder.display());
                continue;
            }
            seen.push(name);
            modes.push(mode);
        }
    }
    ModeRegistry::new(modes)
}

/// Load one mode folder into `Ready` or `Broken`. Total — a read/parse failure
/// yields `Broken` named by the folder (so it stays visible in the cycle).
fn load_mode(base: &AgentProfile, folder: &Path, manifest: &Path, folder_name: &str) -> Mode {
    let text = match std::fs::read_to_string(manifest) {
        Ok(t) => t,
        Err(e) => {
            return Mode::Broken { name: folder_name.to_string(), error: format!("cannot read {}: {e}", manifest.display()) }
        }
    };
    let parsed = match parse_skill_md(&text) {
        Ok(p) => p,
        Err(reason) => {
            return Mode::Broken { name: folder_name.to_string(), error: format!("{}: {reason}", manifest.display()) }
        }
    };
    // Scoped skills: the mode folder's immediate `*/SKILL.md` children.
    let skills = SkillRegistry::new(import_skills(&[folder.to_path_buf()]));
    let profile = AgentProfile {
        name: parsed.name,
        description: parsed.description,
        system_prompt: overlay_prompt(&base.system_prompt, &parsed.body),
        tools: vec![], // SEAMED — a mode's own tool allow-list is not honored this slice
        model: None,   // SEAMED — a mode's model override is not honored this slice
    };
    Mode::Ready { profile, skills }
}
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p zoid --lib mode_import`
Expected: PASS (all 7 tests). Also `cargo test -p zoid-core` (the new `modes()` accessor compiles).

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy -p zoid --all-targets -- -D warnings && cargo fmt --all
git add crates/zoid/src/mode_import.rs crates/zoid/src/lib.rs crates/zoid-core/src/mode.rs
git commit -m "feat(zoid): mode.md folder importer -> ModeRegistry (Ready/Broken, scoped skills, overlay)"
```

---

### Task 4: Wire `ModeRegistry` into the App + per-turn snapshot

**Files:**
- Modify: `crates/zoid/src/main.rs` (App struct ~1006–1017; construction ~1228–1248; `spawn_turn` ~3047–3089)
- Test: `crates/zoid/tests/mode_turn.rs` (new integration test for the pure `active_turn` + `chat_tools` wiring)

**Interfaces:**
- Consumes: `zoid_core::mode::{ModeRegistry, active_turn}`, `zoid::mode_import::{resolve_mode_dirs, build_mode_registry}`, `zoid::agent::default_profile`, `zoid::invoke_skill::chat_tools`, `zoid_core::skill::SkillRegistry`.
- Produces: `App.modes: ModeRegistry` (replaces `App.profiles`). `spawn_turn` builds a per-turn effective-skills snapshot and a fresh `invoke_skill` tool bound to it.

> **Behavior note:** switching is not wired until Task 6, so `modes.active()` stays `Chat` here — this task is a behavior-preserving refactor that installs the machinery. The test drives `active_turn` directly with a non-Chat mode to prove the snapshot/overlay path.

- [ ] **Step 1: Write the failing integration test**

Create `crates/zoid/tests/mode_turn.rs`:

```rust
//! The per-turn snapshot: `active_turn` picks the active mode's profile + scoped
//! skills, and `chat_tools` bound to that snapshot resolves scoped skills only
//! while the mode is active (proving switch/reload can't mutate an in-flight turn).

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::mode::{active_turn, Mode, ModeRegistry};
use zoid_core::skill::{Skill, SkillRegistry};

fn base() -> AgentProfile {
    zoid::agent::default_profile()
}
fn scoped(name: &str) -> Skill {
    Skill { name: name.into(), description: "d".into(), body: format!("BODY-{name}"), base_dir: None }
}

#[test]
fn active_turn_snapshot_scopes_invoke_skill() {
    let base = base();
    let global = SkillRegistry::new(vec![]); // only built-ins would be here in prod; empty is fine
    let mut modes = ModeRegistry::new(vec![
        Mode::chat(base.clone()),
        Mode::Ready {
            profile: AgentProfile { name: "SP".into(), description: "d".into(),
                system_prompt: zoid_core::mode::overlay_prompt(&base.system_prompt, "USE SKILLS"),
                tools: vec![], model: None },
            skills: SkillRegistry::new(vec![scoped("brainstorming")]),
        },
    ]);

    // In Chat: the scoped skill is NOT resolvable.
    let (_p, eff_chat) = active_turn(&modes, &global, &base);
    let tools = Arc::new(zoid::invoke_skill::chat_tools(Arc::new(eff_chat)));
    let inv = tools.iter().find(|t| t.name() == "invoke_skill").unwrap();
    let out = inv.run(&json!({"name": "brainstorming"}), Path::new("."));
    assert!(out.is_error, "brainstorming must be unresolvable in Chat");

    // Switch to SP: overlay present, scoped skill resolvable.
    modes.set_active("SP");
    let (profile, eff_sp) = active_turn(&modes, &global, &base);
    assert!(profile.system_prompt.ends_with("USE SKILLS"));
    let tools = Arc::new(zoid::invoke_skill::chat_tools(Arc::new(eff_sp)));
    let inv = tools.iter().find(|t| t.name() == "invoke_skill").unwrap();
    let out = inv.run(&json!({"name": "brainstorming"}), Path::new("."));
    assert!(!out.is_error && out.text.contains("BODY-brainstorming"));
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p zoid --test mode_turn`
Expected: FAIL — compiles (APIs exist from T1/T3) but this is a NEW behavior contract; if `chat_tools`/`active_turn` are already public it may PASS immediately. If it passes, that confirms the pure contract; proceed to wire the bin (the test guards it going forward).

- [ ] **Step 3: Replace `App.profiles` with `App.modes`**

In `crates/zoid/src/main.rs`, in `struct App` (line ~1012–1014) replace:

```rust
    /// Available mode profiles with the active one marked; drives the turn's
    /// system prompt. v1 holds only the default profile.
    profiles: zoid_core::agent_profile::AgentProfileRegistry,
```
with:
```rust
    /// The active mode + all discovered modes; drives the turn's system prompt,
    /// the effective skill menu, and the mode chip. Index 0 is always Chat.
    modes: zoid_core::mode::ModeRegistry,
    /// The base coding-agent profile (Chat). Kept so mode reload / broken-mode
    /// fallback can recompose without re-reading a const.
    base_profile: zoid_core::agent_profile::AgentProfile,
```

- [ ] **Step 4: Build the registry at construction**

In `main.rs` construction (after the `skills` Arc is built, ~1236, before `let mut app = App {`), add:

```rust
    let base_profile = zoid::agent::default_profile();
    let modes = {
        let mode_dirs = zoid::mode_import::resolve_mode_dirs(
            &config.modes.source_dirs,
            &cfg_dir,
            std::path::Path::new(&root),
            home.as_deref(),
        );
        zoid::mode_import::build_mode_registry(&base_profile, &mode_dirs)
    };
```

Then in the `App { … }` initializer replace the `profiles: …AgentProfileRegistry::new(vec![ zoid::agent::default_profile() ]),` block (lines ~1244–1246) with:

```rust
        modes,
        base_profile,
```

(Leave `tools: Arc::new(zoid::invoke_skill::chat_tools(skills.clone())),` and `skills,` as-is for now; `spawn_turn` overrides tools per turn in the next step, and `app.tools` remains a harmless default the turn no longer reads. Confirm with `git grep -n "app.tools\|\.tools\b" crates/zoid/src/main.rs` that `spawn_turn` is its only reader; if so, you may delete the field + its init in Task 7 cleanup.)

- [ ] **Step 5: Per-turn snapshot in `spawn_turn`**

In `main.rs` `spawn_turn` (lines ~3049–3057) replace:

```rust
    let tools = app.tools.clone();
    …
    let profile = app.profiles.active();
    let menu = app.skills.menu();
    let mut turn_config = zoid::agent::chat_turn_config_with(profile, &menu);
```
with:
```rust
    // Per-turn snapshot: pick the active mode's profile + effective skills ONCE,
    // and bind a fresh invoke_skill tool to that snapshot. A mid-turn mode switch
    // or reload cannot mutate this in-flight turn (spec §5 / risk 1–2).
    let (profile, effective) =
        zoid_core::mode::active_turn(&app.modes, &app.skills, &app.base_profile);
    let menu = effective.menu();
    let tools = std::sync::Arc::new(zoid::invoke_skill::chat_tools(std::sync::Arc::new(effective)));
    let mut turn_config = zoid::agent::chat_turn_config_with(&profile, &menu);
```

(The `let tools = app.tools.clone();` line is removed; everything downstream that moves `tools` into the spawned task is unchanged.)

- [ ] **Step 6: Fix compile fallout (removed `profiles`)**

Run `cargo build -p zoid`. Any remaining reference to `app.profiles` is a compile error — the only expected one is this `spawn_turn` site (now fixed). If `build_mode_registry`/`active_turn` were used with a borrow conflict (`profile` borrows `app.modes` while `tools` also borrows `app`), note `active_turn` returns **owned** values (`AgentProfile`, `SkillRegistry`) — no borrow is held past the call, so `app.turn_cancel = Some(...)` below still compiles.

Run: `cargo build -p zoid`
Expected: builds clean.

- [ ] **Step 7: Run tests, lint, commit**

Run: `cargo test -p zoid --test mode_turn && cargo test -p zoid`
Expected: PASS. Then:

```bash
cargo clippy -p zoid --all-targets -- -D warnings && cargo fmt --all
git add crates/zoid/src/main.rs crates/zoid/tests/mode_turn.rs
git commit -m "feat(zoid): App.modes + per-turn effective-skills snapshot in spawn_turn"
```

---

### Task 5: Per-session persistence — schema migration + get/set active_mode

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (migration in `open`; `set_active_mode`/`get_active_mode`)
- Modify: `crates/zoid-core/src/session.rs` (actor messages + async methods)
- Test: inline `#[cfg(test)] mod tests` in `store.rs`

**Interfaces:**
- Consumes: `rusqlite::Connection`, `ulid::Ulid`.
- Produces:
  - `EventStore::set_active_mode(&self, id: Ulid, mode: &str) -> Result<()>`
  - `EventStore::get_active_mode(&self, id: Ulid) -> Result<Option<String>>`
  - `SessionHandle::set_active_mode(&self, id: Ulid, mode: String) -> Result<()>`
  - `SessionHandle::get_active_mode(&self, id: Ulid) -> Result<Option<String>>`

- [ ] **Step 1: Write failing store tests**

In `crates/zoid-core/src/store.rs`, in `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn active_mode_round_trips_and_defaults_none() {
        let store = EventStore::open(":memory:").unwrap();
        let id = Ulid::new();
        store.insert_session(id, "s", "/repo", 1, 1).unwrap();
        assert_eq!(store.get_active_mode(id).unwrap(), None); // fresh session
        store.set_active_mode(id, "Superpowers").unwrap();
        assert_eq!(store.get_active_mode(id).unwrap(), Some("Superpowers".to_string()));
    }

    #[test]
    fn migration_is_idempotent_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        // First open creates the column; second open must NOT error on re-migrate.
        {
            let s = EventStore::open(p).unwrap();
            let id = Ulid::new();
            s.insert_session(id, "s", "/r", 1, 1).unwrap();
            s.set_active_mode(id, "M").unwrap();
        }
        let s2 = EventStore::open(p).unwrap(); // re-open: column already exists
        // A value written before still reads back.
        let rows = s2.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn migrates_an_old_shape_db_without_active_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let p = path.to_str().unwrap();
        // Simulate a pre-slice-3 DB: sessions table WITHOUT active_mode.
        {
            let conn = rusqlite::Connection::open(p).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT NOT NULL, \
                 root_path TEXT NOT NULL, created_ts INTEGER NOT NULL, last_touched_ts INTEGER NOT NULL);"
            ).unwrap();
            conn.execute(
                "INSERT INTO sessions (id,name,root_path,created_ts,last_touched_ts) VALUES (?1,'s','/r',1,1)",
                rusqlite::params![Ulid::new().to_string()],
            ).unwrap();
        }
        // Opening must add the column (not throw) and reads default to None.
        let store = EventStore::open(p).unwrap();
        let rows = store.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        let id: Ulid = rows[0].id.parse().unwrap();
        assert_eq!(store.get_active_mode(id).unwrap(), None);
    }
```

> Check `SessionRow.id`'s type in `sessions.rs` — if it is `Ulid` (not `String`), drop the `.parse()` in the last test. Adjust to the actual type.

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p zoid-core active_mode_round_trips migration_is_idempotent migrates_an_old_shape`
Expected: FAIL — `no method 'set_active_mode'`; the old-shape test fails when `get_active_mode`'s `SELECT active_mode` hits a missing column.

- [ ] **Step 3: Add the migration to `open`**

In `store.rs`, in `EventStore::open`, after the `execute_batch(…)?;` closes (line ~67, before `Ok(EventStore { conn })`), add:

```rust
        // First-ever schema migration (spec §11). `CREATE TABLE IF NOT EXISTS`
        // above is a no-op for an existing DB, so a NEW column must be added with
        // ALTER TABLE — probed so re-open is idempotent (SQLite has no
        // ADD COLUMN IF NOT EXISTS).
        let has_active_mode: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'active_mode'",
            [],
            |r| r.get(0),
        )?;
        if has_active_mode == 0 {
            conn.execute("ALTER TABLE sessions ADD COLUMN active_mode TEXT", [])?;
        }
```

- [ ] **Step 4: Add `set_active_mode`/`get_active_mode`**

In `store.rs`, in `impl EventStore` (next to `touch_session`, ~219):

```rust
    /// Persist the active mode name for a session (per-session state, spec §11).
    pub fn set_active_mode(&self, id: Ulid, mode: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET active_mode = ?1 WHERE id = ?2",
            params![mode, id.to_string()],
        )?;
        Ok(())
    }

    /// The stored active mode for a session, or `None` if never set.
    pub fn get_active_mode(&self, id: Ulid) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row(
                "SELECT active_mode FROM sessions WHERE id = ?1",
                params![id.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(v)
    }
```

> `optional()` needs `use rusqlite::OptionalExtension;` — add it to the file's `use` block if not present (`git grep -n OptionalExtension crates/zoid-core/src/store.rs`).

- [ ] **Step 5: Run store tests, verify pass**

Run: `cargo test -p zoid-core active_mode_round_trips migration_is_idempotent migrates_an_old_shape`
Expected: PASS. Also `cargo test -p zoid-core` (existing `sessions_crud_round_trips` still green — `INSERT` names its columns explicitly, so the new nullable column doesn't break it).

- [ ] **Step 6: Add the actor methods (`session.rs`)**

In `crates/zoid-core/src/session.rs`, follow the existing message/reply pattern used by `touch_session` (line ~199) — add a request variant to the actor's message enum, handle it in the `spawn` loop by calling `store.set_active_mode(...)` / `store.get_active_mode(...)`, and expose:

```rust
    /// Persist the active mode for a session.
    pub async fn set_active_mode(&self, id: Ulid, mode: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Msg::SetActiveMode { id, mode, reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())?
    }

    /// Read the stored active mode for a session (None if never set).
    pub async fn get_active_mode(&self, id: Ulid) -> Result<Option<String>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Msg::GetActiveMode { id, reply }).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())?
    }
```

> Match the exact names in this file: the message enum, the `tx` field, the `oneshot` import, and the error helper (`closed()` above is a placeholder — use whatever `touch_session` uses, e.g. `anyhow::anyhow!("session actor closed")`). Add `Msg::SetActiveMode { id: Ulid, mode: String, reply: oneshot::Sender<Result<()>> }` and `Msg::GetActiveMode { id: Ulid, reply: oneshot::Sender<Result<Option<String>>> }` and their handler arms mirroring `touch_session`'s arm at line ~97.

- [ ] **Step 7: Build + test the actor**

Run: `cargo test -p zoid-core`
Expected: PASS (actor compiles; store tests green).

- [ ] **Step 8: Lint + commit**

```bash
cargo clippy -p zoid-core --all-targets -- -D warnings && cargo fmt --all
git add crates/zoid-core/src/store.rs crates/zoid-core/src/session.rs
git commit -m "feat(core): per-session active_mode (first schema migration + actor get/set)"
```

---

### Task 6: TUI cutover — retire `Mode` enum, mirror active mode, wire switch/cycle/reload/persist

This is the atomic cross-crate cutover. It must land as one compiling change.

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`, `command.rs`, `route.rs`, `palette.rs`, `render.rs`
- Modify: `crates/zoid-core/src/agent_profile.rs` (delete `AgentProfileRegistry`)
- Modify: `crates/zoid/src/main.rs` (sync mirror; handle cycle/switch/reload; persist; restore on resume)
- Test: updates to existing `route.rs`/`command.rs`/`palette.rs`/`state.rs` unit tests

**Interfaces:**
- Consumes: `App.modes` (T4), `SessionHandle::{get_active_mode,set_active_mode}` (T5), `zoid::mode_import::{resolve_mode_dirs, build_mode_registry}` (reload).
- Produces: `ShellState { active_mode: String, active_mode_broken: bool, mode_names: Vec<String>, … }`; `Command::SwitchMode(String)` + `Command::ReloadModes`; `Action::CycleMode`.

- [ ] **Step 1: `ShellState` — replace `mode` with mirror fields**

In `crates/zoid-tui/src/state.rs`:
1. Delete `enum Mode { Chat, Build }` (lines ~5–9), `toggle_mode` (~372–377), and `set_mode` (~379–381).
2. In `struct ShellState`, replace `pub mode: Mode,` (line 123) with:
```rust
    /// Active mode name, mirrored from the bin's `ModeRegistry` (the renderer is
    /// pure and can't reach `App`). Set on switch/cycle/reload.
    pub active_mode: String,
    /// Whether the active mode failed to load (renders the ⚠ chip + error card).
    pub active_mode_broken: bool,
    /// All mode names in cycle order, for the palette "Switch mode" rows.
    pub mode_names: Vec<String>,
```
3. In `ShellState`'s constructor/`Default` (find where `mode: Mode::Chat` is set), replace with:
```rust
            active_mode: "Chat".to_string(),
            active_mode_broken: false,
            mode_names: vec!["Chat".to_string()],
```

- [ ] **Step 2: `command.rs` — payload change + `:mode`/reload**

In `crates/zoid-tui/src/command.rs`:
1. Change `use crate::state::{DrawerId, Mode};` → `use crate::state::DrawerId;`
2. Replace `SwitchMode(Mode),` with:
```rust
    /// Switch to the named mode (`:mode <name>` / palette).
    SwitchMode(String),
    /// Re-scan mode folders without a restart (`:mode reload`).
    ReloadModes,
```
3. In `parse_command`, replace the `"build"`/`"chat"` arms (lines 33–34) with:
```rust
        "mode reload" => Command::ReloadModes,
        s if s.starts_with("mode ") => Command::SwitchMode(s["mode ".len()..].trim().to_string()),
```
4. Update the test `parses_known_commands_with_or_without_colon` (lines 58–63): replace the `:build`/`chat` asserts with:
```rust
        assert_eq!(parse_command(":mode Superpowers"), Command::SwitchMode("Superpowers".into()));
        assert_eq!(parse_command("mode reload"), Command::ReloadModes);
        assert_eq!(parse_command("  :q "), Command::Quit);
```

- [ ] **Step 3: `route.rs` — `CycleMode` + drop Esc-from-Build**

In `crates/zoid-tui/src/route.rs`:
1. In `enum Action` rename `SwitchMode,` (line 19) → `CycleMode,`.
2. Line 174: `KeyCode::BackTab => return Action::CycleMode,`
3. Delete the Esc-from-Build block (lines ~179–182):
```rust
    // Esc returns to Chat from the Build surface (spec §6.2).
    if state.mode == Mode::Build && key.code == KeyCode::Esc {
        return Action::SwitchMode;
    }
```
4. Update the tests that reference the old names/behavior: line ~597 (`backtab_switches_mode…`) → assert `Action::CycleMode`; delete the Esc-in-Build test (~1032–1035); lines ~764 and ~1011 build a `Command::SwitchMode(Mode::Build)` — change those to `Command::ReloadModes` or a `SwitchMode("Chat".into())` as appropriate to what the test exercises, or remove if they only tested the old `:build` path. Any remaining `crate::state::Mode` reference in this file must go.

- [ ] **Step 4: `palette.rs` — mode rows from names**

In `crates/zoid-tui/src/palette.rs`:
1. Remove `use crate::state::Mode;`
2. Change the signature `pub fn all_items(mode: Mode, companion_on: bool)` → `pub fn all_items(active_mode: &str, mode_names: &[String], companion_on: bool)`.
3. Replace the `let (mode_label, mode_cmd) = match mode { … }` block (lines 57–61) and its single `PaletteItem` with a **row per other mode** plus a reload row. Where the old single mode `PaletteItem` was pushed, build:
```rust
    // One "Switch to <mode>" row per mode other than the active one, in order.
    let mut mode_rows: Vec<PaletteItem> = mode_names
        .iter()
        .filter(|n| n.as_str() != active_mode)
        .map(|n| PaletteItem {
            // Leak the label to satisfy the &'static str field (labels are built
            // once per palette open; acceptable). If the field is later widened to
            // String, drop the leak.
            label: Box::leak(format!("Switch to {n}").into_boxed_str()),
            command: Command::SwitchMode(n.clone()),
        })
        .collect();
    mode_rows.push(PaletteItem { label: "Reload modes", command: Command::ReloadModes });
```
   Then insert `mode_rows` into the returned `vec![…]` (e.g. extend after the session rows). Update the two call sites of `all_items` in `route.rs` (line ~444) and `render.rs` (line ~691) to pass `(&state.active_mode, &state.mode_names, state.companion_on)`.

> **Label lifetime:** `PaletteItem.label` is `&'static str`. The `Box::leak` above is a pragmatic fix for dynamic mode names. Preferred cleaner alternative (do this if quick): change `label` to `String` in the struct and update the ~6 static-str literals elsewhere in the file to `.to_string()`/`.into()`. Pick one; the leak is acceptable for a bounded, user-triggered set.

- [ ] **Step 5: `render.rs` — chip from mirror, error card, delete placeholder**

In `crates/zoid-tui/src/render.rs`:
1. Conversation body (line ~173): replace `Mode::Build => render_build_placeholder(frame, layout.conversation),` and its surrounding `match state.mode { Mode::Chat => {…}` so the Chat body renders unconditionally, **except** when `state.active_mode_broken`, in which case render an error card. Concretely, wrap the existing Chat rendering:
```rust
    if state.active_mode_broken {
        render_mode_error(frame, state, layout.conversation);
    } else {
        // …the existing Mode::Chat body block, unindented one level…
    }
```
   Delete the `Mode::Build => …` arm and the now-unused `fn render_build_placeholder`. Add:
```rust
/// The crafted error card shown when the active mode failed to load (spec §9).
fn render_mode_error(frame: &mut Frame, state: &ShellState, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            format!("⚠ mode '{}' failed to load", state.active_mode),
            Style::new().fg(color::BUILD_ACCENT),
        )),
        Line::from(Span::styled(
            "Fix its mode.md, then run  :mode reload",
            Style::new().fg(color::DIM),
        )),
    ];
    let p = ratatui::widgets::Paragraph::new(lines)
        .block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL).title(" mode error "));
    frame.render_widget(p, area);
}
```
   (Use the crate's existing import style for `Line`/`Span`/`Style`/`color`; match the top-of-file `use`s.)
2. Status chip (lines ~277–283): replace the `match state.mode { Mode::Chat => …, Mode::Build => … }` with a single dynamic chip:
```rust
    let chip = if state.active_mode_broken {
        format!(" ⚠ {} ", state.active_mode)
    } else {
        format!(" {} ", state.active_mode.to_uppercase())
    };
    let mut left = vec![Span::styled(chip, Style::new().fg(color::CHAT_ACCENT).bg(color::CHAT_BG))];
```
3. Right segment (lines ~308–311): replace the `match state.mode` with the Chat form unconditionally:
```rust
    let right = format!(" zoom {} ", view.zoom.label());
```
4. Remove any now-unused imports (`Mode`, `color::BUILD_BG` if unused, etc.) — `cargo build` will name them.

- [ ] **Step 6: Delete `AgentProfileRegistry` (M1)**

In `crates/zoid-core/src/agent_profile.rs`: delete `struct AgentProfileRegistry` (lines ~52–98) and its `#[cfg(test)] mod tests` case `registry_active_defaults_to_first_and_switches_by_name` (lines ~130–147). Keep `AgentProfile`, `allows`, `builtin`, and their tests. Confirm no remaining references: `git grep -n AgentProfileRegistry` must return nothing.

- [ ] **Step 7: Bin — sync mirror, handle cycle/switch/reload, persist, restore**

In `crates/zoid/src/main.rs`:
1. Add a helper to push the registry state onto the shell (place near `spawn_turn`):
```rust
/// Mirror the active mode + names onto the shell for the pure renderer/palette.
fn sync_mode_mirror(app: &mut App) {
    app.shell.active_mode = app.modes.active_name().to_string();
    app.shell.active_mode_broken = app.modes.active_is_broken();
    app.shell.mode_names = app.modes.names();
}
```
2. Call `sync_mode_mirror(&mut app);` once right after `let mut app = App { … };` (construction) so the chip is correct on boot.
3. **Restore on resume:** where the session id is resolved at startup (near where `skills`/`modes` are built), after `modes` exists and the session id is known, read + apply:
```rust
    if let Ok(Some(saved)) = app.session.get_active_mode(app.session_id).await {
        app.modes.set_active(&saved); // no-op if the mode vanished ⇒ stays Chat
        sync_mode_mirror(&mut app);
    }
```
   (Place this after `app` is constructed and before the event loop; `get_active_mode` is async — ensure it's in an async context, mirroring how `list_sessions` is called at startup.)
4. **Handle the actions/commands.** Find where `Action::SwitchMode` was handled (the bin matches routed `Action`s — `git grep -n "Action::SwitchMode" crates/zoid/src/main.rs`). Replace that handler with:
```rust
        Action::CycleMode => {
            app.modes.cycle_next();
            sync_mode_mirror(&mut app);
            persist_active_mode(&app).await;
        }
```
   And where `Command`s are executed, add arms:
```rust
        Command::SwitchMode(name) => {
            app.modes.set_active(&name);
            sync_mode_mirror(&mut app);
            persist_active_mode(&app).await;
        }
        Command::ReloadModes => {
            let prev = app.modes.active_name().to_string();
            let mode_dirs = zoid::mode_import::resolve_mode_dirs(
                &app.config.modes.source_dirs, &cfg_dir_for(&app), &cwd_for(&app), home_for().as_deref());
            app.modes = zoid::mode_import::build_mode_registry(&app.base_profile, &mode_dirs);
            app.modes.set_active(&prev); // preserve by name; no-op ⇒ Chat
            sync_mode_mirror(&mut app);
        }
```
   with a small helper:
```rust
async fn persist_active_mode(app: &App) {
    let _ = app.session.set_active_mode(app.session_id, app.modes.active_name().to_string()).await;
}
```
   `cfg_dir_for`/`cwd_for`/`home_for` are however the construction computed `cfg_dir`/`root`/`home` — reuse the same expressions inline rather than inventing helpers if simpler (the construction already has `cfg_dir`, `root`, `home`; if they're locals not on `App`, recompute them the same way, or stash `cfg_dir`/`root` onto `App` in Task 4). **Simplest:** in Task 4, also add `mode_dirs: Vec<PathBuf>` to `App` (computed once) and reuse it here for reload — avoids recomputing paths. If you do that, this arm is just `build_mode_registry(&app.base_profile, &app.mode_dirs)`.
5. Delete the old `toggle_mode`/`Mode` handling and any `app.shell.mode = …` / `set_mode` writes (the `git grep` from Task 4 lists them). Update the two `all_items(...)` call sites to the new signature (Step 4).

- [ ] **Step 8: Update `preview.rs` example**

`crates/zoid-tui/examples/preview.rs:191` calls `s.set_mode(Mode::Build)`. Replace with `s.active_mode = "Build".into();` (or delete the line if the example only demonstrated the Build surface, which no longer exists). Ensure the example compiles.

- [ ] **Step 9: Build the whole workspace, fix fallout**

Run: `cargo build --workspace`
Expected: clean. Chase any residual `Mode`/`toggle_mode`/`profiles`/`set_mode` references the compiler flags (there should be none).

- [ ] **Step 10: Run the full suite + lint**

Run: `cargo test --workspace`
Expected: PASS (snapshot tests may fail on the chip text — that's Task 7; if a snapshot asserts ` CHAT ` it will change to ` CHAT ` from the new dynamic path, which is byte-identical for Chat, so most should still pass. Broken/other-mode snapshots are added in Task 7.)

- [ ] **Step 11: Commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all
git add -A
git commit -m "feat: retire Chat/Build enum; cycle real modes (Shift+Tab), mirror+persist active mode, delete AgentProfileRegistry"
```

---

### Task 7: Fidelity snapshots, broken-mode card, cleanup, verification

**Files:**
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (chip + error-card snapshots)
- Modify: `crates/zoid/src/main.rs` (drop the now-dead `tools` App field if unused — see Task 4 Step 4)
- Test: `insta` snapshots

**Interfaces:** none new.

- [ ] **Step 1: Add a mode-chip snapshot (ready + broken)**

In `crates/zoid-tui/tests/shell_snapshot.rs`, add a test building a `ShellState` with `active_mode = "Superpowers"`, `active_mode_broken = false` and snapshot the status bar; and one with `active_mode_broken = true` asserting the ⚠ chip + the error card render. Follow the file's existing `TestBackend` + `assert_snapshot!` pattern (mirror an existing status-bar test). Example skeleton:

```rust
    #[test]
    fn status_chip_shows_active_mode() {
        let mut st = base_shell(); // however this file builds a ShellState
        st.active_mode = "Superpowers".into();
        st.mode_names = vec!["Chat".into(), "Superpowers".into()];
        let buf = render_to_buffer(&st); // the file's existing helper
        insta::assert_snapshot!(format!("{:#?}", buf));
    }

    #[test]
    fn broken_mode_shows_warn_chip_and_error_card() {
        let mut st = base_shell();
        st.active_mode = "Superpowers".into();
        st.active_mode_broken = true;
        let buf = render_to_buffer(&st);
        insta::assert_snapshot!(format!("{:#?}", buf));
    }
```

Use the actual helper names in that file (`base_shell`/`render_to_buffer` are placeholders for whatever it already uses).

- [ ] **Step 2: Review + accept snapshots**

Run: `cargo test -p zoid-tui`
Then: `cargo insta review` (accept the new/changed snapshots after eyeballing that the chip shows the mode name and the broken card shows the ⚠ + `:mode reload` hint). If `cargo-insta` isn't installed, inspect the `.snap.new` files and rename to `.snap`.

- [ ] **Step 3: Drop the dead `App.tools` field if unused**

`git grep -n "\.tools" crates/zoid/src/main.rs`. If `spawn_turn` no longer reads `app.tools` (Task 4 replaced it) and nothing else does, remove the `tools:` field from `struct App` (line ~1011) and its initializer (line ~1243). Rebuild.

- [ ] **Step 4: Full workspace verification**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 5: Manual smoke (documented, not automated)**

Build and run zoid. Create `./.zoid/modes/superpowers/mode.md` (`name: Superpowers`, body `Use your skills.`) with a `brainstorming/SKILL.md`. Verify: Shift+Tab cycles `CHAT → SUPERPOWERS → CHAT`; the chip updates; in Superpowers the model's system prompt carries the overlay and `invoke_skill("brainstorming")` resolves; back in Chat it doesn't; `:mode reload` picks up a newly-dropped folder without restart; a malformed `mode.md` shows the ⚠ chip + error card; quitting and resuming the repo lands back in the last mode.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test(tui): mode chip + broken-mode error-card snapshots; drop dead App.tools"
```

---

## Self-Review (completed against the spec)

**Spec coverage:**
- §3 minimum contract (`mode.md` = `parse_skill_md`) → T3 `load_mode`. §4 tiers/shadowing → T1 `effective_skills`. §5 architecture + per-turn snapshot + bin-composed overlay → T1 (`active_turn`/`overlay_prompt`), T4 (`spawn_turn`). §6 components → all tasks (store.rs migration = T5; `ShellState` mirror = T6; `Command` payload = T6). §7 discovery/config → T2 (`[modes]`), T3 (`resolve_mode_dirs`). §8 switch UX + enum retirement → T6. §9 error safety (Ready/Broken, error card) → T1/T3/T6. §10 hot reload → T6 Step 7 `ReloadModes`. §11 persistence + migration + restore-onto-Broken → T5, T6 Step 7. §12 seams (tools/model unparsed/unhonored; picker deferred) → T3 `load_mode` leaves `tools/model` default. §13 tests → each task's test steps (pure overlay on arbitrary base = T1; menu-after-overlay = covered by `chat_turn_config_with`'s existing test + T4 contract; snapshot fidelity = T7).
- Gilfoyle C1 (per-turn snapshot) = T4 Step 5 + T4 test. I1 (migration mechanism) = T5 Steps 3–4. I2 (bin composes overlay) = T1 `overlay_prompt` + T3 `load_mode`. I3 (`Command`→String + mirror) = T6 Steps 1–2. M1 (delete `AgentProfileRegistry`) = T6 Step 6.

**Placeholder scan:** the only intentional "adapt to the actual file" notes are in T5 Step 6 (session actor message names) and T6 Steps 4–5/7 (exact helper/import names, palette label lifetime) — each names the concrete pattern to mirror (`touch_session`'s arm; the file's `use` block) rather than leaving logic unwritten. All code steps carry real code.

**Type consistency:** `active_turn` returns owned `(AgentProfile, SkillRegistry)` (used T1/T4). `effective_skills(global, active)` arg order consistent T1↔T4. `Command::SwitchMode(String)` + `ReloadModes` and `Action::CycleMode` consistent across command.rs/route.rs/palette.rs/main.rs. `set_active_mode(id, &str)`/`get_active_mode(id) -> Option<String>` consistent store↔session↔bin.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-05-mode-promotion-quickswitch.md`.
