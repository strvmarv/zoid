//! Observability layer: a `tracing` subscriber with a JSON file sink (env-gated
//! by `ZOID_LOG`) plus an in-memory aggregator (`ObsState`) that powers the
//! Overview page. Never panics: file-sink failure is silent, locks are
//! `.ok()`-guarded, aggregates are bounded.

use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

pub const MAX_ERR_RING: usize = 20;

#[derive(Debug, Default, Clone)]
pub struct ToolStat {
    pub count: u64,
    pub total_ms: u64,
}
impl ToolStat {
    pub fn avg_ms(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ms / self.count
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrEntry {
    pub ts_ms: i64,
    pub level: &'static str,
    pub context: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ObsState {
    pub turn: RollingStats,
    pub iterations: RollingStats,
    pub provider_ttft: RollingStats,
    pub provider_total: RollingStats,
    pub frame: RollingStats,
    pub tools: std::collections::BTreeMap<String, ToolStat>,
    pub cache_hits: u64,
    pub cache_total: u64,
    pub proj_rebuilds: u64,
    pub errors: std::collections::VecDeque<ErrEntry>,
}

impl ObsState {
    pub fn record_turn(&mut self, ms: u64, iterations: u64) {
        self.turn.record(ms);
        self.iterations.record(iterations);
    }
    pub fn record_tool(&mut self, name: &str, ms: u64) {
        let e = self.tools.entry(name.to_string()).or_default();
        e.count += 1;
        e.total_ms += ms;
    }
    pub fn record_provider(&mut self, ttft_ms: u64, total_ms: u64) {
        self.provider_ttft.record(ttft_ms);
        self.provider_total.record(total_ms);
    }
    pub fn record_frame(&mut self, ms: u64, cache_hit: bool, proj_rebuilt: bool) {
        self.frame.record(ms);
        self.cache_total += 1;
        if cache_hit {
            self.cache_hits += 1;
        }
        if proj_rebuilt {
            self.proj_rebuilds += 1;
        }
    }
    pub fn record_error(
        &mut self,
        ts_ms: i64,
        level: &'static str,
        context: String,
        message: String,
    ) {
        if self.errors.len() == MAX_ERR_RING {
            self.errors.pop_front();
        }
        self.errors.push_back(ErrEntry {
            ts_ms,
            level,
            context,
            message,
        });
    }
}

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
    pub fn count(&self) -> u64 {
        self.count
    }
    pub fn last(&self) -> u64 {
        self.last
    }
    pub fn avg(&self) -> u64 {
        if self.window.is_empty() {
            return 0;
        }
        (self.window.iter().sum::<u64>()) / self.window.len() as u64
    }
    pub fn p90(&self) -> u64 {
        if self.window.is_empty() {
            return 0;
        }
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

/// Collects the fields of one event into a flat record we can fold.
#[derive(Default)]
struct FieldGrab {
    kind: Option<String>,
    name: Option<String>,
    ctx: Option<String>,
    message: Option<String>,
    ms: u64,
    ttft_ms: u64,
    total_ms: u64,
    iterations: u64,
    cache_hit: bool,
    proj_rebuilt: bool,
}

impl Visit for FieldGrab {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "ms" => self.ms = value,
            "ttft_ms" => self.ttft_ms = value,
            "total_ms" => self.total_ms = value,
            "iterations" => self.iterations = value,
            _ => {}
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        match field.name() {
            "cache_hit" => self.cache_hit = value,
            "proj_rebuilt" => self.proj_rebuilt = value,
            _ => {}
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "kind" => self.kind = Some(value.to_string()),
            "name" => self.name = Some(value.to_string()),
            "ctx" => self.ctx = Some(value.to_string()),
            "message" => self.message = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // The implicit event message arrives as the `message` field via Debug.
        if field.name() == "message" && self.message.is_none() {
            self.message = Some(format!("{value:?}"));
        }
    }
}

pub struct ObsLayer {
    pub state: Arc<Mutex<ObsState>>,
}

impl<S: tracing::Subscriber> Layer<S> for ObsLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut g = FieldGrab::default();
        event.record(&mut g);
        let Ok(mut s) = self.state.lock() else { return }; // poisoned → skip, never panic
        match g.kind.as_deref() {
            Some("turn") => s.record_turn(g.ms, g.iterations),
            Some("tool") => s.record_tool(g.name.as_deref().unwrap_or("?"), g.ms),
            Some("provider") => s.record_provider(g.ttft_ms, g.total_ms),
            Some("frame") => s.record_frame(g.ms, g.cache_hit, g.proj_rebuilt),
            _ => {}
        }
        let level = *event.metadata().level();
        if level == tracing::Level::WARN || level == tracing::Level::ERROR {
            let lvl = if level == tracing::Level::ERROR {
                "error"
            } else {
                "warn"
            };
            s.record_error(
                now_ms(),
                lvl,
                g.ctx.unwrap_or_default(),
                g.message.unwrap_or_default(),
            );
        }
    }
}

/// Epoch millis (kept local so obs has no cross-module dep).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Install the global subscriber: ObsLayer (always on) + optional JSON file
/// layer (when `ZOID_LOG` is set). Safe to call once.
fn install(state: Arc<Mutex<ObsState>>) {
    type Base = tracing_subscriber::layer::Layered<ObsLayer, Registry>;
    let file_layer = std::env::var(LOG_ENV)
        .ok()
        .and_then(|p| json_file_layer::<Base>(std::path::Path::new(&p)));
    let _ = Registry::default()
        .with(ObsLayer { state })
        .with(file_layer)
        .try_init();
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
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert!(
            s.contains("\"ms\":42"),
            "json line must carry the ms field: {s}"
        );
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

    #[test]
    fn obsstate_folds_tools_and_caps_errors() {
        let mut s = ObsState::default();
        s.record_tool("read_file", 10);
        s.record_tool("read_file", 20);
        s.record_tool("shell", 240);
        assert_eq!(s.tools["read_file"].count, 2);
        assert_eq!(s.tools["read_file"].avg_ms(), 15);
        assert_eq!(s.tools["shell"].avg_ms(), 240);

        for i in 0..30 {
            s.record_error(i, "warn", "ctx".into(), format!("err {i}"));
        }
        assert_eq!(s.errors.len(), MAX_ERR_RING);
        // oldest dropped: the ring keeps the most recent MAX_ERR_RING.
        assert_eq!(s.errors.back().unwrap().message, "err 29");
        assert_eq!(
            s.errors.front().unwrap().message,
            format!("err {}", 30 - MAX_ERR_RING)
        );
    }

    #[test]
    fn obsstate_folds_frame_cache_ratio() {
        let mut s = ObsState::default();
        s.record_frame(7, true, false);
        s.record_frame(11, true, true);
        s.record_frame(16, false, false);
        assert_eq!(s.frame.count(), 3);
        assert_eq!(s.cache_total, 3);
        assert_eq!(s.cache_hits, 2);
        assert_eq!(s.proj_rebuilds, 1);
    }

    #[test]
    fn obslayer_folds_events_into_state() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        let sub = Registry::default().with(ObsLayer {
            state: state.clone(),
        });
        tracing::subscriber::with_default(sub, || {
            tracing::info!(
                kind = "tool",
                name = "shell",
                ms = 240u64,
                ok = true,
                "tool"
            );
            tracing::info!(kind = "turn", ms = 4200u64, iterations = 3u64, "turn");
            tracing::info!(
                kind = "frame",
                ms = 7u64,
                cache_hit = true,
                proj_rebuilt = false,
                "frame"
            );
            tracing::warn!(ctx = "provider", message = "HTTP 429", "provider error");
        });
        let s = state.lock().unwrap();
        assert_eq!(s.tools["shell"].avg_ms(), 240);
        assert_eq!(s.turn.last(), 4200);
        assert_eq!(s.iterations.last(), 3);
        assert_eq!(s.frame.last(), 7);
        assert_eq!(s.cache_hits, 1);
        assert_eq!(s.errors.len(), 1);
        assert_eq!(s.errors.back().unwrap().context, "provider");
    }
}
