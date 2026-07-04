# SKILL.md Source Adapter / Importer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate zoid's `SkillRegistry` from `SKILL.md` files on disk so `invoke_skill` can pull real skill bodies, with each imported skill's bundled sibling files reachable from the returned body.

**Architecture:** Honor the existing pure-core / effectful-bin seam. A pure `parse_skill_md(&str) -> Result<ParsedSkill, String>` and the `Skill` type live in `zoid-core` (no filesystem, no new dependency). The filesystem walker (`import_skills`) and the registry builder (`build_registry`) live in a new bin module `crates/zoid/src/skill_import.rs`. `invoke_skill` appends a resolved `base_dir` anchor line so the model reads siblings via the existing `read_file` tool. Wired into `App` construction at `main.rs`.

**Tech Stack:** Rust 2021 workspace (`zoid-core` pure domain; `zoid` bin = composition root). `serde`/`toml` for config (already present). No new external dependency. `tempfile` (already in the tree) for bin fixtures.

## Global Constraints

- **Pure-core / effectful-bin seam:** `zoid-core` takes NO filesystem/process/`git2`/provider deps. The parser is pure; all filesystem IO lives in the `zoid` bin. (Spec §Architecture.)
- **No new external dependency:** the frontmatter parser is hand-rolled; do not add a YAML crate. (Spec decision 2.)
- **Import-only scope:** SKILL.md → `Skill` into `SkillRegistry`. Do NOT build promotion, `AgentProfile`s, `[[mode]]`, or Shift+Tab. The `AgentProfileRegistry` stays at `[default]`. (Spec decision 5, §Out of scope.)
- **Collision policy = first-wins:** built-ins and earlier-scanned dirs are protected; a later duplicate name is skipped. (Spec §Error handling.)
- **Never abort startup:** a missing dir is skipped silently; a present-but-unreadable dir, an unreadable file, or a malformed `SKILL.md` is skipped with a warning to stderr. `parse_skill_md` returns `Result`, never panics. (Spec §Error handling.)
- **Keep the two `spike-*` built-in skills** in the registry alongside imports.
- **Do NOT touch files owned by the parallel ACM session:** `crates/zoid-core/src/{compaction,context,economy,event,projection}.rs` and `crates/zoid/src/agent.rs`. This plan does not need them.
- **Commit messages:** no `Co-Authored-By` / co-author trailer (repo CLAUDE.md).
- **Anchor line format (exact):** `"\n\n---\nSkill files are in: {abs_dir}/"` appended to the body, where `{abs_dir}` is the skill's absolute source directory.

---

### Task 1: `Skill.base_dir` field + `SkillRegistry::push_unique`

**Files:**
- Modify: `crates/zoid-core/src/skill.rs`
- Test: `crates/zoid-core/src/skill.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (extends existing `Skill` / `SkillRegistry`).
- Produces:
  - `Skill.base_dir: Option<std::path::PathBuf>` — `None` for built-ins.
  - `SkillRegistry::push_unique(&mut self, skill: Skill) -> bool` — appends unless the name already exists (first-wins); returns whether it appended.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-core/src/skill.rs`:

```rust
    #[test]
    fn builtin_skills_have_no_base_dir() {
        let r = SkillRegistry::builtin();
        assert!(r.get("spike-plan").unwrap().base_dir.is_none());
        assert!(r.get("spike-implement").unwrap().base_dir.is_none());
    }

    #[test]
    fn push_unique_appends_new_and_rejects_duplicate() {
        let mk = |n: &str| Skill {
            name: n.into(),
            description: "d".into(),
            body: "b".into(),
            base_dir: None,
        };
        let mut r = SkillRegistry::new(vec![]);
        assert!(r.push_unique(mk("a")));
        assert!(!r.push_unique(mk("a"))); // duplicate name rejected, no change
        assert!(r.push_unique(mk("b")));
        assert_eq!(r.names(), vec!["a".to_string(), "b".to_string()]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib skill 2>&1 | tail -20`
Expected: FAIL — compile error (`Skill` has no field `base_dir`; no method `push_unique`).

- [ ] **Step 3: Add the `base_dir` field**

In `crates/zoid-core/src/skill.rs`, extend the struct:

```rust
/// A single named skill: its one-line menu description, its full body, and the
/// source directory it was imported from (for bundled sibling files). Built-in
/// skills have `base_dir: None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub base_dir: Option<std::path::PathBuf>,
}
```

Add `base_dir: None,` to BOTH `Skill { … }` literals inside `builtin()` (the `spike-plan` and `spike-implement` skills).

- [ ] **Step 4: Add `push_unique`**

Add to `impl SkillRegistry` (after `new`):

```rust
    /// Append `skill` unless a skill with the same name already exists. Returns
    /// `true` if appended, `false` (and leaves the registry unchanged) on a name
    /// collision — first-wins, so built-ins and earlier imports are protected.
    pub fn push_unique(&mut self, skill: Skill) -> bool {
        if self.skills.iter().any(|s| s.name == skill.name) {
            return false;
        }
        self.skills.push(skill);
        true
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib skill 2>&1 | tail -20`
Expected: PASS (all `skill` tests, including the two new ones).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/skill.rs
git commit -m "feat(core): Skill.base_dir + SkillRegistry::push_unique (first-wins)"
```

---

### Task 2: `parse_skill_md` — the pure frontmatter parser

**Files:**
- Modify: `crates/zoid-core/src/skill.rs`
- Test: `crates/zoid-core/src/skill.rs` (inline `tests`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `ParsedSkill { name: String, description: String, body: String }`
  - `parse_skill_md(text: &str) -> Result<ParsedSkill, String>` — pure; `Err` (human-readable reason) if there is no frontmatter block or `name` is missing/empty.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-core/src/skill.rs`:

```rust
    #[test]
    fn parses_name_description_and_body() {
        let md = "---\nname: brainstorming\n\
                  description: \"Explore: intent, and design\"\n\
                  ---\n# Body\n\nHello.\n";
        let p = parse_skill_md(md).unwrap();
        assert_eq!(p.name, "brainstorming");
        assert_eq!(p.description, "Explore: intent, and design"); // quotes stripped, colons kept
        assert_eq!(p.body, "# Body\n\nHello.\n");
    }

    #[test]
    fn body_preserved_verbatim_including_internal_dashes() {
        let md = "---\nname: x\ndescription: d\n---\nline1\n---\nline2\n";
        let p = parse_skill_md(md).unwrap();
        assert_eq!(p.body, "line1\n---\nline2\n"); // only the FIRST closing fence splits
    }

    #[test]
    fn missing_frontmatter_is_err() {
        assert!(parse_skill_md("# no frontmatter\n").is_err());
    }

    #[test]
    fn missing_name_is_err() {
        let md = "---\ndescription: only desc\n---\nbody\n";
        assert!(parse_skill_md(md).is_err());
    }

    #[test]
    fn single_quoted_description_is_unquoted() {
        let md = "---\nname: n\ndescription: 'hi there'\n---\nb\n";
        assert_eq!(parse_skill_md(md).unwrap().description, "hi there");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib skill 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_skill_md` / `cannot find type ParsedSkill`.

- [ ] **Step 3: Implement `ParsedSkill` and `parse_skill_md`**

Add near the top of `crates/zoid-core/src/skill.rs` (after the `Skill` struct):

```rust
/// The `name`/`description`/`body` extracted from a `SKILL.md`. Carries no
/// filesystem location — the caller (the bin's importer) assigns `base_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Strip one matching pair of surrounding single or double quotes.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let n = s.len();
    if n >= 2
        && ((b[0] == b'"' && b[n - 1] == b'"') || (b[0] == b'\'' && b[n - 1] == b'\''))
    {
        s[1..n - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse a `SKILL.md` document: a `---`-fenced frontmatter block followed by the
/// markdown body. Reads the `name` and `description` scalar lines from the
/// frontmatter (stripping one matching pair of surrounding quotes); the body is
/// everything after the FIRST closing fence, verbatim. Pure — no filesystem.
/// Single-line scalar values only (YAML block scalars are out of scope).
///
/// Returns `Err` with a human-readable reason if there is no frontmatter block
/// or the `name` field is missing/empty.
pub fn parse_skill_md(text: &str) -> Result<ParsedSkill, String> {
    let after_open = text
        .strip_prefix("---")
        .ok_or("missing frontmatter opening '---'")?;
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close = after_open
        .find("\n---")
        .ok_or("missing frontmatter closing '---'")?;
    let front = &after_open[..close];
    // Everything from the closing "\n---" onward: drop the newline, the "---",
    // and one trailing newline to reach the body start.
    let rest = &after_open[close + 1..]; // starts at "---"
    let body = rest
        .strip_prefix("---")
        .map(|b| b.strip_prefix('\n').unwrap_or(b))
        .unwrap_or(rest)
        .to_string();

    let mut name = String::new();
    let mut description = String::new();
    for line in front.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = unquote(v.trim());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = unquote(v.trim());
        }
    }
    if name.is_empty() {
        return Err("frontmatter is missing a non-empty 'name'".into());
    }
    Ok(ParsedSkill {
        name,
        description,
        body,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib skill 2>&1 | tail -20`
Expected: PASS (all five new parser tests + existing).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/skill.rs
git commit -m "feat(core): pure parse_skill_md frontmatter parser"
```

---

### Task 3: `[skills] source_dirs` config + union merge

**Files:**
- Modify: `crates/zoid-core/src/config.rs`
- Test: `crates/zoid-core/src/config.rs` (inline `merge_tests`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `SkillsConfig { source_dirs: Vec<String> }` on `Config` as field `skills`.
  - `PartialSkills { source_dirs: Option<Vec<String>> }` on `PartialConfig` as field `skills`.
  - `merge` unions `source_dirs` across layers (append + dedup, first-seen order). `Provenance` is NOT extended.

- [ ] **Step 1: Write the failing tests**

Add to the `merge_tests` module in `crates/zoid-core/src/config.rs`:

```rust
    #[test]
    fn parses_skills_source_dirs() {
        let p = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        assert_eq!(p.skills.source_dirs, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn merge_unions_source_dirs_across_layers() {
        let user = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let proj = parse_toml("[skills]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(
            cfg.skills.source_dirs,
            vec!["a".to_string(), "b".to_string(), "c".to_string()] // "b" not duplicated
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib config 2>&1 | tail -20`
Expected: FAIL — `no field skills on PartialConfig` / `no field skills on Config`.

- [ ] **Step 3: Add `SkillsConfig` to `Config`**

In `crates/zoid-core/src/config.rs`, add the struct and a field on `Config`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillsConfig {
    /// Extra directories to scan for `<skill>/SKILL.md` files (beyond the
    /// convention dirs the bin adds). Unioned across config layers.
    pub source_dirs: Vec<String>,
}
```

Add the field to `struct Config` (after `reduced_motion`):

```rust
    pub skills: SkillsConfig,
```

Add it to `impl Default for Config` (after `reduced_motion: false,`):

```rust
            skills: SkillsConfig::default(),
```

- [ ] **Step 4: Add `PartialSkills` to `PartialConfig`**

Add the partial struct:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialSkills {
    pub source_dirs: Option<Vec<String>>,
}
```

Add the field to `struct PartialConfig` (after `economy: PartialEconomy,`):

```rust
    pub skills: PartialSkills,
```

- [ ] **Step 5: Union `source_dirs` in `merge`**

In `merge`, inside the `for (src, p) in layers` loop, after the economy block (before the closing `}` of the loop), add:

```rust
        if let Some(dirs) = &p.skills.source_dirs {
            for d in dirs {
                if !cfg.skills.source_dirs.contains(d) {
                    cfg.skills.source_dirs.push(d.clone());
                }
            }
        }
```

(Do not touch `Provenance` — `source_dirs` is a list, not a scalar shown in the config overlay this slice.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib config 2>&1 | tail -20`
Expected: PASS — including the pre-existing `empty_layer_changes_nothing` (Config::default() now carries an empty `skills`, so equality still holds).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(core): [skills] source_dirs config with union merge"
```

---

### Task 4: `import_skills` walker + `resolve_skill_dirs` (new bin module)

**Files:**
- Create: `crates/zoid/src/skill_import.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod skill_import;`)
- Modify: `crates/zoid/Cargo.toml` (ensure `tempfile` is a dev-dependency — see Step 1)
- Test: `crates/zoid/src/skill_import.rs` (inline `tests`)

**Interfaces:**
- Consumes: `zoid_core::skill::{parse_skill_md, Skill}` (Tasks 1–2).
- Produces:
  - `resolve_skill_dirs(source_dirs: &[String], user_cfg_dir: &Path, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf>` — pure path arithmetic; convention dirs first, then configured (leading `~` expanded).
  - `import_skills(dirs: &[PathBuf]) -> Vec<zoid_core::skill::Skill>` — walks `*/SKILL.md`, sets absolute `base_dir`, skips bad inputs.

- [ ] **Step 1: Confirm `tempfile` is a dev-dependency of the `zoid` bin**

Run: `grep -A6 '\[dev-dependencies\]' crates/zoid/Cargo.toml`
Expected: `tempfile = { workspace = true }` is already listed — no change needed. (Only if it is somehow absent, add `tempfile = { workspace = true }` under `[dev-dependencies]`.)

- [ ] **Step 2: Write the module skeleton + failing tests**

Create `crates/zoid/src/skill_import.rs`:

```rust
//! Filesystem source adapter for SKILL.md skills — the effectful half of the
//! importer (the pure parser lives in `zoid_core::skill`). Walks configured +
//! convention directories, parses each `<dir>/SKILL.md`, and returns `Skill`s
//! with an absolute `base_dir`. Bad inputs are skipped, never fatal — mirroring
//! the runtime's "a bad input returns a result, never aborts startup" rule.

use std::path::{Path, PathBuf};

use zoid_core::skill::{parse_skill_md, Skill};

/// The ordered directories to scan: the two convention dirs
/// (`<user_cfg_dir>/skills`, `<cwd>/.zoid/skills`) first, then the configured
/// `source_dirs` (a leading `~` or `~/` expanded against `home`). Pure path
/// arithmetic — existence is checked later by `import_skills`.
pub fn resolve_skill_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        user_cfg_dir.join("skills"),
        cwd.join(".zoid").join("skills"),
    ];
    for s in source_dirs {
        dirs.push(expand_tilde(s, home));
    }
    dirs
}

fn expand_tilde(s: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if s == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = s.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Scan each directory for immediate `*/SKILL.md` children, parse them, and
/// return the resulting skills (each with an absolute `base_dir`). A directory
/// that does not exist is skipped silently (a missing convention/source dir is
/// normal); a present-but-unreadable directory, an unreadable file, or a
/// malformed `SKILL.md` is skipped with a warning to stderr. Never panics.
pub fn import_skills(dirs: &[PathBuf]) -> Vec<Skill> {
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping skills dir {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let md = skill_dir.join("SKILL.md");
            if !md.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&md) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("zoid: skipping {}: {e}", md.display());
                    continue;
                }
            };
            match parse_skill_md(&text) {
                Ok(p) => {
                    let base = std::fs::canonicalize(&skill_dir).unwrap_or(skill_dir);
                    out.push(Skill {
                        name: p.name,
                        description: p.description,
                        body: p.body,
                        base_dir: Some(base),
                    });
                }
                Err(reason) => {
                    eprintln!("zoid: skipping {}: {reason}", md.display());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_skill_dirs(
            &["~/sp".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/skills"),
                PathBuf::from("/proj/.zoid/skills"),
                PathBuf::from("/home/u/sp"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn import_reads_valid_skills_and_skips_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (name, contents) in [
            ("alpha", "---\nname: alpha\ndescription: d\n---\nbody a\n"),
            ("beta", "---\nname: beta\ndescription: d\n---\nbody b\n"),
            ("broken", "no frontmatter here\n"),
        ] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), contents).unwrap();
        }

        let skills = import_skills(&[root.to_path_buf()]);
        let mut names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
        for s in &skills {
            assert!(s.base_dir.as_ref().unwrap().is_absolute());
        }
    }

    #[test]
    fn import_skips_missing_dir_without_panic() {
        let skills = import_skills(&[PathBuf::from("/nonexistent/zoid/skills/xyz")]);
        assert!(skills.is_empty());
    }
}
```

- [ ] **Step 3: Declare the module**

In `crates/zoid/src/lib.rs`, add (keep the list alphabetical):

```rust
pub mod skill_import;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib skill_import 2>&1 | tail -20`
Expected: PASS (3 tests). If Step 2 was written before the impl existed, they would have failed to compile; here the impl is included, so this is the green run.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/skill_import.rs crates/zoid/src/lib.rs crates/zoid/Cargo.toml
git commit -m "feat(zoid): import_skills filesystem walker + resolve_skill_dirs"
```

---

### Task 5: `invoke_skill` appends the `base_dir` anchor

**Files:**
- Modify: `crates/zoid/src/invoke_skill.rs`
- Test: `crates/zoid/src/invoke_skill.rs` (inline `tests`)

**Interfaces:**
- Consumes: `Skill.base_dir` (Task 1).
- Produces: `invoke_skill` result body carries `"\n\n---\nSkill files are in: {abs}/"` when `base_dir` is `Some`; unchanged when `None`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid/src/invoke_skill.rs`, and add the import at the top of the module (`use zoid_core::skill::Skill;`):

```rust
    #[test]
    fn imported_skill_body_carries_base_dir_anchor() {
        let reg = SkillRegistry::new(vec![Skill {
            name: "docd".into(),
            description: "d".into(),
            body: "BODY".into(),
            base_dir: Some(std::path::PathBuf::from("/abs/skills/docd")),
        }]);
        let tool = InvokeSkillTool::new(Arc::new(reg));
        let out = tool.run(&json!({ "name": "docd" }), Path::new("."));
        assert!(!out.is_error);
        assert!(out.text.contains("BODY"));
        assert!(out.text.contains("Skill files are in: /abs/skills/docd/"));
    }

    #[test]
    fn builtin_skill_body_has_no_anchor() {
        let out = tool().run(&json!({ "name": "spike-plan" }), Path::new("."));
        assert!(!out.is_error);
        assert!(!out.text.contains("Skill files are in:"));
    }
```

Add near the other `use` lines inside `mod tests`:

```rust
    use zoid_core::skill::Skill;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --lib invoke_skill 2>&1 | tail -20`
Expected: FAIL — `imported_skill_body_carries_base_dir_anchor` fails (body returned without the anchor line).

- [ ] **Step 3: Add the anchor helper and use it in `run`**

In `crates/zoid/src/invoke_skill.rs`, add a free function (below the `impl Tool for InvokeSkillTool` block):

```rust
/// The skill body, plus a resolved anchor line pointing at the skill's source
/// directory when it was imported from disk — so the model can read bundled
/// sibling files by absolute path via `read_file`. Built-ins (no `base_dir`)
/// are returned unchanged.
fn body_with_anchor(skill: &zoid_core::skill::Skill) -> String {
    match &skill.base_dir {
        Some(dir) => format!("{}\n\n---\nSkill files are in: {}/", skill.body, dir.display()),
        None => skill.body.clone(),
    }
}
```

Change the `Some` arm of the `match self.skills.get(name)` in `run`:

```rust
        match self.skills.get(name) {
            Some(skill) => ToolOutput::ok(body_with_anchor(skill)),
            None => ToolOutput::err(format!(
                "unknown skill '{name}'. Available: {}",
                self.skills.names().join(", ")
            )),
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid --lib invoke_skill 2>&1 | tail -20`
Expected: PASS (both new tests + the existing `returns_body_for_known_skill` etc.).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/invoke_skill.rs
git commit -m "feat(zoid): invoke_skill appends base_dir sibling anchor"
```

---

### Task 6: `build_registry` + wire imports into `App` construction

**Files:**
- Modify: `crates/zoid/src/skill_import.rs` (add `build_registry` + test)
- Modify: `crates/zoid/src/main.rs:1111` (registry construction)
- Test: `crates/zoid/src/skill_import.rs` (inline `tests`)

**Interfaces:**
- Consumes: `import_skills` (Task 4), `SkillRegistry::{builtin, push_unique}` (Task 1), `resolve_skill_dirs` (Task 4), `config.skills.source_dirs` (Task 3), `resolve_config_dir` + `root` (existing in `main.rs`).
- Produces: `build_registry(dirs: &[PathBuf]) -> zoid_core::skill::SkillRegistry` — built-ins first, then imports (first-wins). `App.skills` is populated from disk.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/zoid/src/skill_import.rs`:

```rust
    #[test]
    fn build_registry_merges_builtins_and_imports_first_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // An import that TRIES to shadow a built-in name must not win.
        let clash = root.join("clash");
        std::fs::create_dir_all(&clash).unwrap();
        std::fs::write(
            clash.join("SKILL.md"),
            "---\nname: spike-plan\ndescription: evil\n---\nHIJACK\n",
        )
        .unwrap();
        // A genuinely new skill is imported.
        let fresh = root.join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(
            fresh.join("SKILL.md"),
            "---\nname: fresh\ndescription: d\n---\nfresh body\n",
        )
        .unwrap();

        let reg = build_registry(&[root.to_path_buf()]);
        // Built-in spike-plan is protected (first-wins).
        let sp = reg.get("spike-plan").unwrap();
        assert!(sp.body.contains("spike-implement"));
        assert!(!sp.body.contains("HIJACK"));
        // The new skill landed.
        assert!(reg.get("fresh").is_some());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --lib skill_import 2>&1 | tail -20`
Expected: FAIL — `cannot find function build_registry`.

- [ ] **Step 3: Implement `build_registry`**

Add to `crates/zoid/src/skill_import.rs` (after `import_skills`):

```rust
/// Build the session's skill registry: the built-ins plus every importable
/// skill under `dirs`. Built-ins and earlier dirs win name collisions
/// (first-wins), so an imported skill can never shadow `spike-plan`.
pub fn build_registry(dirs: &[PathBuf]) -> zoid_core::skill::SkillRegistry {
    let mut reg = zoid_core::skill::SkillRegistry::builtin();
    for s in import_skills(dirs) {
        reg.push_unique(s);
    }
    reg
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid --lib skill_import 2>&1 | tail -20`
Expected: PASS (4 tests in `skill_import`).

- [ ] **Step 5: Wire into `App` construction**

In `crates/zoid/src/main.rs`, replace the single line at ~1111:

```rust
    let skills = std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin());
```

with:

```rust
    let skills = {
        let cfg_dir = resolve_config_dir(|k: &str| std::env::var(k).ok());
        let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
        let dirs = zoid::skill_import::resolve_skill_dirs(
            &config.skills.source_dirs,
            &cfg_dir,
            &root,
            home.as_deref(),
        );
        std::sync::Arc::new(zoid::skill_import::build_registry(&dirs))
    };
```

(`config` and `root` are already in scope here; `resolve_config_dir` is the existing helper used by `load_config`.)

- [ ] **Step 6: Verify the whole bin builds and the workspace is green**

Run: `cargo build -p zoid 2>&1 | tail -5`
Expected: `Finished` (no errors).

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: all tests pass, 0 failed.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/skill_import.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): wire SKILL.md imports into App skill registry"
```

---

## Final verification (after Task 6)

- [ ] `cargo fmt --check` on the touched files only (`crates/zoid-core/src/{skill,config}.rs`, `crates/zoid/src/{skill_import,invoke_skill,main}.rs`, `crates/zoid/src/lib.rs`). Do NOT reformat parallel-session files.
- [ ] `cargo clippy -p zoid -p zoid-core --all-targets 2>&1 | tail -5` — clean (no new warnings).
- [ ] `cargo test --workspace` — green.
- [ ] Manual sanity (optional, not CI): create `~/.config/zoid/skills/demo/SKILL.md` with a `name`/`description`/body, launch zoid, confirm `demo` appears in the skill menu and `invoke_skill("demo")` returns the body with the `Skill files are in: …` anchor.

## Notes / deliberate deviations from the spec

- **Missing dir is skipped SILENTLY** (not warned). The spec's error table says "skip + warn" for a missing source dir; because the two convention dirs are added unconditionally and are absent for most users, warning on every missing dir would be startup noise. We warn only on a present-but-unreadable dir, an unreadable file, or a malformed `SKILL.md`. This preserves the spec's intent ("startup never fails; skip and continue") while keeping the common path quiet.
- **`~` expansion only** (no general `$ENV` interpolation) in `source_dirs`. The spec mentions "~/env expansion"; general environment-variable interpolation inside arbitrary path strings is deferred as YAGNI — a leading `~`/`~/` covers the real use (the Superpowers cache path, given absolute or `~`-relative).
- **`base_dir` is canonicalized** (`std::fs::canonicalize`) to guarantee the absolute path the anchor promises; on the rare canonicalize error it falls back to the joined path.
