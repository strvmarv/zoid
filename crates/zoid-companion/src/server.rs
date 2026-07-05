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
