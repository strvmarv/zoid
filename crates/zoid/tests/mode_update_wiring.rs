//! Integration: a pre-seeded mode + sidecar; a fresh scan that moves one file,
//! adds one, drops one; a scripted merged mapping; Approve; assert the on-disk
//! file set reconciled (add/update/drop) and the sidecar refreshed. This
//! exercises the materializer's file-set reconciliation directly.

use zoid::mode_wizard::{materialize, slugify};
use zoid_core::wizard::{MappingEntry, ModeMapping, ProvenanceFile, ScannedFile, UpstreamScan};

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
            file(
                "skills/a/SKILL.md",
                "sha-a-v1",
                "---\nname: a\ndescription: d\n---\nA-v1\n",
            ),
            file(
                "skills/b/SKILL.md",
                "sha-b-v1",
                "---\nname: b\ndescription: d\n---\nB-v1\n",
            ),
            file(
                "skills/c/SKILL.md",
                "sha-c-v1",
                "---\nname: c\ndescription: d\n---\nC-v1\n",
            ),
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
            file(
                "skills/a/SKILL.md",
                "sha-a-v2",
                "---\nname: a\ndescription: d\n---\nA-v2\n",
            ),
            file(
                "skills/b/SKILL.md",
                "sha-b-v1",
                "---\nname: b\ndescription: d\n---\nB-v1\n",
            ),
            file(
                "skills/d/SKILL.md",
                "sha-d-v1",
                "---\nname: d\ndescription: d\n---\nD-v1\n",
            ),
        ],
    }
}

#[test]
fn update_file_set_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join(slugify("TestMode"));

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

    assert_eq!(
        std::fs::read_to_string(dest.join("a/SKILL.md")).unwrap(),
        "---\nname: a\ndescription: d\n---\nA-v2\n"
    );
    assert!(dest.join("b/SKILL.md").is_file());
    assert!(
        !dest.join("c/SKILL.md").exists(),
        "dropped file must be deleted"
    );
    assert!(dest.join("d/SKILL.md").is_file());

    let side = std::fs::read_to_string(dest.join(".zoid-provenance.json")).unwrap();
    let pf: ProvenanceFile = serde_json::from_str(&side).unwrap();
    let paths: Vec<&str> = pf.files.iter().map(|f| f.canonical_path.as_str()).collect();
    assert!(paths.contains(&"a/SKILL.md"));
    assert!(paths.contains(&"b/SKILL.md"));
    assert!(paths.contains(&"d/SKILL.md"));
    assert!(!paths.contains(&"c/SKILL.md"));
}
