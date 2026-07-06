//! The URL import wizard's effectful half: `ModeImportWizard` state held in
//! `App`, `ProposeModeMappingTool` + `ApplyModeMappingTool` (Tasks 7-8), and
//! the `materialize` function that writes canonical files + a
//! `.zoid-provenance.json` sidecar to the user-global modes dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use zoid_core::skill::parse_skill_md;
use zoid_core::wizard::{
    MappingEntry, ModeMapping, ProvenanceEntry, ProvenanceFile, ProvenanceSource, UpstreamScan,
};
use zoid_provider::ToolSpec;
use zoid_tools::{Tool, ToolKind, ToolOutput};

/// The wizard state held in `App.wizard` while an import or update is in
/// flight. `scan` is cached so the chat-iterate loop never re-fetches.
/// `mode_name_target` is `Some(name)` for the update flow (the existing mode
/// being updated); `None` for import.
#[derive(Debug, Clone)]
pub struct ModeImportWizard {
    pub scan: UpstreamScan,
    pub mode_name_target: Option<String>,
    pub reconciliation_brief: Option<String>,
}

impl ModeImportWizard {
    pub fn new_import(scan: UpstreamScan) -> Self {
        Self {
            scan,
            mode_name_target: None,
            reconciliation_brief: None,
        }
    }

    pub fn new_update(scan: UpstreamScan, target: String, brief: String) -> Self {
        Self {
            scan,
            mode_name_target: Some(target),
            reconciliation_brief: Some(brief),
        }
    }
}

/// Lowercase-kebab-case filesystem-safe slug from a mode name.
pub fn slugify(name: &str) -> String {
    let raw: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars() {
        if c == '-' {
            if !prev_dash && !out.is_empty() {
                out.push(c);
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// A structured materialize failure. The wizard stays open on error; the bin
/// re-raises AskUser with the joined `problems`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeError {
    pub problems: Vec<String>,
}

impl MaterializeError {
    pub fn one(msg: impl Into<String>) -> Self {
        Self {
            problems: vec![msg.into()],
        }
    }
}

/// Validate + write canonical files + sidecar. `dest_dir` is the mode folder
/// (`<user_cfg_dir>/modes/<slug>`); the bin resolves it. Returns the dest dir
/// on success. Atomic-ish: on any mid-write failure, previously-written files
/// in this attempt are deleted (best-effort) and `Err` is returned.
pub fn materialize(
    mapping: &ModeMapping,
    scan: &UpstreamScan,
    dest_dir: &Path,
    fetched_at: &str,
) -> Result<PathBuf, MaterializeError> {
    let mut problems = Vec::new();

    // Validate mode name.
    let slug = slugify(&mapping.mode_name);
    if slug.is_empty() {
        problems.push("mode name slugifies to empty".into());
    }
    if mapping.mode_name == "default" {
        problems.push("mode name 'default' collides with the Chat floor".into());
    }

    // Validate entries: parse every proposed SKILL.md (sibling files are
    // verbatim payload), dedupe paths, check sources exist. NOTE: `mode.md` is
    // SYNTHESIZED from the mapping's mode fields (write step below), so we
    // don't parse-check its source — we control the synthesized frontmatter.
    let mut seen_paths: Vec<String> = Vec::new();
    for entry in &mapping.entries {
        match entry {
            MappingEntry::Materialize {
                canonical_path,
                source,
                ..
            } => {
                if seen_paths.iter().any(|p| p == canonical_path) {
                    problems.push(format!("duplicate canonical path '{canonical_path}'"));
                    continue;
                }
                seen_paths.push(canonical_path.clone());
                let require_parse =
                    canonical_path.ends_with("SKILL.md") && canonical_path != "mode.md";
                if canonical_path == "mode.md" && mapping.mode_name.is_empty() {
                    problems.push("mode.md entry but mode_name is empty".into());
                }
                let Some(file) = scan.files.iter().find(|f| f.upstream_path == *source) else {
                    problems.push(format!(
                        "entry references upstream path '{source}' not in scan"
                    ));
                    continue;
                };
                if require_parse {
                    if let Err(reason) = parse_skill_md(&file.content) {
                        problems.push(format!(
                            "proposed '{canonical_path}' (from {source}) fails parse: {reason}"
                        ));
                    }
                }
            }
            MappingEntry::Skip { .. } => {}
        }
    }
    if !problems.is_empty() {
        return Err(MaterializeError { problems });
    }

    // Write files. Track written paths for rollback on error.
    let mut written: Vec<PathBuf> = Vec::new();
    for entry in &mapping.entries {
        let MappingEntry::Materialize {
            canonical_path,
            source,
            ..
        } = entry
        else {
            continue;
        };
        let dest = dest_dir.join(canonical_path);
        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                rollback(&written);
                return Err(MaterializeError::one(format!(
                    "create_dir_all({}): {e}",
                    parent.display()
                )));
            }
        }
        // The content to write: for `mode.md`, SYNTHESIZE from the mapping's
        // mode fields (spec §6 — the mode manifest is composed from
        // mode_name/mode_description/mode_body, not copied from the source).
        // For all other paths, copy the source file's content verbatim.
        let content: String = if canonical_path == "mode.md" {
            let mut s = String::new();
            s.push_str("---\n");
            s.push_str(&format!("name: {}\n", mapping.mode_name));
            if !mapping.mode_description.is_empty() {
                s.push_str(&format!("description: {}\n", mapping.mode_description));
            }
            s.push_str("---\n");
            s.push_str(&mapping.mode_body);
            s
        } else {
            let file = scan
                .files
                .iter()
                .find(|f| f.upstream_path == *source)
                .expect("validated above");
            file.content.clone()
        };
        if let Err(e) = std::fs::write(&dest, &content) {
            rollback(&written);
            return Err(MaterializeError::one(format!(
                "write {}: {e}",
                dest.display()
            )));
        }
        written.push(dest);
    }

    // File-set reconciliation: delete canonical paths from the old sidecar
    // that are not in the new mapping. (Update flow; import flow has no old
    // sidecar so this is a no-op.)
    let old_sidecar = dest_dir.join(".zoid-provenance.json");
    if old_sidecar.is_file() {
        if let Ok(old_text) = std::fs::read_to_string(&old_sidecar) {
            if let Ok(old_pf) = serde_json::from_str::<ProvenanceFile>(&old_text) {
                let new_paths: std::collections::HashSet<&str> = mapping
                    .entries
                    .iter()
                    .filter_map(|e| match e {
                        MappingEntry::Materialize { canonical_path, .. } => {
                            Some(canonical_path.as_str())
                        }
                        MappingEntry::Skip { .. } => None,
                    })
                    .collect();
                for old_entry in &old_pf.files {
                    if !new_paths.contains(old_entry.canonical_path.as_str()) {
                        let p = dest_dir.join(&old_entry.canonical_path);
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
    }

    // Write the sidecar.
    let sidecar = build_sidecar(mapping, scan, fetched_at);
    let sidecar_path = dest_dir.join(".zoid-provenance.json");
    let sidecar_json = match serde_json::to_string_pretty(&sidecar) {
        Ok(s) => s,
        Err(e) => {
            rollback(&written);
            return Err(MaterializeError::one(format!("serialize sidecar: {e}")));
        }
    };
    if let Err(e) = std::fs::write(&sidecar_path, sidecar_json) {
        rollback(&written);
        return Err(MaterializeError::one(format!(
            "write sidecar {}: {e}",
            sidecar_path.display()
        )));
    }

    Ok(dest_dir.to_path_buf())
}

/// Delete any files written in this attempt (best-effort rollback).
fn rollback(written: &[PathBuf]) {
    for p in written {
        let _ = std::fs::remove_file(p);
    }
}

/// Build the provenance sidecar from a successful materialize. For `mode.md`,
/// the snapshot is the SYNTHESIZED content (what we wrote), not the source's
/// raw content — so a later `classify_update` sees `local == snapshot` when the
/// user hasn't edited.
fn build_sidecar(mapping: &ModeMapping, scan: &UpstreamScan, fetched_at: &str) -> ProvenanceFile {
    let files = mapping
        .entries
        .iter()
        .filter_map(|e| match e {
            MappingEntry::Materialize {
                canonical_path,
                source,
                ..
            } => {
                let f = scan.files.iter().find(|f| f.upstream_path == *source)?;
                let snapshot: String = if canonical_path == "mode.md" {
                    let mut s = String::new();
                    s.push_str("---\n");
                    s.push_str(&format!("name: {}\n", mapping.mode_name));
                    if !mapping.mode_description.is_empty() {
                        s.push_str(&format!("description: {}\n", mapping.mode_description));
                    }
                    s.push_str("---\n");
                    s.push_str(&mapping.mode_body);
                    s
                } else {
                    f.content.clone()
                };
                Some(ProvenanceEntry {
                    canonical_path: canonical_path.clone(),
                    upstream_path: source.clone(),
                    upstream_sha: f.sha.clone(),
                    upstream_ref: scan.resolved_ref.clone(),
                    upstream_snapshot: snapshot,
                })
            }
            MappingEntry::Skip { .. } => None,
        })
        .collect();
    ProvenanceFile {
        schema: 1,
        source: ProvenanceSource {
            url: scan.url.clone(),
            repo: scan.repo.clone(),
            ref_: scan.resolved_ref.clone(),
            subtree_path: scan.subtree_path.clone(),
            fetched_at: fetched_at.to_string(),
        },
        mode_name: mapping.mode_name.clone(),
        files,
    }
}

/// The `propose_mode_mapping` tool: returns the cached upstream scan (or the
/// reconciliation brief on update — a later task wires that path) as a tool
/// result. The model reads it, then calls `apply_mode_mapping` with its
/// proposal.
pub struct ProposeModeMappingTool {
    wizard: Arc<ModeImportWizard>,
}

impl ProposeModeMappingTool {
    pub fn new(wizard: Arc<ModeImportWizard>) -> Self {
        Self { wizard }
    }
}

impl Tool for ProposeModeMappingTool {
    fn name(&self) -> &str {
        "propose_mode_mapping"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "propose_mode_mapping".into(),
            description: "Read the upstream scan for the active import/update wizard, then call \
                apply_mode_mapping with your proposed mapping onto the canonical contract. \
                Available skills are listed in the scan; a mode.md body is the overlay text."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        if let Some(brief) = &self.wizard.reconciliation_brief {
            ToolOutput::ok(brief.clone())
        } else {
            ToolOutput::ok(render_scan(&self.wizard.scan))
        }
    }
}

/// Render the scan as a text block the model can read.
fn render_scan(scan: &UpstreamScan) -> String {
    let mut s = format!(
        "Upstream scan of {} (repo {}, ref {}, subtree {}):\n\n",
        scan.url, scan.repo, scan.resolved_ref, scan.subtree_path
    );
    for f in &scan.files {
        s.push_str(&format!(
            "---\npath: {}\nsha: {}\ncontent:\n{}\n\n",
            f.upstream_path, f.sha, f.content
        ));
    }
    s
}

/// Build the human-readable reconciliation brief for the update flow. Reads
/// the old sidecar + the on-disk canonical files, classifies each against the
/// fresh scan, and returns a text block the model reads via
/// `propose_mode_mapping`. Effectful (FS reads) — called by the bin at
/// wizard-open time.
pub fn build_reconciliation_brief(
    mode_dir: &Path,
    old_sidecar: &zoid_core::wizard::ProvenanceFile,
    fresh_scan: &UpstreamScan,
) -> String {
    let mut s = format!(
        "Update reconciliation for mode '{}' (fresh scan: {} files):\n\n",
        old_sidecar.mode_name,
        fresh_scan.files.len()
    );
    for entry in &old_sidecar.files {
        let local =
            std::fs::read_to_string(mode_dir.join(&entry.canonical_path)).unwrap_or_default();
        let class = zoid_core::wizard::classify_update(entry, &local, fresh_scan);
        s.push_str(&format!(
            "- {} (upstream {}): {}\n",
            entry.canonical_path, entry.upstream_path, class
        ));
    }
    let old_paths: std::collections::HashSet<&str> = old_sidecar
        .files
        .iter()
        .map(|f| f.upstream_path.as_str())
        .collect();
    for f in &fresh_scan.files {
        if !old_paths.contains(f.upstream_path.as_str()) {
            s.push_str(&format!(
                "- (new upstream) {}: upstream added\n",
                f.upstream_path
            ));
        }
    }
    s.push_str(
        "\nPropose a merged mapping: carry unchanged, re-materialize \
         upstream-moved (if local untouched), keep local-only-changed, decide \
         for both-changed, add new-upstream, drop or keep upstream-deleted.",
    );
    s
}

/// The `apply_mode_mapping` tool: an `Approving` tool the agent loop intercepts
/// by name. The loop parses the model's `ModeMapping` from the args, validates
/// it, and raises `AgentUpdate::ModeMappingApproval`. `run()` is never called.
pub struct ApplyModeMappingTool {
    _wizard: Arc<ModeImportWizard>,
}

impl ApplyModeMappingTool {
    pub fn new(wizard: Arc<ModeImportWizard>) -> Self {
        Self { _wizard: wizard }
    }
}

impl Tool for ApplyModeMappingTool {
    fn name(&self) -> &str {
        "apply_mode_mapping"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_mode_mapping".into(),
            description:
                "Propose a mode mapping for approval. args: { mode_name, mode_description, \
                mode_body, entries: [{ Materialize: { canonical_path, source, summary } } | \
                { Skip: { upstream_path, reason } }] }. The user approves, rejects, or adjusts."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode_name": { "type": "string" },
                    "mode_description": { "type": "string" },
                    "mode_body": { "type": "string" },
                    "entries": { "type": "array" }
                },
                "required": ["mode_name", "entries"]
            }),
        }
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Approving
    }

    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("apply_mode_mapping must be handled by the agent loop")
    }
}

/// Parse a `ModeMapping` from the tool-call args. Returns `Err` with a
/// human-readable reason if the args are malformed.
pub fn parse_mapping_args(args: &Value) -> Result<ModeMapping, String> {
    let mode_name = args
        .get("mode_name")
        .and_then(|v| v.as_str())
        .ok_or("missing 'mode_name'")?
        .to_string();
    if mode_name.is_empty() {
        return Err("mode_name is empty".into());
    }
    let mode_description = args
        .get("mode_description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mode_body = args
        .get("mode_body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let entries_arr = args
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or("missing 'entries' array")?;
    let mut entries = Vec::new();
    for (i, e) in entries_arr.iter().enumerate() {
        let mat = e.get("Materialize").unwrap_or(e);
        if let (Some(cp), Some(src)) = (
            mat.get("canonical_path").and_then(|v| v.as_str()),
            mat.get("source").and_then(|v| v.as_str()),
        ) {
            entries.push(MappingEntry::Materialize {
                canonical_path: cp.to_string(),
                source: src.to_string(),
                summary: mat
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
            continue;
        }
        let skip = e.get("Skip").unwrap_or(e);
        if let Some(up) = skip.get("upstream_path").and_then(|v| v.as_str()) {
            entries.push(MappingEntry::Skip {
                upstream_path: up.to_string(),
                reason: skip
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
            continue;
        }
        return Err(format!("entries[{i}] is neither Materialize nor Skip"));
    }
    Ok(ModeMapping {
        mode_name,
        mode_description,
        mode_body,
        entries,
    })
}

/// A one-line approval summary: mode name, skill count, skipped count.
pub fn approval_summary(mapping: &ModeMapping) -> String {
    let skills = mapping
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                MappingEntry::Materialize { canonical_path, .. }
                if canonical_path.ends_with("SKILL.md") && canonical_path != "mode.md"
            )
        })
        .count();
    let skipped = mapping
        .entries
        .iter()
        .filter(|e| matches!(e, MappingEntry::Skip { .. }))
        .count();
    format!(
        "Proposed mode '{}': {} skills, {} skipped. Approve?",
        mapping.mode_name, skills, skipped
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::wizard::ScannedFile;

    fn scan() -> UpstreamScan {
        UpstreamScan {
            url: "https://github.com/o/r/tree/main/skills".into(),
            repo: "o/r".into(),
            resolved_ref: "abc123".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/using-superpowers/SKILL.md".into(),
                    sha: "sha-u".into(),
                    content: "---\nname: using-superpowers\ndescription: d\n---\nLOADER\n".into(),
                },
                ScannedFile {
                    upstream_path: "skills/brainstorming/SKILL.md".into(),
                    sha: "sha-b".into(),
                    content: "---\nname: brainstorming\ndescription: d\n---\nBODY\n".into(),
                },
                ScannedFile {
                    upstream_path: "skills/README.md".into(),
                    sha: "sha-r".into(),
                    content: "# readme".into(),
                },
            ],
        }
    }

    fn mapping() -> ModeMapping {
        ModeMapping {
            mode_name: "Superpowers".into(),
            mode_description: "sp".into(),
            mode_body: "LOADER\n".into(),
            entries: vec![
                MappingEntry::Materialize {
                    canonical_path: "mode.md".into(),
                    source: "skills/using-superpowers/SKILL.md".into(),
                    summary: "loader as mode body".into(),
                },
                MappingEntry::Materialize {
                    canonical_path: "brainstorming/SKILL.md".into(),
                    source: "skills/brainstorming/SKILL.md".into(),
                    summary: "brainstorming skill".into(),
                },
                MappingEntry::Skip {
                    upstream_path: "skills/README.md".into(),
                    reason: "repo readme".into(),
                },
            ],
        }
    }

    #[test]
    fn slugify_lowercases_and_kebab_cases() {
        assert_eq!(slugify("Superpowers"), "superpowers");
        assert_eq!(slugify("My Cool Mode"), "my-cool-mode");
        assert_eq!(slugify("a__b!!c"), "a-b-c");
    }

    #[test]
    fn materialize_writes_files_and_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("superpowers");
        let res = materialize(&mapping(), &scan(), &dest, "2026-07-05T12:00:00Z").unwrap();
        assert_eq!(res, dest);
        assert!(dest.join("mode.md").is_file());
        assert!(dest.join("brainstorming/SKILL.md").is_file());
        assert!(dest.join(".zoid-provenance.json").is_file());
        let side = std::fs::read_to_string(dest.join(".zoid-provenance.json")).unwrap();
        let pf: ProvenanceFile = serde_json::from_str(&side).unwrap();
        assert_eq!(pf.mode_name, "Superpowers");
        assert_eq!(pf.files.len(), 2);
        assert_eq!(pf.files[0].canonical_path, "mode.md");
        assert_eq!(pf.files[0].upstream_sha, "sha-u");
    }

    #[test]
    fn materialize_rejects_default_name() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = mapping();
        m.mode_name = "default".into();
        let err = materialize(&m, &scan(), &tmp.path().join("default"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("default")));
        assert!(!tmp.path().join("default").exists());
    }

    #[test]
    fn materialize_rejects_duplicate_canonical_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = mapping();
        m.entries.push(MappingEntry::Materialize {
            canonical_path: "mode.md".into(),
            source: "skills/brainstorming/SKILL.md".into(),
            summary: "dup".into(),
        });
        let err = materialize(&m, &scan(), &tmp.path().join("m"), "t").unwrap_err();
        assert!(err
            .problems
            .iter()
            .any(|p| p.contains("duplicate canonical path 'mode.md'")));
    }

    #[test]
    fn materialize_rejects_bad_source() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = mapping();
        m.entries.push(MappingEntry::Materialize {
            canonical_path: "x/SKILL.md".into(),
            source: "skills/nope.md".into(),
            summary: "x".into(),
        });
        let err = materialize(&m, &scan(), &tmp.path().join("m"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("not in scan")));
    }

    #[test]
    fn materialize_rejects_unparseable_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = scan();
        s.files[1].content = "no frontmatter here\n".into();
        let err = materialize(&mapping(), &s, &tmp.path().join("m"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("fails parse")));
    }

    #[test]
    fn propose_tool_returns_scan_as_text() {
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ProposeModeMappingTool::new(std::sync::Arc::new(wiz));
        let out = tool.run(&serde_json::json!({}), std::path::Path::new("."));
        assert!(!out.is_error);
        assert!(out.text.contains("skills/brainstorming/SKILL.md"));
        assert!(out.text.contains("BODY"));
    }

    #[test]
    fn propose_tool_name_and_spec_agree() {
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ProposeModeMappingTool::new(std::sync::Arc::new(wiz));
        assert_eq!(tool.name(), "propose_mode_mapping");
        assert_eq!(tool.spec().name, "propose_mode_mapping");
    }

    #[test]
    fn apply_tool_is_approving_kind() {
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ApplyModeMappingTool::new(std::sync::Arc::new(wiz));
        assert_eq!(tool.kind(), zoid_tools::ToolKind::Approving);
        assert_eq!(tool.name(), "apply_mode_mapping");
    }

    #[test]
    fn apply_tool_run_is_never_called() {
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ApplyModeMappingTool::new(std::sync::Arc::new(wiz));
        let out = tool.run(&serde_json::json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out
            .text
            .contains("apply_mode_mapping must be handled by the agent loop"));
    }

    #[test]
    fn parse_mapping_args_round_trip() {
        let args = serde_json::json!({
            "mode_name": "Superpowers",
            "mode_description": "sp",
            "mode_body": "LOADER",
            "entries": [
                { "Materialize": { "canonical_path": "mode.md", "source": "skills/u/SKILL.md", "summary": "loader" } },
                { "Skip": { "upstream_path": "skills/README.md", "reason": "readme" } }
            ]
        });
        let m = parse_mapping_args(&args).unwrap();
        assert_eq!(m.mode_name, "Superpowers");
        assert_eq!(m.entries.len(), 2);
        assert!(matches!(m.entries[0], MappingEntry::Materialize { .. }));
        assert!(matches!(m.entries[1], MappingEntry::Skip { .. }));
    }

    #[test]
    fn parse_mapping_args_rejects_missing_name() {
        let err = parse_mapping_args(&serde_json::json!({ "entries": [] })).unwrap_err();
        assert!(err.contains("mode_name"));
    }

    #[test]
    fn approval_summary_counts_skills_and_skips() {
        let s = approval_summary(&mapping());
        assert!(s.contains("Superpowers"));
        assert!(s.contains("1 skills"));
        assert!(s.contains("1 skipped"));
    }
}
