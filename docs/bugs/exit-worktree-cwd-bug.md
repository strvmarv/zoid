## Bug: Shell tool fails with ENOENT after exit_worktree

### Symptom

After calling `exit_worktree` (which deletes the worktree directory), the next
`shell` tool call fails with `No such file or directory (os error 2)`. The
shell is unusable until the session is re-invoked (e.g., via `schedule_wake`,
which starts a fresh turn from a valid CWD).

### Root cause investigation (incomplete)

The `shell` tool (`crates/zoid-tools/src/shell.rs`) spawns a child process via
`Command::new("sh").current_dir(cwd)`. The `cwd` comes from `cwd_for_exec` in
the agent loop (`crates/zoid/src/agent.rs:1090`). When `cwd` points at a
deleted directory, `spawn()` fails with ENOENT.

The `exit_worktree` handler (`crates/zoid/src/main.rs:6187`) correctly returns
the repo root as `new_cwd`, and the agent loop sets `cwd_for_exec = new_cwd`
(agent.rs:1770). In the next turn, `config.cwd` is `PathBuf::from(".")`
(agent.rs:265), which resolves to the process's CWD (the main checkout). There
is no `set_current_dir` or `chdir` anywhere in the codebase. The process CWD
is immutable from startup.

Static analysis says the CWD should be correct at every step. But the shell
still fails. The actual `cwd` value passed to `Shell::run` after `exit_worktree`
has NOT been confirmed — a debug `eprintln` was added to `shell.rs` but the
binary couldn't be run to capture the output.

### What's been checked

1. `exit_worktree` returns `(repo_root, warn)` — `repo_root` is
   `std::fs::canonicalize(".")` which is the main checkout path. OK
2. Agent loop sets `cwd_for_exec = new_cwd` (the repo root). OK
3. Next turn: `turn_config.cwd = PathBuf::from(".")` (agent.rs:265), since
   `app.active_worktree` is `None` after exit. OK
4. `PathBuf::from(".")` resolves to the process CWD (the main checkout) when
   passed to `Command::current_dir()`. Verified with a standalone test. OK
5. No `set_current_dir`, `chdir`, or process CWD mutation anywhere in the
   codebase (grep confirmed). OK
6. `Usage` event is emitted before `TurnComplete` (agent.rs:993 vs :721). OK
7. The `WorktreeRequested` message is processed synchronously by the main
   loop before the agent loop continues. OK

### What's NOT been checked

- The actual `cwd` value that `Shell::run` receives after `exit_worktree`
  (requires running the binary with debug output).
- Whether `exit_worktree` and the failing `shell` call are in the same turn
  or different turns (affects which `cwd_for_exec` initialization path is
  taken).
- Whether the process CWD itself is somehow the worktree path (unlikely given
  no `set_current_dir`, but unconfirmed at runtime).

### Next steps

1. Run the binary with a debug eprintln in shell.rs to capture the actual
   cwd value when the failure occurs. The debug line was removed; re-add it
   in `Shell::run` (shell.rs line 44, before `self.spawn_and_wait`):

       eprintln!("[DEBUG shell] cwd={cwd:?} exists={}", cwd.exists());

2. Alternative: add a tracing::warn instead of eprintln so it appears in the
   zoid log:

       if !cwd.exists() {
           tracing::warn!(cwd = %cwd.display(), "shell cwd does not exist");
       }

3. Once the CWD value is confirmed, trace backward to find where it was set.
   If it's the worktree path, the `cwd_for_exec` update or the `config.cwd`
   initialization is the bug. If it's `PathBuf::from(".")` and the process
   CWD is the worktree, something is mutating the process CWD.

4. Defensive fix (regardless of root cause): make `Shell::run` fall back to
   the process CWD or `/` when `cwd` doesn't exist:

       let cwd = if cwd.exists() {
           cwd
       } else {
           std::env::current_dir()
               .unwrap_or_else(|_| std::path::PathBuf::from("/"))
       };

   This prevents the ENOENT crash but masks the root cause. A `tracing::warn`
   should accompany it so the bad CWD is logged for diagnosis.

### Root cause (found by parallel agent)

`cwd_for_exec` was initialized from `config.cwd` at the start of each
**sub-turn** (inside the `'turn` loop in `run_turn_inner`, agent.rs:1090),
not per-turn. When `exit_worktree` updated `cwd_for_exec` to the repo root
in one sub-turn, the next sub-turn iteration reset it via
`let mut cwd_for_exec = config.cwd.clone()` — which still held the old
worktree path from the turn's frozen `config` snapshot. The shell then
tried to `spawn()` with the deleted worktree as CWD → ENOENT.

Cross-turn CWD is handled correctly: `spawn_turn` rebuilds `config.cwd`
from `app.active_worktree` between turns, and after exit that's `None`
so `config.cwd` is `PathBuf::from(".")` which resolves to the process
CWD (the main checkout). The bug was only in the sub-turn scope.

### Fix

Move `cwd_for_exec` initialization from inside the `'turn` loop (per-sub-turn)
to before the loop (per-turn). The CWD update from `exit_worktree` now
survives the sub-turn boundary. Committed as `5315980`.

Includes integration test `worktree_wt2_exit_cwd.rs` verifying the CWD
survives `exit_worktree` across sub-turn boundaries.

### Key files

- `crates/zoid-tools/src/shell.rs` — `Shell::run` / `spawn_and_wait` (the
  failing `spawn()` call, line 83-91)
- `crates/zoid/src/agent.rs` — `cwd_for_exec` initialization (line 1090),
  `exit_worktree` handler (line 1768-1770)
- `crates/zoid/src/main.rs` — `compute_worktree_switch` Exit arm (line 6187),
  `spawn_turn` / `turn_config.cwd` (line 6524-6530), `handle_worktree_request`
  (line 6222)
- `crates/zoid/src/worktree.rs` — `remove_worktree` (line 105)