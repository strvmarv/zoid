//! Isolated git worktrees for subagent execution (spec §3/§4.4). A dispatched
//! Chat subagent runs in its own worktree so its file edits are isolated from
//! the main working copy until judged. A `WorktreeGuard` removes its worktree on
//! drop, so a panicking or abandoned subagent never leaks a registration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Repository, WorktreeAddOptions, WorktreePruneOptions};

/// An isolated worktree. Dropping it removes the working directory and prunes the
/// git registration (best-effort).
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

    /// Remove the worktree dir + prune the registration. Does NOT delete the
    /// branch ref. Shared by `Drop` (then deletes the branch) and
    /// `into_kept_branch` (keeps the branch).
    fn prune_dir(&self) {
        let _ = std::fs::remove_dir_all(&self.path);
        if let Ok(repo) = Repository::open(&self.repo_root) {
            if let Ok(wt) = repo.find_worktree(&self.name) {
                let mut po = WorktreePruneOptions::new();
                po.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut po));
            }
        }
    }

    /// Consume the guard, removing the worktree DIR but KEEPING the branch
    /// (with the subagent's commits) so `subagent_diff` can retrieve it.
    /// Returns (path, branch_name) for the caller. Suppresses `Drop`.
    pub fn into_kept_branch(self) -> (PathBuf, String) {
        let path = self.path.clone();
        let name = self.name.clone();
        self.prune_dir();
        std::mem::forget(self);
        (path, name)
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
    Ok(WorktreeGuard {
        name: name.to_string(),
        path,
        repo_root: repo_root.to_path_buf(),
    })
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        self.prune_dir();
        // The worktree add created a local branch named after the worktree;
        // prune leaves it behind. Delete it too so delegations don't
        // accumulate refs.
        if let Ok(repo) = Repository::open(&self.repo_root) {
            if let Ok(mut branch) = repo.find_branch(&self.name, git2::BranchType::Local) {
                let _ = branch.delete();
            }
        }
    }
}
