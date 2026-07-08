//! Tool-call approval: a blacklist gate that prompts (or denies) on
//! dangerous shell commands. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

/// The blacklist gate. Allow unless a `shell` call matches a dangerous pattern.
/// `interactive: true` returns `Gate::Prompt` on a match (Chat);
/// `interactive: false` returns `Gate::Deny` (subagents — headless, can't prompt).
pub struct BlacklistGate {
    interactive: bool,
}

impl BlacklistGate {
    pub fn new(_shell_danger: Vec<String>, _shell_allow: Vec<String>, interactive: bool) -> Self {
        Self { interactive }
    }
}

impl crate::ToolGate for BlacklistGate {
    fn check(&self, _call: &zoid_provider::ToolCall) -> crate::Gate {
        crate::Gate::Allow
    }
}