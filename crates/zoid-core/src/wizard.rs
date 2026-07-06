//! Pure value types for the URL import wizard (Slice 4 of the mode/skill seam).
//! The bin's `github_fetch.rs` builds `UpstreamScan`; the model proposes a
//! `ModeMapping`; the bin's `mode_wizard.rs` materializes it. This module is
//! pure — no FS/network deps. Provenance serde + `classify_update` live here
//! too (Tasks 2-3).

/// One file fetched from upstream at scan time. `content` is the raw bytes
/// decoded as UTF-8 (lossy); `sha` is the GitHub blob SHA (stable identity
/// across ref moves).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub upstream_path: String,
    pub sha: String,
    pub content: String,
}

/// The scanned tree the wizard holds in `App` state. `resolved_ref` is the
/// commit SHA at scan time, so an update can re-fetch at the same ref and
/// compare SHAs apples-to-apples.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}