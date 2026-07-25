//! The `invoke_skill` tool: the model calls it with a skill name; the tool
//! returns that skill's body as its result (fed back as a `Message::tool`), which
//! the model then follows. Chaining is just the model calling this again.
//! Implemented in the bin (not `zoid-tools`) so the tool crate keeps its
//! `zoid-provider`-only dependency — the bin is the composition root that owns
//! the `SkillRegistry`.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use zoid_core::agent_profile::AgentRegistry;
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
            Some(skill) => ToolOutput::ok(body_with_anchor(skill)),
            None => ToolOutput::err(format!(
                "unknown skill '{name}'. Available: {}",
                self.skills.names().join(", ")
            )),
        }
    }
}

/// The skill body, plus a resolved anchor line pointing at the skill's source
/// directory when it was imported from disk — so the model can read bundled
/// sibling files by absolute path via `Read`. Built-ins (no `base_dir`)
/// are returned unchanged.
fn body_with_anchor(skill: &zoid_core::skill::Skill) -> String {
    match &skill.base_dir {
        Some(dir) => format!(
            "{}\n\n---\nSkill files are in: {}/",
            skill.body,
            dir.display()
        ),
        None => skill.body.clone(),
    }
}

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
    // `list_subagents` shows which subagents are currently running (id + task).
    // Chat-only (needs the shared in_flight registry); never in the subagent
    // registry so a subagent can't list its siblings.
    tools.push(Box::new(zoid_tools::subagent_list::ListSubagents));
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

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::skill::Skill;

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
        let tools = chat_tools(
            Arc::new(SkillRegistry::builtin()),
            Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"invoke_skill"));
        assert!(names.contains(&"write"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"list_agents"), "chat_tools includes list_agents");
        // recall is a chat-only tool; it must be registered here (and never in the
        // subagent registry — see chat_tools).
        assert!(names.contains(&"recall"));
        // show is a chat-only tool; it must be registered here (and never in the
        // subagent registry — see chat_tools).
        assert!(names.contains(&"show"));
    }

    #[test]
    fn imported_skill_body_carries_base_dir_anchor() {
        let reg = SkillRegistry::new(vec![Skill {
            name: "docd".into(),
            description: "d".into(),
            body: "BODY".into(),
            base_dir: Some(std::path::PathBuf::from("/abs/skills/docd")),
        }]);
        let tool = InvokeSkillTool::new(Arc::new(reg));
        let out = tool.run(&json!({ "name": "docd" }), Path::new("."));
        assert!(!out.is_error);
        assert!(out.text.contains("BODY"));
        assert!(out.text.contains("Skill files are in: /abs/skills/docd/"));
    }

    #[test]
    fn builtin_skill_body_has_no_anchor() {
        let out = tool().run(&json!({ "name": "spike-plan" }), Path::new("."));
        assert!(!out.is_error);
        assert!(!out.text.contains("Skill files are in:"));
    }

    #[test]
    fn chat_tools_includes_dispatch_and_diff() {
        let tools = chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        );
        assert!(
            tools.iter().any(|t| t.name() == "dispatch_subagent"),
            "dispatch_subagent must be in chat_tools"
        );
        assert!(
            tools.iter().any(|t| t.name() == "subagent_diff"),
            "subagent_diff must be in chat_tools"
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"schedule_wake"), "chat_tools includes schedule_wake");
        assert!(names.contains(&"cancel_wake"), "chat_tools includes cancel_wake");
        assert!(names.contains(&"list_subagents"), "chat_tools includes list_subagents");
    }
}
