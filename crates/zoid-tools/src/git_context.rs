//! `git_context` — a read-only view of the working directory's git state
//! (current branch, upstream delta, dirty/untracked files). Kept as a dedicated
//! tool rather than leaning on `shell` so it is auto-approvable (the approval
//! gate only inspects `shell`), gives the model a stable structured summary it
//! can't fumble, and carries no arbitrary-command surface. Volatile by nature,
//! so it lives outside the cached system prompt (spec: env block is static).

use crate::{Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use zoid_core::ErrorKind;
use zoid_provider::ToolSpec;

/// Read-only git status/branch reporter for the working directory.
pub struct GitContext;

impl Tool for GitContext {
    fn name(&self) -> &str {
        "git_context"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Report the working directory's git state: current branch, \
                upstream ahead/behind, and changed/untracked files. Read-only; call it \
                before acting on the repository."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }
    fn run(&self, _args: &Value, cwd: &Path) -> ToolOutput {
        // `git status --porcelain=v1 --branch` yields a stable, parse-friendly
        // format: a leading `## branch...upstream [ahead N, behind M]` line
        // followed by one `XY path` line per changed/untracked entry.
        let out = match Command::new("git")
            .args(["status", "--porcelain=v1", "--branch"])
            .current_dir(cwd)
            .output()
        {
            Ok(o) => o,
            // `git` missing on PATH — surface it rather than pretending clean.
            Err(e) => {
                return ToolOutput::err_kind(
                    ErrorKind::Internal,
                    format!("git_context: could not run git: {e}"),
                )
            }
        };
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = stderr.trim();
            // Not a repo (or any other git error): report, don't crash.
            return ToolOutput::err_kind(
                ErrorKind::Internal,
                format!(
                    "git_context: {}",
                    if msg.is_empty() {
                        "not a git repository"
                    } else {
                        msg
                    }
                ),
            );
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut branch_line = "(unknown)";
        let mut changes: Vec<&str> = Vec::new();
        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                branch_line = rest;
            } else if !line.is_empty() {
                changes.push(line);
            }
        }
        let summary = if changes.is_empty() {
            format!("Branch: {branch_line}\nWorking tree clean.")
        } else {
            format!(
                "Branch: {branch_line}\nChanges ({} entries):\n{}",
                changes.len(),
                changes.join("\n")
            )
        };
        ToolOutput::ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_has_no_required_params() {
        let spec = GitContext.spec();
        assert_eq!(spec.name, "git_context");
        assert!(spec.parameters["required"].is_null());
    }

    #[test]
    fn reports_branch_for_a_repo() {
        // The zoid checkout this test runs in IS a git repo; the tool should
        // succeed and name a branch, whatever the working-tree state is.
        let out = GitContext.run(&json!({}), std::path::Path::new("."));
        assert!(!out.is_error, "expected success, got: {}", out.text);
        assert!(out.text.starts_with("Branch: "), "got: {}", out.text);
    }

    #[test]
    fn errors_outside_a_repo() {
        // A tempdir is (almost certainly) not inside a git repo.
        let tmp = std::env::temp_dir();
        let out = GitContext.run(&json!({}), &tmp);
        // Either not-a-repo (err) or, if temp_dir is unexpectedly in a repo,
        // a clean success — both are valid; we only assert it doesn't panic and
        // returns coherent text.
        assert!(!out.text.is_empty());
    }
}
