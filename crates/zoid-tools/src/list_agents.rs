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
