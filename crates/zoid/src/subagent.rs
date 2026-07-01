//! The subagent runtime (spec §4.4/§7). Builds a subagent's constructed context
//! (task + relevant code, NEVER session history) and runs it in isolation. The
//! orchestrator (the Chat loop) dispatches one at a time.

use zoid_core::agent_profile::AgentProfile;
use zoid_core::assembler::{assemble_context, ContextPolicy};
use zoid_core::context::{context_window, file_contents, ItemKind};
use zoid_core::event::Event;
use zoid_provider::{CompletionRequest, Message};
use zoid_tools::Tool;

use crate::agent::tool_specs;

/// Per-subagent max output tokens (mirrors the Chat loop's budget).
const SUBAGENT_MAX_TOKENS: u32 = 4096;

/// Token ceiling for a subagent's constructed context (≈ half a 64k window,
/// leaving room for the task, tool round-trips, and output).
const SUBAGENT_CONTEXT_CEILING: u64 = 32_000;

/// Default context budget for a dispatched subagent: drop cold items and cap the
/// constructed context so it stays a *precise* slice, not a dump.
pub fn subagent_policy() -> ContextPolicy {
    ContextPolicy {
        token_ceiling: Some(SUBAGENT_CONTEXT_CEILING),
        auto_evict_cold: true,
        compact_threshold: None,
    }
}

/// Build a subagent `CompletionRequest`: the P3 assembler selects the relevant
/// context items from `events`; we keep the included **File** items, resolve
/// their content, and compose a task-focused prompt. Session messages/tool
/// transcripts are intentionally excluded (spec §4.4/§5.4: never session history).
pub fn build_subagent_request(
    task: &str,
    events: &[Event],
    policy: &ContextPolicy,
    profile: &AgentProfile,
    model: &str,
    tools: &[Box<dyn Tool>],
) -> CompletionRequest {
    let window = context_window(events);
    let selection = assemble_context(&window, policy);
    let contents = file_contents(events);

    let mut ctx = String::new();
    for item in selection.included.iter().filter(|i| i.kind == ItemKind::File) {
        if let Some(c) = contents.get(&item.key) {
            ctx.push_str(&format!("\n// {}\n{}\n", item.label, c));
        }
    }

    let user = if ctx.is_empty() {
        format!("Task:\n{task}")
    } else {
        format!("Task:\n{task}\n\nRelevant files:\n{ctx}")
    };

    CompletionRequest {
        model: model.to_string(),
        system: Some(profile.system_prompt.clone()),
        messages: vec![Message::user(user)],
        max_tokens: SUBAGENT_MAX_TOKENS,
        tools: tool_specs(tools),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::agent_profile::AgentProfile;
    use zoid_core::event::{Event, EventKind};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
    fn call(id: &str, path: &str) -> Event {
        ev(EventKind::ToolCall { id: id.into(), name: "read_file".into(), args: format!(r#"{{"path":"{path}"}}"#) })
    }
    fn result(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult { id: id.into(), name: "read_file".into(), output: out.into(), is_error: false })
    }

    #[test]
    fn request_carries_task_and_relevant_file_never_history() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "secret chat history".into() }),
            call("c1", "src/ast.rs"),
            result("c1", "fn parse() {}"),
        ];
        let profile = AgentProfile::builtin();
        let tools = zoid_tools::registry();
        let req = build_subagent_request("refactor parse()", &evs, &subagent_policy(), &profile, "glm", &tools);

        assert_eq!(req.model, "glm");
        assert_eq!(req.system.as_deref(), Some(profile.system_prompt.as_str()));
        assert_eq!(req.messages.len(), 1, "subagent gets ONE constructed user message");
        let body = &req.messages[0].content;
        assert!(body.contains("refactor parse()"), "task present");
        assert!(body.contains("fn parse() {}"), "relevant file content present");
        assert!(body.contains("src/ast.rs"), "file labeled by path");
        // THE SUPERPOWERS INVARIANT: never the session transcript.
        assert!(!body.contains("secret chat history"), "session history excluded (spec §4.4/§5.4)");
        assert!(!req.tools.is_empty(), "tools advertised");
    }

    #[test]
    fn request_without_files_is_just_the_task() {
        let req = build_subagent_request(
            "do a thing", &[], &subagent_policy(), &AgentProfile::builtin(), "glm", &zoid_tools::registry());
        assert!(req.messages[0].content.contains("do a thing"));
    }

    #[test]
    fn subagent_policy_is_bounded_and_evicts_cold() {
        let p = subagent_policy();
        assert!(p.auto_evict_cold, "cold items dropped from a subagent's context");
        assert!(p.token_ceiling.is_some(), "subagent context is token-bounded");
    }
}
