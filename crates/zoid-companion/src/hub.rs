//! The async↔blocking bridge. The render loop (async) publishes cards; the
//! blocking SSE reader threads park on the condvar until the version bumps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Latest {
    card: Option<String>,
    version: u64,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub version: u64,
    pub card: Option<String>,
}

pub struct CompanionHub {
    inner: Mutex<Latest>,
    cv: Condvar,
    enabled: AtomicBool,
}

impl CompanionHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Latest::default()),
            cv: Condvar::new(),
            enabled: AtomicBool::new(false),
        })
    }

    pub fn set_enabled(&self, v: bool) {
        self.enabled.store(v, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn publish_card(&self, html: String) {
        let mut l = self.inner.lock().unwrap();
        l.card = Some(html);
        l.version += 1;
        drop(l);
        self.cv.notify_all();
    }

    pub fn current(&self) -> Frame {
        let l = self.inner.lock().unwrap();
        Frame {
            version: l.version,
            card: l.card.clone(),
        }
    }

    /// Block until `version > last` or `timeout` elapses; return current state.
    pub fn wait_after(&self, last: u64, timeout: Duration) -> Frame {
        let l = self.inner.lock().unwrap();
        let (l, _) = self
            .cv
            .wait_timeout_while(l, timeout, |l| l.version == last)
            .unwrap();
        Frame {
            version: l.version,
            card: l.card.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_card_bumps_version_and_current_reflects() {
        let hub = CompanionHub::new();
        assert_eq!(hub.current().version, 0);
        hub.publish_card("<b>a</b>".into());
        let f = hub.current();
        assert_eq!(f.version, 1);
        assert_eq!(f.card.as_deref(), Some("<b>a</b>"));
    }

    #[test]
    fn wait_after_returns_on_publish_and_times_out_otherwise() {
        let hub = CompanionHub::new();
        // times out at the same version when nothing publishes
        let f = hub.wait_after(0, Duration::from_millis(50));
        assert_eq!(f.version, 0);

        // wakes when another thread publishes
        let h2 = hub.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            h2.publish_card("<b>hi</b>".into());
        });
        let f = hub.wait_after(0, Duration::from_secs(2));
        assert_eq!(f.version, 1);
        assert_eq!(f.card.as_deref(), Some("<b>hi</b>"));
    }

    #[test]
    fn enabled_flag_toggles() {
        let hub = CompanionHub::new();
        assert!(!hub.is_enabled());
        hub.set_enabled(true);
        assert!(hub.is_enabled());
        hub.set_enabled(false);
        assert!(!hub.is_enabled());
    }
}
