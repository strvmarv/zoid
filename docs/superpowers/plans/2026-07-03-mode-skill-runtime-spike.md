# Mode / Skill Runtime Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that a small local model (`glm-5.2:cloud`) can drive a skill graph — call an `invoke_skill` tool and follow a skill body's instruction to invoke another skill — inside zoid's existing agent turn loop, and ship the minimal foundation that makes that answerable.

**Architecture:** Two prompt layers. A *mode* is ambient: the active `AgentProfile` supplies the turn's system prompt (Slice 0 — the turn reads a profile, not a hard-coded const). `invoke_skill` is transient: a tool whose *result* is a skill's body text, injected as a `Message::tool`, so chaining is just the model calling it again. The "runtime" rides the existing tool-call/tool-result loop in `run_agent_turn` — no new loop machinery.

**Tech Stack:** Rust 2021 workspace. `zoid-core` (pure domain types), `zoid-tools` (the `Tool` trait; depends only on `zoid-provider`), `zoid-provider` (`Provider`/`ToolSpec`), `zoid` bin+lib (agent loop, composition root). Tests via `cargo test`, deterministic `ScriptedProvider`.

## Global Constraints

- **No new `zoid-tools → zoid-core` crate edge.** `zoid-tools` depends only on `zoid-provider`. The `invoke_skill` tool is implemented in the **`zoid` bin/lib** (the composition root, which already depends on both crates), against the public `zoid_tools::Tool` trait.
- **Zero regression on the default path.** The default mode profile carries the current `SYSTEM_PROMPT` verbatim and an empty tool allow-list (empty = every tool permitted, per `AgentProfile::allows`). With no skills menu it must produce a `TurnConfig` identical to today's.
- **Every tool failure returns a `ToolOutput::err`, never a panic or `Err`.** Mirrors the existing convention (`run_tool` returns an error `ToolOutput` for unknown tools; providers prefer `ProviderEvent::Error` over `Err`).
- **Built-in skill bodies reference ONLY tools that exist in zoid** (`invoke_skill`, `write_file`) — the spike measures "can the model drive the graph," not ghost-tool tolerance.
- **Do not touch the `Mode` UI enum** (`crates/zoid-tui/src/state.rs`, Chat/Build) — modes-as-agents are a separate concept and this slice leaves that enum alone.
- **Commit messages:** conventional-commit style, imperative subject. **Do NOT add any `Co-Authored-By` or co-author trailer** (user global rule).

### Deliberate deviations from the spec (flagged for the reviewer)

1. **`chat_turn_config` is not re-signatured in place.** The spec wrote a single `chat_turn_config(profile, menu)`. There are 11 existing callers (10 tests + one unit test) that don't care about modes. To avoid churning all of them and risking a missed caller, this plan **keeps zero-arg `chat_turn_config()` as a thin default** (delegates to the new builder with the default profile + empty menu — byte-identical output) and **adds `chat_turn_config_with(profile, skill_menu)`** for the production path. Only `spawn_turn` calls the new one. Same intent, zero regression.
2. **`spawn_turn` tool-allow-list filtering is deferred.** The spec listed filtering `app.tools` by the active profile at `spawn_turn`. This slice ships only the allow-all default profile, so filtering would be untestable dead code (and `Box<dyn Tool>` isn't `Clone`, so it can't filter the shared `Arc` cheaply). The allow-list lives on the profile and is already enforced on the subagent path (`subagent.rs:117`); applying it at `spawn_turn` moves to the slice that first introduces a restricted mode.

---

## File Structure

| File | New/Modify | Responsibility |
|---|---|---|
| `crates/zoid-core/src/skill.rs` | **new** | `Skill` + `SkillRegistry` domain types; `builtin()` = the 2 chaining spike skills. |
| `crates/zoid-core/src/lib.rs` | modify | Register `pub mod skill;`. |
| `crates/zoid-core/src/agent_profile.rs` | modify | Add `AgentProfileRegistry` (active pointer + lookup/switch). |
| `crates/zoid/src/invoke_skill.rs` | **new** | `InvokeSkillTool` (impl `Tool`) + `chat_tools()` composition helper. |
| `crates/zoid/src/lib.rs` | modify | Register `pub mod invoke_skill;`. |
| `crates/zoid/src/agent.rs` | modify | Add `default_profile()`; add `chat_turn_config_with(profile, menu)`; make `chat_turn_config()` delegate. |
| `crates/zoid/src/main.rs` | modify | `App` gains `profiles` + `skills`; construction builds the skill registry + `chat_tools`; `spawn_turn` reads the active profile + menu. |
| `crates/zoid/tests/mode_skill_spike.rs` | **new** | Deterministic wiring test: a scripted `invoke_skill` call flows its body back into the loop. |
| `docs/superpowers/runbooks/2026-07-03-mode-skill-spike-smoke.md` | **new** | The Tier-2 real-model go/no-go runbook + recorded outcome. |

---

## Task 1: Skill + SkillRegistry (zoid-core)

**Files:**
- Create: `crates/zoid-core/src/skill.rs`
- Modify: `crates/zoid-core/src/lib.rs:4` (add module registration)

**Interfaces:**
- Consumes: nothing (pure new types).
- Produces:
  - `pub struct Skill { pub name: String, pub description: String, pub body: String }`
  - `pub struct SkillRegistry` with `new(Vec<Skill>) -> Self`, `builtin() -> Self`, `get(&self, &str) -> Option<&Skill>`, `names(&self) -> Vec<String>`, `menu(&self) -> String`.

- [ ] **Step 1: Register the module**

In `crates/zoid-core/src/lib.rs`, add after line 4 (`pub mod agent_profile;`):

```rust
pub mod skill;
```

- [ ] **Step 2: Write the failing tests + type stubs**

Create `crates/zoid-core/src/skill.rs`:

```rust
//! Skills the agent loop pulls on demand via the `invoke_skill` tool. A skill is
//! a named unit of instructions whose body is returned to the model as a tool
//! result — mirroring Claude Code's Skill tool. v1 ships two hand-written
//! built-in skills that chain (spike-plan → spike-implement) to prove the
//! runtime; the SKILL.md importer is a later slice. Pure: no provider/process deps.

/// A single named skill: its one-line menu description and its full body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// The skills available to the current session.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Build a registry from an explicit skill list.
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    /// The two hand-written built-in spike skills. `spike-plan` ends by
    /// instructing the model to invoke `spike-implement` — the chaining proof.
    /// Both bodies reference ONLY tools that exist in zoid (`invoke_skill`,
    /// `write_file`).
    pub fn builtin() -> Self {
        Self::new(vec![
            Skill {
                name: "spike-plan".into(),
                description:
                    "Draft the plan for the spike task, then hand off to spike-implement.".into(),
                body: "You are executing the 'spike-plan' skill.\n\n\
                    The task: create a file at ./spike-artifact.txt whose only line is: spike ok\n\n\
                    Step 1: restate that plan in one short sentence.\n\
                    Step 2: to carry the plan out, call the invoke_skill tool with name \
                    \"spike-implement\".\n\
                    Do NOT write the file yourself in this step — spike-implement does that."
                    .into(),
            },
            Skill {
                name: "spike-implement".into(),
                description: "Write the spike artifact file described by the plan.".into(),
                body: "You are executing the 'spike-implement' skill.\n\n\
                    Create the file ./spike-artifact.txt with exactly one line of content: spike ok\n\
                    Use the write_file tool. Then confirm in one sentence that you wrote it."
                    .into(),
            },
        ])
    }

    /// Look up a skill by exact name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// All skill names in registry order.
    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|s| s.name.clone()).collect()
    }

    /// The menu injected into a mode's system prompt: one `- name: description`
    /// line per skill, in registry order. Empty string when there are no skills.
    pub fn menu(&self) -> String {
        self.skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_both_spike_skills_that_chain() {
        let r = SkillRegistry::builtin();
        assert_eq!(
            r.names(),
            vec!["spike-plan".to_string(), "spike-implement".to_string()]
        );
        let plan = r.get("spike-plan").unwrap();
        assert!(plan.body.contains("spike-implement"), "spike-plan must chain to spike-implement");
        assert!(plan.body.contains("invoke_skill"));
        let imp = r.get("spike-implement").unwrap();
        assert!(imp.body.contains("write_file"));
        assert!(imp.body.contains("spike-artifact.txt"));
    }

    #[test]
    fn get_misses_unknown_name() {
        assert!(SkillRegistry::builtin().get("nope").is_none());
    }

    #[test]
    fn menu_renders_one_line_per_skill() {
        let menu = SkillRegistry::builtin().menu();
        assert!(menu.contains("- spike-plan: "));
        assert!(menu.contains("- spike-implement: "));
        assert_eq!(menu.lines().count(), 2);
    }

    #[test]
    fn empty_registry_menu_is_empty_string() {
        assert_eq!(SkillRegistry::new(vec![]).menu(), "");
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid-core skill`
Expected: 4 tests pass (`builtin_has_both_spike_skills_that_chain`, `get_misses_unknown_name`, `menu_renders_one_line_per_skill`, `empty_registry_menu_is_empty_string`).

(This task writes the implementation and tests together because the impl is small and total; the RED signal is the pre-implementation compile failure if you stub first. If you prefer strict RED: paste only the `#[cfg(test)] mod tests` block plus empty struct defs first, run `cargo test -p zoid-core skill` → compile error, then fill the impl.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-core/src/skill.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): SkillRegistry + built-in chaining spike skills"
```

---

## Task 2: AgentProfileRegistry (zoid-core)

**Files:**
- Modify: `crates/zoid-core/src/agent_profile.rs` (append registry type + one test)

**Interfaces:**
- Consumes: existing `AgentProfile` (same file).
- Produces: `pub struct AgentProfileRegistry` with `new(Vec<AgentProfile>) -> Self`, `active(&self) -> &AgentProfile`, `by_name(&self, &str) -> Option<&AgentProfile>`, `set_active(&mut self, &str) -> bool`.

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `crates/zoid-core/src/agent_profile.rs` (before its closing `}`):

```rust
    #[test]
    fn registry_active_defaults_to_first_and_switches_by_name() {
        let mk = |name: &str| AgentProfile {
            name: name.into(),
            description: "d".into(),
            system_prompt: "s".into(),
            tools: vec![],
            model: None,
        };
        let mut reg = AgentProfileRegistry::new(vec![mk("default"), mk("plan")]);
        assert_eq!(reg.active().name, "default");
        assert!(reg.set_active("plan"));
        assert_eq!(reg.active().name, "plan");
        assert!(!reg.set_active("ghost")); // miss returns false
        assert_eq!(reg.active().name, "plan"); // and leaves active unchanged
        assert!(reg.by_name("default").is_some());
        assert!(reg.by_name("ghost").is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core registry_active`
Expected: FAIL — compile error, `cannot find type AgentProfileRegistry`.

- [ ] **Step 3: Implement the registry**

Append to `crates/zoid-core/src/agent_profile.rs`, after the `impl AgentProfile { … }` block (before the `#[cfg(test)]` module):

```rust
/// An ordered set of `AgentProfile`s with one marked active. v1 is seeded by the
/// bin with a single "default" profile; the Shift+Tab mode switch (later slice)
/// drives `set_active`. `active()` never fails — `new` requires a non-empty list
/// and the bin always seeds the default.
#[derive(Debug, Clone)]
pub struct AgentProfileRegistry {
    profiles: Vec<AgentProfile>,
    active: usize,
}

impl AgentProfileRegistry {
    /// Build a registry from a non-empty profile list; the first profile is
    /// active. Panics if `profiles` is empty (a programming error — the bin
    /// always seeds the default profile).
    pub fn new(profiles: Vec<AgentProfile>) -> Self {
        assert!(
            !profiles.is_empty(),
            "AgentProfileRegistry needs at least one profile"
        );
        Self { profiles, active: 0 }
    }

    /// The currently active profile (never `None`).
    pub fn active(&self) -> &AgentProfile {
        &self.profiles[self.active]
    }

    /// Look up a profile by name.
    pub fn by_name(&self, name: &str) -> Option<&AgentProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Make the named profile active. Returns `false` (and leaves the active
    /// pointer unchanged) if no profile has that name.
    pub fn set_active(&mut self, name: &str) -> bool {
        match self.profiles.iter().position(|p| p.name == name) {
            Some(i) => {
                self.active = i;
                true
            }
            None => false,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core registry_active`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/agent_profile.rs
git commit -m "feat(core): AgentProfileRegistry with active pointer + by_name/set_active"
```

---

## Task 3: InvokeSkillTool + chat_tools (zoid bin/lib)

**Files:**
- Create: `crates/zoid/src/invoke_skill.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod invoke_skill;`)

**Interfaces:**
- Consumes: `zoid_core::skill::SkillRegistry` (Task 1); `zoid_tools::{Tool, ToolOutput}`; `zoid_provider::ToolSpec`.
- Produces:
  - `pub struct InvokeSkillTool` with `new(Arc<SkillRegistry>) -> Self`, implementing `zoid_tools::Tool` under name `"invoke_skill"`.
  - `pub fn chat_tools(skills: Arc<SkillRegistry>) -> Vec<Box<dyn zoid_tools::Tool>>` — the base registry plus `invoke_skill`.

- [ ] **Step 1: Register the module**

In `crates/zoid/src/lib.rs`, add (keep the list alphabetical-ish; place after `pub mod dbglog;`):

```rust
pub mod invoke_skill;
```

- [ ] **Step 2: Write the tool + helper + tests**

Create `crates/zoid/src/invoke_skill.rs`:

```rust
//! The `invoke_skill` tool: the model calls it with a skill name; the tool
//! returns that skill's body as its result (fed back as a `Message::tool`), which
//! the model then follows. Chaining is just the model calling this again.
//! Implemented in the bin (not `zoid-tools`) so the tool crate keeps its
//! `zoid-provider`-only dependency — the bin is the composition root that owns
//! the `SkillRegistry`.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use zoid_core::skill::SkillRegistry;
use zoid_provider::ToolSpec;
use zoid_tools::{Tool, ToolOutput};

/// A `Tool` that resolves a skill name to its body from the injected registry.
pub struct InvokeSkillTool {
    skills: Arc<SkillRegistry>,
}

impl InvokeSkillTool {
    pub fn new(skills: Arc<SkillRegistry>) -> Self {
        Self { skills }
    }
}

impl Tool for InvokeSkillTool {
    fn name(&self) -> &str {
        "invoke_skill"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "invoke_skill".into(),
            description: "Load a skill by name to get its full instructions, then follow them. \
                Available skills are listed in your system prompt. A skill's instructions may tell \
                you to invoke another skill — do so by calling this tool again."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The exact skill name to load." }
                },
                "required": ["name"]
            }),
        }
    }

    fn run(&self, args: &Value, _cwd: &Path) -> ToolOutput {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => {
                return ToolOutput::err(format!(
                    "invoke_skill: missing or empty 'name'. Available: {}",
                    self.skills.names().join(", ")
                ))
            }
        };
        match self.skills.get(name) {
            Some(skill) => ToolOutput::ok(skill.body.clone()),
            None => ToolOutput::err(format!(
                "unknown skill '{name}'. Available: {}",
                self.skills.names().join(", ")
            )),
        }
    }
}

/// The Chat tool set: the standard curated registry plus the `invoke_skill` tool
/// bound to `skills`. Extracted from `App` construction so it is unit-testable.
pub fn chat_tools(skills: Arc<SkillRegistry>) -> Vec<Box<dyn Tool>> {
    let mut tools = zoid_tools::registry();
    tools.push(Box::new(InvokeSkillTool::new(skills)));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> InvokeSkillTool {
        InvokeSkillTool::new(Arc::new(SkillRegistry::builtin()))
    }

    #[test]
    fn returns_body_for_known_skill() {
        let out = tool().run(&json!({ "name": "spike-plan" }), Path::new("."));
        assert!(!out.is_error);
        assert!(out.text.contains("spike-implement")); // the chaining instruction
    }

    #[test]
    fn unknown_skill_is_error_listing_available() {
        let out = tool().run(&json!({ "name": "nope" }), Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("unknown skill 'nope'"));
        assert!(out.text.contains("spike-plan"));
    }

    #[test]
    fn missing_name_is_error() {
        let out = tool().run(&json!({}), Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("missing or empty 'name'"));
    }

    #[test]
    fn tool_name_and_spec_agree() {
        assert_eq!(tool().name(), "invoke_skill");
        assert_eq!(tool().spec().name, "invoke_skill");
    }

    #[test]
    fn chat_tools_includes_invoke_skill_and_base_registry() {
        let tools = chat_tools(Arc::new(SkillRegistry::builtin()));
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"invoke_skill"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"read_file"));
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid --lib invoke_skill`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/invoke_skill.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): invoke_skill tool + chat_tools composition helper"
```

---

## Task 4: default_profile() (zoid agent.rs)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (add import + `default_profile()` + one test)

**Interfaces:**
- Consumes: `SYSTEM_PROMPT` (same file); `zoid_core::agent_profile::AgentProfile`.
- Produces: `pub fn default_profile() -> AgentProfile` — name `"default"`, `system_prompt = SYSTEM_PROMPT`, empty tool allow-list (all permitted), `model: None`.

- [ ] **Step 1: Add the import**

At the top of `crates/zoid/src/agent.rs`, with the other `use zoid_core::…` lines (after line 15), add:

```rust
use zoid_core::agent_profile::AgentProfile;
```

- [ ] **Step 2: Write the failing test**

In the `#[cfg(test)] mod tests` block of `crates/zoid/src/agent.rs` (around line 585+; it already `use super::*;`), add:

```rust
    #[test]
    fn default_profile_carries_system_prompt_and_allows_all_tools() {
        let p = default_profile();
        assert_eq!(p.name, "default");
        assert_eq!(p.system_prompt, SYSTEM_PROMPT);
        assert!(p.tools.is_empty(), "empty allow-list = all tools permitted");
        assert!(p.allows("invoke_skill"));
        assert!(p.allows("write_file"));
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid --lib default_profile`
Expected: FAIL — compile error, `cannot find function default_profile`.

- [ ] **Step 4: Implement `default_profile()`**

In `crates/zoid/src/agent.rs`, immediately after the `SYSTEM_PROMPT` const (after line 28), add:

```rust
/// The default Chat mode profile: the standard zoid system prompt with an
/// unrestricted tool set (empty allow-list = every tool permitted, per
/// `AgentProfile::allows`). Seeds the `AgentProfileRegistry`; reproduces
/// pre-mode behavior exactly.
pub fn default_profile() -> AgentProfile {
    AgentProfile {
        name: "default".into(),
        description: "General terminal coding assistant.".into(),
        system_prompt: SYSTEM_PROMPT.to_string(),
        tools: vec![], // empty = every tool permitted
        model: None,
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zoid --lib default_profile`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): default_profile() seeds the mode registry with today's prompt"
```

---

## Task 5: chat_turn_config_with (zoid agent.rs)

**Files:**
- Modify: `crates/zoid/src/agent.rs:44-51` (`chat_turn_config`) + add `chat_turn_config_with` + tests

**Interfaces:**
- Consumes: `AgentProfile` (Task 4 import), `TurnConfig`, `default_profile()`.
- Produces:
  - `pub fn chat_turn_config_with(profile: &AgentProfile, skill_menu: &str) -> TurnConfig` — `system` = profile prompt, plus the menu under a header when the menu is non-empty.
  - `chat_turn_config()` unchanged signature, now delegating to `chat_turn_config_with(&default_profile(), "")`.

- [ ] **Step 1: Write the failing tests**

In the `#[cfg(test)] mod tests` block of `crates/zoid/src/agent.rs`, add:

```rust
    #[test]
    fn chat_turn_config_with_embeds_menu_in_system() {
        let p = default_profile();
        let cfg = chat_turn_config_with(&p, "- spike-plan: do the thing");
        assert!(cfg.system.starts_with(SYSTEM_PROMPT));
        assert!(cfg.system.contains("## Available skills"));
        assert!(cfg.system.contains("- spike-plan: do the thing"));
    }

    #[test]
    fn chat_turn_config_with_empty_menu_is_just_prompt() {
        let p = default_profile();
        let cfg = chat_turn_config_with(&p, "");
        assert_eq!(cfg.system, SYSTEM_PROMPT);
    }

    #[test]
    fn zero_arg_chat_turn_config_matches_default_profile_no_menu() {
        // The zero-arg convenience must stay byte-identical to the old behavior.
        assert_eq!(chat_turn_config().system, SYSTEM_PROMPT);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib chat_turn_config`
Expected: FAIL — compile error, `cannot find function chat_turn_config_with`.

- [ ] **Step 3: Add `chat_turn_config_with` and make `chat_turn_config` delegate**

Replace the existing `chat_turn_config` function (`crates/zoid/src/agent.rs:43-51`) with:

```rust
/// The orchestrator (Chat) turn config for an explicit mode profile + skill menu.
/// `system` is the profile's prompt; when `skill_menu` is non-empty it is
/// appended under a header so the model knows what it can `invoke_skill`.
pub fn chat_turn_config_with(profile: &AgentProfile, skill_menu: &str) -> TurnConfig {
    let system = if skill_menu.is_empty() {
        profile.system_prompt.clone()
    } else {
        format!(
            "{}\n\n## Available skills — call invoke_skill(name):\n{}",
            profile.system_prompt, skill_menu
        )
    };
    TurnConfig {
        system,
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
        policy: zoid_core::assembler::ContextPolicy::default(),
    }
}

/// The default Chat turn config: the `default_profile()` with no skill menu.
/// Kept zero-arg for the many callers (tests) that don't exercise modes;
/// byte-identical to the pre-mode behavior.
pub fn chat_turn_config() -> TurnConfig {
    chat_turn_config_with(&default_profile(), "")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib chat_turn_config`
Expected: PASS (3 new tests). The existing `agent.rs:592` test that calls `chat_turn_config()` still compiles and passes.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): chat_turn_config_with(profile, menu); zero-arg delegates"
```

---

## Task 6: Wire modes + skills into the App and the turn (zoid main.rs)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App` struct (~690), real construction (~870-875), test construction (~2821), `spawn_turn` (~2397-2422)

**Interfaces:**
- Consumes: `AgentProfileRegistry` (Task 2), `SkillRegistry` (Task 1), `chat_tools` (Task 3), `default_profile` + `chat_turn_config_with` (Tasks 4-5).
- Produces: an `App` whose `tools` include `invoke_skill` and whose turns are built from `app.profiles.active()` + `app.skills.menu()`.

- [ ] **Step 1: Add the two `App` fields**

In the `struct App { … }` definition (`crates/zoid/src/main.rs:690`), after the `tools` field (line 695), add:

```rust
    /// Available mode profiles with the active one marked; drives the turn's
    /// system prompt. v1 holds only the default profile.
    profiles: zoid_core::agent_profile::AgentProfileRegistry,
    /// Skills the `invoke_skill` tool can load; also rendered as the menu the
    /// active mode's system prompt advertises.
    skills: std::sync::Arc<zoid_core::skill::SkillRegistry>,
```

- [ ] **Step 2: Build the skill registry + tools in the real `App` construction**

In `crates/zoid/src/main.rs`, immediately before the `let mut app = App {` line (currently ~870), add:

```rust
    let skills = std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin());
```

Then change the `tools:` field (line 875) from:

```rust
        tools: Arc::new(zoid_tools::registry()),
```

to:

```rust
        tools: Arc::new(zoid::invoke_skill::chat_tools(skills.clone())),
```

And add these two fields inside the same `App { … }` literal (e.g. right after the `tools:` line):

```rust
        profiles: zoid_core::agent_profile::AgentProfileRegistry::new(vec![
            zoid::agent::default_profile(),
        ]),
        skills,
```

- [ ] **Step 3: Fix the test `App` literal**

In `crates/zoid/src/main.rs` at the test `App` construction (~2821, the `tools: Arc::new(Vec::new()),` line), add the two new fields alongside it:

```rust
            tools: Arc::new(Vec::new()),
            profiles: zoid_core::agent_profile::AgentProfileRegistry::new(vec![
                zoid::agent::default_profile(),
            ]),
            skills: std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
```

- [ ] **Step 4: Make `spawn_turn` read the active profile + menu**

In `crates/zoid/src/main.rs` `spawn_turn` (line 2405), replace:

```rust
    let mut turn_config = zoid::agent::chat_turn_config();
```

with:

```rust
    let profile = app.profiles.active();
    let menu = app.skills.menu();
    let mut turn_config = zoid::agent::chat_turn_config_with(profile, &menu);
```

(`profile` borrows `app` and `menu` is owned; both are consumed building `turn_config` before the `tokio::spawn`, so no lifetime escapes the closure.)

- [ ] **Step 5: Build and run the full bin test suite to verify no regression**

Run: `cargo build -p zoid`
Expected: compiles clean.

Run: `cargo test -p zoid`
Expected: all existing bin tests pass, plus the Task 3-5 unit tests. No test references a removed symbol.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): App carries mode profiles + skills; turns read the active mode"
```

---

## Task 7: Deterministic chaining-wiring integration test (zoid tests)

**Files:**
- Create: `crates/zoid/tests/mode_skill_spike.rs`

**Interfaces:**
- Consumes: `zoid::invoke_skill::chat_tools`, `zoid_core::skill::SkillRegistry`, `zoid::agent::{run_agent_turn, chat_turn_config_with, default_profile, AgentUpdate}`, `zoid_testkit`, the `ScriptedProvider` pattern from `crates/zoid/tests/agent_loop.rs`.
- Produces: proof that when the model emits `invoke_skill(spike-plan)`, the loop records a non-error `ToolResult` carrying the skill body and feeds it back into the next provider request as a `MsgRole::Tool` message.

This is a wiring test — it uses a *scripted* provider, so it proves the plumbing, NOT that a real model chooses to chain (that is Task 8).

- [ ] **Step 1: Write the test**

Create `crates/zoid/tests/mode_skill_spike.rs`. It reuses the exact harness shape from `agent_loop.rs` (a `ScriptedProvider` that replays one event list per `stream()` call and records requests). The new content is the script (a single `invoke_skill` tool call) and the assertions:

```rust
//! Wiring proof for the mode/skill runtime spike: a scripted `invoke_skill` call
//! must have its skill body recorded as a non-error ToolResult AND fed back into
//! the next provider request as a Tool message. Deterministic — no real model.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::skill::SkillRegistry;
use zoid_provider::{CompletionRequest, MsgRole, Provider, ProviderEvent};

/// Replays one scripted stream per `stream()` call and captures every request.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        self.requests.lock().unwrap().push(req.clone());
        let script = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

#[tokio::test]
async fn invoke_skill_body_flows_back_into_the_loop() {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model loads the spike-plan skill, then ends its stream.
            vec![
                zoid_testkit::tool_call("invoke_skill", serde_json::json!({ "name": "spike-plan" })),
                ProviderEvent::Done,
            ],
            // Turn 2: with the skill body in context, the model replies in text.
            vec![zoid_testkit::text("planned"), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let skills = Arc::new(SkillRegistry::builtin());
    let tools = Arc::new(zoid::invoke_skill::chat_tools(skills.clone()));

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "plan and implement the spike task".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    run_agent_turn(
        zoid::agent::chat_turn_config_with(&zoid::agent::default_profile(), &skills.menu()),
        provider.clone(),
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();
    drain.await.unwrap();

    // 1) The loop recorded a non-error ToolResult for invoke_skill carrying the body.
    let log = session.snapshot().await.unwrap();
    let body_result = log.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult {
            name,
            output,
            is_error,
            ..
        } if name == "invoke_skill" => Some((output.clone(), *is_error)),
        _ => None,
    });
    let (output, is_error) = body_result.expect("expected an invoke_skill ToolResult");
    assert!(!is_error, "invoke_skill should succeed for a known skill");
    assert!(
        output.contains("spike-implement"),
        "the returned body must be spike-plan's (which chains to spike-implement)"
    );

    // 2) The skill body was fed back into the second provider request as a Tool message.
    let captured = provider.requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected a tool-call turn + a follow-up turn");
    assert!(
        captured[1]
            .messages
            .iter()
            .any(|m| m.role == MsgRole::Tool && m.content.contains("spike-implement")),
        "second request must carry the skill body back as a Tool message"
    );
}
```

Note for the implementer: confirm the `zoid_testkit` helper names (`tool_call`, `text`) against `crates/zoid/tests/agent_loop.rs` (lines ~68, 72) and `Event::new` argument order against that same file (lines ~81-88) — this test mirrors them exactly. If `SessionHandle::spawn(":memory:")` differs in your tree, copy the exact session setup used at `agent_loop.rs:78`.

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p zoid --test mode_skill_spike`
Expected: PASS — `invoke_skill_body_flows_back_into_the_loop`.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/mode_skill_spike.rs
git commit -m "test(zoid): invoke_skill body flows back into the agent loop (wiring proof)"
```

---

## Task 8: Real-model go/no-go smoke runbook + recorded outcome

**Files:**
- Create: `docs/superpowers/runbooks/2026-07-03-mode-skill-spike-smoke.md`

This is the Tier-2 deliverable: the decision gate for the whole direction. It is a manual protocol against `glm-5.2:cloud` (real network + subscription), plus a place to record the observed outcome. No automated test can answer the behavioral question.

- [ ] **Step 1: Write the runbook**

Create `docs/superpowers/runbooks/2026-07-03-mode-skill-spike-smoke.md`:

```markdown
# Mode/Skill Runtime Spike — Go/No-Go Smoke

**Purpose:** Answer the one non-unit-testable question: will `glm-5.2:cloud`
actually call `invoke_skill` and follow a skill body's instruction to invoke
another skill (A→B), then act?

## Preconditions

- `OLLAMA_API_KEY` is set (Ollama Cloud native provider; default `glm-5.2:cloud`).
- Built from the branch carrying Tasks 1-7. `cargo test` green.
- Run in a scratch directory (the spike writes `./spike-artifact.txt`).

## Protocol

1. Launch zoid: `cargo run -p zoid` (or the built binary) in a scratch dir.
2. Confirm the provider line shows Ollama / `glm-5.2:cloud`.
3. Send exactly: `Plan and implement the spike task.`
4. Observe the tool calls in order.

## Outcome rubric

- **PASS** — the model calls `invoke_skill("spike-plan")`, then (following that
  body) `invoke_skill("spike-implement")`, then `write_file`, and
  `./spike-artifact.txt` contains `spike ok`. The full A→B→work chain, unattended.
- **PARTIAL** — invokes `spike-plan` once but does not chain to `spike-implement`.
- **FAIL** — never calls `invoke_skill`; answers inline.

## Decision gate

- **PASS** → build the SKILL.md importer + Shift+Tab quick-switch slices with confidence.
- **PARTIAL** → the runtime needs prompt/menu tuning (stronger menu framing, an
  explicit "you must invoke a skill" nudge) before further investment.
- **FAIL** → the "consume the methodology" vision is disconfirmed on small local
  models; fall back to modes-as-prompt-overlays (a different, smaller product).

## Recorded outcome

- Date run:
- Model / build commit:
- Observed tool-call sequence:
- Verdict (PASS / PARTIAL / FAIL):
- Notes / next action:
```

- [ ] **Step 2: Commit the runbook**

```bash
git add docs/superpowers/runbooks/2026-07-03-mode-skill-spike-smoke.md
git commit -m "docs(runbook): mode/skill spike go/no-go smoke protocol"
```

- [ ] **Step 3: Run the smoke and record the outcome**

Follow the runbook against `glm-5.2:cloud`. Fill in the "Recorded outcome" section with the observed tool-call sequence and the PASS/PARTIAL/FAIL verdict, then commit:

```bash
git add docs/superpowers/runbooks/2026-07-03-mode-skill-spike-smoke.md
git commit -m "docs(runbook): record mode/skill spike go/no-go outcome"
```

This recorded verdict is the exit criterion for the slice and the input to planning the next one.

---

## Final verification (whole slice)

- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo fmt --all --check` — clean.
- [ ] The go/no-go verdict is recorded in the runbook.
