//! Modes: a named agent that owns a scoped set of skills. Pure value-holders +
//! scoping logic — the effectful discovery of `mode.md` folders lives in the bin
//! (`mode_import.rs`). `Chat` is the non-removable index-0 floor. The ambient
//! system-prompt overlay is composed here (`overlay_prompt`) from a base prompt
//! passed in by the bin, because `SYSTEM_PROMPT` is bin-only.

use crate::agent_profile::AgentProfile;
use crate::skill::SkillRegistry;

/// One mode: either a fully-loaded agent (`Ready`) or a slot that failed to load
/// (`Broken`) but stays visible in the cycle so the failure is surfaced, never
/// silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Ready {
        profile: AgentProfile,
        skills: SkillRegistry,
    },
    Broken {
        name: String,
        error: String,
    },
}

impl Mode {
    /// The `Chat` floor: the base coding-agent profile, owning no skills.
    pub fn chat(base: AgentProfile) -> Mode {
        Mode::Ready {
            profile: base,
            skills: SkillRegistry::new(vec![]),
        }
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
    /// Read-only view of all modes, in order. For importer/bin tests.
    pub fn modes(&self) -> &[Mode] {
        &self.modes
    }
}

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
        Skill {
            name: name.into(),
            description: "d".into(),
            body: format!("body-{name}"),
            base_dir: None,
        }
    }
    fn ready(name: &str, prompt: &str, skills: Vec<Skill>) -> Mode {
        Mode::Ready {
            profile: prof(name, prompt),
            skills: SkillRegistry::new(skills),
        }
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
        let m = Mode::Broken {
            name: "Bust".into(),
            error: "boom".into(),
        };
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
        assert_eq!(eff.get("brainstorming").unwrap().body, "body-brainstorming");
        // the mode copy
    }

    #[test]
    fn effective_skills_broken_is_globals_only() {
        let global = SkillRegistry::new(vec![skill("y")]);
        let broken = Mode::Broken {
            name: "b".into(),
            error: "e".into(),
        };
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
            Mode::Broken {
                name: "B".into(),
                error: "e".into(),
            },
        ]);
        reg.set_active("B");
        let (profile, eff) = active_turn(&reg, &global, &base);
        assert_eq!(profile.system_prompt, "BASE"); // no overlay for broken
        assert_eq!(eff.names(), vec!["y"]);
    }
}
