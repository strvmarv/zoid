//! Standalone subagent spawner — extracted so the spawned future's `Send` bound
//! is analyzed independently of `run_turn_inner`'s context.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::agent_profile::AgentProfile;
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::Provider;

use crate::agent::AgentUpdate;

#[allow(clippy::too_many_arguments)]
pub fn spawn_subagent(
    task: String,
    seed: crate::eventlog::EventLog,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    model: String,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
    sub_id: String,
    wt: Option<crate::worktree::WorktreeGuard>,
    approval: zoid_core::config::ApprovalConfig,
) {
    tokio::spawn(async move {
        let res = crate::subagent::run_subagent(
            &task,
            &seed,
            &AgentProfile::builtin(),
            provider,
            cwd,
            model,
            session.clone(),
            session_id,
            ui.clone(),
            now,
            sub_id,
            approval,
        )
        .await;
        drop(wt);

        let (subagent_id, branch, summary, ok) = match res {
            Ok(r) => (r.id, r.branch, r.summary, r.ok),
            Err(e) => (
                String::new(),
                String::new(),
                format!("subagent failed: {e}"),
                false,
            ),
        };
        let ev = Event::new(
            Ulid::new(),
            None,
            now(),
            EventKind::DelegationResult {
                subagent_id,
                branch,
                summary,
                ok,
            },
        )
        .with_session(session_id);
        let _ = session.append(ev.clone()).await;
        let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    });
}