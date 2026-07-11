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
