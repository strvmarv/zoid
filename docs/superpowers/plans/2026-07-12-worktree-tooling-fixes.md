# Worktree Tooling Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `enter_worktree`/`exit_worktree` actually relocate the main Chat agent's in-flight work (commits land on the worktree branch; exit doesn't break tooling) and make the TUI right-rail Repo widget reflect the active worktree — by turning the fire-and-forget worktree signal into a synchronous request/response and making the git poller worktree-aware.

**Architecture:** Today `enter_worktree`/`exit_worktree` are `Emitting` tools: the turn sends a best-effort `AgentUpdate::WorktreeRequested` and *immediately* emits an optimistic success `ToolResult`, then keeps running against a stale per-turn cwd snapshot (`cwd_for_exec`, taken once at turn start). We make the emit a **request/response**: the turn sends a `oneshot` reply channel, `await`s the main loop's reply carrying the new absolute cwd (or an error), reassigns a now-`mut cwd_for_exec`, and emits a *real* `ToolResult`. The main-loop handler becomes a thin wrapper over a pure, testable core (`compute_worktree_switch`) that does the git work and returns the new cwd. A `watch<Option<PathBuf>>` cell makes the 5s git poller open the active worktree path and re-poll *immediately* on enter/exit, so the rail reflects the worktree with no lag and no revert.

**Tech Stack:** Rust, tokio (`oneshot`, `watch`, `select!`), `git2`, the existing `run_agent_turn_cancellable` turn loop, ratatui TUI (`zoid-tui`).

## Global Constraints

- Scope is the **main Chat agent only**. Subagent-dispatch worktrees (`.zoid/worktrees/sub-*`) and their rail display are OUT of scope — the rail indicator is for the main loop.
- **Enter** returns the worktree's **absolute** path as the new cwd. **Exit** returns the **absolute repo root**, computed **before** `remove_worktree` deletes anything.
- **`App.active_worktree` is `Option<WorktreeSession>`** (`WorktreeSession { path: PathBuf, name: String }`, a plain struct with NO `Drop`), NOT `Option<WorktreeGuard>`. Enter must call `create_worktree(...)?.into_kept()` — which moves the guard's contents into the `Drop`-free session and **suppresses `WorktreeGuard::Drop`** (Drop would prune the dir AND delete the branch). Never store a live `WorktreeGuard` in session state.
- **Dirty worktree on exit is KEPT (not removed):** clean exit calls `remove_worktree(...)`; dirty exit does **nothing** (the `WorktreeSession` has no `Drop`, so the dir + branch survive). The reply still returns the repo root so tooling keeps working.
- **Idempotent re-enter:** if `create_worktree` fails because the dir already exists (the dirty-kept case), enter the existing dir instead of erroring.
- Worktree op failures become a **real error `ToolResult`** on the turn path, and a `status_hint` (the error message) on the `:worktree` slash-command path (no active turn there). Never a fake success.
- Remove the ~10 worktree-specific `status_hint` writes inside the current `handle_worktree_request` (WT-4). The single generic `status_hint` on the slash-command error path (below) is the only worktree `status_hint` that remains. The slot, its render (`render.rs:375-380`), and all ~56 other producers are UNTOUCHED.
- The turn `await`s the reply while the main `select!` loop keeps draining `ui_rx` — no deadlock (the turn is a detached `tokio::spawn`, the loop drains `ui_rx.recv()`). The handler MUST send a reply on every path, or the turn's await hangs.
- Iteration cap, event schema, DB: unchanged (transient runtime + display state only).
- Never add Co-Authored-By or any co-author trailer to commits.
- Each task ends with `cargo test -p zoid` green; a field/variant/signature change to a widely-constructed type demands a **FULL-WORKSPACE `cargo test`** (an exhaustive struct/enum literal in another crate breaks silently otherwise).

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid/src/agent.rs` | `AgentUpdate::WorktreeRequested` gains a `oneshot` reply; the `enter_worktree`/`exit_worktree` Emitting arms create it, `await` it, reassign `mut cwd_for_exec`, and emit a real success/error `ToolResult` (drop the optimistic one). |
| `crates/zoid/src/main.rs` | New pure `compute_worktree_switch(...) -> Result<PathBuf, String>` (git work over `Option<WorktreeSession>`); `handle_worktree_request` becomes a thin wrapper that applies the cwd, replies (turn path) or sets a `status_hint` on error (slash path), and (Task 2) updates the poller's watch cell; the `WorktreeRequested` dispatch threads the reply; the git poller becomes worktree-aware + re-polls on change; add `git_status_at`/`current_branch_at`. |
| `crates/zoid/src/worktree.rs` | No change — `create_worktree`/`remove_worktree`/`into_kept` are correct (reference only). |
| `crates/zoid-tui/src/render.rs`, `state.rs` | No change (the rail already renders `shell.branch`/`shell.worktree`; the fix is upstream in what feeds them). |

**Test location:** `compute_worktree_switch`, `current_branch_at`, `git_status_at` all live in `main.rs`, which is BOTH a `[lib]` and `[[bin]]` target — but items defined in `main.rs` are only reachable from **inline** `#[cfg(test)] mod` blocks via `super::`, not from `tests/` integration files. So all tests go in an inline `#[cfg(test)] mod worktree_switch_tests` at the bottom of `main.rs` — exactly how the existing code tests `subagent_kill_decision`/`escalate_cancel` (`main.rs:~5613`). `tempfile` is already a dev-dependency (`crates/zoid/Cargo.toml:63`).

Two tasks: **Task 1** = the synchronous switch (WT-1, WT-2, WT-4). **Task 2** = the worktree-aware, change-driven poller (WT-3, both immediate and durable).

---

### Task 1: Synchronous worktree switch (WT-1 + WT-2 + WT-4)

**Files:**
- Modify: `crates/zoid/src/agent.rs` — `WorktreeRequested` variant (`~:369`); `enter_worktree` arm (`~:1557-1610`); `exit_worktree` arm (`~:1611-1640`); `cwd_for_exec` (`~:1005`).
- Modify: `crates/zoid/src/main.rs` — add `compute_worktree_switch`; rewrite `handle_worktree_request` (`~:5300-5394`); `WorktreeRequested` dispatch (`~:2997`); `:worktree` slash callers (`~:5065`, `~:5077`); add the inline test module.

> Line numbers are from `main` @ `7510bd2` and WILL drift. Use named anchors (the `WorktreeRequested {` variant, `if tc.name == "enter_worktree"`, `fn handle_worktree_request`, `Command::Worktree(name)`, `Command::WorktreeExit`); grep if a number is off. Confirmed accurate at authoring: `cwd_for_exec` is `let` (not `mut`) at `:1005`; the enter/exit arms do fire-and-forget + optimistic `ToolResult`.

**Interfaces:**
- Consumes: `zoid::worktree::{create_worktree, remove_worktree}`, `zoid::agent::WorktreeAction { Enter { name }, Exit }`, the existing `WorktreeSession { path, name }` struct (`main.rs:~1486`), `is_worktree_clean(&Path)` (`main.rs:~5399`).
- Produces: `AgentUpdate::WorktreeRequested { action: WorktreeAction, reply: tokio::sync::oneshot::Sender<Result<std::path::PathBuf, String>> }`; `fn compute_worktree_switch(active: &mut Option<WorktreeSession>, action: zoid::agent::WorktreeAction, subagent_running: bool, repo_root: &std::path::Path) -> Result<std::path::PathBuf, String>` (Enter → abs worktree path, sets `*active`; Exit → abs repo root, clears `*active`; guards → `Err(msg)`).

- [ ] **Step 1: Write the failing inline test module.** At the bottom of `crates/zoid/src/main.rs`, add:

```rust
#[cfg(test)]
mod worktree_switch_tests {
    use super::compute_worktree_switch;
    use std::path::Path;
    use std::process::Command;
    use zoid::agent::WorktreeAction;

    /// `git init` a fresh repo with one commit so HEAD exists (worktrees need a commit).
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(p)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(p.join("f.txt"), "hi").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn enter_returns_absolute_worktree_path_and_sets_active() {
        let repo = init_repo();
        let mut active = None;
        let cwd = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter { name: "feature-x".into() },
            false,
            repo.path(),
        )
        .expect("enter should succeed");
        assert!(cwd.is_absolute(), "enter cwd must be absolute: {cwd:?}");
        assert!(
            cwd.ends_with(Path::new(".zoid/worktrees/feature-x")),
            "enter cwd points at the worktree: {cwd:?}"
        );
        assert!(cwd.exists(), "the worktree dir was created");
        assert!(active.is_some(), "active worktree is now set");
    }

    #[test]
    fn enter_guard_already_in_worktree_errors() {
        let repo = init_repo();
        let mut active = None;
        compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "a".into() }, false, repo.path()).unwrap();
        let err = compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "b".into() }, false, repo.path()).unwrap_err();
        assert!(err.contains("already in a worktree"), "got: {err}");
    }

    #[test]
    fn enter_guard_subagent_running_errors() {
        let repo = init_repo();
        let mut active = None;
        let err = compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "a".into() }, true, repo.path()).unwrap_err();
        assert!(err.contains("subagent"), "got: {err}");
        assert!(active.is_none(), "guard must not enter");
    }

    #[test]
    fn exit_returns_absolute_repo_root_and_clears() {
        let repo = init_repo();
        let mut active = None;
        compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "a".into() }, false, repo.path()).unwrap();
        let cwd = compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).expect("exit should succeed");
        assert!(cwd.is_absolute());
        assert_eq!(
            cwd.canonicalize().unwrap(),
            repo.path().canonicalize().unwrap(),
            "exit cwd is the repo root"
        );
        assert!(active.is_none(), "exit clears the active worktree");
    }

    #[test]
    fn exit_guard_not_in_worktree_errors() {
        let repo = init_repo();
        let mut active = None;
        let err = compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).unwrap_err();
        assert!(err.contains("not in a worktree"), "got: {err}");
    }

    #[test]
    fn dirty_exit_keeps_worktree_and_reenter_succeeds() {
        // I2 + dirty-keep: a worktree with uncommitted changes survives exit and
        // can be re-entered without error.
        let repo = init_repo();
        let mut active = None;
        let cwd = compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "keep".into() }, false, repo.path()).unwrap();
        // Dirty it: modify the tracked file inside the worktree.
        std::fs::write(cwd.join("f.txt"), "uncommitted change").unwrap();
        compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).unwrap();
        assert!(cwd.exists(), "dirty worktree dir must be KEPT on exit (no data loss)");
        // Re-enter the same name — must NOT error (idempotent re-enter).
        let cwd2 = compute_worktree_switch(&mut active, WorktreeAction::Enter { name: "keep".into() }, false, repo.path()).unwrap();
        assert!(cwd2.exists());
        assert!(active.is_some());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails.**

Run: `cargo test -p zoid worktree_switch_tests`
Expected: compile error — `compute_worktree_switch` does not exist yet.

- [ ] **Step 3: Add `compute_worktree_switch`.** In `crates/zoid/src/main.rs`, next to `fn handle_worktree_request` (grep it), add the pure core. It operates on `Option<WorktreeSession>` and uses `into_kept()` so no live `WorktreeGuard` is ever stored (preventing the dirty-exit branch deletion):

```rust
/// Perform the worktree enter/exit git work and return the new absolute cwd for
/// the in-flight turn. Guard failures return `Err(msg)`. Does NOT touch `App`,
/// `status_hint`, or the rail — the caller applies the cwd and the poller owns
/// the rail labels.
///
/// `repo_root` is the main checkout root (`"."` in production; a temp dir in tests).
/// On Enter, `*active` is set to the new `WorktreeSession`; on Exit it is cleared.
fn compute_worktree_switch(
    active: &mut Option<WorktreeSession>,
    action: zoid::agent::WorktreeAction,
    subagent_running: bool,
    repo_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    use zoid::agent::WorktreeAction;
    match action {
        WorktreeAction::Enter { name } => {
            if active.is_some() {
                return Err("already in a worktree — exit with :worktree exit first".into());
            }
            if subagent_running {
                return Err("cannot enter worktree while a subagent is running".into());
            }
            if !repo_root.join(".git").exists() {
                return Err("not a git repository".into());
            }
            // Create the worktree, or (idempotent re-enter) adopt an existing
            // dir left by a prior dirty-kept exit. `into_kept()` moves the
            // guard's contents into a Drop-free session so the dir + branch are
            // never auto-removed.
            let (path, sess_name) = match zoid::worktree::create_worktree(repo_root, &name) {
                Ok(guard) => guard.into_kept(),
                Err(e) => {
                    let existing = repo_root.join(".zoid").join("worktrees").join(&name);
                    if existing.exists() {
                        (existing, name.clone())
                    } else {
                        return Err(format!("enter_worktree failed: {e}"));
                    }
                }
            };
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            *active = Some(WorktreeSession {
                path: path.clone(),
                name: sess_name,
            });
            Ok(path)
        }
        WorktreeAction::Exit => {
            let wt = match active.take() {
                Some(wt) => wt,
                None => return Err("not in a worktree".into()),
            };
            if subagent_running {
                *active = Some(wt); // put it back — exit refused
                return Err("cannot exit worktree while a subagent is running".into());
            }
            // Absolute repo root computed BEFORE any removal, so tooling never
            // points at a deleted dir (WT-2).
            let root = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
            // Clean → remove (dir + prune + branch). Dirty → keep (WorktreeSession
            // has no Drop, so doing nothing preserves the user's work).
            if is_worktree_clean(&wt.path) {
                let _ = zoid::worktree::remove_worktree(repo_root, &wt.name);
            }
            Ok(root)
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes.**

Run: `cargo test -p zoid worktree_switch_tests`
Expected: PASS — all six tests green (enter path, guards, exit root, dirty-keep + re-enter).

- [ ] **Step 5: Rewrite `handle_worktree_request` as a thin wrapper.** Replace the whole `fn handle_worktree_request` (`~:5300-5394`) with a version that calls the core, applies the cwd, replies (turn path) or surfaces the error via `status_hint` (slash path), and writes NO other `status_hint` (WT-4):

```rust
fn handle_worktree_request(
    app: &mut App,
    action: zoid::agent::WorktreeAction,
    reply: Option<tokio::sync::oneshot::Sender<Result<std::path::PathBuf, String>>>,
) {
    let subagent_running = !app.in_flight_subagents.is_empty();
    let result = compute_worktree_switch(
        &mut app.active_worktree,
        action,
        subagent_running,
        std::path::Path::new("."),
    );
    match &result {
        Ok(cwd) => {
            // WT-2: the Session drawer's cwd display (not clobbered by the poller).
            app.shell.cwd = cwd.display().to_string();
        }
        Err(msg) => {
            // The `:worktree` slash path (reply == None) has no ToolResult to carry
            // the error, so surface it here. The turn path gets the error via its
            // ToolResult and does not need a status_hint.
            if reply.is_none() {
                app.shell.status_hint = Some(msg.clone());
            }
        }
    }
    // Task 2 inserts the poller-cell update here (after the match, before reply).
    if let Some(reply) = reply {
        let _ = reply.send(result);
    }
}
```

> The ~10 worktree `status_hint` confirmation/guard writes that were here are GONE (WT-4). The single `Err`-path `status_hint` above is the ONLY worktree hint that remains, and only for the slash path. Do not touch any `status_hint` write elsewhere in `main.rs`.

- [ ] **Step 6: Add the `reply` field to the `AgentUpdate::WorktreeRequested` variant.** In `crates/zoid/src/agent.rs` (`~:369`):

```rust
    /// The agent (or user via `:worktree`) requested a worktree relocation.
    /// `reply` carries the new absolute cwd (or an error) back to the awaiting
    /// turn so its in-flight tool execution repoints atomically (WT-1/WT-2).
    WorktreeRequested {
        action: WorktreeAction,
        reply: tokio::sync::oneshot::Sender<Result<std::path::PathBuf, String>>,
    },
```

- [ ] **Step 7: Make `cwd_for_exec` mutable + the enter arm synchronous.** In `crates/zoid/src/agent.rs`, change `:1005`:

```rust
        let mut cwd_for_exec = config.cwd.clone();
```

Then replace the `enter_worktree` arm's body (`~:1557-1610`) — keep the empty-name guard, then do the request/response:

```rust
                    // Synchronous relocation: send a reply channel and await the
                    // main loop's new absolute cwd so THIS turn's subsequent tool
                    // calls run in the worktree (WT-1). No optimistic result.
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Enter { name: name.clone() },
                            reply: tx,
                        })
                        .await;
                    match rx.await {
                        Ok(Ok(new_cwd)) => {
                            cwd_for_exec = new_cwd;
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
                        }
                        other => {
                            let msg = match other {
                                Ok(Err(m)) => m,
                                _ => "worktree switch failed (no reply)".to_string(),
                            };
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: msg,
                                    is_error: true,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                    }
```

- [ ] **Step 8: Make the exit arm synchronous.** Replace the `exit_worktree` arm (`~:1611-1640`):

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "exit_worktree" => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Exit,
                            reply: tx,
                        })
                        .await;
                    match rx.await {
                        Ok(Ok(new_cwd)) => {
                            cwd_for_exec = new_cwd;
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: "exited worktree".into(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                        other => {
                            let msg = match other {
                                Ok(Err(m)) => m,
                                _ => "worktree exit failed (no reply)".to_string(),
                            };
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: msg,
                                    is_error: true,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                    }
                }
```

- [ ] **Step 9: Thread the reply through the main-loop dispatch.** In `crates/zoid/src/main.rs` (`~:2997`):

```rust
                    zoid::agent::AgentUpdate::WorktreeRequested { action, reply } => {
                        handle_worktree_request(app, action, Some(reply));
                    }
```

- [ ] **Step 10: Fix the `:worktree` slash-command callers.** In `crates/zoid/src/main.rs` (`Command::Worktree(name)` `~:5065`, `Command::WorktreeExit` `~:5077`), pass `None` (the wrapper sets `status_hint` on error):

```rust
        Command::Worktree(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint =
                    Some("usage: :worktree <name> · :worktree exit".into());
                return Ok(false);
            }
            handle_worktree_request(app, zoid::agent::WorktreeAction::Enter { name }, None);
            Ok(false)
        }
        Command::WorktreeExit => {
            handle_worktree_request(app, zoid::agent::WorktreeAction::Exit, None);
            Ok(false)
        }
```

- [ ] **Step 11: Run the crate + workspace tests.**

Run: `cargo test -p zoid`
Expected: PASS — `worktree_switch_tests` (6) green; existing turn/worktree tests green.

Run: `cargo test`
Expected: PASS across the workspace (the `AgentUpdate::WorktreeRequested` field addition compiles everywhere — grep `WorktreeRequested` to confirm only the two construction sites + one match arm exist, all updated).

- [ ] **Step 12: Commit.**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "fix(worktree): synchronous enter/exit switch — commits land on worktree branch, exit keeps tooling alive (WT-1/WT-2), drop redundant hints (WT-4)"
```

---

### Task 2: Worktree-aware, change-driven git poller (WT-3)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `App` struct (add `active_wt_tx`) + both constructors; the git-poller `watch` setup + task (`~:2210-2248`); `handle_worktree_request` (add the cell update on `Ok`); add `git_status_at` + `current_branch_at`.

**Interfaces:**
- Consumes: `handle_worktree_request` / `App.active_worktree` (Task 1); existing `worktree_label(&git2::Repository)` (`~:1144`), `parse_numstat`.
- Produces: `App.active_wt_tx: tokio::sync::watch::Sender<Option<std::path::PathBuf>>`; `fn git_status_at(dir: &std::path::Path) -> (usize, usize, usize)`; `fn current_branch_at(root: &std::path::Path) -> String`.

- [ ] **Step 1: Write the failing test.** Add to the inline `mod worktree_switch_tests` in `main.rs`:

```rust
    #[test]
    fn git_status_at_reads_the_given_dir_not_cwd() {
        use super::git_status_at;
        let repo = init_repo();
        std::fs::write(repo.path().join("f.txt"), "hi there changed").unwrap();
        let (_added, _removed, files) = git_status_at(repo.path());
        assert!(files >= 1, "git_status_at must see the change in the given dir: files={files}");
    }

    #[test]
    fn current_branch_at_reads_the_given_dir() {
        use super::current_branch_at;
        let repo = init_repo();
        let b = current_branch_at(repo.path());
        assert!(b == "main" || b == "master", "init default branch, got: {b}");
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid worktree_switch_tests`
Expected: compile error — `git_status_at` / `current_branch_at` undefined.

- [ ] **Step 3: Add `git_status_at` (and refactor `git_status` onto it) + `current_branch_at`.** In `crates/zoid/src/main.rs`, next to `fn git_status()` (`~:1192`):

```rust
/// Diff stats (added, removed, files) for the git checkout at `dir`.
fn git_status_at(dir: &std::path::Path) -> (usize, usize, usize) {
    let run = |args: &[&str]| -> String {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let (a1, r1, f1) = parse_numstat(&run(&["diff", "--numstat"]));
    let (a2, r2, f2) = parse_numstat(&run(&["diff", "--numstat", "--cached"]));
    (a1 + a2, r1 + r2, f1 + f2)
}
```

**Delete the now-dead `fn git_status()` (`~:1192`) and `fn current_branch()` (`~:1129`).** Their ONLY caller is the git poller, which Step 7 rewrites to call `git_status_at`/`current_branch_at` directly. Leaving them would emit `dead_code` warnings (grep to confirm no other caller: `grep -n "current_branch()\|git_status()" crates/zoid/src/main.rs` — expect only their definitions + the poller lines you're replacing). Remove both `fn` definitions.

Next to where `fn current_branch()` was (`~:1129`), add:

```rust
/// Current branch of the checkout at `root`. Reads `<root>/.git/HEAD` for a
/// normal checkout; falls back to git2 for a linked worktree (whose `.git` is a
/// gitdir-file, not a directory).
fn current_branch_at(root: &std::path::Path) -> String {
    if let Ok(s) = std::fs::read_to_string(root.join(".git").join("HEAD")) {
        if let Some(b) = s.trim().strip_prefix("ref: refs/heads/") {
            return b.to_string();
        }
    }
    git2::Repository::open(root)
        .ok()
        .and_then(|r| r.head().ok())
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| "main".into())
}
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p zoid worktree_switch_tests`
Expected: PASS — `git_status_at_...` and `current_branch_at_...` green.

- [ ] **Step 5: Add the active-worktree-path `watch` cell to `App` + init.** In `crates/zoid/src/main.rs`, in the `App` struct (grep `struct App`), add:

```rust
    /// Active worktree path the git poller should open (None = main checkout).
    /// Updated on enter/exit so the poller re-polls immediately (WT-3 immediate)
    /// and confirms rather than reverts the rail (WT-3 durable).
    active_wt_tx: tokio::sync::watch::Sender<Option<std::path::PathBuf>>,
```

In BOTH `App` constructors (the two sites Spec 1 touched for `in_flight`; grep `active_worktree: None` — both constructors init it near there), add:

```rust
        active_wt_tx: tokio::sync::watch::channel(None).0,
```

> `watch::channel(None).0` keeps only the sender; the poller gets a receiver via `subscribe()`. A `watch::Sender` keeps the channel alive with no receivers, so this is a valid struct field.

- [ ] **Step 6: Update the cell on enter/exit.** In `handle_worktree_request` (Task 1's wrapper), replace the `// Task 2 inserts...` comment with the cell update, computed from the just-mutated `app.active_worktree`:

```rust
    // Point the git poller at the worktree (enter) or back to "." (exit), and
    // wake it immediately so the rail updates without waiting for the next tick.
    let active_path = app.active_worktree.as_ref().map(|w| w.path.clone());
    let _ = app.active_wt_tx.send(active_path);
```

> `w.path` is a `WorktreeSession` FIELD (not a method). Place this after the `match &result { ... }` block and before the `reply` send.

- [ ] **Step 7: Make the poller worktree-aware + change-driven.** In `crates/zoid/src/main.rs`, replace the poller task (`~:2221-2248`) so it subscribes to the cell, opens the active path, and re-polls immediately when the cell changes (not only every 5s):

```rust
    if app.shell.drawer(zoid_tui::DrawerId::Repo).is_some() {
        let mut active_wt_rx = app.active_wt_tx.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                // Latest active-worktree path (main checkout when None).
                let dir = active_wt_rx
                    .borrow_and_update()
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let dir_status = dir.clone();
                let (added, removed, files) =
                    tokio::task::spawn_blocking(move || git_status_at(&dir_status))
                        .await
                        .unwrap_or((0, 0, 0));
                let dir_labels = dir.clone();
                let (branch, worktree) = tokio::task::spawn_blocking(move || {
                    let branch = current_branch_at(&dir_labels);
                    let worktree = git2::Repository::open(&dir_labels)
                        .ok()
                        .map(|r| worktree_label(&r))
                        .unwrap_or_else(|| "(none)".into());
                    (branch, worktree)
                })
                .await
                .unwrap_or_else(|_| ("main".into(), "(none)".into()));
                if git_tx
                    .send((added, removed, files, branch, worktree))
                    .is_err()
                {
                    break; // receiver dropped — app is exiting
                }
                // Wake on the 5 s tick OR an enter/exit (immediate re-poll).
                tokio::select! {
                    _ = tick.tick() => {}
                    changed = active_wt_rx.changed() => {
                        if changed.is_err() {
                            break; // sender dropped — app is exiting
                        }
                    }
                }
            }
        });
    }
```

> `active_wt_rx = app.active_wt_tx.subscribe()` is taken BEFORE `tokio::spawn` (it borrows `app`; the closure owns the receiver). The consumer side (`~:2256-2264`) is UNCHANGED — it still copies the tuple into `shell.{changes_*,branch,worktree}`; now the tuple is worktree-correct and refreshed within milliseconds of a switch, so it no longer reverts the rail.

- [ ] **Step 8: Run the crate + workspace tests.**

Run: `cargo test -p zoid`
Expected: PASS.

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 9: Commit.**

```bash
git add crates/zoid/src/main.rs
git commit -m "fix(worktree): worktree-aware, change-driven git poller so the rail reflects the active worktree (WT-3)"
```

---

## Final Verification

- [ ] Full workspace test suite:

Run: `cargo test`
Expected: PASS across all crates.

- [ ] Clean build:

Run: `cargo build -p zoid`
Expected: clean build.

- [ ] WT-4 audit — confirm the handler is `status_hint`-free except the one slash-path error surface:

Run: `grep -n "status_hint" crates/zoid/src/main.rs` and confirm the only write inside/near `handle_worktree_request` is the single `Err`-path line from Task 1 Step 5; all other producers unchanged.

---

## Self-Review Notes (author checklist — already applied; gilfoyle plan-review fixes folded in)

- **Spec coverage:** WT-1 (commit lands on worktree branch) + WT-2 (exit keeps tooling alive) → Task 1's `oneshot` switch reassigning `mut cwd_for_exec` + Exit returning the abs repo root computed before removal. WT-3 (rail reflects the worktree, immediately AND durably) → Task 2's change-driven worktree-aware poller. WT-4 → the ~10 handler `status_hint` writes removed (only a single slash-path error hint remains).
- **Plan-review fixes (gilfoyle, opus):** C1 — core takes `&mut Option<WorktreeSession>` (the real `App` field type), not `WorktreeGuard`. C2 — enter uses `create_worktree(...)?.into_kept()` to suppress `WorktreeGuard::Drop`; dirty exit does nothing (no `Drop` on `WorktreeSession` → no data loss); a test asserts the dirty dir survives. I1 — the "immediate" rail is delivered by the poller re-polling on the `active_wt_tx.changed()` signal (the render loop's per-frame copy of the poller tuple no longer fights a handler write, because the handler no longer writes `shell.branch`/`worktree` at all). I2 — the create-fails-→-enter-existing fallback is replicated in the core, with a re-enter test. M1 — the slash path surfaces the actual error via `status_hint` in the wrapper (covers ALL guard cases, including "already in a worktree"/"not in a worktree"). M2 — no `WorktreeGuard` accessors needed (core uses `WorktreeSession` fields).
- **Deviations (deliberate, confirmed correct by review):** the handler does NOT write `TurnConfig.cwd` (the next-turn cwd flows via `spawn_turn` reading `app.active_worktree` at `~:5440`, preserved; the same-turn fix is the `cwd_for_exec` reassignment). The `:worktree` slash path keeps ONE generic `status_hint` error surface (no `ToolResult` there).
- **Deadlock safety (confirmed by review):** the turn is a detached `tokio::spawn` (`main.rs:~5487`); the main loop drains `ui_rx.recv()` in its `select!` (`~:2667`); the handler replies on every path; a dropped receiver is caught by the `other =>` arm. The reply is bounded by synchronous git work.
- **Type consistency:** `compute_worktree_switch` → `Result<PathBuf, String>` (cwd only) is consumed identically by the handler (Ok→cwd, reply) and both agent arms. `active_wt_tx`/`git_status_at`/`current_branch_at` defined once, used consistently. `WorktreeRequested { action, reply }` shape matches sender (agent.rs) and receiver (main.rs).
- **No placeholders:** every code step is complete and compile-ready.
- **KNOWN GAP (for the final whole-branch review):** no automated test drives the live 5 s poller / `watch` `changed()` loop end-to-end, nor a true full-loop WT-1 integration test (FakeProvider emitting `enter_worktree` then a commit tool, asserting `git log` on the worktree branch). The switch VALUES + dirty-keep + re-enter are unit-tested (`worktree_switch_tests`) and the poller wiring is type-checked; a full-loop WT-1 integration test is the highest-value addition if the final review wants the end-to-end guarantee.
