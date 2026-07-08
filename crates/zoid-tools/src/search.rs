use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Recursive regex search over text files under a root directory (default `.`).
/// Skips hidden entries and common build dirs; never follows symlinks.
pub struct Grep;

impl Tool for Grep {
    fn name(&self) -> &str {
        "Grep"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Search file contents with a regular expression.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern":     { "type": "string", "description": "Regular expression to search for." },
                    "path":        { "type": "string", "description": "Root directory to search (default '.')." },
                    "glob":        { "type": "string", "description": "Only search files matching this glob (e.g. '*.rs')." },
                    "-i":          { "type": "boolean", "description": "Case-insensitive match." },
                    "output_mode": { "type": "string", "enum": ["files_with_matches", "content", "count"], "description": "Default 'files_with_matches'." }
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
        let case_insensitive = args.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);
        let re = match RegexBuilder::new(&pattern)
            .case_insensitive(case_insensitive)
            .build()
        {
            Ok(re) => re,
            Err(e) => return ToolOutput::err(format!("Grep: invalid regex: {e}")),
        };
        let glob = match args.get("glob").and_then(|v| v.as_str()) {
            Some(g) => match Glob::new(g) {
                Ok(g) => Some(g.compile_matcher()),
                Err(e) => return ToolOutput::err(format!("Grep: invalid glob: {e}")),
            },
            None => None,
        };
        let mode = args
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");
        let root = crate::resolve(
            cwd,
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
        );

        // (relpath, line_no, line_text) hits, capped at MAX_RESULTS.
        let mut hits: Vec<(String, usize, String)> = Vec::new();
        walk(&root, &root, &re, glob.as_ref(), &mut hits);

        if hits.is_empty() {
            return ToolOutput::ok(format!("no matches for {pattern:?}"));
        }
        let truncated = hits.len() >= MAX_RESULTS;
        let mut text = match mode {
            "content" => hits
                .iter()
                .map(|(rel, n, line)| format!("{rel}:{n}: {}", line.trim_end()))
                .collect::<Vec<_>>()
                .join("\n"),
            "count" => {
                let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
                for (rel, _, _) in &hits {
                    *counts.entry(rel.as_str()).or_default() += 1;
                }
                counts
                    .iter()
                    .map(|(rel, c)| format!("{rel}:{c}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => {
                // files_with_matches: unique paths in first-seen order.
                let mut seen: Vec<&str> = Vec::new();
                for (rel, _, _) in &hits {
                    if !seen.contains(&rel.as_str()) {
                        seen.push(rel);
                    }
                }
                seen.join("\n")
            }
        };
        if truncated {
            text.push_str(&format!("\n… (truncated at {MAX_RESULTS} matches; narrow the pattern or path)"));
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
    re: &regex::Regex,
    glob: Option<&globset::GlobMatcher>,
    hits: &mut Vec<(String, usize, String)>,
) {
    if hits.len() >= MAX_RESULTS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
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
            continue;
        } else if path.is_dir() {
            walk(root, &path, re, glob, hits);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if let Some(g) = glob {
                if !g.is_match(&rel) {
                    continue;
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                for (i, line) in contents.lines().enumerate() {
                    if re.is_match(line) {
                        hits.push((rel.clone(), i + 1, line.to_string()));
                        if hits.len() >= MAX_RESULTS {
                            return;
                        }
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

    fn seed() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello\nWORLD\n").unwrap();
        dir
    }

    #[test]
    fn regex_content_mode_returns_numbered_hits() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": r"fn \w+", "path": dir.path().to_str().unwrap(), "output_mode": "content" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs:1:"));
        assert!(out.text.contains("a.rs:2:"));
    }

    #[test]
    fn files_with_matches_is_default() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("a.rs"));
        assert!(!out.text.contains("b.txt"));
        assert!(!out.text.contains(":1:"), "default mode lists files, not lines");
    }

    #[test]
    fn glob_filter_restricts_file_set() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": ".", "path": dir.path().to_str().unwrap(), "glob": "*.txt" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("b.txt"));
        assert!(!out.text.contains("a.rs"));
    }

    #[test]
    fn case_insensitive_flag() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "world", "-i": true, "path": dir.path().to_str().unwrap(), "output_mode": "content" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("b.txt:2:"));
    }

    #[test]
    fn count_mode_reports_totals() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": dir.path().to_str().unwrap(), "output_mode": "count" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("a.rs:2"));
    }

    #[test]
    fn invalid_regex_is_error() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "(", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "zzzznomatch", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error);
        assert!(out.text.contains("no matches"));
    }
}
