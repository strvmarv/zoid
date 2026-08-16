use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 500;

/// List the entries of a directory (non-recursive): type, size, name.
pub struct Ls;

impl Tool for Ls {
    fn name(&self) -> &str {
        "ls"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "List a directory's entries (type, size, name).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "Directory to list." },
                    "ignore": { "type": "array", "items": { "type": "string" }, "description": "Glob patterns to omit." }
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
        let ignores: Vec<globset::GlobMatcher> = args
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| Glob::new(s).ok())
                    .map(|g| g.compile_matcher())
                    .collect()
            })
            .unwrap_or_default();
        let dir = crate::resolve(cwd, &path);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => return ToolOutput::err(format!("ls({path}): {e}")),
        };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        let mut rows: Vec<String> = Vec::new();
        for p in paths {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if crate::skip_entry(&name) {
                continue;
            }
            if ignores.iter().any(|g| g.is_match(&name)) {
                continue;
            }
            // Cap check comes AFTER the skip/ignore filters so only entries we
            // would actually list count toward the cap (a skip-listed entry past
            // the cap must not trigger a spurious truncation notice).
            if rows.len() >= MAX_RESULTS {
                rows.push(format!("… (truncated at {MAX_RESULTS} entries)"));
                break;
            }
            let (kind, size) = if p.is_symlink() {
                ("link", 0)
            } else if p.is_dir() {
                ("dir", 0)
            } else {
                ("file", p.metadata().map(|m| m.len()).unwrap_or(0))
            };
            rows.push(format!("{kind}\t{size}\t{name}"));
        }
        if rows.is_empty() {
            return ToolOutput::ok("(empty)".to_string());
        }
        ToolOutput::ok(rows.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_entries_with_types() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "abc").unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let out = Ls.run(
            &json!({ "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("file\t3\tf.txt"));
        assert!(out.text.contains("dir\t0\td"));
    }

    #[test]
    fn ignore_globs_and_skiplist_are_applied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.rs"), "").unwrap();
        std::fs::write(dir.path().join("skip.log"), "").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        let out = Ls.run(
            &json!({ "path": dir.path().to_str().unwrap(), "ignore": ["*.log"] }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("keep.rs"));
        assert!(!out.text.contains("skip.log"));
        assert!(!out.text.contains("target"));
    }

    #[test]
    fn missing_dir_is_error() {
        let out = Ls.run(
            &json!({ "path": "/no/such/zoid/dir" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }
}
