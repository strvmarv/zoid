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
        let root = crate::resolve(
            cwd,
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
        );
        let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
        walk(&root, &root, &matcher, &mut found);
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

fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn walk(
    root: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    found: &mut Vec<(std::time::SystemTime, String)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip(name) {
            continue;
        }
        if path.is_symlink() {
            continue;
        } else if path.is_dir() {
            walk(root, &path, matcher, found);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if matcher.is_match(&rel) {
                let mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                found.push((mtime, rel));
            }
        }
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
}
