//! `AgentProfile` (core §4.4/§7): the parameterization of a subagent worker —
//! shaped to mirror the `.claude/agents` file schema (name, description, tools,
//! model, system-prompt body). v1 ships ONE built-in profile used by Chat's
//! delegation; the file loader and named registry are POST-V1 (loaders built on
//! demand — §7). Pure; `zoid-core` takes no provider/`git2`/process deps.

/// A subagent worker's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    /// Stable profile name (`.claude/agents` `name`).
    pub name: String,
    /// One-line description of what this worker is for.
    pub description: String,
    /// The worker's system prompt (the `.claude/agents` markdown body).
    pub system_prompt: String,
    /// Tool-name allow-list. Empty = every tool is permitted.
    pub tools: Vec<String>,
    /// Model override; `None` inherits the orchestrator's model.
    pub model: Option<String>,
}

impl AgentProfile {
    /// Whether this profile permits calling `tool`. An empty allow-list permits
    /// all tools (the profile does not constrain the tool set).
    pub fn allows(&self, tool: &str) -> bool {
        self.tools.is_empty() || self.tools.iter().any(|t| t == tool)
    }

    /// The single built-in delegation profile (v1). Full curated tool set;
    /// inherits the orchestrator's model.
    pub fn builtin() -> AgentProfile {
        AgentProfile {
            name: "delegate".into(),
            description: "Complete one discrete unit of work autonomously.".into(),
            system_prompt: "You are a zoid subagent. You are given ONE discrete task and the \
                relevant code. Complete the task end to end using the tools (read, write, edit, \
                grep, glob, ls, shell). Work autonomously — do not ask questions. When done, give \
                a one-paragraph summary of what you changed."
                .into(),
            tools: vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "grep".into(),
                "glob".into(),
                "ls".into(),
                "shell".into(),
            ],
            model: None,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profile_exposes_allow_list_and_prompt() {
        let p = AgentProfile::builtin();
        assert!(!p.name.is_empty());
        assert!(!p.description.is_empty());
        assert!(!p.system_prompt.is_empty());
        // The built-in profile may edit files and run the shell.
        assert!(p.allows("write"));
        assert!(p.allows("edit"));
        assert!(p.allows("shell"));
        // A tool NOT on the allow-list is denied.
        assert!(!p.allows("launch_missiles"));
    }

    #[test]
    fn empty_allow_list_permits_everything() {
        let p = AgentProfile {
            name: "open".into(),
            description: "anything".into(),
            system_prompt: "sys".into(),
            tools: vec![],
            model: None,
        };
        assert!(p.allows("anything_at_all"));
    }

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
        assert!(false || r.get("delegate").unwrap().system_prompt != "HIJACK");
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
}
