//! Blocking `tiny_http` server: serves the shell page (token-gated, CSP) and an
//! SSE `/events` stream. All threads are std threads — no tokio.

use crate::hub::{CompanionHub, Frame};
use crate::snapshot::DashboardSnapshot;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

// CSP for the *shell* document (the dashboard page). This is the token-bearing
// origin, so it stays locked down: `script-src 'self'` (no 'unsafe-inline')
// means the only JS that runs in this origin is the served `app.js` — an
// agent-authored card can never execute script in the page that holds the
// session token. `connect-src 'self'` permits the dashboard's same-origin SSE
// (`EventSource`) while blocking script-driven egress (fetch/XHR/WS) to any
// other host. `form-action`/`base-uri` are pinned to 'self' (neither falls back
// to `default-src`). `img-src`/`style-src` block image/CSS network exfil.
//
// Agent card JS runs, but NOT in this origin: `app.js` renders each card inside
// an `<iframe sandbox="allow-scripts">` loaded from a `data:` URL. The sandbox
// (no `allow-same-origin`, no `allow-top-navigation`) gives the frame an opaque
// origin and walls its script off from the parent — it cannot read `location`
// (the token), the dashboard DOM, or the SSE stream, and cannot redirect the top
// page. `frame-src 'self' data:` lets the shell embed that frame.
//
// `script-src 'self' 'unsafe-inline'`: a `data:` frame is a *local scheme*, so
// per CSP inheritance it runs under THIS policy, not one of its own — without
// 'unsafe-inline' the card's own inline scripts would be blocked and nothing
// interactive would work. 'unsafe-inline' is safe to grant here because the
// token-bearing shell page carries no inline script (its only script is the
// served `app.js`) and NO agent-authored content is ever inserted into the shell
// DOM — cards live solely inside the sandboxed frame. Isolation is enforced by
// the sandbox (opaque origin), not by `script-src`; 'unsafe-inline' is inert for
// the shell and merely lets the walled-off frame execute. RESIDUAL, accepted: a
// card's sandboxed script can make egress carrying data the *agent itself
// authored* (never anything read from the parent — the opaque origin guarantees
// that), the same trust boundary the agent's own tools already sit on.
pub const CSP: &str = "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; frame-src 'self' data:; form-action 'self'; base-uri 'self'";

const SHELL: &str = include_str!("shell.html");
const APP_JS: &str = include_str!("app.js");

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
pub fn start(hub: Arc<CompanionHub>, port: u16, token: String) -> std::io::Result<CompanionServer> {
    let server = Server::http(("127.0.0.1", port)).map_err(std::io::Error::other)?;
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
    let app_js_path = format!("{base}app.js");

    if url == shell_path {
        // Canonical URL carries the trailing slash so the page's relative refs
        // (`app.js`, `events`) resolve under `/s/<token>/`. Redirect the bare
        // form rather than serving a shell whose resources would 404.
        let resp = Response::empty(StatusCode(301)).with_header(header("Location", &base));
        let _ = request.respond(resp);
    } else if url == base {
        let resp = Response::from_string(SHELL)
            .with_header(header("Content-Type", "text/html; charset=utf-8"))
            .with_header(header("Content-Security-Policy", CSP));
        let _ = request.respond(resp);
    } else if url == app_js_path {
        let resp = Response::from_string(APP_JS)
            .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
            .with_header(header("Content-Security-Policy", CSP));
        let _ = request.respond(resp);
    } else if url == events_path {
        stream_events(request, hub, running);
    } else {
        let _ = request.respond(Response::empty(StatusCode(404)));
    }
}

/// Stream the SSE `/events` response by taking over the raw socket. We can't use
/// tiny_http's `Response` here: it only flushes the writer *after* the whole
/// body is written (`raw_print` → `flush`), but our SSE body never ends — so
/// every frame would sit unflushed in the writer and the browser's `EventSource`
/// would hang forever (dashboard stuck on "waiting for session…"). Instead we
/// grab the writer with `into_writer`, emit the status line + headers, then pump
/// `SseReader` frames, flushing after each so they reach the client at once.
///
/// The body is delimited by connection close (`Connection: close`, no
/// Content-Length or chunking) and runs until the client disconnects (a write
/// error) or the server shuts down (`running` clears, so the reader yields EOF).
fn stream_events(request: tiny_http::Request, hub: Arc<CompanionHub>, running: Arc<AtomicBool>) {
    let mut w = request.into_writer();
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Connection: close\r\n\
                \r\n";
    if w.write_all(head.as_bytes()).is_err() || w.flush().is_err() {
        return;
    }
    // `SseReader` supports partial reads, so a card larger than `buf` simply
    // streams across several iterations; `read` yields `Ok(0)` only on shutdown.
    let mut reader = SseReader::new(hub, running);
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if w.write_all(&buf[..n]).is_err() || w.flush().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// A blocking `Read` that turns hub updates into an SSE byte stream. `read`
/// either drains the pending buffer, emits changed frames after a version bump,
/// or emits a heartbeat on idle. Returns `Ok(0)` (EOF) once `running` is false.
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
                self.hub
                    .wait_after(self.last_version, Duration::from_secs(1))
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
        // CSP must permit the app's own SSE, else the dashboard is dead on
        // arrival: connect-src is 'self' (not 'none'), and the shell pulls its
        // JS from a same-origin file so script-src can stay 'self'.
        assert!(
            resp.contains("connect-src 'self'"),
            "SSE-blocking CSP: {resp}"
        );
        assert!(
            resp.contains("src=\"app.js\""),
            "shell not wired to app.js: {resp}"
        );
        server.shutdown();
    }

    #[test]
    fn app_js_route_serves_script() {
        let hub = CompanionHub::new();
        let server = start(hub, 0, "tok123".into()).unwrap();
        let resp = raw_get(server.port, "/s/tok123/app.js");
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(
            resp.contains("text/javascript"),
            "wrong content-type: {resp}"
        );
        assert!(resp.contains("EventSource"), "missing script body: {resp}");
        server.shutdown();
    }

    #[test]
    fn events_stream_flushes_first_frame_over_http() {
        // End-to-end over a real socket (not `SseReader` in isolation): proves the
        // `/events` body reaches the client without waiting for the endless stream
        // to finish — the flush bug that pinned the dashboard on "waiting for
        // session…". A `read` here must return the dashboard frame, not block.
        let hub = CompanionHub::new();
        hub.publish_snapshot(DashboardSnapshot {
            session_name: "e2e".into(),
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
        });
        let server = start(hub, 0, "tok123".into()).unwrap();

        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(
            s,
            "GET /s/tok123/events HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();

        // Accumulate a couple of reads; each must return promptly (the timeout is
        // the failure mode). The status line + dashboard frame arrive right away.
        let mut acc = String::new();
        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => acc.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => break,
            }
            if acc.contains("event: dashboard") {
                break;
            }
        }
        assert!(acc.starts_with("HTTP/1.1 200"), "bad status: {acc}");
        assert!(
            acc.contains("text/event-stream"),
            "wrong content-type: {acc}"
        );
        assert!(
            acc.contains("event: dashboard"),
            "no dashboard frame: {acc}"
        );
        assert!(
            acc.contains("\"session_name\":\"e2e\""),
            "frame missing data: {acc}"
        );

        server.shutdown();
    }

    #[test]
    fn bare_token_path_redirects_to_canonical() {
        let hub = CompanionHub::new();
        let server = start(hub, 0, "tok123".into()).unwrap();
        let resp = raw_get(server.port, "/s/tok123");
        assert!(resp.starts_with("HTTP/1.1 301"), "got: {resp}");
        assert!(
            resp.contains("Location: /s/tok123/"),
            "missing Location: {resp}"
        );
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
}
