# Hard-Stop Interrupt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Esc` a two-tier interrupt — first press abandons network waits (streaming + MCP calls); second press force-kills a running local tool, including the OS process tree of a `shell` command — without ever leaving an unbalanced `tool_use`/`tool_result` pair.

**Architecture:** A shared `KillSlot` (an `Arc<Mutex<Option<pgid>>>`) is published by the `shell` tool (spawned in its own process group) and read by the agent loop, which SIGKILLs the whole group on a *hard* cancel. The single per-turn `CancellationToken` becomes two — `graceful` and `hard` — threaded through `run_agent_turn_cancellable`. The Local-tool call site gains a `select!` on `hard` (→ kill + `[killed: hard-stop]`); the MCP call site gains a `select!` on `graceful` (→ abandon + `[skipped: turn aborted]`). Escalation lives entirely in the `main.rs` `CancelTurn` handler; no new keybinding.

**Tech Stack:** Rust, tokio (`spawn_blocking`, `tokio::select!`), `tokio_util::sync::CancellationToken`, `nix` 0.29 (`signal` feature, for `killpg`), `std::os::unix::process::CommandExt::process_group`.

## Global Constraints

- env values are NEVER logged (they may carry secrets).
- No `Co-Authored-By` / co-author trailer on commit messages.
- The `Tool` trait signature (`fn run(&self, args: &Value, cwd: &Path) -> ToolOutput`) MUST NOT change.
- Provider message history must never contain an unbalanced `tool_use`/`tool_result` pair after an interrupt — every started tool call gets a synthesized result (`[killed: hard-stop]` or `[skipped: turn aborted]`).
- `KillSlot::kill()` is best-effort and panic-free (no `unwrap` on a possibly-exited process; ignore `ESRCH`).
- Only `shell` gets true child-kill; other blocking local tools get abandon-wait. No configurable escalation window, no third tier.
- Unix is the primary target; non-Unix keeps the current `.output()` behavior and a no-op `kill()`.

## Working directory

All tasks run in the SDD worktree created off `main` (which already carries the spec `docs/superpowers/specs/2026-07-08-hard-stop-interrupt-design.md`). Prefix every command with the worktree root and assert it before committing:
`git rev-parse --show-toplevel` must print the worktree path, NOT `/home/gomanjoe/source/zoid`.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tools/Cargo.toml` | deps | add unix-only `nix` (signal) |
| `crates/zoid-tools/src/kill.rs` | **new** — `KillSlot` type + process-group SIGKILL | create |
| `crates/zoid-tools/src/lib.rs` | tool registry + exports | export `KillSlot`, add `registry_with_kill` |
| `crates/zoid-tools/src/shell.rs` | shell tool | hold `KillSlot`, spawn in process group, register/clear pgid |
| `crates/zoid/src/agent.rs` | agent loop | `TurnConfig.kill` field; `hard` token param; Local call-site child-kill; MCP graceful abandon |
| `crates/zoid/src/subagent.rs` | subagent turn config | set `kill: KillSlot::new()` |
| `crates/zoid/src/invoke_skill.rs` | chat tool list | thread `KillSlot` into `chat_tools` → `registry_with_kill` |
| `crates/zoid/src/main.rs` | UI wiring | two tokens on `App`; build+share `KillSlot`; escalation handler; status text |

---

## Task 1: `KillSlot` + killable shell (zoid-tools)

**Files:**
- Modify: `crates/zoid-tools/Cargo.toml`
- Create: `crates/zoid-tools/src/kill.rs`
- Modify: `crates/zoid-tools/src/lib.rs`
- Modify: `crates/zoid-tools/src/shell.rs`

**Interfaces:**
- Produces:
  - `zoid_tools::KillSlot` — `#[derive(Clone, Default)]`; `KillSlot::new() -> KillSlot`, `fn register(&self, pgid: u32)`, `fn clear(&self)`, `fn pgid(&self) -> Option<u32>`, `fn kill(&self)` (unix: `killpg(pgid, SIGKILL)`, ignore errors; non-unix: no-op).
  - `zoid_tools::shell::Shell` — now `#[derive(Default)]` struct holding a `KillSlot`; `Shell::new(kill: KillSlot) -> Shell`.
  - `zoid_tools::registry_with_kill(kill: KillSlot) -> Vec<Box<dyn Tool>>` — same set as `registry()` but the `shell` tool carries `kill`.
  - `zoid_tools::registry()` — unchanged public signature; internally builds `Shell::default()`.

- [ ] **Step 1: Add the `nix` dependency (unix-only)**

In `crates/zoid-tools/Cargo.toml`, under `[dependencies]` add a target-scoped block after the existing deps:

```toml
[target.'cfg(unix)'.dependencies]
nix = { version = "0.29", default-features = false, features = ["signal"] }
```

- [ ] **Step 2: Write the failing test for `KillSlot` register/pgid/clear**

Create `crates/zoid-tools/src/kill.rs` with ONLY this test module first (no type yet), so the build fails on the missing type:

```rust
//! `KillSlot` — a single-slot registry the `shell` tool publishes its running
//! child's process-group id into, so the async agent loop can SIGKILL the whole
//! group on a hard-stop. One slot suffices: Local tools in a batch run
//! sequentially, so at most one shell child exists at a time.

use std::sync::{Arc, Mutex};

/// Shared handle to the pgid of the currently-running killable child, if any.
#[derive(Clone, Default)]
pub struct KillSlot(Arc<Mutex<Option<u32>>>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_pgid_then_clear() {
        let slot = KillSlot::new();
        assert_eq!(slot.pgid(), None);
        slot.register(4242);
        assert_eq!(slot.pgid(), Some(4242));
        slot.clear();
        assert_eq!(slot.pgid(), None);
    }

    #[test]
    fn kill_on_empty_slot_is_noop() {
        let slot = KillSlot::new();
        slot.kill(); // must not panic when nothing is registered
        assert_eq!(slot.pgid(), None);
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p zoid-tools kill::`
Expected: FAIL — `no function or associated item named 'new'` / `no method 'register'`.

- [ ] **Step 4: Implement `KillSlot`**

Add these `impl` blocks to `crates/zoid-tools/src/kill.rs` (above the test module):

```rust
impl KillSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the pgid of a freshly-spawned killable child.
    pub fn register(&self, pgid: u32) {
        *self.0.lock().unwrap() = Some(pgid);
    }

    /// Forget the child (called when it exits normally).
    pub fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }

    /// The currently-registered pgid, if any (test/introspection).
    pub fn pgid(&self) -> Option<u32> {
        *self.0.lock().unwrap()
    }

    /// Best-effort SIGKILL of the registered process group. No-op when empty.
    /// Ignores errors (e.g. the group already exited — `ESRCH`).
    #[cfg(unix)]
    pub fn kill(&self) {
        if let Some(pgid) = *self.0.lock().unwrap() {
            use nix::sys::signal::{killpg, Signal};
            use nix::unistd::Pid;
            let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGKILL);
        }
    }

    #[cfg(not(unix))]
    pub fn kill(&self) {
        // No process-group signalling on non-unix; hard-stop degrades to
        // abandon-wait for the shell tool.
    }
}
```

- [ ] **Step 5: Wire the module into the crate and run the test**

In `crates/zoid-tools/src/lib.rs`, add `pub mod kill;` near the other `pub mod` lines and re-export the type below the `ToolOutput` definition:

```rust
pub use kill::KillSlot;
```

Run: `cargo test -p zoid-tools kill::`
Expected: PASS (2 tests).

- [ ] **Step 6: Write the failing process-group kill test in `shell.rs`**

Add to the `tests` module in `crates/zoid-tools/src/shell.rs`:

```rust
    #[cfg(unix)]
    #[test]
    fn hard_kill_terminates_process_group_including_grandchildren() {
        use crate::KillSlot;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("SENTINEL");
        let kill = KillSlot::new();
        let shell = Shell::new(kill.clone());
        // A backgrounded grandchild that would write the sentinel after 3s,
        // and a parent that also sleeps. Under non-interactive `sh` the
        // background job shares the shell's process group, so a group-kill
        // must stop the grandchild before it can touch the sentinel.
        let cmd = format!("(sleep 3; touch {}) & sleep 3", sentinel.display());
        let dir_path = dir.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            shell.run(&serde_json::json!({ "command": cmd }), &dir_path)
        });
        // Wait until the child has registered its pgid, then kill the group.
        let mut waited = 0;
        while kill.pgid().is_none() && waited < 2000 {
            std::thread::sleep(Duration::from_millis(10));
            waited += 10;
        }
        assert!(kill.pgid().is_some(), "shell must register its pgid");
        kill.kill();
        let _ = handle.join().unwrap(); // wait must return promptly post-kill
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !sentinel.exists(),
            "group kill must prevent the grandchild from writing the sentinel"
        );
        // Slot is cleared once run() returns.
        assert_eq!(kill.pgid(), None);
    }
```

- [ ] **Step 7: Run it to verify it fails**

Run: `cargo test -p zoid-tools shell::tests::hard_kill_terminates_process_group`
Expected: FAIL — `Shell::new` doesn't exist / `Shell` is a unit struct.

- [ ] **Step 8: Rework `Shell` to be killable**

Replace the top of `crates/zoid-tools/src/shell.rs` (the `use`s and the `Shell` struct through the end of `run`) with:

```rust
use crate::{str_arg, KillSlot, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use zoid_provider::ToolSpec;

/// Run a shell command in the working directory and capture its output.
/// (Chat is safe by human presence, spec §9 — no sandbox.)
///
/// On unix the child is spawned in its own process group and its pgid is
/// published to a shared [`KillSlot`], so a hard-stop can SIGKILL the whole
/// tree (the shell plus any grandchildren it spawned).
#[derive(Default)]
pub struct Shell {
    kill: KillSlot,
}

impl Shell {
    pub fn new(kill: KillSlot) -> Self {
        Self { kill }
    }
}

impl Tool for Shell {
    fn name(&self) -> &str {
        "shell"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Run a shell command in the working directory; returns stdout, stderr, and exit code.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "command": { "type": "string", "description": "Command line to execute." } },
                "required": ["command"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let command = match str_arg(args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };

        let output = self.spawn_and_wait(&command, cwd);
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                let code = o.status.code().unwrap_or(-1);
                let mut text = String::new();
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&format!("[exit {code}]"));
                ToolOutput {
                    text,
                    is_error: code != 0,
                }
            }
            Err(e) => ToolOutput::err(format!("shell({command}): {e}")),
        }
    }
}

impl Shell {
    /// Unix: spawn in a fresh process group, publish the pgid to the kill slot,
    /// wait for output, then clear the slot. Non-unix: the previous
    /// fire-and-collect behavior (no killability).
    #[cfg(unix)]
    fn spawn_and_wait(&self, command: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0) // child's pid becomes its pgid
            .spawn()?;
        self.kill.register(child.id());
        // Take the pipes so wait_with_output isn't needed (it consumes child);
        // read to end after wait via the standard helper.
        let out = child.wait_with_output();
        self.kill.clear();
        out
    }

    #[cfg(not(unix))]
    fn spawn_and_wait(&self, command: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
        Command::new("cmd")
            .arg("/C")
            .arg(command)
            .current_dir(cwd)
            .output()
    }
}
```

Note: `wait_with_output()` consumes `child`, but `child.id()` is read first, so the pgid is captured before the move.

- [ ] **Step 9: Fix the existing unit-struct call sites**

In `crates/zoid-tools/src/shell.rs` tests, the four existing tests call `Shell.run(...)` on a unit value — change each `Shell.run(` to `Shell::default().run(`.

In `crates/zoid-tools/src/lib.rs`, `registry()` builds `Box::new(shell::Shell)`. Change it to `Box::new(shell::Shell::default())`, and add a parallel constructor right after `registry()`:

```rust
/// Like [`registry`] but the `shell` tool carries a shared [`KillSlot`] so a
/// hard-stop can kill its process group. Used by the chat turn; subagents and
/// tests use the zero-arg `registry()` (their shell is not hard-killable).
pub fn registry_with_kill(kill: KillSlot) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::ReadFile),
        Box::new(write::WriteFile),
        Box::new(edit::EditFile),
        Box::new(search::Search),
        Box::new(shell::Shell::new(kill)),
        Box::new(tasks::UpdateTasks),
        Box::new(ask::AskUser),
    ]
}
```

- [ ] **Step 10: Run the full zoid-tools suite**

Run: `cargo test -p zoid-tools`
Expected: PASS — all prior tests plus the two `kill::` tests and the process-group kill test. (The kill test takes ~1s.)

- [ ] **Step 11: Commit**

```bash
git add crates/zoid-tools/
git commit -m "feat(tools): KillSlot + process-group-killable shell tool"
```

---

## Task 2: Two-tier tokens + child-kill at the Local call site (agent.rs)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`TurnConfig`, `chat_turn_config_with`, `run_agent_turn`, `run_agent_turn_cancellable`, `run_turn_inner`, Local call site ~1218-1225)
- Modify: `crates/zoid/src/subagent.rs` (`TurnConfig` literal ~150)

**Interfaces:**
- Consumes: `zoid_tools::KillSlot` (Task 1).
- Produces:
  - `TurnConfig` gains `pub kill: zoid_tools::KillSlot` (defaults to a fresh slot; the chat turn overrides it in Task 4).
  - `run_agent_turn_cancellable(...)` gains a trailing `hard: CancellationToken` parameter (after the existing `cancel: CancellationToken`).
  - `run_agent_turn(...)` unchanged public arity; internally passes `CancellationToken::new()` for `hard`.
  - Behavior: when `hard` fires while a Local tool is executing, the loop kills `config.kill`'s process group, records a `[killed: hard-stop]` result for that call, drains the rest of the batch with `[skipped: turn aborted]`, and ends the turn.

- [ ] **Step 1: Add the `kill` field to `TurnConfig` and its constructors**

In `crates/zoid/src/agent.rs`, add to `struct TurnConfig` (after the `thinking` field, ~line 65):

```rust
    /// Shared kill slot for the `shell` tool's process group. A hard-stop
    /// SIGKILLs whatever pgid the running shell published here. Defaults to a
    /// fresh (unshared) slot for subagents/tests; the chat turn shares the same
    /// slot given to the chat tool list (see spawn_turn).
    pub kill: zoid_tools::KillSlot,
```

In `chat_turn_config_with` (the `TurnConfig { ... }` literal ~line 80), add `kill: zoid_tools::KillSlot::new(),`.

In `crates/zoid/src/subagent.rs` (the `TurnConfig { ... }` literal ~line 150, which already sets `mcp: None` and `thinking: ...`), add `kill: zoid_tools::KillSlot::new(),`.

- [ ] **Step 2: Thread the `hard` token through the turn functions**

In `run_agent_turn` (~line 302), pass a never-firing hard token as the new trailing arg to `run_agent_turn_cancellable`:

```rust
    run_agent_turn_cancellable(
        config, provider, tools, gate, session, events, model, ui, session_id,
        companion_hub, now,
        CancellationToken::new(), // graceful (never fires here)
        CancellationToken::new(), // hard (never fires here)
    )
    .await
```

In `run_agent_turn_cancellable` (~line 325), add the parameter after `cancel: CancellationToken`:

```rust
    cancel: CancellationToken,
    hard: CancellationToken,
```

and pass `&hard` into `run_turn_inner` (add after the `&cancel,` argument ~line 387).

In `run_turn_inner` (~line 400), add the parameter after `cancel: &CancellationToken`:

```rust
    cancel: &CancellationToken,
    hard: &CancellationToken,
```

- [ ] **Step 3: Write the failing test — hard-cancel kills a running shell tool**

Add to the `tests` module in `crates/zoid/src/agent.rs`:

```rust
    #[tokio::test]
    async fn hard_cancel_kills_running_local_shell_and_balances() {
        use ulid::Ulid;
        use zoid_core::event::EventKind;
        use zoid_provider::{ProviderEvent, ToolCall};
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        // The model asks to run a long shell command, then would continue.
        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "shell".into(),
                args: serde_json::json!({ "command": "sleep 30" }),
            }),
            ProviderEvent::Done,
        ]));
        // Shared kill slot wired into both the tool list and the config.
        let kill = zoid_tools::KillSlot::new();
        let tools = std::sync::Arc::new(zoid_tools::registry_with_kill(kill.clone()));
        let mut cfg = chat_turn_config();
        cfg.kill = kill.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let graceful = CancellationToken::new();
        let hard = CancellationToken::new();
        // Fire hard shortly after the turn starts running the tool.
        let hard2 = hard.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            hard2.cancel();
        });
        let started = std::time::Instant::now();
        let out = run_agent_turn_cancellable(
            cfg,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(vec![]),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
            graceful,
            hard,
        )
        .await
        .unwrap();
        // The turn must end well before the 30s sleep would finish.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "hard-stop must not wait for the shell command"
        );
        // The tool call is answered with a killed result (balance preserved).
        let killed = out.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolResult { id, output, .. }
                if id == "call-1" && output.contains("[killed")
        ));
        assert!(killed, "the interrupted shell call must get a [killed] result");
    }
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p zoid hard_cancel_kills_running_local_shell -- --nocapture`
Expected: FAIL — `run_agent_turn_cancellable` takes 12 args not 13 (or, once Step 2 compiles, the test hangs/there is no killed result because the call site doesn't select on `hard`).

- [ ] **Step 5: Make the Local call site interruptible**

In `crates/zoid/src/agent.rs`, replace the Local-tool branch body (the `_ =>` arm, ~lines 1218-1225 — the `let tools_for_exec … spawn_blocking … .await?;` block) with:

```rust
                    let tools_for_exec = tools.clone();
                    let name = tc.name.clone();
                    let args = tc.args.clone();
                    let cwd = cwd_for_exec.clone();
                    let mut exec = tokio::task::spawn_blocking(move || {
                        zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd)
                    });
                    let out = tokio::select! {
                        biased;
                        _ = hard.cancelled() => {
                            // Force-kill the shell's process group; the blocking
                            // wait returns promptly once the child dies. Non-shell
                            // local tools have nothing registered — kill() is a
                            // no-op and we simply stop awaiting.
                            config.kill.kill();
                            let _ = (&mut exec).await; // reclaim the blocking task
                            zoid_tools::ToolOutput::err("[killed: hard-stop]")
                        }
                        joined = &mut exec => joined?,
                    };
```

Then, immediately after the existing `emit(...) ToolResult` block for this branch (right after it `.await?;`, before the `tracing::info!` at ~line 1243), insert the balanced-drain-on-kill:

```rust
                    if hard.is_cancelled() {
                        // Hard-stop mid-batch: answer every remaining call so no
                        // tool_use is left without a tool_result, then end.
                        for rest in pending_iter.by_ref() {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: rest.id,
                                    name: rest.name,
                                    output: "[skipped: turn aborted]".to_string(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                        outcome = "aborted";
                        break 'turn;
                    }
```

(The `pending_iter`, `session`, `events`, `ui`, `config`, `session_id`, `now`, and `outcome` bindings are the same ones the existing `is_error` drain at ~line 1148 uses — mirror that block's shape.)

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p zoid hard_cancel_kills_running_local_shell`
Expected: PASS in ~1s (not 30s).

- [ ] **Step 7: Run the agent suite to check nothing regressed**

Run: `cargo test -p zoid --lib agent::`
Expected: PASS (all existing agent tests plus the new one). The ~10 `run_agent_turn` tests are unaffected because that wrapper passes never-firing tokens.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/subagent.rs
git commit -m "feat(agent): hard cancel token kills running local shell tool, balanced"
```

---

## Task 3: Graceful abandon of an in-flight MCP call (agent.rs)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (new `call_or_abandon` helper; MCP branch ~lines 1169-1181)

**Interfaces:**
- Consumes: the existing `cancel: &CancellationToken` (graceful) already in `run_turn_inner`.
- Produces:
  - `async fn call_or_abandon<F>(cancel: &CancellationToken, fut: F) -> Option<ToolOutput> where F: std::future::Future<Output = ToolOutput>` — races `fut` against `cancel`; `None` if cancel wins first. A free `fn` in `agent.rs` (not a method).
  - Behavior: when `cancel` (graceful, first Esc) fires while an MCP `call_tool` is awaiting, the loop abandons the call (`None`), records `[skipped: turn aborted]` for it, drains the remaining batch, and ends the turn. (The MCP client's own 30s `REQUEST_TIMEOUT` remains the no-Esc backstop.)

**Why a helper:** the fake MCP server exposes only a fast `echo` tool and its
binary path (`CARGO_BIN_EXE_zoid_mcp_fake_server`) is not available to the
`zoid` crate's tests — so an in-flight-abandon cannot be exercised end-to-end
here without a fragile fixture. Lifting the race into `call_or_abandon` makes
the exact abandon behavior deterministically testable with `std::future::pending`,
and DRYs the MCP branch.

- [ ] **Step 1: Write the failing tests for `call_or_abandon`**

Add to the `tests` module in `crates/zoid/src/agent.rs`:

```rust
    #[tokio::test]
    async fn call_or_abandon_yields_none_when_cancel_wins() {
        let cancel = CancellationToken::new();
        cancel.cancel(); // already cancelled → abandon immediately
        let out = call_or_abandon(
            &cancel,
            std::future::pending::<zoid_tools::ToolOutput>(),
        )
        .await;
        assert!(out.is_none(), "a cancelled token must abandon the call");
    }

    #[tokio::test]
    async fn call_or_abandon_yields_result_when_future_completes() {
        let cancel = CancellationToken::new(); // never fired
        let out = call_or_abandon(&cancel, async {
            zoid_tools::ToolOutput::ok("done")
        })
        .await;
        assert_eq!(out.expect("future should win").text, "done");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p zoid call_or_abandon`
Expected: FAIL — `cannot find function 'call_or_abandon'`.

- [ ] **Step 3: Implement `call_or_abandon`**

Add near the other free functions in `crates/zoid/src/agent.rs` (module scope, not inside `impl`):

```rust
/// Race an async tool call against a cancellation token. Returns `Some(output)`
/// if the call finishes first, or `None` if `cancel` fires first (the caller
/// then synthesizes a balanced `[skipped: turn aborted]` result). Used to make
/// an in-flight MCP call abandonable on a graceful cancel (first Esc).
async fn call_or_abandon<F>(cancel: &CancellationToken, fut: F) -> Option<zoid_tools::ToolOutput>
where
    F: std::future::Future<Output = zoid_tools::ToolOutput>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        out = fut => Some(out),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid call_or_abandon`
Expected: PASS (2 tests).

- [ ] **Step 5: Use the helper in the MCP branch**

In `crates/zoid/src/agent.rs`, replace the `let out = match config.mcp.as_ref() { ... };` block in the `Some(ToolKind::Mcp)` arm (~lines 1175-1181) with:

```rust
                    let out = match config.mcp.as_ref() {
                        Some(m) => call_or_abandon(cancel, m.call_tool(&tc.name, &tc.args)).await,
                        None => Some(zoid_tools::ToolOutput::err(format!(
                            "mcp tool '{}' requested but no MCP manager is active",
                            tc.name
                        ))),
                    };
                    let out = match out {
                        Some(o) => o,
                        None => {
                            // Graceful cancel abandoned the call: answer it + drain
                            // the rest of the batch so no tool_use is unbalanced.
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: "[skipped: turn aborted]".to_string(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            for rest in pending_iter.by_ref() {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: rest.id,
                                        name: rest.name,
                                        output: "[skipped: turn aborted]".to_string(),
                                        is_error: false,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                            }
                            outcome = "aborted";
                            break 'turn;
                        }
                    };
```

The existing code after this point (`let tool_ok = !out.is_error; … emit(ToolResult …)`) stays and handles the normal (non-abandoned) result. The abandoned branch `break`s the turn, so it never reaches the normal emit.

- [ ] **Step 6: Run the agent + mcp suites**

Run: `cargo test -p zoid --lib agent:: && cargo test -p zoid-mcp`
Expected: PASS. The existing MCP e2e test (`crates/zoid-mcp/tests/end_to_end.rs`) still exercises a real non-cancelled `call_tool` end-to-end, so the happy path stays covered.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(agent): graceful cancel abandons in-flight MCP call via call_or_abandon"
```

---

## Task 4: UI wiring — escalation handler, two tokens, shared KillSlot (main.rs)

**Files:**
- Modify: `crates/zoid/src/invoke_skill.rs` (`chat_tools` signature)
- Modify: `crates/zoid/src/main.rs` (`App` fields, `spawn_turn`, `CancelTurn` handler, init sites, `run_agent_turn_cancellable` call)

**Interfaces:**
- Consumes: `zoid_tools::registry_with_kill` (Task 1), `TurnConfig.kill` + the `hard` param (Task 2).
- Produces: `chat_tools(skills, kill)` builds the chat tool list from `registry_with_kill(kill)`. `App` holds `turn_cancel` (graceful) + `turn_hard`. First `Esc` fires graceful; a second `Esc` while graceful is already fired fires hard.

- [ ] **Step 1: Thread `KillSlot` into `chat_tools`**

In `crates/zoid/src/invoke_skill.rs`, change the signature (~line 86) and the first line:

```rust
pub fn chat_tools(skills: Arc<SkillRegistry>, kill: zoid_tools::KillSlot) -> Vec<Box<dyn Tool>> {
    let mut tools = zoid_tools::registry_with_kill(kill);
    // … rest unchanged …
```

Update the two `chat_tools(...)` calls in that file's own tests (~lines 143, 180) to pass `zoid_tools::KillSlot::new()` as the second arg.

- [ ] **Step 2: Add the hard token field to `App`**

In `crates/zoid/src/main.rs`, next to `turn_cancel` (~line 1398) add:

```rust
    /// The hard-stop token: fired by a SECOND Esc/Ctrl-C while `turn_cancel` is
    /// already cancelled. Force-kills a running local tool. Cleared with
    /// `turn_cancel` on `TurnComplete`.
    turn_hard: Option<tokio_util::sync::CancellationToken>,
```

Add `turn_hard: None,` to every `App { … }` initializer that sets `turn_cancel: None,` (the runtime init ~line 1746 and the test init ~line 5085).

- [ ] **Step 3: Write the failing test for a real `escalate_cancel` helper**

Add to `crates/zoid/src/main.rs`'s test module a test that calls a REAL helper (defined in Step 5), so the test exercises production code, not a copy:

```rust
    #[test]
    fn second_cancel_escalates_to_hard() {
        let graceful = tokio_util::sync::CancellationToken::new();
        let hard = tokio_util::sync::CancellationToken::new();
        // First Esc: graceful only.
        assert_eq!(
            escalate_cancel(&graceful, &hard),
            "cancelling… (Esc again to force)"
        );
        assert!(graceful.is_cancelled() && !hard.is_cancelled());
        // Second Esc: escalate to hard.
        assert_eq!(escalate_cancel(&graceful, &hard), "force-stopping…");
        assert!(hard.is_cancelled());
    }
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p zoid second_cancel_escalates_to_hard`
Expected: FAIL — `cannot find function 'escalate_cancel'`.

- [ ] **Step 5: Implement `escalate_cancel` and call it from the handler**

In `crates/zoid/src/main.rs`, add the helper at module scope (near the other free `fn`s):

```rust
/// Decide the interrupt tier for a `CancelTurn`. First call fires `graceful`;
/// a second call (graceful already fired) fires `hard`. Returns the status-hint
/// text to show. Kept as a free fn so the escalation contract is unit-tested.
fn escalate_cancel(
    graceful: &tokio_util::sync::CancellationToken,
    hard: &tokio_util::sync::CancellationToken,
) -> &'static str {
    if graceful.is_cancelled() {
        hard.cancel();
        "force-stopping…"
    } else {
        graceful.cancel();
        "cancelling… (Esc again to force)"
    }
}
```

Then replace the `Action::CancelTurn` arm body (~lines 3053-3060) with:

```rust
        Action::CancelTurn => {
            // First Esc: graceful (finish current step, drain, end). Second Esc
            // while already cancelling: hard-stop — force-kill the running tool.
            // The resulting TurnComplete clears both tokens.
            if let (Some(g), Some(h)) = (&app.turn_cancel, &app.turn_hard) {
                app.shell.status_hint = Some(escalate_cancel(g, h).into());
            }
        }
```

- [ ] **Step 6: Clear both tokens on `TurnComplete`**

In the `AgentUpdate::TurnComplete` arm (~line 2276) where `app.turn_cancel = None;` is set, add on the next line:

```rust
                        app.turn_hard = None;
```

- [ ] **Step 7: Build + share the `KillSlot` and pass the hard token in `spawn_turn`**

In `crates/zoid/src/main.rs` `spawn_turn` (~line 4388), make these edits:

Replace the chat-tools line (~4401):

```rust
    let kill = zoid_tools::KillSlot::new();
    let mut tools = zoid::invoke_skill::chat_tools(std::sync::Arc::new(effective), kill.clone());
```

After `turn_config.thinking = …;` (~4429) add:

```rust
    turn_config.kill = kill.clone();
```

Replace the token mint + store (~4432-4433):

```rust
    let cancel = tokio_util::sync::CancellationToken::new();
    let hard = tokio_util::sync::CancellationToken::new();
    app.turn_cancel = Some(cancel.clone());
    app.turn_hard = Some(hard.clone());
```

Add `hard` as the final argument to the `run_agent_turn_cancellable(...)` call (~4448, after `cancel,`):

```rust
            cancel,
            hard,
```

- [ ] **Step 8: Build and run the whole zoid crate suite**

Run: `cargo test -p zoid`
Expected: PASS. Watch for any other `chat_tools(` call site the compiler flags — update it to pass a `KillSlot`.

- [ ] **Step 9: Manual smoke check (documented, not automated)**

Build and run zoid, start a turn that runs `shell` with `sleep 30`, press `Esc` once (hint shows `cancelling… (Esc again to force)`, the sleep keeps running), then `Esc` again (hint shows `force-stopping…`, the turn ends immediately). Confirm no stray `sleep` process remains: `pgrep -af 'sleep 30'` returns nothing.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid/src/invoke_skill.rs
git commit -m "feat(ui): escalating Esc — second press hard-stops, shares KillSlot"
```

---

## Final verification (before whole-branch review)

- [ ] Run the full workspace suite: `cargo test --workspace`
  Expected: PASS. (If a `--workspace` compile error appears in an unrelated crate, verify against a clean checkout of HEAD — a shared checkout can carry another session's uncommitted edits.)
- [ ] `cargo clippy --workspace --all-targets` introduces no NEW warnings beyond the pre-existing baseline.
- [ ] Grep check: no `tracing::*` call added in this branch interpolates an env value or a raw command containing secrets (the shell command text is user-typed, not secret, and is already surfaced to the model — unchanged from before).

## Notes carried from the spec

- Streaming's existing graceful-interrupt behavior (`agent.rs` stream `select!`) is left exactly as-is.
- `subagent_diff`'s `git` subprocesses and `search`/`read_file` get abandon-wait (the Local `select!` returns control; their bounded work finishes orphaned) — no dedicated kill plumbing, by design.
- The MCP client's 30s `REQUEST_TIMEOUT` (`zoid-mcp/src/client.rs`) is the no-Esc backstop; this plan adds no new MCP timeout.
