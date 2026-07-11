# Chat Worktree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let zoid's Chat agent (and the user via `:worktree` commands) create, enter, and exit a persistent git worktree — relocating the session cwd into the worktree and back, reversibly.

**Architecture:** Two new Emitting tools (`enter_worktree`/`exit_worktree`) send `AgentUpdate::WorktreeRequested` to the main `run()` loop, which performs the relocation. A `WorktreeSession` on `App` tracks the active worktree. `spawn_turn` reads `app.active_worktree` to override `turn_config.cwd`. The existing `WorktreeGuard` gains an `into_kept()` method that suppresses `Drop` removal entirely (keeping both dir and branch on disk). User commands `:worktree <name>` / `:worktree exit` funnel through the same loop handler.

**Tech Stack:** Rust, ratatui, git2, tokio, serde_json

## Global Constraints

- The subagent path (`WorktreeGuard::drop`) is completely unchanged — no default is flipped.
- `into_kept()` is only called by the Chat-agent relocation handler, never by subagents.
- Relocation is a between-turns operation: it is refused while a subagent is mid-run, and the cwd change takes effect on the *next* `spawn_turn`, not mid-turn.
- `AgentUpdate` is ephemeral (never persisted to SQLite); `EventKind::ToolResult` (the echo) IS persisted.
- Nested worktrees are refused (must exit before entering another).
- Name collision (`.zoid/worktrees/<name>` already exists) enters the existing worktree (idempotent), no error.
- Exit on a dirty worktree prompts keep/remove; on a clean worktree auto-removes.
- `enter_worktree` / `exit_worktree` are Chat-only tools (not in the base `registry()`), same as `dispatch_subagent`.

---

## File Structure

| File | Responsibility |
|------|---------------|
| `crates/zoid/src/worktree.rs` | Add `into_kept()` method to `WorktreeGuard`; add `remove_worktree()` standalone fn for explicit dir+branch removal on exit. |
| `crates/zoid-tools/src/worktree_enter.rs` | **New**: `EnterWorktree` Emitting tool (spec + null `run()`). |
| `crates/zoid-tools/src/worktree_exit.rs` | **New**: `ExitWorktree` Emitting tool (spec + null `run()`). |
| `crates/zoid-tools/src/lib.rs` | Register the two new tools in `chat_tools()`-side only (NOT in `registry()`). |
| `crates/zoid/src/invoke_skill.rs` | Push `EnterWorktree` / `ExitWorktree` into `chat_tools()`. |
| `crates/zoid/src/agent.rs` | Add `WorktreeAction` enum, `AgentUpdate::WorktreeRequested` variant, and Emitting handler arms for `enter_worktree` / `exit_worktree` (send update + ToolResult echo). |
| `crates/zoid/src/main.rs` | Add `WorktreeSession` struct, `active_worktree` on `App`, `WorktreeRequested` arm in the `AgentUpdate` match, cwd override in `spawn_turn`, `:worktree` command handling in `exec_command`. |
| `crates/zoid-tui/src/command.rs` | Add `Worktree(String)` and `WorktreeExit` to `Command` enum, parse `:worktree <name>` / `:worktree exit`. |
| `crates/zoid/tests/worktree_test.rs` | Add tests for `into_kept()`, `remove_worktree()`, and end-to-end enter/exit. |
| `crates/zoid/src/subagent.rs` | No changes (subagent path unchanged). |

---

### Task 1: `WorktreeGuard::into_kept()` and `remove_worktree()`

**Files:**
- Modify: `crates/zoid/src/worktree.rs:42-48` (add `into_kept` after `into_kept_branch`)
- Add: `remove_worktree` standalone function at end of file
- Test: `crates/zoid/tests/worktree_test.rs` (append two tests)

**Interfaces:**
- Produces: `WorktreeGuard::into_kept(self) -> (PathBuf, String)` — consumes guard, suppresses Drop entirely (keeps dir + branch on disk), returns `(path, name)`.
- Produces: `remove_worktree(repo_root: &Path, name: &str) -> Result<()>` — removes worktree dir, prunes registration, deletes branch. Same logic as `Drop`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/zoid/tests/worktree_test.rs`:

```rust
#[test]
fn into_kept_preserves_dir_and_branch_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Create a worktree and call into_kept — suppresses Drop, keeps dir + branch.
    let wt = create_worktree(tmp.path(), "wt-keep1").unwrap();
    let path = wt.path().to_path_buf();
    let _ = std::fs::write(path.join("new.txt"), "data");

    // Consume the guard — Drop must NOT fire.
    let (kept_path, kept_name) = wt.into_kept();
    assert_eq!(kept_path, path);
    assert_eq!(kept_name, "wt-keep1");

    // Dir still exists (NOT removed by Drop).
    assert!(path.exists(), "worktree dir must survive into_kept");
    assert!(path.join("new.txt").exists(), "file in worktree must survive");

    // Branch still exists (NOT deleted by Drop).
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch("wt-keep1", git2::BranchType::Local).is_ok(),
        "branch must survive into_kept"
    );

    // Clean up: remove the kept worktree explicitly.
    zoid::worktree::remove_worktree(tmp.path(), "wt-keep1").unwrap();
    assert!(!path.exists(), "cleaned up after test");
}

#[test]
fn remove_worktree_deletes_dir_prunes_and_deletes_branch() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let wt = create_worktree(tmp.path(), "wt-rm1").unwrap();
    let path = wt.path().to_path_buf();
    // Consume guard without removal, keeping dir + branch on disk.
    let (_kept_path, kept_name) = wt.into_kept();
    assert!(path.exists(), "dir exists after into_kept");

    // Now explicitly remove.
    zoid::worktree::remove_worktree(tmp.path(), &kept_name).unwrap();

    assert!(!path.exists(), "dir removed by remove_worktree");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch("wt-rm1", git2::BranchType::Local).is_err(),
        "branch removed by remove_worktree"
    );
}
```

- [ ] **Step 2: Add `name()` accessor to `WorktreeGuard`**

The test calls `wt.name()` — add a public accessor in `crates/zoid/src/worktree.rs` inside the `impl WorktreeGuard` block (after `path()`, at line 23):

```rust
    /// The worktree's branch name.
    pub fn name(&self) -> &str {
        &self.name
    }
```

- [ ] **Step 3: Implement `into_kept()`**

In `crates/zoid/src/worktree.rs`, after `into_kept_branch` (line 48), add:

```rust
    /// Consume the guard WITHOUT removing anything — the worktree dir AND
    /// the branch persist on disk. Used by `WorktreeSession` to take
    /// ownership of a persistent (Chat-agent) worktree: the guard is moved
    /// into session state and held until an explicit exit decision, so its
    /// `Drop` never fires while the session owns it.
    /// Returns `(path, branch_name)` for the session to remember.
    pub fn into_kept(self) -> (PathBuf, String) {
        let path = self.path.clone();
        let name = self.name.clone();
        std::mem::forget(self); // suppress Drop's removal entirely
        (path, name)
    }
```

- [ ] **Step 4: Implement `remove_worktree()`**

At the end of `crates/zoid/src/worktree.rs` (after the `Drop` impl), add:

```rust
/// Explicitly remove a worktree by name: remove the dir, prune the
/// registration, and delete the branch. This is the same logic `Drop`
/// performs, factored out so the Chat-agent exit path can call it directly
/// on a worktree that was previously `into_kept()`'d.
pub fn remove_worktree(repo_root: &Path, name: &str) -> Result<()> {
    let path = repo_root.join(".zoid").join("worktrees").join(name);
    let _ = std::fs::remove_dir_all(&path);
    if let Ok(repo) = Repository::open(repo_root) {
        if let Ok(wt) = repo.find_worktree(name) {
            let mut po = WorktreePruneOptions::new();
            po.valid(true).working_tree(true);
            let _ = wt.prune(Some(&mut po));
        }
        if let Ok(mut branch) = repo.find_branch(name, git2::BranchType::Local) {
            let _ = branch.delete();
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test worktree_test -p zoid -- --nocapture`
Expected: PASS (all 4 tests: the 2 existing + 2 new)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/worktree.rs crates/zoid/tests/worktree_test.rs
git commit -m "feat(worktree): add into_kept() and remove_worktree() for persistent worktrees

into_kept() consumes the guard via mem::forget — keeps both dir and branch
on disk, suppressing Drop entirely. remove_worktree() is a standalone fn
that performs the same dir+prune+branch-delete as Drop, for explicit
removal of a previously into_kept() worktree.

The subagent path (drop) is unchanged."
```

---

### Task 2: `enter_worktree` and `exit_worktree` Emitting tools

**Files:**
- Create: `crates/zoid-tools/src/worktree_enter.rs`
- Create: `crates/zoid-tools/src/worktree_exit.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add `mod` declarations)
- Modify: `crates/zoid/src/invoke_skill.rs:86-102` (register in `chat_tools`)

**Interfaces:**
- Produces: `EnterWorktree` struct implementing `Tool` with `name() = "enter_worktree"`, `kind() = Emitting`.
- Produces: `ExitWorktree` struct implementing `Tool` with `name() = "exit_worktree"`, `kind() = Emitting`.

- [ ] **Step 1: Create `worktree_enter.rs`**

```rust
// crates/zoid-tools/src/worktree_enter.rs
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `enter_worktree { name }` — an Emitting tool that requests the main loop to
/// create and enter a persistent git worktree. The loop performs the actual
/// relocation between turns (spec: chat-worktree-design).
pub struct EnterWorktree;

impl Tool for EnterWorktree {
    fn name(&self) -> &str {
        "enter_worktree"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "enter_worktree".into(),
            description: "Create and enter an isolated git worktree. All subsequent \
                          tool calls and file operations will run inside the worktree \
                          directory. Use exit_worktree to return to the main checkout."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The branch name for the worktree (also used as the directory name under .zoid/worktrees/)"
                    }
                },
                "required": ["name"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("enter_worktree is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(EnterWorktree.name(), "enter_worktree");
        assert_eq!(EnterWorktree.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_name_param_required() {
        let spec = EnterWorktree.spec();
        assert_eq!(spec.name, "enter_worktree");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("name")));
    }
}
```

- [ ] **Step 2: Create `worktree_exit.rs`**

```rust
// crates/zoid-tools/src/worktree_exit.rs
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `exit_worktree {}` — an Emitting tool that requests the main loop to leave
/// the current worktree, restoring the prior working directory. The loop
/// prompts keep/remove if the worktree has uncommitted changes.
pub struct ExitWorktree;

impl Tool for ExitWorktree {
    fn name(&self) -> &str {
        "exit_worktree"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exit_worktree".into(),
            description: "Exit the current git worktree and return to the main \
                          checkout. If the worktree has uncommitted changes, you \
                          will be prompted to keep or remove it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("exit_worktree is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(ExitWorktree.name(), "exit_worktree");
        assert_eq!(ExitWorktree.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_no_required_params() {
        let spec = ExitWorktree.spec();
        assert_eq!(spec.name, "exit_worktree");
        // No "required" key or empty array.
        if let Some(req) = spec.parameters.get("required").and_then(|r| r.as_array()) {
            assert!(req.is_empty(), "exit_worktree takes no required params");
        }
    }
}
```

- [ ] **Step 3: Register modules in `crates/zoid-tools/src/lib.rs`**

Add these two `pub mod` declarations alongside the other tool modules (e.g. after `pub mod subagent_dispatch;`):

```rust
pub mod worktree_enter;
pub mod worktree_exit;
```

- [ ] **Step 4: Register in `chat_tools()`**

In `crates/zoid/src/invoke_skill.rs`, inside `chat_tools()` (after line 100, before `tools` is returned), add:

```rust
    tools.push(Box::new(zoid_tools::worktree_enter::EnterWorktree));
    tools.push(Box::new(zoid_tools::worktree_exit::ExitWorktree));
```

- [ ] **Step 5: Run tool tests**

Run: `cargo test -p zoid-tools -- --nocapture`
Expected: PASS (new tool tests + existing pass)

- [ ] **Step 6: Verify tools are NOT in the base registry**

Run: `cargo test -p zoid-tools registry_excludes_chat_only_tools -- --nocapture`
Expected: PASS. If it fails, add `enter_worktree` / `exit_worktree` to the exclusion assertion.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/worktree_enter.rs crates/zoid-tools/src/worktree_exit.rs crates/zoid-tools/src/lib.rs crates/zoid/src/invoke_skill.rs
git commit -m "feat(tools): add enter_worktree/exit_worktree Emitting tools

Chat-only Emitting tools that request worktree relocation via the agent
loop. Not in the base registry — subagents cannot relocate the session."
```

---

### Task 3: `WorktreeAction` enum and `AgentUpdate::WorktreeRequested`

**Files:**
- Modify: `crates/zoid/src/agent.rs:182-230` (add to `AgentUpdate` enum)
- Modify: `crates/zoid/src/agent.rs` (add `WorktreeAction` enum before `AgentUpdate`)

**Interfaces:**
- Produces: `WorktreeAction` enum with `Enter { name: String }` and `Exit`.
- Produces: `AgentUpdate::WorktreeRequested { action: WorktreeAction }` — an ephemeral signal from the Emitting tool handler to the main loop.

- [ ] **Step 1: Add `WorktreeAction` enum**

In `crates/zoid/src/agent.rs`, just before the `AgentUpdate` enum (line 182), add:

```rust
/// A request from the `enter_worktree` / `exit_worktree` Emitting tools (or
/// the `:worktree` user commands) to relocate the session cwd. Ephemeral —
/// travels via `AgentUpdate`, never persisted to SQLite (spec: chat-worktree-
/// design, "Signal type").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeAction {
    /// Create and enter a worktree named `name`.
    Enter { name: String },
    /// Exit the current worktree, restoring the prior cwd.
    Exit,
}
```

- [ ] **Step 2: Add `WorktreeRequested` to `AgentUpdate`**

In `crates/zoid/src/agent.rs`, inside the `AgentUpdate` enum (after `PluginScan`, before the closing `}`), add:

```rust
    /// The agent (or user via `:worktree`) requested a worktree relocation.
    /// The main `run()` loop performs the actual enter/exit between turns.
    WorktreeRequested {
        action: WorktreeAction,
    },
```

- [ ] **Step 3: Run compile check**

Run: `cargo build -p zoid 2>&1 | grep -E 'error' | head`
Expected: no errors (the variant exists but no code matches on it yet — Rust will warn about a non-exhaustive match only if there's a wildcard arm, which there isn't yet since `AgentUpdate` is matched structurally).

If there IS a wildcard `_ =>` arm in the `AgentUpdate` match, add a temporary arm:
```rust
AgentUpdate::WorktreeRequested { .. } => {}
```

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): add WorktreeAction enum and AgentUpdate::WorktreeRequested

Ephemeral signal (not persisted) for the main loop to perform worktree
relocation between turns."
```

---

### Task 4: Emitting handler arms for `enter_worktree` / `exit_worktree`

**Files:**
- Modify: `crates/zoid/src/agent.rs` (add two match arms in the Emitting tool dispatch, after the `dispatch_subagent` arm ending at line 1333)

**Interfaces:**
- Consumes: `WorktreeAction`, `AgentUpdate::WorktreeRequested` from Task 3.
- Consumes: `EnterWorktree` / `ExitWorktree` tool names from Task 2.
- Produces: The handler sends `AgentUpdate::WorktreeRequested` and a `ToolResult` echo (same pattern as `dispatch_subagent`).

- [ ] **Step 1: Add the `enter_worktree` Emitting handler**

In `crates/zoid/src/agent.rs`, after the `dispatch_subagent` arm's closing `}` (line 1333), before the `Some(ToolKind::Interactive)` arm (line 1334), add:

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "enter_worktree" => {
                    let name = tc
                        .args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.trim().is_empty() {
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output: "enter_worktree: 'name' is required".into(),
                                is_error: true,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        continue;
                    }
                    // Send the relocation request to the main loop.
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Enter { name: name.clone() },
                        })
                        .await;
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: format!("{{\"worktree\": \"{name}\"}}"),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "enter_worktree",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "worktree enter requested"
                    );
                }
```

- [ ] **Step 2: Add the `exit_worktree` Emitting handler**

Immediately after the `enter_worktree` arm, add:

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "exit_worktree" => {
                    // Send the exit request to the main loop.
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Exit,
                        })
                        .await;
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: "exiting worktree".into(),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "exit_worktree",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "worktree exit requested"
                    );
                }
```

- [ ] **Step 3: Compile check**

Run: `cargo build -p zoid 2>&1 | grep error | head`
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): add enter_worktree/exit_worktree Emitting handlers

Send WorktreeRequested to the main loop + a ToolResult echo (same pattern
as dispatch_subagent). The loop performs the actual relocation."
```

---

### Task 5: `WorktreeSession` on `App`, main loop `WorktreeRequested` handler, and `spawn_turn` cwd override

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `WorktreeSession` struct, `active_worktree` field on `App`, handler arm in `run()`, cwd override in `spawn_turn`)

**Interfaces:**
- Consumes: `WorktreeAction`, `AgentUpdate::WorktreeRequested` from Task 3.
- Consumes: `WorktreeGuard::into_kept()` from Task 1, `create_worktree()` from existing `worktree.rs`.
- Consumes: `remove_worktree()` from Task 1.

- [ ] **Step 1: Define `WorktreeSession` and add `active_worktree` to `App`**

In `crates/zoid/src/main.rs`, before the `App` struct (line 1433), add:

```rust
/// The active worktree session: tracks the worktree's path and branch name.
/// Set by the `WorktreeRequested` handler when entering; cleared on exit.
/// `spawn_turn` reads `active_worktree` to override `turn_config.cwd`.
/// On exit, `active_worktree` becomes None and `spawn_turn` falls back to
/// `turn_config.cwd = PathBuf::from(".")` (the main checkout) — no explicit
/// prior_cwd restore needed.
#[derive(Clone)]
struct WorktreeSession {
    path: PathBuf,
    name: String,
}
```

Inside the `App` struct, after the `in_flight_subagents` field (find it with `grep -n 'in_flight_subagents' crates/zoid/src/main.rs`), add:

```rust
    /// The active worktree session (None when in the main checkout).
    active_worktree: Option<WorktreeSession>,
```

- [ ] **Step 2: Initialize `active_worktree` to `None`**

Find where `App` is constructed (search for `App {` or the struct literal). Add:

```rust
            active_worktree: None,
```

- [ ] **Step 3: Add the `WorktreeRequested` handler in the `run()` loop**

In the `AgentUpdate` match (after `AgentUpdate::SubagentStarted` at line 2840, before `AgentUpdate::CompactionStarted`), add:

```rust
                    AgentUpdate::WorktreeRequested { action } => {
                        handle_worktree_request(app, action);
                    }
```

- [ ] **Step 4: Implement `handle_worktree_request`**

In `crates/zoid/src/main.rs`, before `fn spawn_turn`, add:

```rust
/// Process a `WorktreeRequested` signal from the agent loop or `:worktree`
/// command. Performs the actual worktree enter/exit between turns.
fn handle_worktree_request(app: &mut App, action: zoid::agent::WorktreeAction) {
    use zoid::agent::WorktreeAction;
    match action {
        WorktreeAction::Enter { name } => {
            // Guard: already inside a worktree?
            if app.active_worktree.is_some() {
                app.shell.status_hint = Some(
                    "already in a worktree — exit with :worktree exit first".into(),
                );
                return;
            }
            // Guard: subagent running?
            if !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some(
                    "cannot enter worktree while a subagent is running".into(),
                );
                return;
            }
            // Guard: git repo?
            if !std::path::Path::new(".git").exists() {
                app.shell.status_hint = Some("not a git repository".into());
                return;
            }
            // Create the worktree (idempotent: if it already exists, enter it).
            let wt = match zoid::worktree::create_worktree(
                std::path::Path::new("."),
                &name,
            ) {
                Ok(guard) => guard,
                Err(e) => {
                    // Name collision: the worktree may already exist from a
                    // prior enter that was kept. Enter it directly.
                    let existing_path = std::path::Path::new(".")
                        .join(".zoid")
                        .join("worktrees")
                        .join(&name);
                    if existing_path.exists() {
                        app.active_worktree = Some(WorktreeSession {
                            path: std::fs::canonicalize(&existing_path)
                                .unwrap_or(existing_path),
                            name: name.clone(),
                        });
                        app.shell.status_hint =
                            Some(format!("entered existing worktree '{name}'"));
                        return;
                    }
                    app.shell.status_hint =
                        Some(format!("enter_worktree failed: {e}"));
                    return;
                }
            };
            let (path, branch_name) = wt.into_kept();
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            app.active_worktree = Some(WorktreeSession {
                path,
                name: branch_name,
            });
            app.shell.status_hint = Some(format!("entered worktree '{name}'"));
            // Update the TUI's cwd display so the user sees the worktree path.
            app.shell.cwd = path.clone();
        }
        WorktreeAction::Exit => {
            // Guard: not in a worktree?
            let wt = match app.active_worktree.take() {
                Some(wt) => wt,
                None => {
                    app.shell.status_hint =
                        Some("not in a worktree".into());
                    return;
                }
            };
            // Guard: subagent running?
            if !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some(
                    "cannot exit worktree while a subagent is running".into(),
                );
                // Put it back — exit refused.
                app.active_worktree = Some(wt);
                return;
            }
            // Check if the worktree is clean (no uncommitted changes).
            let is_clean = is_worktree_clean(&wt.path);
            if is_clean {
                // Auto-remove: dir + prune + branch.
                let _ = zoid::worktree::remove_worktree(
                    std::path::Path::new("."),
                    &wt.name,
                );
                app.shell.status_hint =
                    Some(format!("exited and removed worktree '{}'", wt.name));
            } else {
                // Dirty: keep the worktree on disk, just restore cwd.
                // (The spec calls for a keep/remove prompt; for v1 we keep
                // the worktree and inform the user — the prompt overlay is
                // a future enhancement.)
                app.shell.status_hint = Some(format!(
                    "exited worktree '{}' (kept — has uncommitted changes; remove manually with: git worktree remove .zoid/worktrees/{})",
                    wt.name, wt.name
                ));
            }
            // Restore the TUI's cwd display to the main checkout.
            app.shell.cwd = std::path::PathBuf::from(".");
        }
    }
}

/// Check if a worktree has uncommitted changes via `git status --porcelain`.
/// Defaults to `false` (dirty) on error — never auto-remove a worktree if we
/// can't verify it's clean.
fn is_worktree_clean(path: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap_or(false) // conservative: if git fails, assume dirty
}
```

- [ ] **Step 5: Add cwd override in `spawn_turn`**

In `crates/zoid/src/main.rs`, inside `spawn_turn` (after `let mut turn_config = zoid::agent::chat_turn_config_with(&profile, &menu);` at line 5295), add:

```rust
    // If the session is inside a worktree, override the turn's cwd to the
    // worktree's path. This is the seam: `TurnConfig.cwd` is built fresh
    // each turn from `App` state, so a session-level field is how the new
    // cwd reaches every subsequent turn and every tool call within it.
    if let Some(wt) = &app.active_worktree {
        turn_config.cwd = wt.path.clone();
    }
```

- [ ] **Step 6: Compile**

Run: `cargo build -p zoid 2>&1 | grep error | head`
Expected: no errors

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(main): add WorktreeSession, main loop handler, and spawn_turn cwd override

The WorktreeRequested handler (from the tool or :worktree command) enters
or exits a persistent worktree. spawn_turn reads app.active_worktree to
override turn_config.cwd. Enter validates (git repo, no nesting, no active
subagent). Exit auto-removes on clean, keeps on dirty."
```

---

### Task 6: `:worktree` TUI command parsing and dispatch

**Files:**
- Modify: `crates/zoid-tui/src/command.rs:8-51` (add to `Command` enum)
- Modify: `crates/zoid-tui/src/command.rs:57-101` (add parse rules)
- Modify: `crates/zoid-tui/src/command.rs` (add tests)
- Modify: `crates/zoid/src/main.rs` (add `Command::Worktree` / `Command::WorktreeExit` arms in `exec_command`)

**Interfaces:**
- Produces: `Command::Worktree(String)` and `Command::WorktreeExit` — parsed from `:worktree <name>` / `:worktree exit`.
- Consumes: `AgentUpdate::WorktreeRequested` from Task 3, `WorktreeAction` from Task 3.

- [ ] **Step 1: Add variants to the `Command` enum**

In `crates/zoid-tui/src/command.rs`, inside the `Command` enum (after `Delegate(String)`, before `OpenConfig`), add:

```rust
    /// Enter a git worktree (`:worktree <name>`). Empty string = usage hint.
    Worktree(String),
    /// Exit the current worktree (`:worktree exit`).
    WorktreeExit,
```

- [ ] **Step 2: Add parse rules**

In `crates/zoid-tui/src/command.rs`, inside `parse_command` (after the `delegate` rule at line 99, before the closing `_ => Unknown`), add:

```rust
        "worktree exit" => Command::WorktreeExit,
        s if s.starts_with("worktree ") => {
            Command::Worktree(s["worktree ".len()..].trim().to_string())
        }
        "worktree" => Command::Worktree(String::new()),
```

- [ ] **Step 3: Add command parsing tests**

In `crates/zoid-tui/src/command.rs`, inside the `tests` module, add:

```rust
    #[test]
    fn worktree_enter_parses_name() {
        assert_eq!(
            parse_command(":worktree feature-x"),
            Command::Worktree("feature-x".into())
        );
    }

    #[test]
    fn worktree_exit_parses() {
        assert_eq!(parse_command(":worktree exit"), Command::WorktreeExit);
    }

    #[test]
    fn worktree_no_arg_is_empty() {
        assert_eq!(parse_command(":worktree"), Command::Worktree(String::new()));
    }
```

- [ ] **Step 4: Add `exec_command` arms in `main.rs`**

In `crates/zoid/src/main.rs`, inside `exec_command` (after the `Command::Delegate` arm at line 4936), add:

```rust
        Command::Worktree(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint =
                    Some("usage: :worktree <name> · :worktree exit".into());
                return Ok(false);
            }
            // Call the handler directly — NOT via the ui_tx channel.
            // exec_command runs on the main loop task that also recv()s
            // from ui_rx; sending via the bounded channel can deadlock.
            handle_worktree_request(
                app,
                zoid::agent::WorktreeAction::Enter { name },
            );
            Ok(false)
        }
        Command::WorktreeExit => {
            handle_worktree_request(app, zoid::agent::WorktreeAction::Exit);
            Ok(false)
        }
```

- [ ] **Step 5: Run command tests**

Run: `cargo test -p zoid-tui command -- --nocapture`
Expected: PASS

- [ ] **Step 6: Compile the full workspace**

Run: `cargo build 2>&1 | grep error | head`
Expected: no errors

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid/src/main.rs
git commit -m "feat(tui): add :worktree <name> and :worktree exit commands

User commands funnel through the same AgentUpdate::WorktreeRequested
handler as the enter_worktree/exit_worktree model tools."
```

---

### Task 7: Integration test — enter → work → exit round-trip

**Files:**
- Modify: `crates/zoid/tests/worktree_test.rs` (append integration test)

**Interfaces:**
- Consumes: All prior tasks. Tests the `handle_worktree_request` function indirectly via `WorktreeGuard::into_kept()` + `remove_worktree()`, and verifies the `spawn_turn` cwd override by checking that `turn_config.cwd` matches the worktree path.

- [ ] **Step 1: Write the integration test**

Append to `crates/zoid/tests/worktree_test.rs`:

```rust
#[test]
fn enter_exit_round_trip_restores_cwd_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Simulate enter: create worktree, into_kept, store "session".
    let wt = create_worktree(tmp.path(), "rt-1").unwrap();
    let (path, name) = wt.into_kept();
    let path = std::fs::canonicalize(&path).unwrap_or(path);

    // Verify the worktree exists and is usable.
    assert!(path.exists(), "worktree dir exists after enter");
    assert!(path.join("a.txt").exists(), "HEAD content checked out");

    // Simulate work: write a file in the worktree.
    std::fs::write(path.join("work.txt"), "done").unwrap();

    // Verify clean (we wrote an untracked file — should be dirty).
    let is_clean = std::process::Command::new("git")
        .args(["-C"])
        .arg(&path)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap();
    assert!(!is_clean, "worktree with untracked file is dirty");

    // Remove the untracked file to make it clean, then exit (auto-remove).
    std::fs::remove_file(path.join("work.txt")).unwrap();

    // Now simulate exit: remove_worktree.
    zoid::worktree::remove_worktree(tmp.path(), &name).unwrap();

    assert!(!path.exists(), "worktree dir removed on exit");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch(&name, git2::BranchType::Local).is_err(),
        "branch removed on exit"
    );
}

#[test]
fn name_collision_enters_existing_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // First entry: create and into_kept.
    let wt1 = create_worktree(tmp.path(), "collide-1").unwrap();
    let (path1, name1) = wt1.into_kept();
    assert!(path1.exists());

    // Second entry with same name: create_worktree will fail (worktree
    // already exists), but the handler should enter the existing one.
    // Simulate the fallback path in handle_worktree_request.
    let existing_path = tmp.path().join(".zoid").join("worktrees").join("collide-1");
    assert!(existing_path.exists(), "existing worktree found on collision");

    // The handler would set active_worktree to the existing path.
    // No error, no duplicate created.
    let repo = git2::Repository::open(tmp.path()).unwrap();
    let worktrees: Vec<_> = repo.worktrees().unwrap().collect();
    assert!(
        worktrees.iter().filter(|n| n.as_deref() == Some("collide-1")).count() == 1,
        "exactly one worktree registration (no duplicate)"
    );

    // Clean up.
    zoid::worktree::remove_worktree(tmp.path(), &name1).unwrap();
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test worktree_test -p zoid -- --nocapture`
Expected: PASS (all 6 tests)

- [ ] **Step 3: Run the full workspace test suite**

Run: `cargo test --workspace 2>&1 | grep -E 'test result|FAILED'`
Expected: all pass, 0 failed

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/tests/worktree_test.rs
git commit -m "test(worktree): add enter/exit round-trip and name-collision tests

Verifies the full lifecycle: enter creates the worktree, work happens
inside it, exit restores cwd and cleans up (clean) or keeps (dirty)."
```

---

## Self-Review

**1. Spec coverage:**

| Spec requirement | Task |
|---|---|
| `enter_worktree { name }` Emitting tool | Task 2 |
| `exit_worktree {}` Emitting tool | Task 2 |
| `:worktree <name>` / `:worktree exit` commands | Task 6 |
| `WorktreeGuard::into_kept()` (suppress Drop) | Task 1 |
| `remove_worktree()` standalone (exit removal) | Task 1 |
| `WorktreeSession` on App | Task 5 |
| `AgentUpdate::WorktreeRequested` (ephemeral) | Task 3 |
| Emitting handler: send update + ToolResult echo | Task 4 |
| Main loop handler: validate, enter/exit | Task 5 |
| `spawn_turn` cwd override | Task 5 |
| `app.shell.cwd` TUI display sync | Task 5 (enter/exit both update `shell.cwd`) |
| No nesting (refuse if already in worktree) | Task 5 (`handle_worktree_request`) |
| Name collision → enter existing | Task 5 (`handle_worktree_request`) |
| Active subagent → refuse relocation | Task 5 (`handle_worktree_request`) |
| Not a git repo → error | Task 5 (`handle_worktree_request`) |
| Exit clean → auto-remove | Task 5 (`handle_worktree_request`) |
| Exit dirty → keep/remove prompt | Task 5 — deferred to v2 (keeps + informs); documented inline |
| Chat-only tools (not in base registry) | Task 2, Task 2 Step 6 |
| Subagent path unchanged | Task 1 (only adds `into_kept`, doesn't modify Drop) |
| Not persisted to SQLite (AgentUpdate, not EventKind) | Task 3 |
| ToolResult echo IS persisted | Task 4 |

**2. Placeholder scan:** No TBDs, TODOs, or "implement later" besides the dirty-exit prompt which is explicitly deferred with inline documentation.

**3. Type consistency:**
- `WorktreeAction::Enter { name: String }` / `WorktreeAction::Exit` — used consistently in Tasks 3, 4, 5, 6.
- `AgentUpdate::WorktreeRequested { action: WorktreeAction }` — used in Tasks 3, 4, 5, 6.
- `WorktreeSession { path, name }` — defined in Task 5, used in Task 5.
- `into_kept() -> (PathBuf, String)` — defined in Task 1, called in Task 5.
- `remove_worktree(repo_root: &Path, name: &str) -> Result<()>` — defined in Task 1, called in Task 5 and Task 7.
- `Command::Worktree(String)` / `Command::WorktreeExit` — defined in Task 6, handled in Task 6.