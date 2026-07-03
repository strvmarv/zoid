//! Dependency-free, env-gated diagnostic log. Set `ZOID_LOG=/path/to/file` to
//! enable; every `dbglog!(...)` line is appended with an epoch-millis timestamp.
//! When `ZOID_LOG` is unset the sink is `None` and each call is a cheap branch,
//! so instrumentation can stay in place. This exists because the TUI owns the
//! terminal (alternate screen + raw mode), so `eprintln!` is invisible/corrupting
//! during a session — a file is the only way to trace live behavior.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

fn sink() -> &'static Option<Mutex<std::fs::File>> {
    SINK.get_or_init(|| {
        let path = std::env::var("ZOID_LOG").ok()?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(Mutex::new)
    })
}

fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Append one timestamped line to the debug log iff `ZOID_LOG` is set.
pub fn log(args: std::fmt::Arguments) {
    if let Some(m) = sink() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{} {}", epoch_ms(), args);
        }
    }
}

/// `zlog!("msg {}", x)` — append a timestamped line when `ZOID_LOG` is set.
/// Named `zlog` (not `dbglog`) to avoid clashing with the `dbglog` module name.
#[macro_export]
macro_rules! zlog {
    ($($arg:tt)*) => { $crate::dbglog::log(format_args!($($arg)*)) };
}
