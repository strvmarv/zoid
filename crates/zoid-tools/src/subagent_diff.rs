use crate::{Tool, ToolKind, ToolOutput, str_arg};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use zoid_provider::ToolSpec;

pub struct SubagentDiff;

impl Tool for SubagentDiff {
    fn name(&self) -> &str {
        "subagent_diff"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_diff".into(),
            description: "Retrieve the git diff for a completed subagent's branch. Returns the \
                          commit list, stat summary, and full diff. Use after a DelegationResult \
                          event arrives to review what the subagent changed."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subagent_id": { "type": "string", "description": "The subagent ID returned by dispatch_subagent (e.g. 'sub-01HZ...')" }
                },
                "required": ["subagent_id"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Local
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let id = match str_arg(args, "subagent_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        // The subagent ID is "sub-<ULID>"; the branch is "subagent:<ULID>".
        // Strip the "sub-" prefix and build the branch ref.
        let ulid = id.strip_prefix("sub-").unwrap_or(&id);
        let branch = format!("subagent:{ulid}");

        // Verify the branch exists.
        let verify = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &branch])
            .current_dir(cwd)
            .output();
        match verify {
            Ok(o) if !o.status.success() => {
                return ToolOutput::err(format!(
                    "subagent {id} history not found — it may have been cleaned up."
                ));
            }
            Err(e) => {
                return ToolOutput::err(format!("git rev-parse failed: {e}"));
            }
            _ => {}
        }

        // Use merge-base to diff only what the subagent committed, not working-tree changes.
        let merge_base = Command::new("git")
            .args(["merge-base", "HEAD", &branch])
            .current_dir(cwd)
            .output();
        let base = match merge_base {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => branch.clone(), // fall back to diffing the branch itself
        };
        let range = format!("{base}..{branch}");
        let log = Command::new("git")
            .args(["log", "--oneline", &range])
            .current_dir(cwd)
            .output();
        let stat = Command::new("git")
            .args(["diff", "--stat", &range])
            .current_dir(cwd)
            .output();
        let diff = Command::new("git")
            .args(["diff", "-U10", &range])
            .current_dir(cwd)
            .output();

        let mut out = String::new();
        if let Ok(o) = log {
            out.push_str("## Commits\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
            out.push('\n');
        }
        if let Ok(o) = stat {
            out.push_str("## Files changed\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
            out.push('\n');
        }
        if let Ok(o) = diff {
            out.push_str("## Diff\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
        }
        if out.trim().is_empty() {
            ToolOutput::ok(format!("subagent {id} — no changes on branch {branch}"))
        } else {
            ToolOutput::ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_diff_spec_and_kind() {
        assert_eq!(SubagentDiff.name(), "subagent_diff");
        assert_eq!(SubagentDiff.spec().name, "subagent_diff");
        assert_eq!(SubagentDiff.kind(), ToolKind::Local);
        let params = SubagentDiff.spec().parameters;
        assert_eq!(params["required"][0], "subagent_id");
    }

    #[test]
    fn subagent_diff_missing_id_is_error() {
        let out = SubagentDiff.run(&json!({}), Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("subagent_id"));
    }

    #[test]
    fn subagent_diff_nonexistent_branch_is_error() {
        let out = SubagentDiff.run(
            &json!({"subagent_id": "sub-NONEXISTENT123456"}),
            Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("not found"));
    }
}