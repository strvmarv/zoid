//! WT-1 regression fence: a commit issued after `enter_worktree` in the same
//! agent turn must land on the WORKTREE branch, not the parent branch.
//!
//! This drives the real turn loop (`run_agent_turn`) with a scripted provider
//! that emits `enter_worktree` and then a `git commit` shell call. A UI-drain
//! task plays the role of the main loop's `handle_worktree_request`: it answers
//! the `WorktreeRequested` oneshot by creating the worktree and replying with
//! its absolute path — exactly as `compute_worktree_switch`'s Enter arm does.
//! The test proves the turn loop repoints `cwd_for_exec` at the worktree so the
//! subsequent commit targets the worktree branch. The original WT-1 bug was
//! commits silently landing on the PARENT branch (with a cheerful green
//! ToolResult) because the shell kept the parent's git context — exactly the
//! class of failure this fence guards against a future `run_turn_inner`
//! refactor reintroducing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use zoid::agent::{run_agent_turn, AgentUpdate, WorktreeAction};
use zoid::worktree::create_worktree;
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

/// Init a git repo with one committed file (a worktree needs a HEAD). The
/// commit subject "init" is the parent-branch marker the test asserts stays put.
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

/// `git -C <dir> log -1 --format=%s` → the subject line of the tip commit that
/// `dir`'s checkout currently points at (branch-agnostic: reads HEAD).
fn head_subject(dir: &Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["log", "-1", "--format=%s"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn commit_after_enter_worktree_lands_on_worktree_branch_not_parent() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let repo_dir = repo.path().to_path_buf();

    // Sub-turn 1 batches BOTH tool calls in a single assistant message:
    // enter_worktree THEN the commit. This matches the WT-1 fix's scope — the
    // turn loop reassigns `cwd_for_exec` when it processes enter_worktree, and
    // that reassignment holds for subsequent tools *in the same batch* (it is
    // re-initialized to config.cwd at each sub-turn boundary; the cross-turn
    // case is handled in production by the main loop's spawn_turn reading
    // active_worktree, which the library layer alone cannot model). Sub-turn 2
    // just finishes. `--allow-empty` avoids staging; inline `-c user.*` keeps
    // the commit hermetic (no dependence on global git config).
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call("enter_worktree", json!({ "name": "wt1" })),
                zoid_testkit::tool_call(
                    "shell",
                    json!({
                        "command": "git -c user.name=zoid -c user.email=zoid@example.com \
                                    commit --allow-empty -m wt1-marker"
                    }),
                ),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("done"), ProviderEvent::Done],
        ])),
    });

    // Only the tools this turn needs: the Emitting enter_worktree + shell.
    let tools: Arc<Vec<Box<dyn zoid_tools::Tool>>> = Arc::new(vec![
        Box::new(zoid_tools::worktree_enter::EnterWorktree),
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

    // Main-loop stand-in: answer WorktreeRequested by creating the worktree and
    // replying with its absolute path — mirroring compute_worktree_switch's
    // Enter arm (create_worktree(...).into_kept() → canonicalized path).
    let created: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let created_probe = created.clone();
    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::WorktreeRequested {
                    action: WorktreeAction::Enter { name },
                    reply,
                } => {
                    let (path, _branch) = create_worktree(&repo_dir, &name).unwrap().into_kept();
                    let abs = std::fs::canonicalize(&path).unwrap_or(path);
                    *created.lock().unwrap() = Some(abs.clone());
                    let _ = reply.send(Ok((abs, None)));
                }
                AgentUpdate::WorktreeRequested { reply, .. } => {
                    // Exit is not scripted in this test.
                    let _ = reply.send(Err("exit not expected".into()));
                }
                _ => {}
            }
        }
    });

    let mut cfg = zoid::agent::chat_turn_config();
    cfg.cwd = repo.path().to_path_buf();

    run_agent_turn(
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

    let wt_path = created_probe
        .lock()
        .unwrap()
        .clone()
        .expect("enter_worktree must have created a worktree");

    // WT-1: the commit landed on the WORKTREE branch...
    assert_eq!(
        head_subject(&wt_path),
        "wt1-marker",
        "commit issued after enter_worktree must land on the worktree branch"
    );
    // ...and NOT on the parent branch, whose tip is still the init commit. A
    // regression (commit leaking to the parent) flips this to "wt1-marker".
    assert_eq!(
        head_subject(repo.path()),
        "init",
        "parent branch must NOT receive the worktree commit (the WT-1 bug)"
    );
}
