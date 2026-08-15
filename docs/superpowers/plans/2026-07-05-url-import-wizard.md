# URL Import Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn a GitHub tree URL into an on-disk mode folder by having the active model propose a mapping onto the canonical contract, the user approves via the existing AskUser overlay, zoid materializes canonical files + a `.zoid-provenance.json` sidecar at `~/.config/zoid/modes/<slug>/`, then reloads. `:mode update <name>` re-runs the same loop with the sidecar as a three-way base for model-driven update reconciliation.

**Architecture:** A new pure `zoid-core/src/wizard.rs` carries value types (`UpstreamScan`, `ModeMapping`, `ProvenanceEntry`, `UpdateClass`, `classify_update`). The bin gains `github_fetch.rs` (reqwest + `GithubApi` trait for testability), `mode_wizard.rs` (`ProposeModeMappingTool` + `ApplyModeMappingTool` + `materialize`), and `App.wizard: Option<ModeImportWizard>` state. The agent loop gets a new `ToolKind::Approving` variant + dispatch arm that raises `AgentUpdate::ModeMappingApproval`; the bin's UI handler shows the existing AskUser overlay and, on Approve, runs the materializer + reload + clear-wizard (the loop stays thin; the bin is the composition root). The runtime below the waist (`mode_import.rs`) is unchanged — the wizard's only output is canonical files on disk.

**Tech Stack:** Rust 2021 workspace. `zoid-core` (pure domain types, serde), `zoid` bin+lib (agent loop, fetcher, materializer, composition root), `zoid-tools` (`Tool`/`ToolKind`/`ToolOutput`), `zoid-provider` (`ToolSpec`), `zoid-tui` (commands/overlay). `reqwest` (workspace, already used by `update.rs`), `serde`/`serde_json`, `tempfile` (tests). Tests via `cargo test`, deterministic `ScriptedProvider`/`FakeGithubApi`.

## Global Constraints

- **`zoid-core` stays pure** — no `std::fs` (beyond `parse_skill_md`'s existing none), no `reqwest`, no process, no network. `wizard.rs` is pure types + `classify_update`. All fetch/IO lives in the `zoid` bin.
- **The runtime below the waist is unchanged.** `mode_import.rs` and `skill_import.rs` are not modified. The wizard's only output is canonical files on disk; `mode_import` reads them as before.
- **Imported modes are user-global.** The materializer writes to `~/.config/zoid/modes/<slug>/` (the user-global convention dir), never `<cwd>/.zoid/modes/`. Repo-local modes stay hand-authored.
- **Zero regression on the default path.** With no wizard active, `spawn_turn` builds an identical tool set to today. The wizard tools are gated by `App.wizard.is_some()`.
- **Every tool failure returns a `ToolOutput::err`, never a panic or `Err`.** Mirrors the existing convention. The materializer returns `Result<_, MaterializeError>` and the bin surfaces failures via AskUser re-raise, never a panic.
- **The agent loop stays thin.** `apply_mode_mapping`'s approval gate raises a new `AgentUpdate::ModeMappingApproval`; the bin (not the loop) runs the materializer + reload. The loop only parks for the reply and emits the tool result.
- **No `Co-Authored-By` / co-author trailer** in commit messages (repo rule).
- **Per task:** `cargo test --workspace` green, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo fmt --all` clean. TDD (failing test first). Commit at the end of each task.

---

## File Structure

**Created:**
- `crates/zoid-core/src/wizard.rs` — pure value types (`ScannedFile`, `UpstreamScan`, `MappingEntry`, `ModeMapping`, `ProvenanceEntry`, `ProvenanceFile`, `UpdateClass`, `classify_update`, sidecar serde).
- `crates/zoid/src/github_fetch.rs` — `GithubApi` trait, `HttpGithubApi` (reqwest), `FakeGithubApi` (tests), URL parser, `fetch_tree`.
- `crates/zoid/src/mode_wizard.rs` — `ModeImportWizard` state, `ProposeModeMappingTool`, `ApplyModeMappingTool`, `materialize`, `MaterializeError`, `slugify`.
- `crates/zoid/tests/mode_import_wiring.rs` — integration test: scripted provider + injected scan → AskUser → Approve → files materialized + mode loads.
- `crates/zoid/tests/mode_update_wiring.rs` — integration test: pre-seeded mode + sidecar → fresh scan → merged mapping → Approve → file-set reconciled.
- `docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md` — Tier-2 go/no-go runbook (an internal-process artifact, not carried into the public repo).

**Modified:**
- `crates/zoid-core/src/lib.rs` — `pub mod wizard;`.
- `crates/zoid-tools/src/lib.rs` — add `ToolKind::Approving`.
- `crates/zoid/src/agent.rs` — `AgentUpdate::ModeMappingApproval`; new dispatch arm for `ToolKind::Approving` + `apply_mode_mapping`.
- `crates/zoid/src/lib.rs` — `pub mod github_fetch; pub mod mode_wizard;`.
- `crates/zoid/src/main.rs` — `App.wizard`; `ModeImport`/`ModeUpdate` command handlers; `ModeMappingApproval` UI handler (overlay + materialize-on-approve); `spawn_turn` tool gating; test `App` literal.
- `crates/zoid-tui/src/command.rs` — `Command::ModeImport(String)` + `Command::ModeUpdate(String)`; parser.

**Dependency order:** T1 (pure types) → T2 (sidecar serde) → T3 (classify_update) → T4 (ToolKind) → T5 (github_fetch) → T6 (materializer) → T7 (propose tool) → T8 (apply tool + AgentUpdate) → T9 (agent loop arm) → T10 (commands) → T11 (App wiring) → T12 (UI handler + spawn_turn gating) → T13 (integration: import) → T14 (integration: update) → T15 (runbook).

---

## Task 1: Pure wizard value types (`zoid-core/src/wizard.rs`)

**Files:**
- Create: `crates/zoid-core/src/wizard.rs`
- Modify: `crates/zoid-core/src/lib.rs:4` (add module registration)

**Interfaces:**
- Consumes: nothing (pure new types).
- Produces: `ScannedFile`, `UpstreamScan`, `MappingEntry`, `ModeMapping`.

- [ ] **Step 1: Register the module**

In `crates/zoid-core/src/lib.rs`, add after line 22 (`pub mod zoom;`):

```rust
pub mod wizard;
```

- [ ] **Step 2: Write the failing tests + type stubs**

Create `crates/zoid-core/src/wizard.rs`:

```rust
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
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid-core wizard`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-core/src/wizard.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): pure wizard value types (UpstreamScan, ModeMapping, MappingEntry)"
```

---

## Task 2: Provenance sidecar serde (`zoid-core/src/wizard.rs`)

**Files:**
- Modify: `crates/zoid-core/src/wizard.rs` (append `ProvenanceEntry` + `ProvenanceFile` + serde + tests)

**Interfaces:**
- Consumes: `serde` (workspace, `derive`), `serde_json` (workspace).
- Produces: `ProvenanceEntry`, `ProvenanceFile` (the on-disk JSON shape), `read_provenance`, `write_provenance_str`.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `crates/zoid-core/src/wizard.rs` (before its closing `}`):

```rust
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
        // No absolute path markers survive the round-trip.
        assert!(!json.contains("/home/"));
        assert!(!json.contains("C:\\"));
        let back: ProvenanceFile = serde_json::from_str(&json).unwrap();
        assert!(back.files[0].canonical_path.starts_with("a/"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core provenance`
Expected: FAIL — compile error, `cannot find type ProvenanceFile`.

- [ ] **Step 3: Implement the provenance types**

Append to `crates/zoid-core/src/wizard.rs` (before the `#[cfg(test)]` module):

```rust
use serde::{Deserialize, Serialize};

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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core provenance`
Expected: PASS (2 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/wizard.rs
git commit -m "feat(core): provenance sidecar serde (ProvenanceFile/Entry/Source)"
```

---

## Task 3: `classify_update` (`zoid-core/src/wizard.rs`)

**Files:**
- Modify: `crates/zoid-core/src/wizard.rs` (append `UpdateClass` + `classify_update` + tests)

**Interfaces:**
- Consumes: `ProvenanceEntry`, `UpstreamScan`.
- Produces: `enum UpdateClass`, `fn classify_update`.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block:

```rust
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
    fn classify_new_upstream() {
        let p = prov("old/SKILL.md", "sha-old", "snap");
        let scan = scan_with("skills/new/SKILL.md", "sha-new", "new-content");
        match classify_update(&p, "snap", &scan) {
            UpdateClass::NewUpstream { new_content } => assert_eq!(new_content, "new-content"),
            other => panic!("expected NewUpstream, got {other:?}"),
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core classify`
Expected: FAIL — compile error, `cannot find type UpdateClass`.

- [ ] **Step 3: Implement `UpdateClass` + `classify_update`**

Append to `crates/zoid-core/src/wizard.rs` (before the `#[cfg(test)]` module):

```rust
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
/// If the file's `upstream_path` is not in the fresh scan ⇒ `UpstreamDeleted`.
/// If it is, compare SHAs: same SHA + local==snapshot ⇒ `Unchanged`; same SHA +
/// local!=snapshot ⇒ `LocalOnlyChanged`; different SHA + local==snapshot ⇒
/// `UpstreamMoved`; different SHA + local!=snapshot ⇒ `BothChanged`. A
/// `provenance.upstream_path` that doesn't match any fresh-scan path is treated
/// as `UpstreamDeleted` (the file was renamed/moved upstream).
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core classify`
Expected: PASS (6 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/wizard.rs
git commit -m "feat(core): classify_update three-way merge classification"
```

---

## Task 4: `ToolKind::Approving` (`zoid-tools`)

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs:47-52` (add `Approving` variant)

**Interfaces:**
- Consumes: nothing new.
- Produces: `ToolKind::Approving`.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/zoid-tools/src/lib.rs` (if none exists, create one at the end of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approving_variant_exists_and_is_distinct() {
        assert_ne!(ToolKind::Approving, ToolKind::Local);
        assert_ne!(ToolKind::Approving, ToolKind::Emitting);
        assert_ne!(ToolKind::Approving, ToolKind::Interactive);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tools approving_variant`
Expected: FAIL — compile error, `no variant Approving`.

- [ ] **Step 3: Add the variant**

In `crates/zoid-tools/src/lib.rs`, replace the `ToolKind` enum (lines 47-52):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Local,
    Emitting,
    Interactive,
    /// A tool that requires user approval before its effect lands (e.g.
    /// `apply_mode_mapping`). The agent loop intercepts it by name, raises a
    /// UI approval prompt, and parks until the user answers. `run()` is never
    /// called; the loop emits the tool result from the approval outcome.
    Approving,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tools approving_variant`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/lib.rs
git commit -m "feat(tools): ToolKind::Approving for approval-gate tools"
```

---

## Task 5: GitHub fetcher (`zoid/src/github_fetch.rs`)

**Files:**
- Create: `crates/zoid/src/github_fetch.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod github_fetch;`)

**Interfaces:**
- Consumes: `reqwest` (workspace), `serde_json`, `zoid_core::wizard::{ScannedFile, UpstreamScan}`.
- Produces: `GithubApi` trait, `HttpGithubApi`, `FakeGithubApi`, `parse_github_url`, `fetch_tree`, `GithubUrl`.

- [ ] **Step 1: Register the module**

In `crates/zoid/src/lib.rs`, add after `pub mod eventlog;` (line 6):

```rust
pub mod github_fetch;
```

- [ ] **Step 2: Write the failing tests + URL parser**

Create `crates/zoid/src/github_fetch.rs`:

```rust
//! GitHub tree fetcher for the URL import wizard. Resolves a
//! `github.com/{owner}/{repo}/tree/{ref}/{path}` URL via the GitHub HTTP API
//! (api.github.com/repos/.../git/trees/...?recursive=1) and assembles an
//! `UpstreamScan`. `$GITHUB_TOKEN` is used if present (higher rate limit,
//! private repos). HTTP calls are behind a `GithubApi` trait so tests use
//! `FakeGithubApi` with no real network.

use std::sync::Arc;

use serde_json::Value;
use zoid_core::wizard::{ScannedFile, UpstreamScan};

/// The parsed GitHub URL: owner/repo, ref, and subtree path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubUrl {
    pub owner: String,
    pub repo: String,
    pub ref_: String,
    pub subtree_path: String,
}

/// Parse a `github.com/{owner}/{repo}/tree/{ref}/{path}` URL. Also accepts
/// `/blob/{ref}/{path}` (a single file — the scan will have one entry). Returns
/// `Err` with a human-readable reason for non-GitHub URLs or malformed shapes.
pub fn parse_github_url(url: &str) -> Result<GithubUrl, String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://github.com/")
        .or_else(|| u.strip_prefix("http://github.com/"))
        .or_else(|| u.strip_prefix("github.com/"))
        .ok_or_else(|| format!("URL import supports github.com URLs only (got '{u}')"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    // /owner/repo/tree/ref/path...  (>= 5 parts)
    // /owner/repo/blob/ref/path...  (>= 5 parts)
    if parts.len() < 5 {
        return Err(format!(
            "expected github.com/{{owner}}/{{repo}}/tree/{{ref}}/{{path}}, got '{u}'"
        ));
    }
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let kind = parts[2];
    let ref_ = parts[3].to_string();
    if kind != "tree" && kind != "blob" {
        return Err(format!(
            "expected '/tree/{{ref}}/{{path}}' or '/blob/{{ref}}/{{path}}' in '{u}'"
        ));
    }
    let subtree_path = parts[4..].join("/");
    Ok(GithubUrl {
        owner,
        repo,
        ref_,
        subtree_path,
    })
}

/// The GitHub API seam. `HttpGithubApi` hits the real API; `FakeGithubApi`
/// returns canned JSON for tests.
#[async_trait::async_trait]
pub trait GithubApi: Send + Sync {
    /// Fetch the recursive tree JSON for `owner/repo` at `ref`. Returns the
    /// raw `git/trees` response (a `tree` array of `{ path, sha, type }`).
    async fn fetch_tree_json(&self, owner: &str, repo: &str, ref_: &str)
        -> anyhow::Result<Value>;

    /// Fetch the raw content of one blob by its `download_url` (for `type:
    /// "blob"` entries) OR by path at ref (fallback). Returns the decoded text.
    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String>;
}

/// Real GitHub API client. `token` is `$GITHUB_TOKEN` if set.
pub struct HttpGithubApi {
    client: reqwest::Client,
    token: Option<String>,
}

impl HttpGithubApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("zoid-wizard/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client builds"),
            token: std::env::var("GITHUB_TOKEN").ok(),
        }
    }
}

#[async_trait::async_trait]
impl GithubApi for HttpGithubApi {
    async fn fetch_tree_json(
        &self,
        owner: &str,
        repo: &str,
        ref_: &str,
    ) -> anyhow::Result<Value> {
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/git/trees/{ref_}?recursive=1"
        );
        let mut req = self.client.get(&url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        if resp.status().as_u16() == 403 {
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            if remaining == "0" {
                anyhow::bail!("GitHub rate-limited. Set $GITHUB_TOKEN for a higher limit.");
            }
        }
        resp.error_for_status()?.json().await
    }

    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String> {
        let mut req = self.client.get(download_url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        resp.error_for_status()?.text().await
    }
}

/// Fetch the subtree at `GithubUrl.subtree_path` and assemble an `UpstreamScan`.
/// Files outside `subtree_path` are excluded. `resolved_ref` is the SHA the API
/// resolved `ref_` to (from the tree JSON's `sha` field).
pub async fn fetch_tree(
    api: &dyn GithubApi,
    url: &GithubUrl,
) -> anyhow::Result<UpstreamScan> {
    let tree_json = api.fetch_tree_json(&url.owner, &url.repo, &url.ref_).await?;
    let resolved_ref = tree_json
        .get("sha")
        .and_then(|v| v.as_str())
        .unwrap_or(&url.ref_)
        .to_string();
    let entries = tree_json
        .get("tree")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("GitHub tree response has no 'tree' array"))?;
    let prefix = if url.subtree_path.is_empty() {
        String::new()
    } else {
        format!("{}/", url.subtree_path)
    };
    let mut files = Vec::new();
    for entry in entries {
        let etype = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if etype != "blob" {
            continue;
        }
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !path.starts_with(&prefix) {
            continue;
        }
        let sha = entry
            .get("sha")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Fetch raw content via the raw.githubusercontent.com URL derived from
        // owner/repo/ref/path (avoids a second API call per blob).
        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{path}",
            url.owner, url.repo, url.ref_
        );
        let content = api.fetch_blob_content(&raw_url).await?;
        files.push(ScannedFile {
            upstream_path: path.to_string(),
            sha,
            content,
        });
    }
    Ok(UpstreamScan {
        url: format!(
            "https://github.com/{}/{}/tree/{}/{}",
            url.owner, url.repo, url.ref_, url.subtree_path
        ),
        repo: format!("{}/{}", url.owner, url.repo),
        resolved_ref,
        subtree_path: url.subtree_path.clone(),
        files,
    })
}

/// A fake API for tests. Returns a canned tree JSON + per-path content.
pub struct FakeGithubApi {
    pub tree_json: Value,
    pub contents: std::collections::HashMap<String, String>,
}

#[async_trait::async_trait]
impl GithubApi for FakeGithubApi {
    async fn fetch_tree_json(
        &self,
        _owner: &str,
        _repo: &str,
        _ref_: &str,
    ) -> anyhow::Result<Value> {
        Ok(self.tree_json.clone())
    }

    async fn fetch_blob_content(&self, download_url: &str) -> anyhow::Result<String> {
        self.contents
            .get(download_url)
            .cloned()
            .ok_or_else(|| anyhow!("FakeGithubApi: no content for {download_url}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_url() {
        let g = parse_github_url("github.com/obra/superpowers/tree/main/skills").unwrap();
        assert_eq!(g.owner, "obra");
        assert_eq!(g.repo, "superpowers");
        assert_eq!(g.ref_, "main");
        assert_eq!(g.subtree_path, "skills");
    }

    #[test]
    fn parses_blob_url() {
        let g = parse_github_url("https://github.com/o/r/blob/main/skills/a/SKILL.md").unwrap();
        assert_eq!(g.ref_, "main");
        assert_eq!(g.subtree_path, "skills/a/SKILL.md");
    }

    #[test]
    fn parses_nested_subtree_path() {
        let g =
            parse_github_url("github.com/o/r/tree/main/skills/brainstorming/scripts").unwrap();
        assert_eq!(g.subtree_path, "skills/brainstorming/scripts");
    }

    #[test]
    fn rejects_non_github() {
        let err = parse_github_url("gitlab.com/o/r/tree/main/skills").unwrap_err();
        assert!(err.contains("github.com URLs only"));
    }

    #[test]
    fn rejects_no_tree() {
        let err = parse_github_url("github.com/obra/superpowers").unwrap_err();
        assert!(err.contains("tree"));
    }

    #[test]
    fn rejects_malformed_kind() {
        let err = parse_github_url("github.com/o/r/branches/main/skills").unwrap_err();
        assert!(err.contains("tree") || err.contains("blob"));
    }

    fn fake_tree() -> Value {
        serde_json::json!({
            "sha": "abc123",
            "tree": [
                { "path": "skills/a/SKILL.md", "sha": "sha-a", "type": "blob" },
                { "path": "skills/README.md", "sha": "sha-r", "type": "blob" },
                { "path": "skills/sub", "sha": "sha-tree", "type": "tree" }
            ]
        })
    }

    fn fake_contents() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "https://raw.githubusercontent.com/o/r/main/skills/a/SKILL.md".into(),
            "A BODY".into(),
        );
        m.insert(
            "https://raw.githubusercontent.com/o/r/main/skills/README.md".into(),
            "README".into(),
        );
        m
    }

    #[tokio::test]
    async fn fetch_tree_assembles_scan_with_subtree_filter() {
        let api = FakeGithubApi {
            tree_json: fake_tree(),
            contents: fake_contents(),
        };
        let url = parse_github_url("github.com/o/r/tree/main/skills").unwrap();
        let scan = fetch_tree(&api, &url).await.unwrap();
        assert_eq!(scan.repo, "o/r");
        assert_eq!(scan.resolved_ref, "abc123");
        assert_eq!(scan.subtree_path, "skills");
        assert_eq!(scan.files.len(), 2);
        let a = scan.files.iter().find(|f| f.upstream_path == "skills/a/SKILL.md").unwrap();
        assert_eq!(a.sha, "sha-a");
        assert_eq!(a.content, "A BODY");
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid --lib github_fetch`
Expected: 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/github_fetch.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): github_fetch — URL parser + GithubApi trait + fetch_tree"
```

---

## Task 6: Materializer (`zoid/src/mode_wizard.rs`)

**Files:**
- Create: `crates/zoid/src/mode_wizard.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod mode_wizard;`)

**Interfaces:**
- Consumes: `zoid_core::wizard::*`, `zoid_core::skill::parse_skill_md`, `std::fs`.
- Produces: `slugify`, `MaterializeError`, `materialize`, `ModeImportWizard`.

- [ ] **Step 1: Register the module**

In `crates/zoid/src/lib.rs`, add after `pub mod github_fetch;`:

```rust
pub mod mode_wizard;
```

- [ ] **Step 2: Write the failing tests + materializer**

Create `crates/zoid/src/mode_wizard.rs`:

```rust
//! The URL import wizard's effectful half: `ModeImportWizard` state held in
//! `App`, `ProposeModeMappingTool` + `ApplyModeMappingTool` (Tasks 7-8), and
//! the `materialize` function that writes canonical files + a
//! `.zoid-provenance.json` sidecar to the user-global modes dir.

use std::path::{Path, PathBuf};

use serde_json::Value;
use zoid_core::skill::parse_skill_md;
use zoid_core::wizard::{
    MappingEntry, ModeMapping, ProvenanceEntry, ProvenanceFile, ProvenanceSource, ScannedFile,
    UpstreamScan,
};
use zoid_provider::ToolSpec;
use zoid_tools::{Tool, ToolOutput};

/// The wizard state held in `App.wizard` while an import or update is in
/// flight. `scan` is cached so the chat-iterate loop never re-fetches.
/// `mode_name_target` is `Some(name)` for the update flow (the existing mode
/// being updated); `None` for import.
#[derive(Debug, Clone)]
pub struct ModeImportWizard {
    pub scan: UpstreamScan,
    pub mode_name_target: Option<String>,
}

impl ModeImportWizard {
    pub fn new_import(scan: UpstreamScan) -> Self {
        Self {
            scan,
            mode_name_target: None,
        }
    }

    pub fn new_update(scan: UpstreamScan, target: String) -> Self {
        Self {
            scan,
            mode_name_target: Some(target),
        }
    }
}

/// Lowercase-kebab-case filesystem-safe slug from a mode name.
pub fn slugify(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
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
    // SYNTHESIZED from the mapping's mode fields (Task 6 write step), so we
    // don't parse-check its source — we parse-check the synthesized content
    // after building it would be redundant since we control the frontmatter.
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
                // mode.md is synthesized from mapping fields; its source still
                // must exist in the scan (the body may be derived from it), but
                // we don't parse-check the source content.
                let require_parse = canonical_path.ends_with("SKILL.md") && canonical_path != "mode.md";
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
    // Exactly one or zero mode.md (zero is allowed; behaves like Chat).
    if !problems.is_empty() {
        return Err(MaterializeError { problems });
    }

    // Write files. Track written paths for rollback on error.
    let mut written: Vec<PathBuf> = Vec::new();
    for entry in &mapping.entries {
        let MappingEntry::Materialize {
            canonical_path, source, ..
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

/// Build the provenance sidecar from a successful materialize.
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
                // For mode.md, the snapshot is the SYNTHESIZED content (what
                // we wrote), not the source's raw content — so a later
                // `classify_update` sees `local == snapshot` when the user
                // hasn't edited. For other paths, it's the verbatim source.
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
        assert_eq!(pf.files.len(), 2); // mode.md + brainstorming
        assert_eq!(pf.files[0].canonical_path, "mode.md");
        assert_eq!(pf.files[0].upstream_sha, "sha-u");
    }

    #[test]
    fn materialize_rejects_default_name() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = mapping();
        m.mode_name = "default".into();
        let err = materialize(&m, &scan(), tmp.path().join("default"), "t").unwrap_err();
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
        let err = materialize(&m, &scan(), tmp.path().join("m"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("duplicate canonical path 'mode.md'")));
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
        let err = materialize(&m, &scan(), tmp.path().join("m"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("not in scan")));
    }

    #[test]
    fn materialize_rejects_unparseable_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = scan();
        s.files[1].content = "no frontmatter here\n".into(); // brainstorming now bad
        let err = materialize(&mapping(), &s, tmp.path().join("m"), "t").unwrap_err();
        assert!(err.problems.iter().any(|p| p.contains("fails parse")));
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p zoid --lib mode_wizard`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): mode_wizard — ModeImportWizard + slugify + materialize"
```

---

## Task 7: `ProposeModeMappingTool` (`zoid/src/mode_wizard.rs`)

**Files:**
- Modify: `crates/zoid/src/mode_wizard.rs` (append tool + tests)

**Interfaces:**
- Consumes: `ModeImportWizard` (held by the tool), `Tool`, `ToolOutput`, `ToolSpec`.
- Produces: `ProposeModeMappingTool`.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `crates/zoid/src/mode_wizard.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib propose_tool`
Expected: FAIL — compile error, `cannot find type ProposeModeMappingTool`.

- [ ] **Step 3: Implement the tool**

Append to `crates/zoid/src/mode_wizard.rs` (before the `#[cfg(test)]` module):

```rust
use std::sync::Arc;

/// The `propose_mode_mapping` tool: returns the cached upstream scan (or the
/// reconciliation brief on update — Task 8 wires that path) as a tool result.
/// The model reads it, then calls `apply_mode_mapping` with its proposal.
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
        ToolOutput::ok(render_scan(&self.wizard.scan))
    }
}

/// Render the scan as a text block the model can read: one section per file
/// with its upstream path + content. (The update flow's reconciliation brief
/// is built separately by the bin and passed via the wizard; Task 8.)
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib propose_tool`
Expected: PASS (2 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs
git commit -m "feat(zoid): ProposeModeMappingTool returns the cached scan"
```

---

## Task 8: `ApplyModeMappingTool` + `ModeMappingApproval` (`zoid/src/mode_wizard.rs` + `agent.rs`)

**Files:**
- Modify: `crates/zoid/src/mode_wizard.rs` (append `ApplyModeMappingTool` + tests)
- Modify: `crates/zoid/src/agent.rs` (add `AgentUpdate::ModeMappingApproval`)

**Interfaces:**
- Consumes: `ModeImportWizard`, `ModeMapping`, `Tool`/`ToolKind::Approving`, `AgentUpdate`.
- Produces: `ApplyModeMappingTool`, `AgentUpdate::ModeMappingApproval`.

- [ ] **Step 1: Add the `AgentUpdate` variant**

In `crates/zoid/src/agent.rs`, add to the `AgentUpdate` enum (after the `AskUser` variant, ~line 118):

```rust
    /// The model proposed a mode mapping via `apply_mode_mapping`; the loop
    /// validated it and is parking for user approval. `reply` receives the
    /// user's decision: "Approve" (materialize), "Reject" (cancel), or
    /// free-text (adjust — re-propose). The bin, not the loop, runs the
    /// materializer on "Approve".
    ModeMappingApproval {
        mapping: zoid_core::wizard::ModeMapping,
        summary: String,
        reply: oneshot::Sender<String>,
    },
```

- [ ] **Step 2: Write the failing test for the tool**

Append to the `#[cfg(test)] mod tests` block in `crates/zoid/src/mode_wizard.rs`:

```rust
    #[test]
    fn apply_tool_is_approving_kind() {
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ApplyModeMappingTool::new(std::sync::Arc::new(wiz));
        assert_eq!(tool.kind(), zoid_tools::ToolKind::Approving);
        assert_eq!(tool.name(), "apply_mode_mapping");
    }

    #[test]
    fn apply_tool_run_is_never_called() {
        // The loop intercepts Approving tools by name; run() is a stub.
        let wiz = ModeImportWizard::new_import(scan());
        let tool = ApplyModeMappingTool::new(std::sync::Arc::new(wiz));
        let out = tool.run(&serde_json::json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("apply_mode_mapping must be handled by the agent loop"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p zoid --lib apply_tool`
Expected: FAIL — compile error, `cannot find type ApplyModeMappingTool`.

- [ ] **Step 4: Implement `ApplyModeMappingTool`**

Append to `crates/zoid/src/mode_wizard.rs` (before the `#[cfg(test)]` module):

```rust
use zoid_tools::ToolKind;

/// The `apply_mode_mapping` tool: an `Approving` tool the agent loop intercepts
/// by name. The loop parses the model's `ModeMapping` from the args, validates
/// it, and raises `AgentUpdate::ModeMappingApproval`. `run()` is never called
/// (the stub errors out, mirroring `ask_user`).
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
            description: "Propose a mode mapping for approval. args: { mode_name, mode_description, \
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
/// human-readable reason if the args are malformed. The loop calls this before
/// raising `ModeMappingApproval` so a malformed proposal is fed back to the
/// model as a tool error (no approval prompt for a bad mapping).
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
        // Accept either { "Materialize": {...} } or a flat { canonical_path, source, summary }.
        let mat = e.get("Materialize").unwrap_or(e);
        if let (Some(cp), Some(src)) = (mat.get("canonical_path").and_then(|v| v.as_str()), mat.get("source").and_then(|v| v.as_str()))
        {
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
        .filter(|e| matches!(e, MappingEntry::Materialize { canonical_path, .. } if canonical_path.ends_with("SKILL.md") && canonical_path != "mode.md"))
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
```

- [ ] **Step 5: Add a test for `parse_mapping_args` + `approval_summary`**

Append to the tests block:

```rust
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
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid --lib mode_wizard`
Expected: PASS (all mode_wizard tests).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs crates/zoid/src/agent.rs
git commit -m "feat(zoid): ApplyModeMappingTool + AgentUpdate::ModeMappingApproval"
```

---

## Task 9: Agent loop dispatch arm for `ToolKind::Approving` (`zoid/src/agent.rs`)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (~line 645, the `match kind` block)

**Interfaces:**
- Consumes: `ToolKind::Approving`, `parse_mapping_args`, `approval_summary`, `AgentUpdate::ModeMappingApproval`.
- Produces: a new match arm that validates the mapping, raises `ModeMappingApproval`, parks for the reply, and emits the tool result.

- [ ] **Step 1: Write the failing integration test**

Create `crates/zoid/tests/mode_wizard_loop.rs`:

```rust
//! Wiring proof: the agent loop intercepts apply_mode_mapping (Approving),
//! raises ModeMappingApproval, and on "Approve" emits a non-error ToolResult
//! carrying the approval. Deterministic — no real model.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid::mode_wizard::{
    ApplyModeMappingTool, ModeImportWizard, ProposeModeMappingTool,
};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::wizard::{MappingEntry, ModeMapping, ScannedFile, UpstreamScan};
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        self.requests.lock().unwrap().push(req.clone());
        let script = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

fn scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "abc".into(),
        subtree_path: "skills".into(),
        files: vec![ScannedFile {
            upstream_path: "skills/a/SKILL.md".into(),
            sha: "sha-a".into(),
            content: "---\nname: a\ndescription: d\n---\nBODY\n".into(),
        }],
    }
}

#[tokio::test]
async fn apply_mode_mapping_raises_approval_and_approve_emits_tool_result() {
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call(
                    "apply_mode_mapping",
                    serde_json::json!({
                        "mode_name": "M",
                        "mode_description": "d",
                        "mode_body": "",
                        "entries": [{
                            "Materialize": {
                                "canonical_path": "a/SKILL.md",
                                "source": "skills/a/SKILL.md",
                                "summary": "a"
                            }
                        }]
                    }),
                ),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("done"), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let wiz = Arc::new(ModeImportWizard::new_import(scan()));
    let mut tools = zoid::invoke_skill::chat_tools(Arc::new(
        zoid_core::skill::SkillRegistry::builtin(),
    ));
    tools.push(Box::new(ProposeModeMappingTool::new(wiz.clone())));
    tools.push(Box::new(ApplyModeMappingTool::new(wiz.clone())));
    let tools = Arc::new(tools);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "import the mode".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let approvals = Arc::new(Mutex::new(Vec::<ModeMapping>::new()));
    let approvals_for_task = approvals.clone();
    let handle = tokio::spawn(async move {
        while let Some(upd) = rx.recv().await {
            if let AgentUpdate::ModeMappingApproval { mapping, reply, .. } = upd {
                approvals_for_task.lock().unwrap().push(mapping);
                let _ = reply.send("Approve".to_string());
            }
        }
    });

    run_agent_turn(
        zoid::agent::chat_turn_config_with(&zoid::agent::default_profile(), ""),
        provider.clone(),
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    handle.await.unwrap();

    let captured = approvals.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].mode_name, "M");

    let log = session.snapshot().await.unwrap();
    let result = log.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult { name, is_error, .. } if name == "apply_mode_mapping" => {
            Some(*is_error)
        }
        _ => None,
    });
    assert_eq!(result, Some(false), "approve => non-error tool result");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid --test mode_wizard_loop`
Expected: FAIL — the loop doesn't handle `ToolKind::Approving` yet; `apply_mode_mapping.run()` is called (the stub) and returns an error.

- [ ] **Step 3: Add the dispatch arm**

In `crates/zoid/src/agent.rs`, in the `match kind` block (after the `Some(ToolKind::Interactive) if tc.name == "ask_user"` arm, ~line 810), add:

```rust
                Some(zoid_tools::ToolKind::Approving) if tc.name == "apply_mode_mapping" => {
                    let mapping = match zoid::mode_wizard::parse_mapping_args(&tc.args) {
                        Ok(m) => m,
                        Err(reason) => {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    output: format!("apply_mode_mapping: {reason}. Re-propose with valid args."),
                                    is_error: true,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            continue;
                        }
                    };
                    let summary = zoid::mode_wizard::approval_summary(&mapping);
                    let (rtx, rrx) = oneshot::channel::<String>();
                    let sent = ui
                        .send(AgentUpdate::ModeMappingApproval {
                            mapping,
                            summary,
                            reply: rtx,
                        })
                        .await;
                    if sent.is_err() {
                        continue;
                    }
                    let ans = rrx.await;
                    let output = match ans {
                        Ok(decision) => decision,
                        Err(_) => "approval cancelled".to_string(),
                    };
                    let is_error = output == "Reject" || output.starts_with("approval cancelled");
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output,
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                }
```

> **Implementer note:** the `emit(...)` signature and argument order must match the existing `ask_user` arm exactly (copy the surrounding call shape from lines ~848-862). If `emit` takes additional args in your tree, copy them from the adjacent `ask_user` arm.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid --test mode_wizard_loop`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/tests/mode_wizard_loop.rs
git commit -m "feat(agent): dispatch apply_mode_mapping (Approving) → ModeMappingApproval"
```

---

## Task 10: `:mode import` / `:mode update` commands (`zoid-tui/src/command.rs`)

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (add variants + parser)

**Interfaces:**
- Consumes: existing `Command` enum.
- Produces: `Command::ModeImport(String)`, `Command::ModeUpdate(String)`.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `crates/zoid-tui/src/command.rs`:

```rust
    #[test]
    fn mode_import_parses() {
        assert_eq!(
            parse_command(":mode import github.com/o/r/tree/main/skills"),
            Command::ModeImport("github.com/o/r/tree/main/skills".into())
        );
    }

    #[test]
    fn mode_update_parses() {
        assert_eq!(
            parse_command(":mode update Superpowers"),
            Command::ModeUpdate("Superpowers".into())
        );
    }

    #[test]
    fn bare_mode_import_is_empty_arg() {
        assert_eq!(parse_command(":mode import"), Command::ModeImport(String::new()));
    }

    #[test]
    fn mode_reload_still_wins_over_import() {
        // reload is checked before the fallthrough; import doesn't shadow it.
        assert_eq!(parse_command(":mode reload"), Command::ReloadModes);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui mode_import`
Expected: FAIL — compile error, `no variant ModeImport`.

- [ ] **Step 3: Add the variants + parser**

In `crates/zoid-tui/src/command.rs`, add to the `Command` enum (after `ReloadModes`):

```rust
    /// Start the URL import wizard (`:mode import <url>`). Empty = usage hint.
    ModeImport(String),
    /// Start the update wizard for an existing imported mode (`:mode update <name>`).
    ModeUpdate(String),
```

In `parse_command`, add before the `"mode reload"` arm (so `import`/`update`/`reload` are all checked before the `mode <name>` fallthrough):

```rust
        s if s.starts_with("mode import ") => {
            Command::ModeImport(s["mode import ".len()..].trim().to_string())
        }
        "mode import" => Command::ModeImport(String::new()),
        s if s.starts_with("mode update ") => {
            Command::ModeUpdate(s["mode update ".len()..].trim().to_string())
        }
        "mode update" => Command::ModeUpdate(String::new()),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui mode_`
Expected: PASS (new + existing mode tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/command.rs
git commit -m "feat(tui): :mode import / :mode update commands"
```

---

## Task 11: App wiring — `App.wizard` + `ModeImport`/`ModeUpdate` handlers (`zoid/src/main.rs`)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App` struct, construction, test `App` literal, command handlers.

**Interfaces:**
- Consumes: `ModeImportWizard`, `github_fetch`, `materialize`.
- Produces: `App.wizard: Option<ModeImportWizard>`; `ModeImport`/`ModeUpdate` handlers that fetch + open the wizard + spawn a turn.

- [ ] **Step 1: Add the `App` field**

In `crates/zoid/src/main.rs`, in the `struct App` definition (after the `skills` field, ~line 1043), add:

```rust
    /// The URL import/update wizard state. `Some` while a wizard is in flight;
    /// `None` otherwise. Gated into the turn's tool set in `spawn_turn`.
    wizard: Option<zoid::mode_wizard::ModeImportWizard>,
```

- [ ] **Step 2: Initialize the field in real construction**

In the real `App` construction (~line 1306, after the `mode_dirs:` field), add:

```rust
        wizard: None,
```

- [ ] **Step 3: Initialize the field in the test `App` literal**

In the test `App` construction (~line 3679, near `mode_dirs: Vec::new(),`), add:

```rust
            wizard: None,
```

- [ ] **Step 4: Add the `ModeImport` handler**

In `crates/zoid/src/main.rs`, in the `exec_command` function (after the `Command::ReloadModes` arm, ~line 2937), add:

```rust
        Command::ModeImport(url) => {
            if url.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode import <github-url>".into());
                return Ok(false);
            }
            // Cancel any in-flight wizard first.
            app.wizard = None;
            app.shell.status_hint = Some(format!("fetching {url}…"));
            // Parse the URL eagerly so a non-GitHub URL fails fast (no spawn).
            let parsed = match zoid::github_fetch::parse_github_url(&url) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(e);
                    return Ok(false);
                }
            };
            let ui_tx = app.ui_tx.clone();
            tokio::spawn(async move {
                let api = zoid::github_fetch::HttpGithubApi::new();
                let scan = match zoid::github_fetch::fetch_tree(&api, &parsed).await {
                    Ok(s) => s,
                    Err(e) => {
                        // Signal the failure via the sentinel so the main loop
                        // surfaces it as a status hint (not a silent drop).
                        let _ = ui_tx.send(zoid::agent::AgentUpdate::ModelsFetched {
                            provider: format!("__wizard_error__"),
                            models: vec![format!("fetch failed: {e}")],
                        }).await;
                        return;
                    }
                };
                // Stash the scan and spawn a turn. We can't move `app` into
                // the task, so we signal via the sentinel; the main loop
                // recognizes it, deserializes the scan, sets `app.wizard`,
                // pushes the seed user message, and calls `spawn_turn`.
                let _ = ui_tx.send(zoid::agent::AgentUpdate::ModelsFetched {
                    provider: format!("__wizard_scan__"),
                    models: vec![serde_json::to_string(&scan).unwrap_or_default()],
                }).await;
            });
            Ok(false)
        }
        Command::ModeUpdate(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode update <name>".into());
                return Ok(false);
            }
            // Full update wiring (read sidecar, re-fetch, classify, build
            // brief, open wizard) is in Task 15. This stub is replaced there.
            app.shell.status_hint = Some(format!("update wizard for '{name}' — see Task 15"));
            Ok(false)
        }
```

> **Implementer note:** the `tokio::spawn` in Step 4 can't move `app` in, so it signals via a `ModelsFetched`-shaped message with a `__wizard_scan__` sentinel provider id. The main loop's UI-handler (Task 12) recognizes the sentinel, deserializes the scan, sets `app.wizard`, pushes the seed user message, and calls `spawn_turn`. This is a pragmatic v1 bridge; a dedicated `AgentUpdate::WizardScanReady` variant is cleaner and should be substituted in Step 5 if the sentinel feels too cute — the test in Task 13 exercises the full path and will catch a wiring mistake.

- [ ] **Step 5: Add the receive path (wizard scan ready)**

In `crates/zoid/src/main.rs`, in the UI update handler (the `AgentUpdate::ModelsFetched` arm, ~line 1741), add a sentinel check at the top of the arm:

```rust
                    AgentUpdate::ModelsFetched { provider, models } => {
                        if provider == "__wizard_error__" {
                            // The import fetch failed; surface the error as a
                            // status hint and clear the "fetching…" hint.
                            if let Some(msg) = models.first() {
                                app.shell.status_hint = Some(msg.clone());
                            }
                            continue;
                        }
                        if provider == "__wizard_scan__" {
                            // The import fetch completed; deserialize the scan
                            // and open the wizard.
                            if let Some(json) = models.first() {
                                if let Ok(scan) =
                                    serde_json::from_str::<zoid_core::wizard::UpstreamScan>(json)
                                {
                                    let target = None; // import flow
                                    app.wizard = Some(zoid::mode_wizard::ModeImportWizard {
                                        scan,
                                        mode_name_target: target,
                                    });
                                    app.shell.status_hint = Some(
                                        "Import wizard started. Ask the model to propose a mapping.".into(),
                                    );
                                    // Push a seed user message and spawn the turn.
                                    let ts = now_ms();
                                    let seed_event = zoid_core::event::Event::new(
                                        ulid::Ulid::new(),
                                        None,
                                        ts,
                                        zoid_core::event::EventKind::UserMessage {
                                            text: "Import wizard started. Call propose_mode_mapping \
                                                to see the upstream scan, then call apply_mode_mapping \
                                                with your proposed mapping onto the canonical contract."
                                                .into(),
                                        },
                                    );
                                    app.session.append(seed_event.clone()).await.ok();
                                    app.events.push(seed_event);
                                    spawn_turn(app);
                                }
                            }
                            // Sentinel handled; skip the rest of this arm.
                            // (The UI loop is `while let Some(upd) = ui_rx.recv().await`;
                            // a bare `continue` here skips the rest of the match
                            // arm. If your tree's arm shape differs, adapt.)
                            continue;
                        }
                        // ... existing ModelsFetched handling continues here ...
```

> **Implementer note:** the `continue` inside the sentinel branch short-circuits the rest of the `ModelsFetched` arm in the UI loop (the `while let Some(upd) = ui_rx.recv().await` loop in `run()`, not `exec_command`). If your tree's arm shape differs, adapt the early-return to match — the integration test in Task 13 will catch a miss.

- [ ] **Step 6: Build to verify it compiles**

Run: `cargo build -p zoid`
Expected: compiles clean. (No new tests yet — the full path is exercised in Task 13.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): App.wizard + ModeImport/ModeUpdate handlers + scan-ready bridge"
```

---

## Task 12: `ModeMappingApproval` UI handler + `spawn_turn` tool gating (`zoid/src/main.rs`)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `spawn_turn` (tool gating), UI handler for `ModeMappingApproval`.

**Interfaces:**
- Consumes: `AgentUpdate::ModeMappingApproval`, `materialize`, `reload`, `slugify`.
- Produces: the approval overlay + on-Approve materialize path; wizard tools gated into `spawn_turn`.

- [ ] **Step 1: Gate wizard tools into `spawn_turn`**

In `crates/zoid/src/main.rs` `spawn_turn` (~line 3160), replace the `tools` build:

```rust
    let tools = std::sync::Arc::new(zoid::invoke_skill::chat_tools(std::sync::Arc::new(
        effective,
    )));
```

with:

```rust
    let mut tools = zoid::invoke_skill::chat_tools(std::sync::Arc::new(effective));
    if let Some(wiz) = &app.wizard {
        let wiz = std::sync::Arc::new(wiz.clone());
        tools.push(Box::new(zoid::mode_wizard::ProposeModeMappingTool::new(wiz.clone())));
        tools.push(Box::new(zoid::mode_wizard::ApplyModeMappingTool::new(wiz)));
    }
    let tools = std::sync::Arc::new(tools);
```

- [ ] **Step 2: Add the `ModeMappingApproval` UI handler**

In `crates/zoid/src/main.rs`, in the UI update handler (after the `AgentUpdate::AskUser` arm, ~line 1740), add:

```rust
                    AgentUpdate::ModeMappingApproval { mapping, summary, reply } => {
                        // Stash the mapping + reply channel; show the existing
                        // Question overlay with Approve/Reject/Adjust choices.
                        // On Approve: materialize + reload + clear wizard.
                        // On Reject: clear wizard.
                        // On Adjust (free-text): push the user's reply as a user
                        // message and re-spawn the turn (wizard stays).
                        app.pending_mode_mapping = Some((mapping, reply));
                        app.shell.question =
                            Some(zoid_tui::question::QuestionState::new(
                                summary,
                                vec!["Approve".into(), "Reject".into(), "Adjust".into()],
                            ));
                        app.shell.overlay = zoid_tui::state::Overlay::Question;
                    }
```

- [ ] **Step 3: Add the `pending_mode_mapping` field to `App`**

In `struct App` (after the `wizard` field), add:

```rust
    /// The pending mode mapping + reply channel while the approval overlay is
    /// up. `Some` from `ModeMappingApproval` until the user answers.
    pending_mode_mapping:
        Option<(zoid_core::wizard::ModeMapping, tokio::sync::oneshot::Sender<String>)>,
```

Initialize it to `None` in both the real and test `App` literals.

- [ ] **Step 4: Route the approval answer**

In `crates/zoid/src/main.rs`, find `answer_question` (~line 2859). Add a branch for the mode-mapping approval before the generic `ask_user` path:

```rust
fn answer_question(app: &mut App, ans: zoid::agent::Answer) {
    // Mode-mapping approval path: handle here, don't fall through to ask_user.
    if let Some((mapping, tx)) = app.pending_mode_mapping.take() {
        match &ans {
            zoid::agent::Answer::Choice(c) if c == "Approve" => {
                // Materialize synchronously (best-effort; v1 blocks the UI
                // briefly — the materialize is a few small writes).
                let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
                let dest = cfg_dir.join("modes").join(zoid::mode_wizard::slugify(&mapping.mode_name));
                let scan = app
                    .wizard
                    .as_ref()
                    .expect("wizard open during approval")
                    .scan
                    .clone();
                let fetched_at = chrono::Utc::now().to_rfc3339();
                match zoid::mode_wizard::materialize(&mapping, &scan, &dest, &fetched_at) {
                    Ok(_) => {
                        let prev = app.modes.active_name().to_string();
                        app.modes = zoid::mode_import::build_mode_registry(
                            &app.base_profile,
                            &app.mode_dirs,
                        );
                        app.modes.set_active(&prev);
                        sync_mode_mirror(app);
                        app.wizard = None;
                        app.shell.status_hint = Some(format!(
                            "imported '{}' — Shift+Tab to it",
                            mapping.mode_name
                        ));
                        let _ = tx.send("Approve".to_string());
                    }
                    Err(e) => {
                        // Re-raise: retry or cancel.
                        app.shell.status_hint = Some(format!(
                            "materialize failed: {}. Retry / Cancel?",
                            e.problems.join("; ")
                        ));
                        // Put the mapping back so a Retry can re-attempt; for v1
                        // we drop the wizard and let the user re-run :mode import.
                        app.wizard = None;
                        let _ = tx.send("Reject".to_string());
                    }
                }
            }
            zoid::agent::Answer::Choice(c) if c == "Reject" => {
                app.wizard = None;
                app.shell.status_hint = Some("import cancelled".into());
                let _ = tx.send("Reject".to_string());
            }
            zoid::agent::Answer::Choice(_) | zoid::agent::Answer::FreeText(_) => {
                // "Adjust" or free-text: push the reply as a user message,
                // keep the wizard, re-spawn the turn so the model re-proposes.
                let text = match ans {
                    zoid::agent::Answer::Choice(s) | zoid::agent::Answer::FreeText(s) => s,
                    zoid::agent::Answer::LetYouDecide => "[let you decide]".into(),
                };
                let ts = now_ms();
                let ev = zoid_core::event::Event::new(
                    ulid::Ulid::new(),
                    None,
                    ts,
                    zoid_core::event::EventKind::UserMessage { text },
                );
                // We can't `.await` here (answer_question is sync); push the
                // event to the log synchronously and let the next frame's
                // spawn handle it. For v1, set a flag the main loop checks.
                app.pending_adjust = Some(ev);
                let _ = tx.send("Adjust".to_string());
            }
            zoid::agent::Answer::LetYouDecide => {
                let _ = tx.send("Approve".to_string());
            }
        }
        app.shell.question = None;
        app.shell.overlay = zoid_tui::state::Overlay::None;
        return;
    }
    // ... existing ask_user path unchanged ...
    if let Some(tx) = app.pending_answer.take() {
        let _ = tx.send(ans);
    }
    app.shell.question = None;
    app.shell.overlay = zoid_tui::state::Overlay::None;
}
```

Add the `pending_adjust: Option<Event>` field to `App` (init `None`), and in the main loop's `run()` — at the top of the `loop {}`, before the `tokio::select!` (`main.rs:1643`), add:

```rust
        // Flush a deferred adjust reply from the wizard approval overlay
        // (answer_question is sync and can't `.await` session.append).
        if let Some(ev) = app.pending_adjust.take() {
            app.session.append(ev.clone()).await.ok();
            app.events.push(ev);
            spawn_turn(app);
        }
```

> **Implementer note:** `answer_question` is sync but `session.append` is async. The `pending_adjust` flag + main-loop flush is the v1 bridge (mirrors how other sync handlers defer async work). If your tree has an async answer path, inline the `.await` there instead.

- [ ] **Step 5: Build to verify it compiles**

Run: `cargo build -p zoid`
Expected: compiles clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): ModeMappingApproval UI handler + spawn_turn wizard tool gating"
```

---

## Task 13: Integration test — import wiring (`zoid/tests/mode_import_wiring.rs`)

**Files:**
- Create: `crates/zoid/tests/mode_import_wiring.rs`

- [ ] **Step 1: Write the test**

Create `crates/zoid/tests/mode_import_wiring.rs`:

```rust
//! Integration: a scripted provider calls propose_mode_mapping then
//! apply_mode_mapping; the loop raises ModeMappingApproval; the test answers
//! "Approve"; the materializer writes canonical files to a temp user-global
//! dir; the mode loads as Ready. No real fetch (scan injected via the wizard
//! state); no real model (scripted tool calls).

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid::mode_wizard::{
    approval_summary, ApplyModeMappingTool, ModeImportWizard, ProposeModeMappingTool,
};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::wizard::{MappingEntry, ModeMapping, ScannedFile, UpstreamScan};
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        let script = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

fn scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "abc".into(),
        subtree_path: "skills".into(),
        files: vec![
            ScannedFile {
                upstream_path: "skills/using/SKILL.md".into(),
                sha: "sha-u".into(),
                content: "---\nname: using\ndescription: d\n---\nLOADER\n".into(),
            },
            ScannedFile {
                upstream_path: "skills/brain/SKILL.md".into(),
                sha: "sha-b".into(),
                content: "---\nname: brain\ndescription: d\n---\nBODY\n".into(),
            },
        ],
    }
}

#[tokio::test]
async fn import_wizard_approve_materializes_and_loads() {
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call(
                    "apply_mode_mapping",
                    serde_json::json!({
                        "mode_name": "TestMode",
                        "mode_description": "test",
                        "mode_body": "LOADER",
                        "entries": [
                            { "Materialize": { "canonical_path": "mode.md", "source": "skills/using/SKILL.md", "summary": "loader" } },
                            { "Materialize": { "canonical_path": "brain/SKILL.md", "source": "skills/brain/SKILL.md", "summary": "brain" } }
                        ]
                    }),
                ),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("ok"), ProviderEvent::Done],
        ])),
    });

    let wiz = Arc::new(ModeImportWizard::new_import(scan()));
    let mut tools = zoid::invoke_skill::chat_tools(Arc::new(
        zoid_core::skill::SkillRegistry::builtin(),
    ));
    tools.push(Box::new(ProposeModeMappingTool::new(wiz.clone())));
    tools.push(Box::new(ApplyModeMappingTool::new(wiz.clone())));
    let tools = Arc::new(tools);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "import".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    // Drain UI updates; on ModeMappingApproval, run the materializer against a
    // temp dir (simulating the bin's approval handler) and answer Approve.
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("testmode");
    let dest_for_task = dest.clone();
    let scan_for_task = wiz.scan.clone();
    let handle = tokio::spawn(async move {
        while let Some(upd) = rx.recv().await {
            if let AgentUpdate::ModeMappingApproval { mapping, reply, .. } = upd {
                let res = zoid::mode_wizard::materialize(
                    &mapping,
                    &scan_for_task,
                    &dest_for_task,
                    "2026-07-05T12:00:00Z",
                );
                let _ = reply.send(if res.is_ok() { "Approve".into() } else { "Reject".into() });
            }
        }
    });

    run_agent_turn(
        zoid::agent::chat_turn_config_with(&zoid::agent::default_profile(), ""),
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    handle.await.unwrap();

    // The materializer wrote the canonical files + sidecar.
    assert!(dest.join("mode.md").is_file());
    assert!(dest.join("brain/SKILL.md").is_file());
    assert!(dest.join(".zoid-provenance.json").is_file());

    // The mode loads as Ready via mode_import.
    let reg = zoid::mode_import::build_mode_registry(
        &zoid::agent::default_profile(),
        &[tmp.path().to_path_buf()],
    );
    let m = reg
        .modes()
        .iter()
        .find(|m| m.name() == "TestMode")
        .expect("TestMode loaded");
    assert!(!m.is_broken());
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p zoid --test mode_import_wiring`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/mode_import_wiring.rs
git commit -m "test(zoid): import wizard approve → materialize → mode loads (integration)"
```

---

## Task 14: Integration test — update wiring (`zoid/tests/mode_update_wiring.rs`)

**Files:**
- Create: `crates/zoid/tests/mode_update_wiring.rs`

- [ ] **Step 1: Write the test**

Create `crates/zoid/tests/mode_update_wiring.rs`:

```rust
//! Integration: a pre-seeded mode + sidecar; a fresh scan that moves one file,
//! adds one, drops one; a scripted merged mapping; Approve; assert the on-disk
//! file set reconciled (add/update/drop) and the sidecar refreshed. This
//! exercises the materializer's file-set reconciliation directly (the update
//! fetch path is wired in a later task; here we test the reconcile + write).

use std::path::Path;

use zoid_core::wizard::{
    MappingEntry, ModeMapping, ProvenanceEntry, ProvenanceFile, ProvenanceSource, ScannedFile,
    UpstreamScan,
};
use zoid::mode_wizard::{materialize, slugify};

fn file(path: &str, sha: &str, content: &str) -> ScannedFile {
    ScannedFile {
        upstream_path: path.into(),
        sha: sha.into(),
        content: content.into(),
    }
}

fn import_scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "ref1".into(),
        subtree_path: "skills".into(),
        files: vec![
            file("skills/a/SKILL.md", "sha-a-v1", "---\nname: a\ndescription: d\n---\nA-v1\n"),
            file("skills/b/SKILL.md", "sha-b-v1", "---\nname: b\ndescription: d\n---\nB-v1\n"),
            file("skills/c/SKILL.md", "sha-c-v1", "---\nname: c\ndescription: d\n---\nC-v1\n"),
        ],
    }
}

fn fresh_scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "ref1".into(),
        subtree_path: "skills".into(),
        files: vec![
            // a: content changed (sha differs)
            file("skills/a/SKILL.md", "sha-a-v2", "---\nname: a\ndescription: d\n---\nA-v2\n"),
            // b: unchanged (same sha)
            file("skills/b/SKILL.md", "sha-b-v1", "---\nname: b\ndescription: d\n---\nB-v1\n"),
            // c: deleted upstream (not in fresh scan)
            // d: new upstream
            file("skills/d/SKILL.md", "sha-d-v1", "---\nname: d\ndescription: d\n---\nD-v1\n"),
        ],
    }
}

#[test]
fn update_file_set_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join(slugify("TestMode"));

    // 1) Initial import: materialize {a, b, c}.
    let import_mapping = ModeMapping {
        mode_name: "TestMode".into(),
        mode_description: "d".into(),
        mode_body: "".into(),
        entries: vec![
            MappingEntry::Materialize {
                canonical_path: "a/SKILL.md".into(),
                source: "skills/a/SKILL.md".into(),
                summary: "a".into(),
            },
            MappingEntry::Materialize {
                canonical_path: "b/SKILL.md".into(),
                source: "skills/b/SKILL.md".into(),
                summary: "b".into(),
            },
            MappingEntry::Materialize {
                canonical_path: "c/SKILL.md".into(),
                source: "skills/c/SKILL.md".into(),
                summary: "c".into(),
            },
        ],
    };
    materialize(&import_mapping, &import_scan(), &dest, "t1").unwrap();
    assert!(dest.join("a/SKILL.md").is_file());
    assert!(dest.join("c/SKILL.md").is_file());

    // 2) Update: merged mapping materializes {a (new content), b (unchanged), d (new)};
    //    c is dropped (not in the new mapping). The materializer must DELETE c.
    let merged_mapping = ModeMapping {
        mode_name: "TestMode".into(),
        mode_description: "d".into(),
        mode_body: "".into(),
        entries: vec![
            MappingEntry::Materialize {
                canonical_path: "a/SKILL.md".into(),
                source: "skills/a/SKILL.md".into(),
                summary: "a updated".into(),
            },
            MappingEntry::Materialize {
                canonical_path: "b/SKILL.md".into(),
                source: "skills/b/SKILL.md".into(),
                summary: "b".into(),
            },
            MappingEntry::Materialize {
                canonical_path: "d/SKILL.md".into(),
                source: "skills/d/SKILL.md".into(),
                summary: "d new".into(),
            },
        ],
    };
    materialize(&merged_mapping, &fresh_scan(), &dest, "t2").unwrap();

    // a: overwritten with v2 content.
    assert_eq!(
        std::fs::read_to_string(dest.join("a/SKILL.md")).unwrap(),
        "---\nname: a\ndescription: d\n---\nA-v2\n"
    );
    // b: present (rewritten, same content).
    assert!(dest.join("b/SKILL.md").is_file());
    // c: DELETED (dropped from the mapping).
    assert!(!dest.join("c/SKILL.md").exists(), "dropped file must be deleted");
    // d: created.
    assert!(dest.join("d/SKILL.md").is_file());

    // Sidecar reflects the new file set {a, b, d}.
    let side = std::fs::read_to_string(dest.join(".zoid-provenance.json")).unwrap();
    let pf: ProvenanceFile = serde_json::from_str(&side).unwrap();
    let paths: Vec<&str> = pf.files.iter().map(|f| f.canonical_path.as_str()).collect();
    assert!(paths.contains(&"a/SKILL.md"));
    assert!(paths.contains(&"b/SKILL.md"));
    assert!(paths.contains(&"d/SKILL.md"));
    assert!(!paths.contains(&"c/SKILL.md"));
}
```

> **Implementer note:** this test exercises `materialize`'s *file-set reconciliation* directly. The current `materialize` (Task 6) writes/overwrites files but does NOT delete dropped files. Before this test passes, you must extend `materialize` to take the old sidecar's file list (or read it from disk) and delete any canonical path not in the new mapping. Add that to `materialize` in Task 6's file (`mode_wizard.rs`): after writing all `Materialize` entries, list the old sidecar's `canonical_path`s (read `.zoid-provenance.json` if it exists), compute the set difference, and `std::fs::remove_file` each dropped path (best-effort; log to stderr on failure). Re-run the test.

- [ ] **Step 2: Extend `materialize` to delete dropped files**

In `crates/zoid/src/mode_wizard.rs`, in `materialize`, after writing all `Materialize` entries and before writing the new sidecar, add:

```rust
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
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p zoid --test mode_update_wiring`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs crates/zoid/tests/mode_update_wiring.rs
git commit -m "feat(zoid): materialize file-set reconciliation (delete dropped) + update wiring test"
```

---

## Task 15: Update wiring — `ModeUpdate` handler + reconciliation brief (`zoid/src/main.rs` + `mode_wizard.rs`)

**Files:**
- Modify: `crates/zoid/src/mode_wizard.rs` — `ModeImportWizard.reconciliation_brief: Option<String>` + `ProposeModeMappingTool` returns it when present.
- Modify: `crates/zoid/src/main.rs` — replace the `ModeUpdate` stub (Task 11) with the full handler: read sidecar, re-fetch at original ref, classify, build brief, open wizard.

**Interfaces:**
- Consumes: `ProvenanceFile`, `classify_update`, `fetch_tree`, `parse_github_url`.
- Produces: a working `:mode update <name>` that opens the wizard with a reconciliation brief.

- [ ] **Step 1: Add `reconciliation_brief` to `ModeImportWizard`**

In `crates/zoid/src/mode_wizard.rs`, add a field to `ModeImportWizard`:

```rust
#[derive(Debug, Clone)]
pub struct ModeImportWizard {
    pub scan: UpstreamScan,
    pub mode_name_target: Option<String>,
    /// Pre-computed reconciliation brief for the update flow. `None` for
    /// import. When `Some`, `ProposeModeMappingTool` returns this instead of
    /// the raw scan.
    pub reconciliation_brief: Option<String>,
}
```

Update `new_import` to set `reconciliation_brief: None`. Add `new_update`:

```rust
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
```

- [ ] **Step 2: Make `ProposeModeMappingTool` return the brief when present**

In `ProposeModeMappingTool::run`, replace the body:

```rust
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        if let Some(brief) = &self.wizard.reconciliation_brief {
            ToolOutput::ok(brief.clone())
        } else {
            ToolOutput::ok(render_scan(&self.wizard.scan))
        }
    }
```

- [ ] **Step 3: Add `build_reconciliation_brief` helper**

In `crates/zoid/src/mode_wizard.rs`, add:

```rust
/// Build the human-readable reconciliation brief for the update flow. Reads
/// the old sidecar + the on-disk canonical files, classifies each against the
/// fresh scan, and returns a text block the model reads via
/// `propose_mode_mapping`. Effectful (FS reads) — called by the bin at
/// wizard-open time, not by the tool.
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
        let local = std::fs::read_to_string(mode_dir.join(&entry.canonical_path))
            .unwrap_or_default();
        let class = zoid_core::wizard::classify_update(entry, &local, fresh_scan);
        s.push_str(&format!(
            "- {} (upstream {}): {}\n",
            entry.canonical_path, entry.upstream_path, class
        ));
    }
    // Also list fresh-scan files not in the old sidecar (new upstream).
    let old_paths: std::collections::HashSet<&str> =
        old_sidecar.files.iter().map(|f| f.upstream_path.as_str()).collect();
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
```

- [ ] **Step 4: Write the failing test for the brief**

Append to `crates/zoid/src/mode_wizard.rs` tests:

```rust
    #[test]
    fn reconciliation_brief_lists_classifications() {
        let tmp = tempfile::tempdir().unwrap();
        let mode_dir = tmp.path().join("m");
        std::fs::create_dir_all(&mode_dir).unwrap();
        // Write a local canonical file (unchanged case).
        std::fs::write(
            mode_dir.join("a/SKILL.md"),
            "---\nname: a\ndescription: d\n---\nA-v1\n",
        )
        .unwrap();
        let old = ProvenanceFile {
            schema: 1,
            source: ProvenanceSource {
                url: "u".into(),
                repo: "o/r".into(),
                ref_: "ref1".into(),
                subtree_path: "skills".into(),
                fetched_at: "t".into(),
            },
            mode_name: "M".into(),
            files: vec![ProvenanceEntry {
                canonical_path: "a/SKILL.md".into(),
                upstream_path: "skills/a/SKILL.md".into(),
                upstream_sha: "sha-a-v1".into(),
                upstream_ref: "ref1".into(),
                upstream_snapshot: "---\nname: a\ndescription: d\n---\nA-v1\n".into(),
            }],
        };
        let fresh = UpstreamScan {
            url: "u".into(),
            repo: "o/r".into(),
            resolved_ref: "ref1".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/a/SKILL.md".into(),
                    sha: "sha-a-v1".into(),
                    content: "---\nname: a\ndescription: d\n---\nA-v1\n".into(),
                },
                ScannedFile {
                    upstream_path: "skills/new/SKILL.md".into(),
                    sha: "sha-new".into(),
                    content: "NEW".into(),
                },
            ],
        };
        let brief = build_reconciliation_brief(&mode_dir, &old, &fresh);
        assert!(brief.contains("unchanged"));
        assert!(brief.contains("new upstream"));
        assert!(brief.contains("skills/new/SKILL.md"));
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid --lib reconciliation_brief`
Expected: PASS.

- [ ] **Step 6: Replace the `ModeUpdate` stub with the full handler**

In `crates/zoid/src/main.rs` `exec_command`, replace the `Command::ModeUpdate(name)` arm (the Task 11 stub) with:

```rust
        Command::ModeUpdate(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode update <name>".into());
                return Ok(false);
            }
            // Find the mode's folder under the user-global modes dir.
            let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
            let slug = zoid::mode_wizard::slugify(&name);
            let mode_dir = cfg_dir.join("modes").join(&slug);
            let sidecar_path = mode_dir.join(".zoid-provenance.json");
            if !sidecar_path.is_file() {
                app.shell.status_hint = Some(format!(
                    "mode '{name}' has no import provenance; it was not imported from a URL. Use :mode import <url> instead."
                ));
                return Ok(false);
            }
            let sidecar_text = match std::fs::read_to_string(&sidecar_path) {
                Ok(t) => t,
                Err(e) => {
                    app.shell.status_hint = Some(format!("read sidecar: {e}"));
                    return Ok(false);
                }
            };
            let old: zoid_core::wizard::ProvenanceFile = match serde_json::from_str(&sidecar_text) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(format!("parse sidecar: {e}"));
                    return Ok(false);
                }
            };
            // Re-fetch at the ORIGINAL ref (stable, not latest).
            let url = format!(
                "https://github.com/{}/tree/{}/{}",
                old.source.repo, old.source.ref_, old.source.subtree_path
            );
            let parsed = match zoid::github_fetch::parse_github_url(&url) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(e);
                    return Ok(false);
                }
            };
            app.shell.status_hint = Some(format!("fetching upstream at ref {}…", old.source.ref_));
            let ui_tx = app.ui_tx.clone();
            let mode_dir_clone = mode_dir.clone();
            let old_clone = old.clone();
            tokio::spawn(async move {
                let api = zoid::github_fetch::HttpGithubApi::new();
                let scan = match zoid::github_fetch::fetch_tree(&api, &parsed).await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ui_tx.send(zoid::agent::AgentUpdate::ModelsFetched {
                            provider: format!("__wizard_error__"),
                            models: vec![format!("fetch failed: {e}")],
                        }).await;
                        return;
                    }
                };
                let brief = zoid::mode_wizard::build_reconciliation_brief(
                    &mode_dir_clone,
                    &old_clone,
                    &scan,
                );
                // Stash scan + brief via a second sentinel shape: we re-use
                // __wizard_scan__ but pack the scan JSON + brief together
                // (brief in models[1], target name in models[2]).
                let _ = ui_tx.send(zoid::agent::AgentUpdate::ModelsFetched {
                    provider: format!("__wizard_update__"),
                    models: vec![
                        serde_json::to_string(&scan).unwrap_or_default(),
                        brief,
                        old_clone.mode_name.clone(),
                    ],
                }).await;
            });
            Ok(false)
        }
```

- [ ] **Step 7: Handle the `__wizard_update__` sentinel in the UI loop**

In the `AgentUpdate::ModelsFetched` arm (after the `__wizard_error__` and `__wizard_scan__` branches from Task 11), add:

```rust
                        if provider == "__wizard_update__" {
                            // The update fetch + brief build completed.
                            let mut iter = models.into_iter();
                            let scan_json = iter.next().unwrap_or_default();
                            let brief = iter.next().unwrap_or_default();
                            let target = iter.next().unwrap_or_default();
                            if let Ok(scan) =
                                serde_json::from_str::<zoid_core::wizard::UpstreamScan>(&scan_json)
                            {
                                app.wizard = Some(zoid::mode_wizard::ModeImportWizard::new_update(
                                    scan,
                                    target,
                                    brief,
                                ));
                                app.shell.status_hint = Some(
                                    "Update wizard started. Ask the model to propose a merged mapping.".into(),
                                );
                                let ts = now_ms();
                                let seed_event = zoid_core::event::Event::new(
                                    ulid::Ulid::new(),
                                    None,
                                    ts,
                                    zoid_core::event::EventKind::UserMessage {
                                        text: "Update wizard started. Call propose_mode_mapping \
                                            to see the reconciliation brief, then call \
                                            apply_mode_mapping with your merged mapping."
                                            .into(),
                                    },
                                );
                                app.session.append(seed_event.clone()).await.ok();
                                app.events.push(seed_event);
                                spawn_turn(app);
                            }
                            continue;
                        }
```

- [ ] **Step 8: Build and run the mode_wizard tests**

Run: `cargo test -p zoid --lib mode_wizard`
Expected: PASS.

Run: `cargo build -p zoid`
Expected: compiles clean.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/mode_wizard.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): :mode update — read sidecar, re-fetch, build reconciliation brief, open wizard"
```

---

## Task 16: Real-model go/no-go smoke runbook (`docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md`)

> This runbook was an internal-process artifact tied to the maintainer's spike-testing workflow; it was removed prior to open-sourcing this repo and is no longer present at the path below.

**Files:**
- Create: `docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md`

- [ ] **Step 1: Write the runbook**

Create `docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md`:

```markdown
# URL Import Wizard — Go/No-Go Smoke

**Purpose:** Answer the one non-unit-testable question: will the active model
produce a *valid, useful* mapping of a real GitHub skill tree onto the canonical
contract, and can the update flow reconcile upstream changes with local edits?

## Preconditions

- `$GITHUB_TOKEN` set (higher rate limit; optional for public repos).
- Built from the branch carrying Tasks 1-14. `cargo test --workspace` green.
- A scratch repo (the wizard writes to `~/.config/zoid/modes/`, user-global).

## Import smoke

1. Launch zoid in a scratch dir.
2. Run: `:mode import github.com/obra/superpowers/tree/main/skills`
3. When the model calls `propose_mode_mapping` and then `apply_mode_mapping`,
   review the proposal in the conversation + the AskUser overlay.
4. Approve (or Adjust if the mode/skill split is wrong).

### Outcome rubric (import)

- **PASS** — the model proposes `Superpowers` as the mode name, the ~13
  methodology skills as scoped skills, `using-superpowers` as the `mode.md`
  body, and skips the genuinely-irrelevant files (README, license,
  tests-for-upstream). On Approve, `~/.config/zoid/modes/superpowers/`
  materializes, `:mode` shows `Superpowers`, switching to it loads the skills,
  and `invoke_skill("brainstorming")` returns its body.
- **PARTIAL** — proposes a mapping but gets the mode/skill split wrong (e.g.
  `using-superpowers` as a skill instead of the mode body), or skips too much,
  or generates bad frontmatter the materializer rejects more than once.
- **FAIL** — never calls `propose_mode_mapping`, or proposes an empty/trivial
  mapping, or loops without converging.

## Update smoke

1. After a successful import, hand-edit one local skill body (e.g. add a comment
   to `brainstorming/SKILL.md`).
2. Simulate upstream changing two files: edit `~/.config/zoid/modes/superpowers/.zoid-provenance.json`
   to bump two `upstream_sha` values to fake "moved" SHAs, and add a new file
   entry to simulate "upstream added". (Or, if a real upstream ref moved, point
   the sidecar's `ref` at the new ref.)
3. Run: `:mode update Superpowers`
4. Review the model's merged mapping; Approve.

### Outcome rubric (update)

- **PASS** — the model's merged mapping carries the local edit, re-materializes
  the upstream-only-changed file, flags the both-changed one with its pick, and
  the on-disk result matches the approved mapping.
- **PARTIAL** — reconciles structure but drops or clobbers the local edit
  against the model's stated intent.
- **FAIL** — can't produce a coherent merged proposal.

## Decision gate

- **Import PASS + update PASS** → the wizard ships; the on-ramp is real.
- **Import PARTIAL** → prompt-engineering on the `propose_mode_mapping` tool
  description / the seed user message before shipping.
- **Import FAIL** → fall back to deterministic mapping (model-only-for-
  descriptions) with the provenance sidecar still shipping.
- **Update FAIL specifically** → ship import-only this slice, defer update.

## Recorded outcome

- Date run:
- Model / build commit:
- Import verdict (PASS / PARTIAL / FAIL):
- Update verdict (PASS / PARTIAL / FAIL):
- Observed mapping / reconciliation:
- Notes / next action:
```

- [ ] **Step 2: Commit the runbook**

```bash
git add docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md
git commit -m "docs(runbook): URL import wizard go/no-go smoke protocol"
```

- [ ] **Step 3: Run the smoke and record the outcome**

Follow the runbook against the active model. Fill in the "Recorded outcome" section, then commit:

```bash
git add docs/superpowers/runbooks/2026-07-05-url-import-wizard-smoke.md
git commit -m "docs(runbook): record URL import wizard go/no-go outcome"
```

This recorded verdict is the exit criterion for the slice.

---

## Final verification (whole slice)

- [ ] `cargo test --workspace` — all green.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — clean.
- [ ] `cargo fmt --all --check` — clean.
- [ ] The go/no-go verdict is recorded in the runbook.