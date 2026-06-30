use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Recursive literal (substring) search over text files under a root directory
/// (default `.`). Skips hidden entries and common build dirs. Returns up to
/// `MAX_RESULTS` `relpath:line: text` matches.
pub struct Search;

impl Tool for Search {
    fn name(&self) -> &str {
        "search"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Recursively search files for a literal substring (like grep -F).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Literal substring to find." },
                    "path":  { "type": "string", "description": "Root directory to search (default '.')." }
                },
                "required": ["query"]
            }),
        }
    }
    fn run(&self, args: &Value) -> ToolOutput {
        let query = match str_arg(args, "query") { Ok(q) => q, Err(e) => return e };
        if query.is_empty() {
            return ToolOutput::err("search: empty query");
        }
        let root = args.get("path").and_then(|v| v.as_str()).unwrap_or(".").to_string();
        let mut hits: Vec<String> = Vec::new();
        walk(Path::new(&root), Path::new(&root), &query, &mut hits);
        if hits.is_empty() {
            ToolOutput::ok(format!("no matches for {query:?}"))
        } else {
            let truncated = hits.len() >= MAX_RESULTS;
            let mut text = hits.join("\n");
            if truncated {
                text.push_str(&format!("\n… (truncated at {MAX_RESULTS} matches)"));
            }
            ToolOutput::ok(text)
        }
    }
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn walk(root: &Path, dir: &Path, query: &str, hits: &mut Vec<String>) {
    if hits.len() >= MAX_RESULTS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    // Deterministic order: collect + sort by path.
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if hits.len() >= MAX_RESULTS {
            return;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip(name) {
            continue;
        }
        if path.is_symlink() {
            // Never follow symlinks: a cycle (link back to an ancestor) would
            // recurse unboundedly and overflow the stack before MAX_RESULTS.
            continue;
        } else if path.is_dir() {
            walk(root, &path, query, hits);
        } else if let Ok(contents) = std::fs::read_to_string(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            for (i, line) in contents.lines().enumerate() {
                if line.contains(query) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim_end()));
                    if hits.len() >= MAX_RESULTS {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_matches_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\nNEEDLE here\nthree").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "nothing\nalso NEEDLE").unwrap();

        let out = Search.run(&json!({ "query": "NEEDLE", "path": dir.path().to_str().unwrap() }));
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.txt:2:"));
        assert!(out.text.contains("sub/b.txt:2:") || out.text.contains("sub\\b.txt:2:"));
    }

    #[test]
    fn skips_hidden_and_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/x.txt"), "NEEDLE").unwrap();
        let out = Search.run(&json!({ "query": "NEEDLE", "path": dir.path().to_str().unwrap() }));
        assert!(out.text.contains("no matches"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_terminates() {
        // A directory symlink pointing back to its parent forms a cycle. Search
        // must terminate (we don't follow symlinks) rather than recurse forever.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "NEEDLE").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::os::unix::fs::symlink(dir.path(), sub.join("loop")).unwrap();
        let out = Search.run(&json!({ "query": "NEEDLE", "path": dir.path().to_str().unwrap() }));
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.txt:1:"));
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "abc").unwrap();
        let out = Search.run(&json!({ "query": "zzz", "path": dir.path().to_str().unwrap() }));
        assert!(!out.is_error);
        assert!(out.text.contains("no matches"));
    }
}
