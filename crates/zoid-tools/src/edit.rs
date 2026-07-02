use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// Replace the unique occurrence of `old` with `new` in a file. Errors if `old`
/// is absent or appears more than once (forces unambiguous edits).
pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Replace the unique occurrence of `old` with `new` in a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old":  { "type": "string", "description": "Exact text to find (must occur exactly once)." },
                    "new":  { "type": "string", "description": "Replacement text." }
                },
                "required": ["path", "old", "new"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let old = match str_arg(args, "old") {
            Ok(o) => o,
            Err(e) => return e,
        };
        let new = match str_arg(args, "new") {
            Ok(n) => n,
            Err(e) => return e,
        };

        let full = crate::resolve(cwd, &path);
        let contents = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("edit_file({path}): {e}")),
        };
        let count = contents.matches(&old).count();
        if count == 0 {
            return ToolOutput::err(format!("edit_file({path}): `old` not found"));
        }
        if count > 1 {
            return ToolOutput::err(format!(
                "edit_file({path}): `old` is ambiguous ({count} matches)"
            ));
        }
        let updated = contents.replacen(&old, &new, 1);
        match std::fs::write(&full, updated.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("edited {path}")),
            Err(e) => ToolOutput::err(format!("edit_file({path}): {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed(content: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path.to_str().unwrap().to_string())
    }

    #[test]
    fn replaces_unique_occurrence() {
        let (_d, path) = seed("alpha beta gamma");
        let out = EditFile.run(
            &json!({ "path": path, "old": "beta", "new": "BETA" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha BETA gamma");
    }

    #[test]
    fn ambiguous_match_is_error() {
        let (_d, path) = seed("x x");
        let out = EditFile.run(
            &json!({ "path": path, "old": "x", "new": "y" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("ambiguous"));
    }

    #[test]
    fn absent_match_is_error() {
        let (_d, path) = seed("hello");
        let out = EditFile.run(
            &json!({ "path": path, "old": "zzz", "new": "y" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("not found"));
    }
}
