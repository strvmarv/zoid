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
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
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
        assert!(
            path.join("a.txt").exists(),
            "HEAD content should be checked out"
        );
    } // WorktreeGuard dropped here

    assert!(!path.exists(), "worktree dir removed on drop");

    // After drop: the worktree's branch ref is also cleaned up (no leak).
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch("sub-ax3", git2::BranchType::Local)
            .is_err(),
        "worktree branch removed on drop"
    );
}

#[test]
fn into_kept_branch_removes_dir_but_retains_branch() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let path;
    {
        let wt = create_worktree(tmp.path(), "sub-keep1").unwrap();
        path = wt.path().to_path_buf();
        // Commit something so the branch has a commit to retain.
        std::fs::write(path.join("new.txt"), "data").unwrap();
        let _ = std::process::Command::new("git")
            .args(["-C"])
            .arg(&path)
            .args(["add", "-A"])
            .status();
        let _ = std::process::Command::new("git")
            .args(["-C"])
            .arg(&path)
            .args(["commit", "-m", "test"])
            .status();
        let (_kept_path, _name) = wt.into_kept_branch();
    } // guard consumed, NOT dropped

    assert!(!path.exists(), "worktree dir must be removed");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch("sub-keep1", git2::BranchType::Local).is_ok(),
        "branch must survive into_kept_branch"
    );
    // Cleanup: delete the retained branch manually.
    let _ = repo
        .find_branch("sub-keep1", git2::BranchType::Local)
        .unwrap()
        .delete();
}

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
    zoid::worktree::remove_worktree(tmp.path(), "wt-keep1", true).unwrap();
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
    zoid::worktree::remove_worktree(tmp.path(), &kept_name, true).unwrap();

    assert!(!path.exists(), "dir removed by remove_worktree");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch("wt-rm1", git2::BranchType::Local).is_err(),
        "branch removed by remove_worktree"
    );
}

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
    zoid::worktree::remove_worktree(tmp.path(), &name, true).unwrap();

    assert!(!path.exists(), "worktree dir removed on exit");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch(&name, git2::BranchType::Local).is_err(),
        "branch removed on exit"
    );
}

/// Regression: `branch_has_unmerged_commits` must detect unmerged commits even
/// when called from inside the worktree (where `repo.head()` would otherwise
/// return the worktree's own HEAD — the branch being exited — making
/// `graph_descendant_of(branch, branch)` falsely return false). The process cwd
/// is inside the worktree at `exit_worktree` time, so `repo_root` resolves to the
/// worktree dir. The fix resolves the main checkout's HEAD via the common git dir.
#[test]
fn branch_has_unmerged_commits_detects_from_inside_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let wt = create_worktree(tmp.path(), "unmerged-from-wt").unwrap();
    let wt_path = wt.path().to_path_buf();

    // Commit on the worktree branch so it's ahead of main's HEAD.
    std::fs::write(wt_path.join("new.txt"), "data").unwrap();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["add", "-A"])
        .status();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["commit", "-q", "-m", "unmerged work"])
        .status();

    let (_kept_path, name) = wt.into_kept();

    // Call with repo_root = the WORKTREE path (simulating process cwd inside it).
    // The branch has an unmerged commit — must return true.
    assert!(
        zoid::worktree::branch_has_unmerged_commits(&wt_path, &name),
        "branch_has_unmerged_commits must detect unmerged commits even when \
         called from inside the worktree (repo.head() would otherwise return \
         the worktree's own HEAD, not main's)"
    );

    // Sanity: calling from the main checkout (the original, working path) also
    // returns true.
    assert!(
        zoid::worktree::branch_has_unmerged_commits(tmp.path(), &name),
        "branch_has_unmerged_commits must detect unmerged commits from the \
         main checkout too"
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
    let existing_path = tmp
        .path()
        .join(".zoid")
        .join("worktrees")
        .join("collide-1");
    assert!(existing_path.exists(), "existing worktree found on collision");

    // The handler would set active_worktree to the existing path.
    // No error, no duplicate created.
    let repo = git2::Repository::open(tmp.path()).unwrap();
    let worktrees = repo.worktrees().unwrap();
    assert!(
        worktrees
            .iter()
            .filter(|n| n.map(|s| s == "collide-1").unwrap_or(false))
            .count()
            == 1,
        "exactly one worktree registration (no duplicate)"
    );

    // Clean up.
    zoid::worktree::remove_worktree(tmp.path(), &name1, true).unwrap();
}

/// Regression: `remove_worktree` must correctly remove the worktree directory
/// and (optionally) the branch even when called with the **worktree path** as
/// `repo_root` — which is what happens in production when the process cwd is
/// inside the worktree at `exit_worktree` time and `handle_worktree_request`
/// passes `Path::new(".")` as `repo_root`.
///
/// Before this test, `remove_worktree` used `repo_root.join(".zoid")...` for
/// the directory path, which resolved inside the worktree (wrong) and
/// `find_worktree` + `prune` on a worktree-opened repo didn't work correctly.
/// The fix resolves the main checkout root before calling remove_worktree.
#[test]
fn remove_worktree_from_inside_worktree_path() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Create worktree with an unmerged commit.
    let wt = create_worktree(tmp.path(), "inside-wt-remove").unwrap();
    let (wt_path, name) = wt.into_kept();
    let wt_path = std::fs::canonicalize(&wt_path).unwrap_or(wt_path.clone());

    // Commit on the worktree branch.
    std::fs::write(wt_path.join("new.txt"), "data").unwrap();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["add", "-A"])
        .status();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["commit", "-q", "-m", "unmerged work"])
        .status();

    // Verify unmerged detection works from the worktree path.
    assert!(
        zoid::worktree::branch_has_unmerged_commits(&wt_path, &name),
        "must detect unmerged commits from worktree path"
    );

    // Call remove_worktree with the WORKTREE path as repo_root (delete_branch=false
    // because has_unmerged=true — same as production code path).
    zoid::worktree::remove_worktree(&wt_path, &name, false).unwrap();

    // The worktree directory must be removed.
    assert!(
        !wt_path.exists(),
        "worktree dir must be removed even when repo_root is the worktree path"
    );

    // The branch must be retained (delete_branch=false).
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch(&name, git2::BranchType::Local).is_ok(),
        "branch must be retained when delete_branch=false (unmerged commits)"
    );

    // Now clean up: remove with the main checkout path and delete_branch=true.
    zoid::worktree::remove_worktree(tmp.path(), &name, true).unwrap();
    assert!(
        repo.find_branch(&name, git2::BranchType::Local).is_err(),
        "branch must be deleted when delete_branch=true from main checkout"
    );
}

/// Regression: simulates the exact production scenario — enter worktree, commit
/// from inside it (as subagents do), exit_worktree. The branch must be retained
/// because it has unmerged commits. If `branch_has_unmerged_commits` returns false
/// here, the branch is silently deleted on exit.
#[test]
fn exit_worktree_retains_branch_with_unmerged_commits() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Create worktree (simulates enter_worktree → create_worktree → into_kept).
    let wt = create_worktree(tmp.path(), "retain-test").unwrap();
    let (wt_path, name) = wt.into_kept();
    let wt_path = std::fs::canonicalize(&wt_path).unwrap_or(wt_path.clone());

    // Commit from inside the worktree (simulates subagent work).
    std::fs::write(wt_path.join("work.txt"), "done").unwrap();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["add", "-A"])
        .status();
    let _ = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["commit", "-q", "-m", "subagent work"])
        .status();

    // Verify the worktree is clean (all committed).
    let clean = std::process::Command::new("git")
        .args(["-C"])
        .arg(&wt_path)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap();
    assert!(clean, "worktree must be clean after commit");

    // Simulate exit_worktree: check has_unmerged from the MAIN checkout (repo_root=".").
    let has_unmerged = zoid::worktree::branch_has_unmerged_commits(tmp.path(), &name);
    assert!(
        has_unmerged,
        "branch_has_unmerged_commits must return true — the branch has a commit not on main HEAD"
    );

    // The production code path: delete_branch = !has_unmerged = false → branch retained.
    zoid::worktree::remove_worktree(tmp.path(), &name, !has_unmerged);

    // Verify: worktree dir removed, branch retained.
    assert!(!wt_path.exists(), "worktree dir removed");
    let repo = git2::Repository::open(tmp.path()).unwrap();
    assert!(
        repo.find_branch(&name, git2::BranchType::Local).is_ok(),
        "branch must be retained — it has unmerged commits"
    );
}
