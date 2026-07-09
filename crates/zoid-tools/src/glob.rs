use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Match files by name/glob pattern (e.g. `**/*.rs`) under a root, newest first.
pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Find files by glob pattern (e.g. '**/*.rs'), sorted by modification time."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. '**/*.rs'." },
                    "path":    { "type": "string", "description": "Root directory to search (default '.')." }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let pattern = match str_arg(args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let matcher = match Glob::new(&pattern) {
            Ok(g) => g.compile_matcher(),
            Err(e) => return ToolOutput::err(format!("Glob: invalid pattern: {e}")),
        };
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = crate::resolve(cwd, path_arg);
        if !root.is_dir() {
            return ToolOutput::err(format!("Glob: path is not a directory: {path_arg}"));
        }
        let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
        crate::walk_files(&root, |rel, full| {
            if matcher.is_match(rel) {
                let mtime = full
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                found.push((mtime, rel.to_string()));
            }
            crate::Walk::Continue
        });
        if found.is_empty() {
            return ToolOutput::ok(format!("no files match {pattern:?}"));
        }
        // Newest first.
        found.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        let truncated = found.len() > MAX_RESULTS;
        found.truncate(MAX_RESULTS);
        let mut text = found
            .into_iter()
            .map(|(_, rel)| rel)
            .collect::<Vec<_>>()
            .join("\n");
        if truncated {
            text.push_str(&format!("\n… (truncated at {MAX_RESULTS} files)"));
        }
        ToolOutput::ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_by_extension_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let out = GlobTool.run(
            &json!({ "pattern": "**/*.rs", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs"));
        assert!(out.text.contains("b.rs") || out.text.contains("sub/b.rs") || out.text.contains("sub\\b.rs"));
        assert!(!out.text.contains("c.txt"));
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let out = GlobTool.run(
            &json!({ "pattern": "*.rs", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("no files match"));
    }

    #[test]
    fn file_path_errors_not_silent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "").unwrap();
        let out = GlobTool.run(
            &json!({ "pattern": "*.rs", "path": file.to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error, "expected error for file path, got: {}", out.text);
        assert!(out.text.contains("not a directory"), "{}", out.text);
    }
}
