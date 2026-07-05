# zoid Companion Server — Design

**Status:** Approved for planning
**Date:** 2026-07-04
**Crate:** `zoid-companion` (new, leaf)

## Summary

Add an optional, lazy-started, localhost-only HTTP server to zoid that gives the
running session a **visual surface** in the browser. It exposes two things:

1. A **persistent dashboard** — a live, metrics-only projection of session state
   (context budget vs ceiling, hot/cold tiers, token-churn series, task count,
   model/provider/session/cwd).
2. A **push-fragment channel** — a single, replaceable HTML "card" the agent can
   render on demand via a new `show` tool, for mockups, diagrams, and other
   visual output that a terminal cannot render at fidelity.

The server is off by default, started/stopped explicitly from the command
palette (or a launch flag), bound to `127.0.0.1`, and gated by a per-session
token. It is a second **projection** of the event/state stream the TUI already
renders — not a new subsystem — bridged from the async render loop to a blocking
`tiny_http` server through a small synchronous hub.

## Goals

- One-keystroke transition from terminal work to a visual browser view, with
  **zero standing cost** when unused.
- Reuse zoid's existing per-frame projections (`OverviewData`, `EconomyView`,
  `ShellState`) rather than recomputing state.
- Keep the new crate a dependency-light leaf: `tiny_http` + `serde_json` only,
  pure-Rust/rustls, musl-clean (honors the static-musl release target).
- Full user control over the server lifecycle (enable **and** disable).

## Non-Goals (v1)

- Full conversation transcript mirror.
- Card feed / history (v1 is a single replaceable card).
- Server-side Markdown rendering.
- Multi-session switching in the browser.
- Anything non-localhost: no TLS, no remote binding, no auth beyond the token.

## Global Constraints

These bind every implementation task.

- **Bind address:** `127.0.0.1` only. Never `0.0.0.0`.
- **New crate deps:** `zoid-companion` depends only on `tiny_http` and
  `serde`/`serde_json`. It MUST NOT depend on `zoid-core`, `tokio`, or any
  non-Rust/OpenSSL-linked library. Dependency arrow is one-way: `zoid →
  zoid-companion`.
- **Token:** minted with `ulid::Ulid::new()` (128-bit, URL-safe Crockford
  base32). No new `rand`/`getrandom` dependency.
- **CSP header** on the shell page, verbatim (as shipped):
  `default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; frame-src 'self' data:; form-action 'self'; base-uri 'self'`
  > **CSP correction (implementation).** This design originally specified
  > `connect-src 'none'` with no `script-src`. That was dead-on-arrival:
  > `connect-src` governs the dashboard's own `EventSource`, so `'none'` blocks
  > the SSE stream entirely, and with no `script-src` the shell's inline
  > `<script>` is also blocked. The shipped policy uses `connect-src 'self'`
  > (permits same-origin SSE, still blocks external egress), moves the shell JS
  > to a served same-origin `app.js`, and adds `form-action 'self'`/`base-uri
  > 'self'` (neither falls back to `default-src`) to close form/`<base>` exfil.
  > **Interactive-card revision.** Cards were originally injected as `#card`
  > innerHTML with card JS deliberately inert. To let cards be *interactive*
  > without granting card JS the token-bearing origin, each card now renders in a
  > sandboxed `<iframe sandbox="allow-scripts">` from a `data:` URL (`frame-src
  > 'self' data:` permits it). The sandbox gives the frame an opaque origin (no
  > `allow-same-origin`, no `allow-top-navigation`), so card JS **runs** but
  > cannot read the shell URL (the token), the dashboard DOM, or the SSE stream,
  > and cannot redirect the top page — verified in a real browser. A `data:`
  > frame is a local scheme and inherits the shell CSP, so `script-src` gains
  > `'unsafe-inline'` purely to let the *framed* script execute; it is inert for
  > the shell itself, which has no inline script and into whose DOM no
  > agent-authored content is ever inserted. Isolation is enforced by the
  > sandbox, not by `script-src`. The per-session token is never sent to the
  > model and is structurally unreachable from a card. Residuals, accepted: the
  > shell's own top-level navigation (`<meta refresh>`), and a card's sandboxed
  > script making egress that carries only data the agent itself authored (never
  > anything read from the parent — the opaque origin guarantees that).
- **Token failure response:** any missing/wrong token or unknown path returns
  `404` with an empty body. Never `401`/`403` (do not confirm existence).
- **Default off:** the server never starts unless explicitly enabled (palette,
  `--companion` flag, or `[companion] enabled = true` if later added — not in
  v1 config; v1 config is `port` + `open` only).
- **Server touches no tokio.** All server threads are `std::thread`; all
  cross-thread state moves through `CompanionHub` (`Mutex` + `Condvar`).
- **Edition/workspace:** `edition = "2021"`, `version.workspace = true`,
  workspace dependency inheritance, added to root `Cargo.toml` `members`.

## Architecture

```
zoid (bin) ──owns──▶ CompanionServer ──serves──▶ browser (EventSource)
   │  publish_snapshot()            ▲ SSE (dashboard + card frames)
   └──▶ CompanionHub ◀──────────────┘
        (Mutex<Latest> + Condvar)
   show tool (Emitting) ──publish_card()──▶ hub
```

- The **async render loop** (`crates/zoid/src/main.rs`, the `tokio::select!` at
  ~`:1473`) already builds `OverviewData`/`ShellState`/`EconomyView` each frame.
  When the companion is enabled and the snapshot has changed, it maps those into
  a `DashboardSnapshot` and calls `hub.publish_snapshot(..)`. This call is a
  microsecond, uncontended `Mutex` lock + `Condvar::notify_all` — safe from
  async without blocking the runtime.
- The **`tiny_http` server** runs on its own `std::thread` accept loop. Each SSE
  connection is handled on a worker `std::thread` that parks on
  `hub.wait_after(version, timeout)` and writes a frame when `version` bumps.
- The **`show` tool** is emitting (intercepted in the agent loop like `recall`);
  it calls `hub.publish_card(html)`.

This keeps `zoid-companion` a leaf crate: it defines its own serde
`DashboardSnapshot` and never references `zoid-core` types. The bin performs the
mapping.

## Components

### `zoid-companion` crate

**`CompanionHub`** — the async↔blocking bridge.

```rust
pub struct CompanionHub {
    inner: Mutex<Latest>,
    cv: Condvar,
}

struct Latest {
    snapshot: Option<DashboardSnapshot>,
    card: Option<String>,     // raw HTML
    version: u64,             // monotonic; bumped on every publish
}

impl CompanionHub {
    pub fn new() -> Arc<Self>;

    /// Replace snapshot, bump version, notify all waiters.
    pub fn publish_snapshot(&self, snapshot: DashboardSnapshot);

    /// Replace the single card, bump version, notify all waiters.
    pub fn publish_card(&self, html: String);

    /// Snapshot of current state and version, without blocking.
    pub fn current(&self) -> Frame;

    /// Block until `version > last`, or until `timeout` elapses.
    /// Returns the current Frame (caller compares versions).
    pub fn wait_after(&self, last: u64, timeout: Duration) -> Frame;
}

pub struct Frame {
    pub version: u64,
    pub snapshot: Option<DashboardSnapshot>,
    pub card: Option<String>,
}
```

`wait_after` uses a bounded `Condvar` wait (e.g. 1s) so SSE workers periodically
re-check a shutdown flag even without new data.

**`DashboardSnapshot`** — plain serde, no zoid deps.

```rust
#[derive(Clone, Serialize, PartialEq)]
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
    pub churn: Vec<u64>,   // numeric series; browser draws the SVG sparkline
    pub updated_ms: i64,
}

#[derive(Clone, Serialize, PartialEq)]
pub struct TierRow {
    pub label: String,
    pub tokens: u64,
    pub heat: u8,
    pub cold: bool,
    pub pinned: bool,
}
```

`PartialEq` lets the bin dedupe: publish only when the new snapshot differs from
the last published one, so motion-tick frames (~16ms) do not wake SSE clients.

**`CompanionServer`** — lifecycle handle.

```rust
pub struct CompanionConfig {
    pub port: u16,       // 0 = OS-assigned ephemeral
    pub open: bool,      // auto-open browser after start
}

pub struct CompanionServer {
    server: Arc<tiny_http::Server>,
    accept: JoinHandle<()>,
    running: Arc<AtomicBool>,
    pub url: String,     // http://127.0.0.1:<port>/s/<token>/
    pub port: u16,
    token: String,
}

/// Bind 127.0.0.1:<port>, spawn the accept thread, return the handle.
pub fn start(hub: Arc<CompanionHub>, cfg: CompanionConfig) -> std::io::Result<CompanionServer>;

impl CompanionServer {
    /// Unblock the accept loop, signal workers, join. Consumes self.
    pub fn shutdown(self);
}
```

`shutdown` sets `running = false`, calls `server.unblock()` (so the accept
loop's `recv()` returns `Err` and the loop exits), then joins. SSE workers see
`running == false` on their next bounded `wait_after` wake and exit; their
writes also error once the browser disconnects.

**Static shell** — one `include_str!("shell.html")` string: inline CSS + JS, no
external assets. Opens `new EventSource("events")`, renders the dashboard from
`dashboard` frames, and renders each `card` frame into a sandboxed `<iframe>`
(one reused frame, `src` swapped per card; see the CSP note above).

### Routes & wire format

| Method | Path | Response |
|--------|------|----------|
| GET | `/s/<token>/` | shell HTML + `Content-Type: text/html` + the CSP header |
| GET | `/s/<token>/events` | `text/event-stream`; replays current frame, then streams deltas |
| GET | anything else / wrong token | `404`, empty body |

SSE frames:

```
event: dashboard
data: {"session_name":...,"ctx_used":...,"churn":[...],...}

event: card
data: "<div>...raw html...</div>"     (JSON-encoded string)
```

On connect the server sends the current snapshot and current card (if any)
immediately, then blocks on `wait_after` and emits one frame per version bump,
sending only the part(s) that changed.

### Bin integration (`crates/zoid`)

- **`App`** gains `companion: Option<CompanionServer>` and
  `companion_hub: Arc<CompanionHub>` (hub always present and cheap; server
  optional). Token/URL live on the `CompanionServer`.
- **Render loop:** after building the frame's projections (~`main.rs:1274-1329`),
  if `companion.is_some()`, map `OverviewData` + `EconomyView` rows →
  `DashboardSnapshot`; if it differs from the last published snapshot, call
  `hub.publish_snapshot`.
- **`show` tool:** new `ToolKind::Emitting` tool registered **only** in
  `chat_tools` (never subagent `registry()`), schema `{ html: String, title?:
  String }`. Intercepted in the agent loop (`agent.rs`, alongside the `recall`
  arm): calls `hub.publish_card(html)` when `companion.is_some()`, returns a
  `ToolResult` ack. When disabled, returns the ack *"Companion is disabled;
  enable it from the command palette to view cards."* and does **not** auto-start.
- **Command palette** (`zoid-tui/src/command.rs` + handling in `main.rs`): two
  entries — "Companion: Enable" and "Companion: Disable". Enable mints a token,
  calls `zoid_companion::start`, stores `Some`, and (if `open`) launches the
  browser. Disable calls `server.shutdown()` and sets `None`.
- **CLI** (`crates/zoid/src/cli.rs`): extend the `Run` variant to carry
  `companion: bool` (parsed from `--companion`); `main` enables at boot when set.
  Add a `--companion` line to `help_text()`.
- **Config** (`crates/zoid-core/src/config.rs`): a `[companion]` section with
  `port: Option<u16>` (default `0`/ephemeral) and `open: Option<bool>` (default
  `true`). Env overrides `ZOID_COMPANION_PORT` and `ZOID_COMPANION_OPEN`, read in
  `load_config` (`main.rs`).
- **Browser open:** hand-rolled, no dependency — `std::process::Command` with
  `xdg-open` (Linux), `open` (macOS), `cmd /c start` (Windows). Failure is
  non-fatal (see Error Handling).

## Data Flow

1. **Enable** → mint token → bind `127.0.0.1:<port>` (ephemeral unless config
   pins) → read the actual bound port → build `url` → spawn accept thread →
   if `open`, launch browser → store `Some(server)`.
2. **Steady state** → render loop maps projections → `DashboardSnapshot` →
   (deduped) `hub.publish_snapshot` → SSE workers wake on the version bump →
   each writes a `dashboard` frame.
3. **Card** → model calls `show(html)` → agent loop intercepts → `publish_card`
   → SSE `card` frame → shell loads it into the sandboxed card `<iframe>`
   (replacing the previous card).
4. **Disable** → `server.shutdown()` → accept loop + workers exit and join →
   `App.companion = None`. Browser's `EventSource` reconnect attempts fail
   silently. Re-enable mints a fresh token and URL.

## Error Handling

- **Port pinned and busy:** `start` returns `Err`; enable reports "companion:
  port N busy" in the TUI status line; `companion` stays `None`. The default
  ephemeral port avoids this.
- **`show` while disabled:** no-op ack (message above); never auto-starts.
- **Browser open fails** (headless/SSH): enable still succeeds; the URL is
  printed to the status line for manual paste. `open=false` is the headless
  default-friendly switch.
- **Client disconnect mid-SSE:** the frame write errors; the worker thread ends
  cleanly.
- **Double enable:** when already `Some`, re-open the browser to the existing
  URL (idempotent, no rebind). **Double disable:** no-op.
- **Snapshot serialization error:** logged, frame skipped; never panics the
  render loop.

## Testing

**`zoid-companion` unit**
- `publish_snapshot`/`publish_card` bump `version`; `wait_after(old, _)` returns
  promptly with the new frame; `wait_after(current, short)` times out and
  returns the same version.
- `DashboardSnapshot` serde round-trips; `churn` is emitted as a JSON array.
- SSE frame formatting matches `event: <type>\ndata: <payload>\n\n`.
- Token gate: request to `/s/<good>/` → 200; `/s/<wrong>/` → 404; `/` → 404;
  unknown path → 404.

**`zoid-companion` integration**
- `start` on port `0` → `GET /s/<tok>/` returns 200, body is the shell, and the
  **CSP header equals the constant** verbatim.
- `GET /s/<tok>/events` reads an initial `dashboard` frame after a
  `publish_snapshot`.
- Wrong token → 404.
- `shutdown()` returns (joins) within a timeout — proves `unblock()` works and
  workers observe `running == false`.

**bin unit**
- `OverviewData`/`EconomyView` → `DashboardSnapshot` mapping produces expected
  fields (ceiling, used, tier rows with heat/cold/pinned, churn series).
- `show` while `companion == None` returns the disabled ack and does not panic
  or start a server.
- Palette enable then disable flips `App.companion` `None → Some → None` and the
  second disable is a no-op.

## File Structure

**New — `crates/zoid-companion/`**
- `Cargo.toml` — leaf crate; deps `tiny_http`, `serde`, `serde_json`.
- `src/lib.rs` — re-exports; `CompanionHub`, `Frame`.
- `src/snapshot.rs` — `DashboardSnapshot`, `TierRow`.
- `src/server.rs` — `CompanionConfig`, `CompanionServer`, `start`, routing,
  token gate, SSE encoding, CSP header.
- `src/shell.html` — embedded static UI (inline CSS/JS).
- Tests colocated (`#[cfg(test)]`) plus `tests/` for the integration test.

**Modified**
- Root `Cargo.toml` — add member + `tiny_http` to `[workspace.dependencies]`.
- `crates/zoid/Cargo.toml` — add `zoid-companion` dep.
- `crates/zoid/src/main.rs` — `App.companion`/`companion_hub`; publish in render
  loop; palette handling; browser open; config/env read; `--companion` boot.
- `crates/zoid/src/cli.rs` — `--companion` parse + help line.
- `crates/zoid/src/agent.rs` — `show` emitting-tool interception arm.
- `crates/zoid/src/invoke_skill.rs` — register `show` in `chat_tools`.
- `crates/zoid-tools/` — `show` tool schema/definition.
- `crates/zoid-tui/src/command.rs` — "Companion: Enable"/"Disable" entries.
- `crates/zoid-core/src/config.rs` — `[companion]` section (`port`, `open`).

## Open Questions

None. All design decisions are resolved.
