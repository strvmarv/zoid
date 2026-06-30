# P5a · Sandbox Seam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lay the two filesystem foundations the subagent runtime needs — give the tool trait an explicit **working directory** (so a subagent can run tools somewhere other than the process cwd) and add a **git2 worktree** module (create + auto-cleanup) for isolated execution.

**Architecture:** `Tool::run` gains a `cwd: &Path` parameter; the five tools resolve relative paths against it (absolute paths pass through unchanged), and `run_tool` + the Chat agent loop thread it (Chat passes `"."` — process cwd, behavior-preserving). A new `crates/zoid/src/worktree.rs` wraps `git2` to create a worktree under `repo/.zoid/worktrees/<name>` and remove it on `Drop`. The worktree capability is **built and tested here but first exercised by Build in P6+** (Chat delegation in P5d runs in cwd, per spec §9) — substrate, like the P3 assembler.

**Tech Stack:** Rust 2021, `git2` (libgit2), `tempfile` (dev), `std::fs`/`std::process`.

## Global Constraints

- **Crates & dep direction:** the tool cwd seam lives in `zoid-tools`; the worktree module lives in the `zoid` bin (`git2` is a bin dependency). No new crate — the subagent runtime lives in the bin alongside `agent.rs` (P5 decision, 2026-06-30). `zoid-core` stays free of `git2`/process concerns.
- **Chat = cwd; worktree = substrate for Build (P5 decision, 2026-06-30):** the worktree module is built and unit-tested in P5a but **not** wired into Chat delegation (P5d runs delegated subagents in cwd, spec §9 "Chat = cwd + human-in-loop"). Build (P6+) is the first consumer. Do **not** add worktree usage to the agent loop or Chat path here.
- **Behavior-preserving cwd seam:** threading `cwd` must not change current behavior. Chat passes `Path::new(".")`; relative paths resolve against it (`cwd.join(rel)`), absolute paths pass through unchanged. Existing tool tests (which use absolute tempfile paths or cwd-independent shell) stay green after appending the `cwd` argument.
- **Tools remain cwd-scoped, not path-jailed (spec §9):** resolving against `cwd` is for subagent relocation, **not** a security sandbox — keep the existing "no path-jailing; Chat safe by human presence" stance. Do not add path-escape checks.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit.

---

### Task 1: Thread `cwd: &Path` through the tool trait and all five tools

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs` (`Tool` trait, `run_tool`, `resolve` helper, test call-sites)
- Modify: `crates/zoid-tools/src/read.rs`, `write.rs`, `edit.rs`, `search.rs`, `shell.rs` (`run` signature + path resolution + test call-sites)
- Test: inline (existing tests + one new resolution test).

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `trait Tool { fn run(&self, args: &Value, cwd: &Path) -> ToolOutput; … }`.
  - `pub(crate) fn resolve(cwd: &Path, path: &str) -> PathBuf` — `cwd.join(path)` for relative, passthrough for absolute.
  - `pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value, cwd: &Path) -> ToolOutput`.

> **This is one atomic task:** the trait change forces every impl + `run_tool` to update together, or the crate won't compile. Right-sized as a single reviewable unit (the whole `zoid-tools` crate compiles + tests pass at the end).

- [ ] **Step 1: Write the failing resolution test**

In `crates/zoid-tools/src/lib.rs` `mod tests`, add:

```rust
#[test]
fn resolve_joins_relative_and_passes_absolute() {
    use std::path::Path;
    assert_eq!(resolve(Path::new("/work"), "src/a.rs"), Path::new("/work/src/a.rs"));
    // absolute path ignores cwd
    assert_eq!(resolve(Path::new("/work"), "/etc/hosts"), Path::new("/etc/hosts"));
}

#[test]
fn read_tool_resolves_relative_to_cwd() {
    use std::path::Path;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "in cwd").unwrap();
    // relative path + cwd = the tempdir → reads the file
    let out = read::ReadFile.run(&serde_json::json!({ "path": "note.txt" }), dir.path());
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "in cwd");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tools --lib resolve`
Expected: compile error — `resolve` undefined / `run` takes a 1 argument.

- [ ] **Step 3: Add the trait change + `resolve` + `run_tool` in `lib.rs`**

At the top of `crates/zoid-tools/src/lib.rs`, add `use std::path::{Path, PathBuf};` (next to the existing `use serde_json::Value;`).

Change the trait method:

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput;
}
```

Add the resolver (after `str_arg`):

```rust
/// Resolve a tool's path argument against the run's working directory.
/// Relative paths join `cwd`; absolute paths pass through. This is for
/// subagent relocation, NOT a security jail (spec §9: no path-jailing).
pub(crate) fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}
```

Change `run_tool`:

```rust
pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value, cwd: &Path) -> ToolOutput {
    match tools.iter().find(|t| t.name() == name) {
        Some(t) => t.run(args, cwd),
        None => ToolOutput::err(format!("unknown tool: {name}")),
    }
}
```

In `lib.rs` `mod tests`, fix the existing `unknown_tool_is_error_not_panic` call: `run_tool(&reg, "nope", &json!({}), std::path::Path::new("."))`.

- [ ] **Step 4: Update `read.rs`**

Add `use std::path::Path;` at the top. Change `run`:

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        match std::fs::read_to_string(crate::resolve(cwd, &path)) {
            Ok(contents) => ToolOutput::ok(contents),
            Err(e) => ToolOutput::err(format!("read_file({path}): {e}")),
        }
    }
```

In `read.rs` `mod tests`, append `, std::path::Path::new(".")` to each `ReadFile.run(...)` call (the existing tests pass absolute tempfile paths or `/no/such/...`, so `cwd="."` is behavior-preserving).

- [ ] **Step 5: Update `write.rs`**

Add `use std::path::Path;`. Change `run`:

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") { Ok(p) => p, Err(e) => return e };
        let content = match str_arg(args, "content") { Ok(c) => c, Err(e) => return e };
        match std::fs::write(crate::resolve(cwd, &path), content.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("wrote {} bytes to {path}", content.len())),
            Err(e) => ToolOutput::err(format!("write_file({path}): {e}")),
        }
    }
```

In `write.rs` `mod tests`, append `, std::path::Path::new(".")` to each `WriteFile.run(...)` call.

- [ ] **Step 6: Update `edit.rs`**

Add `use std::path::Path;`. Change `run` to resolve once:

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") { Ok(p) => p, Err(e) => return e };
        let old = match str_arg(args, "old") { Ok(o) => o, Err(e) => return e };
        let new = match str_arg(args, "new") { Ok(n) => n, Err(e) => return e };
        let full = crate::resolve(cwd, &path);

        let contents = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("edit_file({path}): {e}")),
        };
        let count = contents.matches(&old).count();
        if count == 0 {
            return ToolOutput::err(format!("edit_file({path}): `old` not found"));
        }
        if count > 1 {
            return ToolOutput::err(format!("edit_file({path}): `old` is ambiguous ({count} matches)"));
        }
        let updated = contents.replacen(&old, &new, 1);
        match std::fs::write(&full, updated.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("edited {path}")),
            Err(e) => ToolOutput::err(format!("edit_file({path}): {e}")),
        }
    }
```

In `edit.rs` `mod tests`, append `, std::path::Path::new(".")` to each `EditFile.run(...)` call.

- [ ] **Step 7: Update `search.rs`**

Change `run` to root the walk at the resolved directory (`Path` is already imported in `search.rs`):

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let query = match str_arg(args, "query") { Ok(q) => q, Err(e) => return e };
        if query.is_empty() {
            return ToolOutput::err("search: empty query");
        }
        let root_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = crate::resolve(cwd, root_arg);
        let mut hits: Vec<String> = Vec::new();
        walk(&root, &root, &query, &mut hits);
        if hits.is_empty() {
            ToolOutput::ok(format!("no matches for {query:?}"))
        } else {
            let truncated = hits.len() >= MAX_RESULTS;
            let mut text = hits.join("\n");
            if truncated {
                text.push_str(&format!("\n… (truncated at {MAX_RESULTS} matches)"));
            }
            ToolOutput::ok(text)
        }
    }
```

(`walk` keeps its `&Path` signature; `&root` is `&PathBuf` which derefs to `&Path`.) Add `use std::path::Path;` to `search.rs` `mod tests` and append `, Path::new(".")` to each `Search.run(...)` call (the tests pass absolute tempdir paths, so `cwd="."` is behavior-preserving).

- [ ] **Step 8: Update `shell.rs`**

Add `use std::path::Path;`. Change `run` to set the command's working directory:

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let command = match str_arg(args, "command") { Ok(c) => c, Err(e) => return e };

        let output = if cfg!(windows) {
            Command::new("cmd").arg("/C").arg(&command).current_dir(cwd).output()
        } else {
            Command::new("sh").arg("-c").arg(&command).current_dir(cwd).output()
        };
        // ... (rest of the body unchanged)
```

Keep the rest of the body (stdout/stderr/exit handling) exactly as-is. In `shell.rs` `mod tests`, append `, std::path::Path::new(".")` to each `Shell.run(...)` call (shell tests are cwd-independent — `echo`/`exit`).

- [ ] **Step 9: Run the crate suite**

Run: `cargo test -p zoid-tools && cargo clippy -p zoid-tools --all-targets`
Expected: PASS (existing tests + 2 new), zero warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tools/src/
git commit -m "feat(tools): thread cwd through Tool::run + run_tool; resolve relative paths against it"
```

---

### Task 2: Thread `cwd` through the Chat agent loop

**Files:**
- Modify: `crates/zoid/src/agent.rs` (the `run_tool` call-site)
- Test: existing `crates/zoid/tests/` agent-loop tests stay green.

**Interfaces:**
- Consumes: `zoid_tools::run_tool(.., cwd: &Path)` (Task 1).
- Produces: no signature change to `run_agent_turn` (Chat passes `"."`); P5c generalizes the loop to accept a `cwd`.

> Minimal change: the Chat agent always runs in the process cwd, so pass `Path::new(".")`. The executor that needs a *different* cwd (a worktree, or a relocated root) is built in P5c, which generalizes `run_agent_turn`.

- [ ] **Step 1: Update the tool-execution call**

In `crates/zoid/src/agent.rs`, in `run_turn_inner`, the `spawn_blocking` closure currently calls `zoid_tools::run_tool(&tools_for_exec, &name, &args)`. Change it to pass the cwd:

```rust
            let out = tokio::task::spawn_blocking(move || {
                zoid_tools::run_tool(&tools_for_exec, &name, &args, std::path::Path::new("."))
            })
            .await?;
```

- [ ] **Step 2: Build + run the bin tests**

Run: `cargo test -p zoid && cargo clippy -p zoid --all-targets`
Expected: PASS (the agent-loop / economy-integration tests are unaffected — same cwd behavior), zero warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): Chat agent loop passes cwd to run_tool (process cwd)"
```

---

### Task 3: git2 worktree module (create + auto-cleanup)

**Files:**
- Modify: `Cargo.toml` (workspace `git2` dep) + `crates/zoid/Cargo.toml`
- Create: `crates/zoid/src/worktree.rs`
- Modify: `crates/zoid/src/lib.rs` (`pub mod worktree;`)
- Test: `crates/zoid/tests/worktree_test.rs` (new).

**Interfaces:**
- Consumes: `git2`.
- Produces:
  - `struct WorktreeGuard` with `fn path(&self) -> &Path`, removed on `Drop`.
  - `fn create_worktree(repo_root: &Path, name: &str) -> anyhow::Result<WorktreeGuard>` — creates a worktree at `repo_root/.zoid/worktrees/<name>`, branched from HEAD.

> Built + tested here; first *used* by Build in P6+ (Chat delegation runs in cwd). Not wired into the agent loop in P5.

- [ ] **Step 1: Add the dependency**

In the top-level `Cargo.toml` `[workspace.dependencies]`, add:

```toml
git2 = "0.19"
```

In `crates/zoid/Cargo.toml` `[dependencies]`, add:

```toml
git2 = { workspace = true }
```

> If `cargo build` fails to locate a system libgit2, enable the vendored build: `git2 = { workspace = true, features = ["vendored-libgit2"] }` in the bin manifest (heavier build, fully self-contained). Prefer the system lib if present (faster builds).

- [ ] **Step 2: Write the failing test**

`crates/zoid/tests/worktree_test.rs`:

```rust
use std::path::Path;
use zoid::worktree::create_worktree;

/// Init a git repo at `dir` with one committed file (worktrees need a HEAD).
fn init_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hi").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("zoid", "zoid@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
}

#[test]
fn worktree_is_a_working_copy_and_cleans_up_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let path;
    {
        let wt = create_worktree(tmp.path(), "sub-ax3").unwrap();
        path = wt.path().to_path_buf();
        assert!(path.exists(), "worktree dir should exist");
        assert!(path.join("a.txt").exists(), "HEAD content should be checked out");
    } // WorktreeGuard dropped here

    assert!(!path.exists(), "worktree dir removed on drop");
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid --test worktree_test`
Expected: compile error — `zoid::worktree` does not exist.

- [ ] **Step 4: Implement the module**

`crates/zoid/src/worktree.rs`:

```rust
//! Isolated git worktrees for subagent execution (spec §4.4/§9). Built here as
//! a runtime capability; first *used* by Build in P6+ (Chat delegation in P5d
//! runs in cwd). A `WorktreeGuard` removes its worktree on drop so a panicking
//! or abandoned subagent never leaks a registration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Repository, WorktreeAddOptions, WorktreePruneOptions};

/// An isolated worktree. Dropping it removes the working directory and prunes
/// the git registration (best-effort).
pub struct WorktreeGuard {
    name: String,
    path: PathBuf,
    repo_root: PathBuf,
}

impl WorktreeGuard {
    /// The worktree's checked-out directory — a subagent's `cwd`.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Create a worktree named `name` for the repo at `repo_root`, checked out at
/// `repo_root/.zoid/worktrees/<name>` and branched from HEAD.
pub fn create_worktree(repo_root: &Path, name: &str) -> Result<WorktreeGuard> {
    let repo = Repository::open(repo_root).context("open repo for worktree")?;
    let path = repo_root.join(".zoid").join("worktrees").join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create worktree parent dir")?;
    }
    let opts = WorktreeAddOptions::new();
    repo.worktree(name, &path, Some(&opts))
        .with_context(|| format!("add worktree {name}"))?;
    Ok(WorktreeGuard { name: name.to_string(), path, repo_root: repo_root.to_path_buf() })
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        // Remove the working dir first, then prune the registration. Best-effort:
        // Drop can't surface errors, and a leaked worktree is recoverable.
        let _ = std::fs::remove_dir_all(&self.path);
        if let Ok(repo) = Repository::open(&self.repo_root) {
            if let Ok(wt) = repo.find_worktree(&self.name) {
                let mut po = WorktreePruneOptions::new();
                po.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut po));
            }
        }
    }
}
```

In `crates/zoid/src/lib.rs`, add `pub mod worktree;`.

> If a `git2` API name differs in the resolved version (e.g. `WorktreePruneOptions::working_tree` ↔ `locked`), check `cargo doc -p git2 --open` and adapt — the shape (open repo → `worktree(name, path, opts)` → `find_worktree`/`prune`) is stable across recent versions.

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid --test worktree_test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zoid/Cargo.toml crates/zoid/src/worktree.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): git2 worktree module — create + auto-cleanup (Build-mode isolation substrate)"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `Tool::run` takes `cwd: &Path`; relative paths resolve against it, absolute pass through; existing tool behavior unchanged at `cwd="."`.
- [ ] `git2` is a `zoid` dependency only (grep — not in `zoid-core`).
- [ ] The worktree module is **not** referenced by the agent loop / Chat path (grep `create_worktree` — only the test uses it in P5a). Build wires it in P6+.
- [ ] No path-jailing added (spec §9 stance preserved).

## Self-Review notes (author)

- **Spec coverage (P5 foundations):** the tool **cwd seam** (T1/T2) is what lets a subagent run tools in a relocated root; the **git2 worktree** (T3) is the isolation capability spec §4.4 calls for. Both are substrate: P5b builds the constructed-context request, P5c the executor (which threads a real `cwd` and reuses the seam), P5d the Chat delegation (cwd-based). Worktree usage is deferred to Build (P6+) per the §9 Chat=cwd decision.
- **Type consistency:** `Tool::run(&self, args, cwd: &Path)` (T1) is the signature `run_tool` (T1) and the agent loop (T2) call; `resolve(cwd, path)` is the single path-resolution point (DRY) used by all five tools. `create_worktree(repo_root, name) -> WorktreeGuard` with `WorktreeGuard::path()` (T3) is the API P5c will pass as a subagent `cwd`.
- **Behavior-preserving:** every existing tool test stays green by appending `cwd = Path::new(".")` — verified because the tests use absolute tempfile paths (resolve passthrough) or cwd-independent shell. The one new behavior (relative-path resolution) is covered by `read_tool_resolves_relative_to_cwd`.
- **Risk:** `git2`/libgit2 linking is the one external risk — mitigated by the vendored-feature fallback note in T3.
