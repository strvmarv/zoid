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

    /// The worktree's branch name.
    pub fn name(&self) -> &str {
        &self.name
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

    /// Consume the guard WITHOUT removing anything — the worktree dir AND
    /// the branch persist on disk. Used by `WorktreeSession` to take
    /// ownership of a persistent (Chat-agent) worktree: the guard is moved
    /// into session state and held until an explicit exit decision, so its
    /// `Drop` never fires while the session owns it.
    /// Returns (path, branch_name) for the session to remember.
    pub fn into_kept(self) -> (PathBuf, String) {
        let path = self.path.clone();
        let name = self.name.clone();
        std::mem::forget(self); // suppress Drop's removal entirely
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

/// Explicitly remove a worktree by name: remove the dir, prune the
/// registration, and delete the branch. This is the same logic `Drop`
/// performs, factored out so the Chat-agent exit path can call it directly
/// on a worktree that was previously `into_kept()`'d.
pub fn remove_worktree(repo_root: &Path, name: &str, delete_branch: bool) -> Result<()> {
    let path = repo_root.join(".zoid").join("worktrees").join(name);
    let _ = std::fs::remove_dir_all(&path);
    if let Ok(repo) = Repository::open(repo_root) {
        if let Ok(wt) = repo.find_worktree(name) {
            let mut po = WorktreePruneOptions::new();
            po.valid(true).working_tree(true);
            let _ = wt.prune(Some(&mut po));
        }
        if delete_branch {
            if let Ok(mut branch) = repo.find_branch(name, git2::BranchType::Local) {
                let _ = branch.delete();
            }
        }
    }
    Ok(())
}

/// Whether the worktree branch has commits not reachable from the main
/// checkout's HEAD (i.e., unmerged work). Returns `false` if the branch doesn't
/// exist or git operations fail (conservative — don't block cleanup on errors).
///
/// **Important:** `repo_root` may be the *worktree* path (the process cwd is
/// inside the worktree at `exit_worktree` time). A naive `repo.head()` on the
/// worktree repo returns the worktree's own HEAD — the branch being exited —
/// so checking the branch against itself would always report "merged." To
/// compare against the *main* checkout's HEAD, we open the common git dir
/// (`repo.commondir()`) and read HEAD from there.
///
/// **Merge-base check, not descendant-of:** The branch has unmerged work if
/// its tip is NOT reachable from HEAD — i.e., `merge_base(branch, head) !=
/// branch`. The previous check (`graph_descendant_of(branch, head)`) only
/// returned `true` when the branch was a strict descendant of HEAD (HEAD is
/// an ancestor of the branch). When main advances while the worktree is
/// active, HEAD moves past the branch point and the two diverge — HEAD is no
/// longer an ancestor of the branch, so `graph_descendant_of` returns `false`
/// even though the branch still has commits HEAD doesn't. The merge-base
/// check handles both cases: HEAD behind the branch (original) and HEAD
/// diverged from the branch (the bug this fixed).
pub fn branch_has_unmerged_commits(repo_root: &Path, name: &str) -> bool {
    let Ok(repo) = Repository::open(repo_root) else {
        return false;
    };
    let Ok(branch) = repo.find_branch(name, git2::BranchType::Local) else {
        return false;
    };
    let Some(branch_oid) = branch.get().target() else {
        return false;
    };
    // Resolve the main checkout's HEAD, not the worktree's. When `repo_root`
    // is the main checkout, `commondir() == repo.path()` and this is a no-op
    // (same repo). When `repo_root` is a worktree, `commondir()` points at the
    // common git dir; opening it yields a repo whose `head()` is the main
    // checkout's HEAD, not the worktree's.
    let head_oid = match main_head_oid(&repo) {
        Some(oid) => oid,
        None => return false,
    };
    // The branch has unmerged commits if its tip is NOT reachable from HEAD.
    // merge_base(branch, head) == branch means all branch commits are in HEAD
    // (no unmerged work). merge_base != branch means the branch has commits
    // not reachable from HEAD (unmerged work). This correctly handles the case
    // where main advanced while the worktree was active — the branch and HEAD
    // diverged, but the branch still has commits HEAD doesn't.
    let merge_base = repo.merge_base(branch_oid, head_oid).unwrap_or(head_oid);
    merge_base != branch_oid
}

/// Resolve the main checkout's HEAD OID from a repo opened on either the main
/// checkout or a linked worktree. Opens the common git dir (shared by all
/// worktrees of a repo) so `head()` returns the main checkout's HEAD, not the
/// worktree's. Returns `None` if the common dir can't be opened or has no HEAD.
fn main_head_oid(repo: &Repository) -> Option<git2::Oid> {
    // On the main checkout, `commondir()` equals `repo.path()` — opening it
    // again is harmless (same repo, same HEAD). On a linked worktree,
    // `commondir()` is the shared `<repo>/.git`, whose HEAD is the main
    // checkout's.
    let common = repo.commondir();
    let common_repo = Repository::open(common).ok()?;
    let head = common_repo.head().ok()?;
    head.target()
}
