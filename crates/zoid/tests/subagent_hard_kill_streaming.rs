//! Regression test for C1: a subagent parked in the PROVIDER/STREAMING phase
//! (i.e. awaiting `prx.recv()` inside the model-streaming loop in
//! `run_agent_turn_cancellable`) must be stopped by the `hard` cancellation
//! token, not just the (subagent-unused) `cancel` token.
//!
//! `ParkingProvider` never completes its stream — it parks forever, holding
//! the sink open, which keeps the subagent's inner select! loop blocked on
//! `prx.recv()`. This reproduces the subagent's most common parked state.
//! Without the fix (hard observed only in tool-exec select!s, not here),
//! firing `hard` does nothing and the run never returns — this test times
//! out. With the fix, the streaming select! also races `hard.cancelled()`,
//! so the run returns promptly once `hard` fires.

use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use zoid::subagent::run_subagent;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

/// A provider that sends one delta then parks forever, never sending `Done`
/// and never dropping the sink — keeps the caller's `prx.recv()` parked.
struct ParkingProvider;

#[async_trait]
impl Provider for ParkingProvider {
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        let _ = sink
            .send(ProviderEvent::TextDelta("parked...".into()))
            .await;
        // Never returns, never sends Done, keeps `sink` alive.
        std::future::pending::<()>().await;
        unreachable!()
    }
}

#[tokio::test]
async fn hard_token_stops_subagent_parked_in_streaming() {
    let provider = Arc::new(ParkingProvider);
    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let hard = CancellationToken::new();
    let hard_for_run = hard.clone();

    let handle = tokio::spawn(async move {
        run_subagent(
            "stall in streaming",
            &zoid::eventlog::EventLog::new(),
            &AgentProfile::builtin(),
            provider,
            std::path::PathBuf::from("."),
            "glm".into(),
            zoid_provider::ThinkingMode::Off,
            session.clone(),
            Ulid::new(),
            tx,
            || 0,
            "sub-parking-test".into(),
            zoid_core::config::ApprovalConfig::default(),
            CancellationToken::new(), // cancel — never fired; subagents don't use it
            hard_for_run,
            Arc::new(AtomicI64::new(0)),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    hard.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        result.is_ok(),
        "subagent did not return within 5s after `hard` was cancelled while parked in streaming \
         (C1: hard token not observed in the provider streaming select!)"
    );
    // The join itself must also succeed (no panic).
    let _ = result.unwrap().unwrap();
}
