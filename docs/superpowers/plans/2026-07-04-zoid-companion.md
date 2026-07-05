# zoid Companion Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, lazy-started, localhost-only HTTP server (`zoid-companion`) that gives a running zoid session a browser view: a live metrics dashboard plus a single agent-pushed HTML card.

**Architecture:** A new leaf crate `zoid-companion` holds a synchronous `CompanionHub` (`Mutex` + `Condvar` + an `enabled` flag) and a blocking `tiny_http` server. The async render loop in the `zoid` bin maps its existing per-frame projections into a `DashboardSnapshot` and calls `hub.publish_snapshot`; a new emitting `show` tool calls `hub.publish_card`. SSE streams both to the browser. The server runs entirely on `std::thread`s and never touches tokio.

**Tech Stack:** Rust 2021, `tiny_http` 0.12 (blocking HTTP), `serde`/`serde_json`, Server-Sent Events, `std::sync::{Mutex, Condvar, atomic}`.

## Global Constraints

- **Bind address:** `127.0.0.1` only. Never `0.0.0.0`.
- **`zoid-companion` dependencies:** exactly `tiny_http`, `serde`, `serde_json`. It MUST NOT depend on `zoid-core`, `zoid-tui`, `tokio`, `ulid`, or any C/OpenSSL-linked library. The dependency arrow is one-way: `zoid → zoid-companion`.
- **Token:** minted in the bin with `ulid::Ulid::new().to_string()` and passed into `start`. 128-bit, URL-safe Crockford base32.
- **CSP header** on the shell page, verbatim (as shipped — supersedes the original `connect-src 'none'`, which blocked the dashboard's own SSE + inline script): `default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; form-action 'self'; base-uri 'self'` (shell JS is served as same-origin `app.js`; see the design doc's CSP-correction note)
- **Token/404:** any missing or wrong token, or any unknown path, returns HTTP `404` with an empty body. Never `401`/`403`.
- **Default off:** the server starts only on explicit enable (palette `companion`, `--companion` flag). `show` while disabled is a no-op ack; it never auto-starts.
- **No tokio in the server:** all server threads are `std::thread`; all cross-thread state moves through `CompanionHub`.
- **Edition/workspace:** `edition.workspace = true`, `version.workspace = true`, added to root `Cargo.toml` `members`; deps via workspace inheritance.
- **Commit messages:** no `Co-Authored-By` or any co-author trailer (user directive).
- **Test gate:** every task runs `cargo test -p <crate>` for the crate(s) it touched and must be green before commit.

---

## File Structure

**New — `crates/zoid-companion/`**
- `Cargo.toml` — leaf crate manifest (`tiny_http`, `serde`, `serde_json`).
- `src/lib.rs` — module wiring + re-exports (`CompanionHub`, `Frame`, `DashboardSnapshot`, `TierRow`, `CompanionServer`, `start`).
- `src/snapshot.rs` — `DashboardSnapshot`, `TierRow` (serde, `PartialEq`).
- `src/hub.rs` — `CompanionHub`, `Frame`, `Latest`.
- `src/server.rs` — `CompanionServer`, `start`, routing, token gate, CSP, `SseReader`.
- `src/shell.html` — embedded static UI (inline CSS/JS).

**Modified**
- Root `Cargo.toml` — add member + `tiny_http` workspace dep.
- `crates/zoid/Cargo.toml` — add `zoid-companion` path dep.
- `crates/zoid-tools/src/show.rs` (new) + `crates/zoid-tools/src/lib.rs` — the `show` tool.
- `crates/zoid/src/invoke_skill.rs` — register `show` in `chat_tools`.
- `crates/zoid/src/agent.rs` — thread `Arc<CompanionHub>`; `show` emitting arm; `companion_show` helper; `AgentUpdate` unchanged.
- `crates/zoid/src/main.rs` — `App.companion`/`companion_hub`; render-loop publish; `dashboard_snapshot`/`heat_rank`; `enable_companion`/`disable_companion`/`open_url`; config/env read; `--companion` dispatch; call-site threading.
- `crates/zoid/src/cli.rs` — `--companion` parse + help line.
- `crates/zoid-tui/src/command.rs` — `CompanionEnable`/`CompanionDisable` + `parse_command`.
- `crates/zoid-core/src/config.rs` — `CompanionConfig` + `PartialCompanion` + merge.

---

## Task 1: Scaffold `zoid-companion` crate + `DashboardSnapshot`

**Files:**
- Create: `crates/zoid-companion/Cargo.toml`
- Create: `crates/zoid-companion/src/lib.rs`
- Create: `crates/zoid-companion/src/snapshot.rs`
- Modify: `Cargo.toml` (root — `members` + `[workspace.dependencies]`)

**Interfaces:**
- Produces: `zoid_companion::DashboardSnapshot { session_name: String, model: String, provider: String, cwd: String, ctx_used: u64, ctx_ceiling: u64, session_tokens: u64, cached_tokens: u64, cache_supported: bool, tasks_len: usize, busy: bool, tiers: Vec<TierRow>, churn: Vec<u64>, updated_ms: i64 }` and `zoid_companion::TierRow { label: String, tokens: u64, heat: u8, cold: bool, pinned: bool }`. Both derive `Clone, Serialize, PartialEq`.

- [ ] **Step 1: Add the crate to the workspace**

In root `Cargo.toml`, change the `members` line to include the new crate:

```toml
members = ["crates/zoid-core", "crates/zoid-model", "crates/zoid-provider", "crates/zoid-tui", "crates/zoid-tools", "crates/zoid-syntax", "crates/zoid", "crates/zoid-testkit", "crates/zoid-companion"]
```

Add `tiny_http` to `[workspace.dependencies]` (append after the `toml = "0.8"` line):

```toml
tiny_http = "0.12"
```

- [ ] **Step 2: Create the crate manifest**

`crates/zoid-companion/Cargo.toml`:

```toml
[package]
name = "zoid-companion"
version.workspace = true
edition.workspace = true
repository.workspace = true

[dependencies]
tiny_http = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 3: Write the failing test for `DashboardSnapshot`**

Create `crates/zoid-companion/src/snapshot.rs`:

```rust
//! The serializable projection the browser dashboard renders. Plain serde,
//! deliberately free of any `zoid-core` types so this crate stays a leaf.

use serde::Serialize;

#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct TierRow {
    pub label: String,
    pub tokens: u64,
    /// Heat rank: 2 = hot, 1 = warm, 0 = cold. Mirrors `cold` for convenience.
    pub heat: u8,
    pub cold: bool,
    pub pinned: bool,
}

#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct DashboardSnapshot {
    pub session_name: String,
    pub model: String,
    pub provider: String,
    pub cwd: String,
    pub ctx_used: u64,
    pub ctx_ceiling: u64,
    pub session_tokens: u64,
    pub cached_tokens: u64,
    pub cache_supported: bool,
    pub tasks_len: usize,
    pub busy: bool,
    pub tiers: Vec<TierRow>,
    /// Per-turn token series; the browser draws the SVG sparkline from it.
    pub churn: Vec<u64>,
    pub updated_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serializes_churn_as_json_array() {
        let snap = DashboardSnapshot {
            session_name: "demo".into(),
            model: "glm-5.2:cloud".into(),
            provider: "ollama".into(),
            cwd: "/home/x/zoid".into(),
            ctx_used: 312_000,
            ctx_ceiling: 384_000,
            session_tokens: 100,
            cached_tokens: 20,
            cache_supported: true,
            tasks_len: 3,
            busy: false,
            tiers: vec![TierRow {
                label: "system".into(),
                tokens: 1200,
                heat: 2,
                cold: false,
                pinned: true,
            }],
            churn: vec![10, 20, 30],
            updated_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"churn\":[10,20,30]"), "got: {json}");
        assert!(json.contains("\"heat\":2"), "got: {json}");
    }
}
```

- [ ] **Step 4: Create `lib.rs` exposing the module**

`crates/zoid-companion/src/lib.rs`:

```rust
//! Optional localhost companion server for a running zoid session: a live
//! metrics dashboard plus a single agent-pushed HTML card, streamed over SSE.
//! Runs entirely on std threads — no tokio.

pub mod snapshot;

pub use snapshot::{DashboardSnapshot, TierRow};
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid-companion`
Expected: PASS (`snapshot_serializes_churn_as_json_array`), crate builds.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zoid-companion/Cargo.toml crates/zoid-companion/src/lib.rs crates/zoid-companion/src/snapshot.rs
git commit -m "feat(companion): scaffold zoid-companion crate + DashboardSnapshot"
```

---

## Task 2: `CompanionHub`

**Files:**
- Create: `crates/zoid-companion/src/hub.rs`
- Modify: `crates/zoid-companion/src/lib.rs`

**Interfaces:**
- Consumes: `DashboardSnapshot` (Task 1).
- Produces:
  - `CompanionHub::new() -> Arc<CompanionHub>`
  - `CompanionHub::publish_snapshot(&self, snapshot: DashboardSnapshot)` — dedupes: no version bump when equal to the current snapshot.
  - `CompanionHub::publish_card(&self, html: String)`
  - `CompanionHub::current(&self) -> Frame`
  - `CompanionHub::wait_after(&self, last: u64, timeout: Duration) -> Frame`
  - `CompanionHub::set_enabled(&self, v: bool)` / `is_enabled(&self) -> bool`
  - `Frame { version: u64, snapshot: Option<DashboardSnapshot>, card: Option<String> }`

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-companion/src/hub.rs`:

```rust
//! The async↔blocking bridge. The render loop (async) publishes state; the
//! blocking SSE reader threads park on the condvar until the version bumps.

use crate::snapshot::DashboardSnapshot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Latest {
    snapshot: Option<DashboardSnapshot>,
    card: Option<String>,
    version: u64,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub version: u64,
    pub snapshot: Option<DashboardSnapshot>,
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

    pub fn publish_snapshot(&self, snapshot: DashboardSnapshot) {
        let mut l = self.inner.lock().unwrap();
        if l.snapshot.as_ref() == Some(&snapshot) {
            return; // dedupe: identical state, no wake
        }
        l.snapshot = Some(snapshot);
        l.version += 1;
        drop(l);
        self.cv.notify_all();
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
            snapshot: l.snapshot.clone(),
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
            snapshot: l.snapshot.clone(),
            card: l.card.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::DashboardSnapshot;

    fn snap(name: &str) -> DashboardSnapshot {
        DashboardSnapshot {
            session_name: name.into(),
            model: "m".into(),
            provider: "p".into(),
            cwd: "/".into(),
            ctx_used: 0,
            ctx_ceiling: 0,
            session_tokens: 0,
            cached_tokens: 0,
            cache_supported: false,
            tasks_len: 0,
            busy: false,
            tiers: vec![],
            churn: vec![],
            updated_ms: 0,
        }
    }

    #[test]
    fn publish_bumps_version_and_current_reflects() {
        let hub = CompanionHub::new();
        assert_eq!(hub.current().version, 0);
        hub.publish_snapshot(snap("a"));
        let f = hub.current();
        assert_eq!(f.version, 1);
        assert_eq!(f.snapshot.unwrap().session_name, "a");
    }

    #[test]
    fn identical_snapshot_does_not_bump() {
        let hub = CompanionHub::new();
        hub.publish_snapshot(snap("a"));
        hub.publish_snapshot(snap("a")); // identical → no bump
        assert_eq!(hub.current().version, 1);
        hub.publish_snapshot(snap("b")); // different → bump
        assert_eq!(hub.current().version, 2);
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-companion hub::`
Expected: FAIL — `hub` module not declared in `lib.rs` (unresolved module / not compiled).

- [ ] **Step 3: Declare the module and re-export**

In `crates/zoid-companion/src/lib.rs`, add the module and re-exports:

```rust
pub mod hub;
pub mod snapshot;

pub use hub::{CompanionHub, Frame};
pub use snapshot::{DashboardSnapshot, TierRow};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-companion`
Expected: PASS (4 hub tests + the snapshot test).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-companion/src/hub.rs crates/zoid-companion/src/lib.rs
git commit -m "feat(companion): CompanionHub (mutex+condvar bridge, dedupe, enabled flag)"
```

---

## Task 3: HTTP server — routing, token gate, CSP, shell page

**Files:**
- Create: `crates/zoid-companion/src/server.rs`
- Create: `crates/zoid-companion/src/shell.html`
- Modify: `crates/zoid-companion/src/lib.rs`

**Interfaces:**
- Consumes: `CompanionHub` (Task 2).
- Produces:
  - `pub const CSP: &str` (the CSP header value).
  - `start(hub: Arc<CompanionHub>, port: u16, token: String) -> std::io::Result<CompanionServer>` — binds `127.0.0.1:port` (`port = 0` → OS-assigned).
  - `CompanionServer { pub url: String, pub port: u16, .. }`, method `shutdown(self)`.
- The `/events` route exists but only the shell + token-gate + shutdown are exercised here; SSE content is Task 4 (the `SseReader` type is created here as a stub returning EOF, then filled in Task 4).

- [ ] **Step 1: Create the embedded shell page**

Create `crates/zoid-companion/src/shell.html` (minimal but real — inline CSS/JS, known markers `id="dashboard"` / `id="card"` the tests assert on):

```html
<!doctype html>
<meta charset="utf-8">
<title>zoid companion</title>
<style>
  :root { --ink:#191d24; --muted:#5c6672; --teal:#0f7d72; --hair:#d7dde3; }
  body { margin:0; font-family:system-ui,sans-serif; color:var(--ink); background:#eef1f4; }
  main { max-width:960px; margin:0 auto; padding:1.5rem; display:grid; gap:1rem; }
  #dashboard { background:#fff; border:1px solid var(--hair); border-radius:12px; padding:1rem 1.25rem; }
  #card { background:#fff; border:1px solid var(--hair); border-radius:12px; padding:1rem 1.25rem; }
  .k { font-size:.72rem; text-transform:uppercase; letter-spacing:.06em; color:var(--muted); }
  .v { font-variant-numeric:tabular-nums; font-weight:600; }
  .bar { height:9px; border-radius:999px; background:var(--hair); overflow:hidden; }
  .bar > i { display:block; height:100%; background:var(--teal); }
</style>
<main>
  <section id="dashboard"><span class="k">waiting for session…</span></section>
  <section id="card"></section>
</main>
<script>
  const dash = document.getElementById("dashboard");
  const card = document.getElementById("card");
  const es = new EventSource("events");
  es.addEventListener("dashboard", (e) => {
    const d = JSON.parse(e.data);
    const pct = d.ctx_ceiling ? Math.min(100, Math.round((d.ctx_used / d.ctx_ceiling) * 100)) : 0;
    const tiers = (d.tiers || []).map(t =>
      `<div><span class="k">${t.label}</span> <span class="v">${t.tokens}</span>${t.cold ? " · cold" : ""}</div>`
    ).join("");
    dash.innerHTML =
      `<div class="k">${d.provider} · ${d.model} · ${d.session_name}</div>` +
      `<div class="v">${d.ctx_used} / ${d.ctx_ceiling} (${pct}%)</div>` +
      `<div class="bar"><i style="width:${pct}%"></i></div>` +
      `<div class="k">tiers</div>${tiers}` +
      `<div class="k">tasks: ${d.tasks_len}${d.busy ? " · busy" : ""}</div>`;
  });
  es.addEventListener("card", (e) => { card.innerHTML = JSON.parse(e.data); });
</script>
```

- [ ] **Step 2: Write the failing integration test**

Create `crates/zoid-companion/src/server.rs` with a test module that drives the server over a raw TCP socket (no HTTP-client dependency):

```rust
//! Blocking `tiny_http` server: serves the shell page (token-gated, CSP) and an
//! SSE `/events` stream. All threads are std threads — no tokio.

use crate::hub::CompanionHub;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

pub const CSP: &str =
    "default-src 'self'; connect-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'";

const SHELL: &str = include_str!("shell.html");

pub struct CompanionServer {
    server: Arc<Server>,
    accept: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
    pub url: String,
    pub port: u16,
    #[allow(dead_code)]
    token: String,
}

/// Bind `127.0.0.1:port` (0 = OS-assigned), spawn the accept loop, return the
/// handle. `token` is minted by the caller (the bin) and gates every route.
pub fn start(
    hub: Arc<CompanionHub>,
    port: u16,
    token: String,
) -> std::io::Result<CompanionServer> {
    let server = Server::http(("127.0.0.1", port))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);
    let base = format!("/s/{token}/");
    let url = format!("http://127.0.0.1:{bound}{base}");
    let server = Arc::new(server);
    let running = Arc::new(AtomicBool::new(true));

    let accept = {
        let server = server.clone();
        let running = running.clone();
        let hub = hub.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                let hub = hub.clone();
                let running = running.clone();
                let base = base.clone();
                std::thread::spawn(move || handle(request, hub, running, base));
            }
        })
    };

    Ok(CompanionServer {
        server,
        accept: Some(accept),
        running,
        url,
        port: bound,
        token,
    })
}

impl CompanionServer {
    /// Stop accepting, wake SSE readers, and join the accept thread. SSE worker
    /// threads observe `running == false` on their next 1s wait and exit.
    pub fn shutdown(mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.server.unblock();
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn handle(
    request: tiny_http::Request,
    hub: Arc<CompanionHub>,
    running: Arc<AtomicBool>,
    base: String,
) {
    if *request.method() != Method::Get {
        let _ = request.respond(Response::empty(StatusCode(404)));
        return;
    }
    let url = request.url().to_string();
    let shell_path = base.trim_end_matches('/');
    let events_path = format!("{base}events");

    if url == base || url == shell_path {
        let resp = Response::from_string(SHELL)
            .with_header(header("Content-Type", "text/html; charset=utf-8"))
            .with_header(header("Content-Security-Policy", CSP));
        let _ = request.respond(resp);
    } else if url == events_path {
        let reader = SseReader::new(hub, running);
        let resp = Response::new(
            StatusCode(200),
            vec![
                header("Content-Type", "text/event-stream"),
                header("Cache-Control", "no-cache"),
            ],
            reader,
            None,
            None,
        );
        let _ = request.respond(resp);
    } else {
        let _ = request.respond(Response::empty(StatusCode(404)));
    }
}

/// Stub in Task 3; filled with real streaming in Task 4.
pub(crate) struct SseReader {
    #[allow(dead_code)]
    hub: Arc<CompanionHub>,
    #[allow(dead_code)]
    running: Arc<AtomicBool>,
}

impl SseReader {
    pub(crate) fn new(hub: Arc<CompanionHub>, running: Arc<AtomicBool>) -> Self {
        Self { hub, running }
    }
}

impl Read for SseReader {
    fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
        Ok(0) // Task 4 replaces this with real SSE framing.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    fn raw_get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut buf = String::new();
        let _ = s.read_to_string(&mut buf);
        buf
    }

    #[test]
    fn shell_route_serves_html_with_csp() {
        let hub = CompanionHub::new();
        let server = start(hub, 0, "tok123".into()).unwrap();
        let resp = raw_get(server.port, "/s/tok123/");
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(
            resp.contains(&format!("Content-Security-Policy: {CSP}")),
            "missing CSP header: {resp}"
        );
        assert!(resp.contains("id=\"dashboard\""), "missing shell body");
        server.shutdown();
    }

    #[test]
    fn wrong_token_and_unknown_paths_are_404() {
        let hub = CompanionHub::new();
        let server = start(hub, 0, "tok123".into()).unwrap();
        assert!(raw_get(server.port, "/s/WRONG/").starts_with("HTTP/1.1 404"));
        assert!(raw_get(server.port, "/").starts_with("HTTP/1.1 404"));
        assert!(raw_get(server.port, "/s/tok123/other").starts_with("HTTP/1.1 404"));
        server.shutdown();
    }

    #[test]
    fn shutdown_joins_without_hanging() {
        let hub = CompanionHub::new();
        let server = start(hub, 0, "tok123".into()).unwrap();
        // Reaching the line after shutdown() proves the accept thread joined.
        server.shutdown();
    }
}
```

- [ ] **Step 3: Declare the module**

In `crates/zoid-companion/src/lib.rs`:

```rust
pub mod hub;
pub mod server;
pub mod snapshot;

pub use hub::{CompanionHub, Frame};
pub use server::{start, CompanionServer, CSP};
pub use snapshot::{DashboardSnapshot, TierRow};
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zoid-companion server::`
Expected: PASS — `shell_route_serves_html_with_csp`, `wrong_token_and_unknown_paths_are_404`, `shutdown_joins_without_hanging`.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-companion/src/server.rs crates/zoid-companion/src/shell.html crates/zoid-companion/src/lib.rs
git commit -m "feat(companion): tiny_http server — shell route, token gate, CSP, shutdown"
```

---

## Task 4: SSE `/events` streaming

**Files:**
- Modify: `crates/zoid-companion/src/server.rs` (replace the `SseReader` stub with real streaming)

**Interfaces:**
- Consumes: `CompanionHub::{current, wait_after}` (Task 2).
- Produces: `SseReader` now emits `event: dashboard\ndata: <json>\n\n` and `event: card\ndata: <json-string>\n\n` frames, a `: ping\n\n` heartbeat on idle, and EOF (`Ok(0)`) when `running` is false.

- [ ] **Step 1: Write the failing unit test for `SseReader`**

Append to the `tests` module in `crates/zoid-companion/src/server.rs`:

```rust
    #[test]
    fn sse_reader_emits_dashboard_then_card_frames() {
        use crate::snapshot::DashboardSnapshot;
        let hub = CompanionHub::new();
        // Publish a snapshot BEFORE the reader starts, so the first read returns
        // it immediately from `current()` without blocking.
        hub.publish_snapshot(DashboardSnapshot {
            session_name: "s".into(),
            model: "m".into(),
            provider: "p".into(),
            cwd: "/".into(),
            ctx_used: 5,
            ctx_ceiling: 10,
            session_tokens: 0,
            cached_tokens: 0,
            cache_supported: false,
            tasks_len: 0,
            busy: false,
            tiers: vec![],
            churn: vec![1, 2],
            updated_ms: 0,
        });
        let running = Arc::new(AtomicBool::new(true));
        let mut reader = SseReader::new(hub.clone(), running.clone());

        let mut buf = [0u8; 4096];
        let n = reader.read(&mut buf).unwrap();
        let frame = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(frame.contains("event: dashboard"), "got: {frame}");
        assert!(frame.contains("\"churn\":[1,2]"), "got: {frame}");

        // Now push a card; the next read should surface it.
        hub.publish_card("<b>card</b>".into());
        let n = reader.read(&mut buf).unwrap();
        let frame = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(frame.contains("event: card"), "got: {frame}");
        assert!(frame.contains("<b>card</b>"), "got: {frame}");

        // When running flips false, read returns EOF.
        running.store(false, Ordering::Relaxed);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-companion sse_reader_emits`
Expected: FAIL — the stub `read` returns `Ok(0)`, so the assertion `frame.contains("event: dashboard")` fails.

- [ ] **Step 3: Replace the `SseReader` stub with real streaming**

In `crates/zoid-companion/src/server.rs`, replace the entire stub `SseReader` struct + its `new`/`Read` impls with:

```rust
use crate::hub::Frame;
use crate::snapshot::DashboardSnapshot;

/// A blocking `Read` that turns hub updates into an SSE byte stream. tiny_http
/// pulls from it (chunked) for the connection's lifetime; each `read` either
/// drains the pending buffer, emits changed frames after a version bump, or
/// emits a heartbeat on idle. Returns `Ok(0)` (EOF) once `running` is false.
pub(crate) struct SseReader {
    hub: Arc<CompanionHub>,
    running: Arc<AtomicBool>,
    last_version: u64,
    last_snapshot: Option<DashboardSnapshot>,
    last_card: Option<String>,
    buf: Vec<u8>,
    pos: usize,
    started: bool,
}

impl SseReader {
    pub(crate) fn new(hub: Arc<CompanionHub>, running: Arc<AtomicBool>) -> Self {
        Self {
            hub,
            running,
            last_version: 0,
            last_snapshot: None,
            last_card: None,
            buf: Vec::new(),
            pos: 0,
            started: false,
        }
    }

    fn absorb(&mut self, frame: Frame) {
        if frame.snapshot != self.last_snapshot {
            if let Some(s) = &frame.snapshot {
                let json = serde_json::to_string(s).unwrap_or_default();
                self.buf
                    .extend_from_slice(format!("event: dashboard\ndata: {json}\n\n").as_bytes());
            }
            self.last_snapshot = frame.snapshot.clone();
        }
        if frame.card != self.last_card {
            if let Some(c) = &frame.card {
                let json = serde_json::to_string(c).unwrap_or_default();
                self.buf
                    .extend_from_slice(format!("event: card\ndata: {json}\n\n").as_bytes());
            }
            self.last_card = frame.card.clone();
        }
        self.last_version = frame.version;
    }
}

impl Read for SseReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.pos < self.buf.len() {
                let n = (self.buf.len() - self.pos).min(out.len());
                out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if !self.running.load(Ordering::Relaxed) {
                return Ok(0);
            }
            self.buf.clear();
            self.pos = 0;

            let frame = if !self.started {
                self.started = true;
                self.hub.current()
            } else {
                self.hub.wait_after(self.last_version, Duration::from_secs(1))
            };
            if !self.running.load(Ordering::Relaxed) {
                return Ok(0);
            }
            self.absorb(frame);
            if self.buf.is_empty() {
                // Heartbeat: keeps the connection live and lets the write side
                // notice a disconnected client.
                self.buf.extend_from_slice(b": ping\n\n");
            }
        }
    }
}
```

Remove the now-unused stub imports if the compiler flags them; keep `use crate::hub::Frame;` and `use crate::snapshot::DashboardSnapshot;` near the top of the file (or inline as shown).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zoid-companion`
Expected: PASS — all Task 1–4 tests, including `sse_reader_emits_dashboard_then_card_frames`.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-companion/src/server.rs
git commit -m "feat(companion): SSE /events streaming (dashboard + card frames, heartbeat)"
```

---

## Task 5: `show` emitting tool + agent-loop card push

**Files:**
- Create: `crates/zoid-tools/src/show.rs`
- Modify: `crates/zoid-tools/src/lib.rs`
- Modify: `crates/zoid/Cargo.toml` (add `zoid-companion` dep)
- Modify: `crates/zoid/src/invoke_skill.rs` (register `show` + test)
- Modify: `crates/zoid/src/agent.rs` (`companion_show` helper, `show` arm, thread `Arc<CompanionHub>`)
- Modify: `crates/zoid/src/main.rs` (pass `app.companion_hub.clone()` at `run_agent_turn` call sites — App field added in Task 6; if Task 6 not yet done, add a temporary `let companion_hub = zoid_companion::CompanionHub::new();` local at each call site and replace in Task 6)

**Interfaces:**
- Consumes: `zoid_tools::{Tool, ToolKind, ToolOutput}`, `zoid_provider::ToolSpec`, `zoid_companion::CompanionHub` (Tasks 2).
- Produces:
  - `zoid_tools::show::Show` (a `Tool` with `name() == "show"`, `kind() == ToolKind::Emitting`).
  - `zoid::agent::companion_show(hub: &CompanionHub, html: String) -> (String, bool)` — publishes the card when enabled, returns `(output, is_error)`.
  - `run_agent_turn` / `run_agent_turn_cancellable` / `run_turn_inner` gain a parameter `companion_hub: Arc<zoid_companion::CompanionHub>` (added after `session_id` in the existing signature).

- [ ] **Step 1: Write the failing test for the `Show` tool**

Create `crates/zoid-tools/src/show.rs`:

```rust
//! The `show` tool: render an HTML card in the companion browser view. Like
//! `recall`, it is `Emitting` — the agent loop executes it (it needs the
//! companion hub), so `run()` is never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct Show;

impl Tool for Show {
    fn name(&self) -> &str {
        "show"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "show".into(),
            description: "Render a self-contained HTML card in the companion browser view (a \
                          visual side panel). Use for mockups, diagrams, tables, or any visual \
                          the terminal cannot render at fidelity. The card replaces the \
                          previously shown one. Only works when the companion server is enabled."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "html": { "type": "string", "description": "self-contained HTML (inline CSS/SVG; no external resources)" },
                    "title": { "type": "string", "description": "optional short title" }
                },
                "required": ["html"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the loop branches on Emitting before calling run().
        ToolOutput::err("show is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolKind};

    #[test]
    fn show_spec_and_kind() {
        assert_eq!(Show.name(), "show");
        assert_eq!(Show.spec().name, "show");
        assert_eq!(Show.kind(), ToolKind::Emitting);
        // html is a required parameter
        let params = Show.spec().parameters;
        assert_eq!(params["required"][0], "html");
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/zoid-tools/src/lib.rs`, add near the other `pub mod` lines (e.g. next to `pub mod recall;`):

```rust
pub mod show;
```

- [ ] **Step 3: Run the tool test**

Run: `cargo test -p zoid-tools show::`
Expected: PASS (`show_spec_and_kind`).

- [ ] **Step 4: Add `zoid-companion` as a dep of the bin**

In `crates/zoid/Cargo.toml`, under `[dependencies]`, add after the `zoid-tui` line:

```toml
zoid-companion = { path = "../zoid-companion" }
```

- [ ] **Step 5: Write the failing test for `companion_show`**

In `crates/zoid/src/agent.rs`, add a test module entry (or extend the existing `#[cfg(test)] mod tests`) with:

```rust
    #[test]
    fn companion_show_publishes_when_enabled_and_acks_when_disabled() {
        use zoid_companion::CompanionHub;
        let hub = CompanionHub::new();

        // Disabled: no publish, distinct ack.
        let (out, err) = super::companion_show(&hub, "<b>x</b>".into());
        assert!(!err);
        assert!(out.contains("disabled"), "got: {out}");
        assert!(hub.current().card.is_none());

        // Enabled: publishes the card.
        hub.set_enabled(true);
        let (out, err) = super::companion_show(&hub, "<b>y</b>".into());
        assert!(!err);
        assert_eq!(out, "card shown in companion");
        assert_eq!(hub.current().card.as_deref(), Some("<b>y</b>"));
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p zoid companion_show_publishes`
Expected: FAIL — `companion_show` is not defined.

- [ ] **Step 7: Implement `companion_show`**

In `crates/zoid/src/agent.rs`, add the helper (module scope, near the other free helpers):

```rust
/// Result of a `show` tool call: publish the card when the companion is enabled,
/// otherwise return a no-op ack. Never errors (returns `is_error = false`).
pub(crate) fn companion_show(
    hub: &zoid_companion::CompanionHub,
    html: String,
) -> (String, bool) {
    if hub.is_enabled() {
        hub.publish_card(html);
        ("card shown in companion".to_string(), false)
    } else {
        (
            "Companion is disabled; enable it from the command palette to view cards."
                .to_string(),
            false,
        )
    }
}
```

- [ ] **Step 8: Thread the hub into the turn functions and add the `show` arm**

In `crates/zoid/src/agent.rs`, add `companion_hub: std::sync::Arc<zoid_companion::CompanionHub>` as a parameter to `run_agent_turn`, `run_agent_turn_cancellable`, and `run_turn_inner` (place it immediately after the existing `session_id` parameter), and pass it through from each wrapper to `run_turn_inner`.

Then add a new match arm alongside the `recall` arm (after the `recall` arm, before the `Interactive`/`ask_user` arm):

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "show" => {
                    let html = tc
                        .args
                        .get("html")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let (output, is_error) = companion_show(&companion_hub, html);
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output,
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "show",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = !is_error,
                        "tool executed"
                    );
                }
```

- [ ] **Step 9: Register `show` in `chat_tools` and update its test**

In `crates/zoid/src/invoke_skill.rs`, in `chat_tools`, add after the `Recall` push:

```rust
    // `show` renders an HTML card in the companion browser view. Chat-only (it
    // needs the companion hub); never in the subagent registry.
    tools.push(Box::new(zoid_tools::show::Show));
```

In the `chat_tools_includes_invoke_skill_and_base_registry` test, add:

```rust
        assert!(names.contains(&"show"));
```

- [ ] **Step 10: Update `run_agent_turn` call sites**

In `crates/zoid/src/main.rs`, at every call to `run_agent_turn` / `run_agent_turn_cancellable`, pass `app.companion_hub.clone()` in the new argument position (after `session_id`). (If Task 6 has not yet added the `companion_hub` field, temporarily construct one per call: `zoid_companion::CompanionHub::new()` — Task 6 replaces it with `app.companion_hub.clone()`.)

- [ ] **Step 11: Run the tests**

Run: `cargo test -p zoid-tools -p zoid`
Expected: PASS — `show_spec_and_kind`, `companion_show_publishes...`, `chat_tools_includes...` (now asserting `show`). Fix any call sites the compiler flags for the new argument.

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tools/src/show.rs crates/zoid-tools/src/lib.rs crates/zoid/Cargo.toml crates/zoid/src/agent.rs crates/zoid/src/invoke_skill.rs crates/zoid/src/main.rs
git commit -m "feat(companion): show emitting tool + agent-loop card push"
```

---

## Task 6: Bin projection mapping + render-loop publish

**Files:**
- Modify: `crates/zoid/src/main.rs` (`App` fields, hub init, `heat_rank`, `dashboard_snapshot`, render-loop publish, finalize call-site threading from Task 5)

**Interfaces:**
- Consumes: `zoid_companion::{CompanionHub, DashboardSnapshot, TierRow}`, `zoid_core::context::{ContextWindow, Heat}`, `zoid_core::economy::ChurnTimeline`.
- Produces:
  - `App.companion: Option<zoid_companion::CompanionServer>`, `App.companion_hub: Arc<zoid_companion::CompanionHub>`.
  - `heat_rank(h: zoid_core::context::Heat) -> u8`
  - `dashboard_snapshot(session_name, model, provider, cwd, ctx_used, ctx_ceiling, session_tokens, cached_tokens, cache_supported, tasks_len, busy, window, churn, updated_ms) -> DashboardSnapshot`

- [ ] **Step 1: Write the failing tests**

In `crates/zoid/src/main.rs`, add to the test module:

```rust
    #[test]
    fn heat_rank_orders_hot_warm_cold() {
        use zoid_core::context::Heat;
        assert_eq!(super::heat_rank(Heat::Hot), 2);
        assert_eq!(super::heat_rank(Heat::Warm), 1);
        assert_eq!(super::heat_rank(Heat::Cold), 0);
    }

    #[test]
    fn dashboard_snapshot_maps_scalars_and_churn() {
        use zoid_core::context::ContextWindow;
        use zoid_core::economy::{ChurnPoint, ChurnTimeline};
        let window = ContextWindow {
            items: vec![],
            total_tokens: 0,
        };
        let churn = ChurnTimeline {
            points: vec![
                ChurnPoint { tokens: 10, cached: 1 },
                ChurnPoint { tokens: 20, cached: 2 },
            ],
        };
        let snap = super::dashboard_snapshot(
            "sess", "glm", "ollama", "/home/x", 300, 384, 90, 20, true, 3, true, &window,
            &churn, 42,
        );
        assert_eq!(snap.session_name, "sess");
        assert_eq!(snap.model, "glm");
        assert_eq!(snap.provider, "ollama");
        assert_eq!(snap.cwd, "/home/x");
        assert_eq!(snap.ctx_used, 300);
        assert_eq!(snap.ctx_ceiling, 384);
        assert_eq!(snap.session_tokens, 90);
        assert_eq!(snap.cached_tokens, 20);
        assert!(snap.cache_supported);
        assert_eq!(snap.tasks_len, 3);
        assert!(snap.busy);
        assert_eq!(snap.churn, vec![10, 20]);
        assert!(snap.tiers.is_empty());
        assert_eq!(snap.updated_ms, 42);
    }
```

> Note: if `ContextWindow`/`ChurnTimeline`/`ChurnPoint` fields are not all `pub`, construct them via whatever constructor `zoid-core` exposes (check `context.rs`/`economy.rs`); the field names above match the code the extraction confirmed (`ContextWindow { items, total_tokens }`, `ChurnTimeline { points }`, `ChurnPoint { tokens, cached }`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid heat_rank_orders`
Expected: FAIL — `heat_rank`/`dashboard_snapshot` not defined.

- [ ] **Step 3: Implement `heat_rank` and `dashboard_snapshot`**

In `crates/zoid/src/main.rs` (module scope, near `build_overview_data`):

```rust
fn heat_rank(h: zoid_core::context::Heat) -> u8 {
    use zoid_core::context::Heat;
    match h {
        Heat::Hot => 2,
        Heat::Warm => 1,
        Heat::Cold => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn dashboard_snapshot(
    session_name: &str,
    model: &str,
    provider: &str,
    cwd: &str,
    ctx_used: u64,
    ctx_ceiling: u64,
    session_tokens: u64,
    cached_tokens: u64,
    cache_supported: bool,
    tasks_len: usize,
    busy: bool,
    window: &zoid_core::context::ContextWindow,
    churn: &zoid_core::economy::ChurnTimeline,
    updated_ms: i64,
) -> zoid_companion::DashboardSnapshot {
    use zoid_core::context::Heat;
    let tiers = window
        .items
        .iter()
        .map(|i| zoid_companion::TierRow {
            label: i.label.clone(),
            tokens: i.tokens,
            heat: heat_rank(i.heat),
            cold: i.heat == Heat::Cold,
            pinned: i.pinned,
        })
        .collect();
    zoid_companion::DashboardSnapshot {
        session_name: session_name.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        cwd: cwd.to_string(),
        ctx_used,
        ctx_ceiling,
        session_tokens,
        cached_tokens,
        cache_supported,
        tasks_len,
        busy,
        tiers,
        churn: churn.points.iter().map(|p| p.tokens).collect(),
        updated_ms,
    }
}
```

> If `zoid_core::context::Heat` has variants other than `Hot`/`Warm`/`Cold`, extend the `match` in `heat_rank` — the compiler's non-exhaustive error will name them.

- [ ] **Step 4: Add the `App` fields and initialize the hub**

In `crates/zoid/src/main.rs`, add to the `App` struct (after the `shell` field):

```rust
    /// Optional companion HTTP server (None = disabled). Managed via the command
    /// palette (`companion` / `companion off`) or the `--companion` launch flag.
    companion: Option<zoid_companion::CompanionServer>,
    /// The state hub feeding the companion. Always present (cheap); the server is
    /// the optional part. `is_enabled()` gates snapshot publishing and `show`.
    companion_hub: std::sync::Arc<zoid_companion::CompanionHub>,
```

At the `App { .. }` construction site, initialize:

```rust
        companion: None,
        companion_hub: zoid_companion::CompanionHub::new(),
```

Then replace any temporary `CompanionHub::new()` locals from Task 5 Step 10 with `app.companion_hub.clone()` at the `run_agent_turn` call sites.

- [ ] **Step 5: Publish snapshots from the render loop**

In `crates/zoid/src/main.rs`, in the render loop right after `app.shell.input_rows = ...` (the projection-refresh block around line 1282), add:

```rust
        // Feed the companion dashboard when it's enabled. The hub dedupes, so
        // unchanged frames (e.g. motion ticks) don't wake SSE clients.
        if app.companion_hub.is_enabled() {
            let snap = dashboard_snapshot(
                &app.shell.session_name,
                &app.shell.model,
                &app.shell.provider,
                &app.shell.cwd,
                app.shell.ctx_used,
                app.shell.ctx_ceiling,
                app.shell.session_tokens,
                app.shell.cached_tokens,
                app.shell.cache_supported,
                app.shell.tasks_len as usize,
                app.shell.busy,
                &app.proj.window,
                &app.proj.churn,
                now_ms(),
            );
            app.companion_hub.publish_snapshot(snap);
        }
```

> Confirm `app.proj.window` (a `ContextWindow`) and `app.proj.churn` (a `ChurnTimeline`) are the field names on `ProjectionCache` — the extraction confirmed `app.proj.window.total_tokens` and `app.proj.churn.points` are both used nearby.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p zoid`
Expected: PASS — `heat_rank_orders_hot_warm_cold`, `dashboard_snapshot_maps_scalars_and_churn`, and the whole bin suite. Fix compiler-flagged issues (field placement, call sites).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(companion): App wiring + dashboard snapshot projection from render loop"
```

---

## Task 7: Lifecycle wiring — config, `--companion`, palette, browser open

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (`CompanionConfig`, `PartialCompanion`, merge, defaults)
- Modify: `crates/zoid/src/main.rs` (env read, `enable_companion`/`disable_companion`/`open_url`, `--companion` dispatch, palette handling)
- Modify: `crates/zoid/src/cli.rs` (`--companion` parse + help)
- Modify: `crates/zoid-tui/src/command.rs` (`CompanionEnable`/`CompanionDisable` + parse)

**Interfaces:**
- Consumes: `zoid_companion::{start, CompanionServer, CompanionHub}` (Tasks 2–3), `App` (Task 6).
- Produces:
  - `zoid_core::config::CompanionConfig { port: u16, open: bool }` on `Config` (default `{ port: 0, open: true }`).
  - `zoid_tui::command::Command::{CompanionEnable, CompanionDisable}`.
  - `Cli::Run { companion: bool }`.

- [ ] **Step 1: Write the failing config test**

In `crates/zoid-core/src/config.rs`, add to the tests module:

```rust
    #[test]
    fn companion_section_parses_and_merges() {
        let p = parse_toml("[companion]\nport = 9123\nopen = false").unwrap();
        assert_eq!(p.companion.port, Some(9123));
        assert_eq!(p.companion.open, Some(false));
        let (cfg, _prov) = merge(&[(Source::File, p)]);
        assert_eq!(cfg.companion.port, 9123);
        assert!(!cfg.companion.open);
        // default when absent
        let (dflt, _) = merge(&[]);
        assert_eq!(dflt.companion.port, 0);
        assert!(dflt.companion.open);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-core companion_section_parses`
Expected: FAIL — `companion` field does not exist on `Config`/`PartialConfig`.

- [ ] **Step 3: Add `CompanionConfig` and wire the config**

In `crates/zoid-core/src/config.rs`:

Add the config struct + default (near `EconomyConfig`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionConfig {
    /// TCP port for the companion server; 0 = OS-assigned ephemeral.
    pub port: u16,
    /// Auto-open the browser when the companion is enabled.
    pub open: bool,
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self { port: 0, open: true }
    }
}
```

Add the field to `Config`:

```rust
    pub companion: CompanionConfig,
```

Add `companion: CompanionConfig::default()` to the `impl Default for Config` (locate it — it seeds `merge`).

Add the partial (near `PartialEconomy`):

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialCompanion {
    pub port: Option<u16>,
    pub open: Option<bool>,
}
```

Add the field to `PartialConfig`:

```rust
    pub companion: PartialCompanion,
```

In `merge`, inside the `for (src, p) in layers` loop, add (no provenance — companion is not shown in the config overlay):

```rust
        if let Some(v) = p.companion.port {
            cfg.companion.port = v;
        }
        if let Some(v) = p.companion.open {
            cfg.companion.open = v;
        }
```

- [ ] **Step 4: Run the config test**

Run: `cargo test -p zoid-core companion_section_parses`
Expected: PASS.

- [ ] **Step 5: Write the failing CLI + command tests**

In `crates/zoid/src/cli.rs` tests module:

```rust
    #[test]
    fn parses_companion_flag() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string()]),
            super::Cli::Run { companion: true }
        );
        assert_eq!(
            super::parse_args(Vec::<String>::new()),
            super::Cli::Run { companion: false }
        );
        assert_eq!(
            super::parse_args(vec!["--version".to_string()]),
            super::Cli::Version
        );
    }
```

In `crates/zoid-tui/src/command.rs` tests module:

```rust
    #[test]
    fn parses_companion_commands() {
        assert_eq!(parse_command("companion"), Command::CompanionEnable);
        assert_eq!(parse_command(":companion off"), Command::CompanionDisable);
    }
```

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -p zoid parses_companion_flag && cargo test -p zoid-tui parses_companion_commands`
Expected: FAIL — `Cli::Run` has no `companion` field; `Command::CompanionEnable`/`Disable` don't exist.

- [ ] **Step 7: Update `cli.rs`**

In `crates/zoid/src/cli.rs`, change the `Run` variant and `parse_args`:

```rust
pub enum Cli {
    /// Launch the TUI. `companion` starts the companion server at boot.
    Run { companion: bool },
    Version,
    Help,
    Update,
    Unknown(String),
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Cli {
    match args.into_iter().next().as_deref() {
        None => Cli::Run { companion: false },
        Some("--version" | "-V") => Cli::Version,
        Some("--help" | "-h") => Cli::Help,
        Some("update") => Cli::Update,
        Some("--companion") => Cli::Run { companion: true },
        Some(other) => Cli::Unknown(other.to_string()),
    }
}
```

Add a help line to `help_text()` (inside the string, after the `zoid update` line):

```
    zoid --companion  Launch with the companion browser view enabled
```

- [ ] **Step 8: Update `command.rs`**

In `crates/zoid-tui/src/command.rs`, add the two variants to the `Command` enum:

```rust
    /// Enable the companion server (start it if needed, open the browser).
    CompanionEnable,
    /// Disable (stop) the companion server.
    CompanionDisable,
```

In `parse_command`, add arms (before the `other =>` fallback; note the more specific `"companion off"` is a distinct match, and plain `"companion"` maps to enable):

```rust
        "companion" => Command::CompanionEnable,
        "companion off" => Command::CompanionDisable,
```

- [ ] **Step 9: Run the CLI + command tests**

Run: `cargo test -p zoid parses_companion_flag && cargo test -p zoid-tui parses_companion_commands`
Expected: PASS. The `Cli::Run` change will break the dispatch `match` in `main.rs` — fixed next.

- [ ] **Step 10: Wire `enable_companion`/`disable_companion`/`open_url` and dispatch**

In `crates/zoid/src/main.rs`, add the helpers (module scope):

```rust
fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

fn enable_companion(app: &mut App) {
    if let Some(server) = &app.companion {
        // Already running: just re-open (idempotent).
        if app.config.companion.open {
            open_url(&server.url);
        }
        return;
    }
    let token = Ulid::new().to_string();
    match zoid_companion::start(app.companion_hub.clone(), app.config.companion.port, token) {
        Ok(server) => {
            app.companion_hub.set_enabled(true);
            if app.config.companion.open {
                open_url(&server.url);
            } else {
                app.shell.status_hint = Some(format!("companion: {}", server.url));
            }
            app.companion = Some(server);
        }
        Err(e) => {
            app.shell.status_hint = Some(format!("companion: {e}"));
        }
    }
}

fn disable_companion(app: &mut App) {
    if let Some(server) = app.companion.take() {
        app.companion_hub.set_enabled(false);
        server.shutdown();
    }
}
```

In `exec_command`, add arms to the `match cmd`:

```rust
        Command::CompanionEnable => {
            enable_companion(app);
            Ok(false)
        }
        Command::CompanionDisable => {
            disable_companion(app);
            Ok(false)
        }
```

Update the top-level `Cli` dispatch `match` (around `main.rs:1011`) so the `Run` arm binds the flag, e.g. `Cli::Run { companion } => { /* pass `companion` into run() */ }`. Thread a `companion: bool` parameter into `run(..)` and, right after the `App { .. }` is constructed and before the main loop, call:

```rust
    if companion {
        enable_companion(&mut app);
    }
```

- [ ] **Step 11: Add the env overrides**

In `crates/zoid/src/main.rs`, in `load_config`'s env layer (right after the `ZOID_CONTEXT_CEILING` block), add:

```rust
    if let Ok(v) = std::env::var("ZOID_COMPANION_PORT") {
        if let Ok(n) = v.trim().parse::<u16>() {
            envp.companion.port = Some(n);
        }
    }
    if let Ok(v) = std::env::var("ZOID_COMPANION_OPEN") {
        envp.companion.open = Some(matches!(v.trim(), "1" | "true" | "yes"));
    }
```

- [ ] **Step 12: Run the full suite**

Run: `cargo test -p zoid-core -p zoid-tui -p zoid -p zoid-companion -p zoid-tools`
Expected: PASS across all touched crates. Resolve any compiler errors from the `Cli::Run` shape change (the dispatch match and any other `Cli::Run` references).

- [ ] **Step 13: Commit**

```bash
git add crates/zoid-core/src/config.rs crates/zoid/src/main.rs crates/zoid/src/cli.rs crates/zoid-tui/src/command.rs
git commit -m "feat(companion): lifecycle wiring — config, --companion flag, palette enable/disable"
```

---

## Final verification

After Task 7, run the whole workspace test gate and a clippy pass:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Expected: 0 failures; no new clippy warnings. Then a manual smoke test: launch `zoid`, run `:companion` (browser opens to the dashboard), watch the budget/tiers update as you take a turn, have the agent call `show` with a small HTML card, then `:companion off` and confirm the tab goes dead.

---

## Self-Review

**1. Spec coverage**

| Spec requirement | Task |
|---|---|
| Leaf crate, deps = tiny_http/serde/serde_json only | 1 (manifest), Global Constraints |
| `DashboardSnapshot`/`TierRow`, churn as numeric series | 1 |
| `CompanionHub` (Mutex+Condvar), dedupe, `enabled` flag | 2 |
| Localhost bind, token gate, 404-on-miss, CSP header | 3 |
| Shell page (metrics-only dashboard, single card) | 3 (html), 4 (card frame) |
| SSE dashboard + card frames, replace-card semantics | 4 |
| `show` emitting tool, `chat_tools`-only, disabled ack, no auto-start | 5 |
| Token minted with Ulid (in bin), passed to `start` | 5 (helper uses `Ulid::new`), 7 (`enable_companion`) |
| Render-loop projection mapping (dedup, not per motion tick) | 6 |
| Config `[companion] port/open` + env overrides | 7 |
| `--companion` launch flag | 7 |
| Palette enable + disable, browser auto-open, clean stop | 7 |
| No tokio in server; std threads; clean shutdown/join | 3 (shutdown), Global Constraints |

No gaps.

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to". Each code step shows full code; each test step shows the assertions. The two "confirm field names" notes point at extraction-verified names, not unknowns.

**3. Type consistency:** `DashboardSnapshot`/`TierRow` field names and types are identical across Tasks 1, 2, 4, 6. `start(hub, port, token)` is defined in Task 3 and called in Task 7. `companion_show(&hub, html) -> (String, bool)` defined and tested in Task 5, used by the `show` arm. `heat_rank`/`dashboard_snapshot` signatures match between Task 6's definition, tests, and the render-loop call. `Cli::Run { companion: bool }` and `Command::{CompanionEnable, CompanionDisable}` are defined and consumed within Task 7. `CompanionConfig { port: u16, open: bool }` consistent between config (Task 7) and `enable_companion` (Task 7).
