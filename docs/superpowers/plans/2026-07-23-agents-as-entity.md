# Agents as an Entity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make subagent agent profiles a first-class filesystem entity — load `agent.md` files from disk into a named registry at startup, expose a `list_agents` tool, and let `dispatch_subagent` pick a profile by name.

**Architecture:** Mirror the proven skill/mode pattern: a pure `AgentRegistry` + `parse_agent_md` in `zoid-core`, a filesystem adapter `agent_import.rs` in the bin, an `AgentsConfig` config section, a `ListAgents` tool in `zoid-tools`, an `agent` parameter on `DispatchSubagent`, and dispatch-site resolution in `agent.rs` against an `Arc<AgentRegistry>` threaded through `TurnConfig`.

**Tech Stack:** Rust, serde/toml for config, `tempfile` for filesystem tests, `zoid-provider` `ToolSpec` for tool schemas.

## Spec Amendment (decided with the user)

The spec's "Seamed Fields" section says `tools` and `model` are parsed/stored but the runtime does NOT act on them. **We are deviating from that wording:** imported agents' `tools` allow-list and `model` override ARE honored at dispatch time, because the existing `run_subagent` already enforces `profile.allows()` (tool filtering) and `profile.model` (model override). The built-in `delegate` keeps its curated 7-tool list and `model: None`, so its behavior is unchanged. No seaming code is added; the parsed values flow straight through to `run_subagent`. This is more useful than seaming and requires no extra code.

> **Spec supersession note:** the spec document itself (`docs/superpowers/specs/2026-07-23-agents-as-entity-design.md`) still says "seamed" in §Decisions (rows for `tools`/`model`), §Seamed Fields, and §Out of Scope ("Enforcing the `tools` allow-list … is a follow-up slice"). For **agents** (not modes) those sections are superseded by this amendment — enforcement is already live via `run_subagent`. Modes remain seamed (mode_import.rs deliberately zeroes `tools`/`model` so `run_subagent`'s enforcement is a no-op for them). The spec is NOT edited in this plan; this note is the contract. If you want the spec amended in lockstep, do that as a separate doc edit.

## Global Constraints

- The `AgentProfile` struct (`crates/zoid-core/src/agent_profile.rs`) is **unchanged** — it already carries `name`, `description`, `system_prompt`, `tools: Vec<String>`, `model: Option<String>`.
- File format: frontmatter + markdown body, `---`-fenced YAML-scalar pattern identical to `SKILL.md`/`mode.md`, with `tools` (a YAML-style `- item` list) and `model` (scalar) as additional frontmatter fields.
- File layout: `<dir>/<agent-name>/agent.md` (folder-per-entity), plus one level of pack nesting (`<root>/<pack>/<agent>/agent.md`).
- Discovery dirs (unioned): `<user_cfg_dir>/agents`, `<cwd>/.zoid/agents`, plus configurable `[agents] source_dirs`.
- Built-in `delegate` is pre-seeded at registry index 0; first-wins collision protection means an import named `delegate` is silently skipped.
- Bad inputs return a result, never abort startup: missing dirs skipped silently; unreadable dir/file or malformed `agent.md` skipped with an `eprintln!` warning.
- Unknown agent name at dispatch time → `ToolResult` error listing available agents (model self-corrects). Absent `agent` param defaults to `"delegate"`.

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/zoid-core/src/agent_profile.rs` | `AgentRegistry` + `ParsedAgent` + `parse_agent_md` (pure, unit-tested) | Modify (add types/functions + tests) |
| `crates/zoid-core/src/config.rs` | `AgentsConfig` + `PartialAgents`, merged across layers | Modify |
| `crates/zoid-core/src/lib.rs` | (no change — `agent_profile` already exported) | — |
| `crates/zoid/src/agent_import.rs` | Filesystem adapter: `resolve_agent_dirs`, `import_agents`, `build_agent_registry` | Create |
| `crates/zoid/src/lib.rs` | Declare `pub mod agent_import;` | Modify |
| `crates/zoid-tools/src/list_agents.rs` | `ListAgents` tool (holds `Arc<AgentRegistry>`, returns `menu()`) | Create |
| `crates/zoid-tools/src/lib.rs` | Declare `pub mod list_agents;` | Modify |
| `crates/zoid-tools/src/subagent_dispatch.rs` | Add `agent` parameter to `DispatchSubagent::spec()` | Modify |
| `crates/zoid/src/invoke_skill.rs` | `chat_tools` gains `agents: Arc<AgentRegistry>` param, pushes `ListAgents` | Modify |
| `crates/zoid/src/agent.rs` | `TurnConfig.agents` field; dispatch branch resolves agent name; `spawn_subagent` takes resolved `&AgentProfile` | Modify |
| `crates/zoid/src/spawn_subagent.rs` | `spawn_subagent` takes `profile: AgentProfile` (owned) instead of hardcoding `builtin()` | Modify |
| `crates/zoid/src/subagent.rs` | `run_subagent` already takes `&AgentProfile` — no change needed | — |
| `crates/zoid/src/main.rs` | Build `Arc<AgentRegistry>` at startup; thread into `App.agents`, `spawn_turn`, `TurnConfig` | Modify |

---

### Task 1: `AgentRegistry` and `parse_agent_md` (zoid-core, pure)

**Files:**
- Modify: `crates/zoid-core/src/agent_profile.rs`
- Test: `crates/zoid-core/src/agent_profile.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct AgentRegistry { ... }` with `new`, `builtin`, `push_unique`, `get`, `names`, `all`, `menu`; `pub struct ParsedAgent { name, description, system_prompt, tools, model }`; `pub fn parse_agent_md(text: &str) -> Result<ParsedAgent, String>`.

- [ ] **Step 1: Write the failing tests for `AgentRegistry`**

Add to the bottom of the existing `tests` module in `agent_profile.rs`:

```rust
    #[test]
    fn agent_registry_builtin_has_delegate_at_index_zero() {
        let r = AgentRegistry::builtin();
        assert_eq!(r.names().first().map(String::as_str), Some("delegate"));
        let p = r.get("delegate").unwrap();
        assert_eq!(p.name, "delegate");
        assert!(!p.system_prompt.is_empty());
    }

    #[test]
    fn agent_registry_push_unique_rejects_delegate_shadow() {
        let mut r = AgentRegistry::builtin();
        let shadow = AgentProfile {
            name: "delegate".into(),
            description: "shadow".into(),
            system_prompt: "HIJACK".into(),
            tools: vec![],
            model: None,
        };
        assert!(!r.push_unique(shadow), "an import must not shadow delegate");
        assert_eq!(r.get("delegate").unwrap().system_prompt, "HIJACK" && false || r.get("delegate").unwrap().system_prompt != "HIJACK");
    }

    #[test]
    fn agent_registry_push_unique_appends_new_agent() {
        let mut r = AgentRegistry::new(vec![]);
        let a = AgentProfile { name: "a".into(), description: "d".into(), system_prompt: "s".into(), tools: vec![], model: None };
        assert!(r.push_unique(a));
        assert!(!r.push_unique(AgentProfile { name: "a".into(), description: "d".into(), system_prompt: "s".into(), tools: vec![], model: None }));
        let b = AgentProfile { name: "b".into(), description: "d".into(), system_prompt: "s".into(), tools: vec![], model: None };
        assert!(r.push_unique(b));
        assert_eq!(r.names(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn agent_registry_get_hits_known_misses_unknown() {
        let r = AgentRegistry::builtin();
        assert!(r.get("delegate").is_some());
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn agent_registry_names_and_all_are_in_order() {
        let r = AgentRegistry::builtin();
        let via_all: Vec<&str> = r.all().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(via_all, r.names().iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn agent_registry_menu_renders_one_line_per_agent() {
        let r = AgentRegistry::builtin();
        let menu = r.menu();
        assert!(menu.contains("- delegate: "));
        assert_eq!(menu.lines().count(), r.all().len());
    }

    #[test]
    fn agent_registry_empty_menu_is_empty_string() {
        assert_eq!(AgentRegistry::new(vec![]).menu(), "");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib agent_profile`
Expected: compile errors — `AgentRegistry`, `parse_agent_md` not defined.

- [ ] **Step 3: Write `AgentRegistry` and `ParsedAgent`/`parse_agent_md`**

Add this above the `#[cfg(test)] mod tests` block in `agent_profile.rs`:

```rust
/// The result of parsing an `agent.md` — the five fields extracted from the
/// frontmatter + body, before the importer assigns a filesystem location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAgent {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
}

/// Parse an `agent.md` document: a `---`-fenced frontmatter block followed by
/// the markdown body (the system prompt). Same structure as `parse_skill_md`,
/// but extracts five fields: `name`, `description`, `tools` (a YAML-style
/// `- item` list), `model` (an optional scalar), and the body as
/// `system_prompt`. Pure — no filesystem. Single-line scalars only; the
/// `tools` list is a minimal inline list parser (lines starting with `- `
/// under the `tools:` key), NOT a full YAML parser.
///
/// Returns `Err` with a human-readable reason if there is no frontmatter block,
/// no closing fence, or `name` is missing/empty.
pub fn parse_agent_md(text: &str) -> Result<ParsedAgent, String> {
    let after_open = text
        .strip_prefix("---")
        .ok_or("missing frontmatter opening '---'")?;
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close = after_open
        .find("\n---")
        .ok_or("missing frontmatter closing '---'")?;
    let front = &after_open[..close];
    let rest = &after_open[close + 1..]; // starts at "---"
    let body = rest
        .strip_prefix("---")
        .map(|b| b.strip_prefix('\n').unwrap_or(b))
        .unwrap_or(rest)
        .to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut tools: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut in_tools_list = false;
    for line in front.lines() {
        if in_tools_list {
            // A list item: `- value` (YAML block lists are typically indented;
            // trim first so `  - read` parses as well as `- read`). Stop
            // collecting at a non-`- `-prefixed line.
            if let Some(item) = line.trim().strip_prefix("- ") {
                tools.push(unquote(item.trim()));
                continue;
            }
            in_tools_list = false;
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = unquote(v.trim());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = unquote(v.trim());
        } else if line.trim() == "tools:" || line.starts_with("tools:") {
            // A bare `tools:` key switches into list-collecting mode for the
            // following `- ` lines. An inline `tools: foo` is not supported
            // (the spec's format is a YAML-style block list); treat the key
            // line itself as the list opener and ignore any scalar on it.
            in_tools_list = true;
        } else if let Some(v) = line.strip_prefix("model:") {
            let m = unquote(v.trim());
            if !m.is_empty() {
                model = Some(m);
            }
        }
    }
    if name.is_empty() {
        return Err("frontmatter is missing a non-empty 'name'".into());
    }
    Ok(ParsedAgent {
        name,
        description,
        system_prompt: body,
        tools,
        model,
    })
}

/// The agent profiles available to the current session. Mirrors `SkillRegistry`:
/// built-ins pre-seeded, imports merged with first-wins collision protection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRegistry {
    agents: Vec<AgentProfile>,
}

impl AgentRegistry {
    /// Build a registry from an explicit profile list.
    pub fn new(agents: Vec<AgentProfile>) -> Self {
        Self { agents }
    }

    /// Pre-seed the built-in `delegate` profile at index 0.
    pub fn builtin() -> Self {
        Self::new(vec![AgentProfile::builtin()])
    }

    /// Append `profile` unless a profile with the same name already exists.
    /// Returns `false` (and leaves the registry unchanged) on a name collision
    /// — first-wins, so the built-in `delegate` and earlier imports are protected.
    pub fn push_unique(&mut self, profile: AgentProfile) -> bool {
        if self.agents.iter().any(|a| a.name == profile.name) {
            return false;
        }
        self.agents.push(profile);
        true
    }

    /// Look up a profile by exact name.
    pub fn get(&self, name: &str) -> Option<&AgentProfile> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// All profile names in registry order.
    pub fn names(&self) -> Vec<String> {
        self.agents.iter().map(|a| a.name.clone()).collect()
    }

    /// All profiles in registry order (read-only view).
    pub fn all(&self) -> &[AgentProfile] {
        &self.agents
    }

    /// One `- name: description` line per agent, for the `list_agents` tool
    /// result. Empty string when there are no agents (never empty in practice —
    /// `delegate` is always present).
    pub fn menu(&self) -> String {
        self.agents
            .iter()
            .map(|a| format!("- {}: {}", a.name, a.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
```

Note: `unquote` already exists in `skill.rs` but is private there. Add a local copy in `agent_profile.rs` (mirroring the same one-line helper):

```rust
/// Strip one matching pair of surrounding single or double quotes.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let n = s.len();
    if n >= 2 && ((b[0] == b'"' && b[n - 1] == b'"') || (b[0] == b'\'' && b[n - 1] == b'\'')) {
        s[1..n - 1].to_string()
    } else {
        s.to_string()
    }
}
```

- [ ] **Step 4: Run tests to verify the registry tests pass**

Run: `cargo test -p zoid-core --lib agent_profile`
Expected: the seven `agent_registry_*` tests PASS.

- [ ] **Step 5: Write the failing `parse_agent_md` tests**

Add these to the same `tests` module:

```rust
    #[test]
    fn parse_agent_md_parses_all_five_fields() {
        let md = "---\n\
                  name: code-reviewer\n\
                  description: \"Reviews code changes\"\n\
                  tools:\n  - read\n  - grep\n  - glob\n\
                  model: claude-sonnet\n\
                  ---\nYou are a code reviewer.\n";
        let p = parse_agent_md(md).unwrap();
        assert_eq!(p.name, "code-reviewer");
        assert_eq!(p.description, "Reviews code changes");
        assert_eq!(p.tools, vec!["read".to_string(), "grep".to_string(), "glob".to_string()]);
        assert_eq!(p.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(p.system_prompt, "You are a code reviewer.\n");
    }

    #[test]
    fn parse_agent_md_absent_tools_is_empty_and_absent_model_is_none() {
        let md = "---\nname: n\ndescription: d\n---\nbody\n";
        let p = parse_agent_md(md).unwrap();
        assert!(p.tools.is_empty());
        assert!(p.model.is_none());
    }

    #[test]
    fn parse_agent_md_missing_frontmatter_is_err() {
        assert!(parse_agent_md("# no frontmatter\n").is_err());
    }

    #[test]
    fn parse_agent_md_missing_closing_fence_is_err() {
        assert!(parse_agent_md("---\nname: n\nbody but no close\n").is_err());
    }

    #[test]
    fn parse_agent_md_missing_name_is_err() {
        assert!(parse_agent_md("---\ndescription: only\n---\nbody\n").is_err());
    }

    #[test]
    fn parse_agent_md_preserves_body_verbatim_with_internal_dashes() {
        let md = "---\nname: x\n---\nline1\n---\nline2\n";
        let p = parse_agent_md(md).unwrap();
        assert_eq!(p.system_prompt, "line1\n---\nline2\n");
    }

    #[test]
    fn parse_agent_md_strips_one_pair_of_quotes_from_scalars() {
        let md = "---\nname: 'n'\ndescription: \"d\"\nmodel: 'm'\n---\nb\n";
        let p = parse_agent_md(md).unwrap();
        assert_eq!(p.name, "n");
        assert_eq!(p.description, "d");
        assert_eq!(p.model.as_deref(), Some("m"));
    }

    #[test]
    fn parse_agent_md_description_defaults_empty_when_absent() {
        let md = "---\nname: n\n---\nb\n";
        let p = parse_agent_md(md).unwrap();
        assert_eq!(p.description, "");
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib agent_profile`
Expected: all `parse_agent_md` tests PASS (the parser was written in Step 3).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/agent_profile.rs
git commit -m "feat(core): add AgentRegistry + parse_agent_md for agent.md files"
```

---

### Task 2: `AgentsConfig` in config.rs

**Files:**
- Modify: `crates/zoid-core/src/config.rs`
- Test: `crates/zoid-core/src/config.rs` (two `#[cfg(test)]` modules)

**Interfaces:**
- Produces: `pub struct AgentsConfig { pub source_dirs: Vec<String> }` (added to `Config`); `pub struct PartialAgents { pub source_dirs: Option<Vec<String>> }` (added to `PartialConfig`); merge logic unions `agents.source_dirs` across layers.
- Consumes: nothing from earlier tasks.

- [ ] **Step 1: Write the failing tests**

In the `config_tests` module (the one containing `parses_skills_source_dirs`), add:

```rust
    #[test]
    fn parses_agents_source_dirs() {
        let (p, _) = parse_toml("[agents]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        assert_eq!(
            p.agents.source_dirs,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn merge_unions_agents_source_dirs_across_layers() {
        let (user, _) = parse_toml("[agents]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[agents]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(
            cfg.agents.source_dirs,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
```

Also add to the `defaults_are_sane`-style checks — in the `defaults_are_sane` test body, append:

```rust
        assert!(c.agents.source_dirs.is_empty());
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib config`
Expected: compile errors — `AgentsConfig` / `PartialAgents` / `p.agents` / `cfg.agents` not defined.

- [ ] **Step 3: Add `AgentsConfig` and `PartialAgents`, wire into `Config`/`PartialConfig`/default/merge**

Edit 1 — define `AgentsConfig` next to `ModesConfig` (around the `ModesConfig` definition):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentsConfig {
    /// Extra directories to scan for `<agent>/agent.md` files (beyond the two
    /// convention dirs the bin adds). Unioned across config layers.
    pub source_dirs: Vec<String>,
}
```

Edit 2 — add the field to `Config` (after `pub modes: ModesConfig,`):

```rust
    pub agents: AgentsConfig,
```

Edit 3 — add to `Config::default()` (after `modes: ModesConfig::default(),`):

```rust
            agents: AgentsConfig::default(),
```

Edit 4 — add `PartialAgents` next to `PartialModes`:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialAgents {
    pub source_dirs: Option<Vec<String>>,
}
```

Edit 5 — add to `PartialConfig` (after `pub modes: PartialModes,`):

```rust
    pub agents: PartialAgents,
```

Edit 6 — add the merge block in `merge()`, right after the `modes.source_dirs` block (after the `if let Some(dirs) = &p.modes.source_dirs { ... }` block):

```rust
        if let Some(dirs) = &p.agents.source_dirs {
            for d in dirs {
                if !cfg.agents.source_dirs.contains(d) {
                    cfg.agents.source_dirs.push(d.clone());
                }
            }
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib config`
Expected: `parses_agents_source_dirs`, `merge_unions_agents_source_dirs_across_layers`, and `defaults_are_sane` PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [agents] source_dirs config section"
```

---

### Task 3: `agent_import.rs` (bin, filesystem adapter)

**Files:**
- Create: `crates/zoid/src/agent_import.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod agent_import;`)
- Test: `crates/zoid/src/agent_import.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `zoid_core::agent_profile::{parse_agent_md, AgentProfile, AgentRegistry}` (from Task 1).
- Produces: `pub fn resolve_agent_dirs(source_dirs, user_cfg_dir, cwd, home) -> Vec<PathBuf>`; `pub fn import_agents(dirs: &[PathBuf]) -> Vec<AgentProfile>`; `pub fn build_agent_registry(dirs: &[PathBuf]) -> AgentRegistry`.

- [ ] **Step 1: Declare the module**

In `crates/zoid/src/lib.rs`, add alongside `pub mod skill_import;`:

```rust
pub mod agent_import;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid/src/agent_import.rs` with the test module only (the impl will be added in Step 4 so tests fail to compile, then pass):

```rust
//! Filesystem source adapter for `agent.md` agent profiles — the effectful half
//! of the importer (the pure parser lives in `zoid_core::agent_profile`). Walks
//! configured + convention directories, parses each `<dir>/<name>/agent.md`,
//! and returns `AgentProfile`s. Bad inputs are skipped, never fatal — mirroring
//! `skill_import.rs`.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_agent_dirs(
            &["~/ag".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/agents"),
                PathBuf::from("/proj/.zoid/agents"),
                PathBuf::from("/home/u/ag"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn import_reads_valid_agents_and_skips_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (name, contents) in [
            ("alpha", "---\nname: alpha\ndescription: d\n---\nbody a\n"),
            ("beta", "---\nname: beta\ndescription: d\ntools:\n  - read\n---\nbody b\n"),
            ("broken", "no frontmatter here\n"),
        ] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("agent.md"), contents).unwrap();
        }
        let agents = import_agents(&[root.to_path_buf()]);
        let mut names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
        let beta = agents.iter().find(|a| a.name == "beta").unwrap();
        assert_eq!(beta.tools, vec!["read".to_string()]);
    }

    #[test]
    fn import_skips_missing_dir_without_panic() {
        let agents = import_agents(&[PathBuf::from("/nonexistent/zoid/agents/xyz")]);
        assert!(agents.is_empty());
    }

    #[test]
    fn import_supports_per_pack_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Bare agent.
        let bare = root.join("bare-agent");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(
            bare.join("agent.md"),
            "---\nname: bare-agent\ndescription: d\n---\nb\n",
        )
        .unwrap();
        // Per-pack agent: <root>/packA/nested/agent.md
        let nested = root.join("packA").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("agent.md"),
            "---\nname: nested\ndescription: d\n---\nn\n",
        )
        .unwrap();
        let agents = import_agents(&[root.to_path_buf()]);
        let names: std::collections::HashSet<String> =
            agents.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains("bare-agent"));
        assert!(names.contains("nested"));
    }

    #[test]
    fn build_registry_merges_builtins_and_imports_first_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // An import that TRIES to shadow the built-in name must not win.
        let clash = root.join("clash");
        std::fs::create_dir_all(&clash).unwrap();
        std::fs::write(
            clash.join("agent.md"),
            "---\nname: delegate\ndescription: evil\n---\nHIJACK\n",
        )
        .unwrap();
        // A genuinely new agent is imported.
        let fresh = root.join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(
            fresh.join("agent.md"),
            "---\nname: fresh\ndescription: d\n---\nfresh body\n",
        )
        .unwrap();
        let reg = build_agent_registry(&[root.to_path_buf()]);
        // Built-in delegate is protected (first-wins).
        let del = reg.get("delegate").unwrap();
        assert!(!del.system_prompt.contains("HIJACK"));
        // The new agent landed.
        assert!(reg.get("fresh").is_some());
        // delegate is at index 0.
        assert_eq!(reg.names().first().map(String::as_str), Some("delegate"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p zoid --lib agent_import`
Expected: compile errors — `resolve_agent_dirs`, `import_agents`, `build_agent_registry` not defined.

- [ ] **Step 4: Write the implementation**

Add the impl above the `#[cfg(test)] mod tests` block in `agent_import.rs`:

```rust
use std::path::{Path, PathBuf};

use zoid_core::agent_profile::{parse_agent_md, AgentProfile, AgentRegistry};

/// The ordered directories to scan: the two convention dirs
/// (`<user_cfg_dir>/agents`, `<cwd>/.zoid/agents`) first, then the configured
/// `source_dirs` (a leading `~` or `~/` expanded against `home`). Pure path
/// arithmetic — existence is checked later by `import_agents`.
pub fn resolve_agent_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        user_cfg_dir.join("agents"),
        cwd.join(".zoid").join("agents"),
    ];
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

/// Scan each directory for immediate `<name>/agent.md` children, parse them,
/// and return the resulting profiles. A directory that does not exist is
/// skipped silently; a present-but-unreadable directory, an unreadable file,
/// or a malformed `agent.md` is skipped with a warning to stderr. Never panics.
/// Also supports one level of nesting (`<root>/<pack>/<agent>/agent.md`).
pub fn import_agents(dirs: &[PathBuf]) -> Vec<AgentProfile> {
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping agents dir {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let agent_dir = entry.path();
            if !agent_dir.is_dir() {
                continue;
            }
            let md = agent_dir.join("agent.md");
            if md.is_file() {
                push_agent(&mut out, &md);
                continue;
            }
            // No agent.md here → maybe a pack dir; scan one level deeper.
            if let Ok(inner) = std::fs::read_dir(&agent_dir) {
                for e2 in inner.flatten() {
                    let sub = e2.path();
                    let sub_md = sub.join("agent.md");
                    if sub.is_dir() && sub_md.is_file() {
                        push_agent(&mut out, &sub_md);
                    }
                }
            }
        }
    }
    out
}

/// Read, parse, and push a single `agent.md` onto `out`. Shared by both the
/// bare `<root>/<agent>/agent.md` and the per-pack `<root>/<pack>/<agent>/agent.md`
/// call sites in `import_agents`.
fn push_agent(out: &mut Vec<AgentProfile>, md: &Path) {
    let text = match std::fs::read_to_string(md) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("zoid: skipping {}: {e}", md.display());
            return;
        }
    };
    match parse_agent_md(&text) {
        Ok(p) => {
            out.push(AgentProfile {
                name: p.name,
                description: p.description,
                system_prompt: p.system_prompt,
                tools: p.tools,
                model: p.model,
            });
        }
        Err(reason) => {
            eprintln!("zoid: skipping {}: {reason}", md.display());
        }
    }
}

/// Build the session's agent registry: the built-in `delegate` plus every
/// importable agent under `dirs`. Built-ins and earlier dirs win name
/// collisions (first-wins), so an imported agent can never shadow `delegate`.
pub fn build_agent_registry(dirs: &[PathBuf]) -> AgentRegistry {
    let mut reg = AgentRegistry::builtin();
    for a in import_agents(dirs) {
        reg.push_unique(a);
    }
    reg
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid --lib agent_import`
Expected: all five tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent_import.rs crates/zoid/src/lib.rs
git commit -m "feat(bin): add agent_import filesystem adapter for agent.md"
```

---

### Task 4: `ListAgents` tool (zoid-tools)

**Files:**
- Create: `crates/zoid-tools/src/list_agents.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add `pub mod list_agents;`)
- Test: `crates/zoid-tools/src/list_agents.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `zoid_core::agent_profile::AgentRegistry` (from Task 1).
- Produces: `pub struct ListAgents` implementing `Tool` (name `list_agents`, `ToolKind::Local`, empty params, `run()` returns `registry.menu()`).

- [ ] **Step 1: Declare the module**

In `crates/zoid-tools/src/lib.rs`, add (alphabetically near the other `pub mod` lines):

```rust
pub mod list_agents;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid-tools/src/list_agents.rs`:

```rust
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use zoid_core::agent_profile::AgentRegistry;
use zoid_provider::ToolSpec;

/// A read-only tool that lists the available subagent agent profiles by name
/// and description. The model calls this before `dispatch_subagent` to see
/// which agents are available, then passes one's name to `dispatch_subagent`'s
/// `agent` parameter. Holds an `Arc<AgentRegistry>` injected at construction.
pub struct ListAgents {
    agents: Arc<AgentRegistry>,
}

impl ListAgents {
    pub fn new(agents: Arc<AgentRegistry>) -> Self {
        Self { agents }
    }
}

impl Tool for ListAgents {
    fn name(&self) -> &str {
        "list_agents"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_agents".into(),
            description: "List the available subagent agent profiles by name and \
                description. Call this before dispatch_subagent to see which agents \
                are available, then pass one's name to dispatch_subagent's 'agent' \
                parameter."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Local
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::ok(format!("Available agents:\n{}", self.agents.menu()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> ListAgents {
        ListAgents::new(Arc::new(AgentRegistry::builtin()))
    }

    #[test]
    fn name_and_spec_agree() {
        assert_eq!(tool().name(), "list_agents");
        assert_eq!(tool().spec().name, "list_agents");
    }

    #[test]
    fn kind_is_local() {
        assert_eq!(tool().kind(), ToolKind::Local);
    }

    #[test]
    fn spec_has_empty_parameters() {
        let params = tool().spec().parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"].as_object().unwrap().is_empty());
        assert!(params["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn run_returns_registry_menu() {
        let out = tool().run(&json!({}), Path::new("."));
        assert!(!out.is_error);
        assert!(out.text.starts_with("Available agents:\n"));
        assert!(out.text.contains("- delegate: "));
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid-tools --lib list_agents`
Expected: all four tests PASS (the impl is inline in the same file, written in Step 2).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/src/list_agents.rs crates/zoid-tools/src/lib.rs
git commit -m "feat(tools): add list_agents tool"
```

---

### Task 5: `agent` parameter on `DispatchSubagent`

**Files:**
- Modify: `crates/zoid-tools/src/subagent_dispatch.rs`
- Test: `crates/zoid-tools/src/subagent_dispatch.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `DispatchSubagent::spec()` gains an `agent` string parameter (default `"delegate"`); `required` stays `["task"]`.

- [ ] **Step 1: Update the existing test to assert the new parameter**

In `subagent_dispatch.rs`, extend the existing `dispatch_subagent_spec_and_kind` test. Replace its body with:

```rust
    #[test]
    fn dispatch_subagent_spec_and_kind() {
        assert_eq!(DispatchSubagent.name(), "dispatch_subagent");
        assert_eq!(DispatchSubagent.spec().name, "dispatch_subagent");
        assert_eq!(DispatchSubagent.kind(), ToolKind::Emitting);
        let params = DispatchSubagent.spec().parameters;
        assert_eq!(params["required"][0], "task");
        assert_eq!(params["properties"]["agent"]["type"], "string");
        assert_eq!(params["properties"]["agent"]["default"], "delegate");
        assert!(
            params["properties"]["worktree"]["default"].is_boolean(),
            "worktree default must remain boolean"
        );
        assert!(
            params["properties"].get("model").is_none(),
            "model must not be in the dispatch_subagent spec — subagents inherit the session model"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tools --lib subagent_dispatch`
Expected: FAIL — `params["properties"]["agent"]` is null.

- [ ] **Step 3: Add the `agent` parameter to the spec**

In `subagent_dispatch.rs`, replace the `parameters` JSON in `spec()`:

```rust
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The task description for the subagent" },
                    "agent": { "type": "string", "description": "The agent profile name to use (default: 'delegate'). Call list_agents to see available agents.", "default": "delegate" },
                    "worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false }
                },
                "required": ["task"]
            }),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tools --lib subagent_dispatch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/subagent_dispatch.rs
git commit -m "feat(tools): add agent parameter to dispatch_subagent spec"
```

---

### Task 6: Thread `Arc<AgentRegistry>` into `chat_tools` + `ListAgents` registration

**Files:**
- Modify: `crates/zoid/src/invoke_skill.rs`
- Test: `crates/zoid/src/invoke_skill.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `zoid_core::agent_profile::AgentRegistry` (Task 1), `zoid_tools::list_agents::ListAgents` (Task 4).
- Produces: `pub fn chat_tools(skills: Arc<SkillRegistry>, agents: Arc<AgentRegistry>, kill: zoid_tools::KillSlot) -> Vec<Box<dyn Tool>>` — pushes `ListAgents::new(agents)`.

- [ ] **Step 1: Update the existing tests for the new signature**

In `invoke_skill.rs` tests, every `chat_tools(Arc::new(SkillRegistry::builtin()), ...)` call gains an `Arc::new(AgentRegistry::builtin())` second argument. Update:

- `chat_tools_includes_invoke_skill_and_base_registry`:
```rust
        let tools = chat_tools(
            Arc::new(SkillRegistry::builtin()),
            Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        );
```
and add an assertion inside it:
```rust
        assert!(names.contains(&"list_agents"), "chat_tools includes list_agents");
```

- `chat_tools_includes_dispatch_and_diff`:
```rust
        let tools = chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        );
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib invoke_skill`
Expected: compile errors — `chat_tools` takes 2 args, tests pass 2 or 3 mismatched.

- [ ] **Step 3: Update `chat_tools` signature and push `ListAgents`**

In `invoke_skill.rs`, update imports and the function:

```rust
use zoid_core::agent_profile::AgentRegistry;
```

Replace the `chat_tools` function:

```rust
/// The Chat tool set: the standard curated registry plus the `invoke_skill` tool
/// bound to `skills`, and the `list_agents` tool bound to `agents`. Extracted
/// from `App` construction so it is unit-testable.
pub fn chat_tools(
    skills: Arc<SkillRegistry>,
    agents: Arc<AgentRegistry>,
    kill: zoid_tools::KillSlot,
) -> Vec<Box<dyn Tool>> {
    let mut tools = zoid_tools::registry_with_kill(kill);
    tools.push(Box::new(InvokeSkillTool::new(skills)));
    tools.push(Box::new(zoid_tools::list_agents::ListAgents::new(agents)));
    // `recall` is always offered in chat (never gated on eviction.enabled): the
    // cold tier is a standing capability, and a prior session may hold paged-out
    // turns worth recalling even when eviction is currently off. It is NOT in the
    // subagent `registry()`, so subagents (which have no session) can't call it.
    tools.push(Box::new(zoid_tools::recall::Recall));
    // `show` renders an HTML card in the companion browser view. Chat-only (it
    // needs the companion hub); never in the subagent registry.
    tools.push(Box::new(zoid_tools::show::Show));
    // Subagent dispatch + diff: isolated subagent execution for SDD and
    // parallel delegation. Chat-only (not in the base subagent registry).
    tools.push(Box::new(zoid_tools::subagent_dispatch::DispatchSubagent));
    tools.push(Box::new(zoid_tools::subagent_diff::SubagentDiff));
    // Orchestrator kill switch: cancel a dispatched subagent by id, or all.
    // Chat-only (needs the shared registry); never in the subagent registry so
    // a subagent can't cancel its siblings.
    tools.push(Box::new(zoid_tools::subagent_kill::CancelSubagent));
    // Worktree relocation: enter/exit persistent git worktrees. Chat-only —
    // subagents run in their own ephemeral worktrees via the subagent path.
    tools.push(Box::new(zoid_tools::worktree_enter::EnterWorktree));
    tools.push(Box::new(zoid_tools::worktree_exit::ExitWorktree));
    // Scheduled wake-ups: schedule/cancel a one-shot reminder to resume this
    // conversation later. Chat-only — subagents have no session to wake.
    tools.push(Box::new(zoid_tools::wake::ScheduleWake));
    tools.push(Box::new(zoid_tools::wake::CancelWake));
    tools
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib invoke_skill`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/invoke_skill.rs
git commit -m "feat(bin): wire ListAgents into chat_tools with Arc<AgentRegistry>"
```

---

### Task 7: `TurnConfig.agents` + dispatch-site resolution

**Files:**
- Modify: `crates/zoid/src/agent.rs`
- Test: `crates/zoid/src/agent.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `zoid_core::agent_profile::AgentRegistry` (Task 1).
- Produces: `TurnConfig` gains `pub agents: Option<std::sync::Arc<AgentRegistry>>` (None for subagents/tests); the `dispatch_subagent` Emitting branch resolves the agent name and passes a resolved `AgentProfile` to `spawn_subagent`.

- [ ] **Step 1: Write the failing dispatch-resolution tests**

Add to the `tests` module in `agent.rs`. These test the resolution helper in isolation (a pure function we'll extract). First, add the helper's signature to the impl in Step 3; here, write the tests:

```rust
    #[test]
    fn resolve_agent_name_defaults_to_delegate_when_absent() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        // No "agent" key → default "delegate".
        let resolved = resolve_agent_for_dispatch(
            &serde_json::json!({}),
            reg.clone(),
        );
        let (profile, name) = resolved.expect("absent agent should resolve to delegate");
        assert_eq!(name, "delegate");
        assert_eq!(profile.name, "delegate");
    }

    #[test]
    fn resolve_agent_name_known_returns_that_profile() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        // "delegate" is always known.
        let resolved = resolve_agent_for_dispatch(
            &serde_json::json!({ "agent": "delegate" }),
            reg.clone(),
        );
        let (profile, name) = resolved.unwrap();
        assert_eq!(name, "delegate");
        assert_eq!(profile.name, "delegate");
    }

    #[test]
    fn resolve_agent_name_unknown_returns_err_listing_available() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let resolved = resolve_agent_for_dispatch(
            &serde_json::json!({ "agent": "typo-name" }),
            reg.clone(),
        );
        let err = resolved.expect_err("unknown agent should be Err");
        assert!(err.contains("unknown agent 'typo-name'"));
        assert!(err.contains("delegate"), "error should list available agents");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib agent::tests::resolve_agent`
Expected: compile error — `resolve_agent_for_dispatch` not defined.

- [ ] **Step 3: Add `TurnConfig.agents` field and the resolution helper**

Edit 3a — add the field to the `TurnConfig` struct (after `pub subagent_ceiling: ...`):

```rust
    /// The agent profile registry for `dispatch_subagent` name resolution.
    /// `None` for subagent turns (subagents can't dispatch) and tests.
    pub agents: Option<std::sync::Arc<zoid_core::agent_profile::AgentRegistry>>,
```

Edit 3b — add to the `Debug` impl (after the `subagent_ceiling` field line):

```rust
            .field("agents", &self.agents.is_some())
```

Edit 3c — set the field in `chat_turn_config_with`'s `TurnConfig { ... }` literal. Add as the last field before the closing `}`:

```rust
        agents: None,
```

Edit 3d — set the field in the subagent `TurnConfig { ... }` literal in `subagent.rs` (after `subagent_ceiling: None,`):

```rust
        agents: None,
```

Edit 3e — add the resolution helper (near the dispatch branch, e.g. just above `pub fn chat_turn_config_with`):

```rust
/// Resolve the `agent` argument from a `dispatch_subagent` tool call against the
/// registry. Absent/empty `agent` defaults to `"delegate"`. Returns the cloned
/// `AgentProfile` to dispatch with, or an `Err` (listing available agents) for an
/// unknown name so the dispatch site can emit a self-correcting ToolResult.
pub fn resolve_agent_for_dispatch(
    args: &serde_json::Value,
    registry: std::sync::Arc<zoid_core::agent_profile::AgentRegistry>,
) -> Result<(zoid_core::agent_profile::AgentProfile, String), String> {
    let agent_name = args
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("delegate")
        .to_string();
    match registry.get(&agent_name) {
        Some(profile) => Ok((profile.clone(), agent_name)),
        None => Err(format!(
            "dispatch_subagent: unknown agent '{agent_name}'. Available: {}",
            registry.names().join(", ")
        )),
    }
}
```

- [ ] **Step 4: Run the resolution tests to verify they pass**

Run: `cargo test -p zoid --lib agent::tests::resolve_agent`
Expected: the three `resolve_agent_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/subagent.rs
git commit -m "feat(bin): add TurnConfig.agents + resolve_agent_for_dispatch helper"
```

---

### Task 8: Wire the dispatch branch to use the resolved profile + `spawn_subagent` signature

**Files:**
- Modify: `crates/zoid/src/agent.rs` (dispatch branch)
- Modify: `crates/zoid/src/spawn_subagent.rs`
- Test: `crates/zoid/src/agent.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `resolve_agent_for_dispatch` (Task 7), `AgentRegistry` (Task 1).
- Produces: the `dispatch_subagent` Emitting branch resolves the agent name and passes a resolved owned `AgentProfile` to `spawn_subagent`; `spawn_subagent` takes `profile: AgentProfile` (owned) instead of hardcoding `AgentProfile::builtin()`.

- [ ] **Step 1: Update `spawn_subagent` signature**

In `crates/zoid/src/spawn_subagent.rs`, change the parameter list — replace the implicit `AgentProfile::builtin()` usage with an owned `profile` parameter. Add `use zoid_core::agent_profile::AgentProfile;` at the top if not already imported, then:

Replace the signature line `pub fn spawn_subagent(` block's parameter list by inserting a new parameter `profile: AgentProfile,` (place it right after `task: String,`). Then, in the body, replace `&AgentProfile::builtin()` with `&profile`:

```rust
pub fn spawn_subagent(
    task: String,
    profile: AgentProfile,
    seed: crate::eventlog::EventLog,
    provider: Arc<dyn Provider>,
    // ... unchanged remaining params ...
```

and

```rust
        let res = crate::subagent::run_subagent(
            &task,
            &seed,
            &profile,
            provider,
            // ... unchanged remaining args ...
```

- [ ] **Step 2: Write a failing integration test for the dispatch branch**

Add to the `tests` module in `agent.rs`. This test drives the dispatch branch via the agent loop with a scripted provider that emits a `dispatch_subagent` tool call carrying an unknown `agent` name, and asserts the emitted `ToolResult` is an error listing available agents (and that no subagent was spawned).

**Copy the existing `dispatch_subagent_returns_id_as_tool_result` test** (agent.rs:4450-4523) — it is self-contained and builds its own `SessionHandle`, `SequencedProvider`, `chat_tools`, `run_agent_turn` call. Adapt only these parts:
- (a) set `config.agents = Some(reg.clone())` on the `TurnConfig` (use `chat_turn_config()` then assign `.agents`),
- (b) the scripted tool call's `args` → `json!({"task":"x","agent":"nope"})`,
- (c) assert the emitted `ToolResult` `output` contains `"unknown agent 'nope'"` AND `"delegate"`, and `is_error == true`,
- (d) assert no `AgentUpdate::SubagentStarted` was sent on the `ui` channel (drain it; expect empty).

```rust
    #[tokio::test]
    async fn dispatch_with_unknown_agent_emits_error_listing_available() {
        // Red phase: this assertion fails until the dispatch branch resolves
        // the agent name (Task 8 Step 4). The harness is copied from
        // dispatch_subagent_returns_id_as_tool_result (agent.rs:4450).
        assert!(
            false,
            "harness not yet wired — copy dispatch_subagent_returns_id_as_tool_result \
             per the Task 8 Step 2 instructions and replace this stub"
        );
        // The implementer replaces the line above with the full copied+adapted
        // harness (SessionHandle, SequencedProvider emitting one
        // dispatch_subagent call with agent="nope", chat_tools with
        // Arc<AgentRegistry::builtin()>, a TurnConfig with agents=Some(reg),
        // run_agent_turn, then assert the ToolResult error + no SubagentStarted).
        let _ = (|| async {
            let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
            let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
                std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
                reg.clone(),
                zoid_tools::KillSlot::new(),
            ));
            // ... full harness copied + adapted here in Step 4 ...
            let _ = tools;
        });
    }
```

The `assert!(false, ...)` makes Step 3's "verify it FAILS" honest. In Step 4 you replace this stub with the real copied+adapted harness (drop the `assert!(false)` and the no-op closure; write the actual `run_agent_turn` invocation and the four assertions (a)-(d)).

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid --lib dispatch_with_unknown_agent_emits_error_listing_available`
Expected: FAIL — the branch does not yet resolve/emit the error (dispatches with builtin, or the test harness isn't wired yet).

- [ ] **Step 4: Wire the dispatch branch**

In `crates/zoid/src/agent.rs`, in the `Some(zoid_tools::ToolKind::Emitting) if tc.name == "dispatch_subagent" =>` branch, AFTER the `task`-is-empty check and the `want_worktree` extraction, and BEFORE `let sub_ulid = Ulid::new();`, insert agent resolution:

```rust
                    // Resolve the agent profile by name (default "delegate").
                    let profile = match &config.agents {
                        Some(reg) => match resolve_agent_for_dispatch(&tc.args, reg.clone()) {
                            Ok((p, _name)) => p,
                            Err(msg) => {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: tc.id,
                                        name: tc.name,
                                        output: msg,
                                        is_error: true,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                                continue;
                            }
                        },
                        // No registry available (subagent turn) → fall back to builtin.
                        None => zoid_core::agent_profile::AgentProfile::builtin(),
                    };
```

Then change the `crate::spawn_subagent::spawn_subagent(` call to pass `profile` as the second argument (right after `task,`):

```rust
                    crate::spawn_subagent::spawn_subagent(
                        task,
                        profile,
                        events.snapshot(),
                        provider.clone(),
                        // ... unchanged remaining args ...
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid --lib dispatch_with_unknown_agent_emits_error_listing_available`
Expected: PASS.

- [ ] **Step 6: Update the existing `dispatch_subagent_returns_id_as_tool_result` test**

That test (agent.rs ~4450) constructs a `TurnConfig` — it must now set `agents: Some(Arc::new(AgentRegistry::builtin()))` (or the dispatch branch falls back to builtin via the `None` arm, which is also fine and preserves behavior). Simplest: add `agents: Some(std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin())),` to its `TurnConfig` literal if it builds one directly; if it uses `chat_turn_config_with`, no change is needed (that helper now sets `agents: None`, and the `None` arm falls back to builtin — unchanged behavior). Verify by running:

Run: `cargo test -p zoid --lib dispatch_subagent_returns_id_as_tool_result`
Expected: PASS (unchanged behavior for absent `agent` param).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/spawn_subagent.rs
git commit -m "feat(bin): resolve agent profile by name at dispatch_subagent site"
```

---

### Task 9: Startup wiring in `main.rs`

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Test: no new unit test (wiring); verify via `cargo build` and the existing integration test suite.

**Interfaces:**
- Consumes: `zoid::agent_import::{resolve_agent_dirs, build_agent_registry}` (Task 3), `zoid::invoke_skill::chat_tools` new signature (Task 6), `TurnConfig.agents` (Task 7).
- Produces: `App.agents` field; `Arc<AgentRegistry>` built at startup; threaded into `spawn_turn`'s `chat_tools` call and `TurnConfig.agents`.

- [ ] **Step 1: Build the registry at startup**

In `crates/zoid/src/main.rs`, right after the `modes` build block (~line 2077, after `let modes = zoid::mode_import::build_mode_registry(...);`), add:

```rust
    let agents = {
        let dirs = zoid::agent_import::resolve_agent_dirs(
            &config.agents.source_dirs,
            &cfg_dir,
            std::path::Path::new(&root),
            home.as_deref(),
        );
        std::sync::Arc::new(zoid::agent_import::build_agent_registry(&dirs))
    };
```

- [ ] **Step 2: Add `agents` field to `App`**

In the `App` struct definition (~line 1633, near `skills:`), add:

```rust
    /// Agent profiles for `dispatch_subagent` name resolution + the `list_agents`
    /// tool. Built at startup from convention + configured `agents.source_dirs`.
    agents: std::sync::Arc<zoid_core::agent_profile::AgentRegistry>,
```

There are exactly **two** `App { … }` construction literals that must gain the field (adding a field to `struct App` makes both fail to compile without it):

1. **Real startup construction** at `main.rs:2190` — uses shorthand (`skills,` at line 2198 because the local var is named `skills`). Add the shorthand line right after `skills,` (line 2198):
   ```rust
           agents,
   ```
   (The startup `let agents = { … }` block from Step 1 binds that name, so shorthand works.)

2. **Test helper `test_app()`** at `main.rs:7364` — uses explicit form (`skills: std::sync::Arc::new(...)` at line 7374). Add right after the `skills:` line:
   ```rust
               agents: std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
   ```

(Verify with `grep -n "App {" crates/zoid/src/main.rs` — expect exactly two: 2190 and 7364. A third `skills:` at ~8602 is inside a `Mode::Ready { … }` literal, NOT an `App` literal — leave it alone.)

- [ ] **Step 3: Thread into `spawn_turn`**

In `spawn_turn` (~line 6461), update the `chat_tools` call to pass `app.agents.clone()`:

```rust
    let mut tools = zoid::invoke_skill::chat_tools(
        std::sync::Arc::new(effective),
        app.agents.clone(),
        kill.clone(),
    );
```

Then set `turn_config.agents` (after the other `turn_config.* =` assignments, ~line 6508):

```rust
    turn_config.agents = Some(app.agents.clone());
```

- [ ] **Step 4: Build and verify**

Run: `cargo build -p zoid`
Expected: clean build (fix any other `chat_tools` call sites the compiler flags — search for `chat_tools(` across the repo and tests, each now needs the `Arc<AgentRegistry>` second arg).

Then run the test suite:

Run: `cargo test -p zoid --lib`
Expected: PASS.

Then run any integration tests that call `chat_tools`:

Run: `cargo test --test inline_question_card --test mode_import_wiring --test mode_skill_spike --test mode_turn --test mode_wizard_loop`
Expected: these call `chat_tools` — update each to pass `Arc::new(AgentRegistry::builtin())` as the second arg. PASS. (These integration tests live in `crates/zoid/tests/`, not a top-level `tests/` dir — there is no top-level `tests/`.)

Also update in-crate `chat_tools` call sites in `crates/zoid/src/agent.rs` tests (search `grep -n "chat_tools(" crates/zoid/src/agent.rs`): the existing `dispatch_subagent_returns_id_as_tool_result` test (~line 4481) and every other test harness around it (~lines 3997, 4113, 4214, 4288, 4371, 4481, 4561) call the 2-arg `chat_tools` and must gain the `Arc::new(AgentRegistry::builtin())` second arg. Run:

Run: `cargo test -p zoid --lib`
Expected: PASS after the call-site updates.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid/tests/
git commit -m "feat(bin): build AgentRegistry at startup and thread into turns"
```

---

### Task 10: Full build + test + clippy

**Files:** none (verification only).

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 2: Full workspace test**

Run: `cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Final commit (if any fixups)**

```bash
git add -A
git commit -m "chore: clippy/workspace fixes for agents-as-entity"
```
(Skip if nothing to commit.)

---

## Self-Review

**Spec coverage:**
- §1 `AgentRegistry` (zoid-core, pure) → Task 1. ✓
- §2 `parse_agent_md` → Task 1. ✓
- §3 `agent_import.rs` → Task 3. ✓
- §4 `AgentsConfig`/`PartialAgents` config → Task 2. ✓
- §5 `list_agents` tool → Task 4. ✓
- §6 `dispatch_subagent` spec `agent` param → Task 5. ✓
- §7 Dispatch site wiring (agent.rs) → Tasks 7 + 8. ✓
- §8 Startup wiring (main.rs) → Task 9. ✓
- Testing section: core/parser tests (Task 1), bin/import tests (Task 3), config tests (Task 2), tool tests (Tasks 4 + 5), integration dispatch tests (Task 8). ✓
- Seamed Fields → AMENDED (see header): honored rather than seamed, per user decision. ✓
- Out of scope: hot-reload, UI surfaces, agent-scoped skills — correctly absent. ✓

**Placeholder scan:** Task 8 Step 2's integration test references the existing `dispatch_subagent_returns_id_as_tool_result` harness by location and describes the adaptation in full (which fields to copy, which to change). This is not a placeholder — it's a concrete instruction to copy an existing, in-repo harness and alter specific lines. The implementer has the file path and line number.

**Type consistency:** `Arc<AgentRegistry>` is the type threaded everywhere. `resolve_agent_for_dispatch` returns `(AgentProfile, String)` and the dispatch branch uses `Ok((p, _name)) => p`. `spawn_subagent` takes `profile: AgentProfile` (owned), passed `profile` (owned, moved). `chat_tools` signature: `(Arc<SkillRegistry>, Arc<AgentRegistry>, KillSlot)`. `TurnConfig.agents: Option<Arc<AgentRegistry>>`. All consistent across tasks.

**Placeholder scan:** Task 8 Step 2's integration test is a guided adaptation of an existing, in-repo harness (`dispatch_subagent_returns_id_as_tool_result`, agent.rs:4450-4523 — verified self-contained: it builds its own `SessionHandle`, `SequencedProvider`, `chat_tools`, and `run_agent_turn` call). The stub body uses `assert!(false, "harness not yet wired …")` so Step 3's "verify it FAILS" is honest (red phase is real); Step 4 replaces the stub with the copied+adapted harness and the four concrete assertions. The instruction names the exact source test, file, line, and the precise edits (a)-(d). Not a placeholder.

**Parser-list indentation:** `parse_agent_md`'s `tools:` collector trims before `strip_prefix("- ")` (Gilfoyle C1 fix), so both YAML-standard indented items (`  - read`, as in the spec example and the plan's fixture) and bare `- read` parse correctly.