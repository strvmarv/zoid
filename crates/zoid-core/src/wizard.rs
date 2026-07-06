//! Pure value types for the URL import wizard (Slice 4 of the mode/skill seam).
//! The bin's `github_fetch.rs` builds `UpstreamScan`; the model proposes a
//! `ModeMapping`; the bin's `mode_wizard.rs` materializes it. This module is
//! pure — no FS/network deps. Provenance serde + `classify_update` live here
//! too (Tasks 2-3).

use serde::{Deserialize, Serialize};

/// One file fetched from upstream at scan time. `content` is the raw bytes
/// decoded as UTF-8 (lossy); `sha` is the GitHub blob SHA (stable identity
/// across ref moves).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedFile {
    pub upstream_path: String,
    pub sha: String,
    pub content: String,
}

/// The scanned tree the wizard holds in `App` state. `resolved_ref` is the
/// commit SHA at scan time, so an update can re-fetch at the same ref and
/// compare SHAs apples-to-apples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamScan {
    pub url: String,
    pub repo: String,
    pub resolved_ref: String,
    pub subtree_path: String,
    pub files: Vec<ScannedFile>,
}

/// The model's proposal for one canonical file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingEntry {
    Materialize {
        canonical_path: String,
        source: String,
        summary: String,
    },
    Skip {
        upstream_path: String,
        reason: String,
    },
}

impl MappingEntry {
    /// The upstream path this entry concerns (the `source` for `Materialize`,
    /// the `upstream_path` for `Skip`).
    pub fn upstream_path(&self) -> &str {
        match self {
            MappingEntry::Materialize { source, .. } => source,
            MappingEntry::Skip { upstream_path, .. } => upstream_path,
        }
    }
}

/// The full proposed mapping. The model emits this as args to
/// `apply_mode_mapping`; the user approves or adjusts; the bin materializes
/// the `Materialize` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeMapping {
    pub mode_name: String,
    pub mode_description: String,
    pub mode_body: String,
    pub entries: Vec<MappingEntry>,
}

impl ModeMapping {
    /// The canonical paths that will be materialized (Skip entries excluded).
    pub fn canonical_paths(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                MappingEntry::Materialize { canonical_path, .. } => Some(canonical_path.as_str()),
                MappingEntry::Skip { .. } => None,
            })
            .collect()
    }

    /// The `Materialize` entries only.
    pub fn materialize_entries(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                MappingEntry::Materialize {
                    canonical_path,
                    source,
                    ..
                } => Some((canonical_path.as_str(), source.as_str())),
                MappingEntry::Skip { .. } => None,
            })
            .collect()
    }
}

/// One entry in the per-mode provenance sidecar. Read at update time to
/// classify each canonical file against a fresh upstream fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub canonical_path: String,
    pub upstream_path: String,
    pub upstream_sha: String,
    pub upstream_ref: String,
    pub upstream_snapshot: String,
}

/// The `source` block of the sidecar: where the mode was imported from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceSource {
    pub url: String,
    pub repo: String,
    /// The `ref` field (a reserved word, so the serde rename).
    #[serde(rename = "ref")]
    pub ref_: String,
    pub subtree_path: String,
    pub fetched_at: String,
}

/// The on-disk `.zoid-provenance.json` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceFile {
    pub schema: u32,
    pub source: ProvenanceSource,
    pub mode_name: String,
    pub files: Vec<ProvenanceEntry>,
}

/// Classification of one canonical file for update, computed by the pure
/// `classify_update` helper. The bin builds a human-readable reconciliation
/// brief from a vector of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateClass {
    Unchanged,
    UpstreamMoved { new_content: String },
    LocalOnlyChanged,
    BothChanged { new_upstream: String },
    NewUpstream { new_content: String },
    UpstreamDeleted,
}

impl std::fmt::Display for UpdateClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateClass::Unchanged => write!(f, "unchanged"),
            UpdateClass::UpstreamMoved { .. } => write!(f, "upstream changed"),
            UpdateClass::LocalOnlyChanged => write!(f, "local-only changed"),
            UpdateClass::BothChanged { .. } => write!(f, "both changed (conflict)"),
            UpdateClass::NewUpstream { .. } => write!(f, "upstream added"),
            UpdateClass::UpstreamDeleted => write!(f, "upstream deleted"),
        }
    }
}

/// Three-way classify one canonical file for update:
/// - `provenance` is the sidecar entry from last import (the base).
/// - `local_canonical` is the current on-disk canonical content.
/// - `fresh_scan` is the just-fetched upstream tree (at the same ref).
///
/// If the file's `upstream_path` is not in the fresh scan ⇒ `UpstreamDeleted`
/// (whether the scan is empty or not — a rename/move upstream is "deleted"
/// from this entry's perspective; new-upstream detection is the caller's job,
/// iterating fresh-scan files not in the sidecar). If the path is present,
/// compare SHAs: same SHA + local==snapshot ⇒ `Unchanged`; same SHA +
/// local!=snapshot ⇒ `LocalOnlyChanged`; different SHA + local==snapshot ⇒
/// `UpstreamMoved`; different SHA + local!=snapshot ⇒ `BothChanged`.
pub fn classify_update(
    provenance: &ProvenanceEntry,
    local_canonical: &str,
    fresh_scan: &UpstreamScan,
) -> UpdateClass {
    let fresh = fresh_scan
        .files
        .iter()
        .find(|f| f.upstream_path == provenance.upstream_path);
    let Some(fresh) = fresh else {
        return UpdateClass::UpstreamDeleted;
    };
    let upstream_same = fresh.sha == provenance.upstream_sha;
    let local_same = local_canonical == provenance.upstream_snapshot;
    match (upstream_same, local_same) {
        (true, true) => UpdateClass::Unchanged,
        (false, true) => UpdateClass::UpstreamMoved {
            new_content: fresh.content.clone(),
        },
        (true, false) => UpdateClass::LocalOnlyChanged,
        (false, false) => UpdateClass::BothChanged {
            new_upstream: fresh.content.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan() -> UpstreamScan {
        UpstreamScan {
            url: "https://github.com/o/r/tree/main/skills".into(),
            repo: "o/r".into(),
            resolved_ref: "abc123".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/a/SKILL.md".into(),
                    sha: "sha-a".into(),
                    content: "A".into(),
                },
                ScannedFile {
                    upstream_path: "skills/README.md".into(),
                    sha: "sha-r".into(),
                    content: "R".into(),
                },
            ],
        }
    }

    #[test]
    fn canonical_paths_excludes_skips() {
        let m = ModeMapping {
            mode_name: "M".into(),
            mode_description: "d".into(),
            mode_body: "b".into(),
            entries: vec![
                MappingEntry::Materialize {
                    canonical_path: "a/SKILL.md".into(),
                    source: "skills/a/SKILL.md".into(),
                    summary: "a".into(),
                },
                MappingEntry::Skip {
                    upstream_path: "skills/README.md".into(),
                    reason: "readme".into(),
                },
            ],
        };
        assert_eq!(m.canonical_paths(), vec!["a/SKILL.md"]);
    }

    #[test]
    fn materialize_entries_pairs_paths() {
        let m = ModeMapping {
            mode_name: "M".into(),
            mode_description: "d".into(),
            mode_body: "b".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "a/SKILL.md".into(),
                source: "skills/a/SKILL.md".into(),
                summary: "a".into(),
            }],
        };
        assert_eq!(m.materialize_entries(), vec![("a/SKILL.md", "skills/a/SKILL.md")]);
    }

    #[test]
    fn mapping_entry_upstream_path_works_for_both_variants() {
        let mat = MappingEntry::Materialize {
            canonical_path: "a/SKILL.md".into(),
            source: "skills/a/SKILL.md".into(),
            summary: "s".into(),
        };
        assert_eq!(mat.upstream_path(), "skills/a/SKILL.md");
        let skip = MappingEntry::Skip {
            upstream_path: "skills/x.md".into(),
            reason: "r".into(),
        };
        assert_eq!(skip.upstream_path(), "skills/x.md");
    }

    #[test]
    fn scan_carries_repo_ref_and_files() {
        let s = scan();
        assert_eq!(s.repo, "o/r");
        assert_eq!(s.resolved_ref, "abc123");
        assert_eq!(s.subtree_path, "skills");
        assert_eq!(s.files.len(), 2);
    }

    #[test]
    fn provenance_file_round_trips() {
        let pf = ProvenanceFile {
            schema: 1,
            source: ProvenanceSource {
                url: "https://github.com/o/r/tree/main/skills".into(),
                repo: "o/r".into(),
                ref_: "abc123".into(),
                subtree_path: "skills".into(),
                fetched_at: "2026-07-05T12:00:00Z".into(),
            },
            mode_name: "Superpowers".into(),
            files: vec![ProvenanceEntry {
                canonical_path: "brainstorming/SKILL.md".into(),
                upstream_path: "skills/brainstorming/SKILL.md".into(),
                upstream_sha: "sha-1".into(),
                upstream_ref: "abc123".into(),
                upstream_snapshot: "snap".into(),
            }],
        };
        let json = serde_json::to_string_pretty(&pf).unwrap();
        let back: ProvenanceFile = serde_json::from_str(&json).unwrap();
        assert_eq!(pf, back);
    }

    #[test]
    fn provenance_entry_canonical_path_is_relative() {
        let pf = ProvenanceFile {
            schema: 1,
            source: ProvenanceSource {
                url: "u".into(),
                repo: "o/r".into(),
                ref_: "r".into(),
                subtree_path: "s".into(),
                fetched_at: "t".into(),
            },
            mode_name: "M".into(),
            files: vec![ProvenanceEntry {
                canonical_path: "a/SKILL.md".into(),
                upstream_path: "s/a/SKILL.md".into(),
                upstream_sha: "x".into(),
                upstream_ref: "r".into(),
                upstream_snapshot: "snap".into(),
            }],
        };
        let json = serde_json::to_string(&pf).unwrap();
        assert!(!json.contains("/home/"));
        assert!(!json.contains("C:\\"));
        let back: ProvenanceFile = serde_json::from_str(&json).unwrap();
        assert!(back.files[0].canonical_path.starts_with("a/"));
    }

    fn prov(path: &str, sha: &str, snap: &str) -> ProvenanceEntry {
        ProvenanceEntry {
            canonical_path: path.into(),
            upstream_path: format!("skills/{path}"),
            upstream_sha: sha.into(),
            upstream_ref: "abc123".into(),
            upstream_snapshot: snap.into(),
        }
    }

    fn scan_with(file_path: &str, sha: &str, content: &str) -> UpstreamScan {
        UpstreamScan {
            url: "u".into(),
            repo: "o/r".into(),
            resolved_ref: "abc123".into(),
            subtree_path: "skills".into(),
            files: vec![ScannedFile {
                upstream_path: file_path.into(),
                sha: sha.into(),
                content: content.into(),
            }],
        }
    }

    #[test]
    fn classify_unchanged() {
        let p = prov("a/SKILL.md", "sha-1", "snap");
        let scan = scan_with("skills/a/SKILL.md", "sha-1", "snap");
        assert!(matches!(
            classify_update(&p, "snap", &scan),
            UpdateClass::Unchanged
        ));
    }

    #[test]
    fn classify_upstream_moved() {
        let p = prov("a/SKILL.md", "sha-1", "snap");
        let scan = scan_with("skills/a/SKILL.md", "sha-2", "new");
        match classify_update(&p, "snap", &scan) {
            UpdateClass::UpstreamMoved { new_content } => assert_eq!(new_content, "new"),
            other => panic!("expected UpstreamMoved, got {other:?}"),
        }
    }

    #[test]
    fn classify_local_only_changed() {
        let p = prov("a/SKILL.md", "sha-1", "snap");
        let scan = scan_with("skills/a/SKILL.md", "sha-1", "snap");
        assert!(matches!(
            classify_update(&p, "local-edit", &scan),
            UpdateClass::LocalOnlyChanged
        ));
    }

    #[test]
    fn classify_both_changed() {
        let p = prov("a/SKILL.md", "sha-1", "snap");
        let scan = scan_with("skills/a/SKILL.md", "sha-2", "new");
        match classify_update(&p, "local-edit", &scan) {
            UpdateClass::BothChanged { new_upstream } => assert_eq!(new_upstream, "new"),
            other => panic!("expected BothChanged, got {other:?}"),
        }
    }

    #[test]
    fn classify_upstream_deleted() {
        let p = prov("a/SKILL.md", "sha-1", "snap");
        let scan = UpstreamScan {
            url: "u".into(),
            repo: "o/r".into(),
            resolved_ref: "abc123".into(),
            subtree_path: "skills".into(),
            files: vec![],
        };
        assert!(matches!(
            classify_update(&p, "snap", &scan),
            UpdateClass::UpstreamDeleted
        ));
    }

    #[test]
    fn classify_upstream_renamed_is_deleted() {
        // A non-empty scan whose path doesn't match the provenance entry's
        // upstream_path is a rename/move upstream — from the provenance entry's
        // perspective, its file is deleted. (New-upstream detection is the
        // caller's job, iterating fresh-scan files not in the sidecar.)
        let p = prov("old/SKILL.md", "sha-old", "snap");
        let scan = scan_with("skills/new/SKILL.md", "sha-new", "new-content");
        assert!(matches!(
            classify_update(&p, "snap", &scan),
            UpdateClass::UpstreamDeleted
        ));
    }
}