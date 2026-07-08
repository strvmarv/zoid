use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// Replace the unique occurrence of `old` with `new` in a file. Errors if `old`
/// is absent or appears more than once (forces unambiguous edits).
pub struct Edit;

impl Tool for Edit {
    fn name(&self) -> &str {
        "Edit"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Edit a file: replace an exact unique string, or apply a batch of edits atomically.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string" },
                    "old_string":  { "type": "string", "description": "Exact text to find (must occur once unless replace_all)." },
                    "new_string":  { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)." },
                    "edits":       { "type": "array", "description": "Batch of {old_string,new_string,replace_all?} applied atomically.",
                        "items": { "type": "object", "properties": {
                            "old_string": { "type": "string" }, "new_string": { "type": "string" }, "replace_all": { "type": "boolean" }
                        }, "required": ["old_string", "new_string"] } }
                },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        // Normalize to a list of edits: either `edits: [...]` or a single triple.
        let edits: Vec<(String, String, bool)> = if let Some(arr) = args.get("edits").and_then(|v| v.as_array()) {
            let mut v = Vec::new();
            for (i, e) in arr.iter().enumerate() {
                let old = match e.get("old_string").and_then(|x| x.as_str()) {
                    Some(s) => s.to_string(),
                    None => return ToolOutput::err(format!("Edit({path}): edits[{i}] missing old_string")),
                };
                let new = match e.get("new_string").and_then(|x| x.as_str()) {
                    Some(s) => s.to_string(),
                    None => return ToolOutput::err(format!("Edit({path}): edits[{i}] missing new_string")),
                };
                let all = e.get("replace_all").and_then(|x| x.as_bool()).unwrap_or(false);
                v.push((old, new, all));
            }
            v
        } else {
            let old = match str_arg(args, "old_string") {
                Ok(o) => o,
                Err(e) => return e,
            };
            let new = match str_arg(args, "new_string") {
                Ok(n) => n,
                Err(e) => return e,
            };
            let all = args.get("replace_all").and_then(|x| x.as_bool()).unwrap_or(false);
            vec![(old, new, all)]
        };

        let full = crate::resolve(cwd, &path);
        let mut contents = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Edit({path}): {e}")),
        };
        // Apply all edits in memory; bail (writing nothing) on the first failure.
        for (i, (old, new, replace_all)) in edits.iter().enumerate() {
            match apply_one(&contents, old, new, *replace_all) {
                Ok(updated) => contents = updated,
                Err(msg) => return ToolOutput::err(format!("Edit({path}) edit #{}: {msg}", i + 1)),
            }
        }
        match std::fs::write(&full, contents.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("edited {path} ({} change(s))", edits.len())),
            Err(e) => ToolOutput::err(format!("Edit({path}): {e}")),
        }
    }
}

/// Apply one edit to `contents`, enforcing the unambiguous-match rule unless
/// `replace_all`. Returns the updated string or an error message.
fn apply_one(contents: &str, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("`old_string` must not be empty".into());
    }
    let count = contents.matches(old).count();
    if count == 0 {
        return Err("`old_string` not found".into());
    }
    if count > 1 && !replace_all {
        return Err(format!("`old_string` is ambiguous ({count} matches)"));
    }
    if replace_all {
        Ok(contents.replace(old, new))
    } else {
        Ok(contents.replacen(old, new, 1))
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
        let out = Edit.run(
            &json!({ "path": path, "old_string": "beta", "new_string": "BETA" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha BETA gamma");
    }

    #[test]
    fn ambiguous_match_is_error() {
        let (_d, path) = seed("x x");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "x", "new_string": "y" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("ambiguous"));
    }

    #[test]
    fn absent_match_is_error() {
        let (_d, path) = seed("hello");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "zzz", "new_string": "y" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("not found"));
    }

    #[test]
    fn replace_all_replaces_every_occurrence() {
        let (_d, path) = seed("x x x");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "x", "new_string": "y", "replace_all": true }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y y y");
    }

    #[test]
    fn multi_edit_applies_all_atomically() {
        let (_d, path) = seed("alpha beta gamma");
        let out = Edit.run(
            &json!({ "path": path, "edits": [
                { "old_string": "alpha", "new_string": "A" },
                { "old_string": "gamma", "new_string": "G" }
            ]}),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A beta G");
    }

    #[test]
    fn empty_old_string_is_rejected() {
        let (_d, path) = seed("ab");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "", "new_string": "X", "replace_all": true }),
            std::path::Path::new("."),
        );
        assert!(out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ab");
    }

    #[test]
    fn multi_edit_failure_leaves_file_untouched() {
        let (_d, path) = seed("alpha beta");
        let out = Edit.run(
            &json!({ "path": path, "edits": [
                { "old_string": "alpha", "new_string": "A" },
                { "old_string": "zzz", "new_string": "Z" }
            ]}),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        // First edit must NOT have been written.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha beta");
    }
}
