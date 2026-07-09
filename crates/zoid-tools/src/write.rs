use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// Write (create or overwrite) a UTF-8 text file relative to the working dir.
pub struct Write;

impl Tool for Write {
    fn name(&self) -> &str {
        "write"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Create or overwrite a UTF-8 text file in the working directory."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the working directory." },
                    "content": { "type": "string", "description": "Full file contents to write." }
                },
                "required": ["path", "content"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let content = match str_arg(args, "content") {
            Ok(c) => c,
            Err(e) => return e,
        };
        match std::fs::write(crate::resolve(cwd, &path), content.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("wrote {} bytes to {path}", content.len())),
            Err(e) => ToolOutput::err(format!("write({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_then_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap(), "content": "abc" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
    }

    #[test]
    fn missing_content_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("content"));
    }
}
