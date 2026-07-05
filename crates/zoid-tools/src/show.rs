//! The `show` tool: render an HTML card in the companion browser view. Like
//! `recall`, it is `Emitting` — the agent loop executes it (it needs the
//! companion hub), so `run()` is never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct Show;

impl Tool for Show {
    fn name(&self) -> &str {
        "show"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "show".into(),
            description: "Render a self-contained HTML card in the companion browser view (a \
                          visual side panel). Use for mockups, diagrams, tables, or any visual \
                          the terminal cannot render at fidelity. The card replaces the \
                          previously shown one. Only works when the companion server is enabled."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "html": { "type": "string", "description": "self-contained HTML (inline CSS/SVG; no external resources)" },
                    "title": { "type": "string", "description": "optional short title" }
                },
                "required": ["html"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the loop branches on Emitting before calling run().
        ToolOutput::err("show is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolKind};

    #[test]
    fn show_spec_and_kind() {
        assert_eq!(Show.name(), "show");
        assert_eq!(Show.spec().name, "show");
        assert_eq!(Show.kind(), ToolKind::Emitting);
        // html is a required parameter
        let params = Show.spec().parameters;
        assert_eq!(params["required"][0], "html");
    }
}
