use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use zoid_provider::ToolSpec;

/// Read a UTF-8 text file relative to the working directory.
pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Read a UTF-8 text file from the working directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "File path relative to the working directory." } },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => ToolOutput::ok(contents),
            Err(e) => ToolOutput::err(format!("read_file({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn reads_existing_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "hello tools").unwrap();
        let out = ReadFile.run(&json!({ "path": f.path().to_str().unwrap() }));
        assert!(!out.is_error);
        assert_eq!(out.text, "hello tools");
    }

    #[test]
    fn missing_file_is_error() {
        let out = ReadFile.run(&json!({ "path": "/no/such/zoid/file" }));
        assert!(out.is_error);
    }

    #[test]
    fn missing_arg_is_error() {
        let out = ReadFile.run(&json!({}));
        assert!(out.is_error);
        assert!(out.text.contains("path"));
    }
}
