# Bug: exit_worktree deletes branch with unmerged commits when main advanced

### Status

Open — root cause found, fix not yet implemented.

### Symptom

`exit_worktree` reported "no unmerged commits detected" and deleted the
`subagent-dispatch-hardening` branch, which had 5 unmerged commits. The branch
ref was lost; the commits survived only as orphaned SHAs recoverable via
`git cat-file`. The tool result message was:

```
exited worktree (branch 'subagent-dispatch-hardening' deleted — no unmerged
commits detected; branch_oid=None head_oid=Some(816d686...))
```

The `branch_oid=None` in the message is from the post-deletion diagnostic
(line 7048-7051 of main.rs), which runs AFTER `remove_worktree` already deleted
the branch — a red herring. The real failure is in `branch_has_unmerged_commits`.

### Root cause

`branch_has_unmerged_commits` (`crates/zoid/src/worktree.rs:155`) checks for
unmerged work with:

```rust
repo.graph_descendant_of(branch_oid, head_oid).unwrap_or(false)
```

`graph_descendant_of(branch, head)` returns `true` only when `branch` is a
strict descendant of `head` — i.e., `head` is an ancestor of `branch`. This is
true only when the branch was created from the current HEAD and main has not
moved since.

When main advances while the worktree is active (new commits added to main,
fast-forward or otherwise), main's HEAD moves past the original branch point.
The branch and HEAD diverge — HEAD is no longer an ancestor of the branch —
so `graph_descendant_of` returns `false` even though the branch has commits
not reachable from HEAD. The branch is then incorrectly classified as "no
unmerged commits" and deleted.

### Reproduction

```bash
# 1. Create a worktree branched from HEAD
git worktree add .zoid/worktrees/test-branch -b test-branch
cd .zoid/worktrees/test-branch

# 2. Commit work on the branch
echo "work" > file.txt && git add -A && git commit -m "work"

# 3. From the main checkout, advance main past the branch point
cd /path/to/main
git commit --allow-empty -m "main moved"

# 4. Exit the worktree — the branch is deleted despite having unmerged work
#    (via exit_worktree or compute_worktree_switch)
```

### Evidence

The incident: `enter_worktree` created `subagent-dispatch-hardening` from
HEAD `b30a9c5`. 5 commits were added to the branch (`0264970`..`f14e03e`).
Meanwhile, main advanced 8 commits to `816d686` (the AGENTS.md + logging +
clippy + TUI work committed during the same session).

- `merge-base(816d686, f14e03e)` = `b30a9c5` (the original branch point)
- `branch_tip` = `f14e03e`
- `merge-base != branch_tip` → 5 commits in the branch not reachable from HEAD
- But `graph_descendant_of(f14e03e, 816d686)` = `false` because `816d686` is
  NOT an ancestor of `f14e03e` (they diverged at `b30a9c5`)
- `has_unmerged = false` → `remove_worktree(..., delete_branch=true)` → branch deleted

### The fix

Replace `graph_descendant_of(branch_oid, head_oid)` with a merge-base check:

```rust
// The branch has unmerged commits if its tip is NOT reachable from HEAD.
// merge_base(branch, head) == branch means all branch commits are in HEAD
// (no unmerged work). merge_base != branch means the branch has commits
// not reachable from HEAD (unmerged work). This correctly handles the case
// where main advanced while the worktree was active — the branch and HEAD
// diverged, but the branch still has commits HEAD doesn't.
let merge_base = repo.merge_base(branch_oid, head_oid).unwrap_or(branch_oid);
merge_base != branch_oid
```

`graph_descendant_of(branch, head)` is a special case of this check that only
works when HEAD hasn't moved since the branch was created. The merge-base check
is strictly more general and handles both cases: HEAD behind the branch
(original case) and HEAD diverged from the branch (the bug).

### Files to fix

- `crates/zoid/src/worktree.rs:155` — replace `graph_descendant_of` with
  `merge_base` check
- `crates/zoid/src/worktree.rs:123-156` — update the doc comment to describe
  the merge-base approach instead of the descendant-of approach
- `crates/zoid/src/main.rs:7044-7061` — the post-deletion diagnostic captures
  `branch_oid` AFTER the branch is already deleted (by `remove_worktree` at
  line 7034), so it's always `None`. Move the OID capture BEFORE
  `remove_worktree`, or capture it inside `branch_has_unmerged_commits` and
  return it alongside the bool.

### Tests to add

- `branch_has_unmerged_commits` returns `true` when main advanced past the
  branch point (the exact scenario that failed)
- `branch_has_unmerged_commits` returns `true` when the branch is strictly
  ahead of HEAD (the original working case)
- `branch_has_unmerged_commits` returns `false` when the branch tip is
  reachable from HEAD (fully merged)

### Timeline

- Discovered during the subagent-dispatch-hardening SDD execution
  (2026-08-04). The worktree was created from `b30a9c5`, 5 commits were added,
  main advanced to `816d686` during the same session, and `exit_worktree`
  deleted the branch. The commits were recovered by SHA and merged manually.