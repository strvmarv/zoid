//! A reusable timeout supervisor. It watches a heartbeat (`progress`, an epoch-ms
//! `AtomicI64` the caller bumps as work advances) and a start time, and on breach
//! records a caller-supplied reason value (first-writer-wins) then fires a
//! `CancellationToken`. Generic over the reason type `R` so it can carry any
//! value without knowing its meaning. Stops cleanly when `done` fires.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub struct WakeTimer;

impl WakeTimer {
    /// Spawn a supervisor. Fires `fire` when `now() - progress > idle` (no-progress)
    /// or `now() - start > ceiling` (absolute); writes the matching reason value
    /// into `reason` (first-writer-wins) just before firing. Stops when `done`
    /// is cancelled. `None` disables that arm; both `None` → no work is done.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn<R: Send + 'static>(
        idle: Option<Duration>,
        ceiling: Option<Duration>,
        progress: Arc<AtomicI64>,
        now: fn() -> i64,
        idle_reason: R,
        ceiling_reason: R,
        reason: Arc<Mutex<Option<R>>>,
        fire: CancellationToken,
        done: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        // Both arms disabled: nothing to supervise. Return a finished task so the
        // handle is uniform for callers (the kill tool + Esc still work via the
        // registry even when no timer runs).
        if idle.is_none() && ceiling.is_none() {
            return tokio::spawn(async {});
        }
        let start = now();
        let idle_ms = idle.map(|d| d.as_millis() as i64);
        let ceiling_ms = ceiling.map(|d| d.as_millis() as i64);
        tokio::spawn(async move {
            let mut idle_reason = Some(idle_reason);
            let mut ceiling_reason = Some(ceiling_reason);
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    biased;
                    _ = done.cancelled() => return,
                    _ = ticker.tick() => {
                        let t = now();
                        let last = progress.load(Ordering::Relaxed);
                        let idle_breach = idle_ms.is_some_and(|i| i > 0 && t - last > i);
                        let ceiling_breach = ceiling_ms.is_some_and(|c| c > 0 && t - start > c);
                        if idle_breach || ceiling_breach {
                            // Ceiling wins the label when both trip on the same tick.
                            let r = if ceiling_breach {
                                ceiling_reason.take()
                            } else {
                                idle_reason.take()
                            };
                            if let Some(r) = r {
                                let mut slot = reason.lock().unwrap();
                                if slot.is_none() {
                                    *slot = Some(r);
                                }
                            }
                            fire.cancel();
                            return;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestReason {
        Idle,
        Ceiling,
    }

    // Each test owns a distinct process-global clock + fn pointer so parallel
    // test runs never race a shared static. `now: fn() -> i64` cannot close over
    // per-test state, so a dedicated static per test is the race-free way to
    // inject a controllable clock.
    static CLOCK_IDLE: AtomicI64 = AtomicI64::new(0);
    fn now_idle() -> i64 {
        CLOCK_IDLE.load(Ordering::Relaxed)
    }

    static CLOCK_CEIL: AtomicI64 = AtomicI64::new(0);
    fn now_ceil() -> i64 {
        CLOCK_CEIL.load(Ordering::Relaxed)
    }

    static CLOCK_NOBREACH: AtomicI64 = AtomicI64::new(0);
    fn now_nobreach() -> i64 {
        CLOCK_NOBREACH.load(Ordering::Relaxed)
    }

    static CLOCK_DONE: AtomicI64 = AtomicI64::new(0);
    fn now_done() -> i64 {
        CLOCK_DONE.load(Ordering::Relaxed)
    }

    #[tokio::test(start_paused = true)]
    async fn fires_on_idle_breach() {
        CLOCK_IDLE.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0)); // never bumped → idle grows
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_idle,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Advance the injected wall clock past the 1s idle window (progress stays 0).
        CLOCK_IDLE.store(2000, Ordering::Relaxed);
        // Drive the ticker so it samples the clock and breaches.
        tokio::time::advance(Duration::from_millis(300)).await;
        fire.cancelled().await; // resolves once the timer fires
        assert!(fire.is_cancelled(), "idle breach must fire the token");
        assert_eq!(*reason.lock().unwrap(), Some(TestReason::Idle));
        let _ = h.await;
    }

    #[tokio::test(start_paused = true)]
    async fn fires_on_ceiling_breach() {
        CLOCK_CEIL.store(0, Ordering::Relaxed); // start captured as 0 at spawn
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            None,
            Some(Duration::from_secs(1)),
            progress.clone(),
            now_ceil,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Keep progress fresh so idle can never be the cause; only the ceiling trips.
        CLOCK_CEIL.store(2000, Ordering::Relaxed);
        progress.store(2000, Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(300)).await;
        fire.cancelled().await;
        assert!(fire.is_cancelled(), "ceiling breach must fire the token");
        assert_eq!(*reason.lock().unwrap(), Some(TestReason::Ceiling));
        let _ = h.await;
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_fire_before_breach() {
        CLOCK_NOBREACH.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let _h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_nobreach,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Only 500ms of "idle" — under the 1s window.
        CLOCK_NOBREACH.store(500, Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(300)).await;
        assert!(
            !fire.is_cancelled(),
            "must not fire before the window elapses"
        );
        assert!(reason.lock().unwrap().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn done_stops_the_supervisor() {
        CLOCK_DONE.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_done,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done.clone(),
        );
        done.cancel(); // normal completion signalled before any breach
        tokio::time::advance(Duration::from_millis(300)).await;
        h.await.expect("supervisor task should exit on done");
        assert!(
            !fire.is_cancelled(),
            "done must stop the timer without firing"
        );
    }
}
