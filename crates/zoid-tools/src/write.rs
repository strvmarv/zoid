use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_core::ErrorKind;
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
        let full = crate::resolve(cwd, &path);
        // Best-effort pre-image for the ephemeral diff; a new/unreadable file
        // is treated as empty (all-additions).
        let before = std::fs::read_to_string(&full).unwrap_or_default();
        match std::fs::write(&full, content.as_bytes()) {
            Ok(()) => {
                let fd = crate::compute_file_diff(
                    &path,
                    &before,
                    &content,
                    crate::diff::INLINE_LINE_CAP,
                );
                ToolOutput::ok(format!("wrote {} bytes to {path}", content.len())).with_diff(fd)
            }
            Err(e) => {
                let kind = match e.kind() {
                    std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
                    _ => ErrorKind::Internal,
                };
                ToolOutput::err_kind(kind, format!("write({path}): {e}"))
            }
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
        assert_eq!(out.error_kind, Some(ErrorKind::InvalidInput));
    }

    #[test]
    fn write_of_new_file_carries_all_additions_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap(), "content": "a\nb\n" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        let diff = out.diff.expect("write success must carry a diff");
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
    }

    #[test]
    fn overwrite_diffs_against_prior_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, "a\nb\n").unwrap();
        let out = Write.run(
            &json!({ "path": path.to_str().unwrap(), "content": "a\nB\n" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        let diff = out.diff.expect("write success must carry a diff");
        assert_eq!(diff.added, 1, "B is added");
        assert_eq!(diff.removed, 1, "b is removed");
    }
}
