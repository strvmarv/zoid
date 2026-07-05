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
                search, shell). Work autonomously — do not ask questions. When done, give a \
                one-paragraph summary of what you changed."
                .into(),
            tools: vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "search".into(),
                "shell".into(),
            ],
            model: None,
        }
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
        assert!(p.allows("write_file"));
        assert!(p.allows("edit_file"));
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
}
