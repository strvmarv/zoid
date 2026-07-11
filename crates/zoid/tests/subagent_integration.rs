//! Proves the B + C chain: a subagent, given a constructed task and run with
//! its cwd set to an isolated git worktree, writes a file via the `write_file`
//! tool INSIDE that worktree; the main working copy is never touched; and the
//! worktree is cleaned up when its `WorktreeGuard` drops.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use zoid::subagent::run_subagent;
use zoid::worktree::create_worktree;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Provider, ProviderEvent, ToolCall};

fn init_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    std::fs::write(dir.join("seed.txt"), "seed").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("seed.txt")).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("zoid", "zoid@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
}

/// A provider that pops one scripted turn per `stream()` call. Unlike the
/// shared `zoid_provider::FakeProvider` (which replays its whole event vec on
/// every call), this lets us script a two-turn tool-call/summary exchange
/// without the `write_file` ToolCall re-firing on turn 2.
struct ScriptedProvider {
    turns: Mutex<VecDeque<Vec<ProviderEvent>>>,
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

#[tokio::test]
async fn subagent_writes_inside_its_worktree_not_the_main_copy() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Turn 1: the subagent calls write_file with a RELATIVE path. Turn 2 (after
    // the tool result comes back): the subagent summarizes.
    let turn1 = vec![
        ProviderEvent::ToolCall(ToolCall {
            id: "w1".into(),
            name: "write".into(),
            args: serde_json::json!({ "path": "out.txt", "content": "made by subagent" }),
        }),
        ProviderEvent::Done,
    ];
    let turn2 = vec![
        ProviderEvent::TextDelta("Wrote out.txt.".into()),
        ProviderEvent::Done,
    ];
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![turn1, turn2])),
    });

    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let wt_path;
    {
        let wt = create_worktree(tmp.path(), "sub-int").unwrap();
        wt_path = wt.path().to_path_buf();
        let res = run_subagent(
            "create out.txt",
            &zoid::eventlog::EventLog::new(),
            &AgentProfile::builtin(),
            provider,
            wt.path().to_path_buf(), // subagent cwd = the worktree (B1 seam)
            "glm".into(),
            zoid_provider::ThinkingMode::Off,
            session,
            ulid::Ulid::new(), // session_id (B4)
            tx,
            || 0,
            "sub-test".into(),
            zoid_core::config::ApprovalConfig::default(),
        )
        .await
        .unwrap();
        assert!(res.ok);
        // The write landed INSIDE the worktree.
        assert_eq!(
            std::fs::read_to_string(wt.path().join("out.txt")).unwrap(),
            "made by subagent"
        );
    } // worktree dropped -> cleaned up

    // Isolation: the main working copy never saw the subagent's file.
    assert!(!tmp.path().join("out.txt").exists(), "main copy untouched");
    // Cleanup: the worktree directory is gone.
    assert!(!wt_path.exists(), "worktree removed on drop");
}
