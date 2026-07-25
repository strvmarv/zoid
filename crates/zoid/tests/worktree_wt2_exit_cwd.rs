//! WT-2 regression fence: a `shell` call issued in a LATER sub-turn than
//! `exit_worktree` must run in the repo root, not the deleted worktree.
//!
//! The bug: `cwd_for_exec` was declared *inside* the `'turn: loop`, so it was
//! re-initialized from the frozen `TurnConfig.cwd` snapshot at every sub-turn
//! boundary. A turn that started inside a worktree carries `config.cwd =
//! <worktree>`; `exit_worktree` deletes that directory and repoints
//! `cwd_for_exec` at the repo root, but the very next sub-turn threw that
//! away and handed the deleted path back to the shell — `Command::current_dir`
//! chdir's before exec, so `spawn()` failed with ENOENT and the shell stayed
//! unusable for the rest of the turn.
//!
//! The existing WT-1 fence batches its tool calls into a SINGLE sub-turn, so it
//! cannot see this: the reassignment survives within a batch. Only a sub-turn
//! boundary between the relocation and the next tool call exposes it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use zoid::agent::{run_agent_turn, AgentUpdate, WorktreeAction};
use zoid::worktree::{create_worktree, remove_worktree};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

/// Replays one scripted stream per `stream()` call, in order (one per sub-turn).
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        let script = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

/// Init a git repo with one committed file (a worktree needs a HEAD).
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

#[tokio::test]
async fn shell_after_exit_worktree_in_a_later_subturn_runs_in_repo_root() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let repo_dir = std::fs::canonicalize(repo.path()).unwrap();

    // The session is already inside a worktree when the turn starts — exactly
    // what `spawn_turn` models by overriding `turn_config.cwd` from
    // `app.active_worktree` (main.rs). This snapshot is frozen for the turn.
    let (wt_path, _branch) = create_worktree(&repo_dir, "wt2").unwrap().into_kept();
    let wt_abs = std::fs::canonicalize(&wt_path).unwrap_or(wt_path);

    // Sub-turn 1: exit_worktree ALONE (deletes the worktree dir).
    // Sub-turn 2: shell — a fresh batch, so it re-reads whatever cwd the loop
    // considers current. This is the boundary that regressed.
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call("exit_worktree", json!({})),
                ProviderEvent::Done,
            ],
            vec![
                zoid_testkit::tool_call("shell", json!({ "command": "pwd" })),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("done"), ProviderEvent::Done],
        ])),
    });

    let tools: Arc<Vec<Box<dyn zoid_tools::Tool>>> = Arc::new(vec![
        Box::new(zoid_tools::worktree_exit::ExitWorktree),
        Box::new(zoid_tools::shell::Shell::default()),
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    // Main-loop stand-in: mirror `compute_worktree_switch`'s Exit arm — compute
    // the repo root BEFORE removal, delete the worktree, reply with the root.
    let removed: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let removed_probe = removed.clone();
    let root_for_drain = repo_dir.clone();
    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::WorktreeRequested {
                    action: WorktreeAction::Exit,
                    reply,
                } => {
                    remove_worktree(&root_for_drain, "wt2", true).unwrap();
                    *removed.lock().unwrap() = Some(root_for_drain.clone());
                    let _ = reply.send(Ok((root_for_drain.clone(), None)));
                }
                AgentUpdate::WorktreeRequested { reply, .. } => {
                    let _ = reply.send(Err("enter not expected".into()));
                }
                _ => {}
            }
        }
    });

    let mut cfg = zoid::agent::chat_turn_config();
    cfg.cwd = wt_abs.clone();

    let log = run_agent_turn(
        cfg,
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();

    drain.await.unwrap();

    assert!(
        removed_probe.lock().unwrap().is_some(),
        "exit_worktree must have been processed"
    );
    assert!(
        !wt_abs.exists(),
        "the worktree directory must actually be gone — otherwise this test \
         cannot observe the ENOENT it guards against"
    );

    let shell_result = log
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ToolResult {
                name,
                output,
                is_error,
                ..
            } if name == "shell" => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("the shell call in sub-turn 2 must have produced a ToolResult");

    let (output, is_error) = shell_result;
    assert!(
        !is_error,
        "shell must not fail after exit_worktree; got: {output}"
    );
    assert!(
        !output.contains("No such file or directory"),
        "shell inherited the deleted worktree cwd (WT-2 regression): {output}"
    );
    // Positive assertion: it ran in the repo root the exit handler returned,
    // not merely "somewhere that exists".
    assert!(
        output.contains(&repo_dir.display().to_string()),
        "shell must run in the repo root {} after exit_worktree; got: {output}",
        repo_dir.display()
    );
}
