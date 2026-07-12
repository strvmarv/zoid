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

/// Map a subagent run outcome to `DelegationResult` fields `(id, branch,
/// summary, ok)`. On failure the subagent's OWN `sub_id` is preserved, NOT
/// blanked: the UI drops the in-flight drawer row by matching this id
/// (`main.rs` `retain(|s| s.id != subagent_id)`), so a blank id matches no
/// real row and the entry would leak forever — the drawer would never clear
/// after a failed subagent.
fn delegation_fields(
    res: anyhow::Result<crate::subagent::SubagentResult>,
    sub_id: &str,
) -> (String, String, String, bool) {
    match res {
        Ok(r) => (r.id, r.branch, r.summary, r.ok),
        Err(e) => (
            sub_id.to_string(),
            String::new(),
            format!("subagent failed: {e}"),
            false,
        ),
    }
}

/// Build a summary line for an aborted (killed / timed-out) subagent.
pub(crate) fn abort_summary(reason: Option<crate::agent::AbortReason>) -> String {
    match reason {
        Some(r) => format!("aborted ({})", r.label()),
        None => "aborted".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_subagent(
    task: String,
    seed: crate::eventlog::EventLog,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    model: String,
    thinking: zoid_provider::ThinkingMode,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
    sub_id: String,
    wt: Option<crate::worktree::WorktreeGuard>,
    approval: zoid_core::config::ApprovalConfig,
    cancel: tokio_util::sync::CancellationToken,
    hard: tokio_util::sync::CancellationToken,
    progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    abort_reason: std::sync::Arc<std::sync::Mutex<Option<crate::agent::AbortReason>>>,
    idle: Option<std::time::Duration>,
    ceiling: Option<std::time::Duration>,
) {
    tokio::spawn(async move {
        // Supervisor: trips `hard` (with a reason) on idle/ceiling breach. `done`
        // stops it on normal completion so nothing is left spinning.
        let done = tokio_util::sync::CancellationToken::new();
        let _timer = crate::wake_timer::WakeTimer::spawn(
            idle,
            ceiling,
            progress.clone(),
            now,
            crate::agent::AbortReason::IdleTimeout,
            crate::agent::AbortReason::Ceiling,
            abort_reason.clone(),
            hard.clone(),
            done.clone(),
        );

        let res = crate::subagent::run_subagent(
            &task,
            &seed,
            &AgentProfile::builtin(),
            provider,
            cwd,
            model,
            thinking,
            session.clone(),
            session_id,
            ui.clone(),
            now,
            sub_id.clone(),
            approval,
            cancel,
            hard.clone(),
            progress,
        )
        .await;

        // Stop the supervisor now that the run has returned.
        done.cancel();

        // If a firer tripped `hard`, force the failure branch regardless of what
        // the drained turn returned: label it with the abort reason and discard
        // the worktree (partial work is not kept).
        let res = if hard.is_cancelled() {
            let reason = *abort_reason.lock().unwrap();
            Err(anyhow::anyhow!(abort_summary(reason)))
        } else {
            res
        };

        // Commit the subagent's working-tree changes on the success path,
        // then retain the branch (with commits) for subagent_diff retrieval.
        // On error (incl. abort), drop the guard (full cleanup discards partial work).
        match &res {
            Ok(_) => {
                if let Some(wt) = &wt {
                    // `.output()` (NOT `.status()`) so git's stdout/stderr are
                    // captured, not inherited. The TUI owns the alternate screen;
                    // an inherited `On branch …` / `nothing to commit` line would
                    // paint raw text over ratatui's back-buffer, corrupting the
                    // rail (garbled cwd) and appearing as bottom-bar growth.
                    let _ = std::process::Command::new("git")
                        .args(["-C"])
                        .arg(wt.path())
                        .args(["add", "-A"])
                        .output();
                    let _ = std::process::Command::new("git")
                        .args(["-C"])
                        .arg(wt.path())
                        .args(["commit", "-m", &format!("subagent {sub_id}")])
                        .output();
                }
                if let Some(wt) = wt {
                    let _ = wt.into_kept_branch();
                }
            }
            Err(_) => {
                drop(wt);
            }
        }

        let (subagent_id, branch, summary, ok) = delegation_fields(res, &sub_id);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_fields_preserves_id_on_failure() {
        // Regression: a failed subagent used to emit subagent_id="" here, so
        // the UI's id-keyed drawer cleanup matched no row and the in-flight
        // entry leaked forever (drawer never cleared on failure).
        let (id, branch, summary, ok) =
            delegation_fields(Err(anyhow::anyhow!("provider 400")), "sub-01ABC");
        assert_eq!(
            id, "sub-01ABC",
            "failed subagent must keep its id so the drawer row is removed"
        );
        assert!(branch.is_empty(), "no branch is retained on failure");
        assert!(!ok, "failure must be reported as not-ok");
        assert!(
            summary.contains("provider 400"),
            "the error reason should surface in the summary card"
        );
    }

    #[test]
    fn abort_summary_uses_reason_label() {
        // A killed/timed-out subagent must surface its reason in the summary.
        let s = super::abort_summary(Some(crate::agent::AbortReason::IdleTimeout));
        assert!(s.contains("idle timeout"), "summary carries the reason: {s}");
        let s2 = super::abort_summary(None);
        assert!(!s2.is_empty(), "a reasonless abort still has a summary");
    }
}
