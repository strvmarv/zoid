//! Observability layer: a `tracing` subscriber with a JSON file sink (env-gated
//! by `ZOID_LOG`) plus an in-memory aggregator (`ObsState`) that powers the
//! Overview page. Never panics: file-sink failure is silent, locks are
//! `.ok()`-guarded, aggregates are bounded.

use std::sync::{Arc, Mutex};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

#[derive(Debug, Default)]
pub struct ObsState; // expanded in Task 3

pub struct ObsHandle {
    pub state: Arc<Mutex<ObsState>>,
}

/// Build and install the global subscriber. Idempotent-safe to call once at
/// startup. Returns a handle holding the shared aggregate state.
pub fn init() -> ObsHandle {
    let state = Arc::new(Mutex::new(ObsState::default()));
    install(state.clone());
    ObsHandle { state }
}

/// Env var naming the JSON diagnostic file. Unset → no file layer (zero cost),
/// preserving the old `dbglog` activation contract.
const LOG_ENV: &str = "ZOID_LOG";

fn env_filter() -> EnvFilter {
    // RUST_LOG wins; default `info` keeps the 60fps TRACE `frame` events out of
    // the file unless the operator opts in with RUST_LOG=trace.
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// A JSON file layer over `path`, or None if the file can't be opened.
fn json_file_layer<S>(path: &std::path::Path) -> Option<Box<dyn Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::sync::Mutex::new(file))
        .with_filter(env_filter());
    Some(layer.boxed())
}

const ROLL_CAP: usize = 64;

/// Bounded rolling window: total count + last value + avg/p90 over the last
/// `ROLL_CAP` samples. O(1) memory.
#[derive(Debug, Default)]
pub struct RollingStats {
    window: std::collections::VecDeque<u64>,
    count: u64,
    last: u64,
}

impl RollingStats {
    pub fn record(&mut self, sample: u64) {
        self.count += 1;
        self.last = sample;
        if self.window.len() == ROLL_CAP {
            self.window.pop_front();
        }
        self.window.push_back(sample);
    }
    pub fn count(&self) -> u64 { self.count }
    pub fn last(&self) -> u64 { self.last }
    pub fn avg(&self) -> u64 {
        if self.window.is_empty() { return 0; }
        (self.window.iter().sum::<u64>()) / self.window.len() as u64
    }
    pub fn p90(&self) -> u64 {
        if self.window.is_empty() { return 0; }
        let mut v: Vec<u64> = self.window.iter().copied().collect();
        v.sort_unstable();
        // ceil((len)*0.9), clamped — index of the 90th-percentile sample.
        let idx = (((v.len() as f64) * 0.9).ceil() as usize).min(v.len() - 1);
        v[idx]
    }
}

/// Test helper: a Registry with only the file layer (no global install).
#[cfg(test)]
fn file_only_subscriber(path: &std::path::Path) -> Option<impl tracing::Subscriber> {
    Some(Registry::default().with(json_file_layer(path)))
}

/// Install the global subscriber: ObsLayer (always on) + optional JSON file
/// layer (when `ZOID_LOG` is set). Safe to call once.
fn install(_state: Arc<Mutex<ObsState>>) {
    let file_layer = std::env::var(LOG_ENV)
        .ok()
        .and_then(|p| json_file_layer::<Registry>(std::path::Path::new(&p)));
    // ObsLayer is added in Task 4; for now the file layer alone.
    let _ = Registry::default().with(file_layer).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn json_file_layer_writes_a_line_when_env_set() {
        let dir = std::env::temp_dir().join(format!("zoid-obs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("obs.log");
        // build a subscriber with ONLY the file layer, writing to `path`, and
        // emit one event through it (scoped, not global, so the test is isolated).
        let sub = file_only_subscriber(&path).expect("file layer builds");
        tracing::subscriber::with_default(sub, || {
            tracing::info!(kind = "turn", ms = 42u64, "turn done");
        });
        let mut s = String::new();
        std::fs::File::open(&path).unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("\"ms\":42"), "json line must carry the ms field: {s}");
        assert!(s.contains("turn done"));
    }

    #[test]
    fn rolling_stats_tracks_count_last_avg_p90() {
        let mut r = RollingStats::default();
        assert_eq!((r.count(), r.last(), r.avg(), r.p90()), (0, 0, 0, 0));
        for v in [10u64, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            r.record(v);
        }
        assert_eq!(r.count(), 10);
        assert_eq!(r.last(), 100);
        assert_eq!(r.avg(), 55);
        // p90 = value at the 90th percentile index of the sorted window.
        assert_eq!(r.p90(), 100);
    }

    #[test]
    fn rolling_stats_window_caps_at_capacity() {
        let mut r = RollingStats::default();
        for v in 0..200u64 {
            r.record(v);
        }
        // count reflects total records; the window only keeps the last ROLL_CAP.
        assert_eq!(r.count(), 200);
        assert_eq!(r.last(), 199);
        // avg is over the last 64 samples (136..=199), mean = 167.
        assert_eq!(r.avg(), 167);
    }
}
