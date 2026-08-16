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
        "grep"
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
            Err(e) => return ToolOutput::err(format!("grep: invalid regex: {e}")),
        };
        let glob = match args.get("glob").and_then(|v| v.as_str()) {
            Some(g) => match Glob::new(g) {
                Ok(g) => Some(g.compile_matcher()),
                Err(e) => return ToolOutput::err(format!("grep: invalid glob: {e}")),
            },
            None => None,
        };
        let mode = args
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");
        if !matches!(mode, "files_with_matches" | "content" | "count") {
            return ToolOutput::err(format!(
                "grep: invalid output_mode {mode:?}; valid: files_with_matches, content, count"
            ));
        }
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = crate::resolve(cwd, path_arg);
        if !root.is_dir() {
            return ToolOutput::err(format!("grep: path is not a directory: {path_arg}"));
        }

        let (text_body, truncated, is_empty) = match mode {
            "content" => {
                // (relpath, line_no, line_text) hits, capped at MAX_RESULTS raw lines.
                let mut hits: Vec<(String, usize, String)> = Vec::new();
                let mut sink = Sink::Content(&mut hits);
                collect(&root, &re, glob.as_ref(), &mut sink);
                let truncated = hits.len() >= MAX_RESULTS;
                let is_empty = hits.is_empty();
                let text = hits
                    .iter()
                    .map(|(rel, n, line)| format!("{rel}:{n}: {}", line.trim_end()))
                    .collect::<Vec<_>>()
                    .join("\n");
                (text, truncated, is_empty)
            }
            "count" => {
                // rel -> total matching lines in that file. Cap is on distinct files,
                // not raw line volume: every counted file gets its full line count.
                let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
                let mut sink = Sink::Count(&mut counts);
                collect(&root, &re, glob.as_ref(), &mut sink);
                let truncated = counts.len() >= MAX_RESULTS;
                let is_empty = counts.is_empty();
                let text = counts
                    .iter()
                    .map(|(rel, c)| format!("{rel}:{c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                (text, truncated, is_empty)
            }
            _ => {
                // files_with_matches: unique paths in first-seen order, capped at
                // MAX_RESULTS distinct files. Scanning a file stops at its first match
                // so a noisy file can't starve the cap before other files are seen.
                let mut files: Vec<String> = Vec::new();
                let mut sink = Sink::Files(&mut files);
                collect(&root, &re, glob.as_ref(), &mut sink);
                let truncated = files.len() >= MAX_RESULTS;
                let is_empty = files.is_empty();
                let text = files.join("\n");
                (text, truncated, is_empty)
            }
        };

        if is_empty {
            return ToolOutput::ok(format!("no matches for {pattern:?}"));
        }
        let mut text = text_body;
        if truncated {
            text.push_str(&format!(
                "\n… (truncated at {MAX_RESULTS} matches; narrow the pattern or path)"
            ));
        }
        ToolOutput::ok(text)
    }
}

/// Mode-aware collector for `collect`. Each variant caps traversal on a different
/// unit: `Content` caps on raw matching lines, `Files`/`Count` cap on distinct
/// matching files.
enum Sink<'a> {
    Content(&'a mut Vec<(String, usize, String)>),
    Files(&'a mut Vec<String>),
    Count(&'a mut std::collections::BTreeMap<String, usize>),
}

impl<'a> Sink<'a> {
    /// Whether the traversal-wide cap has been reached (stop walking further).
    fn capped(&self) -> bool {
        match self {
            Sink::Content(hits) => hits.len() >= MAX_RESULTS,
            Sink::Files(files) => files.len() >= MAX_RESULTS,
            Sink::Count(counts) => counts.len() >= MAX_RESULTS,
        }
    }

    /// Scan one file's contents for matches and record them per this sink's rules.
    fn record_file(&mut self, rel: &str, re: &regex::Regex, contents: &str) {
        match self {
            Sink::Content(hits) => {
                for (i, line) in contents.lines().enumerate() {
                    if hits.len() >= MAX_RESULTS {
                        return;
                    }
                    if re.is_match(line) {
                        hits.push((rel.to_string(), i + 1, line.to_string()));
                        if hits.len() >= MAX_RESULTS {
                            return;
                        }
                    }
                }
            }
            Sink::Files(files) => {
                if files.len() >= MAX_RESULTS {
                    return;
                }
                for line in contents.lines() {
                    if re.is_match(line) {
                        files.push(rel.to_string());
                        // First match found: stop scanning this file and move on,
                        // so a noisy file can't starve the cap before other files.
                        return;
                    }
                }
            }
            Sink::Count(counts) => {
                if counts.len() >= MAX_RESULTS && !counts.contains_key(rel) {
                    // File cap already reached and this is a new file: skip it.
                    return;
                }
                let n = contents.lines().filter(|line| re.is_match(line)).count();
                if n > 0 {
                    counts.insert(rel.to_string(), n);
                }
            }
        }
    }
}

/// Walk `root`, feeding each glob-matching file's contents to `sink` until its
/// cap is reached. Uses the shared [`crate::walk_files`] traversal (dotfile /
/// build-dir skip + no-symlink guard live there).
///
/// Note: `Grep` caps *during* the walk (it stops the moment `sink` is full) so a
/// huge tree is never fully materialized — the context-safety guarantee. A
/// consequence is that the truncation notice fires at `>= MAX_RESULTS`
/// (conservative: exactly `MAX_RESULTS` matches still says "truncated"), unlike
/// `Glob`, which collects everything to sort by mtime and so can report the
/// exact `> MAX_RESULTS`. The difference is inherent to the two capping
/// strategies, not a bug.
fn collect(root: &Path, re: &regex::Regex, glob: Option<&globset::GlobMatcher>, sink: &mut Sink) {
    crate::walk_files(root, |rel, full| {
        if let Some(g) = glob {
            if !g.is_match(rel) {
                return crate::Walk::Continue;
            }
        }
        if let Ok(contents) = std::fs::read_to_string(full) {
            sink.record_file(rel, re, &contents);
        }
        if sink.capped() {
            crate::Walk::Stop
        } else {
            crate::Walk::Continue
        }
    });
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
        assert!(
            !out.text.contains(":1:"),
            "default mode lists files, not lines"
        );
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
    fn files_with_matches_not_dropped_by_noisy_file() {
        let dir = tempfile::tempdir().unwrap();
        let noisy: String = std::iter::repeat_n("needle\n", 250).collect();
        std::fs::write(dir.path().join("a.rs"), noisy).unwrap();
        std::fs::write(dir.path().join("b.rs"), "needle\n").unwrap();
        let out = Grep.run(
            &json!({ "pattern": "needle", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs"), "missing a.rs: {}", out.text);
        assert!(out.text.contains("b.rs"), "missing b.rs: {}", out.text);
    }

    #[test]
    fn count_mode_counts_all_lines_but_caps_files() {
        let dir = tempfile::tempdir().unwrap();
        let noisy: String = std::iter::repeat_n("needle\n", 250).collect();
        std::fs::write(dir.path().join("a.rs"), noisy).unwrap();
        std::fs::write(dir.path().join("b.rs"), "needle\n").unwrap();
        let out = Grep.run(
            &json!({ "pattern": "needle", "path": dir.path().to_str().unwrap(), "output_mode": "count" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs:250"), "{}", out.text);
        assert!(out.text.contains("b.rs:1"), "{}", out.text);
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

    #[test]
    fn file_path_errors_not_silent_no_match() {
        let dir = seed();
        let file = dir.path().join("a.rs");
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": file.to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(
            out.is_error,
            "expected error for file path, got: {}",
            out.text
        );
        assert!(out.text.contains("not a directory"), "{}", out.text);
    }

    #[test]
    fn unknown_output_mode_is_error() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": dir.path().to_str().unwrap(), "output_mode": "lines" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("output_mode"), "{}", out.text);
    }
}
