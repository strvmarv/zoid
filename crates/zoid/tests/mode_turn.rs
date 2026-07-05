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
    Skill {
        name: name.into(),
        description: "d".into(),
        body: format!("BODY-{name}"),
        base_dir: None,
    }
}

#[test]
fn active_turn_snapshot_scopes_invoke_skill() {
    let base = base();
    let global = SkillRegistry::new(vec![]); // only built-ins would be here in prod; empty is fine
    let mut modes = ModeRegistry::new(vec![
        Mode::chat(base.clone()),
        Mode::Ready {
            profile: AgentProfile {
                name: "SP".into(),
                description: "d".into(),
                system_prompt: zoid_core::mode::overlay_prompt(&base.system_prompt, "USE SKILLS"),
                tools: vec![],
                model: None,
            },
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
