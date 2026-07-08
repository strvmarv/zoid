use crate::config::McpServerConfig;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// The two line-oriented halves of a live MCP connection. The client actor
/// writes requests to `outbound` and reads server lines from `inbound`
/// (which closes when the server's stdout hits EOF).
pub struct TransportHandle {
    pub outbound: mpsc::Sender<String>,
    pub inbound: mpsc::Receiver<String>,
    /// The live child. Held here (NOT moved into a detached reaper task) so
    /// that `kill_on_drop` actually fires when the owning client drops this
    /// handle — a server that ignores stdin-close is still reaped. `None` in
    /// unit tests that wire the channels by hand.
    pub(crate) _child: Option<tokio::process::Child>,
}

/// The transport seam. v1 ships only `StdioTransport`; a future `HttpTransport`
/// implements the same trait and returns the same `TransportHandle`, so the
/// client actor never changes.
pub trait McpTransport: Send + Sync {
    fn connect(&self, cfg: &McpServerConfig) -> anyhow::Result<TransportHandle>;
}

impl TransportHandle {
    /// Test-only: the OS pid of the live child, for liveness probes.
    #[cfg(test)]
    pub(crate) fn child_pid(&self) -> Option<u32> {
        self._child.as_ref().and_then(|c| c.id())
    }
}

pub struct StdioTransport;

impl McpTransport for StdioTransport {
    /// Spawn `cfg.command` and wire its stdio into line channels. The child
    /// inherits zoid's environment, with `cfg.env` layered on top. Stderr is
    /// drained on its own task so a full pipe can't block the child.
    fn connect(&self, cfg: &McpServerConfig) -> anyhow::Result<TransportHandle> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
        let (in_tx, in_rx) = mpsc::channel::<String>(64);

        // Writer: outbound lines -> child stdin (append newline framing).
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() { break; }
                if stdin.write_all(b"\n").await.is_err() { break; }
                let _ = stdin.flush().await;
            }
        });

        // Reader: child stdout lines -> inbound channel (drops on EOF).
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if in_tx.send(line).await.is_err() { break; }
            }
            // in_tx dropped here => inbound closes.
        });

        // Stderr drain: keep the pipe empty; surface as trace diagnostics.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "zoid_mcp::server_stderr", "{line}");
            }
        });

        // The child is owned by the handle so `kill_on_drop` fires when the
        // client drops it. Do NOT move it into a `wait()` task — that would
        // park the only owner forever and defeat kill_on_drop.
        Ok(TransportHandle { outbound: out_tx, inbound: in_rx, _child: Some(child) })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;
    use std::collections::BTreeMap;

    // `cat` echoes each stdin line back on stdout: enough to prove the
    // spawn + line-framing + read path without a real MCP server.
    #[tokio::test]
    async fn stdio_roundtrips_a_line_through_cat() {
        let cfg = McpServerConfig {
            command: "cat".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        let mut h = StdioTransport.connect(&cfg).unwrap();
        h.outbound.send(r#"{"hello":1}"#.to_string()).await.unwrap();
        let line = h.inbound.recv().await.expect("a line back");
        assert_eq!(line, r#"{"hello":1}"#);
    }

    #[tokio::test]
    async fn inbound_closes_on_child_exit() {
        let cfg = McpServerConfig {
            command: "true".into(), // exits immediately, closing stdout
            args: vec![],
            env: BTreeMap::new(),
        };
        let mut h = StdioTransport.connect(&cfg).unwrap();
        assert!(h.inbound.recv().await.is_none(), "EOF => channel closed");
    }

    // A server that ignores stdin-close must still be reaped when the handle
    // drops (regression guard for the kill_on_drop fix). `sleep` never reads
    // stdin, so only kill_on_drop can end it.
    #[tokio::test]
    async fn stdin_ignoring_child_is_reaped_on_handle_drop() {
        let cfg = McpServerConfig { command: "sleep".into(), args: vec!["30".into()], env: BTreeMap::new() };
        let h = StdioTransport.connect(&cfg).unwrap();
        let pid = h.child_pid().expect("a pid");
        drop(h); // kill_on_drop should terminate the child
        // Poll `kill -0 <pid>` until it reports the process is gone.
        let mut gone = false;
        for _ in 0..50 {
            let status = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .unwrap();
            if !status.success() { gone = true; break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(gone, "child {pid} still alive after handle drop");
    }
}
