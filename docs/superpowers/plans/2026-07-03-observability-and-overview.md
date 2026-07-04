# Observability & Overview Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `tracing`-based instrumentation layer feeding a JSON diagnostic file sink and an in-memory aggregator, then surface that aggregate as a new topmost "Overview" zoom page in the TUI.

**Architecture:** `tracing` macros emit events (carrying explicit `ms`/count fields) from any crate. One subscriber built in `main()` fans out to (1) an env-gated JSON file layer and (2) a custom always-on `ObsLayer` that folds events into an `Arc<Mutex<ObsState>>` of bounded rolling aggregates. The render loop snapshots `ObsState` + the economy projection into an `OverviewData`, and at `Zoom::Overview` the conversation pane renders `overview_lines(&data, width)` instead of the transcript.

**Tech Stack:** Rust, `tracing` 0.1, `tracing-subscriber` 0.3 (features `env-filter`, `json`, `fmt`), `ratatui` 0.29, `insta` snapshots, existing `tokio`/`serde_json`.

## Global Constraints

- Baseline window size is **160×40** (cols×rows); snapshot-test the Overview at 160×40, and also at the **100×24** degrade floor. Never blank/overlap at the floor.
- **Static-musl release must keep building.** `tracing`/`tracing-subscriber` are pure Rust — no C deps. Do not enable features pulling C libraries.
- **The TUI owns the terminal** (alternate screen + raw mode). No stdout/stderr writes during a session — diagnostics go to a file only.
- **The observability layer must never panic:** file-sink failure is silent; every mutex access is `.lock().ok()`-guarded; all aggregates are bounded.
- Economy/token data is **not** re-emitted through `tracing` — it is read directly from the existing economy projection.
- No network export, no `Sessions` altitude, no telemetry persistence beyond the optional JSON file (all out of scope).
- Commit messages carry **no** `Co-Authored-By` / co-author trailer.
- Emission uses `tracing` **events with explicit numeric fields** (e.g. `ms`), not span-duration timing — simpler aggregation, testable folds.

## File Structure

**Phase A — instrumentation**
- Create `crates/zoid/src/obs.rs` — the whole layer: `RollingStats`, `ObsState`, `ObsLayer` + field visitor, `ObsHandle`, `init()`, panic hook. One responsibility: capture + aggregate telemetry.
- Modify `Cargo.toml` (workspace deps), `crates/zoid/Cargo.toml`, `crates/zoid-provider/Cargo.toml` — add `tracing`.
- Modify `crates/zoid/src/main.rs` — call `obs::init()`; emit `frame` event; snapshot `ObsState`.
- Modify `crates/zoid/src/agent.rs` — turn + tool events.
- Modify `crates/zoid-provider/src/anthropic.rs`, `ollama.rs` — provider events.
- Delete `crates/zoid/src/dbglog.rs`; remove `zlog!` from `crates/zoid/src/lib.rs`; convert its call sites.

**Phase B — Overview page**
- Modify `crates/zoid-tui/src/state.rs` — `Zoom::Overview`, `zoom_in`/`zoom_out`/`label`.
- Modify `crates/zoid-tui/src/chat.rs` — `Overview` arm in `conversation_view_indexed`.
- Modify `crates/zoid-tui/src/command.rs` — `Command::ShowOverview` + parse.
- Modify `crates/zoid-tui/src/palette.rs` — Overview palette item.
- Create `crates/zoid-tui/src/overview.rs` — `OverviewData` + `overview_lines`.
- Modify `crates/zoid-tui/src/lib.rs` — `pub mod overview;`.
- Modify `crates/zoid/src/main.rs` — `Command::ShowOverview` handler; assemble `OverviewData`; `BodyCache` branch at Overview.

---

## Task 1: tracing deps + subscriber skeleton with JSON file sink

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/zoid/Cargo.toml`
- Create: `crates/zoid/src/obs.rs`
- Modify: `crates/zoid/src/main.rs` (module decl + `obs::init()` call near top of `main`)
- Modify: `crates/zoid/src/lib.rs` (module decl if `obs` needs to be lib-visible; keep it bin-local `mod obs;` in `main.rs`)

**Interfaces:**
- Produces: `pub struct ObsHandle { pub state: std::sync::Arc<std::sync::Mutex<ObsState>> }`; `pub fn init() -> ObsHandle`. (`ObsState` is fully built in Task 3; for this task define it as an empty `#[derive(Default)] pub struct ObsState;` placeholder and expand it in Task 3.)

- [ ] **Step 1: Add workspace deps**

In `Cargo.toml` under `[workspace.dependencies]`, add:

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", default-features = false, features = ["std", "fmt", "env-filter", "json", "registry"] }
```

- [ ] **Step 2: Add deps to the bin**

In `crates/zoid/Cargo.toml` under `[dependencies]`, add:

```toml
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

- [ ] **Step 3: Write the failing smoke test**

Create `crates/zoid/src/obs.rs` with a test module:

```rust
//! Observability layer: a `tracing` subscriber with a JSON file sink (env-gated
//! by `ZOID_LOG`) plus an in-memory aggregator (`ObsState`) that powers the
//! Overview page. Never panics: file-sink failure is silent, locks are
//! `.ok()`-guarded, aggregates are bounded.

use std::sync::{Arc, Mutex};

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
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p zoid obs::tests::json_file_layer_writes_a_line_when_env_set`
Expected: FAIL — `file_only_subscriber` / `install` not defined.

- [ ] **Step 5: Implement the subscriber**

Add to `crates/zoid/src/obs.rs`:

```rust
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

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
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::sync::Mutex::new(file))
        .with_filter(env_filter());
    Some(layer.boxed())
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
```

- [ ] **Step 6: Wire into `main` and declare the module**

In `crates/zoid/src/main.rs`, add near the other `mod` declarations: `mod obs;`. At the very top of `async fn main()` (before the terminal is entered), add:

```rust
    let obs = obs::init();
```

Keep `obs` in scope for the rest of `main` (Task 5+ pass `obs.state` into the loop).

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p zoid obs::tests::json_file_layer_writes_a_line_when_env_set`
Expected: PASS.

- [ ] **Step 8: Build the workspace (musl-relevant deps changed)**

Run: `cargo build --workspace`
Expected: builds clean.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock crates/zoid/Cargo.toml crates/zoid/src/obs.rs crates/zoid/src/main.rs
git commit -m "feat(obs): tracing subscriber skeleton + env-gated JSON file sink"
```

---

## Task 2: RollingStats pure aggregate

**Files:**
- Modify: `crates/zoid/src/obs.rs`

**Interfaces:**
- Produces: `pub struct RollingStats` with `pub fn record(&mut self, sample: u64)`, `pub fn count(&self) -> u64`, `pub fn last(&self) -> u64`, `pub fn avg(&self) -> u64`, `pub fn p90(&self) -> u64`. Fixed capacity `ROLL_CAP = 64`.

- [ ] **Step 1: Write failing tests**

Add to `obs.rs` tests module:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid obs::tests::rolling_stats`
Expected: FAIL — `RollingStats` not defined.

- [ ] **Step 3: Implement**

Add to `obs.rs`:

```rust
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
        // ceil((len)*0.9) - 1, clamped — index of the 90th-percentile sample.
        let idx = (((v.len() as f64) * 0.9).ceil() as usize).saturating_sub(1).min(v.len() - 1);
        v[idx]
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid obs::tests::rolling_stats`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/obs.rs
git commit -m "feat(obs): RollingStats bounded aggregate"
```

---

## Task 3: ObsState aggregates + fold methods

**Files:**
- Modify: `crates/zoid/src/obs.rs`

**Interfaces:**
- Produces: expanded `ObsState` with public fields and fold methods:
  - `pub turn: RollingStats`, `pub provider_ttft: RollingStats`, `pub provider_total: RollingStats`, `pub frame: RollingStats`
  - `pub tools: std::collections::BTreeMap<String, ToolStat>` where `pub struct ToolStat { pub count: u64, pub total_ms: u64 }` with `pub fn avg_ms(&self) -> u64`
  - `pub cache_hits: u64`, `pub cache_total: u64`, `pub proj_rebuilds: u64`, `pub iterations: RollingStats`
  - `pub errors: std::collections::VecDeque<ErrEntry>` where `pub struct ErrEntry { pub ts_ms: i64, pub level: &'static str, pub context: String, pub message: String }`, capped at `MAX_ERR_RING = 20`
  - fold methods: `record_turn(&mut self, ms: u64, iterations: u64)`, `record_tool(&mut self, name: &str, ms: u64)`, `record_provider(&mut self, ttft_ms: u64, total_ms: u64)`, `record_frame(&mut self, ms: u64, cache_hit: bool, proj_rebuilt: bool)`, `record_error(&mut self, ts_ms: i64, level: &'static str, context: String, message: String)`

- [ ] **Step 1: Write failing tests**

Add to `obs.rs` tests:

```rust
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
        assert_eq!(s.errors.front().unwrap().message, format!("err {}", 30 - MAX_ERR_RING));
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid obs::tests::obsstate`
Expected: FAIL — fields/methods not defined.

- [ ] **Step 3: Implement**

Replace the placeholder `ObsState` in `obs.rs` with:

```rust
pub const MAX_ERR_RING: usize = 20;

#[derive(Debug, Default, Clone)]
pub struct ToolStat {
    pub count: u64,
    pub total_ms: u64,
}
impl ToolStat {
    pub fn avg_ms(&self) -> u64 {
        if self.count == 0 { 0 } else { self.total_ms / self.count }
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
        if cache_hit { self.cache_hits += 1; }
        if proj_rebuilt { self.proj_rebuilds += 1; }
    }
    pub fn record_error(&mut self, ts_ms: i64, level: &'static str, context: String, message: String) {
        if self.errors.len() == MAX_ERR_RING {
            self.errors.pop_front();
        }
        self.errors.push_back(ErrEntry { ts_ms, level, context, message });
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid obs::tests::obsstate`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/obs.rs
git commit -m "feat(obs): ObsState bounded aggregates + fold methods"
```

---

## Task 4: ObsLayer — fold tracing events into ObsState

**Files:**
- Modify: `crates/zoid/src/obs.rs`

**Interfaces:**
- Consumes: `ObsState` fold methods (Task 3), `RollingStats` (Task 2).
- Produces: `ObsLayer` (a `tracing_subscriber::Layer`) wired into `install()`. Event contract: an event with field `kind` ∈ {`"turn"`,`"tool"`,`"provider"`,`"frame"`} plus numeric/string fields drives the matching fold; events at level WARN/ERROR record an error entry.

Event field contract (emitters in Tasks 5-6 must match):
- `kind="turn"`: `ms:u64`, `iterations:u64`
- `kind="tool"`: `name:&str`, `ms:u64`, `ok:bool` (ok only affects error ring, not tool timing)
- `kind="provider"`: `ttft_ms:u64`, `total_ms:u64`
- `kind="frame"`: `ms:u64`, `cache_hit:bool`, `proj_rebuilt:bool`
- WARN/ERROR event: fields `ctx:&str`, `message:&str` (or the event message)

- [ ] **Step 1: Write failing test (event → state through a scoped subscriber)**

Add to `obs.rs` tests:

```rust
    #[test]
    fn obslayer_folds_events_into_state() {
        let state = Arc::new(Mutex::new(ObsState::default()));
        let sub = Registry::default().with(ObsLayer { state: state.clone() });
        tracing::subscriber::with_default(sub, || {
            tracing::info!(kind = "tool", name = "shell", ms = 240u64, ok = true, "tool");
            tracing::info!(kind = "turn", ms = 4200u64, iterations = 3u64, "turn");
            tracing::info!(kind = "frame", ms = 7u64, cache_hit = true, proj_rebuilt = false, "frame");
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid obs::tests::obslayer_folds_events_into_state`
Expected: FAIL — `ObsLayer` not defined.

- [ ] **Step 3: Implement the layer + field visitor**

Add to `obs.rs`:

```rust
use tracing::field::{Field, Visit};

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
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
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
            let lvl = if level == tracing::Level::ERROR { "error" } else { "warn" };
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
```

- [ ] **Step 4: Wire ObsLayer into `install()`**

Replace `install()` body with:

```rust
fn install(state: Arc<Mutex<ObsState>>) {
    let file_layer = std::env::var(LOG_ENV)
        .ok()
        .and_then(|p| json_file_layer::<Registry>(std::path::Path::new(&p)));
    let _ = Registry::default()
        .with(ObsLayer { state })
        .with(file_layer)
        .try_init();
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid obs::`
Expected: PASS (all obs tests).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/obs.rs
git commit -m "feat(obs): ObsLayer folds tracing events into ObsState"
```

---

## Task 5: Instrument agent turn/tool + provider stream

**Files:**
- Modify: `crates/zoid/src/agent.rs` (turn: `run_turn_inner`; tool: around each tool execution)
- Modify: `crates/zoid-provider/Cargo.toml` (+ `tracing`)
- Modify: `crates/zoid-provider/src/anthropic.rs`, `crates/zoid-provider/src/ollama.rs` (`stream`)

**Interfaces:**
- Consumes: the Task 4 event field contract.
- Produces: runtime emission — no new Rust API. Verified by an integration test asserting `ObsState` after a scripted turn.

- [ ] **Step 1: Add tracing to the provider crate**

In `crates/zoid-provider/Cargo.toml` `[dependencies]`: `tracing = { workspace = true }`.

- [ ] **Step 2: Emit the provider event**

In `crates/zoid-provider/src/anthropic.rs` `stream()`, wrap the streaming section: capture `let start = std::time::Instant::now();` before sending the request, record `let mut ttft: Option<u64> = None;` and on the first `ProviderEvent` forwarded set `ttft = Some(start.elapsed().as_millis() as u64);`. After the stream loop ends, emit:

```rust
    tracing::info!(
        kind = "provider",
        provider = "anthropic",
        model = %req.model,
        ttft_ms = ttft.unwrap_or(0),
        total_ms = start.elapsed().as_millis() as u64,
        "provider stream complete"
    );
```

Apply the identical pattern in `crates/zoid-provider/src/ollama.rs` `stream()` with `provider = "ollama"`.

- [ ] **Step 3: Emit turn + tool events in the agent loop**

In `crates/zoid/src/agent.rs`:
- At the top of `run_turn_inner`, add `let turn_start = std::time::Instant::now();`.
- Around each tool execution (where the tool's `run`/result is produced in the loop), wrap:

```rust
    let tool_start = std::time::Instant::now();
    let result = /* existing tool execution */;
    tracing::info!(
        kind = "tool",
        name = %tc.name,
        ms = tool_start.elapsed().as_millis() as u64,
        ok = result.is_ok(),
        "tool executed"
    );
```

- At each `break 'turn` / normal loop exit, before returning, emit the turn event (compute `outcome` from the exit path):

```rust
    tracing::info!(
        kind = "turn",
        model = %model,
        iterations,
        ms = turn_start.elapsed().as_millis() as u64,
        outcome = "completed",
        "turn complete"
    );
```

For the provider-error and cap exits, emit with `outcome = "error"` / `outcome = "cap"` respectively, plus a `tracing::warn!(ctx = "provider", message = %msg, "turn error")` at the provider-error site.

- [ ] **Step 4: Write the integration test**

Create `crates/zoid/tests/obs_integration.rs`:

```rust
// A scripted FakeProvider turn must populate ObsState via the ObsLayer.
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread")]
async fn scripted_turn_records_turn_and_tool_stats() {
    // Build a scoped subscriber with ObsLayer over a fresh state, run a scripted
    // turn under it, and assert the aggregates. (Uses the same scripting harness
    // as economy_integration.rs / zoid_testkit.)
    // NOTE: ObsState/ObsLayer are bin-internal; expose them via `pub` in obs.rs
    // and a `pub mod obs;` in lib.rs OR replicate the assertion through a small
    // test-only accessor. See Step 5.
    // ... construct provider script: one tool_call + final text + Done ...
    // ... run_agent_turn under tracing::subscriber::with_default(sub, ...) ...
    // assert state.lock().unwrap().turn.count() == 1
    // assert state.lock().unwrap().tools.contains_key("<tool>")
}
```

Because `run_agent_turn` spawns provider work on tokio tasks, use `tracing::subscriber::set_global_default` is unavailable in a shared test process; instead assert at the unit level in `agent.rs` if the scoped subscriber does not capture spawned-task events. **Decision:** if the multi-thread scoped subscriber proves flaky, drop this integration test and rely on Task 4's unit coverage of the fold + a manual check; do not block the task on it.

- [ ] **Step 5: Make obs types reachable for the test (if keeping Step 4)**

In `crates/zoid/src/lib.rs` add `pub mod obs;` (currently `obs` is `mod obs;` in `main.rs`). Move `mod obs;` out of `main.rs` and reference as `zoid::obs`. Update `main.rs` call to `zoid::obs::init()`.

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p zoid && cargo build --workspace`
Expected: PASS / clean. (If the Step-4 integration test is dropped per the Step-4 decision, the turn/tool/provider emission is still exercised manually and by Task 4's unit test.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid-provider/ crates/zoid/src/lib.rs crates/zoid/src/main.rs crates/zoid/tests/obs_integration.rs
git commit -m "feat(obs): instrument agent turn/tool + provider stream timing"
```

---

## Task 6: Frame event + panic hook; retire dbglog

**Files:**
- Modify: `crates/zoid/src/main.rs` (frame event in render loop; panic hook in `obs::init`)
- Modify: `crates/zoid/src/obs.rs` (panic hook)
- Delete: `crates/zoid/src/dbglog.rs`
- Modify: `crates/zoid/src/lib.rs` (remove `pub mod dbglog;` + `zlog!` macro export)
- Modify: `crates/zoid/src/agent.rs`, `crates/zoid/src/main.rs` (convert/remove `zlog!` call sites)

**Interfaces:**
- Consumes: Task 4 `frame` event contract; Task 1 `init`.
- Produces: panic hook installed by `obs::init`; `frame` events each drawn frame.

- [ ] **Step 1: Emit the frame event**

In `crates/zoid/src/main.rs` render loop, where the frame is drawn and frame timing is already available (the old `zlog!` scroll trace site / around `terminal.draw`), add: capture `let frame_start = std::time::Instant::now();` before `terminal.draw(...)`, and after:

```rust
    tracing::trace!(
        kind = "frame",
        ms = frame_start.elapsed().as_millis() as u64,
        cache_hit = body_cache_was_hit,     // true when BodyCache.refresh reused the cache this frame
        proj_rebuilt = proj_refresh_rebuilt, // the bool returned by app.proj.refresh(...)
        "frame"
    );
```

`proj_rebuilt` is the existing return of `app.proj.refresh(&app.events)` (main.rs ~627). `body_cache_was_hit` = negation of "BodyCache.refresh rebuilt this frame"; add a `bool` return to `BodyCache::refresh` (true when the key was unchanged / cache reused) if not already present.

- [ ] **Step 2: Install the panic hook**

In `crates/zoid/src/obs.rs` `init()`, after `install(...)`, add:

```rust
    install_panic_hook();
```

and:

```rust
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| l.to_string()).unwrap_or_default();
        tracing::error!(ctx = "panic", message = %info, location = %loc, "panic");
        prev(info);
    }));
}
```

- [ ] **Step 3: Convert/remove zlog! sites and delete dbglog**

- Replace each `zoid::zlog!(...)` / `crate::zlog!(...)` call in `agent.rs` and `main.rs` with either deletion (scroll traces) or `tracing::debug!(...)` (keep genuinely useful diagnostics). The tool/ask_user traces in `agent.rs` become `tracing::debug!`.
- Delete `crates/zoid/src/dbglog.rs`.
- In `crates/zoid/src/lib.rs`, remove `pub mod dbglog;` and the `#[macro_export] macro_rules! zlog`.

- [ ] **Step 4: Build + test the workspace**

Run: `cargo build --workspace && cargo test -p zoid`
Expected: builds clean (no references to `zlog!`/`dbglog` remain), tests pass.

- [ ] **Step 5: Manual smoke (documented, not automated)**

Run: `ZOID_LOG=/tmp/zoid-obs.json RUST_LOG=trace cargo run -p zoid` briefly, then confirm `/tmp/zoid-obs.json` has JSON lines with `kind":"frame"` and `kind":"turn"`. (Manual — the TUI can't be driven headlessly here.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(obs): frame events + panic hook; retire dbglog/zlog"
```

---

## Task 7: Zoom::Overview altitude

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`Zoom`, `label`, `zoom_in`, `zoom_out`)
- Modify: `crates/zoid-tui/src/chat.rs` (`conversation_view_indexed` match)

**Interfaces:**
- Produces: `Zoom::Overview` variant; `Zoom::label(Overview) == "overview"`; `zoom_out` reaches Overview from Summary; `zoom_in` leaves Overview to Summary.

- [ ] **Step 1: Write failing tests**

In `crates/zoid-tui/src/state.rs` tests module:

```rust
    #[test]
    fn overview_is_the_topmost_altitude() {
        let mut s = ShellState::new(); // starts Normal
        s.zoom_out(); // Normal -> Summary
        assert_eq!(s.zoom, Zoom::Summary);
        s.zoom_out(); // Summary -> Overview
        assert_eq!(s.zoom, Zoom::Overview);
        s.zoom_out(); // saturates
        assert_eq!(s.zoom, Zoom::Overview);
        assert_eq!(s.zoom.label(), "overview");
        s.zoom_in(); // Overview -> Summary
        assert_eq!(s.zoom, Zoom::Summary);
    }

    #[test]
    fn entering_overview_resets_scroll() {
        let mut s = ShellState::new();
        s.zoom_out();               // Summary
        s.conversation_scroll = 9;
        s.zoom_out();               // -> Overview, must re-anchor
        assert_eq!(s.conversation_scroll, 0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui state::tests::overview`
Expected: FAIL — no `Zoom::Overview`.

- [ ] **Step 3: Implement**

In `state.rs`:
- Add `Overview` as the first variant: `pub enum Zoom { Overview, Summary, Normal, Detail }`.
- `label`: add `Zoom::Overview => "overview"`.
- `zoom_out`: change to

```rust
    pub fn zoom_out(&mut self) {
        let next = match self.zoom {
            Zoom::Detail => Zoom::Normal,
            Zoom::Normal => Zoom::Summary,
            Zoom::Summary | Zoom::Overview => {
                if self.zoom == Zoom::Summary { Zoom::Overview } else { Zoom::Overview }
            }
        };
        if next != self.zoom { self.conversation_scroll = 0; }
        self.zoom = next;
    }
```

(Equivalently: `Zoom::Summary => Zoom::Overview, Zoom::Overview => Zoom::Overview`.)
- `zoom_in`: add the Overview→Summary edge:

```rust
    pub fn zoom_in(&mut self) {
        let next = match self.zoom {
            Zoom::Overview => Zoom::Summary,
            Zoom::Summary => Zoom::Normal,
            Zoom::Normal | Zoom::Detail => Zoom::Detail,
        };
        if next != self.zoom { self.conversation_scroll = 0; }
        self.zoom = next;
    }
```

- [ ] **Step 4: Add the Overview arm to `conversation_view_indexed`**

In `crates/zoid-tui/src/chat.rs`, the `match view.zoom` in `conversation_view_indexed` must stay exhaustive. Add:

```rust
        Zoom::Overview => {
            // Overview is not a transcript view — the bin renders it via
            // `overview::overview_lines`, bypassing this builder. Return empty
            // so the match is exhaustive and any accidental call is harmless.
            Vec::new()
        }
```

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p zoid-tui state::tests::overview && cargo build -p zoid-tui`
Expected: PASS / clean.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/chat.rs
git commit -m "feat(tui): Zoom::Overview topmost altitude"
```

---

## Task 8: Overview command + palette entry

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (`Command::ShowOverview` + `parse_command`)
- Modify: `crates/zoid-tui/src/palette.rs` (`all_items` entry)
- Modify: `crates/zoid/src/main.rs` (`exec_command` handler)

**Interfaces:**
- Consumes: `Zoom::Overview` (Task 7).
- Produces: `Command::ShowOverview`; `parse_command(":overview") == Command::ShowOverview`; a selectable palette row labelled "Overview".

- [ ] **Step 1: Write failing tests**

In `crates/zoid-tui/src/command.rs` tests:

```rust
    #[test]
    fn parses_overview_command() {
        assert_eq!(parse_command(":overview"), Command::ShowOverview);
        assert_eq!(parse_command("overview"), Command::ShowOverview);
    }
```

In `crates/zoid-tui/src/palette.rs` tests (add a module if none):

```rust
    #[test]
    fn palette_has_overview_entry() {
        let items = all_items(crate::state::Mode::Chat);
        assert!(items.iter().any(|i| i.label == "Overview"
            && i.command == Some(Command::ShowOverview)));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui parses_overview_command palette_has_overview_entry`
Expected: FAIL — no `Command::ShowOverview`.

- [ ] **Step 3: Implement command**

In `command.rs`: add variant `ShowOverview` to `Command`, and in `parse_command`'s match add `"overview" => Command::ShowOverview,`.

- [ ] **Step 4: Implement palette entry**

In `palette.rs` `all_items`, add to the `context` group (or a new `view` group) a selectable item:

```rust
        PaletteItem {
            group: "view".to_string(),
            icon: glyph::SETTINGS, // reuse an existing glyph; no new token needed
            label: "Overview",
            hint: "session metrics · tokens · timing · errors",
            keybind: ":overview",
            command: Some(Command::ShowOverview),
        },
```

- [ ] **Step 5: Handle it in the bin**

In `crates/zoid/src/main.rs` `exec_command`, add an arm:

```rust
        Command::ShowOverview => {
            app.shell.zoom = zoid_tui::state::Zoom::Overview;
            app.shell.conversation_scroll = 0;
            Ok(false)
        }
```

(Place beside `Command::OpenConfig`. Confirm `Zoom` is importable as `zoid_tui::state::Zoom` or via the existing re-export.)

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p zoid-tui && cargo build --workspace`
Expected: PASS / clean.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid-tui/src/palette.rs crates/zoid/src/main.rs
git commit -m "feat(tui): :overview command + palette entry jump to Overview"
```

---

## Task 9: OverviewData + overview_lines (layout C)

**Files:**
- Create: `crates/zoid-tui/src/overview.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod overview;`)

**Interfaces:**
- Produces:
  - `pub struct OverviewData { pub session_id: String, pub model: String, pub provider: String, pub uptime: String, pub turns: u64, pub tok_in: u64, pub tok_out: u64, pub tok_total: u64, pub cache_read: u64, pub cache_hit_pct: u8, pub spark: String, pub turn_last_ms: u64, pub turn_avg_ms: u64, pub turn_p90_ms: u64, pub ttft_ms: u64, pub stream_ms: u64, pub iter_avg: u64, pub tools: Vec<(String, u64, u64)>, pub frame_avg_ms: u64, pub frame_p90_ms: u64, pub frame_max_ms: u64, pub render_cache_pct: u8, pub proj_rebuilds: u64, pub event_count: u64, pub errors: Vec<(String, String)> }` (each error tuple = (age+level prefix, message))
  - `pub fn overview_lines(data: &OverviewData, width: usize) -> Vec<ratatui::text::Line<'static>>` — layout C.

- [ ] **Step 1: Write the snapshot test scaffolding**

Create `crates/zoid-tui/src/overview.rs`:

```rust
//! The Overview zoom page: a whole-session metrics dashboard rendered in the
//! conversation pane at `Zoom::Overview`. `overview_lines` is pure; the bin
//! assembles `OverviewData` from the obs aggregate snapshot + economy.

use ratatui::text::Line;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverviewData {
    pub session_id: String,
    pub model: String,
    pub provider: String,
    pub uptime: String,
    pub turns: u64,
    pub tok_in: u64,
    pub tok_out: u64,
    pub tok_total: u64,
    pub cache_read: u64,
    pub cache_hit_pct: u8,
    pub spark: String,
    pub turn_last_ms: u64,
    pub turn_avg_ms: u64,
    pub turn_p90_ms: u64,
    pub ttft_ms: u64,
    pub stream_ms: u64,
    pub iter_avg: u64,
    pub tools: Vec<(String, u64, u64)>, // (name, count, avg_ms)
    pub frame_avg_ms: u64,
    pub frame_p90_ms: u64,
    pub frame_max_ms: u64,
    pub render_cache_pct: u8,
    pub proj_rebuilds: u64,
    pub event_count: u64,
    pub errors: Vec<(String, String)>, // (prefix, message)
}

#[cfg(test)]
fn sample() -> OverviewData {
    OverviewData {
        session_id: "a3f2".into(), model: "glm-5.2:cloud".into(), provider: "ollama".into(),
        uptime: "12m".into(), turns: 8,
        tok_in: 48200, tok_out: 6100, tok_total: 54300, cache_read: 31000, cache_hit_pct: 64,
        spark: "▁▂▃▅▇▆▄".into(),
        turn_last_ms: 4200, turn_avg_ms: 3800, turn_p90_ms: 7100,
        ttft_ms: 600, stream_ms: 3100, iter_avg: 3,
        tools: vec![("read_file".into(), 14, 12), ("shell".into(), 6, 240), ("edit_file".into(), 3, 18)],
        frame_avg_ms: 7, frame_p90_ms: 11, frame_max_ms: 16, render_cache_pct: 98,
        proj_rebuilds: 42, event_count: 1204,
        errors: vec![
            ("⚠ 12m provider".into(), "HTTP 429: rate limited".into()),
            ("⛔ 3m shell".into(), "exit 1: ./deploy.sh: no such file".into()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_dashboard_160x40() {
        // conversation content width at 160×40 baseline ≈ 110 cols.
        let lines = overview_lines(&sample(), 110);
        let text: String = lines.iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>().join("\n");
        insta::assert_snapshot!("overview_160x40", text);
    }

    #[test]
    fn overview_dashboard_100x24_floor() {
        // degrade floor: ~51 cols. Must not panic or overflow.
        let lines = overview_lines(&sample(), 51);
        assert!(!lines.is_empty());
        insta::assert_snapshot!("overview_100x24", 
            lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
                .collect::<Vec<_>>().join("\n"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui overview::tests`
Expected: FAIL — `overview_lines` not defined.

- [ ] **Step 3: Implement `overview_lines` (layout C)**

Implement the function producing the layout C structure: header line, KPI strip between heavy rules, a two-column body (ECONOMY+TOOLS left, TIMING+RUNTIME right) built by composing left/right cell strings per row and joining with a ` │ ` separator when `width >= 90`, else stacking the two columns vertically (degrade path for the 100×24 floor), then an ERRORS band. Use `zoid_tui::tokens::{glyph,color}` for styling (accent for headers via `color::CHAT_ACCENT`, `color::DIM` for rules/separators, `color::OK`/`color::WARN`/`color::ERROR` for figures). Compose `Line::from(vec![Span::styled(...), ...])`. Column width = `(width - 3) / 2` when wide. Keep every produced line ≤ `width` chars (truncate tool names / messages with `…` when needed).

Add `pub mod overview;` to `crates/zoid-tui/src/lib.rs`.

- [ ] **Step 4: Generate snapshots**

Run: `cargo test -p zoid-tui overview::tests`
Expected: FAIL first run (insta writes `.snap.new`). Review the two `.snap.new` files under `crates/zoid-tui/src/snapshots/` (or wherever insta is configured) — confirm the 160×40 output matches the approved layout C and the 100×24 output degrades to stacked columns without overflow. Accept:

```bash
cargo insta accept   # or: mv the .snap.new files to .snap
```

- [ ] **Step 5: Re-run to confirm green**

Run: `cargo test -p zoid-tui overview::tests`
Expected: PASS (2 snapshot tests).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/overview.rs crates/zoid-tui/src/lib.rs crates/zoid-tui/src/snapshots/
git commit -m "feat(tui): Overview dashboard overview_lines (layout C) + snapshots"
```

---

## Task 10: Bin wiring — assemble OverviewData, render at Overview

**Files:**
- Modify: `crates/zoid/src/main.rs` (assemble `OverviewData` from `obs.state` + economy; branch `BodyCache`/pre-draw body build at `Zoom::Overview`)

**Interfaces:**
- Consumes: `obs::ObsHandle.state` (Task 1/3), `overview::{OverviewData, overview_lines}` (Task 9), economy projection (`app.proj`), `Zoom::Overview` (Task 7).

- [ ] **Step 1: Add an OverviewData assembler in the bin**

In `crates/zoid/src/main.rs`, add a helper that snapshots the aggregate + economy into `OverviewData`:

```rust
fn build_overview_data(app: &App, obs: &std::sync::Arc<std::sync::Mutex<zoid::obs::ObsState>>) -> zoid_tui::overview::OverviewData {
    use zoid_tui::overview::OverviewData;
    // Economy split comes from the same projection source the context drawer uses.
    let ledger = zoid_core::economy::token_ledger(&app.events); // fields: input, output, cached, total
    // Prompt-cache hit rate = cache-read as a % of input tokens.
    let cache_hit_pct = if ledger.input == 0 { 0 } else { (ledger.cached * 100 / ledger.input) as u8 };
    // Per-turn cache sparkline: map the churn timeline's per-turn `cached` values
    // onto the glyph::SPARK ramp exactly as render.rs's context drawer does.
    let spark = sparkline_from_churn(&app.proj.churn); // reuse the context-drawer helper (see Step 1a)

    let s = match obs.lock() { Ok(s) => s, Err(_) => return OverviewData::default() };
    OverviewData {
        session_id: last4(&app.session_id.to_string()),
        model: app.shell.model.clone(),
        provider: app.shell.provider.clone(),
        uptime: fmt_duration(app.session_started_ms, now_ms()),
        turns: s.turn.count(),
        tok_in: ledger.input,
        tok_out: ledger.output,
        tok_total: ledger.total,
        cache_read: ledger.cached,
        cache_hit_pct,
        spark,
        turn_last_ms: s.turn.last(), turn_avg_ms: s.turn.avg(), turn_p90_ms: s.turn.p90(),
        ttft_ms: s.provider_ttft.avg(), stream_ms: s.provider_total.avg(), iter_avg: s.iterations.avg(),
        tools: s.tools.iter().map(|(k, v)| (k.clone(), v.count, v.avg_ms())).collect(),
        frame_avg_ms: s.frame.avg(), frame_p90_ms: s.frame.p90(), frame_max_ms: s.frame.window_max(),
        render_cache_pct: if s.cache_total == 0 { 0 } else { (s.cache_hits * 100 / s.cache_total) as u8 },
        proj_rebuilds: s.proj_rebuilds,
        event_count: app.events.len() as u64,
        errors: s.errors.iter()
            .map(|e| (format!("{} {}", if e.level == "error" { '⛔' } else { '⚠' }, e.context), e.message.clone()))
            .collect(),
    }
}

/// Last 4 chars of the session id (matches the mockup's `a3f2`).
fn last4(s: &str) -> String {
    let v: Vec<char> = s.chars().collect();
    v[v.len().saturating_sub(4)..].iter().collect()
}
```

Two supporting additions:
- **Step 1a — reuse the sparkline helper.** `render.rs`'s context drawer already maps a per-turn series onto `glyph::SPARK`. Extract that mapping into a small pure fn (e.g. `pub fn sparkline_from_churn(c: &ChurnTimeline) -> String` in `zoid-tui` or a local bin helper) and call it from both the context drawer and `build_overview_data` (DRY). If extraction is noisy, replicate the ≤10-line mapping locally in the bin — do not invent a new ramp.
- **Step 1b — add `RollingStats::window_max()`** to `obs.rs` (returns the max of the window, 0 if empty), with a one-line unit test, since Task 2 didn't expose it.

Two distinct "cache" figures, labelled distinctly in the dashboard: `cache_hit_pct` = **prompt-cache** read ÷ input (economy); `render_cache_pct` = **body-render** cache hit ratio (obs frame events).

- [ ] **Step 2: Branch the body build at Overview**

In the pre-draw body section of `main.rs` (~line 1028, where `app.body_cache.refresh(BodyKey{...}, msgs, width)` runs), branch:

```rust
    let body: &[Line] = if app.shell.zoom == zoid_tui::state::Zoom::Overview {
        let data = build_overview_data(&app, &obs.state);
        app.overview_body = zoid_tui::overview::overview_lines(&data, width);
        &app.overview_body
    } else {
        app.body_cache.refresh(/* existing BodyKey */, msgs, width);
        &app.body_cache.body
    };
```

Add `overview_body: Vec<Line<'static>>` to `App` (init `Vec::new()` at both construction sites). At Overview, `msg_starts` is empty → cross-zoom anchoring naturally skipped (it already guards on the anchor being present). Ensure `max_scroll` uses `body.len()` as for other altitudes (the scrollbar reuse just works).

- [ ] **Step 3: Build + existing tests**

Run: `cargo build --workspace && cargo test --workspace`
Expected: builds clean; all existing tests pass (no snapshot regressions — the Overview body only renders at the new altitude).

- [ ] **Step 4: Manual verification (documented)**

Run `cargo run -p zoid`, zoom out twice (or `:overview`) → the dashboard renders in the conversation pane with the rail beside it at 160×40; run a couple of turns and a tool, confirm tokens/tool timings/errors populate; `+`/zoom-in returns to Summary. (Manual — TUI not headless-testable.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(tui): render Overview dashboard from obs + economy snapshot"
```

---

## Self-Review Notes

- **Spec coverage:** A.1 emission points → Tasks 5-6; A.2 subscriber + RUST_LOG default → Task 1; A.3 ObsState/ObsLayer → Tasks 2-4; A.4 retire dbglog → Task 6; B.1 zoom → Task 7; B.2 palette/command → Task 8; B.3 overview_lines + snapshots (160×40 + 100×24) → Task 9; B.4 OverviewData seam + bin wiring → Task 10; panic hook → Task 6; error handling (lock().ok(), bounded) → Tasks 3-4; phasing A(1-6)/B(7-10).
- **Open finalizations flagged in the spec:** `ROLL_CAP = 64`, `MAX_ERR_RING = 20` are now pinned in Tasks 2-3.
- **Known soft spot:** Task 5's integration test may be dropped if a scoped subscriber can't observe events from tokio-spawned provider tasks; the fold logic is already unit-covered in Task 4, so this does not reduce correctness confidence. Task 10 Step 1 leaves the exact economy-field mapping to the implementer to wire from the existing sparkline source (named, not guessed — reuse the context drawer's data path).
