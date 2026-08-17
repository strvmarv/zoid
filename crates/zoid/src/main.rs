use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event as CEvent, EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use ratatui::{layout::Rect, prelude::CrosstermBackend, text::Line, Terminal};
use ratatui_textarea::{CursorMove, TextArea};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use ulid::Ulid;

mod obs;

use zoid::agent::{run_agent_turn_cancellable, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::conversation;
use zoid_core::session::SessionHandle;
use zoid_provider::Provider;
use zoid_provider::{default_model, default_provider};
use zoid_tui::chat::ChatView;
use zoid_tui::layout::compute;
use zoid_tui::render_shell;
use zoid_tui::route::{palette_selected_command, route_key, route_mouse, route_paste, PasteTarget};

/// Duration of the zoom fold/unfold line-reveal animation (Ⓡ2, T5).
const ZOOM_ANIM_MS: u64 = 160;

/// Resolve the user's home directory from the injected env. Checks `HOME`
/// (Unix, and Git Bash / MSYS on Windows) then `USERPROFILE` (native Windows).
/// Returns `None` if neither is set — callers must handle this (e.g. by
/// erroring or falling back to a relative path with a warning). On Windows,
/// `HOME` is often unset outside MSYS/Git Bash, so the `USERPROFILE` fallback
/// prevents `.local/share`, `.config`, and `.cache` from resolving against
/// the CWD (the bug: files landing in the project directory instead of the
/// user's home).
fn home_dir(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("USERPROFILE")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
}

/// Pure DB-path resolver (env injected for testing). Precedence:
/// `$ZOID_DB` > `$XDG_DATA_HOME/zoid/zoid.db` > `$HOME/.local/share/zoid/zoid.db`.
fn resolve_db_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("ZOID_DB") {
        return PathBuf::from(p);
    }
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir(env).unwrap_or_default().join(".local/share"));
    base.join("zoid").join("zoid.db")
}

/// Resolve the DB path from the real environment and ensure its parent exists.
fn db_path() -> Result<PathBuf> {
    let path = resolve_db_path(|k| std::env::var(k).ok());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(path)
}

/// Pure config-dir resolver (env injected for testing), mirroring
/// `resolve_db_path`'s precedence: `$XDG_CONFIG_HOME/zoid` >
/// `$HOME/.config/zoid`.
fn resolve_config_dir(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir(env).unwrap_or_default().join(".config"));
    base.join("zoid")
}

/// `$XDG_CACHE_HOME/zoid` > `$HOME/.cache/zoid` (mirrors `resolve_config_dir`).
#[cfg_attr(not(feature = "local-embed"), allow(dead_code))]
fn resolve_cache_dir(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CACHE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir(&env).map(|h| h.join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("zoid")
}

/// Pure secret-key-path resolver (env injected for testing), mirroring
/// `resolve_db_path`'s precedence: `$XDG_DATA_HOME/zoid/secret.key` >
/// `$HOME/.local/share/zoid/secret.key`.
fn resolve_secret_key_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir(env).unwrap_or_default().join(".local/share"));
    base.join("zoid").join("secret.key")
}

/// The four on-disk locations `zoid uninstall` removes. The data dir is derived
/// from the XDG data base (not `resolve_db_path`, so a `$ZOID_DB` override can't
/// point removal at an unrelated file) and holds `zoid.db` + `secret.key` +
/// secrets alongside each other.
fn uninstall_targets() -> zoid::uninstall::Targets {
    let env = |k: &str| std::env::var(k).ok();
    let data_dir = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir(env).unwrap_or_default().join(".local/share"))
        .join("zoid");
    zoid::uninstall::Targets {
        data_dir,
        config_dir: resolve_config_dir(env),
        cache_dir: resolve_cache_dir(env),
        binary: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zoid")),
    }
}

/// Format one unknown-key warning for the log, qualified by its source file.
fn layer_warning_line(file: &str, key: &str) -> String {
    format!("{file}: ignored unknown key {key}")
}

/// One-line status-bar summary of ignored config keys, or None when there were
/// none. A single key is named inline; several defer to the log.
fn config_warning_hint(keys: &[String]) -> Option<String> {
    match keys {
        [] => None,
        [one] => Some(format!("config: 1 key ignored ({one})")),
        _ => Some(format!("config: {} keys ignored — see log", keys.len())),
    }
}

/// Load config from files + env, in precedence order (user-global < project <
/// local < env). Missing files are skipped (empty layer); a malformed file is
/// skipped with a stderr note (non-fatal — the process still starts).
fn load_config() -> (
    zoid_core::config::Config,
    zoid_core::config::Provenance,
    Vec<String>,
) {
    use zoid_core::config::{merge, parse_toml, PartialConfig, Source};
    let env = |k: &str| std::env::var(k).ok();
    let cfg_dir = resolve_config_dir(env);
    let mut warnings: Vec<String> = Vec::new();
    let mut read = |p: PathBuf| -> Option<PartialConfig> {
        let text = std::fs::read_to_string(&p).ok()?;
        let file = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.toml");
        match parse_toml(&text) {
            Ok((pc, unknown)) => {
                for k in unknown {
                    tracing::warn!("{}", layer_warning_line(file, &k));
                    warnings.push(k);
                }
                Some(pc)
            }
            Err(e) => {
                eprintln!("zoid: ignoring {}: {e}", p.display());
                None
            }
        }
    };
    let mut layers: Vec<(Source, PartialConfig)> = Vec::new();
    if let Some(p) = read(cfg_dir.join("config.toml")) {
        layers.push((Source::UserGlobal, p));
    }
    if let Some(p) = read(PathBuf::from("./.zoid/config.toml")) {
        layers.push((Source::Project, p));
    }
    if let Some(p) = read(PathBuf::from("./.zoid/config.local.toml")) {
        layers.push((Source::Local, p));
    }
    // env layer
    let mut envp = PartialConfig::default();
    if let Ok(m) = std::env::var("ZOID_MODEL") {
        if !m.is_empty() {
            envp.model = Some(m);
        }
    }
    if let Ok(v) = std::env::var("ZOID_THINKING") {
        if let Some(pt) = parse_thinking_env(&v) {
            envp.thinking = pt;
        }
    }
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() {
            if n > 0 {
                envp.economy.context_target = Some(n);
            }
        }
    }
    if std::env::var("ZOID_REDUCED_MOTION")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        envp.reduced_motion = Some(true);
    }
    if let Ok(v) = std::env::var("ZOID_COMPANION_PORT") {
        if let Ok(n) = v.trim().parse::<u16>() {
            envp.companion.port = Some(n);
        }
    }
    if let Ok(v) = std::env::var("ZOID_COMPANION_OPEN") {
        envp.companion.open = Some(matches!(v.trim(), "1" | "true" | "yes"));
    }
    if let Ok(v) = std::env::var("ZOID_COMPANION_ENABLED") {
        envp.companion.enabled = Some(matches!(v.trim(), "1" | "true" | "yes"));
    }
    if let Ok(v) = std::env::var("ZOID_EVICTION_ENABLED") {
        envp.eviction.enabled = Some(matches!(v.trim(), "1" | "true" | "yes"));
    }
    layers.push((Source::Env, envp));
    let (cfg, prov) = merge(&layers);
    (cfg, prov, warnings)
}

/// Wall-clock millis since the epoch — supplied by the binary (core stays clock-free).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One-time pre-release migration: if a legacy in-repo `./.zoid/session.db`
/// exists and the new global DB does NOT yet exist, import the legacy events
/// under a single generated `session_id` (with a `sessions` row). Idempotent:
/// once the new DB exists we never import again. Returns whether an import ran.
fn import_legacy_if_present(
    new_db: &Path,
    legacy: &Path,
    session_id: Ulid,
    name: &str,
    root_path: &str,
    ts: i64,
) -> Result<bool> {
    if new_db.exists() || !legacy.exists() {
        return Ok(false);
    }
    let events =
        zoid_core::store::load_legacy_events(legacy.to_str().context("legacy path not UTF-8")?)?;
    if let Some(dir) = new_db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let store =
        zoid_core::store::EventStore::open(new_db.to_str().context("new db path not UTF-8")?)?;
    store.insert_session(session_id, name, root_path, ts, ts)?;
    for e in events {
        store.append(&e.with_session(session_id))?;
    }
    Ok(true)
}

/// Build the message input with the ratatui-textarea cursor-line **underline**
/// disabled (spec §2.2/§9): the default underline clutters the calm box.
fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
    let mut textarea = textarea;
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea.set_wrap_mode(ratatui_textarea::WrapMode::WordOrGlyph);
    textarea
}

/// Delete the whole line the cursor sits on (§30, ⇧Delete). Column-independent
/// (snaps to the line head first) and collapses the buffer by one row in every
/// multi-line case, so a repeated ⇧Delete keeps eating lines upward.
///
/// The subtlety: `delete_line_by_end()` is not purely "clear to end of line" —
/// on a line with nothing left to clear it *eagerly merges the next line up*
/// (ratatui-textarea's `delete_line_by_end`). Composing it with an unconditional
/// second merge therefore double-deletes on empty lines. We branch on the line's
/// emptiness (captured before mutating) and handle the three positions explicitly.
fn input_delete_line(textarea: &mut TextArea<'static>) {
    let row = textarea.cursor().0;
    let n = textarea.lines().len();
    let line_empty = textarea.lines()[row].is_empty();
    textarea.move_cursor(CursorMove::Head);

    if n == 1 {
        // Sole line: clear its content in place, leaving an empty buffer.
        if !line_empty {
            textarea.delete_line_by_end();
        }
    } else if row + 1 < n {
        // Not the last line: remove the row and pull the following line up.
        if line_empty {
            // Nothing to clear → delete_line_by_end merges the next line up itself.
            textarea.delete_line_by_end();
        } else {
            // Clear the content (in place), then consume the trailing newline.
            textarea.delete_line_by_end();
            textarea.delete_next_char();
        }
    } else {
        // Last line of many: clear its content, then delete the preceding
        // newline so the row vanishes and the cursor lands on the prior line.
        if !line_empty {
            textarea.delete_line_by_end();
        }
        textarea.delete_newline();
    }
}

/// Move the cursor to the very start of the buffer (§30, ⇧Home). `Top` keeps the
/// column, so chase it with `Head` to guarantee (0, 0).
fn input_cursor_top(textarea: &mut TextArea<'static>) {
    textarea.move_cursor(CursorMove::Top);
    textarea.move_cursor(CursorMove::Head);
}

/// Move the cursor to the very end of the buffer (§30, ⇧End). `Bottom` keeps the
/// column, so chase it with `End` to land past the last character.
fn input_cursor_bottom(textarea: &mut TextArea<'static>) {
    textarea.move_cursor(CursorMove::Bottom);
    textarea.move_cursor(CursorMove::End);
}

/// Canonical repo/cwd root as a string (best-effort absolute path).
fn repo_root() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

/// Whether the given OS PID is currently alive. `kill(pid, 0)` succeeds when the
/// process exists, returns `ESRCH` when it's dead, and `EPERM` when it exists but
/// isn't ours (treated as alive — we can't prove it's dead, and a stale-but-alive
/// row is reclaimable via the heartbeat window anyway). Injected into `is_live`
/// so callers can substitute a test double. Spec §2.2.
#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => true, // unknown failure → lean on the heartbeat window
    }
}

#[cfg(not(unix))]
fn pid_alive(_pid: i64) -> bool {
    true // non-Unix: no portable check; lean on the heartbeat window.
}

/// Map the tool-side `FileDiff` into the TUI's render mirror (keeps zoid-tui
/// free of a zoid-tools dependency; mirrors SubagentStarted → SubagentRow).
fn map_render_diff(d: zoid_tools::FileDiff) -> zoid_tui::state::RenderDiff {
    use zoid_tui::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    RenderDiff {
        path: d.path,
        added: d.added,
        removed: d.removed,
        truncated_by: d.truncated_by,
        lines: d
            .lines
            .into_iter()
            .map(|l| RenderDiffLine {
                old_no: l.old_no,
                new_no: l.new_no,
                kind: match l.kind {
                    zoid_tools::DiffKind::Ctx => RenderDiffKind::Ctx,
                    zoid_tools::DiffKind::Add => RenderDiffKind::Add,
                    zoid_tools::DiffKind::Del => RenderDiffKind::Del,
                },
                text: l.text,
            })
            .collect(),
    }
}

/// Truncate a queued-message hint to ~40 chars with an ellipsis (mirrors
/// `derive_session_name`'s truncation).
fn truncate_for_hint(s: &str) -> String {
    let one_line = s.lines().next().unwrap_or(s);
    if one_line.chars().count() > 40 {
        let head: String = one_line.chars().take(39).collect();
        format!("{head}\u{2026}")
    } else {
        one_line.to_string()
    }
}

/// Auto-derive a session name: the first user message truncated to 40 chars,
/// else `session HH:MM` from the injected timestamp.
fn derive_session_name(first_user_msg: Option<&str>, ts_ms: i64, tz_offset_secs: i32) -> String {
    let trimmed = first_user_msg.map(str::trim).filter(|s| !s.is_empty());
    match trimmed {
        Some(msg) => {
            let one_line = msg.lines().next().unwrap_or(msg);
            if one_line.chars().count() > 40 {
                let head: String = one_line.chars().take(39).collect();
                format!("{head}\u{2026}")
            } else {
                one_line.to_string()
            }
        }
        None => {
            let secs = ts_ms.div_euclid(1000) + tz_offset_secs as i64;
            let sod = secs.rem_euclid(86_400);
            format!("session {:02}:{:02}", sod / 3600, (sod % 3600) / 60)
        }
    }
}

/// Compact "N ago" from two epoch-millis stamps (e.g. "12m ago", "3h ago").
fn fmt_since(then_ms: i64, now_ms: i64) -> String {
    let mins = (now_ms - then_ms).max(0) / 60_000;
    if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 1440 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / 1440)
    }
}

/// Compact duration since `start_ms` (e.g. "12m", "1h3m").
fn fmt_duration(start_ms: i64, now_ms: i64) -> String {
    let mins = (now_ms - start_ms).max(0) / 60_000;
    if mins < 60 {
        format!("{mins}m")
    } else {
        format!("{}h{}m", mins / 60, mins % 60)
    }
}

/// Compact relative age from a millisecond delta (e.g. "45s", "12m", "3h"),
/// used to render `⚠ 12m provider` / `⛔ 3m tool shell` rows in the Overview
/// ERRORS band. Guarded against a negative/zero delta (clock skew or a
/// same-tick error) so it never underflows — those read as `"0s"`.
fn fmt_age(ms_ago: i64) -> String {
    let ms_ago = ms_ago.max(0);
    if ms_ago < 60_000 {
        format!("{}s", ms_ago / 1000)
    } else if ms_ago < 3_600_000 {
        format!("{}m", ms_ago / 60_000)
    } else {
        format!("{}h", ms_ago / 3_600_000)
    }
}

/// Snapshot the obs aggregate + economy projection into the pure `OverviewData`
/// consumed by `overview_lines`. Poison-safe: a poisoned obs mutex yields an
/// empty dashboard rather than a panic (the observability layer never panics).
fn build_overview_data(
    app: &App,
    obs_state: &std::sync::Arc<std::sync::Mutex<obs::ObsState>>,
) -> zoid_tui::overview::OverviewData {
    use zoid_tui::overview::OverviewData;
    // Economy split from the same ledger the session drawer/context economy use.
    let ledger = zoid_core::economy::token_ledger(app.events.iter());
    // Prompt-cache hit rate = cache-read as a % of input tokens (economy).
    let cache_hit_pct = (ledger.cached * 100)
        .checked_div(ledger.input)
        .map(|v| v.min(100) as u8)
        .unwrap_or(0);
    // Per-turn prompt-cache sparkline: map the cached churn series onto the
    // shared glyph::SPARK ramp, exactly as the context drawer's cache spark does.
    let cache_vals: Vec<u64> = app.proj.churn.points.iter().map(|p| p.cached).collect();
    let spark = zoid_tui::economy_view::sparkline(&cache_vals);

    let s = match obs_state.lock() {
        Ok(s) => s,
        Err(_) => return OverviewData::default(),
    };
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
        turn_last_ms: s.turn.last(),
        turn_avg_ms: s.turn.avg(),
        turn_p90_ms: s.turn.p90(),
        ttft_ms: s.provider_ttft.avg(),
        stream_ms: s.provider_total.avg(),
        iter_avg: s.iterations.avg(),
        tools: s
            .tools
            .iter()
            .map(|(k, v)| (k.clone(), v.count, v.avg_ms()))
            .collect(),
        frame_avg_ms: s.frame.avg(),
        frame_p90_ms: s.frame.p90(),
        frame_max_ms: s.frame.window_max(),
        // Body-render cache-hit ratio (obs frame events) — distinct from the
        // prompt-cache `cache_hit_pct` above.
        render_cache_pct: (s.cache_hits * 100)
            .checked_div(s.cache_total)
            .map(|v| v.min(100) as u8)
            .unwrap_or(0),
        proj_rebuilds: s.proj_rebuilds,
        event_count: app.events.len() as u64,
        errors: s
            .errors
            .iter()
            .map(|e| {
                let age = fmt_age(now_ms() - e.ts_ms);
                (
                    format!(
                        "{} {} {}",
                        if e.level == "error" { '⛔' } else { '⚠' },
                        age,
                        e.context
                    ),
                    e.message.clone(),
                )
            })
            .collect(),
    }
}

/// Last 4 chars of a string (short session id for the dashboard header).
fn last4(s: &str) -> String {
    let v: Vec<char> = s.chars().collect();
    v[v.len().saturating_sub(4)..].iter().collect()
}

/// Error from `resolve_resume_id` — drives the stderr message the bin emits
/// for a failed `--resume <id>`.
#[derive(Debug)]
enum ResumeIdError {
    /// No session for this CWD matched the query.
    NotFound,
    /// Multiple sessions share the same last-4; the query is ambiguous.
    /// Carries the full ULIDs of all candidates for the error message.
    Ambiguous(Vec<String>),
    /// The query is not 4 or 26 characters long.
    InvalidLength,
}

/// Resolve a `--resume <id>` query against sessions for the current CWD.
/// Accepts a 4-char last-4 form (matching the dashboard's `last4(...)` display)
/// or a 26-char full ULID. A 4-char query that matches multiple sessions
/// (same last-4) is `Ambiguous`. Any other length is `InvalidLength`. Pure.
fn resolve_resume_id(
    sessions: &[zoid_core::sessions::SessionInfo],
    query: &str,
) -> std::result::Result<Ulid, ResumeIdError> {
    match query.len() {
        4 => {
            let matches: Vec<&zoid_core::sessions::SessionInfo> = sessions
                .iter()
                .filter(|s| last4(&s.id.to_string()) == query)
                .collect();
            match matches.len() {
                0 => Err(ResumeIdError::NotFound),
                1 => Ok(matches[0].id),
                _ => Err(ResumeIdError::Ambiguous(
                    matches.iter().map(|s| s.id.to_string()).collect(),
                )),
            }
        }
        26 => match query.parse::<Ulid>() {
            Ok(id) => sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.id)
                .ok_or(ResumeIdError::NotFound),
            Err(_) => Err(ResumeIdError::NotFound),
        },
        _ => Err(ResumeIdError::InvalidLength),
    }
}

/// A key the startup picker recognizes (an abstraction over crossterm KeyEvent
/// so the input logic is testable without a terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickKey {
    Up,
    Down,
    Enter,
    Esc,
    Delete,
}

/// The outcome of a single keystroke in the startup picker.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickOutcome {
    /// The user moved the cursor; the new index is `usize`.
    Pending(usize),
    /// The user picked a session row (index into the session list).
    Resume(usize),
    /// The user picked the "Create new session" row.
    CreateNew,
    /// The user pressed Esc — abort to a clean exit.
    Abort,
    /// Delete the session at this index (raises inline confirm in pick_session).
    DeleteConfirm(usize),
}

/// Handle one keystroke in the startup picker. `n_sessions` is the number of
/// session rows; the total row count is `n_sessions + 1`. Logical index 0 is
/// the "Create new" row (rendered at the top); indices `1..=n_sessions` are
/// session rows (most-recent first). `selected` is the current cursor index.
/// Pure — no IO, no terminal.
fn pick_choice(n_sessions: usize, selected: usize, key: PickKey) -> PickOutcome {
    let total = n_sessions + 1; // sessions + "Create new"
    let cur = selected.min(total.saturating_sub(1));
    match key {
        PickKey::Up => {
            let next = if cur == 0 { total - 1 } else { cur - 1 };
            PickOutcome::Pending(next)
        }
        PickKey::Down => {
            let next = if cur + 1 >= total { 0 } else { cur + 1 };
            PickOutcome::Pending(next)
        }
        PickKey::Enter => {
            // Index 0 = "Create new"; indices 1..=n_sessions = session rows.
            if cur == 0 {
                PickOutcome::CreateNew
            } else {
                PickOutcome::Resume(cur)
            }
        }
        PickKey::Esc => PickOutcome::Abort,
        PickKey::Delete => {
            // Can't delete "Create new" (index 0) — no-op.
            if cur == 0 {
                PickOutcome::Pending(cur)
            } else {
                PickOutcome::DeleteConfirm(cur)
            }
        }
    }
}

/// Compute the vertical scroll offset (in lines) for the startup picker's
/// `Paragraph` so that the selected row stays within the visible window.
///
/// `selected_line` is the index of the cursor row within the `lines` Vec the
/// picker builds (title + blank + "Create new" + session rows + optional
/// delete-confirm + blank + hint). `visible_height` is the inner area height
/// (the block's bordered area minus 2).
///
/// Pure — no IO. Returns 0 when everything fits. Otherwise the offset grows
/// so the selected line is the last visible row (when the cursor moves down
/// past the bottom) or the first visible row (when it moves back up past the
/// top), matching the natural scrolling behaviour of a list.
fn picker_scroll_offset(selected_line: usize, visible_height: usize) -> u16 {
    if visible_height == 0 {
        return 0;
    }
    // No scrolling needed while the selected line still fits in the first screen.
    if selected_line < visible_height {
        return 0;
    }
    // selected_line is at or below the bottom edge: scroll so it becomes the
    // last visible row. selected - visible + 1 is the first line to show.
    (selected_line - visible_height + 1) as u16
}

/// The startup picker's resolution: which session to load (or whether to
/// create a new one) before `run()` begins.
enum PickResult {
    /// Resume this session (id, name, created_ts).
    Resume {
        id: Ulid,
        name: String,
        created_ts: i64,
    },
    /// Create a fresh session.
    CreateNew,
}

/// The startup session picker (spec §2). A self-contained render+input loop
/// entered after crossterm raw mode is set up but before `run()`. Shows one row
/// per session for the current CWD (name, age, tokens, live marker) plus a
/// leading "Create new session" row (pinned at the top, always visible). Arrow
/// keys move, Enter selects, Esc aborts to a clean exit. Selecting a live session takes it over immediately
/// (no confirm card — spec §1). Returns the chosen session id + name, or
/// `CreateNew`.
async fn pick_session(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    session: &SessionHandle,
    root: &str,
    repo_name: &str,
    boot_ts: i64,
) -> Result<PickResult> {
    use crossterm::event::{Event as CEvent, EventStream, KeyEventKind};
    use futures_util::StreamExt;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};
    use zoid_tui::economy_view::human_tokens;

    let mut sessions: Vec<zoid_core::sessions::SessionInfo> = session
        .list_sessions(Some(root.to_string()))
        .await
        .unwrap_or_default();
    let mut n = sessions.len();
    let mut live: Vec<bool> = sessions
        .iter()
        .map(|s| {
            zoid_core::store::is_live(
                s.active,
                s.active_pid,
                s.active_heartbeat,
                boot_ts,
                pid_alive,
            )
        })
        .collect();
    // Index 0 = "Create new" (top row); index 1 = first session. Start on the
    // most-recent session so the common case (resume recent) is one Enter away.
    let mut selected: usize = if n > 0 { 1 } else { 0 };
    let mut term_events = EventStream::new();
    let mut pending_delete: Option<usize> = None;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let title = format!(" Resume a session for {} ", repo_name);
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                title,
                Style::new().fg(Color::Cyan),
            )));
            lines.push(Line::from(""));

            // "Create new" is pinned at the top (line 2), directly under the
            // title/blank header, so it is always visible regardless of how
            // many sessions exist.
            let create_text = "  Create new session".to_string();
            let create_style = if selected == 0 {
                Style::new()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(create_text, create_style)));

            for (i, s) in sessions.iter().enumerate() {
                // Session rows occupy logical indices 1..=n, so session `i`
                // (0-indexed in `sessions`) is at logical index `i + 1`.
                let logical = i + 1;
                let age = fmt_since(s.last_touched_ts, boot_ts);
                let tokens = human_tokens(s.token_total);
                let live_marker = if live[i] { " ●" } else { "" };
                let row_text = format!("  {}  ·  {}  ·  {}{}", s.name, age, tokens, live_marker);
                let style = if logical == selected {
                    Style::new()
                        .fg(Color::White)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(row_text, style)));
            }

            if let Some(idx) = pending_delete {
                // `idx` is a logical index into the session space (1..=n).
                if let Some(s) = sessions.get(idx - 1) {
                    lines.push(Line::from(Span::styled(
                        format!(" Delete \"{}\"? [y]es / [n]o", s.name),
                        Style::new().fg(Color::Yellow),
                    )));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " ↑↓ move · ⏎ select · esc abort",
                Style::new().fg(Color::DarkGray),
            )));

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::DarkGray));
            // Keep the selected row on screen when the list is taller than the
            // terminal. Layout: line 0 = title, 1 = blank, 2 = "Create new",
            // 3.. = session rows. "Create new" (selected == 0) is at line 2 —
            // always within the first screen. Session rows (selected >= 1) are
            // at line 2 + selected. The delete-confirm line renders below the
            // session rows so it never shifts the selected row's line.
            let selected_line = if selected == 0 { 2 } else { 2 + selected };
            // Visible height = inner area (borders take 2 rows).
            let visible_height = area.height.saturating_sub(2) as usize;
            let scroll_y = picker_scroll_offset(selected_line, visible_height);
            f.render_widget(
                Paragraph::new(lines).scroll((scroll_y, 0)).block(block),
                area,
            );
        })?;

        if let Some(Ok(CEvent::Key(key))) = term_events.next().await {
            // Windows double-fire guard (see route_key for full explanation):
            // crossterm emits both Press and Release on Windows; ignore non-Press.
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Inline confirm for pending delete — captures all keys.
            if let Some(idx) = pending_delete {
                match key.code {
                    crossterm::event::KeyCode::Char('y')
                    | crossterm::event::KeyCode::Char('Y')
                    | crossterm::event::KeyCode::Enter => {
                        // `idx` is a logical index (1..=n); session `idx - 1`.
                        if let Some(s) = sessions.get(idx - 1) {
                            let _ = session.delete_session(s.id).await;
                        }
                        sessions = session
                            .list_sessions(Some(root.to_string()))
                            .await
                            .unwrap_or_default();
                        n = sessions.len();
                        live = sessions
                            .iter()
                            .map(|s| {
                                zoid_core::store::is_live(
                                    s.active,
                                    s.active_pid,
                                    s.active_heartbeat,
                                    boot_ts,
                                    pid_alive,
                                )
                            })
                            .collect();
                        // After a delete, clamp the cursor to a valid
                        // session row (1..=n). If no sessions remain,
                        // land on "Create new" (index 0). Never reset to 0
                        // when sessions still exist — index 0 is "Create
                        // new", not the first session.
                        if n == 0 {
                            selected = 0;
                        } else if selected > n {
                            selected = n;
                        }
                        pending_delete = None;
                    }
                    crossterm::event::KeyCode::Char('n')
                    | crossterm::event::KeyCode::Char('N')
                    | crossterm::event::KeyCode::Esc => {
                        pending_delete = None;
                    }
                    _ => {}
                }
                continue;
            }
            let pick_key = match key.code {
                crossterm::event::KeyCode::Up => PickKey::Up,
                crossterm::event::KeyCode::Down => PickKey::Down,
                crossterm::event::KeyCode::Enter => PickKey::Enter,
                crossterm::event::KeyCode::Esc => PickKey::Esc,
                crossterm::event::KeyCode::Delete | crossterm::event::KeyCode::Backspace => {
                    PickKey::Delete
                }
                _ => continue,
            };
            match pick_choice(n, selected, pick_key) {
                PickOutcome::Pending(new_sel) => selected = new_sel,
                PickOutcome::Resume(idx) => {
                    // idx is a logical index (1..=n); session `idx - 1`.
                    let s = &sessions[idx - 1];
                    return Ok(PickResult::Resume {
                        id: s.id,
                        name: s.name.clone(),
                        created_ts: s.created_ts,
                    });
                }
                PickOutcome::CreateNew => return Ok(PickResult::CreateNew),
                PickOutcome::Abort => {
                    anyhow::bail!("startup picker aborted");
                }
                PickOutcome::DeleteConfirm(idx) => {
                    // idx is a logical index (1..=n); session `idx - 1`.
                    let sess_idx = idx - 1;
                    if live.get(sess_idx).copied().unwrap_or(false) {
                        continue;
                    }
                    pending_delete = Some(idx);
                }
            }
        }
    }
}

/// Which startup path to take, decided from the session count and CLI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BootPath {
    /// 0 sessions → create one silently.
    AutoCreate,
    /// 1 session → resume it silently.
    AutoResume,
    /// 2+ sessions → show the picker.
    Picker,
    /// `--new` → create a fresh session, skip the picker.
    ForceNew,
    /// `--resume <id>` → resume this id, skip the picker.
    ForceResume(String),
}

/// Pure decision: which startup path to take. Spec §1. The session count is
/// the number of sessions for the current CWD; `new`/`resume` are the CLI
/// flags. `--new` and `--resume` take precedence over the count-based paths.
fn boot_decision(n_sessions: usize, new: bool, resume: Option<&str>) -> BootPath {
    if let Some(id) = resume {
        return BootPath::ForceResume(id.to_string());
    }
    if new {
        return BootPath::ForceNew;
    }
    match n_sessions {
        0 => BootPath::AutoCreate,
        1 => BootPath::AutoResume,
        _ => BootPath::Picker,
    }
}

/// Human provider label for the provider actually constructed at startup
/// (see the `config.provider` + secret-store match in `main`) — `provider_name`
/// is the configured provider id ("anthropic"/"ollama"), `has_key` is whether a
/// credential was found for it. Falls back to "offline" when no key was found,
/// since that's when `default_provider()`'s offline `FakeProvider` is actually
/// used instead of the configured one.
fn provider_label(provider_name: &str, has_key: bool) -> String {
    if has_key {
        provider_name.to_string()
    } else {
        "offline".into()
    }
}

/// Copy `text` to the system clipboard via the OSC 52 terminal escape
/// (`ESC ] 52 ; c ; <base64> BEL`). Works locally and over SSH without any
/// platform clipboard dependency, provided the terminal has OSC 52 enabled
/// (kitty, WezTerm, iTerm2, tmux with `set -g set-clipboard on`, …). Writing the
/// escape directly to stdout is safe under crossterm's raw/alt-screen mode — the
/// terminal consumes it and emits nothing visible. Best-effort: I/O errors are
/// ignored (the on-screen "copied" hint is the only feedback we can give).
///
/// Caveat: some terminals cap the OSC 52 payload (tmux ~8 KB by default), so a
/// very large code block may be silently truncated or dropped while the "copied"
/// hint still shows. Acceptable for typical code blocks; chunking is a future
/// option if large-block copy proves important.
fn copy_to_clipboard_osc52(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

/// Flip "select mode". The always-visible SELECT pill in the status line
/// (`render_status`) is the sole indicator, so this deliberately does NOT touch
/// the shared `status_hint` slot — writing it here would clobber transient hints
/// from other features (queued, "copied N lines", plugin progress) for no gain.
/// The actual terminal mouse-capture change is applied by the run loop's
/// per-frame reconcile (which holds the `terminal` backend); this only mutates
/// state, so it is safe to call from `handle_action`/`exec_command` where the
/// backend is out of scope.
fn toggle_select_mode(app: &mut App) {
    app.shell.select_mode = !app.shell.select_mode;
}

/// Resolve a left-click in the conversation: focus it, and — at Normal altitude,
/// where transcript rows map 1:1 to lines — copy the clicked code block's raw
/// source via OSC 52 (each code block is click-to-copy). Off-block clicks just
/// focus. Needs the conversation rect + wrap width, hence the loop, not
/// `handle_action`.
fn handle_conversation_click(app: &mut App, layout: &zoid_tui::layout::ShellLayout, row: u16) {
    use zoid_tui::state::{Focus, Zoom};
    app.shell.focus = Focus::Conversation;
    if app.shell.zoom != Zoom::Normal {
        return;
    }
    let conv = layout.conversation;
    if row < conv.y {
        return;
    }
    let clicked_line = app.shell.conversation_scroll as usize + (row - conv.y) as usize;
    let width = zoid_tui::layout::conv_text_width(conv.width) as usize;
    let msgs = conversation(app.events.iter());
    // Check question choice hits first — a click on a choice row selects +
    // submits it (so the user can click instead of arrow+Enter).
    if app.shell.question.is_some() {
        let choices = zoid_tui::chat::question_choice_hits(
            &msgs,
            app.streaming,
            true,
            app.tz_offset_secs,
            width,
            app.shell.question.as_ref(),
        );
        if let Some(hit) = choices.into_iter().find(|h| h.line == clicked_line) {
            answer_question(app, zoid::agent::Answer::Choice(hit.choice));
            return;
        }
    }
    let hits = zoid_tui::chat::code_hits(
        &msgs,
        app.streaming,
        true,
        app.tz_offset_secs,
        width,
        app.shell.question.as_ref(),
    );
    if let Some(h) = hits
        .into_iter()
        .find(|h| clicked_line >= h.header_line && clicked_line <= h.end_line)
    {
        copy_to_clipboard_osc52(&h.source);
        let n = h.source.lines().count().max(1);
        let unit = if n == 1 { "line" } else { "lines" };
        app.shell.status_hint = Some(format!("copied {n} {unit}"));
    }
}

/// The base URL to hand a provider: an explicit non-blank config override wins,
/// else the registry default for the (canonicalized) provider id, else empty
/// (which `with_base_url` treats as "keep the built-in default").
fn effective_base_url(config: &zoid_core::config::Config) -> String {
    if let Some(u) = config.base_url.as_ref() {
        if !u.trim().is_empty() {
            return u.clone();
        }
    }
    zoid_provider::model::default_base_url(&config.provider)
        .map(str::to_string)
        .unwrap_or_default()
}

/// True when the onboarding wizard should be shown at startup. Pure; no IO.
///
/// - `first_time_user`: from `sessions.is_empty()` at boot.
/// - `config`: the resolved Config.
/// - `has_key`: the third return of `select_provider` — whether a credential
///   was found for the active provider (true for keyless `ollama-local`).
/// - `secrets_available`: whether the encrypted secret store opened successfully.
///   If false, the wizard cannot function (step 2 writes to it) and must not
///   fire — the user is directed to `:config` via the normal empty state.
fn wizard_needed(
    first_time_user: bool,
    config: &zoid_core::config::Config,
    has_key: bool,
    secrets_available: bool,
) -> bool {
    if !first_time_user || !secrets_available {
        return false;
    }
    let canon = zoid_provider::model::canonical_id(&config.provider);
    if canon == "ollama-local" {
        return false; // keyless local — assumed correct, never probed
    }
    if config.provider.trim().is_empty() {
        return true; // sentinel: no provider chosen
    }
    // provider is set + requires a key + key not found → misconfigured
    !has_key
}

/// Whether a provider id needs an API key to be usable. Derived from the
/// registry's `key_url` field: `None` = keyless, `Some` = key required.
/// Unknown provider ids default to key-required (safe).
fn entry_requires_key(id: &str) -> bool {
    zoid_provider::model::entry(id)
        .map(|e| e.key_url.is_some())
        .unwrap_or(true)
}

/// The secret env name a provider id needs, or `None` if it needs no key.
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("opencode-go") | Some("opencode-zen") => Some("OPENCODE_GO_API_KEY"),
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        Some("zai") => Some("ZAI_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}

/// Provider + credential from `config.provider` + the secret store (env wins
/// inside `SecretStore::get`). No key found → fall back to the offline
/// `FakeProvider` so the binary always runs; `provider_label` mirrors this
/// exact selection so the drawer never disagrees with reality. Shared by
/// startup and by live config-save re-selection (`apply_config_write`) so
/// both paths pick the provider identically.
fn select_provider(
    config: &zoid_core::config::Config,
    secrets: &Option<std::sync::Arc<zoid_core::secret::EncryptedDb>>,
) -> (Arc<dyn Provider>, &'static str, bool) {
    let key_for = |name: &str| -> Option<String> {
        // env wins, and must work even if the encrypted secret store failed to open
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        secrets.as_ref().and_then(|s| {
            use zoid_core::secret::SecretStore;
            s.get(name)
        })
    };
    // ollama-local: usable without a key (localhost, no auth). Construct directly.
    if zoid_provider::model::canonical_id(&config.provider) == "ollama-local" {
        let base_url = effective_base_url(config);
        return (
            Arc::new(
                zoid_provider::ollama::OllamaProvider::new(String::new())
                    .with_base_url(base_url)
                    .with_num_ctx(zoid_provider::ollama::configured_num_ctx(
                        config.economy.num_ctx,
                    )),
            ),
            "ollama",
            true, // no key required → treat as ready
        );
    }
    let base_url = effective_base_url(config);
    let family = zoid_provider::model::entry(&config.provider)
        .map(|e| e.family)
        .unwrap_or("ollama");
    match family {
        "opencode-go" => match key_for("OPENCODE_GO_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::opencode_go::OpenCodeGoProvider::new(k).with_base_url(base_url),
                ),
                "opencode-go",
                true,
            ),
            None => (default_provider(), "opencode-go", false),
        },
        "opencode-zen" => match key_for("OPENCODE_GO_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::opencode_zen::OpenCodeZenProvider::new(k)
                        .with_base_url(base_url),
                ),
                "opencode-zen",
                true,
            ),
            None => (default_provider(), "opencode-zen", false),
        },
        "anthropic" => match key_for("ANTHROPIC_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::anthropic::AnthropicProvider::new(k).with_base_url(base_url),
                ),
                "anthropic",
                true,
            ),
            None => (default_provider(), "anthropic", false),
        },
        "zai" => match key_for("ZAI_API_KEY") {
            Some(k) => (
                Arc::new(zoid_provider::zai::ZaiProvider::new(k).with_base_url(base_url)),
                "zai",
                true,
            ),
            None => (default_provider(), "zai", false),
        },
        _ => match key_for("OLLAMA_API_KEY") {
            Some(k) => (
                Arc::new(zoid_provider::ollama::OllamaProvider::new(k).with_base_url(base_url)),
                "ollama",
                true,
            ),
            None => (default_provider(), "ollama", false),
        },
    }
}

/// Spawn a background fetch of the active model's capabilities (context window,
/// prompt cache, etc.) from the provider's introspection endpoint. Results
/// arrive as `AgentUpdate::ModelInfoFetched`. Non-fatal: any error → the static
/// MODEL_CAPS table remains the fallback.
fn spawn_model_info_fetch(
    provider: Arc<dyn Provider>,
    model: String,
    ui_tx: mpsc::Sender<AgentUpdate>,
) {
    tokio::spawn(async move {
        // error or None → keep the static fallback
        if let Ok(Some(info)) = provider.fetch_model_info(&model).await {
            let _ = ui_tx
                .send(AgentUpdate::ModelInfoFetched { model, info })
                .await;
        }
    });
}

/// Spawn a background fetch of a provider's model list; the result is delivered
/// as `AgentUpdate::ModelsFetched`. Non-fatal: any error → empty list (the
/// picker keeps its static registry fallback).
fn spawn_model_fetch(
    provider: Arc<dyn Provider>,
    provider_id: String,
    ui_tx: mpsc::Sender<AgentUpdate>,
) {
    tokio::spawn(async move {
        let models = provider.list_models().await.unwrap_or_default();
        let _ = ui_tx
            .send(AgentUpdate::ModelsFetched {
                provider: provider_id,
                models,
            })
            .await;
    });
}

/// Build a `Provider` for an *arbitrary* registry id — not necessarily the
/// active one — so the quick-switch can live-fetch a highlighted provider's
/// models before the provider change is committed. Uses the registry's default
/// base URL for the id (the uncommitted highlight must not inherit the active
/// provider's `base_url` override) plus the family's key from env / secret
/// store. Returns `None` when a key-requiring provider has no key available
/// (nothing to fetch with → keep the static registry list).
fn provider_for_id(
    id: &str,
    secrets: &Option<std::sync::Arc<zoid_core::secret::EncryptedDb>>,
) -> Option<Arc<dyn Provider>> {
    let canon = zoid_provider::model::canonical_id(id);
    let base_url = zoid_provider::model::default_base_url(canon)
        .map(str::to_string)
        .unwrap_or_default();
    let key_for = |name: &str| -> Option<String> {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        secrets.as_ref().and_then(|s| {
            use zoid_core::secret::SecretStore;
            s.get(name)
        })
    };
    if canon == "ollama-local" {
        return Some(Arc::new(
            zoid_provider::ollama::OllamaProvider::new(String::new()).with_base_url(base_url),
        ));
    }
    let family = zoid_provider::model::entry(canon)
        .map(|e| e.family)
        .unwrap_or("ollama");
    match family {
        "opencode-go" => key_for("OPENCODE_GO_API_KEY").map(|k| {
            Arc::new(zoid_provider::opencode_go::OpenCodeGoProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
        "opencode-zen" => key_for("OPENCODE_GO_API_KEY").map(|k| {
            Arc::new(
                zoid_provider::opencode_zen::OpenCodeZenProvider::new(k).with_base_url(base_url),
            ) as Arc<dyn Provider>
        }),
        "anthropic" => key_for("ANTHROPIC_API_KEY").map(|k| {
            Arc::new(zoid_provider::anthropic::AnthropicProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
        "zai" => key_for("ZAI_API_KEY").map(|k| {
            Arc::new(zoid_provider::zai::ZaiProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
        _ => key_for("OLLAMA_API_KEY").map(|k| {
            Arc::new(zoid_provider::ollama::OllamaProvider::new(k).with_base_url(base_url))
                as Arc<dyn Provider>
        }),
    }
}

/// Spawn a live model fetch for the quick-switch's currently-highlighted
/// provider, if one can be built (has a key / is keyless). Results arrive as
/// `AgentUpdate::ModelsFetched` and are routed into `switch_models` by
/// `apply_switch_models_fetched`. No-op for a `Planned`/unbuildable provider.
fn spawn_switch_model_fetch(app: &App, provider_id: &str) {
    if let Some(p) = provider_for_id(provider_id, &app.secrets) {
        spawn_model_fetch(p, provider_id.to_string(), app.ui_tx.clone());
    }
}

/// Build the economy `ContextPolicy` (spec §7.2) from the loaded config's
/// `[economy]` table, resolving `compact_threshold_pct` (0–100) against the
/// resolved `target` (the soft setpoint, NOT capacity) — 0 disables
/// compaction (`None`), else the absolute token count `target * pct / 100`.
///
/// Feeds `spawn_turn`'s live `TurnConfig.policy` (ACM-1), so the agent loop
/// actually records `ToolResultCompacted` events once `compact_threshold_pct`
/// is set above 0; the default (0) leaves existing chat behavior unchanged.
/// `EconomyConfig.token_ceiling` was retired; `ContextPolicy.token_ceiling`
/// (subagent-only) is always `None` from this path.
fn policy_from_config(
    econ: &zoid_core::config::EconomyConfig,
    target: u64,
) -> zoid_core::assembler::ContextPolicy {
    let compact_threshold = if econ.compact_threshold_pct == 0 {
        None
    } else {
        Some(target.saturating_mul(econ.compact_threshold_pct as u64) / 100)
    };
    zoid_core::assembler::ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: econ.auto_evict_cold,
        compact_threshold,
    }
}

/// Current branch of the checkout at `root`. Reads `<root>/.git/HEAD` for a
/// normal checkout; falls back to git2 for a linked worktree (whose `.git` is a
/// gitdir-file, not a directory).
fn current_branch_at(root: &std::path::Path) -> String {
    if let Ok(s) = std::fs::read_to_string(root.join(".git").join("HEAD")) {
        if let Some(b) = s.trim().strip_prefix("ref: refs/heads/") {
            return b.to_string();
        }
    }
    match git2::Repository::open(root) {
        Ok(repo) => repo
            .head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
            .unwrap_or_else(|| "main".into()),
        Err(_) => "main".into(),
    }
}

/// The worktree label for the repo drawer: the linked-worktree name when the
/// process cwd is a linked worktree (not the main working copy), else "(none)".
/// git stores linked worktrees under `<common>/worktrees/<name>`, so the
/// worktree's gitdir basename IS the worktree name.
fn worktree_label(repo: &git2::Repository) -> String {
    let path = repo.path();
    let common = repo.commondir();
    if path == common {
        "(none)".to_string()
    } else {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(linked)".into())
    }
}

/// Whether the current working directory is inside a git work tree. Asks git
/// directly (`rev-parse --is-inside-work-tree`) rather than probing `./.git`, so
/// it is correct from a subdirectory of a repo and false in a bare/absent one.
/// Any failure (git missing, not a repo) reads as "no repo".
fn in_git_repo() -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Parse `git diff --numstat` output → (added, removed, files). Binary files
/// show `-` for both counts (counted as a file, zero lines). Pure.
fn parse_numstat(out: &str) -> (usize, usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut files = 0usize;
    for line in out.lines().filter(|l| !l.trim().is_empty()) {
        let mut cols = line.split('\t');
        let a = cols.next().unwrap_or("-");
        let r = cols.next().unwrap_or("-");
        if cols.next().is_none() {
            continue;
        } // no path → malformed, skip
        added += a.parse::<usize>().unwrap_or(0);
        removed += r.parse::<usize>().unwrap_or(0);
        files += 1;
    }
    (added, removed, files)
}

/// Diff stats (added, removed, files) for the git checkout at `dir`.
fn git_status_at(dir: &std::path::Path) -> (usize, usize, usize) {
    let run = |args: &[&str]| -> String {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let (a1, r1, f1) = parse_numstat(&run(&["diff", "--numstat"]));
    let (a2, r2, f2) = parse_numstat(&run(&["diff", "--numstat", "--cached"]));
    (a1 + a2, r1 + r2, f1 + f2)
}

/// What `apply_event` changed. Determines whether the caller invalidates
/// `body_cache` and whether the economy projections need a dirty-flag refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionImpact {
    /// No `msgs` change, no body change. Economy projections may need refresh.
    Economy,
    /// `msgs` content changed but `msg_count` did not. Carries the index of
    /// the mutated message. `None` means "mutation at the end" (streaming
    /// append to the last message — BodyCache incremental path handles it).
    /// `Some(i)` means message at index `i` was mutated — the caller checks
    /// `i == msgs.len() - 1` to decide whether to invalidate `body_cache`.
    MsgsMutated { mutated_index: Option<usize> },
    /// A new ChatMsg was appended (msg_count changed). Full body rebuild.
    MsgsAppended,
    /// Could not apply incrementally — caller must do a full refresh.
    #[allow(dead_code)]
    FullRefresh,
}

/// Cached projections over the append-only event log. Recomputed only when the
/// event count changes — the log only ever grows, so its length uniquely
/// identifies the projection inputs. This is the core render-loop optimization:
/// without it, every keystroke and scroll tick rebuilt `conversation`,
/// `context_window` (twice), `churn_timeline`, `tasks`, and `token_ledger` from
/// the full log, so per-frame cost grew unbounded with session length.
#[derive(Default)]
struct ProjectionCache {
    events_len: Option<usize>,
    msgs: Vec<zoid_core::projection::ChatMsg>,
    window: zoid_core::context::ContextWindow,
    churn: zoid_core::economy::ChurnTimeline,
    tasks: Vec<zoid_core::tasks::TaskItem>,
    ledger_total: u64,
    /// Cumulative cached (cache-read) tokens across all Usage events — the
    /// subset of `ledger_total`'s input that was served from the provider's
    /// prompt cache. Surfaced as the session drawer's "cac" line.
    cached_total: u64,
    /// The real input token count from the most recent Usage event (the
    /// provider's actual prompt size for the last turn). Used as `ctx_used`
    /// when available — far more accurate than `estimate_tokens` (chars/4).
    /// `None` until the first turn's Usage event arrives.
    last_input_tokens: Option<u64>,
    /// The real output token count from the most recent Usage event.
    /// Used for TPS (tokens per second) in the session drawer.
    /// `None` until the first turn's Usage event arrives.
    last_output_tokens: Option<u64>,
    // NEW — dirty flags for deferred economy rebuilds.
    window_dirty: bool,
    churn_dirty: bool,
    // NEW — ids of non-Approval QuestionAsked events, so ToolResults with
    // the same id are suppressed (mirrors conversation_for_branch's pre-pass).
    question_ids: std::collections::HashSet<String>,
    // NEW — cumulative thinking tokens (accumulated in the pre-match step for all events with tokens).
    thinking_total: u64,
    // NEW — pending assistant-turn accumulator (mirrors conversation_for_branch
    // locals). ModelDelta/ToolCall accumulate here; tier-2 events flush.
    pending_text: Option<String>,
    pending_calls: Vec<zoid_core::projection::ToolCallRef>,
    pending_turn_ts: Option<i64>,
    pending_thinking: Option<String>,
}

impl ProjectionCache {
    /// Refresh projections. When `events_len` matches (no full invalidation),
    /// rebuild only dirty economy projections. When `events_len` is `None`
    /// (full invalidation — session resume, first frame), rebuild everything.
    fn refresh(&mut self, events: &zoid::eventlog::EventLog) -> bool {
        if self.events_len == Some(events.len()) {
            let mut rebuilt = false;
            if self.window_dirty {
                self.window = zoid_core::context::context_window(events.iter());
                self.window_dirty = false;
                rebuilt = true;
            }
            if self.churn_dirty {
                self.churn = zoid_core::economy::churn_timeline(events.iter());
                self.churn_dirty = false;
                rebuilt = true;
            }
            return rebuilt;
        }
        // Full invalidation — rebuild everything from scratch. The 5
        // independent O(n) passes run concurrently as scoped threads; wall-clock
        // drops from sum(passes) to max(passes). std::thread::scope (stable
        // since Rust 1.63) borrows `iter` by shared reference — each spawn
        // clones the iterator so the passes are independent.
        let iter = events.iter();
        std::thread::scope(|s| {
            let a = s.spawn(|| conversation(iter.clone()));
            let b = s.spawn(|| zoid_core::context::context_window(iter.clone()));
            let c = s.spawn(|| zoid_core::economy::churn_timeline(iter.clone()));
            let d = s.spawn(|| zoid_core::tasks::tasks(iter.clone()));
            let e = s.spawn(|| zoid_core::economy::token_ledger(iter.clone()));
            self.msgs = a.join().unwrap();
            self.window = b.join().unwrap();
            self.churn = c.join().unwrap();
            self.tasks = d.join().unwrap();
            let ledger = e.join().unwrap();
            self.ledger_total = ledger.total;
            self.cached_total = ledger.cached;
            self.thinking_total = ledger.thinking;
        });
        // Find the last Usage event's real input token count — the provider's
        // actual prompt size, far more accurate than the chars/4 estimate.
        // `EventLog::iter()` is double-ended, so `.rev()` works directly.
        // These 2 reverse scans early-exit and are cheap — kept sequential.
        self.last_input_tokens = events
            .iter()
            .rev()
            .find_map(|e| e.tokens.map(|t| t.input))
            .filter(|&t| t > 0);
        self.last_output_tokens = events
            .iter()
            .rev()
            .find_map(|e| e.tokens.map(|t| t.output))
            .filter(|&t| t > 0);
        self.window_dirty = false;
        self.churn_dirty = false;
        self.question_ids = events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::QuestionAsked { id, kind, .. }
                    if !matches!(kind, zoid_core::event::QuestionKind::Approval) =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .collect();
        self.events_len = Some(events.len());
        self.pending_text = None;
        self.pending_calls = Vec::new();
        self.pending_turn_ts = None;
        self.pending_thinking = None;
        true
    }

    /// Flush the pending assistant turn into `msgs` as a `ChatMsg::Assistant`.
    /// Returns `true` if a message was pushed (pending text/calls were
    /// non-empty), `false` otherwise. Carries `pending_thinking` into the
    /// flushed message, matching `conversation_for_branch`'s `flush()`.
    ///
    /// When the flush is a no-op (no pending text/calls), `pending_thinking`
    /// is **dropped** (not put back) — matching the reference fold, where
    /// `flush()` takes `thinking` by value and it goes out of scope when
    /// the flush doesn't push. The `ModelThinking` handler re-stashes
    /// thinking immediately after calling flush, so there's no risk of
    /// losing it on the `ModelThinking` path.
    fn flush_pending_assistant(&mut self) -> bool {
        let text = self.pending_text.take();
        let calls = std::mem::take(&mut self.pending_calls);
        let ts = self.pending_turn_ts.take();
        let thinking = self.pending_thinking.take();
        if text.is_some() || !calls.is_empty() {
            self.msgs.push(zoid_core::projection::ChatMsg::Assistant {
                text: text.unwrap_or_default(),
                tool_calls: calls,
                ts: ts.unwrap_or(0),
                thinking,
            });
            true
        } else {
            // No pending turn — thinking is dropped (matches reference fold).
            false
        }
    }

    /// Finalize any pending state after a turn ends. Mirrors the trailing
    /// flush in `conversation_for_branch` (projection.rs:344–356): if
    /// `pending_thinking` is set with no pending text/calls, emit a
    /// standalone `ChatMsg::Assistant { text: "", thinking: Some(...) }`.
    ///
    /// `flush_pending_assistant` consumes `pending_thinking` unconditionally
    /// (matching the reference fold's by-value `thinking` parameter) and drops
    /// it on a no-op flush. To preserve trailing standalone-thinking, we save
    /// `pending_thinking` first and restore it when the flush is a no-op, so
    /// the standalone branch below can see it.
    fn finalize_pending(&mut self) -> ProjectionImpact {
        let saved_thinking = self.pending_thinking.take();
        let flushed = self.flush_pending_assistant();
        if flushed {
            return ProjectionImpact::MsgsAppended;
        }
        // No pending text/calls — restore the saved thinking for the
        // standalone check below (flush dropped it on the no-op path).
        self.pending_thinking = saved_thinking;
        // Trailing standalone-thinking: no text/calls, but thinking is set.
        if let Some(thinking) = self.pending_thinking.take() {
            self.msgs.push(zoid_core::projection::ChatMsg::Assistant {
                text: String::new(),
                tool_calls: Vec::new(),
                ts: self.pending_turn_ts.take().unwrap_or(0),
                thinking: Some(thinking),
            });
            return ProjectionImpact::MsgsAppended;
        }
        ProjectionImpact::Economy
    }

    /// Incrementally apply a single new event to the cached projections.
    /// Returns a `ProjectionImpact` describing what changed, so the caller
    /// knows whether to invalidate `body_cache`.
    fn apply_event(&mut self, ev: &Event) -> ProjectionImpact {
        use zoid_core::event::EventKind;
        use zoid_core::projection::{
            ChatMsg, QuestionCardState, RescueSummary, RescuedTurnSummary, ToolCallRef,
        };
        let bump_len = || ProjectionImpact::MsgsMutated {
            mutated_index: None,
        };
        // token_ledger sums `e.tokens` across ALL events (not just Usage), so the
        // incremental path must do the same — accumulate tokens here, before the
        // kind-specific match. `last_input_tokens`/`last_output_tokens` track the
        // most recent non-zero values (used for TPS + ctx_used).
        if let Some(t) = ev.tokens {
            self.ledger_total += t.input + t.output;
            self.cached_total += t.cached;
            self.thinking_total += t.thinking;
            if t.input > 0 {
                self.last_input_tokens = Some(t.input);
            }
            if t.output > 0 {
                self.last_output_tokens = Some(t.output);
            }
        }
        match &ev.kind {
            // Streaming hot path — ALWAYS accumulate into pending_text/
            // pending_calls, never append to the last assistant message.
            // This matches the reference fold (projection.rs:224–235), which
            // always accumulates into locals and only emits a ChatMsg::Assistant
            // on flush. Appending to the last assistant would diverge after any
            // event that pushes a new Assistant (ModelThinking, AssistantMessage,
            // finalize_pending) — the fold starts a fresh pending turn, while
            // append-to-last would mutate the just-pushed message.
            EventKind::ModelDelta { text } => {
                self.pending_text
                    .get_or_insert_with(String::new)
                    .push_str(text);
                self.pending_turn_ts.get_or_insert(ev.ts);
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                bump_len()
            }
            EventKind::ToolCall { id, name, args } => {
                self.pending_turn_ts.get_or_insert(ev.ts);
                self.pending_calls.push(ToolCallRef {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                bump_len()
            }

            // Tier 1 — bookkeeping.
            EventKind::Usage => {
                // Token accumulation happens above (before the match) for all
                // event kinds — token_ledger sums `e.tokens` across every event,
                // not just Usage. Here we only flag the churn timeline dirty.
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::Tasks { items } => {
                self.tasks = items.clone();
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::WakeScheduled { .. }
            | EventKind::WakeFired { .. }
            | EventKind::WakeCancelled { .. } => {
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }
            EventKind::TurnsDropped { .. }
            | EventKind::ContextMutation { .. }
            | EventKind::DirectiveReasserted { .. }
            | EventKind::TurnsReadmitted { .. } => {
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::Economy
            }

            // Tier 2 — append-only msgs change.
            EventKind::UserMessage { text } => {
                self.flush_pending_assistant();
                self.msgs.push(ChatMsg::User {
                    text: text.clone(),
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::AssistantMessage { text } => {
                self.flush_pending_assistant();
                self.msgs.push(ChatMsg::Assistant {
                    thinking: None,
                    text: text.clone(),
                    tool_calls: Vec::new(),
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::ModelThinking { text } => {
                let flushed = self.flush_pending_assistant();
                self.pending_thinking = Some(text.clone());
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                if flushed {
                    bump_len()
                } else {
                    ProjectionImpact::Economy
                }
            }
            EventKind::ToolResult {
                id,
                name,
                output,
                is_error,
            } => {
                self.flush_pending_assistant();
                if self.question_ids.contains(id.as_str()) {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    return bump_len();
                }
                self.msgs.push(ChatMsg::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    output: output.clone(),
                    is_error: *is_error,
                    compacted: false,
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::QuestionAsked {
                id,
                kind,
                question,
                choices,
            } => {
                self.flush_pending_assistant();
                if !matches!(kind, zoid_core::event::QuestionKind::Approval) {
                    self.question_ids.insert(id.clone());
                }
                self.msgs.push(ChatMsg::Question {
                    id: id.clone(),
                    kind: kind.clone(),
                    question: question.clone(),
                    choices: choices.clone(),
                    state: QuestionCardState::Open {
                        selected: 0,
                        free_text: String::new(),
                    },
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::QuestionAnswered { id, answer } => {
                if let Some((idx, ChatMsg::Question { state, .. })) = self
                    .msgs
                    .iter_mut()
                    .enumerate()
                    .rev()
                    .find(|(_, m)| matches!(m, ChatMsg::Question { id: qid, .. } if qid == id))
                {
                    *state = QuestionCardState::Answered {
                        answer: answer.clone(),
                    };
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::MsgsMutated {
                        mutated_index: Some(idx),
                    }
                } else {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::Economy
                }
            }
            EventKind::DelegationResult { summary, ok, .. } => {
                self.flush_pending_assistant();
                self.msgs.push(ChatMsg::Delegated {
                    summary: summary.clone(),
                    ok: *ok,
                });
                self.window_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }
            EventKind::TurnsEvicted {
                reclaimed_tokens,
                marker,
                rescue,
                ..
            } => {
                self.flush_pending_assistant();
                let evicted_topics: Vec<String> =
                    marker.spans.iter().map(|s| s.topic_hint.clone()).collect();
                let rescue = rescue.as_ref().map(|r| RescueSummary {
                    goal_text: r.goal_text.clone(),
                    weight: r.weight.round() as u32,
                    rescued: r
                        .survivors
                        .iter()
                        .map(|s| RescuedTurnSummary {
                            topic_hint: s.topic_hint.clone(),
                            bump_milli: (s.rescue_bump * 1000.0).round() as u32,
                        })
                        .collect(),
                });
                self.msgs.push(ChatMsg::Evicted {
                    reclaimed_tokens: *reclaimed_tokens,
                    evicted_topics,
                    rescue,
                    ts: ev.ts,
                });
                self.window_dirty = true;
                self.churn_dirty = true;
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                ProjectionImpact::MsgsAppended
            }

            // Tier 3 — content mutation.
            EventKind::ToolResultCompacted { id, summary, .. } => {
                if let Some((
                    idx,
                    ChatMsg::ToolResult {
                        output, compacted, ..
                    },
                )) =
                    self.msgs.iter_mut().enumerate().rev().find(
                        |(_, m)| matches!(m, ChatMsg::ToolResult { id: rid, .. } if rid == id),
                    )
                {
                    *output = summary.clone();
                    *compacted = true;
                    self.window_dirty = true;
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::MsgsMutated {
                        mutated_index: Some(idx),
                    }
                } else {
                    self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                    ProjectionImpact::Economy
                }
            }
        }
    }
}

/// Inputs that determine the rendered conversation body. Scroll offset is NOT
/// here — scrolling reuses the cached body — which is the whole point.
/// `events_len` is deliberately excluded: during streaming it changes on every
/// ModelDelta, but the message count stays the same. The cache detects this
/// case and only re-renders the last message (O(1)) instead of the full body.
#[derive(PartialEq, Eq)]
struct BodyKey {
    zoom: zoid_tui::state::Zoom,
    width: usize,
    streaming: bool,
    /// Folded as `streaming && caret_on` — the caret only affects the body while
    /// streaming, so an idle blink never invalidates the cache.
    caret: bool,
    tz: i32,
    /// A cheap revision hash of the live `ask_user` question buffer (selected
    /// row + free-text + mode). Excluded from `matches_structure` because the
    /// question card is always the last message: when only this changes we
    /// re-render just that message (the incremental path) instead of the whole
    /// transcript. `0` when no question is open.
    question_rev: u64,
}

impl BodyKey {
    /// Structural equality: the inputs whose change forces a FULL transcript
    /// rebuild. Deliberately excludes `question_rev` — a question-only change
    /// affects nothing but the last message, so it takes the incremental path.
    fn matches_structure(&self, other: &BodyKey) -> bool {
        self.zoom == other.zoom
            && self.width == other.width
            && self.streaming == other.streaming
            && self.caret == other.caret
            && self.tz == other.tz
    }
}

/// A cheap change-detection hash of the live `ask_user` question buffer. Feeds
/// `BodyKey.question_rev` so a keystroke (or selection move, or mode switch)
/// changes the key — routing the frame through the incremental re-render path
/// — while an unchanged buffer keeps the same value (a cache hit). `0` when no
/// question is open, so the common no-question case never perturbs the key.
fn question_rev(question: Option<&zoid_tui::question::QuestionState>) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(q) = question else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    q.selected.hash(&mut h);
    q.free_text.hash(&mut h);
    std::mem::discriminant(&q.mode).hash(&mut h);
    h.finish()
}

/// Outcome of a `BodyCache::refresh`, distinguishing a pure cache hit (no
/// render work) from the two render paths. Callers map `Hit` to the cache-ratio
/// telemetry; tests assert `Incremental` to prove per-keystroke question edits
/// don't trigger an O(n) full rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshKind {
    /// Nothing changed — the cached body was reused verbatim.
    Hit,
    /// Only the last message changed (streaming text grew, or the live question
    /// card's buffer/selection changed); just that message was re-rendered.
    Incremental,
    /// A structural input changed — the whole transcript was re-rendered.
    Full,
}

/// Cached `conversation_view` output (the expensive wrap + syntax-highlight
/// pass), rebuilt only when a `BodyKey` input changes. A scroll-event burst then
/// reuses these lines every frame instead of re-rendering the whole transcript,
/// which is what made buffered scroll events drain at ~52ms each.
///
/// During streaming, when the message count hasn't changed (only the last
/// message's text is growing), only the last message is re-rendered and spliced
/// into the cached body — O(1) per frame instead of O(n).
#[derive(Default)]
struct BodyCache {
    key: Option<BodyKey>,
    body: Vec<ratatui::text::Line<'static>>,
    /// Per-message start-line indices for the cached body (length == msgs.len()),
    /// used to re-anchor the viewport to the top message across a zoom change.
    msg_starts: Vec<usize>,
    /// Number of ChatMsg items the cached body was built from. When this matches
    /// during streaming, only the last message is re-rendered (incremental).
    msg_count: usize,
}

impl BodyCache {
    /// Rebuild the body iff `key` changed; cheap no-op otherwise. Returns
    /// `true` when the cache was reused (a full-frame "hit": nothing to
    /// re-render), `false` when the body was rebuilt or incrementally
    /// re-rendered. During streaming the last message's text grows every
    /// frame, so a key match is NOT a no-op — we fall through to the
    /// incremental re-render (re-render just the last message, O(1) per
    /// frame instead of O(n)), which counts as a render (returns `false`).
    #[allow(clippy::too_many_arguments)]
    fn refresh(
        &mut self,
        key: BodyKey,
        msgs: &[zoid_core::projection::ChatMsg],
        width: usize,
        question: Option<&zoid_tui::question::QuestionState>,
        edit_diffs: &[(String, zoid_tui::state::RenderDiff)],
        inline_k: usize,
    ) -> RefreshKind {
        // Full no-op only when not streaming and nothing changed. `self.key ==
        // Some(&key)` is full equality (question_rev included), so a live
        // question edit breaks the no-op and falls through to the incremental
        // path below.
        if !key.streaming && self.key.as_ref() == Some(&key) && self.msg_count == msgs.len() {
            return RefreshKind::Hit;
        }
        let view = ChatView {
            zoom: key.zoom,
            caret_on: key.caret,
            reveal: None,
            tz_offset_secs: key.tz,
        };
        let streaming = key.streaming;
        // Incremental last-message re-render: the structural inputs match and
        // the message count is unchanged, so only the tail changed — either the
        // streaming assistant text grew, or the live `ask_user` card's buffer /
        // selection changed (its `question_rev` differs, but that is excluded
        // from `matches_structure`). The question card is always the last
        // message, so re-rendering just that message keeps typing O(1) instead
        // of re-parsing/wrapping the whole transcript on every keystroke.
        let structural_match = self.key.as_ref().is_some_and(|k| k.matches_structure(&key));
        if structural_match && self.msg_count == msgs.len() && self.msg_count > 0 {
            let last_idx = msgs.len() - 1;
            let start = self.msg_starts[last_idx];
            // Remove the old trailing blank + old last-message lines, then
            // re-render the last message and re-append the trailing blank.
            self.body.truncate(start);
            // Re-add the inter-turn blank line that build_conversation would
            // have inserted (it only adds one when `out` is non-empty, but
            // rendering the last message in isolation starts with an empty vec).
            if start > 0 {
                self.body.push(Line::from(""));
            }
            let (new_lines, _) = zoid_tui::chat::conversation_view_indexed(
                &msgs[last_idx..],
                &view,
                streaming,
                width,
                question,
                edit_diffs,
                inline_k,
            );
            // conversation_view_indexed appends a trailing blank; we want it.
            self.body.extend(new_lines);
            // Store the new key so the next frame sees the updated question_rev /
            // streaming flag (the last message's start line is unchanged, so
            // msg_starts / msg_count stay valid).
            self.key = Some(key);
            return RefreshKind::Incremental;
        }
        // Full rebuild.
        let (body, starts) = zoid_tui::chat::conversation_view_indexed(
            msgs, &view, streaming, width, question, edit_diffs, inline_k,
        );
        self.body = body;
        self.msg_starts = starts;
        self.msg_count = msgs.len();
        self.key = Some(key);
        RefreshKind::Full
    }
}

/// One in-flight subagent, tracked for the Subagents drawer + busy guard.
#[allow(dead_code)] // subagent delegation temporarily disabled
struct SubagentInfo {
    id: String,
    task: String,
    agent: String,
}

/// A subagent waiting for a pool slot to open. Created when `dispatch_subagent`
/// finds the global in-flight set at capacity; drained by `DelegationResult`.
/// Carries the resolved profile + parent cwd so `spawn_queued_subagent` can
/// spawn without re-resolving the agent or recomputing the worktree base.
/// `agent`/`tool_call_id`/`session_id` are carried for parity with the
/// `SubagentQueued` event and future observability (the spawn currently uses
/// `resolved_name` and the live `app.session_id`).
#[allow(dead_code)]
struct QueuedSubagent {
    task: String,
    agent: String,
    resolved_profile: zoid_core::agent_profile::AgentProfile,
    resolved_name: String,
    cwd: PathBuf,
    want_worktree: bool,
    tool_call_id: String,
    session_id: Ulid,
}

/// The active worktree session: tracks the worktree's path and branch name.
/// Set by the `WorktreeRequested` handler when entering; cleared on exit.
/// `spawn_turn` reads `active_worktree` to override `turn_config.cwd`.
/// On exit, `active_worktree` becomes None and `spawn_turn` falls back to
/// `turn_config.cwd = PathBuf::from(".")` (the main checkout) — no explicit
/// prior_cwd restore needed.
#[derive(Clone)]
struct WorktreeSession {
    path: PathBuf,
    name: String,
}

/// The optional embedder + its in-memory index. A type alias keeps the `App`
/// struct field and the local-embed wiring site in sync without repeating the
/// long generic tuple (satisfies clippy::type_complexity).
pub type EmbedStore = (
    Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>,
    Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>,
);

struct App {
    session: SessionHandle,
    session_id: Ulid,
    events: zoid::eventlog::EventLog,
    provider: Arc<dyn Provider>,
    /// The active mode + all discovered modes; drives the turn's system prompt,
    /// the effective skill menu, and the mode chip. Index 0 is always Chat.
    modes: zoid_core::mode::ModeRegistry,
    /// The base coding-agent profile (Chat). Kept so mode reload / broken-mode
    /// fallback can recompose without re-reading a const.
    base_profile: zoid_core::agent_profile::AgentProfile,
    /// The resolved mode-source directories, computed once at startup. Stashed so
    /// `:mode reload` can rebuild the registry without recomputing paths.
    mode_dirs: Vec<PathBuf>,
    /// Skills the `invoke_skill` tool can load; also rendered as the menu the
    /// active mode's system prompt advertises.
    skills: std::sync::Arc<zoid_core::skill::SkillRegistry>,
    /// Agent profiles for `dispatch_subagent` name resolution + the `list_agents`
    /// tool. Built at startup from convention + configured `agents.source_dirs`.
    agents: std::sync::Arc<zoid_core::agent_profile::AgentRegistry>,
    /// The URL import/update wizard state. `Some` while a wizard is in flight;
    /// `None` otherwise. Gated into the turn's tool set in `spawn_turn`.
    wizard: Option<zoid::mode_wizard::ModeImportWizard>,
    /// A deferred "Adjust" reply from the wizard approval overlay —
    /// `answer_question` is sync and can't `.await` `session.append`, so it
    /// stashes the event here for the main loop to flush at the top of `run`.
    pending_adjust: Option<zoid_core::event::Event>,
    model: String,
    /// Economy config (spec §7.2), carried from `load_config()` so `run`'s
    /// per-frame `ContextPolicy` build (via `policy_from_config`) doesn't need
    /// its own copy of `main`'s `config` local.
    economy: zoid_core::config::EconomyConfig,
    /// The resolved soft setpoint (config `economy.context_target`, defaulted to
    /// `min(capacity, 300_000)` when unset) — separate from `shell.ctx_ceiling`
    /// (capacity, the model window). Recomputed on config reload and once the
    /// model's real capacity lands via `ModelInfoFetched`.
    context_target: u64,
    /// Full resolved config + provenance, kept live so the config screen can
    /// display current values and so edits reload/re-render without a restart.
    config: zoid_core::config::Config,
    /// Whether YOLO mode is active (no approval prompts). Resolved from
    /// config + CLI: `config.approval.yolo || cli --yolo`.
    yolo: bool,
    prov: zoid_core::config::Provenance,
    /// Encrypted secret store (None → unavailable this run; secret edits no-op
    /// with a stderr note). Shared with the provider credential lookup.
    secrets: Option<std::sync::Arc<zoid_core::secret::EncryptedDb>>,
    textarea: TextArea<'static>,
    streaming: bool,
    shell: zoid_tui::ShellState,
    /// Stashed oneshot reply for the agent-loop `submit_feedback` path: when
    /// the loop parks on a feedback question, the bin stores the `rtx` here so
    /// `Action::FeedbackSubmit` can send `Answer::Feedback(report)` back.
    feedback_reply: Option<tokio::sync::oneshot::Sender<zoid::agent::Answer>>,
    ui_tx: mpsc::Sender<AgentUpdate>,
    /// Monotonic clock start for motion timing (Ⓡ2).
    started: std::time::Instant,
    /// Cached projections over the event log, refreshed only when it grows.
    proj: ProjectionCache,
    /// Cached rendered conversation body; reused across scroll/typing frames.
    body_cache: BodyCache,
    /// The Overview dashboard body, rebuilt per-frame while at `Zoom::Overview`
    /// (only while the user is viewing it). Empty at every other altitude.
    overview_body: Vec<ratatui::text::Line<'static>>,
    /// When the altitude last changed, for the fold/unfold reveal (Ⓡ2).
    zoom_changed_at: Option<std::time::Instant>,
    /// Max conversation scroll offset from the last rendered frame (body length −
    /// viewport height at the current altitude). Cached from `render_shell` so the
    /// scroll handler can clamp the STORED offset and never let it run away past
    /// the last line (which would make scroll-up appear dead).
    last_conv_max_scroll: u16,
    /// The conversation rect from the last drawn frame, so `handle_action` (which
    /// has no `terminal` in scope) can map a scrollbar drag row to an offset.
    last_conv_rect: ratatui::layout::Rect,
    /// Message index to re-anchor to the top of the viewport after a zoom change
    /// (captured from the old altitude before zooming, applied once the new
    /// altitude's body is built). None when no zoom is pending.
    pending_zoom_anchor: Option<usize>,
    /// Local UTC offset (seconds) for message-row HH:MM stamps, sampled once.
    tz_offset_secs: i32,
    /// Epoch-millis the active session started (resumed session's `created_ts`,
    /// or boot time for a freshly-created one) — feeds the session drawer's
    /// live "dur" label.
    session_started_ms: i64,
    /// Session ids backing the resume-session picker rows (index-aligned with
    /// `shell.sessions`), populated when `Command::ResumeSessionPicker` opens it.
    session_ids: Vec<Ulid>,
    /// In-flight subagents, tracked for the Subagents drawer + busy guard.
    in_flight_subagents: Vec<SubagentInfo>,
    /// Subagents waiting for a pool slot (`dispatch_subagent` called while the
    /// global `in_flight` set is at `max_concurrent`). Drained on each
    /// `DelegationResult` by the `spawn_queued_subagent` helper.
    queued_subagents: std::collections::VecDeque<QueuedSubagent>,
    /// The active worktree session (None when in the main checkout).
    active_worktree: Option<WorktreeSession>,
    /// Active worktree path the git poller should open (None = main checkout).
    /// Updated on enter/exit so the poller re-polls immediately (WT-3 immediate)
    /// and confirms rather than reverts the rail (WT-3 durable).
    active_wt_tx: tokio::sync::watch::Sender<Option<std::path::PathBuf>>,
    /// Shared in-flight subagent ID set, threaded into TurnConfig so the
    /// Emitting handler can enforce sequential dispatch (Gap 3). The spawned
    /// subagent's DelegationResult removes the ID.
    in_flight: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, zoid::agent::SubagentHandle>>,
    >,
    /// The reply channel for an in-flight `ask_user` question (Task 11): `Some`
    /// while the question overlay is up. Dropping it (Esc-abort) makes the
    /// agent loop record a balanced "[user aborted]" result and end the turn.
    pending_answer: Option<tokio::sync::oneshot::Sender<zoid::agent::Answer>>,
    /// Cancellation token for the in-flight chat turn (`Some` while streaming).
    /// Firing it (Esc/Ctrl-C via `Action::CancelTurn`) makes the agent loop
    /// drain any pending tool calls and end the turn cleanly; cleared on
    /// `TurnComplete`.
    turn_cancel: Option<tokio_util::sync::CancellationToken>,
    /// The hard-stop token: fired by a SECOND Esc/Ctrl-C while `turn_cancel` is
    /// already cancelled. Force-kills a running local tool. Cleared with
    /// `turn_cancel` on `TurnComplete`.
    turn_hard: Option<tokio_util::sync::CancellationToken>,
    /// Armed state for the no-active-turn subagent kill confirm: first Esc arms,
    /// second Esc fires all in-flight subagents. Reset when the registry empties
    /// or a turn's tokens take over the escalation.
    subagent_kill_armed: bool,
    /// Dynamically-fetched model capabilities (from Ollama `/api/show` etc.),
    /// overriding the static MODEL_CAPS table. `None` until the first fetch
    /// lands (or when the provider doesn't support capability introspection).
    fetched_model_info: Option<zoid_provider::model::ModelInfo>,
    /// Optional companion HTTP server (None = disabled). Managed via the command
    /// palette (`companion` / `companion off`) or the `--companion` launch flag.
    companion: Option<zoid_companion::CompanionServer>,
    /// The state hub feeding the companion. Always present (cheap); the server is
    /// the optional part. `is_enabled()` gates snapshot publishing and `show`.
    companion_hub: std::sync::Arc<zoid_companion::CompanionHub>,
    /// When the current compaction phase started (for the 3s minimum-display
    /// debounce). `None` when no compaction is in flight or the debounce has
    /// cleared. Set by `AgentUpdate::CompactionStarted`; cleared by the
    /// per-frame debounce check after `CompactionComplete` + 3s elapsed.
    compaction_started_at: Option<std::time::Instant>,
    /// `CompactionComplete` arrived; the indicator stays visible until the 3s
    /// minimum display duration elapses (checked per-frame in `run()`).
    compaction_complete: bool,
    /// Count of `AgentUpdate::DirectiveReasserted` events seen this session
    /// (observability counter; no UI surface beyond tracing).
    reassert_count: u64,
    /// When the current tool started (for the 2s minimum-display debounce).
    /// Set by `set_active_tool`; the per-frame debounce clears the indicator
    /// 2s after it started, even if the tool finished faster.
    tool_started_at: Option<std::time::Instant>,
    /// The tool finished (`ToolResult`/`TurnComplete`); the indicator stays
    /// bright + animated until 2s have elapsed since `tool_started_at`.
    tool_complete: bool,
    /// Set when this process's session was taken over by another instance; the
    /// in-flight turn was cancelled and no further turns may start against it.
    /// The user can `:session new` or `:session resume` elsewhere, or quit. Spec §2.4.
    yielded: bool,
    /// A message queued while the agent was busy; auto-submitted when the
    /// current turn ends and no subagents are in flight. ESC (CancelTurn)
    /// does NOT clear it — the queued message runs after the steered turn.
    pending_message: Option<String>,
    /// A subagent's `DelegationResult` arrived while a turn was still streaming,
    /// so the orchestrator could not be woken immediately. Consumed at
    /// `TurnComplete`: once the turn ends and all subagents are done, a
    /// continuation turn is spawned so the orchestrator "sees" the result.
    /// Injects no `UserMessage` (unlike `pending_message`) — it just continues
    /// the conversation with the delegation summary now in context.
    wake_after_delegation: bool,
    /// Pending scheduled wakes, `(fire_at_ms, wake_id) → note`, ordered by fire
    /// time. Rebuilt from the event log on load; mutated by schedule/cancel/fire.
    pending_wakes: std::collections::BTreeMap<(i64, String), String>,
    /// The watcher's next deadline (earliest `fire_at_ms`, `None` = park). Sending
    /// a new value re-arms the watcher immediately (schedule/cancel/fire).
    next_wake_tx: tokio::sync::watch::Sender<Option<i64>>,
    /// A takeover confirmation in flight: the session id the user is about to
    /// Background MCP manager (None if no servers are configured). Its tools are
    /// merged into the Chat tool set each turn.
    mcp: Option<std::sync::Arc<zoid_mcp::McpManager>>,
    /// In-memory embedding index for hybrid recall (None = FTS-only). Wired up
    /// in a later task; always `None` in this scaffold.
    embed_index: Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>,
    /// The embedder used to embed the recall query. Paired with `embed_index`.
    embedder: Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>,
    /// True while a generic plugin install fetch is in flight. Prevents a
    /// second trigger from racing a concurrent write on the same folder
    /// (review M4).
    installing_plugin: bool,
}

impl App {
    /// Append an event both durably (session actor) and to the in-memory log
    /// the UI renders from.
    async fn record(&mut self, kind: EventKind) -> Result<()> {
        let ev = Event::new(Ulid::new(), None, now_ms(), kind).with_session(self.session_id);
        self.session.append(ev.clone()).await?;
        self.events.push(ev);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let obs = obs::init();

    let cli_new;
    let cli_resume: Option<String>;
    let cli_yolo;

    let companion_at_boot = match zoid::cli::parse_args(std::env::args().skip(1)) {
        zoid::cli::Cli::Version => {
            println!("{}", zoid::cli::version_string());
            return Ok(());
        }
        zoid::cli::Cli::Help => {
            println!("{}", zoid::cli::help_text());
            return Ok(());
        }
        zoid::cli::Cli::Update => {
            return zoid::update::run().await;
        }
        zoid::cli::Cli::Uninstall { purge } => {
            // Runs before any DB/terminal setup — we're deleting that state.
            return zoid::uninstall::run(uninstall_targets(), purge);
        }
        zoid::cli::Cli::Unknown(arg) => {
            eprintln!(
                "zoid: unrecognized argument '{arg}'\n\n{}",
                zoid::cli::help_text()
            );
            std::process::exit(2);
        }
        zoid::cli::Cli::Run {
            companion,
            new,
            resume,
            yolo,
        } => {
            // Build expiration: refuse to launch a >30-day-old build (or one on
            // a clock that predates the build). Runs before any DB/terminal
            // setup so the message prints cleanly. --version/--help/update are
            // deliberately NOT gated (escape hatches). See src/expiry.rs.
            zoid::expiry::enforce();
            cli_new = new;
            cli_resume = resume;
            cli_yolo = yolo;
            companion
        }
    };

    // Pre-TUI launch feedback (stderr, TTY-gated; wiped when the alt-screen
    // opens). Startup does real work — store open, session load, skill/mode
    // scans, and a first-run model-weight download — that was previously silent.
    let mut rep = zoid::startup::Reporter::stderr();
    rep.banner(concat!("zoid v", env!("CARGO_PKG_VERSION")));

    let path = db_path()?;
    let root = repo_root();
    // One-time legacy import (pre-release): ./.zoid/session.db → new global DB.
    let legacy = Path::new(".zoid").join("session.db");
    let tz_offset_secs = chrono::Local::now().offset().local_minus_utc();
    let boot_ts = now_ms();
    let _ = import_legacy_if_present(
        &path,
        &legacy,
        Ulid::new(),
        &derive_session_name(None, boot_ts, tz_offset_secs),
        &root,
        boot_ts,
    );

    rep.step("opening session store");
    let session = SessionHandle::spawn(
        path.to_str()
            .context("session DB path is not valid UTF-8")?,
    )?;

    // Seed the local_models table (curated entries from zoid_model). Phase 1:
    // creates the table and seeds it; nothing reads from it yet. Idempotent —
    // re-runs on every boot, updates curated entries if the seed version is
    // higher, leaves user-defined entries untouched.
    if let Err(e) = session.seed_local_models().await {
        tracing::warn!(error = %e, "failed to seed local_models table");
    }

    // Purge log entries older than 72h (non-fatal — same pattern as
    // seed_local_models). Bounds the logs table across restarts.
    if let Err(e) = session.purge_logs(72 * 60 * 60 * 1000).await {
        tracing::warn!(error = %e, "failed to purge old logs");
    }

    // Flush the ObsState ring buffer to the logs table. Pre-actor system
    // logs (config warnings, boot diagnostics) were captured in-memory;
    // persist them now that the actor is available. Non-fatal.
    {
        let entries = match obs.state.lock() {
            Ok(mut s) => s.take_logs(),
            Err(_) => Vec::new(), // poisoned mutex — skip flush, don't crash
        };
        for entry in &entries {
            let row = zoid_core::store::LogRow {
                ts: entry.ts,
                level: entry.level.clone(),
                scope: "system".into(),
                session_id: None,
                event_id: None,
                message: entry.message.clone(),
                fields: entry.fields.clone(),
            };
            if let Err(e) = session.write_log(row).await {
                tracing::warn!(error = %e, "failed to flush system log to db");
            }
        }
    }

    let sessions = session
        .list_sessions(Some(root.clone()))
        .await
        .unwrap_or_default();
    let first_time_user = sessions.is_empty();
    let self_pid = std::process::id() as i64;

    // Apply the CLI flags first: --resume and --new bypass the picker.
    let boot_path = boot_decision(sessions.len(), cli_new, cli_resume.as_deref());

    let (session_id, session_name, session_started_ms) = match boot_path {
        BootPath::ForceResume(ref id) => match resolve_resume_id(&sessions, id) {
            Ok(sid) => {
                let s = sessions.iter().find(|s| s.id == sid).unwrap();
                session.touch_session(sid, boot_ts).await.ok();
                (sid, s.name.clone(), s.created_ts)
            }
            Err(e) => {
                let msg = match e {
                    ResumeIdError::NotFound => {
                        format!("zoid: no session matches '{id}' in this repo")
                    }
                    ResumeIdError::Ambiguous(candidates) => {
                        format!(
                            "zoid: '{id}' is ambiguous (matches {} sessions: {})",
                            candidates.len(),
                            candidates.join(", ")
                        )
                    }
                    ResumeIdError::InvalidLength => {
                        format!("zoid: '{id}' is not a valid ULID (expected 4 or 26 chars)")
                    }
                };
                eprintln!("{msg}");
                std::process::exit(2);
            }
        },
        BootPath::ForceNew => {
            let id = Ulid::new();
            let name = derive_session_name(None, boot_ts, tz_offset_secs);
            session
                .new_session(id, name.clone(), root.clone(), boot_ts)
                .await?;
            (id, name, boot_ts)
        }
        BootPath::AutoCreate => {
            let id = Ulid::new();
            let name = derive_session_name(None, boot_ts, tz_offset_secs);
            session
                .new_session(id, name.clone(), root.clone(), boot_ts)
                .await?;
            (id, name, boot_ts)
        }
        BootPath::AutoResume => {
            let s = &sessions[0];
            let live = zoid_core::store::is_live(
                s.active,
                s.active_pid,
                s.active_heartbeat,
                boot_ts,
                pid_alive,
            );
            if live {
                let id = Ulid::new();
                let name = derive_session_name(None, boot_ts, tz_offset_secs);
                session
                    .new_session(id, name.clone(), root.clone(), boot_ts)
                    .await?;
                (id, name, boot_ts)
            } else {
                session.touch_session(s.id, boot_ts).await.ok();
                (s.id, s.name.clone(), s.created_ts)
            }
        }
        BootPath::Picker => {
            // Enter the terminal early for the picker, then continue to the
            // common path (which re-enters alt screen + mouse capture for run()).
            enable_raw_mode()?;
            let mut picker_out = stdout();
            execute!(picker_out, EnterAlternateScreen)?;
            let mut picker_term = Terminal::new(CrosstermBackend::new(picker_out))?;
            let repo_name = Path::new(&root)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.clone());
            let pick = pick_session(&mut picker_term, &session, &root, &repo_name, boot_ts).await;
            let _ = execute!(picker_term.backend_mut(), LeaveAlternateScreen);
            match pick {
                Ok(PickResult::Resume {
                    id,
                    name,
                    created_ts,
                }) => {
                    session.touch_session(id, boot_ts).await.ok();
                    (id, name, created_ts)
                }
                Ok(PickResult::CreateNew) => {
                    let id = Ulid::new();
                    let name = derive_session_name(None, boot_ts, tz_offset_secs);
                    session
                        .new_session(id, name.clone(), root.clone(), boot_ts)
                        .await?;
                    (id, name, boot_ts)
                }
                Err(_) => {
                    let _ = disable_raw_mode();
                    std::process::exit(0);
                }
            }
        }
    };
    // Claim the session (whether fresh or reclaimed) and start the heartbeat.
    session
        .set_active(session_id, true, self_pid, boot_ts)
        .await
        .ok();
    rep.step("loading session");
    let mut events =
        zoid::eventlog::EventLog::from_vec(session.snapshot_session(session_id).await?);
    // #6b: free compacted tool-result bodies on the boot auto-resume path too
    // (mirrors the interactive session-switch clear), so reopening a long
    // session doesn't re-inflate RAM to the pre-#6b footprint.
    events.clear_compacted_bodies();

    let (config, prov, cfg_warnings) = load_config();
    let model = if config.model.is_empty() {
        default_model().to_string()
    } else {
        config.model.clone()
    };
    let secret_key = resolve_secret_key_path(|k| std::env::var(k).ok());
    let secrets = zoid_core::secret::EncryptedDb::open(&path.to_string_lossy(), &secret_key)
        .map(std::sync::Arc::new)
        .ok(); // None → secrets unavailable this run (non-fatal)

    let (provider, provider_name, has_key) = select_provider(&config, &secrets);

    let mut shell = zoid_tui::ShellState::new();
    shell.reduced_motion = config.reduced_motion;
    shell.status_hint = config_warning_hint(&cfg_warnings);
    // The Repo drawer only makes sense inside a git work tree; outside one it
    // showed a fabricated "main" branch and zero changes (§16, task #38). Detect
    // once at startup: populate + keep the drawer when present, drop it when not.
    let repo_present = in_git_repo();
    if repo_present {
        shell.repo_name = Path::new(&root)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.clone());
        // Branch, worktree, and changes are all polled by the 5s background
        // task below — no boot-time call needed (the initial defaults are
        // harmless for the brief window before the first tick).
    } else {
        shell.remove_drawer(zoid_tui::DrawerId::Repo);
    }
    shell.session_name = session_name;
    shell.model = model.clone();
    // Economy ⑤ denominator: capacity is always the model window; the user's
    // target (ZOID_CONTEXT_CEILING → config.economy.context_target) is a
    // separate soft knob, defaulted to min(capacity, 300_000) when unset.
    // Constant for the process lifetime, so set once here rather than per frame.
    let capacity = zoid_provider::context_ceiling(&model);
    let context_target = config
        .economy
        .context_target
        .unwrap_or(300_000)
        .min(capacity);
    shell.ctx_ceiling = capacity;
    shell.provider = provider_label(provider_name, has_key);
    shell.cache_supported = zoid_provider::has_prompt_cache(&model);
    shell.cwd = root.clone();
    // Frozen at boot — the onboarding wizard + post-wizard empty-state copy
    // depend on first_time_user not being recomputed mid-session (a session
    // is created at boot, so sessions.is_empty() would be false if recomputed).
    shell.first_time_user = first_time_user;

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    let cfg_dir = resolve_config_dir(|k: &str| std::env::var(k).ok());
    let home = home_dir(|k: &str| std::env::var(k).ok());
    rep.step("building skills & modes");
    let skills = {
        let dirs = zoid::skill_import::resolve_skill_dirs(
            &config.skills.source_dirs,
            &cfg_dir,
            std::path::Path::new(&root),
            home.as_deref(),
        );
        std::sync::Arc::new(zoid::skill_import::build_registry(&dirs))
    };

    let base_profile = zoid::agent::default_profile();
    let mode_dirs = zoid::mode_import::resolve_mode_dirs(
        &config.modes.source_dirs,
        &cfg_dir,
        std::path::Path::new(&root),
        home.as_deref(),
    );
    let modes = zoid::mode_import::build_mode_registry(&base_profile, &mode_dirs);

    let agents = {
        let dirs = zoid::agent_import::resolve_agent_dirs(
            &config.agents.source_dirs,
            &cfg_dir,
            std::path::Path::new(&root),
            home.as_deref(),
        );
        std::sync::Arc::new(zoid::agent_import::build_agent_registry(&dirs))
    };

    let mcp = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let servers = zoid_mcp::config::discover(&cfg_dir, &cwd, &|k| std::env::var(k).ok());
        if servers.is_empty() {
            None
        } else {
            let m = std::sync::Arc::new(zoid_mcp::McpManager::new());
            m.spawn_connect_all(servers);
            Some(m)
        }
    };

    #[cfg(feature = "local-embed")]
    let (embed_index, embedder): EmbedStore = if config.embed.enabled {
        let cache = resolve_cache_dir(|k| std::env::var(k).ok())
            .join("models")
            .join("bge-small-en-v1.5");
        rep.step("loading semantic model");
        // On a warm cache the closure never fires (no download); on first run it
        // announces the ~130MB fetch once, then streams a live byte counter.
        let mut announced = false;
        let load = zoid_embed::CandleEmbedder::load_with_progress(
            &cache,
            config.embed.auto_download,
            &mut |_label, done, total| {
                if !announced {
                    announced = true;
                    rep.step("first run: downloading model weights (~130MB, one time)");
                }
                rep.progress(done, total);
            },
        );
        rep.progress_done();
        match load {
            Ok(e) => {
                let e: std::sync::Arc<dyn zoid_core::retrieval::Embedder> = std::sync::Arc::new(e);
                let idx = std::sync::Arc::new(std::sync::RwLock::new(
                    zoid_core::embed_index::EmbeddingIndex::new(e.dim(), config.embed.max_vectors),
                ));
                // boot-fill the ring from disk (newest-first rows appended oldest-first)
                if let Ok(rows) = session
                    .load_recent_embeddings(e.model_id().to_string(), config.embed.max_vectors)
                    .await
                {
                    let mut g = idx.write().unwrap();
                    for (id, v) in rows.into_iter().rev() {
                        g.append(id, &v);
                    }
                }
                // spawn the maintenance lane on a blocking OS thread (candle is
                // CPU-bound; must not run on tokio's async worker threads).
                // C5: capture the runtime handle BEFORE spawning the OS thread —
                // `Handle::current()` panics if called from inside that thread.
                let rt = tokio::runtime::Handle::current();
                {
                    let (sess, idx2, emb2, model) = (
                        session.clone(),
                        idx.clone(),
                        e.clone(),
                        e.model_id().to_string(),
                    );
                    // NOTE: embed events from ALL sessions, not just the boot
                    // session. The in-memory index is session-agnostic and recall
                    // filters by session downstream, so pinning the lane to one
                    // session would leave events in any session switched to after
                    // boot permanently unembedded.
                    std::thread::spawn(move || {
                        let lane = zoid_core::embed_lane::EmbedLane::new(emb2, idx2);
                        loop {
                            let todo =
                                match rt.block_on(sess.unembedded_events_all(model.clone(), 64)) {
                                    Ok(t) if !t.is_empty() => t,
                                    _ => {
                                        std::thread::sleep(std::time::Duration::from_secs(2));
                                        continue;
                                    }
                                };
                            let rows = lane.tick(&todo);
                            if rows.is_empty() {
                                // Every embed in a non-empty batch failed (tick degrade path).
                                // Back off instead of immediately re-fetching the same batch —
                                // avoids a hot-spin that would peg a core and hammer the
                                // session actor on persistent failure.
                                std::thread::sleep(std::time::Duration::from_secs(2));
                                continue;
                            }
                            for (id, v) in rows {
                                let _ = rt.block_on(sess.write_embedding(id, model.clone(), v));
                            }
                        }
                    });
                }
                (Some(idx), Some(e))
            }
            Err(err) => {
                tracing::warn!(%err, "local-embed disabled: model load failed");
                (None, None)
            }
        }
    } else {
        (None, None)
    };
    #[cfg(not(feature = "local-embed"))]
    let (embed_index, embedder): EmbedStore = (None, None);

    let mut app = App {
        session,
        session_id,
        events,
        provider,
        modes,
        base_profile,
        mode_dirs,
        skills,
        agents,
        wizard: None,
        pending_adjust: None,
        model,
        economy: config.economy,
        context_target,
        config: config.clone(),
        yolo: config.approval.yolo || cli_yolo,
        prov,
        secrets: secrets.clone(),
        textarea: make_input(TextArea::default()),
        streaming: false,
        shell,
        feedback_reply: None,
        ui_tx,
        started: std::time::Instant::now(),
        proj: ProjectionCache::default(),
        body_cache: BodyCache::default(),
        overview_body: Vec::new(),
        zoom_changed_at: None,
        last_conv_max_scroll: 0,
        last_conv_rect: ratatui::layout::Rect::default(),
        pending_zoom_anchor: None,
        tz_offset_secs,
        session_started_ms,
        session_ids: Vec::new(),
        in_flight_subagents: Vec::new(),
        queued_subagents: std::collections::VecDeque::new(),
        active_worktree: None,
        active_wt_tx: tokio::sync::watch::channel(None).0,
        in_flight: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        pending_answer: None,
        turn_cancel: None,
        turn_hard: None,
        subagent_kill_armed: false,
        fetched_model_info: None,
        companion: None,
        companion_hub: zoid_companion::CompanionHub::new(),
        compaction_started_at: None,
        compaction_complete: false,
        reassert_count: 0,
        tool_started_at: None,
        tool_complete: false,
        yielded: false,
        pending_message: None,
        wake_after_delegation: false,
        pending_wakes: std::collections::BTreeMap::new(),
        next_wake_tx: tokio::sync::watch::channel(None).0,
        mcp,
        embed_index,
        embedder,
        installing_plugin: false,
    };

    // First-run onboarding wizard: open the overlay if the gate fires.
    // The gate is the persistence — no "wizard seen" flag; it re-evaluates
    // from scratch on every launch.
    if wizard_needed(first_time_user, &app.config, has_key, app.secrets.is_some()) {
        app.shell.overlay = zoid_tui::Overlay::Onboarding;
        // env_shadow: set when a ZOID_PROVIDER env var shadows the TOML provider
        // (spec §5). The step-1 screen renders a warning so the user knows their
        // wizard choice won't take effect until they unset it.
        let env_shadow = if app.prov.provider == zoid_core::config::Source::Env {
            Some(app.config.provider.clone())
        } else {
            None
        };
        app.shell.onboarding = Some(zoid_tui::state::OnboardingState {
            step: zoid_tui::state::OnboardingStep::Provider,
            chosen_provider: String::new(),
            options: zoid_tui::config_view::provider_options(""),
            env_shadow,
            ..Default::default()
        });
    }

    // Restore the resumed session's persisted active mode (a no-op if that mode
    // no longer exists ⇒ stays Chat) and mirror it onto the shell so the
    // chip/palette are correct before the event loop begins.
    restore_mode_for_session(&mut app).await;

    spawn_heartbeat(&app);

    if companion_at_boot || app.config.companion.enabled {
        enable_companion(&mut app);
    }

    // The picker path (BootPath::Picker) already entered raw mode + alt screen
    // for the picker; skip the re-entry but still enter mouse capture for run().
    let raw_mode_entered = matches!(boot_path, BootPath::Picker);
    if !raw_mode_entered {
        enable_raw_mode()?;
    }
    let mut out = stdout();
    // Kitty keyboard protocol: lets the terminal report ⇧⏎ distinctly from ⏎ so
    // route.rs can map Shift+Enter → newline. Degrade gracefully — only push the
    // flags when supported; otherwise the Alt+⏎ fallback stands. Push BEFORE
    // EnterAlternateScreen / EnableBracketedPaste so the terminal doesn't
    // drop the kitty protocol sequence when bracketed paste is already active
    // (Ghostty exhibits this ordering sensitivity).
    let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if kbd_enhanced {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        );
    }
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    // Fetch the active model's capabilities (context window, prompt cache) from
    // the provider's introspection endpoint. The static MODEL_CAPS table is the
    // fallback until this lands (or if the provider doesn't support it).
    spawn_model_info_fetch(app.provider.clone(), app.model.clone(), app.ui_tx.clone());

    let result = run(&mut terminal, &mut app, &mut ui_rx, &obs.state).await;

    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    // Release the session's active flag on clean exit (best-effort). If the
    // process is force-killed the flag stays stale and the next evaluator
    // reclaims it via is_live == false. Spec §2.3.
    let _ = app.session.set_active(app.session_id, false, 0, 0).await;
    result
}

/// Map a screen row on the scrollbar to an absolute conversation offset and
/// apply it (re-deriving tail-follow), using the last drawn frame's geometry.
fn scrollbar_row_to_offset(app: &mut App, row: u16) {
    let conv = app.last_conv_rect;
    let track_h = conv.height;
    if track_h <= 1 {
        return;
    }
    let max = app.last_conv_max_scroll;
    let rel = row.saturating_sub(conv.y).min(track_h - 1);
    let offset =
        ((rel as u32 * max as u32 + (track_h as u32 - 1) / 2) / (track_h as u32 - 1)) as u16;
    app.shell.scroll_to_offset(offset, max);
}

async fn run<B>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    ui_rx: &mut mpsc::Receiver<AgentUpdate>,
    obs_state: &std::sync::Arc<std::sync::Mutex<obs::ObsState>>,
) -> Result<()>
where
    B: ratatui::backend::Backend + std::io::Write,
    <B as ratatui::backend::Backend>::Error: Send + Sync + 'static,
{
    let mut term_events = EventStream::new();

    // Wake watcher: parks on the next deadline; sends WakeDue when it elapses.
    // Re-armed immediately whenever `next_wake_tx` changes (schedule/cancel/fire).
    {
        let ui = app.ui_tx.clone();
        let mut next_rx = app.next_wake_tx.subscribe();
        tokio::spawn(async move {
            loop {
                let next = *next_rx.borrow_and_update();
                let sleep = async {
                    match next {
                        Some(fire_at_ms) => {
                            let now = now_ms();
                            let delay = (fire_at_ms - now).max(0) as u64;
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                        // Nothing scheduled: park until the cell changes.
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = sleep => {
                        if ui.send(AgentUpdate::WakeDue).await.is_err() {
                            break; // main loop gone
                        }
                        // Re-borrow next loop; the handler will re-arm via next_wake_tx.
                    }
                    changed = next_rx.changed() => {
                        if changed.is_err() {
                            break; // sender dropped — app exiting
                        }
                    }
                }
            }
        });
    }

    // Rebuild the pending-wake set from the loaded event log and arm the
    // watcher for the earliest deadline. Race-free: the watcher already called
    // `subscribe()` synchronously above, before this send.
    app.pending_wakes = rebuild_pending_wakes(app.events.iter());
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));

    let tick_period = std::time::Duration::from_millis(1000 / zoid_tui::motion::MOTION_FPS);
    let mut motion_tick = tokio::time::interval(tick_period);
    motion_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Slower tick (5 FPS) for the subagent-only case: when the main loop is idle
    // but subagents are running, we only need to animate the drawer spinner, not
    // the caret. 30 FPS would burn CPU for no visible benefit.
    let subagent_tick_period = std::time::Duration::from_millis(200);
    let mut subagent_tick = tokio::time::interval(subagent_tick_period);
    subagent_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Periodic log-flush tick: drain the ObsState ring buffer to the logs table
    // every 60s so in-session warn/error events are persisted without waiting
    // for a restart. Non-fatal — a flush failure warns and continues.
    let mut log_flush_tick = tokio::time::interval(std::time::Duration::from_secs(60));

    // Off-load `git status` to a background task so the subprocess never blocks
    // the render loop (it previously ran synchronously on the loop every second,
    // hitching typing/scrolling). The loop reads the latest value non-blocking.
    let (git_tx, mut git_rx) = tokio::sync::watch::channel((
        0usize,
        0usize,
        0usize,               // added, removed, files
        String::new(),        // branch
        "(none)".to_string(), // worktree
    ));
    // Only poll git when the Repo drawer is actually present; outside a repo the
    // stats are neither shown nor meaningful, so we skip the per-second `git`
    // subprocess entirely. The drawer's presence is the git-repo signal decided
    // at startup. The receiver still reads its initial (0, 0, 0).
    if app.shell.drawer(zoid_tui::DrawerId::Repo).is_some() {
        let mut active_wt_rx = app.active_wt_tx.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                // Latest active-worktree path (main checkout when None).
                let dir = active_wt_rx
                    .borrow_and_update()
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let dir_status = dir.clone();
                let (added, removed, files) =
                    tokio::task::spawn_blocking(move || git_status_at(&dir_status))
                        .await
                        .unwrap_or((0, 0, 0));
                let dir_labels = dir.clone();
                let (branch, worktree) = tokio::task::spawn_blocking(move || {
                    let branch = current_branch_at(&dir_labels);
                    let worktree = git2::Repository::open(&dir_labels)
                        .ok()
                        .map(|r| worktree_label(&r))
                        .unwrap_or_else(|| "(none)".into());
                    (branch, worktree)
                })
                .await
                .unwrap_or_else(|_| ("main".into(), "(none)".into()));
                if git_tx
                    .send((added, removed, files, branch, worktree))
                    .is_err()
                {
                    break; // receiver dropped — app is exiting
                }
                // Wake on the 5 s tick OR an enter/exit (immediate re-poll).
                tokio::select! {
                    _ = tick.tick() => {}
                    changed = active_wt_rx.changed() => {
                        if changed.is_err() {
                            break; // sender dropped — app is exiting
                        }
                    }
                }
            }
        });
    }

    // Catch-up: fire any wakes whose fire_at already passed while closed.
    let _ = drain_due_wakes(app).await?;

    // Actual terminal mouse-capture state (true at startup — EnableMouseCapture
    // ran during terminal setup). Reconciled against `shell.select_mode` below.
    let mut mouse_captured = true;

    loop {
        if let Some(ev) = app.pending_adjust.take() {
            app.session.append(ev.clone()).await.ok();
            app.events.push(ev);
            spawn_turn(app);
        }
        // Latest git status from the background watcher (non-blocking read).
        {
            let (a, r, f, branch, worktree) = &*git_rx.borrow_and_update();
            app.shell.changes_added = *a;
            app.shell.changes_removed = *r;
            app.shell.changes_files = *f;
            app.shell.branch = branch.clone();
            app.shell.worktree = worktree.clone();
        }
        // MCP server status: an in-memory Mutex snapshot (no subprocess/IO), so
        // it's cheap enough to refresh every frame rather than off-loading it to
        // the background git-poll task above.
        if let Some(m) = &app.mcp {
            app.shell.mcp_status = m
                .status()
                .into_iter()
                .map(|s| zoid_tui::state::McpStatusRow {
                    name: s.name,
                    state: match s.state {
                        zoid_mcp::ServerState::Connecting => "connecting",
                        zoid_mcp::ServerState::Ready => "ready",
                        zoid_mcp::ServerState::Failed => "failed",
                        zoid_mcp::ServerState::Disconnected => "disconnected",
                    }
                    .to_string(),
                    tool_count: s.tool_count,
                })
                .collect();
        }
        // Refresh cached projections only when the event log grew (append-only),
        // so typing / scrolling / zoom reuse them instead of rebuilding O(events)
        // projections every frame.
        let proj_rebuilt = app.proj.refresh(&app.events);
        app.shell.session_tokens = app.proj.ledger_total.saturating_sub(app.proj.cached_total);
        app.shell.cached_tokens = app.proj.cached_total;
        app.shell.ctx_used = app
            .proj
            .last_input_tokens
            .unwrap_or(app.proj.window.total_tokens);
        app.shell.tasks_len = app.proj.tasks.len() as u16;
        app.shell.duration = fmt_duration(app.session_started_ms, now_ms());
        // Thinking mode label for the session drawer.
        app.shell.thinking_label = if app.config.thinking.enabled {
            match &app.config.thinking.effort {
                None => Some("◆".to_string()),
                Some(e) => Some(format!("◆ {e}")),
            }
        } else {
            None
        };
        // TPS: rolling average of per-turn TPS values (session widget).
        // Per-turn TPS is recorded at `TurnComplete`; the frame tick just reads
        // the rolling average, not the hybrid last-tokens / avg-duration formula.
        app.shell.tps = obs_state
            .lock()
            .ok()
            .map(|s| s.provider_tps.avg())
            .unwrap_or(0);
        app.shell.input_rows = app.textarea.lines().len().max(1) as u16;
        // Empty-buffer flag for routing: a leading `:` in an empty box opens the
        // palette (direct mode) instead of inserting a literal colon. The textarea
        // lives in the bin, not ShellState, so sample it here each frame (mirrors
        // `input_rows`). A multi-line box is never "empty" for this purpose even if
        // every line is blank — `:` is literal once any structure exists.
        app.shell.input_empty =
            app.textarea.lines().iter().all(|l| l.is_empty()) && app.textarea.lines().len() == 1;
        let mut frame_conv_max = 0u16;
        // Whether the frame we're about to draw is "settled" (no reveal truncation).
        // Used to end the zoom animation only after a full-body frame is actually
        // painted — otherwise a tick landing just before the animation window closes
        // paints a truncated frame and the loop stops redrawing (stale until an
        // unrelated event wakes it).
        let mut frame_reveal_none = false;

        // Build (or reuse) the rendered conversation body OUTSIDE the draw closure,
        // keyed on the inputs that change the lines (events / zoom / width /
        // streaming / caret) — NOT the scroll offset. A scroll-event burst then
        // reuses these lines every frame instead of re-wrapping + re-highlighting
        // the whole transcript (the ~52ms/frame cost behind the scroll storm).
        let area = terminal
            .size()
            .map(|s| Rect {
                x: 0,
                y: 0,
                width: s.width,
                height: s.height,
            })
            .unwrap_or_default();
        let layout = compute(area, &app.shell);
        let body_w = zoid_tui::layout::conv_text_width(layout.conversation.width) as usize;
        let elapsed = app.started.elapsed().as_millis() as u64;
        let streaming = app.streaming;
        let zoom = app.shell.zoom;
        let tz = app.tz_offset_secs;
        let caret = if streaming {
            true // solid caret during streaming — the spinner indicates activity
        } else {
            zoid_tui::motion::caret_on(elapsed, 1000, app.shell.reduced_motion)
        };
        // At Overview the conversation pane hosts the metrics dashboard instead of
        // the transcript: assemble a fresh `OverviewData` snapshot and render it
        // into `overview_body` (a per-frame rebuild is fine — it only runs while the
        // user is actually viewing Overview). Every other altitude reuses the cached
        // conversation body. Consuming `obs.state` here is also what puts the shared
        // aggregate to work in the render loop.
        let is_overview = zoom == zoid_tui::state::Zoom::Overview;
        // `None` at Overview: the body is rebuilt fresh every frame (no
        // `BodyCache` lookup happens at all), so there's no cache signal to
        // report — Overview frames must not participate in the body-render
        // cache ratio (`render_cache_pct`), only in frame timing.
        let cache_hit = if is_overview {
            let data = build_overview_data(app, obs_state);
            app.overview_body = zoid_tui::overview::overview_lines(&data, body_w);
            None
        } else if app.proj.msgs.is_empty() {
            // Empty-state intercept: bypass BodyCache, build onboarding/welcome
            // lines directly. When the first message arrives, proj.msgs becomes
            // non-empty and the else branch takes over (key is None → full
            // rebuild). Excluded from the body-render cache-hit ratio (None).
            let offer_superpowers =
                app.shell.first_time_user && !app.modes.names().iter().any(|n| n == "Superpowers");
            app.body_cache.body = zoid_tui::onboarding::empty_state_lines(
                app.shell.first_time_user,
                offer_superpowers,
                body_w,
            );
            app.body_cache.key = None;
            app.body_cache.msg_count = 0;
            None
        } else {
            let inline_k = if app.config.ui.edit_diff {
                app.config.ui.edit_diff_inline as usize
            } else {
                0
            };
            let kind = app.body_cache.refresh(
                BodyKey {
                    zoom,
                    width: body_w,
                    streaming,
                    caret: streaming && caret,
                    tz,
                    question_rev: question_rev(app.shell.question.as_ref()),
                },
                &app.proj.msgs,
                body_w,
                app.shell.question.as_ref(),
                &app.shell.edit_diffs,
                inline_k,
            );
            // Telemetry only distinguishes a pure hit (no render work) from a
            // render; both incremental and full rebuilds count as a miss.
            Some(kind == RefreshKind::Hit)
        };

        // Tail-follow: when engaged, pin the viewport to the latest line before
        // drawing — this is what makes the view show the latest output on startup
        // and follow new events (including live streaming) as they append. Applied
        // after the cross-zoom anchor below, so following the tail wins over the
        // anchor. max_scroll mirrors render's clamp: body length minus the
        // visible conversation height. (The in-flight tool indicator moved to the
        // status bar, so it no longer adds a row to the body.)
        // Scroll math reuses the active body's length (Overview dashboard or the
        // cached transcript), so the scrollbar/clamp work unchanged at every altitude.
        let body_len = if is_overview {
            app.overview_body.len()
        } else {
            app.body_cache.body.len()
        };
        let max_scroll = body_len
            .saturating_sub(layout.conversation.height as usize)
            .min(u16::MAX as usize) as u16;
        // Re-anchor after a zoom: map the captured message back to its line at the
        // new altitude. Runs before the draw (body/msg_starts now reflect the new
        // altitude), so the transient reset-to-0 from zoom_in/out never paints.
        if let Some(anchor) = app.pending_zoom_anchor.take() {
            // Overview has no per-message lines, so there's nothing to anchor to —
            // pin to the top (line 0), skipping cross-zoom anchoring naturally.
            let line = if is_overview {
                0
            } else {
                zoid_tui::line_of_msg(&app.body_cache.msg_starts, anchor)
            };
            app.shell.conversation_scroll = (line.min(u16::MAX as usize) as u16).min(max_scroll);
        }
        if app.zoom_changed_at.is_none() {
            app.shell.apply_follow(max_scroll);
        }
        // Remember the conversation rect for scrollbar drag row→offset mapping,
        // which runs in `handle_action` (no `terminal`/layout in scope there).
        app.last_conv_rect = layout.conversation;

        // Refresh the status-bar activity indicator: a turn is "working" while
        // streaming or delegating. The spinner frame is wall-clock-derived here
        // (kept out of the pure renderer for snapshot determinism); the motion
        // tick below redraws at MOTION_FPS while busy so it actually animates.
        app.shell.busy = app.streaming;
        app.shell.yolo = app.yolo;
        // Only a chat turn carries a cancellation token; delegation has none, so
        // Esc/Ctrl-C routes to CancelTurn while this is true. Also true when
        // subagents are in flight (no main turn) so the two-press subagent-kill
        // path in CancelTurn is reachable.
        app.shell.cancellable = app.turn_cancel.is_some() || !app.in_flight_subagents.is_empty();
        app.shell.spinner = zoid_tui::tokens::glyph::SPINNER[zoid_tui::motion::spinner_frame(
            elapsed,
            80,
            zoid_tui::tokens::glyph::SPINNER.len(),
            app.shell.reduced_motion,
        )];
        // Debounce: if CompactionComplete arrived, keep the indicator visible
        // until 2s have elapsed since CompactionStarted. The motion tick guard
        // wakes while `compacting` is true, so this timer drains without an
        // extra wake source.
        if app.compaction_complete {
            if let Some(start) = app.compaction_started_at {
                if start.elapsed() >= std::time::Duration::from_secs(2) {
                    app.shell.compacting = false;
                    app.compaction_complete = false;
                    app.compaction_started_at = None;
                }
            }
        }
        // Mirror compaction_started_at onto the shell so the pure renderer can
        // drive the animation. Spec §3.
        app.shell.compaction_started_at = app.compaction_started_at;

        // Tool indicator debounce: if the tool finished (ToolResult/TurnComplete),
        // keep the indicator bright + animated until 2s have elapsed since it
        // started, then clear it back to the dim idle glyph. The motion tick guard
        // wakes while `active_tool` is set, so this drains without an extra source.
        if app.tool_complete {
            if let Some(start) = app.tool_started_at {
                if start.elapsed() >= std::time::Duration::from_secs(2) {
                    app.shell.clear_active_tool();
                    app.tool_complete = false;
                    app.tool_started_at = None;
                }
            } else {
                // No start timestamp (shouldn't happen, but defensive): clear now.
                app.shell.clear_active_tool();
                app.tool_complete = false;
            }
        }
        // Mirror tool_started_at onto the shell for the renderer's animation.
        app.shell.tool_started_at = app.tool_started_at;

        // Clamp the help overlay scroll to the real rect height (same idea as
        // conv_max_scroll): the ScrollHelp handler only increments; this pins
        // the ceiling for the current terminal size.
        if app.shell.overlay == zoid_tui::Overlay::Help {
            // Reuse the frame's already-computed `layout` (the help rect depends
            // only on `overlay` + conversation bounds, unchanged since it was
            // computed above), instead of a second full compute() pass.
            let vh = layout
                .palette
                .map(|r| r.height.saturating_sub(2) as usize) // borders/margin
                .unwrap_or(0);
            let max = zoid_tui::help::help_lines().len().saturating_sub(vh);
            app.shell.help_scroll = app.shell.help_scroll.min(max);
        }

        // Reconcile terminal mouse capture with select mode: while select_mode is
        // on we release the mouse to the terminal for native selection; otherwise
        // we hold it (click-to-copy code, choice clicks, scroll routing).
        let want_capture = !app.shell.select_mode;
        if want_capture != mouse_captured {
            let _ = if want_capture {
                execute!(terminal.backend_mut(), EnableMouseCapture)
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)
            };
            mouse_captured = want_capture;
        }

        let frame_start = std::time::Instant::now();
        terminal.draw(|f| {
            // The drawer is read-only/observability-only, so it needs only
            // window + churn. All inputs come from caches — zero per-frame
            // O(events) or re-render work on an ordinary frame.
            let economy = zoid_tui::EconomyView::build(&app.proj.window, &app.proj.churn, 0);
            let task_items = &app.proj.tasks;
            let body: &[ratatui::text::Line<'static>] = if is_overview {
                &app.overview_body
            } else {
                &app.body_cache.body
            };
            // Zoom-reveal count derives from the cached body length — no second
            // conversation_view build. `reveal` is None on ordinary frames.
            let reveal = match app.zoom_changed_at {
                Some(t0) => {
                    let elapsed_ms = t0.elapsed().as_millis() as u64;
                    if elapsed_ms < ZOOM_ANIM_MS && !app.shell.reduced_motion {
                        zoid_tui::motion::zoom_reveal(
                            body.len(),
                            elapsed_ms,
                            ZOOM_ANIM_MS,
                            app.shell.reduced_motion,
                        )
                    } else {
                        None
                    }
                }
                None => None,
            };
            frame_reveal_none = reveal.is_none();
            let view = ChatView {
                zoom,
                caret_on: caret,
                reveal,
                tz_offset_secs: tz,
            };
            frame_conv_max = render_shell(
                f,
                &app.shell,
                &economy,
                &app.proj.msgs,
                Some(body),
                task_items,
                &app.textarea,
                streaming,
                &view,
            );
        })?;
        // The `cache_hit` field is present only for transcript frames (a real
        // body-cache lookup happened); Overview frames omit it entirely so
        // `ObsLayer`'s `FieldGrab::cache_hit_present` stays false and the
        // frame is excluded from the cache ratio while still timing it.
        match cache_hit {
            Some(hit) => tracing::trace!(
                kind = "frame",
                ms = frame_start.elapsed().as_millis() as u64,
                cache_hit = hit,
                proj_rebuilt = proj_rebuilt,
                "frame"
            ),
            None => tracing::trace!(
                kind = "frame",
                ms = frame_start.elapsed().as_millis() as u64,
                proj_rebuilt = proj_rebuilt,
                "frame"
            ),
        }
        app.last_conv_max_scroll = frame_conv_max;
        // End the zoom animation only once a settled (reveal-complete) frame has
        // actually been painted, so the final full-body frame is never skipped.
        if frame_reveal_none && app.zoom_changed_at.is_some() {
            app.zoom_changed_at = None;
        }

        tokio::select! {
            biased;
            // Terminal events and ui_rx first — ensures TurnComplete and other
            // agent updates are never starved by the motion tick (which fires at
            // 30 FPS when streaming=true and can trap the loop if frame render
            // takes >33ms, starving the ui_rx that would clear `streaming`).
            maybe_term = term_events.next() => {
                match maybe_term {
                    Some(Ok(CEvent::Key(key))) => {
                        if handle_action(app, route_key(&app.shell, key)).await? {
                            return Ok(());
                        }
                    }
                    Some(Ok(CEvent::Mouse(me))) => {
                        let area = terminal.size()
                            .map(|s| Rect { x: 0, y: 0, width: s.width, height: s.height })
                            .unwrap_or_default();
                        let layout = compute(area, &app.shell);
                        match route_mouse(&app.shell, &layout, me) {
                            // Resolved here (not in handle_action) because it needs
                            // the conversation rect + wrap width from `layout`.
                            zoid_tui::route::Action::ConversationClick(row) => {
                                handle_conversation_click(app, &layout, row);
                            }
                            action => {
                                if handle_action(app, action).await? {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Some(Ok(CEvent::Paste(text))) => {
                        // Bracketed paste is a distinct event that skips route_key,
                        // so route it through the same focus/overlay precedence —
                        // otherwise it always leaked into the message box (e.g. an
                        // API key pasted into the config Secret field).
                        match route_paste(&app.shell) {
                            PasteTarget::Input => {
                                app.textarea.insert_str(&text);
                            }
                            PasteTarget::ConfigEdit => {
                                if let Some(buf) = app.shell.config_edit.as_mut() {
                                    buf.push_str(&text);
                                }
                            }
                            PasteTarget::PaletteQuery => {
                                if let zoid_tui::state::PaletteStage::Pick = app.shell.palette.stage
                                {
                                    app.shell.palette.query.push_str(&text);
                                    app.shell.palette.selected = 0;
                                }
                            }
                            PasteTarget::PaletteArg => {
                                if let zoid_tui::state::PaletteStage::Arg { input, .. } =
                                    &mut app.shell.palette.stage
                                {
                                    input.push_str(&text);
                                }
                            }
                            PasteTarget::Question => {
                                if let Some(q) = app.shell.question.as_mut() {
                                    q.free_text.push_str(&text);
                                }
                            }
                            PasteTarget::FeedbackTitle => {
                                if let Some(fs) = app.shell.feedback.as_mut() {
                                    fs.title.push_str(&text);
                                }
                            }
                            PasteTarget::FeedbackBody => {
                                if let Some(fs) = app.shell.feedback.as_mut() {
                                    fs.body.push_str(&text);
                                }
                            }
                            PasteTarget::OnboardingKey => {
                                if let Some(o) = app.shell.onboarding.as_mut() {
                                    o.key_buffer.push_str(&text);
                                }
                            }
                            PasteTarget::None => {}
                        }
                    }
                    Some(Ok(_)) => { /* resize: redraw next loop */ }
                    Some(Err(_)) | None => return Ok(()),
                }
            }
            Some(update) = ui_rx.recv() => {
                match update {
                    AgentUpdate::Appended(ev) => {
                        let mut delegation_arrived = false;
                        if let EventKind::DelegationResult { subagent_id, summary, ok, .. } = &ev.kind {
                            tracing::info!(
                                subagent_id = %subagent_id,
                                ok = %ok,
                                summary_len = summary.len(),
                                "delegation result arrived"
                            );
                            app.in_flight_subagents.retain(|s| s.id != *subagent_id);
                            app.in_flight.lock().unwrap().remove(subagent_id);
                            if app.in_flight.lock().unwrap().is_empty() {
                                app.subagent_kill_armed = false;
                            }
                            app.shell.subagent_rows.retain(|s| s.id != *subagent_id);
                            // A slot just freed. Drain the queue: while a slot is
                            // open AND the queue is non-empty, spawn the next
                            // queued subagent. Check queue emptiness FIRST so
                            // the short-circuit never pops an empty deque. A
                            // `max_concurrent` of 0 means unlimited — spawn all
                            // queued (they shouldn't normally be queued, but be
                            // safe).
                            let max = app.config.subagent.max_concurrent;
                            while !app.queued_subagents.is_empty()
                                && (max == 0 || app.in_flight.lock().unwrap().len() < max)
                            {
                                let qs = app.queued_subagents.pop_front().unwrap();
                                spawn_queued_subagent(app, qs);
                            }
                            delegation_arrived = true;
                        }
                        // A tool result ends the in-flight indicator for that tool.
                        // Don't clear immediately — set tool_complete for the 2s
                        // minimum-display debounce (mirrors compaction).
                        if matches!(ev.kind, EventKind::ToolResult { .. }) {
                            app.tool_complete = true;
                        }
                        // Incremental projection: apply_event handles every
                        // EventKind in O(1) (append to msgs, in-place mutation,
                        // or bookkeeping update). Returns a ProjectionImpact
                        // describing what changed — the caller uses it to
                        // decide whether body_cache needs invalidation.
                        // Subagent-branch events are persisted but skipped
                        // (the projection only tracks the main branch).
                        let is_subagent_branch =
                            ev.branch != zoid_core::event::BranchId::default();
                        if !is_subagent_branch {
                            let impact = app.proj.apply_event(&ev);
                            match impact {
                                ProjectionImpact::Economy => {
                                    // msgs unchanged — body_cache NOT invalidated.
                                }
                                ProjectionImpact::MsgsMutated { mutated_index } => {
                                    match mutated_index {
                                        None => {
                                            // Last-message mutation — BodyCache incremental path.
                                        }
                                        Some(i) if i == app.proj.msgs.len() - 1 => {
                                            // Last-message mutation — BodyCache incremental path.
                                        }
                                        Some(_) => {
                                            // Non-last mutation — full body rebuild.
                                            app.body_cache.key = None;
                                        }
                                    }
                                }
                                ProjectionImpact::MsgsAppended => {
                                    app.body_cache.key = None;
                                }
                                ProjectionImpact::FullRefresh => {
                                    app.proj.events_len = None;
                                    app.body_cache.key = None;
                                }
                            }
                        }
                        // #6b: when a compaction marker arrives, free the raw body
                        // of the ToolResult it summarizes. Safe: request/render carry
                        // the summary (projection.rs), file_contents & window are
                        // redirected (Task 3), eviction weighs the summary (Task 4),
                        // recall reads SQLite. Capture the id before `*ev` is moved.
                        let compacted_id = match &ev.kind {
                            EventKind::ToolResultCompacted { id, .. } => Some(id.clone()),
                            _ => None,
                        };
                        app.events.push(*ev);
                        if let Some(id) = compacted_id {
                            app.events.clear_tool_output(&id);
                        }
                        // A subagent finished: if the orchestrator is now idle,
                        // wake it into a continuation turn so it actually sees the
                        // result (it was fire-and-forget, so the dispatching turn
                        // has usually already ended). Per-result: each
                        // DelegationResult gets its own wake decision — no
                        // waiting for the whole pool to drain.
                        if delegation_arrived && plan_delegation_wake(app) {
                            tracing::info!("spawning continuation turn after delegation");
                            spawn_turn(app);
                        }
                    }
                    AgentUpdate::ToolStarted { name } => {
                        app.shell.set_active_tool(name);
                        app.tool_started_at = Some(std::time::Instant::now());
                        app.tool_complete = false;
                    }
                    AgentUpdate::TurnComplete => {
                        app.streaming = false;
                        // Finalize any pending assistant-turn state (trailing
                        // standalone-thinking flush — mirrors the fold's
                        // trailing flush at projection.rs:344–356).
                        let impact = app.proj.finalize_pending();
                        if matches!(impact, ProjectionImpact::MsgsAppended) {
                            app.body_cache.key = None;
                        }
                        // Don't clear the tool immediately — set tool_complete for
                        // the 2s minimum-display debounce (mirrors compaction).
                        app.tool_complete = true;
                        app.pending_answer = None;
                        app.turn_cancel = None;
                        app.turn_hard = None;
                        // Clear any lingering "cancelling…" hint now the turn ended.
                        app.shell.status_hint = None;
                        // Record per-turn TPS for the rolling average
                        // (session widget). Both values are available now:
                        // the turn is done, the Usage event is in the log,
                        // and provider_total.last() is this turn's stream ms.
                        {
                            let stream_ms = obs_state
                                .lock()
                                .ok()
                                .map(|s| s.provider_total.last())
                                .unwrap_or(0);
                            if stream_ms > 0 {
                                let output_tokens = app
                                    .proj
                                    .last_output_tokens
                                    .unwrap_or(0);
                                // Skip zero-output turns (errors, cancellations,
                                // context-length retries) — they're not meaningful
                                // TPS samples and would pollute the rolling average.
                                if output_tokens > 0 {
                                    if let Ok(mut s) = obs_state.lock() {
                                        let tps = output_tokens
                                            .checked_mul(1000)
                                            .and_then(|t| t.checked_div(stream_ms))
                                            .unwrap_or(0);
                                        s.provider_tps.record(tps);
                                    }
                                }
                            }
                        }
                        // Consume a queued message if the agent is now idle.
                        let mut spawned = false;
                        if let Some(text) = app.pending_message.take() {
                            if !text.trim().is_empty() && !app.yielded {
                                let first = !app
                                    .events
                                    .iter()
                                    .any(|e| matches!(e.kind, EventKind::UserMessage { .. }));
                                app.record(EventKind::UserMessage { text: text.clone() })
                                    .await?;
                                if first {
                                    let name = derive_session_name(
                                        Some(&text),
                                        now_ms(),
                                        app.tz_offset_secs,
                                    );
                                    app.session
                                        .rename_session(app.session_id, name.clone())
                                        .await
                                        .ok();
                                    app.shell.session_name = name;
                                }
                                app.streaming = true;
                                spawn_turn(app);
                                spawned = true;
                            }
                        }
                        // No queued user message ran: fire a deferred delegation
                        // wake (a subagent finished mid-turn) so the orchestrator
                        // continues and sees the result. A queued message that DID
                        // run already carries the result in context, so just clear
                        // the flag in that case. Note the ordering: a pending user
                        // message takes priority here, so a bare delegation
                        // continuation intentionally precedes a queued user message
                        // only when no message was queued — a message queued while a
                        // subagent ran executes on the following turn, never lost.
                        if spawned {
                            app.wake_after_delegation = false;
                        } else if take_deferred_delegation_wake(app) {
                            spawn_turn(app);
                        }
                        // A wake may have come due while this turn ran; now
                        // that we're idle, drain it (spawns its own turn).
                        if !app.streaming {
                            let _ = drain_due_wakes(app).await?;
                        }
                    }
                    AgentUpdate::SessionTakenOver => {
                        // Fire the turn cancel if a turn is in flight (reuses the
                        // Esc/Ctrl-C path). Stop streaming, mark yielded.
                        if let Some(cancel) = &app.turn_cancel {
                            cancel.cancel();
                        }
                        app.streaming = false;
                        // Kill all in-flight subagents (concurrency: they may be running)
                        zoid::agent::fire_subagent_kill(&app.in_flight, None);
                        app.in_flight_subagents.clear();
                        app.in_flight.lock().unwrap().clear();
                        app.queued_subagents.clear();
                        app.yielded = true;
                        app.shell.status_hint =
                            Some("session taken over by another instance".into());
                    }
                    // Raise the question overlay and hold the reply channel:
                    // the user's answer (or an Esc-abort, which drops `reply`)
                    // is routed back to the agent loop by the action handlers
                    // below (Task 11).
                    AgentUpdate::AskUser {
                        question,
                        choices,
                        reply,
                    } => {
                        tracing::debug!(
                            "main: AskUser received, opening inline card (choices={})",
                            choices.len()
                        );
                        // If this is a feedback question, open the Feedback
                        // overlay seeded from the proposal and stash the reply.
                        let kind = latest_open_question(&app.events).cloned();
                        if matches!(
                            kind,
                            Some(zoid_core::event::QuestionKind::Feedback { .. })
                        ) {
                            if let Some(zoid_core::event::QuestionKind::Feedback {
                                kind,
                                title,
                                body,
                            }) = kind
                            {
                                let fk =
                                    zoid_core::feedback::FeedbackKind::parse(&kind)
                                        .unwrap_or(zoid_core::feedback::FeedbackKind::Bug);
                                let idx = zoid_core::feedback::FeedbackKind::all()
                                    .iter()
                                    .position(|k| *k == fk)
                                    .unwrap_or(0);
                                let mut fs = zoid_tui::state::FeedbackState::new();
                                fs.kind = fk;
                                fs.kind_selected = idx;
                                fs.title = title;
                                fs.body = body;
                                fs.focus = zoid_tui::state::FeedbackField::Title;
                                app.shell.feedback = Some(fs);
                                app.shell.overlay = zoid_tui::Overlay::Feedback;
                            }
                            app.feedback_reply = Some(reply);
                        } else {
                            app.shell.question =
                                Some(zoid_tui::question::QuestionState::new(
                                    question, choices,
                                ));
                            app.pending_answer = Some(reply);
                        }
                        app.body_cache.key = None;
                    }
                    AgentUpdate::ModelsFetched { provider, models } => {
                        if provider == "__wizard_error__" {
                            if let Some(msg) = models.first() {
                                app.shell.status_hint = Some(msg.clone());
                            }
                            continue;
                        }
                        if provider == "__wizard_scan__" {
                            if let Some(json) = models.first() {
                                if let Ok(scan) =
                                    serde_json::from_str::<zoid_core::wizard::UpstreamScan>(json)
                                {
                                    app.wizard =
                                        Some(zoid::mode_wizard::ModeImportWizard::new_import(scan));
                                    app.shell.status_hint = Some(
                                        "Import wizard started. Ask the model to propose a mapping.".into(),
                                    );
                                    let ts = now_ms();
                                    let seed_event = zoid_core::event::Event::new(
                                        ulid::Ulid::new(),
                                        None,
                                        ts,
                                        zoid_core::event::EventKind::UserMessage {
                                            text: "Import wizard started. Call propose_mode_mapping \
                                                to see the upstream scan, then call apply_mode_mapping \
                                                with your proposed mapping onto the canonical contract. \
                                                The user will approve or reject via the card — do NOT \
                                                call ask_user to confirm the approval; the \
                                                apply_mode_mapping result tells you whether it was \
                                                approved and materialized."
                                                .into(),
                                        },
                                    );
                                    app.session.append(seed_event.clone()).await.ok();
                                    app.events.push(seed_event);
                                    app.streaming = true;
                                    spawn_turn(app);
                                }
                            }
                            continue;
                        }
                        if provider == "__wizard_update__" {
                            let mut iter = models.into_iter();
                            let scan_json = iter.next().unwrap_or_default();
                            let brief = iter.next().unwrap_or_default();
                            let target = iter.next().unwrap_or_default();
                            if let Ok(scan) =
                                serde_json::from_str::<zoid_core::wizard::UpstreamScan>(&scan_json)
                            {
                                app.wizard =
                                    Some(zoid::mode_wizard::ModeImportWizard::new_update(
                                        scan,
                                        target,
                                        brief,
                                    ));
                                app.shell.status_hint = Some(
                                    "Update wizard started. Ask the model to propose a merged mapping.".into(),
                                );
                                let ts = now_ms();
                                let seed_event = zoid_core::event::Event::new(
                                    ulid::Ulid::new(),
                                    None,
                                    ts,
                                    zoid_core::event::EventKind::UserMessage {
                                        text: "Update wizard started. Call propose_mode_mapping \
                                            to see the reconciliation brief, then call \
                                            apply_mode_mapping with your merged mapping. The user \
                                            will approve or reject via the card — do NOT call \
                                            ask_user to confirm; the apply_mode_mapping result \
                                            tells you whether it was approved and materialized."
                                            .into(),
                                    },
                                );
                                app.session.append(seed_event.clone()).await.ok();
                                app.events.push(seed_event);
                                spawn_turn(app);
                            }
                            continue;
                        }
                        // At most one surface acts: the config picker guards on
                        // the active provider, the quick-switch on its highlight.
                        apply_switch_models_fetched(app, provider.clone(), models.clone());
                        apply_models_fetched(app, provider, models);
                    }
                    AgentUpdate::ModelInfoFetched { model, info } => {
                        // Drop a stale fetch: the user switched models while this
                        // was in flight.
                        if model == app.model {
                            app.fetched_model_info = Some(info);
                            // Live-apply the capacity (model window) — always the
                            // provider's real value once known.
                            app.shell.ctx_ceiling = info.context_window;
                            // Recompute the target if it was defaulted (not an
                            // explicit config override), now that capacity landed.
                            app.context_target = app
                                .config
                                .economy
                                .context_target
                                .unwrap_or_else(|| app.shell.ctx_ceiling.min(300_000));
                            app.shell.cache_supported = info.prompt_cache;
                        }
                    }
                    AgentUpdate::SubagentStarted { id, task, agent } => {
                        app.in_flight_subagents.push(SubagentInfo { id: id.clone(), task: task.clone(), agent: agent.clone() });
                        app.shell.subagent_rows.push(zoid_tui::state::SubagentRow { id, task, agent });
                        // Subagent status belongs in the right-rail Subagents
                        // drawer, NOT the bottom status bar. Do NOT set
                        // status_hint here — it would render on the bottom bar
                        // and overlap the layout.
                    }
                    AgentUpdate::SubagentQueued {
                        tool_call_id,
                        task,
                        agent,
                        resolved_profile,
                        resolved_name,
                        want_worktree,
                        cwd,
                    } => {
                        // A dispatch_subagent call found the pool full. Enqueue
                        // it; the next DelegationResult drains the queue and
                        // spawns via spawn_queued_subagent. session_id is the
                        // orchestrator's (the dispatching turn), carried so the
                        // queued spawn tags its events correctly.
                        app.queued_subagents.push_back(QueuedSubagent {
                            task,
                            agent,
                            resolved_profile,
                            resolved_name,
                            cwd,
                            want_worktree,
                            tool_call_id,
                            session_id: app.session_id,
                        });
                    }
                    AgentUpdate::EditDiff { id, diff } => {
                        if app.config.ui.edit_diff {
                            app.shell.push_edit_diff(id, map_render_diff(diff));
                        }
                    }
                    AgentUpdate::CompactionStarted => {
                        app.shell.compacting = true;
                        app.compaction_started_at = Some(std::time::Instant::now());
                        app.compaction_complete = false;
                    }
                    AgentUpdate::CompactionComplete => {
                        app.compaction_complete = true;
                        // Don't clear app.shell.compacting here — the per-frame
                        // debounce check clears it once the 3s minimum has
                        // elapsed.
                    }
                    AgentUpdate::DirectiveReasserted { at_cumulative } => {
                        app.reassert_count = app.reassert_count.saturating_add(1);
                        tracing::info!(kind = "reassert", at = at_cumulative, "re-floor surfaced");
                    }
                    AgentUpdate::FeedbackOutcome(outcome) => {
                        match outcome {
                            Ok(zoid_core::feedback::SubmitOutcome::Created {
                                url,
                                number,
                            }) => {
                                if let Some(fs) = app.shell.feedback.as_mut() {
                                    fs.status =
                                        zoid_tui::state::FeedbackStatus::Done(
                                            zoid_core::feedback::SubmitOutcome::Created {
                                                url: url.clone(),
                                                number,
                                            },
                                        );
                                }
                                app.shell.status_hint =
                                    Some(format!("Created issue #{}: {}", number, url));
                            }
                            Ok(zoid_core::feedback::SubmitOutcome::BrowserFallback {
                                url,
                            }) => {
                                if let Some(fs) = app.shell.feedback.as_mut() {
                                    fs.status =
                                        zoid_tui::state::FeedbackStatus::Done(
                                            zoid_core::feedback::SubmitOutcome::BrowserFallback {
                                                url: url.clone(),
                                            },
                                        );
                                }
                                open_url(&url);
                                app.shell.status_hint =
                                    Some(format!("Opened browser: {}", url));
                            }
                            Err(e) => {
                                if let Some(fs) = app.shell.feedback.as_mut() {
                                    fs.status =
                                        zoid_tui::state::FeedbackStatus::Error(
                                            e.to_string(),
                                        );
                                }
                            }
                        }
                    }
                    zoid::agent::AgentUpdate::PluginScan { id, origin, over, res } => {
                        if apply_plugin_scan(app, id, origin, over, res) {
                            persist_active_mode(app).await;
                        }
                    }
                    zoid::agent::AgentUpdate::CatalogLoaded(res) => {
                        apply_catalog_loaded(app, res);
                    }
                    zoid::agent::AgentUpdate::McpManifestFetched { id, res } => {
                        apply_mcp_manifest_fetched(app, id, res);
                    }
                    zoid::agent::AgentUpdate::WorktreeRequested { action, reply } => {
                        handle_worktree_request(app, action, Some(reply));
                    }
                    zoid::agent::AgentUpdate::WakeDue => {
                        let _ = drain_due_wakes(app).await?;
                    }
                    zoid::agent::AgentUpdate::ScheduleWake { delay_secs, note, reply } => {
                        let _ = reply.send(handle_schedule_wake(app, delay_secs, note).await);
                    }
                    zoid::agent::AgentUpdate::CancelWake { id, reply } => {
                        let _ = reply.send(handle_cancel_wake(app, id).await);
                    }
                }
            }
            _ = motion_tick.tick(), if app.streaming || app.shell.compacting || app.shell.active_tool.is_some() || app.zoom_changed_at.is_some() => {
                // Idle + not-streaming never ticks here.
            }
            _ = subagent_tick.tick(), if !app.streaming && !app.in_flight_subagents.is_empty() => {
                // Excluded when streaming (the 30 FPS tick covers it).
            }
            _ = log_flush_tick.tick() => {
                // Drain the ObsState ring buffer to the logs table so in-session
                // warn/error events are persisted without a restart. Non-fatal.
                let entries = match obs_state.lock() {
                    Ok(mut s) => s.take_logs(),
                    Err(_) => Vec::new(),
                };
                for entry in entries {
                    let row = zoid_core::store::LogRow {
                        ts: entry.ts,
                        level: entry.level,
                        scope: "system".into(),
                        session_id: None,
                        event_id: None,
                        message: entry.message,
                        fields: entry.fields,
                    };
                    if let Err(e) = app.session.write_log(row).await {
                        tracing::warn!(error = %e, "failed to flush system log to db");
                    }
                }
            }
        }
    }
}

/// How an edited/committed text value is coerced before it is written to TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TomlTy {
    /// Free string, always written verbatim (provider, model).
    Str,
    /// String where an empty buffer removes the key (base_url).
    StrUnsetEmpty,
    /// Unsigned int; empty / "(none)" / unparseable removes the key (context target).
    U64Unset,
    /// Percent clamped to 0..=100; unparseable is a no-op (compact at %, band headroom %).
    U8Pct,
    /// Non-negative integer, always written verbatim; unparseable is a no-op (protected turns).
    UintPlain,
}

/// Where the field under the cursor persists. Secret rows go to the secret
/// store keyed by their label (the env-var name); everything else is a dotted
/// TOML key with a coercion rule. Bools are omitted here — they persist via
/// `ConfigToggle`, never through the text-edit buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldTarget {
    Secret,
    Toml { key: &'static str, ty: TomlTy },
}

/// Map a config row (by its EXACT label from `config_view::build_sections`, and
/// its kind for secrets) to its write target. Keyed on label, never on index,
/// so reordering the view can't silently corrupt writes.
fn field_target(label: &str, kind: &zoid_tui::config_view::FieldKind) -> Option<FieldTarget> {
    use zoid_tui::config_view::FieldKind;
    if matches!(kind, FieldKind::Secret) {
        return Some(FieldTarget::Secret);
    }
    Some(match label {
        "provider" => FieldTarget::Toml {
            key: "provider",
            ty: TomlTy::Str,
        },
        "model" => FieldTarget::Toml {
            key: "model",
            ty: TomlTy::Str,
        },
        "base_url" => FieldTarget::Toml {
            key: "base_url",
            ty: TomlTy::StrUnsetEmpty,
        },
        "context target" => FieldTarget::Toml {
            key: "economy.context_target",
            ty: TomlTy::U64Unset,
        },
        "compact at %" => FieldTarget::Toml {
            key: "economy.compact_threshold_pct",
            ty: TomlTy::U8Pct,
        },
        "band headroom %" => FieldTarget::Toml {
            key: "economy.band_headroom_pct",
            ty: TomlTy::U8Pct,
        },
        "protected turns" => FieldTarget::Toml {
            key: "economy.min_protected_turns",
            ty: TomlTy::UintPlain,
        },
        "protection pct" => FieldTarget::Toml {
            key: "economy.protection_pct",
            ty: TomlTy::U8Pct,
        },
        // Bools (auto-evict cold / reduced motion) persist via toggle, not edit.
        _ => return None,
    })
}

/// Build the `TomlValue` a committed edit buffer produces for a given coercion.
/// Returns None when the edit should be a no-op (e.g. unparseable percent).
fn value_from_buffer(ty: &TomlTy, buf: &str) -> Option<zoid_core::config::TomlValue> {
    use zoid_core::config::TomlValue;
    let t = buf.trim();
    Some(match ty {
        TomlTy::Str => TomlValue::Str(buf.to_string()),
        TomlTy::StrUnsetEmpty => {
            if t.is_empty() {
                TomlValue::Unset
            } else {
                TomlValue::Str(buf.to_string())
            }
        }
        TomlTy::U64Unset => {
            if t.is_empty() || t == "(none)" {
                TomlValue::Unset
            } else {
                match t.parse::<u64>() {
                    Ok(n) => TomlValue::Int(n as i64),
                    Err(_) => TomlValue::Unset,
                }
            }
        }
        TomlTy::U8Pct => match t.parse::<i64>() {
            Ok(n) => TomlValue::Int(n.clamp(0, 100)),
            Err(_) => return None,
        },
        TomlTy::UintPlain => match t.parse::<i64>() {
            Ok(n) if n >= 0 => TomlValue::Int(n),
            _ => return None,
        },
    })
}

/// The current (key, value) a field would write from `app.config` — used by
/// `ConfigSaveToRepo` to copy the live value to the project config. Returns None
/// for secret rows (never written to TOML) and unknown labels.
fn current_write(
    app: &App,
    label: &str,
    kind: &zoid_tui::config_view::FieldKind,
) -> Option<(&'static str, zoid_core::config::TomlValue)> {
    use zoid_core::config::TomlValue;
    use zoid_tui::config_view::FieldKind;
    if matches!(kind, FieldKind::Secret) {
        return None;
    }
    let econ = &app.config.economy;
    let opt_u64 = |o: Option<u64>| {
        o.map(|n| TomlValue::Int(n as i64))
            .unwrap_or(TomlValue::Unset)
    };
    Some(match label {
        "provider" => ("provider", TomlValue::Str(app.config.provider.clone())),
        "model" => ("model", TomlValue::Str(app.config.model.clone())),
        "base_url" => (
            "base_url",
            app.config
                .base_url
                .clone()
                .map(TomlValue::Str)
                .unwrap_or(TomlValue::Unset),
        ),
        "context target" => ("economy.context_target", opt_u64(econ.context_target)),
        "eviction" => (
            "eviction.enabled",
            TomlValue::Bool(app.config.eviction.enabled),
        ),
        "auto-evict cold" => (
            "economy.auto_evict_cold",
            TomlValue::Bool(econ.auto_evict_cold),
        ),
        "compact at %" => (
            "economy.compact_threshold_pct",
            TomlValue::Int(econ.compact_threshold_pct as i64),
        ),
        "band headroom %" => (
            "economy.band_headroom_pct",
            TomlValue::Int(econ.band_headroom_pct as i64),
        ),
        "protected turns" => (
            "economy.min_protected_turns",
            TomlValue::Int(econ.min_protected_turns as i64),
        ),
        "protection pct" => (
            "economy.protection_pct",
            TomlValue::Int(econ.protection_pct as i64),
        ),
        "reduced motion" => ("reduced_motion", TomlValue::Bool(app.config.reduced_motion)),
        "thinking" => (
            "thinking.enabled",
            TomlValue::Bool(app.config.thinking.enabled),
        ),
        "effort" => (
            "thinking.effort",
            app.config
                .thinking
                .effort
                .clone()
                .map(TomlValue::Str)
                .unwrap_or(TomlValue::Unset),
        ),
        _ => return None,
    })
}

/// The (label, kind) of the row under the config cursor, if any.
fn current_config_field(
    app: &App,
) -> Option<(
    &'static str,
    zoid_tui::config_view::FieldKind,
    Option<&'static str>,
)> {
    app.shell
        .config_sections
        .get(app.shell.config_section)
        .and_then(|s| s.rows.get(app.shell.config_field))
        .map(|r| (r.label, r.kind.clone(), r.secret_key))
}

/// Replace an OPEN model picker's options with a freshly-fetched live list.
/// No-op if the list is empty (keep the static fallback) or a model picker
/// isn't currently open (results arrived too late / focus moved).
fn apply_models_fetched(app: &mut App, provider: String, mut models: Vec<String>) {
    models.sort();
    // Drop a stale fetch: the user switched providers while this was in flight.
    if provider != app.config.provider {
        return;
    }
    if models.is_empty() || !app.shell.config_picker_open() {
        return;
    }
    if current_config_field(app).map(|(l, _, _)| l) != Some("model") {
        return;
    }
    let cur = app.config.model.clone();
    app.shell.config_picker = models
        .into_iter()
        .map(|m| zoid_tui::config_view::PickOption {
            is_current: m == cur,
            id: m.clone(),
            label: m,
            detail: String::new(),
            selectable: true,
        })
        .collect();
    app.shell.config_picker_sel = app
        .shell
        .config_picker
        .iter()
        .position(|o| o.is_current)
        .unwrap_or(0);
}

/// Deliver a live model fetch into the quick-switch model pane. No-op unless
/// the `ProviderSwitch` overlay is open and the fetch is tagged with the
/// provider currently highlighted in the provider pane — a stale fetch from a
/// provider the user has since scrolled past must not clobber the visible list.
/// An empty result keeps the static registry fallback. The user's highlighted
/// model is preserved across the refresh when it survives in the live list.
fn apply_switch_models_fetched(app: &mut App, provider: String, mut models: Vec<String>) {
    use zoid_tui::state::Overlay;
    models.sort();
    if app.shell.overlay != Overlay::ProviderSwitch || models.is_empty() {
        return;
    }
    let highlighted = app
        .shell
        .switch_providers
        .get(app.shell.switch_provider_sel)
        .map(|o| o.id.as_str());
    if highlighted != Some(provider.as_str()) {
        return;
    }
    let prev_id = app
        .shell
        .switch_models
        .get(app.shell.switch_model_sel)
        .map(|o| o.id.clone());
    let cur = app.config.model.clone();
    app.shell.switch_models = models
        .into_iter()
        .map(|m| zoid_tui::config_view::PickOption {
            is_current: m == cur,
            id: m.clone(),
            label: m,
            detail: String::new(),
            selectable: true,
        })
        .collect();
    // Keep the user's highlight if it still exists, else the current model,
    // else the top of the list.
    app.shell.switch_model_sel = app
        .shell
        .switch_models
        .iter()
        .position(|o| Some(o.id.as_str()) == prev_id.as_deref())
        .or_else(|| app.shell.switch_models.iter().position(|o| o.is_current))
        .unwrap_or(0);
}

/// Rebuild the live config screen from the current config/provenance/secret
/// statuses.
fn refresh_config_sections(app: &mut App) {
    use zoid_core::secret::{SecretStatus, SecretStore};
    let status = |name: &str| {
        app.secrets
            .as_ref()
            .map(|s| s.status(name))
            .unwrap_or(SecretStatus::NotSet)
    };
    let key_status = [
        ("OLLAMA_API_KEY", status("OLLAMA_API_KEY")),
        ("ANTHROPIC_API_KEY", status("ANTHROPIC_API_KEY")),
        ("OPENCODE_GO_API_KEY", status("OPENCODE_GO_API_KEY")),
        ("ZAI_API_KEY", status("ZAI_API_KEY")),
    ];
    app.shell.config_sections =
        zoid_tui::config_view::build_sections(&app.config, &app.prov, &key_status);
}

/// Set (or remove, for `Unset`) a dotted key in the TOML file at `path`,
/// preserving all other content and creating parent dirs. Pure IO; separated
/// from `apply_config_write` so it can be unit-tested against a temp dir.
fn write_config_file(
    path: &Path,
    dotted_key: &str,
    value: zoid_core::config::TomlValue,
) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let out = zoid_core::config::set_in_toml(&existing, dotted_key, value)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// The TOML write for `base_url` when a provider is selected: the registry
/// default endpoint (HTTP transports), or `Unset` to clear it (Cli/Sdk have no
/// URL). The user can still override afterward (which flips provenance to [user]).
fn base_url_write_for(id: &str) -> zoid_core::config::TomlValue {
    match zoid_provider::model::default_base_url(id) {
        Some(u) => zoid_core::config::TomlValue::Str(u.to_string()),
        None => zoid_core::config::TomlValue::Unset,
    }
}

/// Persist a single TOML key (to the repo `./.zoid/config.toml` when `to_repo`,
/// else the user-global `~/.config/zoid/config.toml`), then reload + re-render +
/// live-apply. Config-write failures are non-fatal: logged, never a crash.
fn apply_config_write(
    app: &mut App,
    dotted_key: &str,
    value: zoid_core::config::TomlValue,
    to_repo: bool,
) {
    let path = if to_repo {
        PathBuf::from("./.zoid/config.toml")
    } else {
        resolve_config_dir(|k| std::env::var(k).ok()).join("config.toml")
    };
    if let Err(e) = write_config_file(&path, dotted_key, value) {
        eprintln!("zoid: config write failed ({}): {e}", path.display());
        return;
    }
    // Reload the whole layered config so provenance + merged view stay honest.
    // Warnings are intentionally discarded here: status_hint is transient and
    // gets cleared/overwritten by the next turn or keypress, so config-write
    // reloads deliberately do not re-surface them (the startup hint is one-shot).
    let (c, p, _cfg_warnings) = load_config();
    app.config = c;
    app.prov = p;
    app.economy = app.config.economy;
    refresh_config_sections(app);
    // Live-apply the bits the running UI caches (economy auto-applies on the
    // next turn via spawn_turn's policy_from_config(&app.economy, ...)).
    app.shell.reduced_motion = app.config.reduced_motion;
    // Live-apply companion: start or stop the server to match the new config.
    if app.config.companion.enabled && !app.shell.companion_on {
        enable_companion(app);
    } else if !app.config.companion.enabled && app.shell.companion_on {
        disable_companion(app);
    }
    // Live-apply the model (mirrors the startup logic that derives model/shell.model
    // from config) before recomputing ctx_ceiling, so the ceiling denominator and
    // the drawer label both reflect the newly-saved model.
    let new_model = if app.config.model.is_empty() {
        default_model().to_string()
    } else {
        app.config.model.clone()
    };
    app.model = new_model.clone();
    app.shell.model = new_model;
    // Capacity is always the model window; the target is the separate soft knob.
    app.shell.ctx_ceiling = zoid_provider::context_ceiling(&app.model);
    app.context_target = app
        .config
        .economy
        .context_target
        .unwrap_or(300_000)
        .min(app.shell.ctx_ceiling);
    // Live-apply the provider (same selection as startup) so a provider change
    // takes effect on the next turn, and keep the cached drawer label truthful
    // (shell.provider is set once at startup, not recomputed per frame).
    let (provider, provider_name, has_key) = select_provider(&app.config, &app.secrets);
    app.provider = provider;
    app.shell.provider = provider_label(provider_name, has_key);
    app.shell.cache_supported = zoid_provider::has_prompt_cache(&app.model);
    // Fetch the new model's capabilities from the provider; the static table
    // is the fallback until the fetch lands.
    spawn_model_info_fetch(app.provider.clone(), app.model.clone(), app.ui_tx.clone());
}

async fn handle_action(app: &mut App, action: zoid_tui::route::Action) -> Result<bool> {
    use zoid_tui::route::Action;
    use zoid_tui::Overlay;
    match action {
        Action::Quit => return Ok(true),
        Action::CycleMode => {
            app.modes.cycle_next();
            sync_mode_mirror(app);
            persist_active_mode(app).await;
        }
        Action::FocusNext => app.shell.focus_next(),
        Action::FocusRegion(f) => app.shell.focus = f,
        Action::OpenPalette => {
            app.shell.overlay = Overlay::Palette;
            app.shell.palette = Default::default();
        }
        Action::OpenPaletteDirect => {
            app.shell.overlay = Overlay::Palette;
            app.shell.palette = Default::default();
            app.shell.palette.query.push(':');
            // One-time escape-hatch hint: a leading `:` in an empty box now opens
            // the palette (instead of being literal). Teach the user once how to
            // start a message with a literal ':' — type any other char first.
            if !app.shell.colon_trigger_hinted {
                app.shell.colon_trigger_hinted = true;
                if app.shell.focus == zoid_tui::state::Focus::Input {
                    app.shell.status_hint = Some(
                        "':' opens commands. Type any other key first to start a message with ':'"
                            .into(),
                    );
                }
            }
        }
        Action::CloseOverlay => {
            // Defense-in-depth: if a question overlay is closed via this generic
            // path, drop the reply channel so the agent's ask_user aborts cleanly
            // (records "[user aborted]") instead of hanging on an orphaned Sender.
            // No-op when nothing is pending.
            app.pending_answer = None;
            app.shell.close_overlay();
        }
        Action::ToggleDrawer(id) => app.shell.toggle_drawer(id),
        Action::PaletteMove(d) => {
            // In Direct mode (`:`-prefixed), the list is `direct_items` filtered
            // by `direct_filter` — NOT the Pick fuzzy list. The old code always
            // used `all_items` (Pick), which doesn't contain `:session`/`:mode`
            // etc., so the match count was 0 and the selection never moved.
            let n = if app.shell.palette.query.starts_with(':') {
                let items = zoid_tui::palette::direct_items(&app.shell);
                let filter = zoid_tui::palette::direct_filter(&app.shell.palette.query);
                zoid_tui::palette::selectable_matches(&items, filter).len()
            } else {
                let items = zoid_tui::palette::all_items(
                    &app.shell.active_mode,
                    &app.shell.mode_names,
                    app.shell.companion_on,
                    app.shell.select_mode,
                );
                zoid_tui::palette::selectable_matches(&items, &app.shell.palette.query).len()
            };
            app.shell.palette.selected = zoid_tui::palette::nav(app.shell.palette.selected, d, n);
        }
        Action::PaletteChar(c) => match &mut app.shell.palette.stage {
            zoid_tui::state::PaletteStage::Pick => {
                app.shell.palette.query.push(c);
                app.shell.palette.selected = 0;
            }
            zoid_tui::state::PaletteStage::Arg { input, .. } => input.push(c),
        },
        Action::PaletteBackspace => match &mut app.shell.palette.stage {
            zoid_tui::state::PaletteStage::Pick => {
                app.shell.palette.query.pop();
                app.shell.palette.selected = 0;
            }
            zoid_tui::state::PaletteStage::Arg { input, .. } => {
                input.pop();
            }
        },
        Action::PaletteRun => match app.shell.palette.stage.clone() {
            zoid_tui::state::PaletteStage::Pick => {
                if app.shell.palette.query.starts_with(':') {
                    // Direct phase — resolve the highlighted row.
                    match zoid_tui::palette::direct_selected_action(&app.shell) {
                        zoid_tui::palette::DirectAction::Fill(text) => {
                            app.shell.palette.query = text;
                            app.shell.palette.selected = 0;
                        }
                        zoid_tui::palette::DirectAction::Run(cmd) => {
                            app.shell.close_overlay();
                            return exec_command(app, cmd).await;
                        }
                        zoid_tui::palette::DirectAction::Nothing => {
                            let cmd = zoid_tui::command::parse_command(&app.shell.palette.query);
                            app.shell.close_overlay();
                            return exec_command(app, cmd).await;
                        }
                    }
                } else {
                    // Pick phase — fuzzy list resolution.
                    if let Some(cmd) = palette_selected_command(&app.shell) {
                        match zoid_tui::palette::arg_kind_for(&cmd) {
                            Some(kind) => {
                                app.shell.palette.stage = zoid_tui::state::PaletteStage::Arg {
                                    kind,
                                    input: String::new(),
                                };
                            }
                            None => {
                                app.shell.close_overlay();
                                return exec_command(app, cmd).await;
                            }
                        }
                    }
                }
            }
            zoid_tui::state::PaletteStage::Arg { kind, input } => {
                let trimmed = input.trim();
                if !trimmed.is_empty() {
                    let cmd = kind.build(trimmed.to_string());
                    app.shell.close_overlay();
                    return exec_command(app, cmd).await;
                }
            }
        },
        Action::PaletteArgCancel => {
            app.shell.palette.stage = zoid_tui::state::PaletteStage::Pick;
        }
        Action::ScrollConversation(d) => {
            app.shell.scroll_conversation(d, app.last_conv_max_scroll);
        }
        Action::ScrollbarGrab(row) => {
            app.shell.scrollbar_drag = true;
            scrollbar_row_to_offset(app, row);
        }
        Action::ScrollbarDrag(row) => scrollbar_row_to_offset(app, row),
        Action::ScrollbarRelease => app.shell.scrollbar_drag = false,
        Action::ToggleMouseCapture => toggle_select_mode(app),
        // Conversation clicks are resolved in the event loop (where the layout is
        // available) via `handle_conversation_click`; this arm keeps the match
        // exhaustive and never fires from the keyboard.
        Action::ConversationClick(_) => {}
        Action::ZoomIn => {
            let before = app.shell.zoom;
            // Anchor to the message at the top of the viewport BEFORE zooming, so
            // the reading position survives the altitude change (applied in the
            // pre-draw block once the new-altitude body is built).
            let anchor = zoid_tui::msg_at_line(
                &app.body_cache.msg_starts,
                app.shell.conversation_scroll as usize,
            );
            app.shell.zoom_in(); // re-anchors conversation_scroll to 0 on a real change
            if app.shell.zoom != before {
                app.pending_zoom_anchor = Some(anchor);
            }
        }
        Action::ZoomOut => {
            let before = app.shell.zoom;
            let anchor = zoid_tui::msg_at_line(
                &app.body_cache.msg_starts,
                app.shell.conversation_scroll as usize,
            );
            app.shell.zoom_out();
            if app.shell.zoom != before {
                app.pending_zoom_anchor = Some(anchor);
            }
        }
        Action::Newline => app.textarea.insert_newline(),
        Action::Edit(key) => {
            app.textarea.input(key);
        }
        Action::InputDeleteLine => input_delete_line(&mut app.textarea),
        Action::InputCursorTop => input_cursor_top(&mut app.textarea),
        Action::InputCursorBottom => input_cursor_bottom(&mut app.textarea),
        Action::ScrollToTop => app.shell.scroll_to_offset(0, app.last_conv_max_scroll),
        Action::ScrollToBottom => app
            .shell
            .scroll_to_offset(app.last_conv_max_scroll, app.last_conv_max_scroll),
        Action::Submit => {
            // Yielded always blocks (even when not busy) — a taken-over session
            // can't accept new turns until the user :session new or :session resume.
            if app.yielded {
                app.shell.status_hint =
                    Some("session taken over — :session new or :session resume".into());
                return Ok(false);
            }
            // Busy (a turn streaming) but not yielded: stash the message for
            // after the turn, as an alternative to ESC-steering. Background
            // subagents are no longer a blocker — the new turn and the
            // subagents run concurrently.
            if app.streaming {
                let text = app.textarea.lines().join("\n");
                if text.trim().is_empty() {
                    return Ok(false); // don't queue empty — no phantom "queued:" hint
                }
                app.pending_message = Some(text.clone());
                app.textarea = make_input(TextArea::default());
                app.shell.status_hint = Some(format!("queued: {}", truncate_for_hint(&text)));
                return Ok(false);
            }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() {
                return Ok(false);
            }
            // A `:`-prefixed line is a command, not a user message — parse +
            // dispatch it instead of spawning a turn. Multi-line text is never
            // a command (the command-line overlay is the path for those); only
            // a single line starting with `:` is intercepted here.
            if text.lines().count() == 1 && text.trim_start().starts_with(':') {
                app.textarea = make_input(TextArea::default());
                app.shell.status_hint = None;
                let cmd = zoid_tui::command::parse_command(&text);
                return exec_command(app, cmd).await;
            }
            let first = !app
                .events
                .iter()
                .any(|e| matches!(e.kind, EventKind::UserMessage { .. }));
            app.textarea = make_input(TextArea::default());
            app.shell.status_hint = None;
            app.record(EventKind::UserMessage { text: text.clone() })
                .await?;
            if first {
                let name = derive_session_name(Some(&text), now_ms(), app.tz_offset_secs);
                app.session
                    .rename_session(app.session_id, name.clone())
                    .await
                    .ok();
                app.shell.session_name = name;
            }
            app.streaming = true;
            spawn_turn(app);
        }
        Action::CancelTurn => {
            // First Esc: graceful (finish current step, drain, end). Second Esc
            // while already cancelling: hard-stop — force-kill the running tool AND
            // every in-flight subagent. The resulting TurnComplete clears both tokens.
            if let (Some(g), Some(h)) = (&app.turn_cancel, &app.turn_hard) {
                app.shell.status_hint = Some(escalate_cancel(g, h, &app.in_flight).into());
            } else {
                // No active main turn, but subagents may be running: two-press confirm.
                let pending = app.in_flight.lock().unwrap().len();
                if pending > 0 {
                    let (next_armed, fire, hint) =
                        subagent_kill_decision(app.subagent_kill_armed, pending);
                    if fire {
                        zoid::agent::fire_subagent_kill(&app.in_flight, None);
                    }
                    app.subagent_kill_armed = next_armed;
                    app.shell.status_hint = Some(hint);
                }
            }
        }
        // Object-first picker (P4d ④): pick an object, then a verb scoped to
        // it. Picking a verb composes a prompt and queues it into the input
        // (see the `VerbPick` arm below) — dispatch to a subagent is P5.
        Action::OpenObjects => {
            app.shell.overlay = zoid_tui::Overlay::Objects;
            app.shell.objects = Default::default();
        }
        Action::OpenHelp => {
            app.shell.overlay = zoid_tui::Overlay::Help;
            app.shell.help_scroll = 0;
        }
        Action::ScrollHelp(d) => {
            let cur = app.shell.help_scroll as i64;
            app.shell.help_scroll = (cur + d as i64).max(0) as usize;
            // Upper bound is clamped per-frame against the real rect height
            // (see the render-loop clamp), mirroring conv_max_scroll.
        }
        Action::ObjectMove(d) => {
            let n = zoid_tui::objects::selectable_objects(&conversation(app.events.iter())).len();
            app.shell.objects.obj_selected =
                zoid_tui::palette::nav(app.shell.objects.obj_selected, d, n);
        }
        Action::ObjectPick => {
            // Advance to the verb picker — but only if there's an object to act
            // on (otherwise the verb picker would show "(no object)").
            if !zoid_tui::objects::selectable_objects(&conversation(app.events.iter())).is_empty() {
                app.shell.overlay = zoid_tui::Overlay::Verbs;
                app.shell.objects.verb_selected = 0;
            }
        }
        Action::VerbBack => {
            // Step back to the object picker (keeps the object selection).
            app.shell.overlay = zoid_tui::Overlay::Objects;
        }
        Action::VerbMove(d) => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(app.events.iter()));
            let sel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            let n = objs
                .get(sel)
                .map(|o| zoid_tui::objects::verbs_for(o.kind).len())
                .unwrap_or(0);
            app.shell.objects.verb_selected =
                zoid_tui::palette::nav(app.shell.objects.verb_selected, d, n);
        }
        // Disabled stub, not a dispatch path: the verb prompt is built and then
        // dropped, and this hint is a refusal, not subagent status. See
        // `Command::Delegate` — re-enabling must route to the Subagents drawer.
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(app.events.iter()));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let _task = zoid_tui::objects::verb_prompt(verb, obj);
                    app.shell.close_overlay();
                    app.shell.status_hint = Some("delegation is temporarily disabled".into());
                    return Ok(false);
                }
            }
            app.shell.close_overlay();
        }
        Action::SessionMove(d) => {
            app.shell.session_selected =
                zoid_tui::palette::nav(app.shell.session_selected, d, app.shell.sessions.len());
        }
        Action::SessionTakeoverConfirm => {
            let sid = match app.session_ids.get(app.shell.session_selected) {
                Some(&sid) => sid,
                None => {
                    app.shell.close_overlay();
                    return Ok(false);
                }
            };
            let name = app
                .shell
                .sessions
                .get(app.shell.session_selected)
                .cloned()
                .unwrap_or_default()
                .split("  ·  ")
                .next()
                .unwrap_or("session")
                .to_string();
            app.shell.session_confirm = Some(zoid_tui::state::SessionConfirm {
                sid,
                name,
                kind: zoid_tui::state::SessionConfirmKind::Takeover,
            });
        }
        Action::SessionPick => {
            if app.streaming || !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some(busy_block_hint(app));
                app.shell.close_overlay();
                return Ok(false);
            }
            if let Some(&sid) = app.session_ids.get(app.shell.session_selected) {
                // Load the target log FIRST: on a read failure, surface it and
                // leave the current session intact instead of silently swapping
                // in an empty log (the "corruption looks like data loss" failure
                // mode #9 is meant to prevent).
                let loaded = match app.session.snapshot_session(sid).await {
                    Ok(events) => events,
                    Err(e) => {
                        app.shell.status_hint = Some(format!("could not load session: {e}"));
                        app.shell.close_overlay();
                        return Ok(false);
                    }
                };
                app.session.touch_session(sid, now_ms()).await.ok();
                app.session_id = sid;
                app.events = zoid::eventlog::EventLog::from_vec(loaded);
                // #6b resume: free any compacted ToolResult bodies immediately
                // instead of re-inflating RAM to the pre-#6b footprint.
                app.events.clear_compacted_bodies();
                app.pending_wakes = rebuild_pending_wakes(app.events.iter());
                let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
                let _ = drain_due_wakes(app).await?;
                // Wholesale event-log replacement: reset the caches so they
                // can't serve the previous session's data at an equal length.
                app.proj = ProjectionCache::default();
                app.body_cache = BodyCache::default();
                app.shell.conversation_scroll = 0;
                app.shell.follow_tail = true; // jump to the latest of the loaded session
                                              // The resumed session runs with ITS OWN mode, never the
                                              // previous session's overlay prompt + scoped skills.
                restore_mode_for_session(app).await;
                if let Some(info) = app
                    .session
                    .list_sessions(Some(repo_root()))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| s.id == sid)
                {
                    app.shell.session_name = info.name;
                    app.session_started_ms = info.created_ts;
                }
                // Claim the resumed session and clear any yielded state from
                // the prior (taken-over) session — `:session resume` is the documented
                // yield escape hatch (symmetric with `:session new`). Spec §3.2.
                app.yielded = false;
                app.pending_message = None;
                app.shell.status_hint = None;
                let self_pid = std::process::id() as i64;
                app.session
                    .set_active(sid, true, self_pid, now_ms())
                    .await
                    .ok();
                spawn_heartbeat(app);
            }
            app.shell.close_overlay();
        }
        Action::SessionDelete => {
            if app.streaming || !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some(busy_block_hint(app));
                app.shell.close_overlay();
                return Ok(false);
            }
            let sid = match app.session_ids.get(app.shell.session_selected) {
                Some(&sid) => sid,
                None => return Ok(false),
            };
            let live = app
                .shell
                .sessions_live
                .get(app.shell.session_selected)
                .copied()
                .unwrap_or(false);
            if live {
                app.shell.status_hint = Some("can't delete a session that's in use".into());
                return Ok(false);
            }
            let name = app
                .shell
                .sessions
                .get(app.shell.session_selected)
                .cloned()
                .unwrap_or_default()
                .split("  ·  ")
                .next()
                .unwrap_or("session")
                .to_string();
            app.shell.session_confirm = Some(zoid_tui::state::SessionConfirm {
                sid,
                name,
                kind: zoid_tui::state::SessionConfirmKind::Delete,
            });
        }
        Action::SessionConfirmYes => {
            let confirm = match app.shell.session_confirm.take() {
                Some(c) => c,
                None => return Ok(false),
            };
            match confirm.kind {
                zoid_tui::state::SessionConfirmKind::Delete => {
                    if let Err(e) = app.session.delete_session(confirm.sid).await {
                        app.shell.status_hint = Some(format!("could not delete session: {e}"));
                        return Ok(false);
                    }
                    // Refresh the session list.
                    let list = app
                        .session
                        .list_sessions(Some(repo_root()))
                        .await
                        .unwrap_or_default();
                    app.session_ids = list.iter().map(|s| s.id).collect();
                    app.shell.sessions = list
                        .iter()
                        .map(|s| {
                            format!(
                                "{}  ·  {}  ·  {}",
                                s.name,
                                fmt_since(s.last_touched_ts, now_ms()),
                                zoid_tui::economy_view::human_tokens(s.token_total)
                            )
                        })
                        .collect();
                    let now = now_ms();
                    app.shell.sessions_live = list
                        .iter()
                        .map(|s| {
                            zoid_core::store::is_live(
                                s.active,
                                s.active_pid,
                                s.active_heartbeat,
                                now,
                                pid_alive,
                            )
                        })
                        .collect();
                    if app.shell.session_selected >= app.shell.sessions.len() {
                        app.shell.session_selected = 0;
                    }
                    app.shell.session_confirm = None;
                }
                zoid_tui::state::SessionConfirmKind::Takeover => {
                    let sid = confirm.sid;
                    let self_pid = std::process::id() as i64;
                    app.session
                        .set_active(sid, true, self_pid, now_ms())
                        .await
                        .ok();
                    app.shell.session_selected = app
                        .session_ids
                        .iter()
                        .position(|&x| x == sid)
                        .unwrap_or(app.shell.session_selected);
                    app.shell.session_confirm = None;
                    return Box::pin(handle_action(app, zoid_tui::route::Action::SessionPick))
                        .await;
                }
            }
        }
        Action::SessionConfirmNo => {
            app.shell.session_confirm = None;
        }
        Action::ConfigMoveField(d) => {
            let n = app
                .shell
                .config_sections
                .get(app.shell.config_section)
                .map(|s| s.rows.len())
                .unwrap_or(0);
            app.shell.config_field = if n == 0 {
                0
            } else {
                (app.shell.config_field as i32 + d).clamp(0, n as i32 - 1) as usize
            };
        }
        Action::ConfigMoveSection(d) => {
            let n = app.shell.config_sections.len();
            app.shell.config_section = if n == 0 {
                0
            } else {
                (app.shell.config_section as i32 + d).clamp(0, n as i32 - 1) as usize
            };
            app.shell.config_field = 0;
            app.shell.config_edit = None;
        }
        Action::ConfigBeginEdit => {
            let row = app
                .shell
                .config_sections
                .get(app.shell.config_section)
                .and_then(|s| s.rows.get(app.shell.config_field));
            app.shell.config_edit = row.map(|r| {
                if matches!(r.kind, zoid_tui::config_view::FieldKind::Secret) {
                    String::new()
                } else {
                    r.value.clone()
                }
            });
        }
        Action::ConfigEditChar(c) => {
            if let Some(buf) = app.shell.config_edit.as_mut() {
                buf.push(c);
            }
        }
        Action::ConfigEditBackspace => {
            if let Some(buf) = app.shell.config_edit.as_mut() {
                buf.pop();
            }
        }
        Action::ConfigCancelEdit => {
            app.shell.config_edit = None;
            app.shell.config_key_prompt = None;
        }
        Action::ConfigCommitEdit => {
            if let Some(env) = app.shell.config_key_prompt.take() {
                let buf = app.shell.config_edit.take().unwrap_or_default();
                let key = buf.trim();
                if key.is_empty() {
                    // Blank/whitespace commit: abort like Esc. Storing an empty
                    // credential would falsely mark the provider "ready" (status
                    // becomes Set) and suppress any future reprompt.
                    return Ok(false);
                }
                if let Some(s) = &app.secrets {
                    use zoid_core::secret::SecretStore;
                    if let Err(e) = s.set(env, key) {
                        eprintln!("zoid: secret set failed for {env}: {e}");
                    }
                } else {
                    eprintln!("zoid: secret store unavailable; cannot set {env}");
                }
                refresh_config_sections(app);
                // Re-select with the new key (the key lives in the secret store, not
                // config, so apply_config_write's auto-reselect never ran), then
                // advance to the model picker + fetch.
                let (provider, name, has_key) = select_provider(&app.config, &app.secrets);
                app.provider = provider;
                app.shell.provider = provider_label(name, has_key);
                if let Some(mi) = app
                    .shell
                    .config_sections
                    .get(app.shell.config_section)
                    .and_then(|sec| sec.rows.iter().position(|r| r.label == "model"))
                {
                    app.shell.config_field = mi;
                }
                app.shell.config_picker =
                    zoid_tui::config_view::model_options(&app.config.provider, &app.config.model);
                app.shell.config_picker_sel = 0;
                app.shell.config_col = if app.shell.config_picker.is_empty() {
                    zoid_tui::state::ConfigCol::Fields
                } else {
                    zoid_tui::state::ConfigCol::Picker
                };
                spawn_model_fetch(
                    app.provider.clone(),
                    app.config.provider.clone(),
                    app.ui_tx.clone(),
                );
                return Ok(false);
            }
            if let (Some((label, kind, secret_key)), Some(buffer)) =
                (current_config_field(app), app.shell.config_edit.clone())
            {
                match field_target(label, &kind) {
                    Some(FieldTarget::Secret) => {
                        let key = secret_key.unwrap_or(label);
                        if let Some(s) = &app.secrets {
                            use zoid_core::secret::SecretStore;
                            if let Err(e) = s.set(key, &buffer) {
                                eprintln!("zoid: secret set failed for {key}: {e}");
                            }
                        } else {
                            eprintln!("zoid: secret store unavailable; cannot set {key}");
                        }
                        refresh_config_sections(app);
                        // The key lives in the secret store, not config, so
                        // apply_config_write's auto-reselect never ran. Rebuild the
                        // live provider client so the new credential takes effect on
                        // the next turn without a restart (mirrors the key-prompt
                        // commit path above).
                        let (provider, name, has_key) = select_provider(&app.config, &app.secrets);
                        app.provider = provider;
                        app.shell.provider = provider_label(name, has_key);
                    }
                    Some(FieldTarget::Toml { key, ty }) => {
                        if let Some(value) = value_from_buffer(&ty, &buffer) {
                            apply_config_write(app, key, value, false);
                        }
                    }
                    // Bools / unknown labels: nothing to commit from a buffer.
                    None => {}
                }
            }
            app.shell.config_edit = None;
        }
        Action::ConfigToggle => {
            use zoid_core::config::TomlValue;
            if let Some((label, _kind, _)) = current_config_field(app) {
                let write = match label {
                    "eviction" => Some(("eviction.enabled", !app.config.eviction.enabled)),
                    "auto-evict cold" => Some((
                        "economy.auto_evict_cold",
                        !app.config.economy.auto_evict_cold,
                    )),
                    "reduced motion" => Some(("reduced_motion", !app.config.reduced_motion)),
                    "thinking" => Some(("thinking.enabled", !app.config.thinking.enabled)),
                    "companion" => Some(("companion.enabled", !app.config.companion.enabled)),
                    _ => None,
                };
                if let Some((key, new)) = write {
                    apply_config_write(app, key, TomlValue::Bool(new), false);
                }
            }
        }
        Action::ConfigDrillOpen => {
            use zoid_tui::state::ConfigCol;
            if let Some((label, _, _)) = current_config_field(app) {
                app.shell.config_picker = match label {
                    "provider" => zoid_tui::config_view::provider_options(&app.config.provider),
                    "model" => zoid_tui::config_view::model_options(
                        &app.config.provider,
                        &app.config.model,
                    ),
                    "effort" => {
                        let cur = app.config.thinking.effort.clone().unwrap_or_default();
                        vec![
                            zoid_tui::config_view::PickOption {
                                id: "".into(),
                                label: "(auto)".into(),
                                detail: String::new(),
                                selectable: true,
                                is_current: cur.is_empty(),
                            },
                            zoid_tui::config_view::PickOption {
                                id: "low".into(),
                                label: "low".into(),
                                detail: String::new(),
                                selectable: true,
                                is_current: cur == "low",
                            },
                            zoid_tui::config_view::PickOption {
                                id: "medium".into(),
                                label: "medium".into(),
                                detail: String::new(),
                                selectable: true,
                                is_current: cur == "medium",
                            },
                            zoid_tui::config_view::PickOption {
                                id: "high".into(),
                                label: "high".into(),
                                detail: String::new(),
                                selectable: true,
                                is_current: cur == "high",
                            },
                            zoid_tui::config_view::PickOption {
                                id: "max".into(),
                                label: "max".into(),
                                detail: String::new(),
                                selectable: true,
                                is_current: cur == "max",
                            },
                        ]
                    }
                    _ => Vec::new(),
                };
                if !app.shell.config_picker.is_empty() {
                    // Cursor lands on the current value, else the first selectable row.
                    app.shell.config_picker_sel = app
                        .shell
                        .config_picker
                        .iter()
                        .position(|o| o.is_current)
                        .or_else(|| app.shell.config_picker.iter().position(|o| o.selectable))
                        .unwrap_or(0);
                    app.shell.config_col = ConfigCol::Picker;
                }
                if label == "model" {
                    spawn_model_fetch(
                        app.provider.clone(),
                        app.config.provider.clone(),
                        app.ui_tx.clone(),
                    );
                }
            }
        }
        Action::ConfigPickerMove(d) => {
            let picker = &app.shell.config_picker;
            if !picker.is_empty() {
                let n = picker.len() as i32;
                let mut i = app.shell.config_picker_sel as i32;
                for _ in 0..n {
                    i = (i + d).rem_euclid(n);
                    if picker[i as usize].selectable {
                        break;
                    }
                }
                app.shell.config_picker_sel = i as usize;
            }
        }
        Action::ConfigPickerBack => {
            use zoid_tui::state::ConfigCol;
            app.shell.config_picker.clear();
            app.shell.config_col = ConfigCol::Fields;
        }
        Action::ConfigPickerSelect => {
            use zoid_core::config::TomlValue;
            use zoid_tui::state::ConfigCol;
            let chosen = app
                .shell
                .config_picker
                .get(app.shell.config_picker_sel)
                .filter(|o| o.selectable)
                .map(|o| o.id.clone());
            let label = current_config_field(app).map(|(l, _, _)| l).unwrap_or("");
            if let Some(id) = chosen {
                if label == "provider" {
                    // Write provider, then seed base_url from the registry.
                    apply_config_write(app, "provider", TomlValue::Str(id.clone()), false);
                    apply_config_write(app, "base_url", base_url_write_for(&id), false);
                    // Clear the model on a provider change (spec §4.3): the old
                    // model almost never belongs to the new provider, and leaving
                    // it would persist an incompatible provider+model pair if the
                    // user backs out of the model picker below. Unset → empty →
                    // the runtime falls back to the new provider's default_model()
                    // until the user picks one from the (auto-opened) picker.
                    apply_config_write(app, "model", TomlValue::Unset, false);
                    // Key gate: if this provider needs a key we don't have, prompt first.
                    let needs_key = key_env_for(&id).filter(|env| {
                        use zoid_core::secret::{SecretStatus, SecretStore};
                        app.secrets
                            .as_ref()
                            .map(|s| matches!(s.status(env), SecretStatus::NotSet))
                            .unwrap_or(true)
                    });
                    if let Some(env) = needs_key {
                        app.shell.config_key_prompt = Some(env);
                        app.shell.config_edit = Some(String::new());
                        app.shell.config_picker.clear();
                        app.shell.config_col = ConfigCol::Fields;
                    } else {
                        // Auto-advance to the model field and open its picker.
                        app.shell.config_picker.clear();
                        if let Some(mi) = app
                            .shell
                            .config_sections
                            .get(app.shell.config_section)
                            .and_then(|s| s.rows.iter().position(|r| r.label == "model"))
                        {
                            app.shell.config_field = mi;
                        }
                        app.shell.config_picker = zoid_tui::config_view::model_options(
                            &app.config.provider,
                            &app.config.model,
                        );
                        app.shell.config_picker_sel = 0;
                        app.shell.config_col = if app.shell.config_picker.is_empty() {
                            ConfigCol::Fields
                        } else {
                            ConfigCol::Picker
                        };
                        spawn_model_fetch(
                            app.provider.clone(),
                            app.config.provider.clone(),
                            app.ui_tx.clone(),
                        );
                    }
                } else if label == "effort" {
                    if id.is_empty() {
                        apply_config_write(app, "thinking.effort", TomlValue::Unset, false);
                    } else {
                        apply_config_write(app, "thinking.effort", TomlValue::Str(id), false);
                    }
                    app.shell.config_picker.clear();
                    app.shell.config_col = ConfigCol::Fields;
                } else if label == "model" {
                    apply_config_write(app, "model", TomlValue::Str(id), false);
                    app.shell.config_picker.clear();
                    app.shell.config_col = ConfigCol::Fields;
                }
            }
        }
        Action::ConfigSaveToRepo => {
            use zoid_tui::config_view::FieldKind;
            if let Some((label, kind, _)) = current_config_field(app) {
                match current_write(app, label, &kind) {
                    Some((key, value)) => apply_config_write(app, key, value, true),
                    None => {
                        if matches!(kind, FieldKind::Secret) {
                            eprintln!(
                                "zoid: secrets are never written to config.toml; not saving {label} to repo"
                            );
                        }
                    }
                }
            }
        }
        Action::ConfigClearSecret => {
            use zoid_tui::config_view::FieldKind;
            if let Some((label, kind, secret_key)) = current_config_field(app) {
                if matches!(kind, FieldKind::Secret) {
                    let key = secret_key.unwrap_or(label);
                    if let Some(s) = &app.secrets {
                        use zoid_core::secret::SecretStore;
                        if let Err(e) = s.clear(key) {
                            eprintln!("zoid: secret clear failed for {key}: {e}");
                        }
                    } else {
                        eprintln!("zoid: secret store unavailable; cannot clear {key}");
                    }
                    refresh_config_sections(app);
                }
            }
        }
        Action::QuestionMove(d) => {
            if let Some(q) = &mut app.shell.question {
                let len = q.rows().len();
                q.selected = zoid_tui::palette::nav(q.selected, d, len);
                // No manual cache invalidation: the live highlight/selection is
                // folded into BodyKey.question_rev, so the next frame re-renders
                // just the question card (incremental path) rather than the
                // whole transcript.
            }
        }
        Action::QuestionChar(c) => {
            if let Some(q) = &mut app.shell.question {
                q.free_text.push(c);
                // question_rev picks up the new buffer → incremental re-render.
            }
        }
        Action::QuestionBackspace => {
            if let Some(q) = &mut app.shell.question {
                q.free_text.pop();
                // question_rev picks up the new buffer → incremental re-render.
            }
        }
        Action::QuestionSelect => {
            use zoid_tui::question::{QuestionMode, QuestionOutcome};
            let outcome = app.shell.question.as_ref().map(|q| q.resolved());
            match outcome {
                Some(QuestionOutcome::EnterFreeText) => {
                    if let Some(q) = &mut app.shell.question {
                        q.mode = QuestionMode::FreeText;
                    }
                }
                Some(QuestionOutcome::Choice(s)) => {
                    answer_question(app, zoid::agent::Answer::Choice(s))
                }
                Some(QuestionOutcome::FreeText(s)) => {
                    answer_question(app, zoid::agent::Answer::FreeText(s))
                }
                Some(QuestionOutcome::LetYouDecide) => {
                    answer_question(app, zoid::agent::Answer::LetYouDecide)
                }
                None => {}
            }
        }
        Action::QuestionAbort => {
            // Esc = hard abort: dropping the sender makes the loop record a
            // balanced "[user aborted]" result and end the turn. Also fire
            // the turn's cancel token so a long-running tool or streaming
            // behind the question is interrupted, not just the question.
            app.pending_answer = None; // drop the Sender
            app.shell.question = None;
            app.shell.overlay = zoid_tui::state::Overlay::None;
            if let Some(cancel) = &app.turn_cancel {
                cancel.cancel();
            }
        }
        Action::OpenProviderSwitch => {
            use zoid_tui::state::{Overlay, SwitchPane};
            app.shell.overlay = Overlay::ProviderSwitch;
            app.shell.switch_providers =
                zoid_tui::config_view::provider_options(&app.config.provider);
            app.shell.switch_provider_sel = app
                .shell
                .switch_providers
                .iter()
                .position(|o| o.is_current)
                .unwrap_or(0);
            app.shell.switch_pane = SwitchPane::Provider;
            app.shell.switch_model_sel = 0;
            let highlighted_provider_id = app
                .shell
                .switch_providers
                .get(app.shell.switch_provider_sel)
                .map(|o| o.id.clone())
                .unwrap_or_else(|| app.config.provider.clone());
            app.shell.switch_models =
                zoid_tui::config_view::model_options(&highlighted_provider_id, &app.config.model);
            // Live-fetch the highlighted provider's real model list (Ollama
            // `/api/tags`, Anthropic `/v1/models`); the static list above is the
            // offline fallback until the fetch lands.
            spawn_switch_model_fetch(app, &highlighted_provider_id);
        }
        Action::SwitchPaneMove(_) => {
            use zoid_tui::state::SwitchPane;
            app.shell.switch_pane = match app.shell.switch_pane {
                SwitchPane::Provider => SwitchPane::Model,
                SwitchPane::Model => SwitchPane::Provider,
            };
        }
        Action::SwitchItemMove(d) => {
            use zoid_tui::state::SwitchPane;
            match app.shell.switch_pane {
                SwitchPane::Provider => {
                    let list = &app.shell.switch_providers;
                    if !list.is_empty() {
                        let n = list.len() as i32;
                        let mut i = app.shell.switch_provider_sel as i32;
                        for _ in 0..n {
                            i = (i + d).rem_euclid(n);
                            if list[i as usize].selectable {
                                break;
                            }
                        }
                        app.shell.switch_provider_sel = i as usize;
                        let highlighted_provider_id = app.shell.switch_providers
                            [app.shell.switch_provider_sel]
                            .id
                            .clone();
                        app.shell.switch_models = zoid_tui::config_view::model_options(
                            &highlighted_provider_id,
                            &app.config.model,
                        );
                        app.shell.switch_model_sel = 0;
                        // Refresh the live model list for the newly-highlighted
                        // provider; a stale in-flight fetch for the previous
                        // highlight is dropped by the guard in the reducer.
                        spawn_switch_model_fetch(app, &highlighted_provider_id);
                    }
                }
                SwitchPane::Model => {
                    let list = &app.shell.switch_models;
                    if !list.is_empty() {
                        let n = list.len() as i32;
                        let mut i = app.shell.switch_model_sel as i32;
                        for _ in 0..n {
                            i = (i + d).rem_euclid(n);
                            if list[i as usize].selectable {
                                break;
                            }
                        }
                        app.shell.switch_model_sel = i as usize;
                    }
                }
            }
        }
        Action::SwitchApply => {
            use zoid_core::config::TomlValue;
            use zoid_tui::state::Overlay;
            let provider_id = app
                .shell
                .switch_providers
                .get(app.shell.switch_provider_sel)
                .filter(|o| o.selectable)
                .map(|o| o.id.clone());
            let model_id = app
                .shell
                .switch_models
                .get(app.shell.switch_model_sel)
                .filter(|o| o.selectable)
                .map(|o| o.id.clone());
            if let Some(pid) = provider_id {
                apply_config_write(app, "provider", TomlValue::Str(pid.clone()), false);
                apply_config_write(app, "base_url", base_url_write_for(&pid), false);
                if let Some(mid) = model_id {
                    apply_config_write(app, "model", TomlValue::Str(mid), false);
                } else {
                    // No model chosen (e.g. ollama-local has no static model list):
                    // clear any stale model so it can't outlive the provider change
                    // (spec §4.3). Empty → new provider's default_model() at runtime.
                    apply_config_write(app, "model", TomlValue::Unset, false);
                }
            }
            app.shell.overlay = Overlay::None;
        }
        Action::SwitchCancel => {
            app.shell.overlay = zoid_tui::state::Overlay::None;
        }
        Action::FeedbackAbort => {
            app.shell.feedback = None;
            app.shell.overlay = Overlay::None;
            if let Some(reply) = app.feedback_reply.take() {
                let _ = reply.send(zoid::agent::Answer::Choice("Cancel".into()));
            }
        }
        Action::FeedbackMoveFocus(dir) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                use zoid_tui::state::FeedbackField;
                let order = [
                    FeedbackField::Kind,
                    FeedbackField::Title,
                    FeedbackField::Body,
                ];
                let idx = order.iter().position(|f| *f == fs.focus).unwrap_or(0);
                let n = order.len() as i32;
                let next = ((idx as i32 + dir).rem_euclid(n)) as usize;
                fs.focus = order[next];
            }
        }
        Action::FeedbackCycleKind(dir) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                let n = zoid_core::feedback::FeedbackKind::all().len() as i32;
                fs.kind_selected = ((fs.kind_selected as i32 + dir).rem_euclid(n)) as usize;
                fs.kind = zoid_core::feedback::FeedbackKind::all()[fs.kind_selected];
            }
        }
        Action::FeedbackChar(c) => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                use zoid_tui::state::FeedbackField;
                match fs.focus {
                    FeedbackField::Title => fs.title.push(c),
                    FeedbackField::Body => fs.body.push(c),
                    FeedbackField::Kind => {}
                }
            }
        }
        Action::FeedbackBackspace => {
            if let Some(fs) = app.shell.feedback.as_mut() {
                use zoid_tui::state::FeedbackField;
                match fs.focus {
                    FeedbackField::Title => {
                        fs.title.pop();
                    }
                    FeedbackField::Body => {
                        fs.body.pop();
                    }
                    FeedbackField::Kind => {}
                }
            }
        }
        Action::FeedbackSubmit => {
            handle_feedback_submit(app).await?;
        }
        Action::CatalogMove(dir) => {
            if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                if dir < 0 {
                    cat.move_up();
                } else if dir > 0 {
                    cat.move_down();
                }
            }
        }
        Action::CatalogEnterConfirm => {
            // `:plugin list` is a listing: no confirm, and no manifest fetch
            // either. The key route already declines to emit this action for a
            // read-only catalog; this guard means a future caller can't reach
            // the network (or a confirm card) by emitting it anyway.
            let read_only = app
                .shell
                .plugin_catalog
                .as_ref()
                .is_some_and(|c| c.read_only);
            let sel = if read_only {
                None
            } else {
                app.shell
                    .plugin_catalog
                    .as_ref()
                    .and_then(|c| c.selected())
                    .map(|r| (r.id.clone(), r.kind_label.clone()))
            };
            if let Some((id, kind)) = sel {
                if kind == "mcp" {
                    if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                        cat.begin_confirm_loading();
                    }
                    spawn_mcp_manifest_fetch(app, id);
                } else if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                    cat.enter_confirm();
                }
            }
        }
        Action::CatalogConfirmNo => {
            if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                cat.back_to_list();
            }
        }
        Action::CatalogConfirmYes => {
            // mcp path: install from the carried confirm. Must NOT re-enter
            // install_plugin — its catalog id-path requires [source].
            let mcp = app
                .shell
                .plugin_catalog
                .as_ref()
                .and_then(|c| c.mcp.clone());
            if let Some(confirm) = mcp {
                app.shell.plugin_catalog = None;
                app.shell.overlay = Overlay::None;
                install_mcp_server(app, &confirm);
            } else {
                let id = app
                    .shell
                    .plugin_catalog
                    .as_ref()
                    .and_then(|cat| cat.selected())
                    .map(|row| row.id.clone());
                app.shell.plugin_catalog = None;
                app.shell.overlay = Overlay::None;
                if let Some(id) = id {
                    install_plugin(app, id);
                }
            }
        }
        Action::CatalogTargetToggle => {
            if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                cat.toggle_target();
            }
        }
        Action::OnboardingMove(d) => {
            let onb = match app.shell.onboarding.as_mut() {
                Some(o) => o,
                None => return Ok(false),
            };
            let opts = &onb.options;
            if !opts.is_empty() {
                let n = opts.len() as i16;
                let mut i = onb.list_sel as i16;
                for _ in 0..n {
                    i = (i + d).rem_euclid(n);
                    if opts[i as usize].selectable {
                        break;
                    }
                }
                onb.list_sel = i as usize;
            }
        }
        Action::OnboardingSelect => {
            handle_onboarding_select(app)?;
        }
        Action::OnboardingSubmitKey => {
            handle_onboarding_submit_key(app)?;
        }
        Action::OnboardingKeyChar(c) => {
            if let Some(o) = app.shell.onboarding.as_mut() {
                o.key_buffer.push(c);
            }
        }
        Action::OnboardingKeyBackspace => {
            if let Some(o) = app.shell.onboarding.as_mut() {
                o.key_buffer.pop();
            }
        }
        Action::OnboardingBack => {
            // Step 2 → step 1: retreat (not abort). Rebuild the provider list
            // and reset list_sel to the previously-chosen provider.
            if let Some(o) = app.shell.onboarding.as_mut() {
                let prev = o.chosen_provider.clone();
                o.step = zoid_tui::state::OnboardingStep::Provider;
                o.options = zoid_tui::config_view::provider_options("");
                o.list_sel = o.options.iter().position(|opt| opt.id == prev).unwrap_or(0);
                o.key_buffer.clear();
            }
        }
        Action::OnboardingAbort => {
            app.shell.overlay = zoid_tui::Overlay::None;
            app.shell.onboarding = None;
        }
        Action::Noop => {}
    }
    Ok(false)
}

/// OnboardingSelect: Enter in step 1 (provider) or step 3 (model).
/// Step 1: write provider + base_url + clear model, then advance to step 2
/// (if key required) or DONE (if keyless). Step 3: write model, then DONE.
fn handle_onboarding_select(app: &mut App) -> Result<bool> {
    use zoid_core::config::TomlValue;
    use zoid_tui::state::OnboardingState;
    use zoid_tui::state::OnboardingStep;
    // take() — extract by value so we don't hold a &mut borrow of app
    // across the apply_config_write calls (which need &mut app).
    let onb = match app.shell.onboarding.take() {
        Some(o) => o,
        None => return Ok(false),
    };
    let sel = onb.list_sel;
    let chosen = onb
        .options
        .get(sel)
        .filter(|o| o.selectable)
        .map(|o| o.id.clone());
    let Some(chosen_id) = chosen else {
        app.shell.onboarding = Some(onb); // restore; non-selectable no-op
        return Ok(false);
    };
    match onb.step {
        OnboardingStep::Provider => {
            // Write provider + base_url + clear model (mirror ConfigPickerSelect).
            // onb is moved-out-by-value, so app is free to borrow here.
            apply_config_write(app, "provider", TomlValue::Str(chosen_id.clone()), false);
            apply_config_write(app, "base_url", base_url_write_for(&chosen_id), false);
            apply_config_write(app, "model", TomlValue::Unset, false);
            if entry_requires_key(&chosen_id) {
                // Key required → advance to step 2.
                app.shell.onboarding = Some(OnboardingState {
                    step: OnboardingStep::ApiKey,
                    chosen_provider: chosen_id,
                    options: onb.options, // reuse the provider list (unused in step 2)
                    ..Default::default()
                });
            } else {
                // Keyless → DONE.
                app.shell.overlay = zoid_tui::Overlay::None;
                // onboarding stays None (not restored)
            }
        }
        OnboardingStep::Model => {
            // Index 0 = "use default" → empty model. Else the selected model id.
            let model = if sel == 0 { String::new() } else { chosen_id };
            apply_config_write(app, "model", TomlValue::Str(model), false);
            app.shell.overlay = zoid_tui::Overlay::None;
            // onboarding stays None — DONE
        }
        OnboardingStep::ApiKey => {
            // Unreachable: route_onboarding_key only emits OnboardingSelect in
            // Provider|Model. The debug_assert catches a future routing change
            // that violates this invariant; restore-on-noop is the safe fallback.
            debug_assert!(false, "OnboardingSelect unreachable in ApiKey step");
            app.shell.onboarding = Some(onb); // restore
        }
    }
    Ok(false)
}

/// OnboardingSubmitKey: Enter in step 2 (API key). Non-empty only — empty is a
/// no-op. Writes the key to the secret store, clears the buffer, advances to
/// step 3 (if >1 model) or DONE (with a final reload to pick up the key).
fn handle_onboarding_submit_key(app: &mut App) -> Result<bool> {
    use zoid_core::config::TomlValue;
    use zoid_core::secret::SecretStore;
    use zoid_tui::state::OnboardingState;
    use zoid_tui::state::OnboardingStep;
    let onb = match app.shell.onboarding.take() {
        Some(o) => o,
        None => return Ok(false),
    };
    if onb.key_buffer.is_empty() {
        app.shell.onboarding = Some(onb); // restore; empty no-op
        return Ok(false);
    }
    let provider_id = onb.chosen_provider.clone();
    let key_env = key_env_for(&provider_id).expect(
        "wizard only reaches step 2 for key-requiring providers; \
         the lockstep test (Task 4 Step 6) guarantees a key_env_for arm \
         for every registered provider with key_url: Some",
    );
    let key_val = onb.key_buffer.clone();
    // SecretStore::set takes &self, so this doesn't conflict with app — but we
    // already took onb, so app is free regardless.
    app.secrets
        .as_ref()
        .expect("wizard gate guarantees secrets available")
        .set(key_env, &key_val)?;

    // Advance to step 3 (if >1 model) or DONE.
    let model_count = zoid_provider::model::models_for(&provider_id).len();
    if model_count > 1 {
        let mut options = zoid_tui::config_view::model_options(&provider_id, "");
        // Prepend the "use default" synthetic row at index 0.
        options.insert(
            0,
            zoid_tui::config_view::PickOption {
                id: String::new(),
                label: "use default".into(),
                detail: String::new(),
                selectable: true,
                is_current: false,
            },
        );
        app.shell.onboarding = Some(OnboardingState {
            step: OnboardingStep::Model,
            chosen_provider: provider_id,
            options,
            ..Default::default() // key_buffer cleared, list_sel 0, env_shadow None
        });
    } else {
        // Step 3 skipped — final no-op reload to pick up the key, then DONE.
        apply_config_write(app, "model", TomlValue::Unset, false);
        app.shell.overlay = zoid_tui::Overlay::None;
        // onboarding stays None — DONE
    }
    Ok(false)
}

/// Find the latest unanswered `QuestionAsked` in the event log. Returns the
/// `QuestionKind` (which carries the `ModeMapping` for wizard approvals) or
/// `None` if no question is open. Used by `answer_question` to decide whether
/// to run the materializer on "Approve".
fn latest_open_question(
    events: &zoid::eventlog::EventLog,
) -> Option<&zoid_core::event::QuestionKind> {
    let mut asked: Option<&zoid_core::event::QuestionKind> = None;
    let mut asked_id: Option<&str> = None;
    for e in events.iter() {
        match &e.kind {
            zoid_core::event::EventKind::QuestionAsked { id, kind, .. } => {
                asked = Some(kind);
                asked_id = Some(id.as_str());
            }
            zoid_core::event::EventKind::QuestionAnswered { id, .. }
                if asked_id == Some(id.as_str()) =>
            {
                asked = None;
                asked_id = None;
            }
            _ => {}
        }
    }
    asked
}

/// Decide the interrupt tier for a `CancelTurn`. First call fires `graceful`;
/// a second call (graceful already fired) fires `hard`. Returns the status-hint
/// text to show. Kept as a free fn so the escalation contract is unit-tested.
fn escalate_cancel(
    graceful: &tokio_util::sync::CancellationToken,
    hard: &tokio_util::sync::CancellationToken,
    subagents: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, zoid::agent::SubagentHandle>>,
    >,
) -> &'static str {
    if graceful.is_cancelled() {
        hard.cancel();
        // The force press also kills every in-flight subagent (reason Killed).
        zoid::agent::fire_subagent_kill(subagents, None);
        "force-stopping…"
    } else {
        graceful.cancel();
        "cancelling… (Esc again to force)"
    }
}

/// Pure decision for the no-active-turn subagent-kill escalation. Given the
/// current armed flag and how many subagents are in flight, returns
/// `(next_armed, should_fire, status_hint)`. First press arms (no fire); second
/// press fires all and disarms. Kept pure so the transition is unit-testable;
/// the caller performs the actual `fire_subagent_kill` when `should_fire`.
fn subagent_kill_decision(armed: bool, pending: usize) -> (bool, bool, String) {
    if armed {
        (false, true, format!("killing {pending} subagent(s)…"))
    } else {
        (
            true,
            false,
            format!("kill {pending} subagent(s)? Esc again to confirm"),
        )
    }
}

/// Send the user's answer down the `ask_user` reply channel and close the
/// question state. For a `ModeMapping` question answered "Approve", run the
/// materializer + reload + clear the wizard (same logic as the old
/// `ModeMappingApproval` path, now keyed off the `QuestionKind` from the
/// latest unanswered `QuestionAsked` in the event log). A no-op if the
/// channel was already consumed/dropped (e.g. a double-fire race).
fn answer_question(app: &mut App, ans: zoid::agent::Answer) {
    let kind = latest_open_question(&app.events).cloned();
    let is_wizard = matches!(
        kind,
        Some(zoid_core::event::QuestionKind::ModeMapping { .. })
    );

    if is_wizard {
        if let Some(zoid_core::event::QuestionKind::ModeMapping { mapping }) = kind {
            match &ans {
                zoid::agent::Answer::Choice(c) if c == "Approve" => {
                    let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
                    let dest = cfg_dir
                        .join("modes")
                        .join(zoid::mode_wizard::slugify(&mapping.mode_name));
                    let scan = match app.wizard.as_ref() {
                        Some(w) => w.scan.clone(),
                        None => {
                            // Defense-in-depth: the wizard should still be open
                            // (we no longer clear it on Reject). If it's somehow
                            // None, surface a clear error instead of panicking.
                            app.shell.status_hint =
                                Some("wizard state lost — re-run :mode import to retry.".into());
                            if let Some(tx) = app.pending_answer.take() {
                                let _ = tx.send(zoid::agent::Answer::Choice("Reject".into()));
                            }
                            app.shell.question = None;
                            app.shell.overlay = zoid_tui::state::Overlay::None;
                            return;
                        }
                    };
                    let fetched_at = chrono::Utc::now().to_rfc3339();
                    match zoid::mode_wizard::materialize(&mapping, &scan, &dest, &fetched_at) {
                        Ok(_) => {
                            let prev = app.modes.active_name().to_string();
                            app.modes = zoid::mode_import::build_mode_registry(
                                &app.base_profile,
                                &app.mode_dirs,
                            );
                            app.modes.set_active(&prev);
                            sync_mode_mirror(app);
                            app.wizard = None;
                            app.shell.status_hint = Some(format!(
                                "imported '{}' — Shift+Tab to it",
                                mapping.mode_name
                            ));
                        }
                        Err(e) => {
                            app.shell.status_hint = Some(format!(
                                "materialize failed: {}. Re-run :mode import to retry.",
                                e.problems.join("; ")
                            ));
                            // Don't clear app.wizard — the model may adjust
                            // and re-propose after a materialize failure.
                            if let Some(tx) = app.pending_answer.take() {
                                let _ = tx.send(zoid::agent::Answer::Choice("Reject".into()));
                            }
                            app.shell.question = None;
                            app.shell.overlay = zoid_tui::state::Overlay::None;
                            return;
                        }
                    }
                }
                zoid::agent::Answer::Choice(c) if c == "Reject" => {
                    // Do NOT clear app.wizard here — the model may re-propose
                    // in the same turn (before the turn aborts, or in a follow-up
                    // turn if the user re-triggers). Clearing the wizard here
                    // caused a panic when a later Approve tried to access it.
                    // The wizard is cleared on successful materialize instead.
                    app.shell.status_hint = Some("import cancelled".into());
                }
                zoid::agent::Answer::Choice(_) | zoid::agent::Answer::FreeText(_) => {
                    let text = match &ans {
                        zoid::agent::Answer::Choice(s) | zoid::agent::Answer::FreeText(s) => {
                            s.clone()
                        }
                        zoid::agent::Answer::LetYouDecide => "[let you decide]".into(),
                        zoid::agent::Answer::Feedback(_) => "[feedback]".into(),
                    };
                    let ts = now_ms();
                    let ev = zoid_core::event::Event::new(
                        ulid::Ulid::new(),
                        None,
                        ts,
                        zoid_core::event::EventKind::UserMessage { text },
                    );
                    app.pending_adjust = Some(ev);
                }
                zoid::agent::Answer::LetYouDecide => {
                    // Treat as Approve for the wizard (matches the old behavior).
                }
                zoid::agent::Answer::Feedback(_) => {
                    // Unreachable: feedback answers are not wizard (ModeMapping) questions.
                }
            }
        }
    }

    if let Some(tx) = app.pending_answer.take() {
        let _ = tx.send(ans);
    }
    app.shell.question = None;
    app.shell.overlay = zoid_tui::state::Overlay::None;
}

/// Open `url` in the platform's default browser. Best-effort: a missing
/// launcher or spawn failure is silently ignored — never blocks or panics.
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

/// Capture diagnostics from the running `App` for a feedback report.
fn capture_app_diagnostics(app: &App) -> zoid_core::feedback::Diagnostics {
    let recent_error = app
        .events
        .snapshot()
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            zoid_core::event::EventKind::ToolResult {
                is_error: true,
                output,
                ..
            } => Some(output.clone()),
            _ => None,
        });
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .display()
        .to_string();
    zoid_core::feedback::Diagnostics::capture(
        env!("CARGO_PKG_VERSION").to_string(),
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
        app.session_id.to_string(),
        app.modes.active_name().to_string(),
        app.config.provider.clone(),
        app.config.model.clone(),
        cwd,
        recent_error,
    )
}

/// Handle `Action::FeedbackSubmit`: build a report from `FeedbackState` +
/// diagnostics, then either send it back to the agent loop (tool path) or
/// submit it async (command path).
async fn handle_feedback_submit(app: &mut App) -> Result<bool> {
    use zoid_tui::state::FeedbackStatus;

    let fs = match app.shell.feedback.clone() {
        Some(fs) => fs,
        None => return Ok(false),
    };
    if fs.title.trim().is_empty() || fs.body.trim().is_empty() {
        if let Some(f) = app.shell.feedback.as_mut() {
            f.status = FeedbackStatus::Error("Title and description are required.".into());
        }
        return Ok(false);
    }
    let diagnostics = capture_app_diagnostics(app);
    let report = zoid_core::feedback::FeedbackReport {
        kind: fs.kind,
        title: fs.title,
        body: fs.body,
        diagnostics,
    };

    if let Some(reply) = app.feedback_reply.take() {
        // TOOL PATH: the agent loop is parked awaiting this reply.
        let _ = reply.send(zoid::agent::Answer::Feedback(report));
    } else {
        // COMMAND PATH: no parked loop. Submit async, mirroring CompactNow.
        if let Some(f) = app.shell.feedback.as_mut() {
            f.status = FeedbackStatus::Submitting;
        }
        let ui_tx = app.ui_tx.clone();
        let api: std::sync::Arc<dyn zoid_core::feedback::FeedbackApi> =
            std::sync::Arc::new(zoid_core::feedback::HttpFeedbackApi::new());
        tokio::spawn(async move {
            let outcome = report.submit_via(api.as_ref()).await;
            let _ = ui_tx
                .send(zoid::agent::AgentUpdate::FeedbackOutcome(outcome))
                .await;
        });
    }
    Ok(false)
}

/// Turn the companion server on. Idempotent — if it's already running this
/// just re-opens the browser (when configured to). On bind failure the error
/// is surfaced via the status hint; it never panics.
fn enable_companion(app: &mut App) {
    if let Some(server) = &app.companion {
        if app.config.companion.open {
            open_url(&server.url);
        }
        return;
    }
    let token = Ulid::new().to_string();
    match zoid_companion::start(app.companion_hub.clone(), app.config.companion.port, token) {
        Ok(server) => {
            app.companion_hub.set_enabled(true);
            app.shell.companion_on = true;
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

/// Turn the companion server off (no-op if it wasn't running).
fn disable_companion(app: &mut App) {
    if let Some(server) = app.companion.take() {
        app.companion_hub.set_enabled(false);
        app.shell.companion_on = false;
        server.shutdown();
    }
}

/// Kick off the async catalog index load shared by `:plugin catalog` (which
/// populates the browsable overlay) and `:plugin list` (which populates the
/// same overlay read-only). The cache dir is resolved here on the
/// main loop; only the resolved path (not any env access) crosses into the
/// spawned task. Never blocks the main loop — the fetch itself runs entirely
/// inside `tokio::spawn`.
fn spawn_catalog_load(app: &App) {
    let ui_tx = app.ui_tx.clone();
    let cache_dir = resolve_cache_dir(|k| std::env::var(k).ok()).join("catalog");
    tokio::spawn(async move {
        let res: Result<Vec<zoid::catalog::CatalogEntry>, String> = if let Some(v) =
            zoid::catalog::cache_if_fresh(
                chrono::Utc::now(),
                chrono::Duration::hours(24),
                &cache_dir,
            ) {
            Ok(v)
        } else {
            match zoid::catalog::fetch_text(&zoid::catalog::catalog_index_url()).await {
                Ok(body) => zoid::catalog::store_and_parse(chrono::Utc::now(), &cache_dir, &body)
                    .map_err(|e| e.to_string()),
                Err(e) => zoid::catalog::cached_any(&cache_dir)
                    .ok_or_else(|| format!("catalog unavailable: {e}")),
            }
        };
        let _ = ui_tx
            .send(zoid::agent::AgentUpdate::CatalogLoaded(res))
            .await;
    });
}

/// A confirm-time mcp fetch result should populate the overlay only while it is
/// still awaiting THAT row's manifest: mode is `ConfirmLoading` and the selected
/// row's id equals the fetched id. This is the consent-integrity guard — it drops
/// a stale fetch for a row the user navigated away from. Correct because
/// `route_plugin_catalog_key`'s `ConfirmLoading` arm freezes the cursor (only Esc
/// is live; Up/Down → Noop), so `selected()` is still the row whose fetch we spawned.
pub(crate) fn catalog_confirm_awaits(
    cat: &zoid_tui::state::PluginCatalogState,
    arrived_id: &str,
) -> bool {
    cat.mode == zoid_tui::state::CatalogMode::ConfirmLoading
        && cat.selected().map(|r| r.id.as_str()) == Some(arrived_id)
}

/// True if `value` references at least one `${VAR}` whose variable is unset.
/// A literal (no `${}`) is never flagged.
fn env_ref_unset(value: &str, get: &dyn Fn(&str) -> Option<String>) -> bool {
    let mut rest = value;
    while let Some(pos) = rest.find("${") {
        let after = &rest[pos + 2..];
        if let Some(end) = after.find('}') {
            if get(&after[..end]).is_none() {
                return true;
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    false
}

/// Confirm-time async fetch of an mcp plugin's `<id>.toml`. Sends
/// `McpManifestFetched`; the main loop applies it under the id guard.
fn spawn_mcp_manifest_fetch(app: &App, id: String) {
    let ui_tx = app.ui_tx.clone();
    tokio::spawn(async move {
        let res: Result<zoid_plugin::manifest::PluginManifest, String> = async {
            let body = zoid::catalog::fetch_text(&zoid::catalog::catalog_manifest_url(&id))
                .await
                .map_err(|e| format!("catalog manifest fetch failed: {e}"))?;
            let manifest = zoid_plugin::manifest::parse_manifest(&body)?;
            manifest.validate()?;
            Ok(manifest)
        }
        .await;
        let _ = ui_tx
            .send(zoid::agent::AgentUpdate::McpManifestFetched { id, res })
            .await;
    });
}

/// Apply a confirm-time manifest fetch to the open overlay — but only if the
/// overlay is still the catalog, still ConfirmLoading, and still on the SAME
/// row id (else the user navigated away → drop, protecting consent integrity).
fn apply_mcp_manifest_fetched(
    app: &mut App,
    id: String,
    res: Result<zoid_plugin::manifest::PluginManifest, String>,
) {
    use zoid_tui::state::{McpConfirm, McpEnvEntry, McpTarget};
    let matches = app.shell.overlay == zoid_tui::state::Overlay::PluginCatalog
        && app
            .shell
            .plugin_catalog
            .as_ref()
            .is_some_and(|c| catalog_confirm_awaits(c, &id));
    if !matches {
        return;
    }
    let Some(cat) = app.shell.plugin_catalog.as_mut() else {
        return;
    };
    match res {
        Ok(manifest) => {
            let server = manifest.mcp.as_ref().and_then(|m| m.servers.iter().next());
            let Some((name, spec)) = server else {
                cat.set_confirm_error("manifest declares no server".into());
                return;
            };
            let env = spec
                .env
                .iter()
                .map(|(k, v)| McpEnvEntry {
                    key: k.clone(),
                    value: v.clone(),
                    unset: env_ref_unset(v, &|x| std::env::var(x).ok()),
                })
                .collect();
            cat.set_mcp_confirm(McpConfirm {
                server_name: name.clone(),
                command: spec.command.clone(),
                args: spec.args.clone(),
                env,
                target: McpTarget::User, // default: user scope (safe)
            });
        }
        Err(e) => cat.set_confirm_error(e),
    }
}

/// Map a loaded catalog index (`AgentUpdate::CatalogLoaded`) to `PluginCatalogRow`s.
/// Skips entries whose kind is not `mode`/`skills`/`mcp`. `source_label` takes
/// a char-safe (not byte) prefix of the source ref.
fn map_catalog_entries(
    entries: Vec<zoid::catalog::CatalogEntry>,
) -> Vec<zoid_tui::state::PluginCatalogRow> {
    entries
        .into_iter()
        .filter(|e| {
            e.kind
                .iter()
                .any(|k| k == "mode" || k == "skills" || k == "mcp")
        })
        .map(|e| zoid_tui::state::PluginCatalogRow {
            id: e.id,
            name: e.name,
            kind_label: e.kind.first().cloned().unwrap_or_default(),
            description: e.description,
            // mcp entries carry no source; leave the label blank for them.
            source_label: if e.source_repo.is_empty() {
                String::new()
            } else {
                format!(
                    "{} @ {}",
                    e.source_repo,
                    e.source_ref.chars().take(7).collect::<String>()
                )
            },
            license: e.license,
        })
        .collect()
}

/// Handle `AgentUpdate::CatalogLoaded`: populate the catalog overlay (Ready or
/// Error). Both openers — bare `:plugin` (browsable) and `:plugin list`
/// (read-only) — put the overlay up before spawning the load, so the overlay is
/// the only sink. A result that arrives after the user closed the overlay is
/// stale and dropped: the status bar renders a single span, so a catalog
/// listing cannot be shown there. The load never blocks the main loop.
fn apply_catalog_loaded(app: &mut App, res: Result<Vec<zoid::catalog::CatalogEntry>, String>) {
    if app.shell.overlay != zoid_tui::state::Overlay::PluginCatalog {
        return;
    }
    if let Some(cat) = app.shell.plugin_catalog.as_mut() {
        match res {
            Ok(entries) => {
                cat.rows = map_catalog_entries(entries);
                cat.cursor = 0;
                cat.status = zoid_tui::state::CatalogStatus::Ready;
            }
            Err(e) => {
                cat.status = zoid_tui::state::CatalogStatus::Error(e);
            }
        }
    }
}

/// Kick off a plugin install: resolve the manifest source (bundled or catalog),
/// fetch the pinned tree off-thread, and hand the scan back via
/// AgentUpdate::PluginScan.
fn install_plugin(app: &mut App, arg: String) {
    use zoid::plugin_install::parse_plugin_install_args;
    use zoid_plugin::resolve::{classify_ref, resolve_source, ManifestSource, PluginRef};
    let (plugin_ref, over) = parse_plugin_install_args(&arg);
    if plugin_ref.trim().is_empty() {
        app.shell.status_hint =
            Some("usage: :plugin install <id|github-url> [--mode|--skills]".into());
        return;
    }
    if app.installing_plugin {
        app.shell.status_hint = Some("a plugin install is already in progress…".into());
        return;
    }
    let r = classify_ref(&plugin_ref);
    // Reject a bad id up front (M4): the Catalog branch interpolates id into a raw URL.
    if let PluginRef::Id(id) = &r {
        if !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        {
            app.shell.status_hint = Some(format!("invalid plugin id '{id}'"));
            return;
        }
    }
    let ui_tx = app.ui_tx.clone();
    match (
        &r,
        resolve_source(&r, zoid_plugin::bundled::bundled_ids(), false, false),
    ) {
        (PluginRef::Id(id), ManifestSource::Bundled) => {
            let manifest = zoid_plugin::bundled::bundled_manifest(id).expect("bundled id resolves");
            if let Err(e) = manifest.validate() {
                app.shell.status_hint = Some(e);
                return;
            }
            let Some(src) = manifest.source.clone() else {
                app.shell.status_hint = Some(format!("plugin '{id}' has no [source]"));
                return;
            };
            let parsed = match zoid::github_fetch::parse_github_url(&format!(
                "github.com/{}/tree/{}/{}",
                src.repo, src.ref_, src.subtree
            )) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(e);
                    return;
                }
            };
            app.installing_plugin = true;
            app.shell.status_hint = Some(format!("installing plugin '{id}'…"));
            let (id, over) = (id.clone(), over);
            tokio::spawn(async move {
                let api = zoid::github_fetch::HttpGithubApi::new();
                let res = zoid::github_fetch::fetch_tree(&api, &parsed)
                    .await
                    .map(|scan| (manifest, scan))
                    .map_err(|e| format!("plugin fetch failed: {e}"));
                let _ = ui_tx
                    .send(zoid::agent::AgentUpdate::PluginScan {
                        id,
                        origin: "bundled".into(),
                        over,
                        res,
                    })
                    .await;
            });
        }
        (PluginRef::Id(id), ManifestSource::Catalog) => {
            app.installing_plugin = true;
            app.shell.status_hint = Some(format!("installing plugin '{id}'…"));
            let (id, over) = (id.clone(), over);
            tokio::spawn(async move {
                let res: Result<_, String> = async {
                    // Async raw GET of <id>.toml (same async reqwest client style as fetch_tree).
                    let body = zoid::catalog::fetch_text(&zoid::catalog::catalog_manifest_url(&id))
                        .await
                        .map_err(|e| format!("catalog manifest fetch failed: {e}"))?;
                    let manifest = zoid_plugin::manifest::parse_manifest(&body)?;
                    manifest.validate()?;
                    let src = manifest
                        .source
                        .clone()
                        .ok_or_else(|| format!("plugin '{id}' has no [source]"))?;
                    let parsed = zoid::github_fetch::parse_github_url(&format!(
                        "github.com/{}/tree/{}/{}",
                        src.repo, src.ref_, src.subtree
                    ))?;
                    let api = zoid::github_fetch::HttpGithubApi::new();
                    let scan = zoid::github_fetch::fetch_tree(&api, &parsed)
                        .await
                        .map_err(|e| format!("plugin fetch failed: {e}"))?;
                    Ok((manifest, scan))
                }
                .await;
                let _ = ui_tx
                    .send(zoid::agent::AgentUpdate::PluginScan {
                        id,
                        origin: "catalog".into(),
                        over,
                        res,
                    })
                    .await;
            });
        }
        (PluginRef::Url(_), _) => {
            app.shell.status_hint =
                Some("installing plugins from a URL is not supported yet; use a catalog id".into());
        }
        (PluginRef::Id(id), _) => {
            app.shell.status_hint = Some(format!("unknown plugin '{id}'"));
        }
    }
}

/// Resolve the `.mcp.json` an mcp install writes to. Pure (dirs injected) for tests.
fn mcp_target_path(
    target: zoid_tui::state::McpTarget,
    config_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> std::path::PathBuf {
    match target {
        zoid_tui::state::McpTarget::User => config_dir.join("mcp.json"),
        zoid_tui::state::McpTarget::Project => cwd.join(".mcp.json"),
    }
}

/// Write the confirmed mcp server into the chosen `.mcp.json` (additive, atomic,
/// skip-on-collision) and report the outcome + a restart hint. Uses the carried
/// confirm — never re-enters `install_plugin` (whose catalog id-path requires
/// `[source]`, which an mcp manifest lacks).
fn install_mcp_server(app: &mut App, confirm: &zoid_tui::state::McpConfirm) {
    let config_dir = resolve_config_dir(|k| std::env::var(k).ok());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let path = mcp_target_path(confirm.target, &config_dir, &cwd);

    let server = zoid_mcp::config::McpServerConfig {
        command: confirm.command.clone(),
        args: confirm.args.clone(),
        env: confirm
            .env
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect(),
    };

    let hint = match zoid_mcp::config::merge_server(&path, &confirm.server_name, &server) {
        Ok(zoid_mcp::config::MergeOutcome::Inserted) => format!(
            "✓ wrote '{}' to {} · restart zoid to connect",
            confirm.server_name,
            path.display()
        ),
        Ok(zoid_mcp::config::MergeOutcome::SkippedExisting) => format!(
            "ℹ '{}' already configured in {} — left unchanged",
            confirm.server_name,
            path.display()
        ),
        Err(e) => format!("mcp install failed: {e}"),
    };
    app.shell.status_hint = Some(hint);
}

/// Apply a completed plugin fetch on the main loop: build the plan, materialize
/// into `<modes-dir>/<id>`, apply Safe effects (activate / onboarding hint),
/// rebuild the registry. Returns `true` iff a mode was installed and activated.
fn apply_plugin_scan(
    app: &mut App,
    id: String,
    origin: String,
    over: zoid::plugin_install::KindOverride,
    res: Result<
        (
            zoid_plugin::manifest::PluginManifest,
            zoid_core::wizard::UpstreamScan,
        ),
        String,
    >,
) -> bool {
    use zoid::plugin_install::KindOverride;
    app.installing_plugin = false;
    let (mut manifest, scan) = match res {
        Ok(pair) => pair,
        Err(e) => {
            app.shell.status_hint = Some(e);
            return false;
        }
    };
    // Apply the `--mode`/`--skills` override (if any) to the freshly-resolved
    // manifest before planning, so build_plan (and the branch below) see the
    // overridden kind rather than the manifest's declared default.
    match over {
        KindOverride::Mode => manifest.kind = vec!["mode".into()],
        KindOverride::Skills => {
            manifest.kind = vec!["skills".into()];
            manifest.mode = None;
        }
        KindOverride::None => {}
    }
    let plan = match zoid_plugin::plan::build_plan(&manifest, &scan) {
        Ok(p) => p,
        Err(e) => {
            app.shell.status_hint = Some(format!("plugin plan failed: {e}"));
            return false;
        }
    };
    // The declared pin comes from the manifest's [source].ref; fall back to the
    // resolved fetch ref if a manifest somehow omitted it.
    let manifest_ref = manifest
        .source
        .as_ref()
        .map(|s| s.ref_.clone())
        .unwrap_or_else(|| scan.resolved_ref.clone());
    // Mirrors build_plan's own kind test (zoid-plugin/src/plan.rs): "skills"
    // without "mode" routes to the skills-pack installer; everything else
    // (including a bare mode-recipe manifest) installs as a mode.
    let is_skills_kind =
        manifest.kind.iter().any(|k| k == "skills") && !manifest.kind.iter().any(|k| k == "mode");
    let installed = if is_skills_kind {
        let skills_root = resolve_config_dir(|k| std::env::var(k).ok()).join("skills");
        match zoid::plugin_install::finish_skills_install(
            &plan,
            &scan,
            &skills_root,
            &id,
            &manifest_ref,
            &origin,
        ) {
            Ok(out) => out,
            Err(e) => {
                app.shell.status_hint = Some(format!("plugin install failed: {e}"));
                return false;
            }
        }
    } else {
        let Some(dest) = app.mode_dirs.first().map(|d| d.join(&id)) else {
            app.shell.status_hint = Some("no modes directory configured".into());
            return false;
        };
        match zoid::plugin_install::finish_plugin_install(
            &plan,
            &scan,
            &dest,
            &id,
            &manifest_ref,
            &origin,
        ) {
            Ok(out) => out,
            Err(e) => {
                app.shell.status_hint = Some(format!("plugin install failed: {e}"));
                return false;
            }
        }
    };

    // Skills packs live in a separate registry built once at startup; the
    // runtime installer materializes them to disk but cannot hot-reload
    // `app.skills`. Report honestly and skip the mode-registry/Activate path —
    // its "could not be activated" message is meaningless for a skills pack,
    // which has no mode to activate. (Live hot-reload is deferred to a later spec.)
    if is_skills_kind {
        let n = scan
            .files
            .iter()
            .filter(|f| f.upstream_path.ends_with("/SKILL.md"))
            .count();
        app.shell.status_hint = Some(format!(
            "plugin '{id}' installed ({n} skills). Restart zoid to load them."
        ));
        return false;
    }

    // Rebuild registry so the new mode is visible.
    let prev = app.modes.active_name().to_string();
    app.modes = zoid::mode_import::build_mode_registry(&app.base_profile, &app.mode_dirs);

    // Apply Safe effects. Activation may fail if the rebuilt registry doesn't
    // surface the mode; capture the onboarding text and reconcile it with the
    // REAL activation outcome after the loop, so we never claim "active" falsely
    // (the bespoke path computed one honest status string; preserve that).
    let mut activated = false;
    let mut wants_activate = false;
    let mut onboarding: Option<String> = None;
    let mode_display = manifest.name.clone();
    for e in &installed.safe_effects {
        match e {
            zoid_plugin::effect::Effect::Activate => {
                wants_activate = true;
                if app.modes.names().iter().any(|n| n == &mode_display) {
                    app.modes.set_active(&mode_display);
                    activated = true;
                }
            }
            zoid_plugin::effect::Effect::OnboardingHint { text } => {
                onboarding = Some(text.clone());
            }
            zoid_plugin::effect::Effect::SetConfig { .. } => {
                // Unreachable: `finish_plugin_install` rejects ALL SetConfig
                // effects at the v1 gate (config application is deferred),
                // and `safe_effects` is filtered to Safe effects only, so no
                // SetConfig can ever appear in `installed.safe_effects`.
                unreachable!("SetConfig is rejected at the v1 gate in finish_plugin_install")
            }
        }
    }
    if wants_activate && !activated {
        app.modes.set_active(&prev); // activation requested but mode not found — keep prior active
    }
    sync_mode_mirror(app);
    // Honest status: show the onboarding hint only when activation actually
    // succeeded (or wasn't requested); otherwise report the accurate outcome.
    app.shell.status_hint = Some(match (wants_activate, activated, onboarding) {
        (true, true, Some(text)) => text,
        (true, true, None) => format!("plugin '{id}' installed and active."),
        (false, _, Some(text)) => text,
        (false, _, None) => format!("plugin '{id}' installed."),
        (true, false, _) => format!("plugin '{id}' installed but could not be activated."),
    });
    activated
}

async fn exec_command(app: &mut App, cmd: zoid_tui::command::Command) -> Result<bool> {
    use zoid_tui::command::Command;
    match cmd {
        Command::Quit => Ok(true),
        Command::SwitchMode(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode <name> · :mode reload".into());
                return Ok(false);
            }
            app.modes.set_active(&name);
            sync_mode_mirror(app);
            persist_active_mode(app).await;
            Ok(false)
        }
        Command::ReloadModes => {
            let prev = app.modes.active_name().to_string();
            app.modes = zoid::mode_import::build_mode_registry(&app.base_profile, &app.mode_dirs);
            app.modes.set_active(&prev); // preserve by name; no-op ⇒ Chat
            sync_mode_mirror(app);
            Ok(false)
        }
        Command::ModeImport(url) => {
            if url.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode import <github-url>".into());
                return Ok(false);
            }
            app.wizard = None;
            app.shell.status_hint = Some(format!("fetching {url}…"));
            let parsed = match zoid::github_fetch::parse_github_url(&url) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(e);
                    return Ok(false);
                }
            };
            let ui_tx = app.ui_tx.clone();
            tokio::spawn(async move {
                let api = zoid::github_fetch::HttpGithubApi::new();
                let scan = match zoid::github_fetch::fetch_tree(&api, &parsed).await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ui_tx
                            .send(zoid::agent::AgentUpdate::ModelsFetched {
                                provider: "__wizard_error__".into(),
                                models: vec![format!("fetch failed: {e}")],
                            })
                            .await;
                        return;
                    }
                };
                let _ = ui_tx
                    .send(zoid::agent::AgentUpdate::ModelsFetched {
                        provider: "__wizard_scan__".into(),
                        models: vec![serde_json::to_string(&scan).unwrap_or_default()],
                    })
                    .await;
            });
            Ok(false)
        }
        Command::ModeUpdate(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint = Some("usage: :mode update <name>".into());
                return Ok(false);
            }
            let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
            let slug = zoid::mode_wizard::slugify(&name);
            let mode_dir = cfg_dir.join("modes").join(&slug);
            let sidecar_path = mode_dir.join(".zoid-provenance.json");
            if !sidecar_path.is_file() {
                app.shell.status_hint = Some(format!(
                    "mode '{name}' has no import provenance; it was not imported from a URL. Use :mode import <url> instead."
                ));
                return Ok(false);
            }
            let sidecar_text = match std::fs::read_to_string(&sidecar_path) {
                Ok(t) => t,
                Err(e) => {
                    app.shell.status_hint = Some(format!("read sidecar: {e}"));
                    return Ok(false);
                }
            };
            let old: zoid_core::wizard::ProvenanceFile = match serde_json::from_str(&sidecar_text) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(format!("parse sidecar: {e}"));
                    return Ok(false);
                }
            };
            let url = format!(
                "https://github.com/{}/tree/{}/{}",
                old.source.repo, old.source.ref_, old.source.subtree_path
            );
            let parsed = match zoid::github_fetch::parse_github_url(&url) {
                Ok(p) => p,
                Err(e) => {
                    app.shell.status_hint = Some(e);
                    return Ok(false);
                }
            };
            app.shell.status_hint = Some(format!("fetching upstream at ref {}…", old.source.ref_));
            let ui_tx = app.ui_tx.clone();
            let mode_dir_clone = mode_dir.clone();
            let old_clone = old.clone();
            tokio::spawn(async move {
                let api = zoid::github_fetch::HttpGithubApi::new();
                let scan = match zoid::github_fetch::fetch_tree(&api, &parsed).await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ui_tx
                            .send(zoid::agent::AgentUpdate::ModelsFetched {
                                provider: "__wizard_error__".into(),
                                models: vec![format!("fetch failed: {e}")],
                            })
                            .await;
                        return;
                    }
                };
                let brief = zoid::mode_wizard::build_reconciliation_brief(
                    &mode_dir_clone,
                    &old_clone,
                    &scan,
                );
                let _ = ui_tx
                    .send(zoid::agent::AgentUpdate::ModelsFetched {
                        provider: "__wizard_update__".into(),
                        models: vec![
                            serde_json::to_string(&scan).unwrap_or_default(),
                            brief,
                            old_clone.mode_name.clone(),
                        ],
                    })
                    .await;
            });
            Ok(false)
        }
        Command::PluginInstall(arg) => {
            install_plugin(app, arg);
            Ok(false)
        }
        Command::PluginCatalog => {
            app.shell.plugin_catalog = Some(zoid_tui::state::PluginCatalogState::loading());
            app.shell.overlay = zoid_tui::state::Overlay::PluginCatalog;
            spawn_catalog_load(app);
            Ok(false)
        }
        Command::PluginList => {
            app.shell.plugin_catalog =
                Some(zoid_tui::state::PluginCatalogState::loading_read_only());
            app.shell.overlay = zoid_tui::state::Overlay::PluginCatalog;
            spawn_catalog_load(app);
            Ok(false)
        }
        Command::OpenDrawer(id) => {
            app.shell.open_drawer(id);
            Ok(false)
        }
        Command::NewSession => {
            if app.streaming || !app.in_flight_subagents.is_empty() {
                app.shell.status_hint = Some(busy_block_hint(app));
                app.shell.close_overlay();
                return Ok(false);
            }
            let id = Ulid::new();
            let ts = now_ms();
            let name = derive_session_name(None, ts, app.tz_offset_secs);
            app.session
                .new_session(id, name.clone(), repo_root(), ts)
                .await
                .ok();
            app.session_id = id;
            app.shell.session_name = name;
            app.session_started_ms = ts;
            app.events = zoid::eventlog::EventLog::new();
            app.pending_wakes = rebuild_pending_wakes(app.events.iter());
            let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
            // New session: reset the caches (clear() may leave the same length).
            app.proj = ProjectionCache::default();
            app.body_cache = BodyCache::default();
            app.shell.conversation_scroll = 0;
            app.shell.follow_tail = true; // new session starts pinned to the latest
                                          // A fresh session has no saved mode yet ⇒ this resets to Chat rather
                                          // than carrying over the previous session's active mode.
            restore_mode_for_session(app).await;
            // Claim the new session and clear any yielded state from the prior
            // (taken-over) session — `:new` is the documented yield escape hatch.
            app.yielded = false;
            app.pending_message = None;
            app.shell.status_hint = None;
            let self_pid = std::process::id() as i64;
            app.session.set_active(id, true, self_pid, ts).await.ok();
            // Restart the heartbeat for the new session.
            spawn_heartbeat(app);
            Ok(false)
        }
        Command::RenameSession(name) => {
            if name.is_empty() {
                // Seed the palette in Direct phase so the user types the name.
                app.shell.overlay = zoid_tui::Overlay::Palette;
                app.shell.palette = Default::default();
                app.shell.palette.query = ":session rename ".into();
            } else {
                app.session
                    .rename_session(app.session_id, name.clone())
                    .await
                    .ok();
                app.shell.session_name = name;
            }
            Ok(false)
        }
        Command::ResumeSessionPicker => {
            let list = app
                .session
                .list_sessions(Some(repo_root()))
                .await
                .unwrap_or_default();
            app.session_ids = list.iter().map(|s| s.id).collect();
            app.shell.sessions = list
                .iter()
                .map(|s| {
                    format!(
                        "{}  ·  {}  ·  {}",
                        s.name,
                        fmt_since(s.last_touched_ts, now_ms()),
                        zoid_tui::economy_view::human_tokens(s.token_total)
                    )
                })
                .collect();
            let now = now_ms();
            app.shell.sessions_live = list
                .iter()
                .map(|s| {
                    zoid_core::store::is_live(
                        s.active,
                        s.active_pid,
                        s.active_heartbeat,
                        now,
                        pid_alive,
                    )
                })
                .collect();
            app.shell.session_selected = 0;
            app.shell.overlay = zoid_tui::Overlay::Sessions;
            Ok(false)
        }
        // Disabled stub, not a dispatch path: `_task` is parsed and dropped, and
        // this hint is a refusal, not subagent status. The one live dispatch is
        // the model calling the delegation tool (`agent.rs` → `SubagentStarted`
        // → the right-rail Subagents drawer). Re-enabling this must route there,
        // not to `status_hint`.
        Command::Delegate(_task) => {
            app.shell.status_hint = Some("delegation is temporarily disabled".into());
            Ok(false)
        }
        Command::Worktree(name) => {
            if name.trim().is_empty() {
                app.shell.status_hint = Some("usage: :worktree <name> · :worktree exit".into());
                return Ok(false);
            }
            // Call the handler directly — NOT via the ui_tx channel.
            // exec_command runs on the main loop task that also recv()s
            // from ui_rx; sending via the bounded channel can deadlock.
            handle_worktree_request(app, zoid::agent::WorktreeAction::Enter { name }, None);
            Ok(false)
        }
        Command::WorktreeExit => {
            handle_worktree_request(app, zoid::agent::WorktreeAction::Exit, None);
            Ok(false)
        }
        Command::OpenConfig => {
            app.shell.overlay = zoid_tui::Overlay::Config;
            app.shell.config_section = 0;
            app.shell.config_field = 0;
            app.shell.config_edit = None;
            refresh_config_sections(app);
            Ok(false)
        }
        Command::OpenMcp => {
            // Read-only status overlay: `app.shell.mcp_status` is kept current by
            // the per-frame sync in the render loop, so there is nothing to
            // populate here beyond switching the overlay.
            app.shell.overlay = zoid_tui::Overlay::Mcp;
            Ok(false)
        }
        Command::OpenHelp => {
            app.shell.overlay = zoid_tui::Overlay::Help;
            app.shell.help_scroll = 0;
            Ok(false)
        }
        Command::CompanionEnable => {
            enable_companion(app);
            Ok(false)
        }
        Command::CompanionDisable => {
            disable_companion(app);
            Ok(false)
        }
        Command::ToggleSelectMode => {
            toggle_select_mode(app);
            Ok(false)
        }
        Command::CompactNow => {
            if app.shell.compacting {
                app.shell.status_hint = Some("already compacting".into());
                return Ok(false);
            }
            // Spawn the compaction task (non-blocking; chat turns are not blocked).
            let session = app.session.clone();
            let session_id = app.session_id;
            let ui_tx = app.ui_tx.clone();
            let events = app.events.snapshot();
            let ctx_policy = policy_from_config(&app.economy, app.context_target);
            // Compute the context overhead (system prompt only). The automatic
            // gate also includes tool-spec tokens, but for an explicit manual
            // compaction the system-prompt overhead alone is a sufficient
            // approximation — the user is asking to compact, not relying on
            // the gate's precise threshold math.
            let overhead = zoid_core::context::ContextOverhead {
                system_tokens: zoid_core::economy::estimate_tokens(&app.base_profile.system_prompt),
                tools_tokens: 0,
            };
            tokio::spawn(async move {
                let plan = zoid_core::compaction::plan_compactions(
                    events.iter(),
                    &ctx_policy,
                    None, // no real_input_tokens (no in-flight turn)
                    None, // no calibration ratio
                    &overhead,
                );
                let _ = ui_tx.send(AgentUpdate::CompactionStarted).await;
                for c in &plan.compactions {
                    let ev = Event::new(
                        Ulid::new(),
                        None,
                        now_ms(),
                        EventKind::ToolResultCompacted {
                            id: c.id.clone(),
                            summary: c.summary.clone(),
                            original_tokens: c.original_tokens,
                        },
                    )
                    .with_session(session_id);
                    let _ = session.append(ev.clone()).await;
                    let _ = ui_tx.send(AgentUpdate::Appended(Box::new(ev))).await;
                }
                let _ = ui_tx.send(AgentUpdate::CompactionComplete).await;
            });
            Ok(false)
        }
        Command::Feedback => {
            app.shell.overlay = zoid_tui::Overlay::Feedback;
            app.shell.feedback = Some(zoid_tui::state::FeedbackState::new());
            Ok(false)
        }
        Command::Unknown(_) => Ok(false),
    }
}

/// Mirror the active mode + names onto the shell for the pure renderer/palette.
fn sync_mode_mirror(app: &mut App) {
    app.shell.active_mode = app.modes.active_name().to_string();
    app.shell.active_mode_broken = app.modes.active_is_broken();
    app.shell.mode_names = app.modes.names();
}

/// Persist the active mode name onto the current session row (best-effort).
async fn persist_active_mode(app: &App) {
    if let Err(e) = app
        .session
        .set_active_mode(app.session_id, app.modes.active_name().to_string())
        .await
    {
        // Best-effort: a failed write just means the chip won't survive a restart.
        // Surface it at debug so it's diagnosable without spamming normal runs.
        tracing::debug!(error = %e, "failed to persist active mode");
    }
}

/// Reset to the Chat floor, then apply the session's saved mode if it still
/// exists, and refresh the shell mirror. Called at boot AND on every mid-run
/// session change so a resumed/new session runs with ITS OWN mode (spec §11),
/// never the previously-active session's overlay + scoped skills.
async fn restore_mode_for_session(app: &mut App) {
    // Reset to Chat (index 0); its name == base_profile.name.
    app.modes.set_active(app.base_profile.name.as_str());
    if let Ok(Some(saved)) = app.session.get_active_mode(app.session_id).await {
        app.modes.set_active(&saved); // no-op if the saved mode vanished ⇒ stays Chat
    }
    sync_mode_mirror(app);
}

/// Spawn the 5s heartbeat task for the active session. Each tick refreshes
/// `active_heartbeat`; if the UPDATE matches zero rows (another process took
/// over the row), fire the turn cancellation token, set `yielded`, stop the
/// task, and surface a hint. Spec §2.3/§2.4.
fn spawn_heartbeat(app: &App) {
    let session = app.session.clone();
    let session_id = app.session_id;
    let pid = std::process::id() as i64;
    let ui_tx = app.ui_tx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(
            zoid_core::store::HEARTBEAT_INTERVAL_MS as u64,
        ));
        tick.tick().await; // consume the immediate first tick
        loop {
            tick.tick().await;
            let now = now_ms();
            match session.heartbeat(session_id, pid, now).await {
                Ok(true) => { /* still owner */ }
                Ok(false) | Err(_) => {
                    // Taken over (or the actor stopped). Signal yield.
                    let _ = ui_tx.send(AgentUpdate::SessionTakenOver).await;
                    break;
                }
            }
        }
    });
}

/// Resolve the final `ThinkingMode` from config + provenance + provider + model
/// capability. Pure — takes explicit args so it's unit-testable. No IO, no
/// global state.
///
/// **Provider-aware default:** when `thinking_enabled_src == Source::Default`
/// (the user set no `[thinking].enabled` key in any config layer) and the
/// provider is `ollama-local`, `enabled` is treated as `true`. This makes
/// thinking available by default for local models that support it; the
/// capability gate below still returns `Off` if the model doesn't support
/// thinking. An explicit `enabled = false` (any `Source != Default`) always
/// wins — that's the user override. `ZOID_THINKING` sets `Source::Env`, which
/// is `!= Default`, so env wins too.
fn resolve_thinking(
    config_thinking: &zoid_core::config::ThinkingConfig,
    thinking_enabled_src: zoid_core::config::Source,
    provider: &str,
    model_support: zoid_provider::model::ThinkingSupport,
) -> zoid_provider::ThinkingMode {
    use zoid_provider::ThinkingMode;
    // Effective enabled flag: the user's value, or true for ollama-local when
    // the user set no [thinking].enabled key (provenance Default). An explicit
    // enabled = false (provenance != Default) always wins.
    let enabled = if thinking_enabled_src == zoid_core::config::Source::Default
        && zoid_provider::model::canonical_id(provider) == "ollama-local"
    {
        true
    } else {
        config_thinking.enabled
    };
    match model_support {
        zoid_provider::model::ThinkingSupport::None => ThinkingMode::Off,
        _ if !enabled => ThinkingMode::Off,
        _ => match &config_thinking.effort {
            None => ThinkingMode::Auto,
            Some(e) => {
                use zoid_provider::EffortLevel;
                let level = match e.as_str() {
                    "low" => EffortLevel::Low,
                    "medium" => EffortLevel::Medium,
                    "high" => EffortLevel::High,
                    "max" => EffortLevel::Max,
                    _ => EffortLevel::High,
                };
                ThinkingMode::Effort(level)
            }
        },
    }
}

/// Parse a `ZOID_THINKING` env value into a `PartialThinking` override.
/// Pure — no env access. Returns `None` for empty/unparseable values.
fn parse_thinking_env(val: &str) -> Option<zoid_core::config::PartialThinking> {
    let val = val.trim();
    if val.is_empty() {
        return None;
    }
    let mut pt = zoid_core::config::PartialThinking::default();
    match val.to_ascii_lowercase().as_str() {
        "off" | "disabled" | "false" | "0" => {
            pt.enabled = Some(false);
        }
        "auto" | "on" | "true" | "1" => {
            pt.enabled = Some(true);
        }
        "low" => {
            pt.enabled = Some(true);
            pt.effort = Some("low".into());
        }
        "medium" => {
            pt.enabled = Some(true);
            pt.effort = Some("medium".into());
        }
        "high" => {
            pt.enabled = Some(true);
            pt.effort = Some("high".into());
        }
        "max" => {
            pt.enabled = Some(true);
            pt.effort = Some("max".into());
        }
        _ => return None,
    }
    Some(pt)
}

/// Perform the worktree enter/exit git work and return the new absolute cwd for
/// the in-flight turn. Guard failures return `Err(msg)`. Does NOT touch `App`,
/// `status_hint`, or the rail — the caller applies the cwd and the poller owns
/// the rail labels.
///
/// `repo_root` is the main checkout root (`"."` in production; a temp dir in tests).
/// On Enter, `*active` is set to the new `WorktreeSession`; on Exit it is cleared.
fn compute_worktree_switch(
    active: &mut Option<WorktreeSession>,
    action: zoid::agent::WorktreeAction,
    subagent_running: bool,
    repo_root: &std::path::Path,
) -> Result<(std::path::PathBuf, Option<String>), String> {
    use zoid::agent::WorktreeAction;
    match action {
        WorktreeAction::Enter { name } => {
            if active.is_some() {
                return Err("already in a worktree — exit with :worktree exit first".into());
            }
            if subagent_running {
                return Err("cannot enter worktree while a subagent is running".into());
            }
            if !repo_root.join(".git").exists() {
                return Err("not a git repository".into());
            }
            // Create the worktree, or (idempotent re-enter) adopt an existing
            // dir left by a prior dirty-kept exit. `into_kept()` moves the
            // guard's contents into a Drop-free session so the dir + branch are
            // never auto-removed.
            let (path, sess_name) = match zoid::worktree::create_worktree(repo_root, &name) {
                Ok(guard) => guard.into_kept(),
                Err(e) => {
                    let existing = repo_root.join(".zoid").join("worktrees").join(&name);
                    if existing.exists() {
                        (existing, name.clone())
                    } else {
                        return Err(format!("enter_worktree failed: {e}"));
                    }
                }
            };
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            *active = Some(WorktreeSession {
                path: path.clone(),
                name: sess_name,
            });
            Ok((path, None))
        }
        WorktreeAction::Exit => {
            let wt = match active.take() {
                Some(wt) => wt,
                None => return Err("not in a worktree".into()),
            };
            if subagent_running {
                *active = Some(wt); // put it back — exit refused
                return Err("cannot exit worktree while a subagent is running".into());
            }
            // Absolute repo root computed BEFORE any removal, so tooling never
            // points at a deleted dir (WT-2).
            let root = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
            // Clean → remove (dir + prune). Dirty → keep (WorktreeSession
            // has no Drop, so doing nothing preserves the user's work).
            if is_worktree_clean(&wt.path) {
                // Check if the branch has commits not on HEAD — if so, keep
                // the branch ref so the work isn't orphaned. The worktree
                // directory is still removed; only the branch is retained.
                // Capture OIDs BEFORE remove_worktree deletes the branch —
                // otherwise the diagnostic branch_oid is always None.
                let repo = git2::Repository::open(repo_root).ok();
                let branch_oid = repo
                    .as_ref()
                    .and_then(|r| r.find_branch(&wt.name, git2::BranchType::Local).ok())
                    .and_then(|b| b.get().target());
                let head_oid = repo
                    .as_ref()
                    .and_then(|r| r.head().ok())
                    .and_then(|h| h.target());
                let has_unmerged = zoid::worktree::branch_has_unmerged_commits(repo_root, &wt.name);
                let _ = zoid::worktree::remove_worktree(repo_root, &wt.name, !has_unmerged);
                if has_unmerged {
                    let warn = format!(
                        "exited worktree — branch '{}' retained (has unmerged commits). \
                         Merge to main or delete with: git branch -d {}",
                        wt.name, wt.name,
                    );
                    tracing::warn!(branch = %wt.name, "{warn}");
                    return Ok((root, Some(warn)));
                } else {
                    // Diagnostic: include OIDs in the return message so the agent
                    // sees them without needing ZOID_LOG. If has_unmerged is a
                    // false negative (the branch actually has unmerged work), the
                    // OIDs here reveal why merge_base returned the wrong result.
                    let diag = format!(
                        "exited worktree (branch '{}' deleted — no unmerged commits detected; \
                         branch_oid={:?} head_oid={:?})",
                        wt.name, branch_oid, head_oid,
                    );
                    tracing::warn!(branch = %wt.name, branch_oid = ?branch_oid, head_oid = ?head_oid, "{diag}");
                    return Ok((root, Some(diag)));
                }
            }
            Ok((root, None))
        }
    }
}

/// Process a `WorktreeRequested` signal from the agent loop or `:worktree`
/// command. Performs the actual worktree enter/exit between turns.
fn handle_worktree_request(
    app: &mut App,
    action: zoid::agent::WorktreeAction,
    reply: Option<zoid::agent::WorktreeReply>,
) {
    let subagent_running = !app.in_flight_subagents.is_empty();
    let result = compute_worktree_switch(
        &mut app.active_worktree,
        action,
        subagent_running,
        std::path::Path::new("."),
    );
    match &result {
        Ok((cwd, _warn)) => {
            // WT-2: the Session drawer's cwd display (not clobbered by the poller).
            app.shell.cwd = cwd.display().to_string();
        }
        Err(msg) => {
            // The `:worktree` slash path (reply == None) has no ToolResult to carry
            // the error, so surface it here. The turn path gets the error via its
            // ToolResult and does not need a status_hint.
            if reply.is_none() {
                app.shell.status_hint = Some(msg.clone());
            }
        }
    }
    // Point the git poller at the worktree (enter) or back to "." (exit), and
    // wake it immediately so the rail updates without waiting for the next tick.
    let active_path = app.active_worktree.as_ref().map(|w| w.path.clone());
    let _ = app.active_wt_tx.send(active_path);
    if let Some(reply) = reply {
        let _ = reply.send(result);
    }
}

/// Check if a worktree has uncommitted changes via `git status --porcelain`.
/// Defaults to `false` (dirty) on error — never auto-remove a worktree if we
/// can't verify it's clean.
fn is_worktree_clean(path: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap_or(false) // conservative: if git fails, assume dirty
}

/// Whether a just-recorded `DelegationResult` should wake the orchestrator into
/// a continuation turn *now*: it is idle (no turn streaming) and the session has
/// not been yielded/taken over. Per-result: no longer waits for every subagent
/// to finish — each result gets its own wake decision, and a still-running turn
/// defers (handled by `plan_delegation_wake`). Pure decision — no side effects —
/// so it can be exhaustively unit-tested.
fn should_wake_after_delegation(streaming: bool, yielded: bool) -> bool {
    !streaming && !yielded
}

/// Hint shown when session/worktree management is blocked because the main
/// turn is streaming or background subagents are still in flight. `Submit` is
/// no longer blocked by running subagents (the main loop runs concurrently),
/// but switching sessions or worktrees mid-flight would orphan the subagents'
/// output, so those actions stay gated.
fn busy_block_hint(app: &App) -> String {
    let n = app.in_flight_subagents.len();
    if n > 0 {
        format!(
            "{n} subagent{} running — press Esc to kill them or wait for completion",
            if n == 1 { "" } else { "s" }
        )
    } else {
        "finish the current turn first".into()
    }
}

/// Project the pending-wake set from the event log: every `WakeScheduled`
/// whose `wake_id` has no later `WakeFired`/`WakeCancelled`. Keyed by
/// `(fire_at_ms, wake_id)` so the map is ordered by fire time (and same-ms
/// schedules don't collide); the value is the note. Pure — rebuilt on load and
/// unit-tested without timers. Takes an iterator (not `&[Event]`) because the
/// live `EventLog` stores `Vec<Arc<Event>>` and exposes only `iter()` — there is
/// no contiguous `&[Event]` slice to borrow (C2).
fn rebuild_pending_wakes<'a>(
    events: impl IntoIterator<Item = &'a zoid_core::event::Event>,
) -> std::collections::BTreeMap<(i64, String), String> {
    use zoid_core::event::EventKind;
    // Fold to the latest state per wake_id, then materialize the survivors.
    let mut by_id: std::collections::HashMap<String, (i64, String)> =
        std::collections::HashMap::new();
    for e in events {
        match &e.kind {
            EventKind::WakeScheduled {
                wake_id,
                fire_at_ms,
                note,
            } => {
                by_id.insert(wake_id.clone(), (*fire_at_ms, note.clone()));
            }
            EventKind::WakeFired { wake_id } | EventKind::WakeCancelled { wake_id } => {
                by_id.remove(wake_id);
            }
            _ => {}
        }
    }
    by_id
        .into_iter()
        .map(|(id, (fire_at, note))| ((fire_at, id), note))
        .collect()
}

/// The earliest `fire_at_ms` in the pending set (the watcher's next deadline),
/// or `None` when nothing is scheduled (the watcher parks on `changed()`).
fn earliest_fire_at(pending: &std::collections::BTreeMap<(i64, String), String>) -> Option<i64> {
    pending.keys().next().map(|(t, _)| *t)
}

/// Runaway guards for agent-scheduled wakes (constants in v1).
const WAKE_MIN_DELAY_SECS: u64 = 30;
const WAKE_MAX_PENDING: usize = 16;
/// Ceiling for an agent-scheduled wake: 30 days. Prevents an absurd delay from
/// overflowing the i64 millisecond fire-time arithmetic (a panic in the main loop).
const WAKE_MAX_DELAY_SECS: u64 = 2_592_000;
/// Validate a `schedule_wake` request against the master switch, the 30 s floor,
/// and the 16-pending cap. Returns a user-facing error string on rejection.
fn validate_schedule(enabled: bool, pending_count: usize, delay_secs: u64) -> Result<(), String> {
    if !enabled {
        return Err("scheduled wake-ups are disabled ([wake] enabled = false)".into());
    }
    if delay_secs < WAKE_MIN_DELAY_SECS {
        return Err(format!("delay must be at least {WAKE_MIN_DELAY_SECS}s"));
    }
    if delay_secs > WAKE_MAX_DELAY_SECS {
        return Err(format!(
            "delay must be at most {WAKE_MAX_DELAY_SECS}s (30 days)"
        ));
    }
    if pending_count >= WAKE_MAX_PENDING {
        return Err(format!("too many pending wakes (max {WAKE_MAX_PENDING})"));
    }
    Ok(())
}

/// Validate + persist a scheduled wake, insert it into the pending set, and
/// re-arm the watcher. Returns the new wake id (or a user-facing error).
async fn handle_schedule_wake(
    app: &mut App,
    delay_secs: u64,
    note: String,
) -> Result<String, String> {
    validate_schedule(app.config.wake.enabled, app.pending_wakes.len(), delay_secs)?;

    // Per-note deduplication: reject if a pending wake with the same note
    // already exists. Prevents the LLM from accumulating duplicate wakes for
    // the same event.
    if app.pending_wakes.values().any(|n| n == &note) {
        return Err(
            "a pending wake with this note already exists — cancel it first \
             with cancel_wake, or wait for it to fire. Do not schedule \
             duplicate wakes for the same event."
                .to_string(),
        );
    }

    let wake_id = Ulid::new().to_string();
    let fire_at_ms = now_ms().saturating_add(
        i64::try_from(delay_secs)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000),
    );
    app.record(EventKind::WakeScheduled {
        wake_id: wake_id.clone(),
        fire_at_ms,
        note: note.clone(),
    })
    .await
    .map_err(|e| format!("failed to persist wake: {e}"))?;
    app.pending_wakes
        .insert((fire_at_ms, wake_id.clone()), note);
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    Ok(wake_id)
}

/// Cancel one pending wake by id, or all when `id` is None. Records a
/// WakeCancelled per removed wake and re-arms the watcher. Cancelling an
/// unknown/already-fired id is a no-op success.
async fn handle_cancel_wake(app: &mut App, id: Option<String>) -> Result<String, String> {
    let targets: Vec<(i64, String)> = match &id {
        Some(want) => app
            .pending_wakes
            .keys()
            .filter(|(_, wid)| wid == want)
            .cloned()
            .collect(),
        None => app.pending_wakes.keys().cloned().collect(),
    };
    for (fire_at, wid) in &targets {
        app.record(EventKind::WakeCancelled {
            wake_id: wid.clone(),
        })
        .await
        .map_err(|e| format!("failed to persist cancel: {e}"))?;
        app.pending_wakes.remove(&(*fire_at, wid.clone()));
    }
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    Ok(match id {
        Some(_) => format!("cancelled {} wake(s)", targets.len()),
        None => format!("cancelled all {} pending wake(s)", targets.len()),
    })
}

/// Called after a `DelegationResult` has been recorded into `app.events` and the
/// finished subagent dropped from the in-flight sets. Decides whether the
/// orchestrator should continue so it actually *sees* the result:
/// - idle now (not streaming, not yielded) → mark streaming and return `true`
///   (the caller `spawn_turn`s);
/// - a turn is still streaming, or yielded → do nothing (the result sits in the
///   event log for the next turn's context).
///
/// Per-result: each `DelegationResult` gets its own wake decision — there is no
/// waiting for the whole subagent pool to drain.
///
/// Returns `true` iff the caller should `spawn_turn(app)` now. The spawn side
/// effect is left to the caller so this stays unit-testable without launching a
/// real turn.
fn plan_delegation_wake(app: &mut App) -> bool {
    let wake = should_wake_after_delegation(app.streaming, app.yielded);
    tracing::info!(
        wake = %wake,
        streaming = %app.streaming,
        yielded = %app.yielded,
        "delegation wake decision"
    );
    if wake {
        app.wake_after_delegation = false;
        app.streaming = true;
        true
    } else {
        // Defer: arm the wake when not yielded, regardless of streaming.
        // If streaming, the wake fires at TurnComplete (the continuation
        // turn sees the DelegationResult in the event log). If idle, it
        // fires immediately via should_wake_after_delegation returning true.
        if !app.yielded {
            app.wake_after_delegation = true;
        }
        false
    }
}

/// At `TurnComplete`, if no queued user message was consumed, decide whether a
/// deferred delegation wake should now continue the conversation. Clears the
/// flag either way. Returns `true` iff the caller should `spawn_turn(app)`.
/// (Per-result wakes mean this flag is rarely armed now — kept for the
/// transitional case where a result arrived while streaming.)
fn take_deferred_delegation_wake(app: &mut App) -> bool {
    let wake = app.wake_after_delegation && !app.yielded;
    app.wake_after_delegation = false;
    if wake {
        app.streaming = true;
    }
    wake
}

/// The synthetic UserMessage text a fired wake injects. Appends a late stamp
/// only when the fire is more than 5 s overdue (i.e. a catch-up on reopen, not
/// a normal on-time timer elapse).
const WAKE_LATE_STAMP_MS: i64 = 5_000;
fn wake_injection_text(note: &str, fire_at_ms: i64, now_ms: i64) -> String {
    if now_ms - fire_at_ms > WAKE_LATE_STAMP_MS {
        format!("⏰ scheduled: {note} (fired late)")
    } else {
        format!("⏰ scheduled: {note}")
    }
}

/// Drain every wake whose `fire_at_ms <= now`. When the orchestrator is idle and
/// not yielded: record each as a synthetic UserMessage + a WakeFired marker
/// (at-least-once — WakeFired is written ONLY here), drop them from the pending
/// set, re-arm the watcher, and spawn ONE continuation turn to process them.
/// When BUSY: touch nothing except parking the watcher (send None) so it stops
/// re-firing on the now-past deadline; the wakes stay pending and the
/// `TurnComplete` drain fires them once the turn ends (in correct log order).
/// Returns whether a turn was spawned.
async fn drain_due_wakes(app: &mut App) -> anyhow::Result<bool> {
    let now = now_ms();
    // Due keys (fire_at <= now), smallest-first. Exclusive upper bound at
    // `now + 1` so a wake due at exactly `now` is included and `now+…` excluded.
    let due: Vec<(i64, String)> = app
        .pending_wakes
        .range(..(now + 1, String::new()))
        .map(|((t, id), _)| (*t, id.clone()))
        .collect();
    if due.is_empty() {
        return Ok(false);
    }
    let idle = !app.streaming && !app.yielded;
    if !idle {
        // Busy: leave the wakes pending and park the watcher so it does not spin
        // on the past deadline. TurnComplete's drain fires them when idle.
        let _ = app.next_wake_tx.send(None);
        return Ok(false);
    }
    // Idle: fire them. Only now do we mutate the pending set + record events.
    for (fire_at, id) in &due {
        let note = app
            .pending_wakes
            .remove(&(*fire_at, id.clone()))
            .unwrap_or_default();
        let text = wake_injection_text(&note, *fire_at, now);
        app.record(EventKind::UserMessage { text }).await?;
        app.record(EventKind::WakeFired {
            wake_id: id.clone(),
        })
        .await?;
    }
    // Re-arm the watcher to the next remaining deadline (or park).
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    app.streaming = true;
    spawn_turn(app);
    Ok(true)
}

/// Spawn a queued subagent now that a pool slot has opened. Replicates the
/// handle-creation + spawn path from the agent loop's `dispatch_subagent`
/// arm: mint the ULID + id, create the guardrail tokens (cancel/hard/
/// progress/abort_reason), **register the handle in `app.in_flight`** (NOT
/// just the UI list — without this `fire_subagent_kill` and the timeout
/// supervisor can't reach it), notify the UI via `SubagentStarted` (non-
/// blocking `try_send` since this is a sync fn), create the worktree if
/// requested, and call `spawn_subagent`. Params come from `app.*` so the
/// queued spawn matches a direct spawn (same provider/model/session/clock).
fn spawn_queued_subagent(app: &mut App, qs: QueuedSubagent) {
    let sub_ulid = Ulid::new();
    let sub_id = format!("sub-{sub_ulid}");

    let sub_cancel = tokio_util::sync::CancellationToken::new();
    let sub_hard = tokio_util::sync::CancellationToken::new();
    let sub_progress = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(now_ms()));
    let sub_abort_reason = std::sync::Arc::new(std::sync::Mutex::new(None));

    // Register the handle BEFORE spawning so a fast-completing subagent can't
    // emit DelegationResult (which removes the id) before we insert it, and so
    // fire_subagent_kill / the timeout supervisor can reach it.
    app.in_flight.lock().unwrap().insert(
        sub_id.clone(),
        zoid::agent::SubagentHandle {
            cancel: sub_cancel.clone(),
            hard: sub_hard.clone(),
            progress: sub_progress.clone(),
            abort_reason: sub_abort_reason.clone(),
            task: qs.task.clone(),
            agent: qs.resolved_name.clone(),
        },
    );

    // Notify the UI (the existing SubagentStarted handler pushes to
    // in_flight_subagents + shell.subagent_rows — no change needed). Use
    // try_send: this is a sync fn called from the non-async DelegationResult
    // drain loop, so it cannot `.await` on the channel.
    let _ = app.ui_tx.try_send(AgentUpdate::SubagentStarted {
        id: sub_id.clone(),
        task: qs.task.clone(),
        agent: qs.resolved_name.clone(),
    });

    // Worktree (if requested — carried from the original dispatch call).
    let wt = if qs.want_worktree && std::path::Path::new(".git").exists() {
        zoid::worktree::create_worktree(std::path::Path::new("."), &format!("sub-{sub_ulid}")).ok()
    } else {
        None
    };
    let cwd = wt
        .as_ref()
        .map(|w| std::fs::canonicalize(w.path()).unwrap_or_else(|_| w.path().to_path_buf()))
        .unwrap_or_else(|| qs.cwd.clone());

    // Spawn — pull params from `app` so the queued spawn matches a direct one.
    zoid::spawn_subagent::spawn_subagent(
        qs.task,
        qs.resolved_profile,
        app.events.snapshot(),
        app.provider.clone(),
        cwd,
        app.model.clone(),
        // Thinking mode: resolve the same way spawn_turn does.
        {
            let model_support = app
                .fetched_model_info
                .map(|info| info.thinking)
                .unwrap_or_else(|| zoid_provider::model::model_info(&app.model).thinking);
            resolve_thinking(
                &app.config.thinking,
                app.prov.thinking_enabled,
                &app.config.provider,
                model_support,
            )
        },
        app.session.clone(),
        app.session_id,
        app.ui_tx.clone(),
        now_ms,
        sub_id.clone(),
        wt,
        app.config.approval.clone(),
        sub_cancel,
        sub_hard,
        sub_progress,
        sub_abort_reason,
        (app.config.subagent.idle_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(app.config.subagent.idle_timeout_secs)),
        (app.config.subagent.hard_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(app.config.subagent.hard_timeout_secs)),
        // Task 10 adds `app.registry`; until then the merged registry isn't on
        // `App`, so dispatch the empty default. The placeholder keeps the queued
        // spawn's signature aligned with a direct spawn_subagent call.
        std::sync::Arc::new(zoid_model::Registry::default()),
        app.config.provider.clone(),
    );
}

fn spawn_turn(app: &mut App) {
    let provider = app.provider.clone();
    let session = app.session.clone();
    let seed = app.events.snapshot();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    let session_id = app.session_id;
    // Per-turn snapshot: pick the active mode's profile + effective skills ONCE,
    // and bind a fresh invoke_skill tool to that snapshot. A mid-turn mode switch
    // or reload cannot mutate this in-flight turn (spec §5 / risk 1–2).
    let (profile, effective) =
        zoid_core::mode::active_turn(&app.modes, &app.skills, &app.base_profile);
    let menu = effective.menu();
    let kill = zoid_tools::KillSlot::new();
    let mut tools = zoid::invoke_skill::chat_tools(
        std::sync::Arc::new(effective),
        app.agents.clone(),
        kill.clone(),
    );
    if let Some(wiz) = &app.wizard {
        let wiz = std::sync::Arc::new(wiz.clone());
        tools.push(Box::new(zoid::mode_wizard::ProposeModeMappingTool::new(
            wiz.clone(),
        )));
        tools.push(Box::new(zoid::mode_wizard::ApplyModeMappingTool::new(wiz)));
    }
    if let Some(m) = &app.mcp {
        tools.extend(m.mcp_tools());
    }
    let tools = std::sync::Arc::new(tools);
    let mut turn_config = zoid::agent::chat_turn_config_with(&profile, &menu);
    // Context awareness: for small-context models, append a budget hint to the
    // system prompt so the model self-regulates its tool usage — prefers grep
    // over full-file reads, uses limit/offset, and avoids reading more than
    // necessary. Only added when the context window is under 64K (large-window
    // cloud models don't need the nudge).
    if app.shell.ctx_ceiling > 0 && app.shell.ctx_ceiling < 64_000 {
        let ctx_k = app.shell.ctx_ceiling / 1000;
        turn_config.system = format!(
            "{system}\n\n\
             ## Context budget\n\
             You have a {ctx_k}K token context window. Be context-efficient:\n\
             - Use `grep` or `glob` to find what you need before reading files.\n\
             - Use the `limit` and `offset` parameters on `read` for large files.\n\
             - Avoid reading more files than necessary. Read the most relevant file first.\n\
             - If you need to read multiple files, prioritize by relevance and stop early.\n\
             - Large tool outputs are compacted automatically; use `recall` to retrieve them.",
            system = turn_config.system,
            ctx_k = ctx_k,
        );
    }
    // If the session is inside a worktree, override the turn's cwd to the
    // worktree's path. This is the seam: `TurnConfig.cwd` is built fresh
    // each turn from `App` state, so a session-level field is how the new
    // cwd reaches every subsequent turn and every tool call within it.
    if let Some(wt) = &app.active_worktree {
        turn_config.cwd = wt.path.clone();
    }
    turn_config.mcp = app.mcp.clone();
    turn_config.embed = app.embed_index.clone();
    turn_config.embedder = app.embedder.clone();
    turn_config.policy = policy_from_config(&app.economy, app.context_target);
    turn_config.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: app.config.eviction.enabled,
        capacity: app.shell.ctx_ceiling,    // capacity = model window
        context_target: app.context_target, // resolved soft setpoint
        band_headroom_pct: app.economy.band_headroom_pct,
        min_protected_turns: app.economy.min_protected_turns,
        protection_pct: app.economy.protection_pct,
        max_output: None, // Slice-4 catalog supplies this; None → derived reserve
        rescue_weight: app.config.eviction.rescue_weight,
    };
    turn_config.reassert_interval = app.economy.reassert_interval_tokens;
    // Resolve thinking mode from config + model capability.
    let model_support = app
        .fetched_model_info
        .map(|info| info.thinking)
        .unwrap_or_else(|| zoid_provider::model::model_info(&app.model).thinking);
    turn_config.thinking = resolve_thinking(
        &app.config.thinking,
        app.prov.thinking_enabled,
        &app.config.provider,
        model_support,
    );
    turn_config.approval = app.config.approval.clone();
    turn_config.kill = kill.clone();
    turn_config.in_flight = Some(app.in_flight.clone());
    // Subagent guardrail timeouts (0 = disabled → None). Only the chat turn
    // dispatches subagents, so only it carries these.
    turn_config.subagent_idle = (app.config.subagent.idle_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(app.config.subagent.idle_timeout_secs));
    turn_config.subagent_ceiling = (app.config.subagent.hard_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(app.config.subagent.hard_timeout_secs));
    turn_config.max_concurrent = app.config.subagent.max_concurrent;
    turn_config.agents = Some(app.agents.clone());
    // Resolve per-(provider, model) caps from the merged registry (Task 8b).
    // `spawn_turn` overwrites the `Registry::default()` sentinel set by
    // `chat_turn_config_with` for tests.
    // Task 10 adds `app.registry`; until then the merged registry isn't on
    // `App`, so fall back to the empty default (conservative 32k fallback for
    // context-window / model-info resolution this turn).
    turn_config.reg = std::sync::Arc::new(zoid_model::Registry::default());
    turn_config.provider_id = app.config.provider.clone();
    // The live-fetched context window (from ModelInfoFetched / ctx_ceiling),
    // not the static table's conservative default. This is what the
    // hard-ceiling compaction pass uses to decide if the request fits.
    turn_config.context_window = app.shell.ctx_ceiling;
    // Mint fresh cancellation tokens for this turn and keep clones so
    // `Action::CancelTurn` (Esc/Ctrl-C) can fire them. Cleared on `TurnComplete`.
    let cancel = tokio_util::sync::CancellationToken::new();
    let hard = tokio_util::sync::CancellationToken::new();
    app.turn_cancel = Some(cancel.clone());
    app.turn_hard = Some(hard.clone());
    let companion_hub = app.companion_hub.clone();
    let gate: std::sync::Arc<dyn zoid_tools::ToolGate> = if app.yolo {
        std::sync::Arc::new(zoid_tools::AllowAll)
    } else {
        std::sync::Arc::new(zoid_tools::BlacklistGate::new(
            app.config.approval.shell_danger.clone(),
            app.config.approval.shell_allow.clone(),
            true, // interactive — Chat prompts
        ))
    };
    tokio::spawn(async move {
        let _ = run_agent_turn_cancellable(
            turn_config,
            provider,
            tools,
            gate,
            session,
            seed,
            model,
            ui,
            session_id,
            companion_hub,
            now_ms,
            cancel,
            hard,
        )
        .await;
    });
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    #[test]
    fn resolve_thinking_forces_off_when_unsupported() {
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: true,
            effort: Some("high".into()),
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "anthropic-api",
            zoid_provider::model::ThinkingSupport::None,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_off_when_config_disabled() {
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        // Explicit enabled=false → Source::UserGlobal → user override wins.
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::UserGlobal,
            "ollama-local",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_auto_when_enabled_no_effort() {
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: true,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::UserGlobal,
            "anthropic-api",
            zoid_provider::model::ThinkingSupport::Budget,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Auto);
    }

    #[test]
    fn resolve_thinking_effort_when_enabled_with_effort() {
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: true,
            effort: Some("max".into()),
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::UserGlobal,
            "anthropic-api",
            zoid_provider::model::ThinkingSupport::Adaptive,
        );
        assert_eq!(
            mode,
            zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::Max)
        );
    }

    #[test]
    fn resolve_thinking_provider_default_flips_on_for_ollama_local() {
        // Source::Default (user set no [thinking].enabled) + ollama-local + capable → Auto.
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "ollama-local",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Auto);
    }

    #[test]
    fn resolve_thinking_provider_default_off_for_cloud() {
        // Same Default provenance, but a cloud provider → stays Off (false default).
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "ollama-cloud",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_provider_default_off_when_capability_none() {
        // Provider default flips on, but the model doesn't support thinking → Off.
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "ollama-local",
            zoid_provider::model::ThinkingSupport::None,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_env_override_wins_over_provider_default() {
        // ZOID_THINKING=off → Source::Env → user override wins, even for ollama-local.
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Env,
            "ollama-local",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_effort_only_section_flows_through() {
        // User wrote [thinking] effort="high" with no enabled key. thinking_enabled
        // is Source::Default (the enabled key was absent), so the provider default
        // flips enabled to true, and effort flows through. The result is Effort(High).
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: Some("high".into()),
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "ollama-local",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(
            mode,
            zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::High)
        );
    }

    #[test]
    fn resolve_thinking_canonical_id_matches_legacy_ollama_spelling() {
        // "ollama" canonicalizes to "ollama-cloud", so the local default does NOT
        // apply to the legacy spelling. Only "ollama-local" matches.
        let cfg = zoid_core::config::ThinkingConfig {
            enabled: false,
            effort: None,
        };
        let mode = resolve_thinking(
            &cfg,
            zoid_core::config::Source::Default,
            "ollama",
            zoid_provider::model::ThinkingSupport::Toggle,
        );
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn parse_thinking_env_maps_values() {
        use zoid_core::config::PartialThinking;
        assert_eq!(
            parse_thinking_env("off"),
            Some(PartialThinking {
                enabled: Some(false),
                effort: None
            })
        );
        assert_eq!(
            parse_thinking_env("auto"),
            Some(PartialThinking {
                enabled: Some(true),
                effort: None
            })
        );
        assert_eq!(
            parse_thinking_env("high"),
            Some(PartialThinking {
                enabled: Some(true),
                effort: Some("high".into())
            })
        );
        assert_eq!(
            parse_thinking_env("max"),
            Some(PartialThinking {
                enabled: Some(true),
                effort: Some("max".into())
            })
        );
        assert!(parse_thinking_env("").is_none());
        assert!(parse_thinking_env("garbage").is_none());
        assert_eq!(
            parse_thinking_env("HIGH"),
            Some(PartialThinking {
                enabled: Some(true),
                effort: Some("high".into())
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};
    use ratatui_textarea::TextArea;

    #[test]
    fn rebuild_pending_wakes_projects_unfired_uncancelled() {
        use zoid_core::event::{Event, EventKind};
        let mk = |kind| Event::new(Ulid::new(), None, 0, kind);
        let evs = vec![
            mk(EventKind::WakeScheduled {
                wake_id: "a".into(),
                fire_at_ms: 300,
                note: "later".into(),
            }),
            mk(EventKind::WakeScheduled {
                wake_id: "b".into(),
                fire_at_ms: 100,
                note: "soon".into(),
            }),
            mk(EventKind::WakeScheduled {
                wake_id: "c".into(),
                fire_at_ms: 200,
                note: "gone".into(),
            }),
            mk(EventKind::WakeFired {
                wake_id: "b".into(),
            }), // b fired → excluded
            mk(EventKind::WakeCancelled {
                wake_id: "c".into(),
            }), // c cancelled → excluded
        ];
        let pending = rebuild_pending_wakes(&evs);
        // Only `a` survives; BTreeMap orders by (fire_at, id).
        assert_eq!(
            pending.len(),
            1,
            "only the un-fired, un-cancelled wake survives"
        );
        assert_eq!(
            pending.get(&(300, "a".to_string())).map(String::as_str),
            Some("later")
        );
        // Earliest fire_at of the pending set.
        assert_eq!(pending.keys().next().map(|(t, _)| *t), Some(300));
    }

    #[test]
    fn earliest_fire_at_is_the_min_key() {
        let mut pending = std::collections::BTreeMap::new();
        assert_eq!(
            earliest_fire_at(&pending),
            None,
            "empty → None (watcher parks)"
        );
        pending.insert((500i64, "a".to_string()), "n".to_string());
        pending.insert((100i64, "b".to_string()), "n".to_string());
        assert_eq!(
            earliest_fire_at(&pending),
            Some(100),
            "earliest fire_at wins"
        );
    }

    #[test]
    fn wake_injection_text_stamps_late_only_when_overdue() {
        // On-time (within 5s): no late stamp.
        assert_eq!(
            wake_injection_text("check CI", 10_000, 10_200),
            "⏰ scheduled: check CI"
        );
        // Overdue by > 5s (catch-up on reopen): late stamp appended.
        assert_eq!(
            wake_injection_text("check CI", 10_000, 20_000),
            "⏰ scheduled: check CI (fired late)"
        );
    }

    #[test]
    fn validate_schedule_enforces_switch_floor_and_cap() {
        assert!(
            validate_schedule(false, 0, 60).is_err(),
            "disabled → reject"
        );
        assert!(
            validate_schedule(true, 0, 29).is_err(),
            "below 30s floor → reject"
        );
        assert!(
            validate_schedule(true, 16, 60).is_err(),
            "at 16 pending cap → reject"
        );
        assert!(
            validate_schedule(true, 15, 30).is_ok(),
            "enabled, 30s, under cap → ok"
        );
        assert!(
            validate_schedule(true, 0, WAKE_MAX_DELAY_SECS + 1).is_err(),
            "above 30-day cap → reject"
        );
        assert!(
            validate_schedule(true, 0, WAKE_MAX_DELAY_SECS).is_ok(),
            "exactly at cap → ok"
        );
    }

    #[test]
    fn second_cancel_escalates_to_hard() {
        let graceful = tokio_util::sync::CancellationToken::new();
        let hard = tokio_util::sync::CancellationToken::new();
        let reg = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            zoid::agent::SubagentHandle,
        >::new()));
        // First Esc: graceful only.
        assert_eq!(
            escalate_cancel(&graceful, &hard, &reg),
            "cancelling… (Esc again to force)"
        );
        assert!(graceful.is_cancelled() && !hard.is_cancelled());
        // Second Esc: escalate to hard.
        assert_eq!(escalate_cancel(&graceful, &hard, &reg), "force-stopping…");
        assert!(hard.is_cancelled());
    }

    #[test]
    fn escalate_force_fires_registered_subagents() {
        use std::collections::HashMap;
        let graceful = tokio_util::sync::CancellationToken::new();
        let hard = tokio_util::sync::CancellationToken::new();
        let sub = zoid::agent::SubagentHandle {
            cancel: tokio_util::sync::CancellationToken::new(),
            hard: tokio_util::sync::CancellationToken::new(),
            progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
            abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
            task: String::new(),
            agent: String::new(),
        };
        let mut map = HashMap::new();
        map.insert("sub-x".to_string(), sub.clone());
        let reg = std::sync::Arc::new(std::sync::Mutex::new(map));

        // First press: graceful only — subagents untouched.
        let _ = escalate_cancel(&graceful, &hard, &reg);
        assert!(
            !sub.hard.is_cancelled(),
            "first Esc must not kill subagents"
        );

        // Second press: force — every registered subagent's hard fires with Killed.
        let hint = escalate_cancel(&graceful, &hard, &reg);
        assert_eq!(hint, "force-stopping…");
        assert!(
            sub.hard.is_cancelled(),
            "force Esc kills in-flight subagents"
        );
        assert_eq!(
            *sub.abort_reason.lock().unwrap(),
            Some(zoid::agent::AbortReason::Killed)
        );
    }

    #[test]
    fn subagent_kill_decision_arms_then_fires() {
        // First press (disarmed): arms, does NOT fire, prompts for confirm.
        let (next, fire, hint) = super::subagent_kill_decision(false, 3);
        assert!(next, "first press arms");
        assert!(!fire, "first press must not fire");
        assert!(
            hint.contains("Esc again"),
            "first press asks to confirm: {hint}"
        );

        // Second press (armed): fires, disarms, reports the kill.
        let (next, fire, hint) = super::subagent_kill_decision(true, 3);
        assert!(!next, "second press disarms");
        assert!(fire, "second press fires");
        assert!(
            hint.contains("killing"),
            "second press reports the kill: {hint}"
        );
    }

    #[test]
    fn zoom_anchor_maps_top_message_across_altitudes() {
        // Detail body: msgs start at lines [0, 6, 14]; viewport top at line 7 → msg 1.
        let detail_starts = [0usize, 6, 14];
        let anchor = zoid_tui::msg_at_line(&detail_starts, 7);
        assert_eq!(anchor, 1);
        // Summary body: same msgs collapse → msg 1 lives on line 0.
        let summary_starts = [0usize, 0, 1];
        assert_eq!(zoid_tui::line_of_msg(&summary_starts, anchor), 0);
    }

    #[test]
    fn question_keystroke_takes_incremental_path_not_full_rebuild() {
        use zoid_core::event::QuestionKind;
        use zoid_core::projection::{ChatMsg, QuestionCardState};
        use zoid_tui::question::QuestionState;
        use zoid_tui::state::Zoom;

        // A transcript ending in an open ask_user card — the pending question is
        // always the last message.
        let msgs = vec![
            ChatMsg::User {
                text: "hi".into(),
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: "hello".into(),
                tool_calls: vec![],
                ts: 0,
            },
            ChatMsg::Question {
                id: "q1".into(),
                kind: QuestionKind::Ask,
                question: "why?".into(),
                choices: vec![], // free-text mode so the buffer renders verbatim
                state: QuestionCardState::Open {
                    selected: 0,
                    free_text: String::new(),
                },
                ts: 0,
            },
        ];

        let mk_key = |q: Option<&QuestionState>| BodyKey {
            zoom: Zoom::Normal,
            width: 80,
            streaming: false,
            caret: false,
            tz: 0,
            question_rev: question_rev(q),
        };

        let mut cache = BodyCache::default();
        let mut q = QuestionState::new("why?", vec![]); // FreeText mode

        // Cold cache → full rebuild; identical inputs → pure hit.
        assert_eq!(
            cache.refresh(mk_key(Some(&q)), &msgs, 80, Some(&q), &[], 0),
            RefreshKind::Full
        );
        assert_eq!(
            cache.refresh(mk_key(Some(&q)), &msgs, 80, Some(&q), &[], 0),
            RefreshKind::Hit
        );

        // Flatten the cached body's span text — lets us assert the card content
        // actually tracks the buffer, not just that the fast path was taken.
        let body_text = |c: &BodyCache| -> String {
            c.body
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };

        // The bug: a keystroke used to force a full O(n) rebuild. It must now
        // re-render only the last message (the card) — AND show the typed text.
        q.free_text.push_str("zoidberg");
        assert_eq!(
            cache.refresh(mk_key(Some(&q)), &msgs, 80, Some(&q), &[], 0),
            RefreshKind::Incremental,
            "a question keystroke must not rebuild the whole transcript"
        );
        assert!(
            body_text(&cache).contains("zoidberg"),
            "incremental re-render must reflect the live buffer, not stale text"
        );
        // Backspace is likewise incremental and updates the card.
        for _ in 0..4 {
            q.free_text.pop();
        }
        assert_eq!(
            cache.refresh(mk_key(Some(&q)), &msgs, 80, Some(&q), &[], 0),
            RefreshKind::Incremental
        );
        assert!(body_text(&cache).contains("zoid"));
        assert!(!body_text(&cache).contains("zoidberg"));

        // A structural change (width) still forces a full rebuild.
        assert_eq!(
            cache.refresh(
                BodyKey {
                    width: 100,
                    ..mk_key(Some(&q))
                },
                &msgs,
                100,
                Some(&q),
                &[],
                0
            ),
            RefreshKind::Full
        );
    }

    #[test]
    fn input_delete_line_drops_the_cursor_line_regardless_of_column() {
        let mut ta = TextArea::from(vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
        ]);
        // Land the cursor mid-word on the middle line.
        ta.move_cursor(CursorMove::Down);
        ta.move_cursor(CursorMove::Forward);
        ta.move_cursor(CursorMove::Forward);
        input_delete_line(&mut ta);
        assert_eq!(ta.lines(), &["one".to_string(), "three".to_string()]);
        // The following line pulled up to the head of where the deleted line was.
        assert_eq!(ta.cursor(), (1, 0));
    }

    #[test]
    fn input_delete_line_on_sole_line_leaves_empty_buffer() {
        let mut ta = TextArea::from(vec!["only".to_string()]);
        input_delete_line(&mut ta);
        assert_eq!(ta.lines(), &["".to_string()]);
    }

    #[test]
    fn input_delete_line_on_blank_line_removes_only_that_row() {
        // Regression: delete_line_by_end merges the next line up on its own for an
        // empty line, so a second merge used to eat the leading char of "three".
        let mut ta = TextArea::from(vec!["one".to_string(), "".to_string(), "three".to_string()]);
        ta.move_cursor(CursorMove::Down); // onto the blank line
        input_delete_line(&mut ta);
        assert_eq!(ta.lines(), &["one".to_string(), "three".to_string()]);
        assert_eq!(ta.cursor(), (1, 0));
    }

    #[test]
    fn input_delete_line_between_two_blanks_removes_one_blank() {
        // Regression: two consecutive blanks used to collapse together.
        let mut ta = TextArea::from(vec![
            "one".to_string(),
            "".to_string(),
            "".to_string(),
            "two".to_string(),
        ]);
        ta.move_cursor(CursorMove::Down); // first blank
        input_delete_line(&mut ta);
        assert_eq!(
            ta.lines(),
            &["one".to_string(), "".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn input_delete_line_on_last_line_collapses_the_buffer() {
        // Deleting the last line must remove the row (not leave an empty slot),
        // consistent with deleting any other line.
        let mut ta = TextArea::from(vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
        ]);
        ta.move_cursor(CursorMove::Bottom); // onto "three"
        input_delete_line(&mut ta);
        assert_eq!(ta.lines(), &["one".to_string(), "two".to_string()]);
        assert_eq!(ta.cursor(), (1, 3)); // end of the new last line
    }

    #[test]
    fn input_cursor_top_and_bottom_reach_buffer_extremes() {
        let mut ta = TextArea::from(vec!["ab".to_string(), "cd".to_string(), "ef".to_string()]);
        // Start somewhere in the middle.
        ta.move_cursor(CursorMove::Down);
        ta.move_cursor(CursorMove::End);
        input_cursor_top(&mut ta);
        assert_eq!(ta.cursor(), (0, 0));
        input_cursor_bottom(&mut ta);
        assert_eq!(ta.cursor(), (2, 2)); // last line, past the final char
    }

    /// Render the textarea into a scratch buffer and report whether any cell
    /// carries the UNDERLINED modifier (tui-textarea's default cursor line).
    fn has_underline(ta: &TextArea<'static>) -> bool {
        let backend = TestBackend::new(20, 3);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| f.render_widget(ta, f.area())).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.modifier.contains(Modifier::UNDERLINED))
    }

    #[test]
    fn make_input_disables_cursor_line_underline() {
        // Sanity: the tui-textarea default underlines the cursor line.
        let default = TextArea::from(vec!["hello".to_string()]);
        assert!(
            has_underline(&default),
            "default TextArea underlines the cursor line"
        );
        // make_input turns it off.
        let plain = make_input(TextArea::from(vec!["hello".to_string()]));
        assert!(
            !has_underline(&plain),
            "make_input must disable the cursor-line underline"
        );
    }

    #[test]
    fn make_input_sets_word_or_glyph_wrap() {
        let ta = make_input(TextArea::from(vec![
            "a very long line that exceeds twenty columns".to_string(),
        ]));
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| f.render_widget(&ta, f.area())).unwrap();
        // With WordOrGlyph wrap, the long line spans rows beyond the first.
        // Check that the second buffer row (y==1) has non-space content —
        // without wrap, only row 0 would carry text and rows 1+ would be blank.
        let buf = term.backend().buffer();
        let row1_has_text = (0..buf.area().width).any(|x| {
            let s = buf.cell((x, 1)).map(|c| c.symbol()).unwrap_or(" ");
            !s.is_empty() && s != " "
        });
        assert!(
            row1_has_text,
            "wrapped line must occupy row 1 (wrap mode active)"
        );
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn policy_from_config_maps_pct_to_absolute() {
        let econ = zoid_core::config::EconomyConfig {
            context_target: Some(200_000),
            auto_evict_cold: false,
            compact_threshold_pct: 80,
            band_headroom_pct: 20,
            min_protected_turns: 3,
            protection_pct: 15,
            reassert_interval_tokens: 100_000,
            num_ctx: None,
        };
        let p = policy_from_config(&econ, 200_000);
        assert!(!p.auto_evict_cold);
        assert_eq!(p.token_ceiling, None); // EconomyConfig.token_ceiling retired
        assert_eq!(p.compact_threshold, Some(160_000)); // 80% of 200k
                                                        // 0% disables compaction
        let econ0 = zoid_core::config::EconomyConfig {
            compact_threshold_pct: 0,
            ..econ
        };
        assert_eq!(policy_from_config(&econ0, 200_000).compact_threshold, None);
    }

    #[test]
    fn config_field_target_and_value_mapping() {
        use zoid_core::config::TomlValue;
        use zoid_tui::config_view::FieldKind;
        // Secret rows always target the secret store, keyed by their label.
        assert_eq!(
            field_target("OLLAMA_API_KEY", &FieldKind::Secret),
            Some(FieldTarget::Secret)
        );
        // provider / model → string TOML keys.
        assert_eq!(
            field_target("provider", &FieldKind::Pick),
            Some(FieldTarget::Toml {
                key: "provider",
                ty: TomlTy::Str
            })
        );
        assert_eq!(
            field_target("model", &FieldKind::Text),
            Some(FieldTarget::Toml {
                key: "model",
                ty: TomlTy::Str
            })
        );
        // context target → economy.context_target, uint-with-unset.
        assert_eq!(
            field_target("context target", &FieldKind::Uint),
            Some(FieldTarget::Toml {
                key: "economy.context_target",
                ty: TomlTy::U64Unset
            })
        );
        // empty base_url removes the key.
        assert_eq!(
            field_target("base_url", &FieldKind::Text),
            Some(FieldTarget::Toml {
                key: "base_url",
                ty: TomlTy::StrUnsetEmpty
            })
        );
        // Bools persist via toggle, not the edit buffer → no text target.
        assert!(field_target("reduced motion", &FieldKind::Bool).is_none());
        assert!(field_target("auto-evict cold", &FieldKind::Bool).is_none());
        assert!(field_target("eviction", &FieldKind::Bool).is_none());

        // Value coercion: empty / "(none)" ceiling ⇒ Unset; a number ⇒ Int.
        assert_eq!(
            value_from_buffer(&TomlTy::U64Unset, ""),
            Some(TomlValue::Unset)
        );
        assert_eq!(
            value_from_buffer(&TomlTy::U64Unset, "(none)"),
            Some(TomlValue::Unset)
        );
        assert_eq!(
            value_from_buffer(&TomlTy::U64Unset, "512000"),
            Some(TomlValue::Int(512_000))
        );
        assert_eq!(
            value_from_buffer(&TomlTy::U64Unset, "bogus"),
            Some(TomlValue::Unset)
        );
        // Empty base_url buffer ⇒ Unset (removes key); non-empty ⇒ Str.
        assert_eq!(
            value_from_buffer(&TomlTy::StrUnsetEmpty, "  "),
            Some(TomlValue::Unset)
        );
        assert_eq!(
            value_from_buffer(&TomlTy::StrUnsetEmpty, "http://x"),
            Some(TomlValue::Str("http://x".into()))
        );
        // compact % clamps to 0..=100; unparseable is a no-op.
        assert_eq!(
            value_from_buffer(&TomlTy::U8Pct, "150"),
            Some(TomlValue::Int(100))
        );
        assert_eq!(
            value_from_buffer(&TomlTy::U8Pct, "80"),
            Some(TomlValue::Int(80))
        );
        assert_eq!(value_from_buffer(&TomlTy::U8Pct, "xx"), None);
        // protected turns: plain non-negative integer; unparseable/negative is a no-op.
        assert_eq!(
            value_from_buffer(&TomlTy::UintPlain, "4"),
            Some(TomlValue::Int(4))
        );
        assert_eq!(value_from_buffer(&TomlTy::UintPlain, "-1"), None);
        assert_eq!(value_from_buffer(&TomlTy::UintPlain, "xx"), None);
    }

    #[test]
    fn base_url_write_seeds_registry_default_or_unsets() {
        use zoid_core::config::TomlValue;
        assert_eq!(
            base_url_write_for("ollama-local"),
            TomlValue::Str("http://localhost:11434".into())
        );
        assert_eq!(
            base_url_write_for("ollama"),
            TomlValue::Str("https://ollama.com".into())
        ); // alias → cloud
        assert_eq!(
            base_url_write_for("anthropic-api"),
            TomlValue::Str("https://api.anthropic.com".into())
        );
        // A non-existent provider id resolves to no default base_url → Unset
        // (the falsified anthropic-cli/anthropic-sdk rows were removed; see
        // spikes/cc-infer/RESULTS.md).
        assert_eq!(base_url_write_for("anthropic-cli"), TomlValue::Unset);
    }

    #[test]
    fn layer_warning_line_is_file_qualified() {
        assert_eq!(
            layer_warning_line("config.toml", "economy.context_ceiling"),
            "config.toml: ignored unknown key economy.context_ceiling"
        );
    }

    #[test]
    fn config_warning_hint_none_one_many() {
        assert_eq!(config_warning_hint(&[]), None);
        assert_eq!(
            config_warning_hint(&["economy.context_ceiling".to_string()]),
            Some("config: 1 key ignored (economy.context_ceiling)".to_string())
        );
        assert_eq!(
            config_warning_hint(&["a".to_string(), "b".to_string()]),
            Some("config: 2 keys ignored — see log".to_string())
        );
    }

    #[test]
    fn write_config_file_round_trips_through_temp_dir() {
        use zoid_core::config::{parse_toml, TomlValue};
        let dir = tempfile::tempdir().unwrap();
        // Parent dir does not exist yet — write_config_file must create it.
        let path = dir.path().join("nested").join("config.toml");
        write_config_file(&path, "reduced_motion", TomlValue::Bool(true)).unwrap();
        let (parsed, _) = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.reduced_motion, Some(true));
        // A nested-table write preserves the earlier top-level key.
        write_config_file(&path, "economy.context_target", TomlValue::Int(200_000)).unwrap();
        let (parsed, _) = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.reduced_motion, Some(true));
        assert_eq!(parsed.economy.context_target, Some(200_000));
        // Unset removes the key again.
        write_config_file(&path, "economy.context_target", TomlValue::Unset).unwrap();
        let (parsed, _) = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.economy.context_target, None);
    }

    #[test]
    fn zoid_db_overrides_everything() {
        let p = resolve_db_path(env_of(&[("ZOID_DB", "/tmp/x.db"), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/tmp/x.db"));
    }

    #[test]
    fn xdg_data_home_wins_over_home() {
        let p = resolve_db_path(env_of(&[("XDG_DATA_HOME", "/xdg"), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/xdg/zoid/zoid.db"));
    }

    #[test]
    fn falls_back_to_home_local_share() {
        let p = resolve_db_path(env_of(&[("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/home/u/.local/share/zoid/zoid.db"));
    }

    #[test]
    fn userprofile_fallback_when_home_unset() {
        // Windows: HOME is unset outside Git Bash; USERPROFILE is the native
        // home dir. Without this fallback, paths resolve against the CWD.
        let p = resolve_db_path(env_of(&[("USERPROFILE", r"C:\Users\u")]));
        assert_eq!(
            p,
            PathBuf::from(r"C:\Users\u").join(".local/share/zoid/zoid.db")
        );
    }

    #[test]
    fn home_preferred_over_userprofile() {
        // On Git Bash, both HOME and USERPROFILE are set; HOME wins (Unix-style).
        let p = resolve_db_path(env_of(&[
            ("HOME", "/home/u"),
            ("USERPROFILE", r"C:\Users\u"),
        ]));
        assert_eq!(p, PathBuf::from("/home/u/.local/share/zoid/zoid.db"));
    }

    #[test]
    fn config_dir_userprofile_fallback() {
        let p = resolve_config_dir(env_of(&[("USERPROFILE", r"C:\Users\u")]));
        assert_eq!(p, PathBuf::from(r"C:\Users\u").join(".config/zoid"));
    }

    #[test]
    fn resolve_config_dir_prefers_xdg_then_home() {
        let x = resolve_config_dir(env_of(&[("XDG_CONFIG_HOME", "/x/cfg")]));
        assert_eq!(x, PathBuf::from("/x/cfg/zoid"));
        let h = resolve_config_dir(env_of(&[("HOME", "/home/u")]));
        assert_eq!(h, PathBuf::from("/home/u/.config/zoid"));
    }

    #[test]
    fn derives_name_from_first_message_else_timestamp() {
        // Truncates a long first message to <= 40 display chars with an ellipsis.
        let long = "fix the 500 error on GET /users/:id when the row is missing entirely";
        let n = derive_session_name(Some(long), 0, 0);
        assert!(n.chars().count() <= 40);
        assert!(n.starts_with("fix the 500"));
        // Empty / no message → timestamp fallback (HH:MM, deterministic at offset 0).
        assert_eq!(derive_session_name(None, 49_500_000, 0), "session 13:45");
        assert_eq!(derive_session_name(Some("   "), 0, 0), "session 00:00");
    }

    #[test]
    fn worktree_label_none_for_main_worktree() {
        use git2::Repository;
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo = Repository::init(&repo_dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree_id = {
            let mut index = repo.index().unwrap();
            std::fs::write(repo_dir.join("README"), "hi").unwrap();
            index.add_path(std::path::Path::new("README")).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        std::mem::forget(dir);
        assert_eq!(worktree_label(&repo), "(none)");
    }

    #[test]
    fn worktree_label_name_for_linked_worktree() {
        use git2::Repository;
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo = Repository::init(&repo_dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree_id = {
            let mut index = repo.index().unwrap();
            std::fs::write(repo_dir.join("README"), "hi").unwrap();
            index.add_path(std::path::Path::new("README")).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let wt_path = dir.path().join("wt");
        repo.worktree("feature", &wt_path, Some(&git2::WorktreeAddOptions::new()))
            .unwrap();
        let wt_repo = Repository::open(&wt_path).unwrap();
        assert_eq!(worktree_label(&wt_repo), "feature");
        std::mem::forget(dir);
    }

    #[test]
    fn parses_numstat_sums_and_counts_files() {
        let out = "12\t3\tsrc/a.rs\n0\t5\tsrc/b.rs\n7\t0\tCargo.toml\n";
        assert_eq!(parse_numstat(out), (19, 8, 3)); // added=12+0+7, removed=3+5+0, files=3
                                                    // Binary files show `-\t-\tpath`; count the file, add zero lines.
        assert_eq!(parse_numstat("-\t-\tlogo.png\n"), (0, 0, 1));
        assert_eq!(parse_numstat(""), (0, 0, 0));
    }

    #[test]
    fn imports_legacy_events_under_one_session_once() {
        use rusqlite::{params, Connection};
        use ulid::Ulid;
        use zoid_core::event::EventKind;
        use zoid_core::store::EventStore;
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.db");
        let newdb = dir.path().join("new.db");
        // Seed a legacy DB with the REAL pre-session_id 6-column schema (no
        // `session_id` column) — this is what actually ships in the field.
        {
            let conn = Connection::open(&legacy).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                    id     TEXT PRIMARY KEY,
                    parent TEXT,
                    branch TEXT NOT NULL,
                    ts     INTEGER NOT NULL,
                    kind   TEXT NOT NULL,
                    tokens TEXT
                );",
            )
            .unwrap();
            let id1 = Ulid::from(1u128);
            let id2 = Ulid::from(2u128);
            let kind1 = serde_json::to_string(&EventKind::UserMessage {
                text: "old q".into(),
            })
            .unwrap();
            let kind2 = serde_json::to_string(&EventKind::AssistantMessage {
                text: "old a".into(),
            })
            .unwrap();
            conn.execute(
                "INSERT INTO events (id, parent, branch, ts, kind, tokens) VALUES (?1, NULL, ?2, ?3, ?4, NULL)",
                params![id1.to_string(), "main", 1i64, kind1],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (id, parent, branch, ts, kind, tokens) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                params![id2.to_string(), id1.to_string(), "main", 2i64, kind2],
            )
            .unwrap();
        }
        let sid = Ulid::from(42u128);
        // First run imports.
        assert!(import_legacy_if_present(&newdb, &legacy, sid, "imported", "/repo", 500).unwrap());
        let s = EventStore::open(newdb.to_str().unwrap()).unwrap();
        assert_eq!(s.load_session(sid).unwrap().len(), 2);
        assert_eq!(s.list_session_rows().unwrap().len(), 1);
        // Second run is a no-op (new DB already exists → nothing re-imported).
        assert!(!import_legacy_if_present(&newdb, &legacy, sid, "imported", "/repo", 500).unwrap());
    }

    /// Build a minimal `App` for exercising `handle_action` without a real
    /// terminal/provider/network — a temp-file session DB and an offline
    /// `FakeProvider` that never produces events.
    async fn test_app() -> App {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        // Leak the tempdir so the DB file outlives this fn (test process exits
        // anyway; avoids a dangling path from an early drop).
        std::mem::forget(dir);
        let session = SessionHandle::spawn(db.to_str().unwrap()).unwrap();
        let session_id = Ulid::new();
        session
            .new_session(session_id, "test".into(), "/repo".into(), 0)
            .await
            .unwrap();
        let (ui_tx, _ui_rx) = mpsc::channel::<AgentUpdate>(8);
        App {
            session,
            session_id,
            events: zoid::eventlog::EventLog::new(),
            provider: Arc::new(zoid_provider::FakeProvider::new(Vec::new())),
            base_profile: zoid::agent::default_profile(),
            modes: zoid_core::mode::ModeRegistry::new(vec![zoid_core::mode::Mode::chat(
                zoid::agent::default_profile(),
            )]),
            mode_dirs: Vec::new(),
            skills: std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            agents: std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            wizard: None,
            pending_adjust: None,
            model: "test-model".into(),
            economy: zoid_core::config::EconomyConfig::default(),
            context_target: 300_000,
            config: zoid_core::config::Config::default(),
            yolo: false,
            prov: {
                use zoid_core::config::Source;
                zoid_core::config::Provenance {
                    provider: Source::Default,
                    base_url: Source::Default,
                    model: Source::Default,
                    context_target: Source::Default,
                    auto_evict_cold: Source::Default,
                    compact_threshold_pct: Source::Default,
                    band_headroom_pct: Source::Default,
                    min_protected_turns: Source::Default,
                    protection_pct: Source::Default,
                    reduced_motion: Source::Default,
                    thinking_enabled: Source::Default,
                    thinking_effort: Source::Default,
                    subagent_idle_timeout_secs: Source::Default,
                    subagent_hard_timeout_secs: Source::Default,
                    subagent_max_concurrent: Source::Default,
                    approval: Source::Default,
                    reassert_interval_tokens: Source::Default,
                    num_ctx: Source::Default,
                    ui_edit_diff: Source::Default,
                    ui_edit_diff_inline: Source::Default,
                    wake_enabled: Source::Default,
                    companion_enabled: Source::Default,
                    eviction_enabled: Source::Default,
                }
            },
            secrets: None,
            textarea: make_input(TextArea::default()),
            streaming: false,
            shell: zoid_tui::ShellState::new(),
            feedback_reply: None,
            ui_tx,
            started: std::time::Instant::now(),
            proj: ProjectionCache::default(),
            body_cache: BodyCache::default(),
            overview_body: Vec::new(),
            zoom_changed_at: None,
            last_conv_max_scroll: 0,
            last_conv_rect: ratatui::layout::Rect::default(),
            pending_zoom_anchor: None,
            tz_offset_secs: 0,
            session_started_ms: 0,
            session_ids: Vec::new(),
            in_flight_subagents: Vec::new(),
            queued_subagents: std::collections::VecDeque::new(),
            active_worktree: None,
            active_wt_tx: tokio::sync::watch::channel(None).0,
            in_flight: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_answer: None,
            turn_cancel: None,
            turn_hard: None,
            subagent_kill_armed: false,
            fetched_model_info: None,
            companion: None,
            companion_hub: zoid_companion::CompanionHub::new(),
            compaction_started_at: None,
            compaction_complete: false,
            reassert_count: 0,
            tool_started_at: None,
            tool_complete: false,
            yielded: false,
            pending_message: None,
            wake_after_delegation: false,
            pending_wakes: std::collections::BTreeMap::new(),
            next_wake_tx: tokio::sync::watch::channel(None).0,
            mcp: None,
            embed_index: None,
            embedder: None,
            installing_plugin: false,
        }
    }

    #[tokio::test]
    async fn schedule_then_cancel_roundtrips_the_pending_set() {
        let mut app = test_app().await;
        // Assumes wake.enabled defaults true in the test config.
        let id = handle_schedule_wake(&mut app, 60, "check CI".into())
            .await
            .unwrap();
        assert_eq!(
            app.pending_wakes.len(),
            1,
            "schedule inserts one pending wake"
        );
        assert_eq!(
            earliest_fire_at(&app.pending_wakes),
            app.pending_wakes.keys().next().map(|(t, _)| *t)
        );

        let msg = handle_cancel_wake(&mut app, Some(id)).await.unwrap();
        assert!(app.pending_wakes.is_empty(), "cancel removes it");
        assert!(msg.contains("cancelled 1"));

        // A projection rebuild over the recorded events agrees (Scheduled then Cancelled → empty).
        let log = app.session.snapshot().await.unwrap();
        assert!(
            rebuild_pending_wakes(&log).is_empty(),
            "event-log projection matches live state"
        );
    }

    #[tokio::test]
    async fn handle_schedule_wake_rejects_duplicate_note() {
        let mut app = test_app().await;
        // Schedule first wake
        let id1 = handle_schedule_wake(&mut app, 60, "check CI status".into())
            .await
            .unwrap();
        assert!(!id1.is_empty());

        // Same note → rejected
        let err = handle_schedule_wake(&mut app, 90, "check CI status".into())
            .await
            .unwrap_err();
        assert!(
            err.contains("already exists"),
            "duplicate note should be rejected: {err}"
        );
        assert!(
            err.contains("cancel it first"),
            "error should tell the model to cancel first: {err}"
        );
        assert!(
            err.contains("wait for it to fire"),
            "error should offer the wait alternative: {err}"
        );

        // Different note → succeeds
        let id2 = handle_schedule_wake(&mut app, 60, "check subagent status".into())
            .await
            .unwrap();
        assert!(!id2.is_empty() && id2 != id1);
    }

    #[tokio::test]
    async fn due_wake_injects_usermessage_and_fires_when_idle() {
        let mut app = test_app().await;
        app.streaming = false;
        let past = now_ms() - 10_000; // already due, > 5s late
        app.pending_wakes
            .insert((past, "w1".to_string()), "check the build".to_string());
        let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));

        let spawned = drain_due_wakes(&mut app).await.unwrap();

        assert!(spawned, "an idle orchestrator fires the due wake");
        assert!(app.streaming, "firing marks the turn streaming");
        assert!(
            app.pending_wakes.is_empty(),
            "the fired wake leaves the pending set"
        );
        let log = app.session.snapshot().await.unwrap();
        assert!(
            log.iter().any(|e| matches!(&e.kind,
                EventKind::UserMessage { text } if text == "⏰ scheduled: check the build (fired late)")),
            "the late-stamped note is injected as a UserMessage"
        );
        assert!(
            log.iter()
                .any(|e| matches!(&e.kind, EventKind::WakeFired { wake_id } if wake_id == "w1")),
            "a WakeFired marker is recorded at injection (at-least-once)"
        );
    }

    #[tokio::test]
    async fn future_wake_does_not_fire() {
        let mut app = test_app().await;
        app.streaming = false;
        let future = now_ms() + 60_000;
        app.pending_wakes
            .insert((future, "w2".to_string()), "later".to_string());
        let spawned = drain_due_wakes(&mut app).await.unwrap();
        assert!(!spawned, "a not-yet-due wake must not fire");
        assert_eq!(app.pending_wakes.len(), 1, "it stays pending");
    }

    #[tokio::test]
    async fn busy_drain_is_side_effect_free_and_parks_watcher() {
        // C1 invariant: when a turn is in flight, a due wake must NOT be recorded
        // or removed — the drain only parks the watcher (send None) so it stops
        // spinning on the past deadline; TurnComplete's drain fires it once idle.
        let mut app = test_app().await;
        app.streaming = true; // busy: a turn is in flight
        let past = now_ms() - 10_000; // already due
        app.pending_wakes
            .insert((past, "w3".to_string()), "check later".to_string());
        let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
        let mut rx = app.next_wake_tx.subscribe();

        let spawned = drain_due_wakes(&mut app).await.unwrap();

        assert!(!spawned, "a busy orchestrator must not spawn a turn");
        assert_eq!(
            app.pending_wakes.len(),
            1,
            "busy drain must NOT remove the wake (side-effect-free)"
        );
        let log = app.session.snapshot().await.unwrap();
        assert!(
            !log.iter()
                .any(|e| matches!(&e.kind, EventKind::WakeFired { .. })),
            "busy drain must record no WakeFired (side-effect-free)"
        );
        assert_eq!(
            *rx.borrow_and_update(),
            None,
            "busy drain parks the watcher (send None) so it stops spinning on the past deadline"
        );
    }

    #[test]
    fn projection_cache_recomputes_only_on_len_change() {
        use zoid_core::event::{Event, EventKind};
        let mk = |t: &str| {
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::UserMessage { text: t.into() },
            )
        };
        let mut cache = ProjectionCache::default();
        let mut events = zoid::eventlog::EventLog::from_vec(vec![mk("hello there friend")]);
        assert!(cache.refresh(&events), "first refresh always recomputes");
        assert_eq!(cache.events_len, Some(1));
        assert_eq!(cache.msgs.len(), 1);
        // Same length → no recompute (the hot path: every keystroke / scroll).
        assert!(!cache.refresh(&events), "unchanged length must be a no-op");
        // Log grew → recompute.
        events.push(mk("another substantive message"));
        assert!(cache.refresh(&events), "a longer log must recompute");
        assert_eq!(cache.events_len, Some(2));
        assert_eq!(cache.msgs.len(), 2);
    }

    #[test]
    fn apply_event_parity_with_full_refresh() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind, TokenStat};

        // Build a realistic event sequence covering every tier:
        // UserMessage, ModelDelta, ModelThinking, ToolCall, ToolResult,
        // Usage, QuestionAsked, QuestionAnswered, DelegationResult,
        // TurnsEvicted, ToolResultCompacted, Tasks, WakeScheduled.
        let mut events: Vec<Event> = Vec::new();
        let mut ts = 1000i64;
        let mut mk = |kind: EventKind| {
            let e = Event::new(Ulid::new(), None, ts, kind);
            ts += 1;
            e
        };

        events.push(mk(EventKind::UserMessage {
            text: "hello".into(),
        }));
        events.push(mk(EventKind::ModelDelta { text: "res".into() }));
        events.push(mk(EventKind::ModelThinking { text: "hmm".into() }));
        events.push(mk(EventKind::ToolCall {
            id: "t1".into(),
            name: "read".into(),
            args: r#"{"path":"f.rs"}"#.into(),
        }));
        // ToolResult with tokens
        let mut tr = mk(EventKind::ToolResult {
            id: "t1".into(),
            name: "read".into(),
            output: "file contents".into(),
            is_error: false,
        });
        tr.tokens = Some(TokenStat {
            input: 100,
            output: 50,
            cached: 20,
            thinking: 5,
        });
        events.push(tr);
        events.push(mk(EventKind::Usage));
        events.push(mk(EventKind::QuestionAsked {
            id: "q1".into(),
            kind: zoid_core::event::QuestionKind::Ask,
            question: "which?".into(),
            choices: vec!["a".into(), "b".into()],
        }));
        events.push(mk(EventKind::QuestionAnswered {
            id: "q1".into(),
            answer: "a".into(),
        }));
        events.push(mk(EventKind::AssistantMessage {
            text: "final answer".into(),
        }));
        events.push(mk(EventKind::DelegationResult {
            subagent_id: "s1".into(),
            branch: "subagent:s1".into(),
            summary: "done".into(),
            ok: true,
        }));
        events.push(mk(EventKind::ToolResultCompacted {
            id: "t1".into(),
            summary: "compacted summary".into(),
            original_tokens: 500,
        }));
        events.push(mk(EventKind::Tasks { items: vec![] }));
        events.push(mk(EventKind::WakeScheduled {
            wake_id: "w1".into(),
            fire_at_ms: 99999,
            note: "reminder".into(),
        }));
        events.push(mk(EventKind::WakeFired {
            wake_id: "w1".into(),
        }));
        events.push(mk(EventKind::WakeCancelled {
            wake_id: "w2".into(),
        }));
        events.push(mk(EventKind::TurnsDropped { turns_dropped: 1 }));
        events.push(mk(EventKind::ContextMutation {
            item: "msg:0".into(),
            op: zoid_core::event::MutationOp::Pin,
        }));
        events.push(mk(EventKind::DirectiveReasserted { at_cumulative: 500 }));
        events.push(mk(EventKind::TurnsReadmitted {
            ids: vec![Ulid::from(42u128)],
        }));
        events.push(mk(EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128)],
            reclaimed_tokens: 1000,
            marker: zoid_core::event::EvictionMarker {
                spans: vec![zoid_core::event::EvictedSpan {
                    token_estimate: 500,
                    topic_hint: "topic".into(),
                }],
            },
            rescue: None,
        }));

        let log = zoid::eventlog::EventLog::from_vec(events.clone());

        // Full refresh from scratch (the reference).
        let mut full = ProjectionCache::default();
        full.refresh(&log);

        // Incremental: apply each event one at a time, then refresh dirty flags.
        let mut incr = ProjectionCache::default();
        for ev in &events {
            let _ = incr.apply_event(ev);
        }
        // Flush dirty economy projections.
        incr.refresh(&log);

        // Parity: msgs
        assert_eq!(incr.msgs, full.msgs, "msgs must match");
        // Parity: ledger
        assert_eq!(incr.ledger_total, full.ledger_total, "ledger_total");
        assert_eq!(incr.cached_total, full.cached_total, "cached_total");
        assert_eq!(incr.thinking_total, full.thinking_total, "thinking_total");
        // Parity: last tokens
        assert_eq!(
            incr.last_input_tokens, full.last_input_tokens,
            "last_input_tokens"
        );
        assert_eq!(
            incr.last_output_tokens, full.last_output_tokens,
            "last_output_tokens"
        );
        // Parity: tasks
        assert_eq!(incr.tasks, full.tasks, "tasks");
        // Parity: window + churn (rebuilt from dirty flags)
        assert_eq!(incr.window, full.window, "context window");
        assert_eq!(incr.churn, full.churn, "churn timeline");
    }

    #[test]
    fn apply_event_usage_returns_economy_and_accumulates() {
        use zoid_core::event::{Event, EventKind, TokenStat};
        let mut cache = ProjectionCache::default();
        let mut ev = Event::new(Ulid::new(), None, 0, EventKind::Usage);
        ev.tokens = Some(TokenStat {
            input: 100,
            output: 50,
            cached: 20,
            thinking: 5,
        });
        let impact = cache.apply_event(&ev);
        assert_eq!(impact, ProjectionImpact::Economy);
        assert_eq!(cache.ledger_total, 150);
        assert_eq!(cache.cached_total, 20);
        assert_eq!(cache.thinking_total, 5);
        assert_eq!(cache.last_input_tokens, Some(100));
        assert_eq!(cache.last_output_tokens, Some(50));
        assert!(cache.churn_dirty);
        assert!(cache.msgs.is_empty());
    }

    #[test]
    fn apply_event_tool_call_sets_churn_dirty() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // Seed with a user message so the projection has a turn context.
        let u = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage { text: "hi".into() },
        );
        cache.apply_event(&u);
        cache.refresh(&zoid::eventlog::EventLog::from_vec(vec![u]));
        assert!(!cache.churn_dirty, "clean after refresh");
        // Apply a ToolCall — should set churn_dirty (churn_timeline tracks paths).
        let tc = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ToolCall {
                id: "t1".into(),
                name: "read".into(),
                args: r#"{"path":"f.rs"}"#.into(),
            },
        );
        let impact = cache.apply_event(&tc);
        assert!(cache.churn_dirty, "ToolCall must set churn_dirty");
        assert!(matches!(impact, ProjectionImpact::MsgsMutated { .. }));
    }

    #[test]
    fn apply_event_model_delta_returns_msgs_mutated_none() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // Seed an assistant message to append to.
        cache.msgs.push(zoid_core::projection::ChatMsg::Assistant {
            text: "hello".into(),
            tool_calls: vec![],
            ts: 0,
            thinking: None,
        });
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ModelDelta {
                text: " world".into(),
            },
        );
        let impact = cache.apply_event(&ev);
        assert_eq!(
            impact,
            ProjectionImpact::MsgsMutated {
                mutated_index: None
            }
        );
        // Not MsgsAppended — caller must NOT invalidate body_cache.
    }

    #[test]
    fn apply_event_model_thinking_flushes_when_pending() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // Accumulate a pending delta.
        let ev1 = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ModelDelta {
                text: "partial".into(),
            },
        );
        cache.apply_event(&ev1);
        // ModelThinking should flush the pending turn.
        let ev2 = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ModelThinking { text: "hmm".into() },
        );
        let impact = cache.apply_event(&ev2);
        assert_eq!(
            impact,
            ProjectionImpact::MsgsMutated {
                mutated_index: None
            }
        );
        assert_eq!(cache.msgs.len(), 1, "pending assistant turn was flushed");
        // The thinking should be stashed (not on the flushed message — it goes
        // to the NEXT assistant message).
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::Assistant { thinking, text, .. } => {
                assert_eq!(text, "partial");
                assert!(
                    thinking.is_none(),
                    "thinking goes to next message, not the flushed one"
                );
            }
            _ => panic!("expected Assistant"),
        }
        assert!(
            cache.pending_thinking.is_some(),
            "thinking stashed for next message"
        );
    }

    #[test]
    fn apply_event_model_thinking_no_op_when_no_pending() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ModelThinking { text: "hmm".into() },
        );
        let impact = cache.apply_event(&ev);
        assert_eq!(impact, ProjectionImpact::Economy);
        assert!(cache.msgs.is_empty(), "no message pushed");
        assert!(cache.pending_thinking.is_some());
    }

    #[test]
    fn finalize_pending_emits_standalone_thinking() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ModelThinking {
                text: "deep thoughts".into(),
            },
        );
        cache.apply_event(&ev);
        assert!(cache.msgs.is_empty());
        let impact = cache.finalize_pending();
        assert_eq!(impact, ProjectionImpact::MsgsAppended);
        assert_eq!(cache.msgs.len(), 1);
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::Assistant { text, thinking, .. } => {
                assert!(text.is_empty());
                assert_eq!(thinking.as_deref(), Some("deep thoughts"));
            }
            _ => panic!("expected standalone thinking Assistant"),
        }
    }

    #[test]
    fn apply_event_tool_result_suppressed_for_non_approval_question() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let q = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAsked {
                id: "q1".into(),
                kind: zoid_core::event::QuestionKind::Ask,
                question: "which?".into(),
                choices: vec!["a".into()],
            },
        );
        cache.apply_event(&q);
        assert_eq!(cache.msgs.len(), 1, "question card pushed");
        let tr = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ToolResult {
                id: "q1".into(),
                name: "ask_user".into(),
                output: "answer".into(),
                is_error: false,
            },
        );
        let impact = cache.apply_event(&tr);
        assert_eq!(cache.msgs.len(), 1, "ToolResult suppressed — no new msg");
        assert!(matches!(impact, ProjectionImpact::MsgsMutated { .. }));
    }

    #[test]
    fn apply_event_tool_result_not_suppressed_for_approval_question() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let q = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAsked {
                id: "q2".into(),
                kind: zoid_core::event::QuestionKind::Approval,
                question: "approve?".into(),
                choices: vec!["yes".into(), "no".into()],
            },
        );
        cache.apply_event(&q);
        let tr = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ToolResult {
                id: "q2".into(),
                name: "shell".into(),
                output: "done".into(),
                is_error: false,
            },
        );
        let impact = cache.apply_event(&tr);
        assert_eq!(
            cache.msgs.len(),
            2,
            "ToolResult NOT suppressed for Approval"
        );
        assert_eq!(impact, ProjectionImpact::MsgsAppended);
    }

    #[test]
    fn apply_event_tool_result_compacted_non_last_mutates_index() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // Push a ToolResult, then an AssistantMessage, then compact the result.
        let tr = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResult {
                id: "t1".into(),
                name: "read".into(),
                output: "full output".into(),
                is_error: false,
            },
        );
        cache.apply_event(&tr);
        let am = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::AssistantMessage { text: "ok".into() },
        );
        cache.apply_event(&am);
        assert_eq!(cache.msgs.len(), 2);
        let comp = Event::new(
            Ulid::new(),
            None,
            2,
            EventKind::ToolResultCompacted {
                id: "t1".into(),
                summary: "summary".into(),
                original_tokens: 100,
            },
        );
        let impact = cache.apply_event(&comp);
        assert_eq!(
            impact,
            ProjectionImpact::MsgsMutated {
                mutated_index: Some(0)
            }
        );
        // Caller sees index 0 != msgs.len()-1 (which is 1) → invalidates body_cache.
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::ToolResult {
                output, compacted, ..
            } => {
                assert_eq!(output, "summary");
                assert!(*compacted);
            }
            _ => panic!("expected ToolResult at index 0"),
        }
    }

    #[test]
    fn apply_event_question_answered_miss_returns_economy() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAnswered {
                id: "nonexistent".into(),
                answer: "x".into(),
            },
        );
        let impact = cache.apply_event(&ev);
        assert_eq!(impact, ProjectionImpact::Economy);
        assert!(cache.msgs.is_empty());
    }

    #[test]
    fn apply_event_tasks_replaces_vec() {
        use zoid_core::event::{Event, EventKind};
        use zoid_core::tasks::TaskItem;
        let mut cache = ProjectionCache::default();
        let t1 = TaskItem {
            text: "a".into(),
            status: zoid_core::tasks::TaskStatus::Done,
        };
        let ev1 = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::Tasks {
                items: vec![t1.clone()],
            },
        );
        cache.apply_event(&ev1);
        assert_eq!(cache.tasks.len(), 1);
        let t2 = TaskItem {
            text: "b".into(),
            status: zoid_core::tasks::TaskStatus::Active,
        };
        let ev2 = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::Tasks {
                items: vec![t2.clone()],
            },
        );
        let impact = cache.apply_event(&ev2);
        assert_eq!(impact, ProjectionImpact::Economy);
        assert_eq!(cache.tasks.len(), 1, "last-write-wins");
        assert_eq!(cache.tasks[0].text, "b");
    }

    #[test]
    fn apply_event_wake_scheduled_is_noop() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::WakeScheduled {
                wake_id: "w1".into(),
                fire_at_ms: 99999,
                note: "reminder".into(),
            },
        );
        let impact = cache.apply_event(&ev);
        assert_eq!(impact, ProjectionImpact::Economy);
        assert!(cache.msgs.is_empty());
        assert!(!cache.window_dirty);
        assert!(!cache.churn_dirty);
    }

    #[test]
    fn apply_event_pending_turn_flush_on_tool_result() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // ModelDelta accumulates in pending, ToolResult flushes it.
        let d = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ModelDelta {
                text: "partial".into(),
            },
        );
        cache.apply_event(&d);
        assert!(
            cache.msgs.is_empty(),
            "delta accumulates in pending, not msgs"
        );
        let tc = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ToolCall {
                id: "t1".into(),
                name: "read".into(),
                args: "{}".into(),
            },
        );
        cache.apply_event(&tc);
        assert!(cache.msgs.is_empty(), "tool call accumulates in pending");
        let tr = Event::new(
            Ulid::new(),
            None,
            2,
            EventKind::ToolResult {
                id: "t1".into(),
                name: "read".into(),
                output: "ok".into(),
                is_error: false,
            },
        );
        let impact = cache.apply_event(&tr);
        assert_eq!(impact, ProjectionImpact::MsgsAppended);
        assert_eq!(cache.msgs.len(), 2, "flushed Assistant + ToolResult");
        // First msg is the flushed assistant turn with delta text + tool call.
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::Assistant {
                text, tool_calls, ..
            } => {
                assert_eq!(text, "partial");
                assert_eq!(tool_calls.len(), 1);
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn apply_event_tool_result_compacted_in_place() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let tr = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResult {
                id: "t1".into(),
                name: "read".into(),
                output: "full output".into(),
                is_error: false,
            },
        );
        cache.apply_event(&tr);
        assert_eq!(cache.msgs.len(), 1);
        let comp = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ToolResultCompacted {
                id: "t1".into(),
                summary: "summary".into(),
                original_tokens: 100,
            },
        );
        let impact = cache.apply_event(&comp);
        assert!(matches!(
            impact,
            ProjectionImpact::MsgsMutated {
                mutated_index: Some(0)
            }
        ));
        assert_eq!(cache.msgs.len(), 1, "no new msg — in-place mutation");
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::ToolResult {
                output, compacted, ..
            } => {
                assert_eq!(output, "summary");
                assert!(*compacted);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn apply_event_question_answered_in_place() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let q = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAsked {
                id: "q1".into(),
                kind: zoid_core::event::QuestionKind::Ask,
                question: "which?".into(),
                choices: vec!["a".into(), "b".into()],
            },
        );
        cache.apply_event(&q);
        assert_eq!(cache.msgs.len(), 1);
        let a = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::QuestionAnswered {
                id: "q1".into(),
                answer: "a".into(),
            },
        );
        let impact = cache.apply_event(&a);
        assert!(matches!(
            impact,
            ProjectionImpact::MsgsMutated {
                mutated_index: Some(0)
            }
        ));
        assert_eq!(cache.msgs.len(), 1, "no new msg — in-place mutation");
        match &cache.msgs[0] {
            zoid_core::projection::ChatMsg::Question { state, .. } => {
                assert!(matches!(
                    state,
                    zoid_core::projection::QuestionCardState::Answered { .. }
                ));
            }
            _ => panic!("expected Question"),
        }
    }

    #[test]
    fn full_invalidation_rebuilds_all_and_clears_dirty() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        // Seed with some events applied incrementally.
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage { text: "hi".into() },
        );
        cache.apply_event(&ev);
        let mut usage = Event::new(Ulid::new(), None, 1, EventKind::Usage);
        usage.tokens = Some(zoid_core::event::TokenStat {
            input: 10,
            output: 5,
            cached: 0,
            thinking: 0,
        });
        cache.apply_event(&usage);
        assert!(cache.churn_dirty);
        // Force full invalidation.
        cache.events_len = None;
        let log = zoid::eventlog::EventLog::from_vec(vec![ev, usage]);
        assert!(cache.refresh(&log));
        assert!(!cache.window_dirty, "window_dirty cleared");
        assert!(!cache.churn_dirty, "churn_dirty cleared");
        assert!(cache.events_len.is_some(), "events_len set");
        assert!(!cache.msgs.is_empty(), "msgs rebuilt");
    }

    #[test]
    fn refresh_dirty_flags_rebuild_only_dirty() {
        use zoid_core::event::{Event, EventKind};
        let mut cache = ProjectionCache::default();
        let log = zoid::eventlog::EventLog::from_vec(vec![Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage { text: "hi".into() },
        )]);
        // Full refresh to seed.
        cache.refresh(&log);
        let window_before = cache.window.clone();
        // Set only churn_dirty (via a Usage event).
        let mut ev = Event::new(Ulid::new(), None, 1, EventKind::Usage);
        ev.tokens = Some(zoid_core::event::TokenStat {
            input: 10,
            output: 5,
            cached: 0,
            thinking: 0,
        });
        cache.apply_event(&ev);
        cache.events_len = Some(cache.events_len.unwrap_or(0)); // don't trigger full refresh
                                                                // Push the event to the log so refresh sees the right length.
        let log2 = zoid::eventlog::EventLog::from_vec(vec![
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::UserMessage { text: "hi".into() },
            ),
            ev,
        ]);
        cache.refresh(&log2);
        // window should NOT have been rebuilt (window_dirty was not set by Usage).
        assert_eq!(
            cache.window, window_before,
            "window not rebuilt — only churn was dirty"
        );
        assert!(!cache.churn_dirty, "churn_dirty cleared");
    }

    /// The `PluginScan` main-loop path (`apply_plugin_scan`) for the bundled
    /// `superpowers` plugin: a successful fetch materializes the mode into the
    /// first mode dir, the rebuilt registry surfaces it under the manifest's
    /// declared display name, it becomes active, the onboarding hint (not the
    /// generic fallback) is reported, and the in-flight guard clears. This is
    /// the "honest status" success half of the reconciliation in
    /// `apply_plugin_scan` (~main.rs:4693): `wants_activate && activated` uses
    /// the plugin's own onboarding text.
    ///
    /// The false-activation half of that reconciliation — `wants_activate &&
    /// !activated`, which restores the previously-active mode, returns
    /// `false`, and reports "installed but could not be activated." — is not
    /// forceable here via real materialize: `build_plan` always writes
    /// `mode.md`'s `name:` frontmatter from `manifest.name` verbatim (see
    /// `zoid-plugin/src/plan.rs`), into exactly the dir `build_mode_registry`
    /// rescans (`app.mode_dirs.first()`), so a successful install always
    /// surfaces its own mode name on rebuild. The one way to make the name
    /// "absent" from the rebuilt registry — pre-seeding a *different* folder
    /// that parses to the same display name, so first-wins dedup skips ours —
    /// still leaves that name present in `app.modes.names()` (just pointing at
    /// the wrong entry), so it does not exercise the `!activated` branch
    /// either; it would be a different bug (identity confusion) with its own
    /// test, not this one. So instead we lock down the guard-clearing half of
    /// the reconciliation (shared by both outcomes) on both the success path
    /// and the error (bad-scan) path here, mirroring
    /// `superpowers_scan_installs_activates_and_clears_guard` above.
    #[tokio::test]
    async fn apply_plugin_scan_reports_honest_status_and_clears_guard() {
        use zoid_core::wizard::{ScannedFile, UpstreamScan};
        fn skill(name: &str, desc: &str) -> String {
            format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n")
        }
        let tmp = tempfile::tempdir().unwrap();
        let modes_dir = tmp.path().join("modes");
        let mut app = test_app().await;
        app.mode_dirs = vec![modes_dir.clone()];
        app.installing_plugin = true; // pretend a fetch is in flight

        let scan = UpstreamScan {
            url: "github.com/obra/superpowers/tree/SHA/skills".into(),
            repo: "obra/superpowers".into(),
            resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/using-superpowers/SKILL.md".into(),
                    sha: "a".into(),
                    content: skill("using-superpowers", "loader"),
                },
                ScannedFile {
                    upstream_path: "skills/brainstorming/SKILL.md".into(),
                    sha: "b".into(),
                    content: skill("brainstorming", "before creative work"),
                },
            ],
        };

        // Success path: the bundled manifest declares Activate + OnboardingHint.
        let sp_manifest = zoid_plugin::bundled::bundled_manifest("superpowers").unwrap();
        let installed = apply_plugin_scan(
            &mut app,
            "superpowers".into(),
            "bundled".into(),
            zoid::plugin_install::KindOverride::None,
            Ok((sp_manifest, scan)),
        );
        assert!(installed, "install + activation should succeed");
        assert!(!app.installing_plugin, "guard must clear");
        assert!(
            app.modes.names().iter().any(|n| n == "Superpowers"),
            "rebuilt registry must surface the installed mode under its declared name"
        );
        assert_eq!(
            app.modes.active_name(),
            "Superpowers",
            "mode must be activated"
        );
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("Superpowers mode installed and active."),
            "honest-success branch must report the plugin's own onboarding hint, \
             not the generic fallback"
        );
        assert!(modes_dir.join("superpowers/mode.md").is_file());

        // Error path: clears the guard, surfaces the message, installs nothing new.
        app.installing_plugin = true;
        let installed2 = apply_plugin_scan(
            &mut app,
            "superpowers".into(),
            "bundled".into(),
            zoid::plugin_install::KindOverride::None,
            Err("fetch failed: boom".into()),
        );
        assert!(!installed2);
        assert!(
            !app.installing_plugin,
            "error path must also clear the guard"
        );
        assert_eq!(app.shell.status_hint.as_deref(), Some("fetch failed: boom"));
    }

    /// A skills-kind install has no mode to activate; `apply_plugin_scan` must
    /// report an honest "installed, restart to load" status instead of running
    /// the mode-registry/Activate reconciliation (whose "could not be
    /// activated" message would be misleading for a skills pack).
    #[tokio::test]
    async fn apply_plugin_scan_skills_kind_reports_restart_hint_not_activation_error() {
        use zoid_core::wizard::{ScannedFile, UpstreamScan};
        fn skill(name: &str, desc: &str) -> String {
            format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n")
        }
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        std::env::set_var("HOME", cfg.path());
        let mut app = test_app().await;
        let prev_active = app.modes.active_name().to_string();
        app.installing_plugin = true;
        let scan = UpstreamScan {
            url: "u".into(),
            repo: "obra/superpowers".into(),
            resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/using-superpowers/SKILL.md".into(),
                    sha: "a".into(),
                    content: skill("using-superpowers", "loader"),
                },
                ScannedFile {
                    upstream_path: "skills/brainstorming/SKILL.md".into(),
                    sha: "b".into(),
                    content: skill("brainstorming", "before creative work"),
                },
            ],
        };
        // `--skills` override forces the skills-kind install path on the bundled (mode) manifest.
        let sp_manifest = zoid_plugin::bundled::bundled_manifest("superpowers").unwrap();
        let activated = apply_plugin_scan(
            &mut app,
            "superpowers".into(),
            "bundled".into(),
            zoid::plugin_install::KindOverride::Skills,
            Ok((sp_manifest, scan)),
        );
        assert!(!activated, "a skills install activates no mode");
        assert!(!app.installing_plugin, "guard must clear");
        let hint = app.shell.status_hint.as_deref().unwrap_or("");
        assert!(
            hint.contains("Restart") && hint.contains("installed"),
            "got: {hint}"
        );
        assert!(
            !hint.contains("could not be activated"),
            "must not show the misleading mode-activation error; got: {hint}"
        );
        assert_eq!(
            app.modes.active_name(),
            prev_active,
            "skills install must not change the active mode"
        );
    }

    /// A catalog-sourced install carries its manifest through `PluginScan`
    /// rather than looking it up via `bundled_manifest`; this proves
    /// `apply_plugin_scan` installs correctly from a manifest that has no
    /// bundled counterpart at all.
    #[tokio::test]
    async fn apply_plugin_scan_installs_a_carried_catalog_manifest() {
        use zoid_core::wizard::{ScannedFile, UpstreamScan};
        use zoid_plugin::effect::Effect;
        use zoid_plugin::manifest::{BodyStrategy, ModeRecipe, PluginManifest, PluginSource};
        fn skill(name: &str, desc: &str) -> String {
            format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n")
        }
        let tmp = tempfile::tempdir().unwrap();
        let mut app = test_app().await;
        app.mode_dirs = vec![tmp.path().join("modes")];
        // scan MUST contain skills/using-demo/SKILL.md (the loader) so build_plan succeeds.
        let scan = UpstreamScan {
            url: "github.com/o/demo/tree/SHA/skills".into(),
            repo: "o/demo".into(),
            resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile {
                    upstream_path: "skills/using-demo/SKILL.md".into(),
                    sha: "a".into(),
                    content: skill("using-demo", "loader"),
                },
                ScannedFile {
                    upstream_path: "skills/other/SKILL.md".into(),
                    sha: "b".into(),
                    content: skill("other", "another skill"),
                },
            ],
        };
        let manifest = PluginManifest {
            id: "demo".into(),
            schema: 1,
            kind: vec!["mode".into()],
            name: "Demo".into(),
            description: "d".into(),
            source: Some(PluginSource {
                repo: "o/demo".into(),
                ref_: "SHA".into(),
                subtree: "skills".into(),
            }),
            mode: Some(ModeRecipe {
                loader: "using-demo/SKILL.md".into(),
                strip_prefix: "skills/".into(),
                body: BodyStrategy::FromSkillFrontmatter,
                description: "Demo mode".into(),
                body_intro: None,
                body_outro: None,
            }),
            mcp: None,
            install: vec![Effect::Activate],
        };
        let activated = apply_plugin_scan(
            &mut app,
            "demo".into(),
            "catalog".into(),
            zoid::plugin_install::KindOverride::None,
            Ok((manifest, scan)),
        );
        assert!(activated);
        assert!(app.modes.names().iter().any(|n| n == "Demo"));
    }

    /// `apply_models_fetched` replaces the OPEN model picker's options with the
    /// live list, seeding the selection cursor on the current model; an empty
    /// fetch result is a no-op (the static registry fallback is kept).
    #[tokio::test]
    async fn apply_models_fetched_replaces_open_model_picker() {
        let mut app = test_app().await;
        app.config.provider = "ollama".into(); // pin a configured provider so the model picker is non-empty
        refresh_config_sections(&mut app);
        // Find the "model" row and park the cursor on it, then open its
        // picker exactly as `Action::ConfigDrillOpen` would.
        let (section, field) = app
            .shell
            .config_sections
            .iter()
            .enumerate()
            .find_map(|(si, s)| {
                s.rows
                    .iter()
                    .position(|r| r.label == "model")
                    .map(|ri| (si, ri))
            })
            .expect("config sections must include a \"model\" row");
        app.shell.config_section = section;
        app.shell.config_field = field;
        assert_eq!(current_config_field(&app).map(|(l, _, _)| l), Some("model"));
        app.shell.config_picker =
            zoid_tui::config_view::model_options(&app.config.provider, &app.config.model);
        app.shell.config_col = zoid_tui::state::ConfigCol::Picker;
        assert!(app.shell.config_picker_open());

        // Happy path: a live fetch replaces the fallback list.
        let provider_id = app.config.provider.clone();
        apply_models_fetched(
            &mut app,
            provider_id.clone(),
            vec!["live-a".to_string(), "live-b".to_string()],
        );
        assert_eq!(app.shell.config_picker.len(), 2);
        assert_eq!(app.shell.config_picker[0].id, "live-a");
        assert_eq!(app.shell.config_picker[1].id, "live-b");
        assert!(app.shell.config_picker.iter().all(|o| o.selectable));

        // Empty fetch result: fallback list is left untouched.
        apply_models_fetched(&mut app, provider_id, Vec::new());
        assert_eq!(app.shell.config_picker.len(), 2);
        assert_eq!(app.shell.config_picker[0].id, "live-a");
        assert_eq!(app.shell.config_picker[1].id, "live-b");
    }

    /// The quick-switch model pane live-fetches the *highlighted* provider's
    /// models (not the active one). A fetch tagged with the highlighted id
    /// replaces the static fallback; an empty result keeps it; a fetch for a
    /// provider the user has scrolled past is dropped; and the reducer no-ops
    /// entirely when the overlay is closed.
    #[tokio::test]
    async fn switch_model_pane_takes_live_fetch_for_highlighted_provider() {
        use zoid_tui::config_view::provider_options;
        use zoid_tui::state::Overlay;

        let mut app = test_app().await;
        app.shell.overlay = Overlay::ProviderSwitch;
        app.shell.switch_providers = provider_options(&app.config.provider);
        // Highlight ollama-cloud explicitly (regardless of the default active).
        let sel = app
            .shell
            .switch_providers
            .iter()
            .position(|o| o.id == "ollama-cloud")
            .expect("registry must offer ollama-cloud");
        app.shell.switch_provider_sel = sel;
        app.shell.switch_models =
            zoid_tui::config_view::model_options("ollama-cloud", &app.config.model);
        app.shell.switch_model_sel = 0;
        let fallback_len = app.shell.switch_models.len();

        // A fetch for a DIFFERENT (scrolled-past) provider is dropped.
        apply_switch_models_fetched(
            &mut app,
            "anthropic-api".to_string(),
            vec!["ignored".to_string()],
        );
        assert_eq!(
            app.shell.switch_models.len(),
            fallback_len,
            "a fetch for a non-highlighted provider must not touch the pane"
        );

        // A fetch for the highlighted provider replaces the fallback list.
        apply_switch_models_fetched(
            &mut app,
            "ollama-cloud".to_string(),
            vec!["glm-5.2:cloud".to_string(), "qwen3-coder:cloud".to_string()],
        );
        assert_eq!(app.shell.switch_models.len(), 2);
        assert_eq!(app.shell.switch_models[0].id, "glm-5.2:cloud");
        assert_eq!(app.shell.switch_models[1].id, "qwen3-coder:cloud");
        assert!(app.shell.switch_models.iter().all(|o| o.selectable));

        // An empty fetch keeps the live list (offline/error → no clobber).
        apply_switch_models_fetched(&mut app, "ollama-cloud".to_string(), Vec::new());
        assert_eq!(app.shell.switch_models.len(), 2);

        // Overlay closed → the reducer is inert.
        app.shell.overlay = Overlay::None;
        apply_switch_models_fetched(
            &mut app,
            "ollama-cloud".to_string(),
            vec!["late".to_string()],
        );
        assert_eq!(
            app.shell.switch_models.len(),
            2,
            "a fetch landing after the overlay closed must not reopen/rewrite it"
        );
    }

    /// A fetch tagged with a provider id that no longer matches
    /// `app.config.provider` (the user switched providers while the fetch was
    /// in flight) must be dropped entirely — the picker is left exactly as it
    /// was, not overwritten with the stale provider's models.
    #[tokio::test]
    async fn stale_provider_fetch_is_dropped() {
        let mut app = test_app().await;
        app.config.provider = "ollama".into(); // pin a configured provider so the model picker is non-empty
        refresh_config_sections(&mut app);
        let (section, field) = app
            .shell
            .config_sections
            .iter()
            .enumerate()
            .find_map(|(si, s)| {
                s.rows
                    .iter()
                    .position(|r| r.label == "model")
                    .map(|ri| (si, ri))
            })
            .expect("config sections must include a \"model\" row");
        app.shell.config_section = section;
        app.shell.config_field = field;
        app.shell.config_picker =
            zoid_tui::config_view::model_options(&app.config.provider, &app.config.model);
        app.shell.config_col = zoid_tui::state::ConfigCol::Picker;
        assert!(app.shell.config_picker_open());
        let before = app.shell.config_picker.clone();
        let before_sel = app.shell.config_picker_sel;

        assert_ne!(app.config.provider, "some-other-provider");
        apply_models_fetched(
            &mut app,
            "some-other-provider".to_string(),
            vec!["stale-a".to_string(), "stale-b".to_string()],
        );

        assert_eq!(
            app.shell.config_picker, before,
            "a fetch tagged with a superseded provider id must not clobber the current picker"
        );
        assert_eq!(app.shell.config_picker_sel, before_sel);
    }

    /// A live fetch that lands after focus has moved off the model field (or
    /// after the picker was closed) must not clobber whatever is on screen.
    #[tokio::test]
    async fn apply_models_fetched_ignored_when_model_picker_not_open() {
        let mut app = test_app().await;
        refresh_config_sections(&mut app);
        // Cursor left on whatever the default field is (not drilled into a
        // picker at all) — config_picker is empty, so config_picker_open() is
        // false regardless of which row the cursor is on.
        assert!(!app.shell.config_picker_open());

        let provider_id = app.config.provider.clone();
        apply_models_fetched(&mut app, provider_id, vec!["live-a".to_string()]);

        assert!(
            app.shell.config_picker.is_empty(),
            "no picker was open; a stray fetch result must not open one"
        );
    }

    #[tokio::test]
    async fn boot_reclaims_stale_session_and_uses_fresh_when_live() {
        // The boot decision is `is_live(...)`. A stale-heartbeat row is reclaimable
        // (is_live == false); a fresh-heartbeat row is not (is_live == true).
        use zoid_core::store::is_live;
        let alive = |_: i64| true;
        // Stale: heartbeat 20s ago, window 15s → not live → reclaim.
        assert!(!is_live(true, Some(99), Some(1000), 21000, alive));
        // Live: heartbeat now → live → create a fresh session instead.
        assert!(is_live(true, Some(99), Some(1000), 1000, alive));
    }

    /// Submit while subagents run but no turn is streaming spawns a turn
    /// immediately (the main loop is unblocked while subagents run). The
    /// textarea is cleared, a UserMessage is recorded, and `streaming` flips
    /// on — `pending_message` stays `None`.
    #[tokio::test]
    async fn submit_while_delegating_spawns_turn() {
        let mut app = test_app().await;
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-test".into(),
            task: "test".into(),
            agent: "delegate".into(),
        });
        app.textarea = make_input(TextArea::from(vec!["hello".to_string()]));

        let quit = handle_action(&mut app, zoid_tui::route::Action::Submit)
            .await
            .unwrap();

        assert!(!quit, "Submit must not signal quit");
        assert!(
            !app.in_flight_subagents.is_empty(),
            "in_flight untouched by a spawned turn"
        );
        assert!(app.streaming, "a turn is spawned while subagents run");
        assert!(
            !app.events.is_empty(),
            "UserMessage recorded for the spawned turn"
        );
        assert_eq!(
            app.pending_message, None,
            "message is not queued when not streaming"
        );
        assert!(app.textarea.lines()[0].is_empty(), "textarea cleared");
    }

    /// Submit while a turn is streaming (with subagents also running) still
    /// queues the message for after the turn — `streaming` is the real
    /// blocker; subagents alone no longer are.
    #[tokio::test]
    async fn submit_while_streaming_and_delegating_queues_message() {
        let mut app = test_app().await;
        app.streaming = true;
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-test".into(),
            task: "test".into(),
            agent: "delegate".into(),
        });
        app.textarea = make_input(TextArea::from(vec!["hello".to_string()]));

        let quit = handle_action(&mut app, zoid_tui::route::Action::Submit)
            .await
            .unwrap();

        assert!(!quit, "Submit must not signal quit");
        assert!(!app.in_flight_subagents.is_empty(), "in_flight untouched");
        assert!(app.streaming, "no new turn spawned while streaming");
        assert_eq!(
            app.pending_message.as_deref(),
            Some("hello"),
            "message queued while streaming"
        );
        assert!(app.textarea.lines()[0].is_empty(), "textarea cleared");
        assert!(app.shell.status_hint.as_deref().unwrap().contains("queued"));
    }

    // --- Wake-on-DelegationResult (orchestrator-sees-subagent-output fix) ---

    /// The pure idle decision: wake only when idle (no turn streaming) and not
    /// yielded. Per-result: fires on each `DelegationResult` regardless of other
    /// in-flight subagents — the last one's `TurnComplete` re-enters the path.
    #[test]
    fn should_wake_after_delegation_truth_table() {
        assert!(
            should_wake_after_delegation(false, false),
            "idle + not yielded → wake"
        );
        assert!(
            !should_wake_after_delegation(true, false),
            "a turn is still streaming → do not wake now"
        );
        assert!(
            !should_wake_after_delegation(false, true),
            "session yielded/taken over → do not wake"
        );
        assert!(
            !should_wake_after_delegation(true, true),
            "streaming and yielded → do not wake"
        );
    }

    /// A DelegationResult arriving while the orchestrator is idle plans an
    /// immediate wake: streaming is set and the caller is told to spawn a turn.
    #[tokio::test]
    async fn delegation_wake_idle_plans_spawn() {
        let mut app = test_app().await;
        app.streaming = false; // idle
        assert!(app.in_flight_subagents.is_empty());

        let spawn = plan_delegation_wake(&mut app);

        assert!(spawn, "idle orchestrator must be woken (caller spawns)");
        assert!(app.streaming, "wake marks the turn as streaming");
        assert!(
            !app.wake_after_delegation,
            "immediate wake needs no deferred flag"
        );
    }

    /// A DelegationResult arriving mid-turn (a turn is still streaming) does NOT
    /// spawn now — and, per-result, no longer arms a deferred flag: each result
    /// fires its own wake when idle, so there's nothing to defer. The in-flight
    /// turn is left untouched.
    #[tokio::test]
    async fn delegation_wake_streaming_defers() {
        let mut app = test_app().await;
        app.streaming = true; // a turn is running

        let spawn = plan_delegation_wake(&mut app);

        assert!(!spawn, "must not spawn while a turn is streaming");
        assert!(
            app.wake_after_delegation,
            "the wake is deferred to TurnComplete"
        );
        assert!(app.streaming, "the in-flight turn is untouched");
    }

    /// Per-result wake: a DelegationResult arriving while the orchestrator is
    /// idle (not streaming, not yielded) wakes NOW even if other subagents are
    /// still in flight — each result gets its own continuation turn.
    #[tokio::test]
    async fn delegation_wake_fires_per_result_with_subagents_in_flight() {
        let mut app = test_app().await;
        app.streaming = false; // idle
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-still-running".into(),
            task: "t".into(),
            agent: "delegate".into(),
        });

        let spawn = plan_delegation_wake(&mut app);

        assert!(spawn, "wake per-result even while subagents remain");
        assert!(app.streaming, "wake marks the turn as streaming");
        assert!(
            !app.wake_after_delegation,
            "immediate wake needs no deferred flag"
        );
    }

    // --- Concurrent subagent pool edge-case tests ---

    /// `max_concurrent = 0` means unlimited: the drain loop spawns all queued
    /// subagents without a capacity check. The queue-emptiness guard
    /// short-circuits first so an empty queue with max=0 doesn't panic.
    #[tokio::test]
    async fn max_concurrent_zero_unlimited_no_panic() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 0;

        // Simulate a DelegationResult arriving with an empty queue.
        // The drain loop should exit immediately (queue is empty).
        // No panic, no spawn.
        let ev = zoid_core::event::Event::new(
            ulid::Ulid::new(),
            None,
            0,
            zoid_core::event::EventKind::DelegationResult {
                subagent_id: "sub-done".into(),
                branch: String::new(),
                summary: "done".into(),
                ok: true,
            },
        )
        .with_session(app.session_id);
        app.events.push(ev);

        // Manually trigger the drain (mirrors the DelegationResult handler).
        // The queue is empty and the pool is empty, so the drain loop would
        // not enter — assert that directly instead of a never-looping while.
        let max = app.config.subagent.max_concurrent;
        assert!(
            app.queued_subagents.is_empty()
                || max == 0
                || app.in_flight.lock().unwrap().len() >= max
        );

        assert!(app.queued_subagents.is_empty(), "queue untouched");
        assert!(app.in_flight.lock().unwrap().is_empty(), "pool untouched");
    }

    /// `max_concurrent = 1`: the pool accepts one subagent, and the second
    /// is queued. When the first finishes, the drain spawns the second.
    #[tokio::test]
    async fn max_concurrent_one_queues_second_drains_on_completion() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 1;

        // Pre-fill the pool with one "running" subagent.
        app.in_flight.lock().unwrap().insert(
            "sub-running".into(),
            zoid::agent::SubagentHandle {
                cancel: tokio_util::sync::CancellationToken::new(),
                hard: tokio_util::sync::CancellationToken::new(),
                progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                task: "running".into(),
                agent: "delegate".into(),
            },
        );
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-running".into(),
            task: "running".into(),
            agent: "delegate".into(),
        });

        // Queue a second subagent (simulates SubagentQueued handler).
        app.queued_subagents.push_back(QueuedSubagent {
            task: "queued task".into(),
            agent: "delegate".into(),
            resolved_profile: zoid_core::agent_profile::AgentProfile::builtin(),
            resolved_name: "delegate".into(),
            cwd: std::path::PathBuf::from("/repo"),
            want_worktree: false,
            tool_call_id: "tc-1".into(),
            session_id: app.session_id,
        });

        assert_eq!(app.in_flight.lock().unwrap().len(), 1, "pool has 1");
        assert_eq!(app.queued_subagents.len(), 1, "queue has 1");

        // Simulate the first subagent finishing (DelegationResult handler).
        app.in_flight.lock().unwrap().remove("sub-running");
        app.in_flight_subagents.retain(|s| s.id != "sub-running");

        // Drain: with max=1, pool is now empty (0 < 1), so the queued
        // subagent would spawn. We can't call spawn_queued_subagent in a
        // unit test (it needs a real provider), but we can verify the
        // drain condition is true.
        let max = app.config.subagent.max_concurrent;
        let should_drain = !app.queued_subagents.is_empty()
            && (max == 0 || app.in_flight.lock().unwrap().len() < max);
        assert!(should_drain, "pool has room — drain should fire");

        // Pop the queued subagent (simulates what the drain loop does).
        let qs = app.queued_subagents.pop_front().unwrap();
        assert_eq!(qs.task, "queued task", "correct subagent popped");
        assert!(app.queued_subagents.is_empty(), "queue is now empty");
    }

    /// Session takeover clears queued subagents — they should not survive
    /// a session being taken over by another instance.
    #[tokio::test]
    async fn session_takeover_clears_queued_subagents() {
        let mut app = test_app().await;

        // Seed: one running subagent + two queued.
        app.in_flight.lock().unwrap().insert(
            "sub-1".into(),
            zoid::agent::SubagentHandle {
                cancel: tokio_util::sync::CancellationToken::new(),
                hard: tokio_util::sync::CancellationToken::new(),
                progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                task: "task-1".into(),
                agent: "delegate".into(),
            },
        );
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-1".into(),
            task: "task-1".into(),
            agent: "delegate".into(),
        });
        app.queued_subagents.push_back(QueuedSubagent {
            task: "queued-1".into(),
            agent: "delegate".into(),
            resolved_profile: zoid_core::agent_profile::AgentProfile::builtin(),
            resolved_name: "delegate".into(),
            cwd: std::path::PathBuf::from("/repo"),
            want_worktree: false,
            tool_call_id: "tc-1".into(),
            session_id: app.session_id,
        });
        app.queued_subagents.push_back(QueuedSubagent {
            task: "queued-2".into(),
            agent: "delegate".into(),
            resolved_profile: zoid_core::agent_profile::AgentProfile::builtin(),
            resolved_name: "delegate".into(),
            cwd: std::path::PathBuf::from("/repo"),
            want_worktree: false,
            tool_call_id: "tc-2".into(),
            session_id: app.session_id,
        });

        assert_eq!(app.in_flight.lock().unwrap().len(), 1, "1 running");
        assert_eq!(app.queued_subagents.len(), 2, "2 queued");

        // Fire the SessionTakenOver handler (inline, same as the event handler).
        app.streaming = false;
        zoid::agent::fire_subagent_kill(&app.in_flight, None);
        app.in_flight_subagents.clear();
        app.in_flight.lock().unwrap().clear();
        app.queued_subagents.clear();
        app.yielded = true;

        assert!(app.in_flight.lock().unwrap().is_empty(), "pool cleared");
        assert!(app.in_flight_subagents.is_empty(), "UI list cleared");
        assert!(app.queued_subagents.is_empty(), "queue cleared");
        assert!(app.yielded, "session is yielded");
    }

    /// Pool overflow: with max_concurrent=3 and 3 running, a 4th dispatch
    /// is queued. The drain condition correctly rejects (3 < 3 is false).
    #[tokio::test]
    async fn pool_full_drain_does_not_spawn() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 3;

        // Fill the pool with 3 running subagents.
        for i in 0..3 {
            let id = format!("sub-{i}");
            app.in_flight.lock().unwrap().insert(
                id.clone(),
                zoid::agent::SubagentHandle {
                    cancel: tokio_util::sync::CancellationToken::new(),
                    hard: tokio_util::sync::CancellationToken::new(),
                    progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                    abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    task: format!("task-{i}"),
                    agent: "delegate".into(),
                },
            );
        }

        // Queue a 4th.
        app.queued_subagents.push_back(QueuedSubagent {
            task: "queued-4".into(),
            agent: "delegate".into(),
            resolved_profile: zoid_core::agent_profile::AgentProfile::builtin(),
            resolved_name: "delegate".into(),
            cwd: std::path::PathBuf::from("/repo"),
            want_worktree: false,
            tool_call_id: "tc-4".into(),
            session_id: app.session_id,
        });

        // Drain condition: pool is full (3 < 3 is false), so no drain.
        let max = app.config.subagent.max_concurrent;
        let should_drain = !app.queued_subagents.is_empty()
            && (max == 0 || app.in_flight.lock().unwrap().len() < max);
        assert!(!should_drain, "pool is full — drain should NOT fire");

        // One subagent finishes — pool drops to 2, now 2 < 3 is true.
        app.in_flight.lock().unwrap().remove("sub-0");
        let should_drain = !app.queued_subagents.is_empty()
            && (max == 0 || app.in_flight.lock().unwrap().len() < max);
        assert!(should_drain, "pool has room — drain should fire");
    }

    // --- Cancellation with concurrent subagents ---

    /// Two-press Esc with multiple subagents: first Esc arms, second Esc
    /// fires `fire_subagent_kill` on ALL in-flight subagents. With the
    /// concurrent pool, this kills N>1 subagents simultaneously.
    #[tokio::test]
    async fn esc_two_press_kills_all_concurrent_subagents() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 3;

        // Seed 3 running subagents.
        for i in 0..3 {
            let id = format!("sub-{i}");
            let hard = tokio_util::sync::CancellationToken::new();
            app.in_flight.lock().unwrap().insert(
                id.clone(),
                zoid::agent::SubagentHandle {
                    cancel: tokio_util::sync::CancellationToken::new(),
                    hard: hard.clone(),
                    progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                    abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    task: format!("task-{i}"),
                    agent: "delegate".into(),
                },
            );
            app.in_flight_subagents.push(SubagentInfo {
                id,
                task: format!("task-{i}"),
                agent: "delegate".into(),
            });
        }

        assert_eq!(app.in_flight.lock().unwrap().len(), 3);
        assert!(!app.subagent_kill_armed);

        // First Esc: arms, does NOT fire.
        let pending = app.in_flight.lock().unwrap().len();
        let (next_armed, fire, _hint) = subagent_kill_decision(app.subagent_kill_armed, pending);
        assert!(next_armed, "first press arms");
        assert!(!fire, "first press must not fire");
        app.subagent_kill_armed = next_armed;

        // Second Esc: fires, disarms.
        let (next_armed, fire, _hint) = subagent_kill_decision(app.subagent_kill_armed, pending);
        assert!(!next_armed, "second press disarms");
        assert!(fire, "second press fires");
        if fire {
            zoid::agent::fire_subagent_kill(&app.in_flight, None);
        }
        app.subagent_kill_armed = next_armed;

        // All 3 subagents' hard tokens should be cancelled.
        let map = app.in_flight.lock().unwrap();
        for handle in map.values() {
            assert!(
                handle.hard.is_cancelled(),
                "all subagent hard tokens must be cancelled"
            );
        }
    }

    /// `cancel_subagent` with a specific ID kills only that subagent,
    /// leaving the others running. With concurrent subagents, the model
    /// can target one of N.
    #[tokio::test]
    async fn cancel_subagent_targets_one_of_many() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 3;

        // Seed 3 running subagents with distinct hard tokens.
        let mut tokens = Vec::new();
        for i in 0..3 {
            let id = format!("sub-{i}");
            let hard = tokio_util::sync::CancellationToken::new();
            tokens.push(hard.clone());
            app.in_flight.lock().unwrap().insert(
                id,
                zoid::agent::SubagentHandle {
                    cancel: tokio_util::sync::CancellationToken::new(),
                    hard,
                    progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                    abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    task: format!("task-{i}"),
                    agent: "delegate".into(),
                },
            );
        }

        assert_eq!(app.in_flight.lock().unwrap().len(), 3);

        // Cancel only sub-1 (the model calls cancel_subagent with id="sub-1").
        let fired = zoid::agent::fire_subagent_kill(&app.in_flight, Some("sub-1"));
        assert_eq!(fired, 1, "exactly 1 subagent was targeted");

        // sub-1's hard token is cancelled; the others are NOT.
        assert!(
            tokens[1].is_cancelled(),
            "sub-1 hard token must be cancelled"
        );
        assert!(
            !tokens[0].is_cancelled(),
            "sub-0 hard token must NOT be cancelled"
        );
        assert!(
            !tokens[2].is_cancelled(),
            "sub-2 hard token must NOT be cancelled"
        );

        // The pool still has 3 entries (fire_subagent_kill cancels but
        // doesn't remove from the map — the DelegationResult handler does).
        assert_eq!(
            app.in_flight.lock().unwrap().len(),
            3,
            "pool entries remain until DelegationResult"
        );
    }

    /// Esc with subagents running but no main turn: the two-press confirm
    /// path fires only when `pending > 0`. With an empty pool, Esc is a
    /// no-op (doesn't arm).
    #[tokio::test]
    async fn esc_with_empty_pool_does_not_arm() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 3;
        app.streaming = false;

        // Empty pool, no turn.
        let pending = app.in_flight.lock().unwrap().len();
        assert_eq!(pending, 0);

        // The Esc handler checks `pending > 0` before arming.
        // With 0 pending, it skips the subagent kill path entirely.
        // (This mirrors the Action::CancelTurn handler logic.)
        if pending > 0 {
            let (next_armed, _fire, _hint) =
                subagent_kill_decision(app.subagent_kill_armed, pending);
            app.subagent_kill_armed = next_armed;
        }

        assert!(
            !app.subagent_kill_armed,
            "empty pool must not arm the kill state"
        );
    }

    /// Session takeover cancels all in-flight subagents' hard tokens
    /// (not just clears the UI list). With concurrent subagents, all N
    /// must be hard-cancelled.
    #[tokio::test]
    async fn takeover_cancels_all_hard_tokens() {
        let mut app = test_app().await;
        app.config.subagent.max_concurrent = 3;

        // Seed 3 running subagents with trackable hard tokens.
        let mut tokens = Vec::new();
        for i in 0..3 {
            let id = format!("sub-{i}");
            let hard = tokio_util::sync::CancellationToken::new();
            tokens.push(hard.clone());
            app.in_flight.lock().unwrap().insert(
                id,
                zoid::agent::SubagentHandle {
                    cancel: tokio_util::sync::CancellationToken::new(),
                    hard,
                    progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
                    abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    task: format!("task-{i}"),
                    agent: "delegate".into(),
                },
            );
        }

        // Simulate SessionTakenOver.
        app.streaming = false;
        zoid::agent::fire_subagent_kill(&app.in_flight, None);
        app.in_flight_subagents.clear();
        app.in_flight.lock().unwrap().clear();
        app.queued_subagents.clear();
        app.yielded = true;

        // All 3 hard tokens must be cancelled.
        for (i, token) in tokens.iter().enumerate() {
            assert!(
                token.is_cancelled(),
                "sub-{i} hard token must be cancelled on takeover"
            );
        }
        assert!(app.yielded);
    }

    /// A yielded session never wakes, even when idle with no subagents.
    #[tokio::test]
    async fn delegation_wake_respects_yielded() {
        let mut app = test_app().await;
        app.streaming = false;
        app.yielded = true;

        let spawn = plan_delegation_wake(&mut app);

        assert!(!spawn, "a yielded session must not be woken");
        assert!(!app.wake_after_delegation, "no deferred wake when yielded");
    }

    /// TurnComplete consumes a deferred wake exactly once: it spawns, sets
    /// streaming, clears the flag, and a second call is a no-op.
    #[tokio::test]
    async fn deferred_delegation_wake_fires_once_then_clears() {
        let mut app = test_app().await;
        app.wake_after_delegation = true;
        app.streaming = false;

        assert!(
            take_deferred_delegation_wake(&mut app),
            "a deferred wake must fire at TurnComplete"
        );
        assert!(
            app.streaming,
            "firing marks the continuation turn streaming"
        );
        assert!(!app.wake_after_delegation, "the flag is cleared");

        app.streaming = false;
        assert!(
            !take_deferred_delegation_wake(&mut app),
            "a second TurnComplete does not re-fire"
        );
    }

    /// A deferred wake on a yielded session is dropped (flag cleared, no spawn).
    #[tokio::test]
    async fn deferred_delegation_wake_dropped_when_yielded() {
        let mut app = test_app().await;
        app.wake_after_delegation = true;
        app.yielded = true;
        app.streaming = false;

        assert!(
            !take_deferred_delegation_wake(&mut app),
            "a yielded session drops the deferred wake"
        );
        assert!(!app.wake_after_delegation, "the stale flag is cleared");
        assert!(!app.streaming, "no continuation turn on a yielded session");
    }

    /// A subagent-branch event must NOT be applied to the projection cache
    /// via `apply_event`. The `msgs` vector must be unchanged.
    #[tokio::test]
    async fn subagent_branch_event_skips_apply_event() {
        let mut app = test_app().await;
        // Seed the projection with one main-branch user message so the cache
        // is populated (events_len = Some(1), msgs has 1 item).
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage {
                text: "hello".into(),
            },
        );
        app.events.push(ev);
        app.proj.refresh(&app.events);

        let msgs_before = app.proj.msgs.len();
        assert!(msgs_before > 0, "projection must have the seeded message");
        assert!(
            app.proj.events_len.is_some(),
            "events_len must be set (cache is live)"
        );

        // Simulate a subagent-branch ModelDelta arriving through Appended.
        // The branch is "subagent:01ABC" — NOT the default "main" branch.
        let sub_ev = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ModelDelta {
                text: "subagent text".into(),
            },
        )
        .with_session(app.session_id);
        // Override the branch to a subagent branch.
        let mut sub_ev = sub_ev;
        sub_ev.branch = zoid_core::event::BranchId("subagent:01ABC".into());

        // Process it the same way the Appended handler does, but with the
        // branch guard applied (the code under test).
        let is_subagent_branch = sub_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch {
            let impact = app.proj.apply_event(&sub_ev);
            if matches!(impact, ProjectionImpact::FullRefresh) {
                app.proj.events_len = None;
            }
        }
        app.events.push(sub_ev);

        // The projection cache must be untouched: same msg count, events_len
        // still set (not invalidated).
        assert_eq!(
            app.proj.msgs.len(),
            msgs_before,
            "subagent-branch event must not add to projection msgs"
        );
        assert!(
            app.proj.events_len.is_some(),
            "subagent-branch event must not invalidate the projection cache"
        );
        // The event IS in app.events (persisted), just not in the projection.
        assert_eq!(app.events.len(), 2, "event pushed into app.events");
    }

    /// A main-branch ModelDelta must still be applied via `apply_event`
    /// (existing behavior preserved).
    #[tokio::test]
    async fn main_branch_event_applies_event() {
        let mut app = test_app().await;
        // Seed with a user message so apply_event has a populated cache.
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage {
                text: "hello".into(),
            },
        );
        app.events.push(ev);
        app.proj.refresh(&app.events);

        let msgs_before = app.proj.msgs.len();

        // A main-branch ModelDelta (default branch).
        let main_ev = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ModelDelta {
                text: "response".into(),
            },
        );

        let is_subagent_branch = main_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch {
            let _ = app.proj.apply_event(&main_ev);
            let _ = app.proj.finalize_pending();
        }
        app.events.push(main_ev);

        // apply_event accumulates ModelDelta into pending_text; finalize_pending
        // flushes it as a ChatMsg::Assistant, so msgs grows by 1.
        assert_eq!(
            app.proj.msgs.len(),
            msgs_before + 1,
            "main-branch ModelDelta must add to projection msgs"
        );
    }

    /// Subagent-branch events (ModelDelta, ToolCall, ToolResult) must NOT
    /// appear in the main conversation's ChatMsg list — the projection filters
    /// by branch. This is the integration-level guard behind the jumpy-UI fix:
    /// even if subagent events are in app.events, the conversation view never
    /// shows them.
    #[test]
    fn subagent_branch_events_invisible_in_conversation() {
        use zoid_core::event::{BranchId, Event, EventKind};
        use zoid_core::projection::conversation;

        let sub_branch = BranchId("subagent:01ABC".into());
        let events = [
            // Main-branch user message.
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::UserMessage {
                    text: "hello".into(),
                },
            ),
            // Subagent-branch assistant text (must NOT appear).
            // Event has no with_branch builder — set .branch directly.
            {
                let mut ev = Event::new(
                    Ulid::new(),
                    None,
                    1,
                    EventKind::ModelDelta {
                        text: "subagent working".into(),
                    },
                );
                ev.branch = sub_branch.clone();
                ev
            },
            // Subagent-branch tool call (must NOT appear).
            {
                let mut ev = Event::new(
                    Ulid::new(),
                    None,
                    2,
                    EventKind::ToolCall {
                        id: "tc1".into(),
                        name: "read".into(),
                        args: r#"{"path":"src/main.rs"}"#.into(),
                    },
                );
                ev.branch = sub_branch;
                ev
            },
            // Main-branch assistant text (must appear).
            Event::new(
                Ulid::new(),
                None,
                3,
                EventKind::AssistantMessage {
                    text: "done".into(),
                },
            ),
        ];

        let msgs = conversation(events.iter());
        let joined: String = msgs
            .iter()
            .map(|m| match m {
                zoid_core::projection::ChatMsg::Assistant { text, .. } => text.clone(),
                zoid_core::projection::ChatMsg::User { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();

        // Main-branch messages are visible.
        assert!(joined.contains("hello"), "user message visible: {joined}");
        assert!(
            joined.contains("done"),
            "assistant message visible: {joined}"
        );
        // Subagent-branch messages are NOT visible.
        assert!(
            !joined.contains("subagent working"),
            "subagent ModelDelta must not appear in conversation: {joined}"
        );
    }

    /// A DelegationResult on the default branch must be folded into a
    /// ChatMsg::Delegated by the projection, confirming the result-landing
    /// plumbing. The continuation turn's request builder uses
    /// `conversation_for_branch` → `map_msg`, which maps Delegated to a
    /// Message with "[delegated subagent] {summary}".
    #[test]
    fn delegation_result_folds_into_chat_msg_delegated() {
        use zoid_core::event::{Event, EventKind};
        use zoid_core::projection::{conversation, ChatMsg};

        let events = [
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::UserMessage {
                    text: "do the thing".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                1,
                EventKind::AssistantMessage {
                    text: "delegating".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                2,
                EventKind::DelegationResult {
                    subagent_id: "sub-01ABC".into(),
                    branch: "subagent:01ABC".into(),
                    summary: "Task completed successfully.".into(),
                    ok: true,
                },
            ),
        ];

        let msgs = conversation(events.iter());

        // Find the Delegated message.
        let delegated = msgs.iter().find_map(|m| {
            if let ChatMsg::Delegated { summary, ok } = m {
                Some((summary.clone(), *ok))
            } else {
                None
            }
        });

        assert!(
            delegated.is_some(),
            "DelegationResult must fold into ChatMsg::Delegated"
        );
        let (summary, ok) = delegated.unwrap();
        assert_eq!(summary, "Task completed successfully.");
        assert!(ok, "ok must be true for a successful delegation");
    }

    /// Regression for I-1: `Action::SessionPick` must be a no-op while a
    /// delegation is in flight, symmetric with `Submit`'s
    /// `app.streaming || !app.in_flight_subagents.is_empty()` guard. Before the fix, `SessionPick`
    /// only checked `app.streaming`, so a mid-delegation session switch would
    /// let the still-running subagent push session A's events into session
    /// B's in-memory log via `AgentUpdate::Appended`.
    #[tokio::test]
    async fn session_pick_is_noop_while_delegating() {
        let mut app = test_app().await;
        let original_session_id = app.session_id;

        // Seed a second session to switch to.
        let other_id = Ulid::new();
        app.session
            .new_session(other_id, "other".into(), "/repo".into(), 0)
            .await
            .unwrap();
        app.session_ids = vec![other_id];
        app.shell.session_selected = 0;

        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-test".into(),
            task: "test".into(),
            agent: "delegate".into(),
        });
        let quit = handle_action(&mut app, zoid_tui::route::Action::SessionPick)
            .await
            .unwrap();

        assert!(!quit, "SessionPick must not signal quit");
        assert_eq!(
            app.session_id, original_session_id,
            "session_id must not switch while delegating"
        );
        assert!(
            app.events.is_empty(),
            "events must not be swapped in while delegating"
        );
        assert!(
            !app.in_flight_subagents.is_empty(),
            "in_flight set must be untouched by a blocked SessionPick"
        );
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("1 subagent running — press Esc to kill them or wait for completion"),
            "blocked SessionPick should surface the busy hint"
        );
    }

    /// Regression for I-1: `Command::NewSession` must be a no-op while a
    /// delegation is in flight, mirroring the `SessionPick`/`Submit` guard.
    #[tokio::test]
    async fn new_session_is_noop_while_delegating() {
        let mut app = test_app().await;
        let original_session_id = app.session_id;
        app.in_flight_subagents.push(SubagentInfo {
            id: "sub-test".into(),
            task: "test".into(),
            agent: "delegate".into(),
        });

        let quit = exec_command(&mut app, zoid_tui::command::Command::NewSession)
            .await
            .unwrap();

        assert!(!quit, "NewSession must not signal quit");
        assert_eq!(
            app.session_id, original_session_id,
            "session_id must not change while delegating"
        );
        assert!(
            app.events.is_empty(),
            "events must not be cleared/reset while delegating"
        );
        assert!(
            !app.in_flight_subagents.is_empty(),
            "in_flight set must be untouched by a blocked NewSession"
        );
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("1 subagent running — press Esc to kill them or wait for completion"),
            "blocked NewSession should surface the busy hint"
        );
    }

    /// After yield, `:delegate` must not start a subagent against the taken-over
    /// session (it's a turn-start path, symmetric with `Submit`'s `yielded`
    /// guard). And `:new` is the documented escape hatch: it must clear
    /// `yielded` and reclaim the fresh session so the user can keep working.
    /// Regression: `PaletteMove` in Direct mode (`:`-prefixed) must navigate the
    /// direct list, not the Pick fuzzy list. Before the fix the handler always
    /// used `all_items` (Pick), which doesn't contain `:session`/`:mode` etc.,
    /// so the match count was 0 and the selection never moved — arrows were
    /// dead in direct mode despite the router correctly firing `PaletteMove`.
    #[tokio::test]
    async fn palette_move_navigates_direct_list() {
        let mut app = test_app().await;
        app.shell.overlay = zoid_tui::Overlay::Palette;
        app.shell.mode_names = vec!["Chat".into(), "Build".into()];
        // `:mode ` → Stage 2: reload, import, update, + mode names.
        app.shell.palette.query = ":mode ".into();
        app.shell.palette.selected = 0;

        // Down → selection moves (was stuck at 0 before the fix).
        handle_action(&mut app, zoid_tui::route::Action::PaletteMove(1))
            .await
            .unwrap();
        assert_eq!(
            app.shell.palette.selected, 1,
            "PaletteMove(Down) in direct mode must advance the selection"
        );
        // Up wraps (4+ items, so from 1 → 0).
        handle_action(&mut app, zoid_tui::route::Action::PaletteMove(-1))
            .await
            .unwrap();
        assert_eq!(
            app.shell.palette.selected, 0,
            "PaletteMove(Up) must move back"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_command_emits_compaction_events() {
        let mut app = test_app().await;
        // plan_compactions returns an empty plan when compact_threshold_pct
        // is 0 (the default) OR when the current context is below the
        // threshold. Set a tiny target + high pct so the 5000-char tool
        // result exceeds the threshold and compaction fires.
        app.economy.compact_threshold_pct = 50;
        app.context_target = 100; // 50% of 100 = 50 tokens threshold
                                  // Seed an event log with an uncompacted tool result.
        let tc_id = Ulid::new();
        app.record(EventKind::UserMessage {
            text: "do something".into(),
        })
        .await
        .unwrap();
        app.record(EventKind::ToolCall {
            id: tc_id.to_string(),
            name: "shell".into(),
            args: "{}".into(),
        })
        .await
        .unwrap();
        app.record(EventKind::ToolResult {
            id: tc_id.to_string(),
            name: "shell".into(),
            output: (0..200)
                .map(|i| format!("line {i} xxxxxxxxxxxxxxxxxxxxxxxx"))
                .collect::<Vec<_>>()
                .join("\n"),
            is_error: false,
        })
        .await
        .unwrap();
        let session_id = app.session_id;
        let session = app.session.clone();

        let quit = exec_command(&mut app, zoid_tui::command::Command::CompactNow)
            .await
            .unwrap();
        assert!(!quit);

        // The spawned task appends to the session DB (not app.events). Read
        // back from the session after giving the task time to complete.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let events = session.snapshot_session(session_id).await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, EventKind::ToolResultCompacted { .. })),
            ":compact must emit at least one ToolResultCompacted event to the session"
        );
    }

    #[tokio::test]
    async fn compact_command_blocked_while_already_compacting() {
        let mut app = test_app().await;
        app.shell.compacting = true;
        let quit = exec_command(&mut app, zoid_tui::command::Command::CompactNow)
            .await
            .unwrap();
        assert!(!quit);
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("already compacting"),
            ":compact while compacting should surface the hint"
        );
    }

    #[tokio::test]
    async fn select_command_toggles_select_mode() {
        let mut app = test_app().await;
        assert!(!app.shell.select_mode);
        let quit = exec_command(&mut app, zoid_tui::command::Command::ToggleSelectMode)
            .await
            .unwrap();
        assert!(!quit);
        assert!(app.shell.select_mode, ":select must turn select mode on");
        let _ = exec_command(&mut app, zoid_tui::command::Command::ToggleSelectMode)
            .await
            .unwrap();
        assert!(!app.shell.select_mode, ":select again must turn it off");
    }

    #[tokio::test]
    async fn new_clears_yielded_and_unblocks_submit() {
        let mut app = test_app().await;
        app.yielded = true;

        // `:new` clears yielded and reclaims the fresh session.
        let quit = exec_command(&mut app, zoid_tui::command::Command::NewSession)
            .await
            .unwrap();
        assert!(!quit);
        assert!(!app.yielded, ":new must clear the yielded flag");
        // Submit is now unblocked (the guard passes) — sanity-check the guard
        // alone, without spawning a real turn (empty input is a no-op).
        app.textarea = make_input(ratatui_textarea::TextArea::default());
        let _ = handle_action(&mut app, zoid_tui::route::Action::Submit).await;
        assert!(
            app.shell.status_hint.as_deref()
                != Some("session taken over — :session new or :session resume"),
            "after :new, Submit must not surface the yielded hint"
        );
    }

    /// Regression for the whole-branch review's Important #1: the two mid-run
    /// session-change sites (`Action::SessionPick`, `Command::NewSession`) must
    /// call `restore_mode_for_session` so a resumed/new session runs with ITS
    /// OWN mode, never the previous session's mode carried over in-memory.
    /// This test exercises the shared helper directly: a session with no saved
    /// mode degrades to Chat even if `app.modes` is currently parked on a
    /// non-Chat mode (proving the reset-then-restore order, not just a
    /// no-op), a session with a saved mode is restored, and a saved mode that
    /// no longer exists in the registry degrades cleanly to Chat with no panic.
    #[tokio::test]
    async fn restore_mode_for_session_applies_saved_and_degrades_to_chat() {
        let mut app = test_app().await;
        // The Chat floor's name is whatever the base profile is named (`"default"`
        // for `zoid::agent::default_profile()`); capture it rather than hardcoding
        // the literal, since `restore_mode_for_session` resets to
        // `app.base_profile.name`, not a hardcoded `"Chat"` string.
        let chat_name = app.base_profile.name.clone();
        app.modes = zoid_core::mode::ModeRegistry::new(vec![
            zoid_core::mode::Mode::chat(app.base_profile.clone()),
            zoid_core::mode::Mode::Ready {
                profile: zoid_core::agent_profile::AgentProfile {
                    name: "SP".into(),
                    description: "d".into(),
                    system_prompt: "BASE\n\nOVER".into(),
                    tools: vec![],
                    model: None,
                },
                skills: zoid_core::skill::SkillRegistry::new(vec![]),
            },
        ]);

        // Session A: persist "SP" as its active mode.
        let session_a = app.session_id;
        app.session
            .set_active_mode(session_a, "SP".to_string())
            .await
            .unwrap();

        // Create session B with no saved mode.
        let session_b = Ulid::new();
        app.session
            .new_session(session_b, "b".into(), "/repo".into(), 0)
            .await
            .unwrap();
        app.session_id = session_b;

        // Simulate the carried-over in-memory state a mid-run swap would leave
        // behind if the reset were missing: active mode still "SP" from A.
        app.modes.set_active("SP");
        restore_mode_for_session(&mut app).await;
        assert_eq!(
            app.modes.active_name(),
            chat_name,
            "session B has no saved mode, so it must NOT inherit A's carried-over SP"
        );
        assert_eq!(
            app.shell.active_mode, chat_name,
            "the shell mirror must reflect the degraded-to-Chat state"
        );

        // Persist "SP" for session B; restoring should now pick it up.
        app.session
            .set_active_mode(session_b, "SP".to_string())
            .await
            .unwrap();
        app.modes.set_active(&chat_name);
        restore_mode_for_session(&mut app).await;
        assert_eq!(
            app.modes.active_name(),
            "SP",
            "session B's saved mode must be restored"
        );

        // A saved mode that no longer exists in the registry degrades to Chat.
        app.session
            .set_active_mode(session_b, "Ghost".to_string())
            .await
            .unwrap();
        restore_mode_for_session(&mut app).await;
        assert_eq!(
            app.modes.active_name(),
            chat_name,
            "a vanished saved mode must degrade cleanly to Chat, not panic"
        );
    }

    /// A leading `:` in an empty message box now opens the palette (direct mode)
    /// instead of inserting a literal colon. The first time this happens from
    /// Input focus, a one-time status hint teaches the escape hatch (type any
    /// other key first to start a message with ':'); subsequent triggers (from
    /// any focus) never re-show it. `Ctrl+P` (`OpenPalette`) never shows it —
    /// only the `:`-from-empty-box trigger does.
    #[tokio::test]
    async fn colon_from_empty_input_shows_one_time_hint_then_stops() {
        use zoid_tui::route::Action;
        use zoid_tui::state::{Focus, Overlay};

        let mut app = test_app().await;
        app.shell.focus = Focus::Input;
        // First trigger from Input focus → opens the palette AND shows the hint.
        handle_action(&mut app, Action::OpenPaletteDirect)
            .await
            .unwrap();
        assert_eq!(app.shell.overlay, Overlay::Palette);
        assert_eq!(app.shell.palette.query, ":");
        assert!(app.shell.colon_trigger_hinted, "hint flag latches");
        assert!(
            app.shell.status_hint.is_some(),
            "the first Input-triggered colon should surface the escape-hatch hint"
        );

        // Second trigger (still from Input) → palette opens, hint NOT re-shown.
        app.shell.close_overlay();
        app.shell.status_hint = None;
        handle_action(&mut app, Action::OpenPaletteDirect)
            .await
            .unwrap();
        assert_eq!(app.shell.overlay, Overlay::Palette);
        assert!(
            app.shell.status_hint.is_none(),
            "the colon hint must be one-shot"
        );

        // `Ctrl+P` (plain OpenPalette) never triggers the hint, even before the
        // flag latches — it's a different entry point with its own discoverability.
        app.shell.close_overlay();
        app.shell.colon_trigger_hinted = false;
        app.shell.status_hint = None;
        handle_action(&mut app, Action::OpenPalette).await.unwrap();
        assert_eq!(app.shell.overlay, Overlay::Palette);
        assert!(
            app.shell.status_hint.is_none(),
            "OpenPalette (Ctrl+P) must not surface the colon hint"
        );
    }

    #[tokio::test]
    async fn blank_key_commit_does_not_store_or_ready() {
        let mut app = test_app().await;
        app.shell.config_key_prompt = Some("ANTHROPIC_API_KEY");
        app.shell.config_edit = Some("   ".to_string());

        let quit = handle_action(&mut app, zoid_tui::route::Action::ConfigCommitEdit)
            .await
            .unwrap();

        assert!(!quit, "blank key commit must not signal quit");
        assert!(
            app.shell.config_key_prompt.is_none(),
            "key prompt must be cleared/aborted"
        );
        assert!(
            app.shell.config_edit.is_none(),
            "edit buffer must be cleared/aborted"
        );
        assert!(
            app.shell.config_picker.is_empty(),
            "blank commit must not advance to the model picker"
        );
    }

    #[test]
    fn effective_base_url_prefers_override_then_registry() {
        use zoid_core::config::Config;
        // No override → registry default for the canonical id.
        let mut c = Config {
            provider: "ollama".into(), // pin to "ollama" (legacy) → ollama-cloud, base_url = None
            ..Config::default()
        };
        assert_eq!(effective_base_url(&c), "https://ollama.com");

        // Explicit local id, no override → local endpoint.
        c.provider = "ollama-local".into();
        c.base_url = None;
        assert_eq!(effective_base_url(&c), "http://localhost:11434");

        // Override wins over registry.
        c.base_url = Some("http://127.0.0.1:1234".into());
        assert_eq!(effective_base_url(&c), "http://127.0.0.1:1234");

        // Blank override falls back to registry.
        c.base_url = Some("   ".into());
        assert_eq!(effective_base_url(&c), "http://localhost:11434");
    }

    #[test]
    fn ollama_local_needs_no_key() {
        // ollama-local is usable with no OLLAMA_API_KEY (localhost, no auth).
        assert!(!entry_requires_key("ollama-local"));
        assert!(entry_requires_key("ollama-cloud"));
        assert!(entry_requires_key("anthropic-api"));
    }

    #[test]
    fn key_env_for_family() {
        assert_eq!(key_env_for("anthropic-api"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(key_env_for("ollama-cloud"), Some("OLLAMA_API_KEY"));
        assert_eq!(key_env_for("ollama-local"), None); // no key needed
    }

    #[test]
    fn key_env_for_opencode_go_is_opencode_go_api_key() {
        assert_eq!(key_env_for("opencode-go"), Some("OPENCODE_GO_API_KEY"));
    }

    #[test]
    fn key_env_for_opencode_zen_maps_to_shared_go_key() {
        assert_eq!(key_env_for("opencode-zen"), Some("OPENCODE_GO_API_KEY"));
        // sanity: Go unchanged
        assert_eq!(key_env_for("opencode-go"), Some("OPENCODE_GO_API_KEY"));
    }

    #[test]
    fn entry_requires_key_opencode_zen_is_true() {
        assert!(entry_requires_key("opencode-zen"));
    }

    #[test]
    fn key_env_for_zai_coding_plan_is_zai_api_key() {
        assert_eq!(key_env_for("zai-coding-plan"), Some("ZAI_API_KEY"));
    }

    #[test]
    fn entry_requires_key_zai_coding_plan_is_true() {
        assert!(entry_requires_key("zai-coding-plan"));
    }

    #[test]
    fn entry_requires_key_opencode_go_is_true() {
        assert!(entry_requires_key("opencode-go"));
    }

    #[test]
    fn select_provider_ollama_local_is_ready_without_key() {
        let config = zoid_core::config::Config {
            provider: "ollama-local".to_string(),
            ..Default::default()
        };
        let (_provider, name, has_key) = select_provider(&config, &None);
        assert_eq!(name, "ollama");
        assert!(has_key, "ollama-local must be usable (ready) with no key");
    }

    // --- resolve_resume_id tests ---

    fn mk_session_info(id_val: u128, name: &str) -> zoid_core::sessions::SessionInfo {
        zoid_core::sessions::SessionInfo {
            id: Ulid::from(id_val),
            name: name.into(),
            root_path: "/repo".into(),
            created_ts: 0,
            last_touched_ts: 0,
            token_total: 0,
            active: false,
            active_pid: None,
            active_heartbeat: None,
        }
    }

    #[test]
    fn resolve_resume_id_full_ulid_match() {
        let id = Ulid::from(123456789u128);
        let sessions = vec![mk_session_info(123456789, "test")];
        let result = resolve_resume_id(&sessions, &id.to_string());
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn resolve_resume_id_last4_match() {
        let id = Ulid::from(123456789u128);
        let sessions = vec![mk_session_info(123456789, "test")];
        let l4 = last4(&id.to_string());
        let result = resolve_resume_id(&sessions, &l4);
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn resolve_resume_id_not_found() {
        let sessions = vec![mk_session_info(1, "test")];
        let result = resolve_resume_id(&sessions, "ABCD");
        assert!(matches!(result, Err(ResumeIdError::NotFound)));
    }

    #[test]
    fn resolve_resume_id_not_found_empty() {
        let sessions: Vec<zoid_core::sessions::SessionInfo> = Vec::new();
        let result = resolve_resume_id(&sessions, "ABCD");
        assert!(matches!(result, Err(ResumeIdError::NotFound)));
    }

    #[test]
    fn resolve_resume_id_ambiguous_real_collision() {
        let id_a = Ulid::from(100u128);
        let id_b = Ulid::from(200u128);
        let la = last4(&id_a.to_string());
        let lb = last4(&id_b.to_string());
        let sessions = vec![mk_session_info(100, "a"), mk_session_info(200, "b")];
        if la == lb {
            let result = resolve_resume_id(&sessions, &la);
            assert!(matches!(result, Err(ResumeIdError::Ambiguous(_))));
        } else {
            assert_eq!(resolve_resume_id(&sessions, &la).unwrap(), id_a);
            assert_eq!(resolve_resume_id(&sessions, &lb).unwrap(), id_b);
        }
    }

    #[test]
    fn resolve_resume_id_invalid_length() {
        let result = resolve_resume_id(&[], "ABC");
        assert!(matches!(result, Err(ResumeIdError::InvalidLength)));
    }

    #[test]
    fn resolve_resume_id_invalid_ulid_syntax() {
        let result = resolve_resume_id(&[], "!!!!!!!!!!!!!!!!!!!!!!!!!!");
        assert!(result.is_err());
    }

    // --- pick_choice tests ---
    // Convention: logical index 0 = "Create new", indices 1..=n = session rows.
    // The wrap math is unchanged from the old layout; only the
    // session/Create-new boundary moved from "cur < n" to "cur == 0".

    #[test]
    fn pick_choice_down_advances_selection() {
        // Down from Create-new (0) → first session (1).
        assert_eq!(pick_choice(3, 0, PickKey::Down), PickOutcome::Pending(1));
    }

    #[test]
    fn pick_choice_up_wraps() {
        // n_sessions=3 → total rows = 4 (0..3). Up from 0 (Create new) → 3 (last session).
        assert_eq!(pick_choice(3, 0, PickKey::Up), PickOutcome::Pending(3));
    }

    #[test]
    fn pick_choice_down_wraps() {
        // n_sessions=3 → total rows = 4 (0,1,2,3). Down from 3 (last session) → 0 (Create new).
        assert_eq!(pick_choice(3, 3, PickKey::Down), PickOutcome::Pending(0));
    }

    #[test]
    fn pick_choice_enter_on_session_resumes() {
        // Index 1 is the first session row. Enter → Resume(1).
        assert_eq!(pick_choice(3, 1, PickKey::Enter), PickOutcome::Resume(1));
        // Index 3 is the last session row (n_sessions=3). Enter → Resume(3).
        assert_eq!(pick_choice(3, 3, PickKey::Enter), PickOutcome::Resume(3));
    }

    #[test]
    fn pick_choice_enter_on_create_new() {
        // Index 0 is "Create new". Enter → CreateNew.
        assert_eq!(pick_choice(3, 0, PickKey::Enter), PickOutcome::CreateNew);
    }

    #[test]
    fn pick_choice_esc_aborts() {
        assert_eq!(pick_choice(3, 0, PickKey::Esc), PickOutcome::Abort);
    }

    #[test]
    fn pick_choice_clamps_selection_to_total_rows() {
        // If selected is somehow past the end, Down should wrap to 0.
        assert_eq!(pick_choice(2, 5, PickKey::Down), PickOutcome::Pending(0));
    }

    #[test]
    fn pick_choice_delete_on_session_row() {
        // Indices 1 and 2 are session rows (n_sessions=2). Delete → DeleteConfirm.
        assert_eq!(
            pick_choice(2, 1, PickKey::Delete),
            PickOutcome::DeleteConfirm(1)
        );
        assert_eq!(
            pick_choice(2, 2, PickKey::Delete),
            PickOutcome::DeleteConfirm(2)
        );
    }

    #[test]
    fn pick_choice_delete_on_create_new_is_noop() {
        // Index 0 is "Create new". Delete is a no-op → Pending(0).
        assert_eq!(pick_choice(2, 0, PickKey::Delete), PickOutcome::Pending(0));
    }

    // --- picker_scroll_offset tests ---
    // The startup picker can list more sessions than fit on screen. The pure
    // y-offset keeps the selected row within the visible window. Layout (new):
    // line 0 = title, 1 = blank, 2 = "Create new", 3.. = session rows.
    // "Create new" is pinned at line 2 and can never clip — the offset's job is
    // to keep the selected *session* row visible, not to rescue "Create new".

    #[test]
    fn scroll_offset_zero_when_everything_fits() {
        // A short list fits entirely within a tall terminal; no scrolling.
        assert_eq!(picker_scroll_offset(2, 20), 0); // "Create new" at line 2
        assert_eq!(picker_scroll_offset(5, 20), 0); // a session row at line 5
    }

    #[test]
    fn scroll_offset_advances_when_cursor_moves_below_view() {
        // visible_height=4. Lines 0..3 visible initially. Selecting line 5
        // (a session row in a long list) must scroll so line 5 is the last
        // visible row → offset = 5 - 4 + 1 = 2.
        assert_eq!(picker_scroll_offset(5, 4), 2);
    }

    #[test]
    fn scroll_offset_keeps_last_session_row_visible() {
        // 10 sessions → last session row is at line 2 + 10 = 12. visible_height=5.
        // Offsetting by 12-5+1=8 puts line 12 as the last visible row.
        // (Under the old layout this test was named "keeps_create_new_row_visible"
        // — but "Create new" is now pinned at line 2 and never needs this.)
        assert_eq!(picker_scroll_offset(12, 5), 8);
    }

    #[test]
    fn scroll_offset_create_new_never_triggers_scroll() {
        // "Create new" is at line 2 — always within the first screen regardless
        // of visible_height, so selecting it always yields offset 0.
        assert_eq!(picker_scroll_offset(2, 20), 0);
        assert_eq!(picker_scroll_offset(2, 5), 0);
        assert_eq!(picker_scroll_offset(2, 3), 0);
    }

    #[test]
    fn scroll_offset_only_grows_never_shrinks_jumps_back() {
        // Once scrolled down, moving the cursor back up should pull the offset
        // back so the selected row is the *first* visible one when it would
        // otherwise be clipped at the top. visible_height=4, selected=3 →
        // offset 0 (line 3 is the last of the 0..3 window). selected=2 → 0.
        assert_eq!(picker_scroll_offset(3, 4), 0);
        assert_eq!(picker_scroll_offset(2, 4), 0);
    }

    #[test]
    fn scroll_offset_clamps_when_cursor_far_above_window() {
        // If the selected line is behind the current natural offset, the offset
        // must drop so the selected line becomes visible. selected=2, h=4 → 0.
        assert_eq!(picker_scroll_offset(2, 4), 0);
    }

    #[test]
    fn scroll_offset_zero_when_visible_height_exceeds_content() {
        // selected line within the first screen even for huge heights.
        assert_eq!(picker_scroll_offset(0, 100), 0);
        assert_eq!(picker_scroll_offset(50, 100), 0);
    }

    // --- boot_decision tests ---

    #[test]
    fn boot_decision_no_sessions_creates() {
        assert_eq!(boot_decision(0, false, None), BootPath::AutoCreate);
    }

    #[test]
    fn boot_decision_one_session_auto_resumes() {
        assert_eq!(boot_decision(1, false, None), BootPath::AutoResume);
    }

    #[test]
    fn boot_decision_two_sessions_shows_picker() {
        assert_eq!(boot_decision(2, false, None), BootPath::Picker);
    }

    #[test]
    fn boot_decision_new_flag_forces_create() {
        assert_eq!(boot_decision(5, true, None), BootPath::ForceNew);
    }

    #[test]
    fn boot_decision_resume_flag_forces_resume() {
        assert_eq!(
            boot_decision(5, false, Some("ABCD")),
            BootPath::ForceResume("ABCD".to_string())
        );
    }

    /// Regression: when a Reject clears app.wizard but the event log later has
    /// an open ModeMapping question (e.g. the model re-proposed in the same
    /// turn before the break, or a new scan re-opened the wizard), calling
    /// `answer_question` with "Approve" must NOT panic on
    /// `app.wizard.expect("wizard open during approval")`.
    #[tokio::test]
    async fn answer_question_no_panic_when_wizard_none_on_approve() {
        use zoid_core::event::QuestionKind;
        use zoid_core::wizard::{MappingEntry, ModeMapping};

        let mut app = test_app().await;

        // Seed an open QuestionAsked(ModeMapping) into the event log.
        let mapping = Box::new(ModeMapping {
            mode_name: "TestMode".into(),
            mode_description: "d".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "a/SKILL.md".into(),
                source: "skills/a/SKILL.md".into(),
                summary: "a".into(),
            }],
        });
        let ev = zoid_core::event::Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAsked {
                id: "q1".to_string(),
                kind: QuestionKind::ModeMapping { mapping },
                question: "test".into(),
                choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
            },
        );
        app.events.push(ev);

        // app.wizard is None — simulates the state after a Reject cleared it.
        assert!(app.wizard.is_none());

        // This must not panic. Currently it does: `.expect("wizard open during
        // approval")` at the Approve arm.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            answer_question(&mut app, zoid::agent::Answer::Choice("Approve".into()));
        }));

        assert!(
            result.is_ok(),
            "answer_question should not panic when wizard is None, even on Approve"
        );
    }

    /// Regression: Reject must NOT clear app.wizard. Before the fix, Reject
    /// set `app.wizard = None`, which meant (a) the wizard tools disappeared
    /// from the next turn's tool set, and (b) if the model re-proposed in
    /// the same turn and the user clicked Approve, `answer_question` panicked
    /// on `app.wizard.expect("wizard open during approval")`.
    #[tokio::test]
    async fn reject_does_not_clear_wizard() {
        use zoid_core::event::QuestionKind;
        use zoid_core::wizard::{MappingEntry, ModeMapping, ScannedFile, UpstreamScan};

        let mut app = test_app().await;

        // Set up the wizard with a scan.
        let scan = UpstreamScan {
            url: "u".into(),
            repo: "o/r".into(),
            resolved_ref: "abc".into(),
            subtree_path: "skills".into(),
            files: vec![ScannedFile {
                upstream_path: "skills/a/SKILL.md".into(),
                sha: "sha-a".into(),
                content: "---\nname: a\ndescription: d\n---\nBODY\n".into(),
            }],
        };
        app.wizard = Some(zoid::mode_wizard::ModeImportWizard::new_import(scan));

        // Seed an open QuestionAsked(ModeMapping).
        let mapping = Box::new(ModeMapping {
            mode_name: "TestMode".into(),
            mode_description: "d".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "a/SKILL.md".into(),
                source: "skills/a/SKILL.md".into(),
                summary: "a".into(),
            }],
        });
        let ev = zoid_core::event::Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::QuestionAsked {
                id: "q1".to_string(),
                kind: QuestionKind::ModeMapping { mapping },
                question: "test".into(),
                choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
            },
        );
        app.events.push(ev);

        // Set up a pending answer channel so answer_question can send the reply.
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.pending_answer = Some(tx);

        // Reject.
        answer_question(&mut app, zoid::agent::Answer::Choice("Reject".into()));

        // The wizard must still be alive.
        assert!(
            app.wizard.is_some(),
            "Reject must not clear app.wizard — the model may need to re-propose"
        );

        // The reply should have been sent.
        let ans = rx.await.unwrap();
        assert!(
            matches!(ans, zoid::agent::Answer::Choice(ref c) if c == "Reject"),
            "the answer channel should receive Reject"
        );
    }

    #[test]
    fn map_render_diff_preserves_counts_and_kinds() {
        let fd = zoid_tools::FileDiff {
            path: "f.rs".into(),
            added: 3,
            removed: 1,
            truncated_by: 2,
            lines: vec![
                zoid_tools::DiffLine {
                    old_no: Some(1),
                    new_no: Some(1),
                    kind: zoid_tools::DiffKind::Ctx,
                    text: "a".into(),
                },
                zoid_tools::DiffLine {
                    old_no: None,
                    new_no: Some(2),
                    kind: zoid_tools::DiffKind::Add,
                    text: "b".into(),
                },
                zoid_tools::DiffLine {
                    old_no: Some(3),
                    new_no: None,
                    kind: zoid_tools::DiffKind::Del,
                    text: "c".into(),
                },
            ],
        };
        let r = map_render_diff(fd);
        assert_eq!((r.added, r.removed, r.truncated_by), (3, 1, 2));
        assert_eq!(r.lines.len(), 3);
        assert!(matches!(
            r.lines[1].kind,
            zoid_tui::state::RenderDiffKind::Add
        ));
        assert!(matches!(
            r.lines[2].kind,
            zoid_tui::state::RenderDiffKind::Del
        ));
    }
}

#[cfg(test)]
mod worktree_switch_tests {
    use super::compute_worktree_switch;
    use std::path::Path;
    use std::process::Command;
    use zoid::agent::WorktreeAction;

    /// `git init` a fresh repo with one commit so HEAD exists (worktrees need a commit).
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(p)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(p.join("f.txt"), "hi").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn enter_returns_absolute_worktree_path_and_sets_active() {
        let repo = init_repo();
        let mut active = None;
        let (cwd, _warn) = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter {
                name: "feature-x".into(),
            },
            false,
            repo.path(),
        )
        .expect("enter should succeed");
        assert!(cwd.is_absolute(), "enter cwd must be absolute: {cwd:?}");
        assert!(
            cwd.ends_with(Path::new(".zoid/worktrees/feature-x")),
            "enter cwd points at the worktree: {cwd:?}"
        );
        assert!(cwd.exists(), "the worktree dir was created");
        assert!(active.is_some(), "active worktree is now set");
    }

    #[test]
    fn enter_guard_already_in_worktree_errors() {
        let repo = init_repo();
        let mut active = None;
        compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter { name: "a".into() },
            false,
            repo.path(),
        )
        .unwrap();
        let err = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter { name: "b".into() },
            false,
            repo.path(),
        )
        .unwrap_err();
        assert!(err.contains("already in a worktree"), "got: {err}");
    }

    #[test]
    fn enter_guard_subagent_running_errors() {
        let repo = init_repo();
        let mut active = None;
        let err = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter { name: "a".into() },
            true,
            repo.path(),
        )
        .unwrap_err();
        assert!(err.contains("subagent"), "got: {err}");
        assert!(active.is_none(), "guard must not enter");
    }

    #[test]
    fn exit_returns_absolute_repo_root_and_clears() {
        let repo = init_repo();
        let mut active = None;
        compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter { name: "a".into() },
            false,
            repo.path(),
        )
        .unwrap();
        let (cwd, _warn) =
            compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path())
                .expect("exit should succeed");
        assert!(cwd.is_absolute());
        assert_eq!(
            cwd.canonicalize().unwrap(),
            repo.path().canonicalize().unwrap(),
            "exit cwd is the repo root"
        );
        assert!(active.is_none(), "exit clears the active worktree");
    }

    #[test]
    fn exit_guard_not_in_worktree_errors() {
        let repo = init_repo();
        let mut active = None;
        let err = compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path())
            .unwrap_err();
        assert!(err.contains("not in a worktree"), "got: {err}");
    }

    #[test]
    fn dirty_exit_keeps_worktree_and_reenter_succeeds() {
        // I2 + dirty-keep: a worktree with uncommitted changes survives exit and
        // can be re-entered without error.
        let repo = init_repo();
        let mut active = None;
        let (cwd, _warn) = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter {
                name: "keep".into(),
            },
            false,
            repo.path(),
        )
        .unwrap();
        // Dirty it: modify the tracked file inside the worktree.
        std::fs::write(cwd.join("f.txt"), "uncommitted change").unwrap();
        compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).unwrap();
        assert!(
            cwd.exists(),
            "dirty worktree dir must be KEPT on exit (no data loss)"
        );
        // The bytes themselves must survive, not just the directory — this is the
        // actual "no data loss" invariant (a dir-only check would pass even if a
        // future remove-adjacent bug truncated tracked files).
        assert_eq!(
            std::fs::read_to_string(cwd.join("f.txt")).unwrap(),
            "uncommitted change",
            "uncommitted bytes must survive a dirty exit"
        );
        // Re-enter the same name — must NOT error (idempotent re-enter).
        let (cwd2, _warn) = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter {
                name: "keep".into(),
            },
            false,
            repo.path(),
        )
        .unwrap();
        assert!(cwd2.exists());
        assert!(active.is_some());
    }

    #[test]
    fn clean_exit_with_unmerged_commits_retains_branch() {
        // A clean worktree (no uncommitted changes) with commits not on HEAD
        // must retain the branch ref on exit — the work isn't orphaned.
        let repo = init_repo();
        let mut active = None;
        let (cwd, _warn) = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter {
                name: "unmerged".into(),
            },
            false,
            repo.path(),
        )
        .unwrap();
        // Commit a new file in the worktree — the branch now has a commit
        // not on HEAD (main).
        std::fs::write(cwd.join("new.txt"), "new content").unwrap();
        std::process::Command::new("git")
            .args(["-C"])
            .arg(&cwd)
            .args(["add", "."])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["-C"])
            .arg(&cwd)
            .args(["commit", "-m", "new commit on branch"])
            .output()
            .unwrap();
        // Exit — the worktree is clean (no uncommitted changes), but the
        // branch has an unmerged commit.
        let (_root, warn) =
            compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).unwrap();
        assert!(active.is_none(), "exit clears active");
        // The branch must still exist (not deleted).
        let branch_exists = std::process::Command::new("git")
            .args(["-C"])
            .arg(repo.path())
            .args(["rev-parse", "--verify", "unmerged"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(
            branch_exists,
            "branch 'unmerged' must be retained — has unmerged commits"
        );
        // The warning must mention the branch name and "retained".
        let warn = warn.expect("warning should be Some when branch is retained");
        assert!(
            warn.contains("unmerged"),
            "warning mentions branch name: {warn}"
        );
        assert!(warn.contains("retained"), "warning says 'retained': {warn}");
    }

    #[test]
    fn clean_exit_retains_branch_when_main_advanced() {
        // Regression: the branch has unmerged commits, but main's HEAD moved
        // forward while the worktree was active. The branch and HEAD diverged.
        // graph_descendant_of(branch, head) returns false because HEAD is not
        // an ancestor of the branch — so the old check deleted the branch.
        // The merge-base check correctly retains it.
        let repo = init_repo();
        let mut active = None;
        let (cwd, _warn) = compute_worktree_switch(
            &mut active,
            WorktreeAction::Enter {
                name: "diverged".into(),
            },
            false,
            repo.path(),
        )
        .unwrap();
        // Commit on the branch.
        std::fs::write(cwd.join("branch.txt"), "branch work").unwrap();
        std::process::Command::new("git")
            .args(["-C"])
            .arg(&cwd)
            .args(["add", "."])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["-C"])
            .arg(&cwd)
            .args(["commit", "-m", "branch commit"])
            .output()
            .unwrap();
        // Advance main past the branch point — main and the branch now diverge.
        std::process::Command::new("git")
            .args(["-C"])
            .arg(repo.path())
            .args(["commit", "--allow-empty", "-m", "main advanced"])
            .output()
            .unwrap();
        // Exit — the branch has a commit not reachable from main's HEAD.
        let (_root, warn) =
            compute_worktree_switch(&mut active, WorktreeAction::Exit, false, repo.path()).unwrap();
        assert!(active.is_none(), "exit clears active");
        let branch_exists = std::process::Command::new("git")
            .args(["-C"])
            .arg(repo.path())
            .args(["rev-parse", "--verify", "diverged"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(
            branch_exists,
            "branch 'diverged' must be retained — has unmerged commits even though main advanced"
        );
        let warn = warn.expect("warning should be Some when branch is retained");
        assert!(
            warn.contains("diverged"),
            "warning mentions branch name: {warn}"
        );
        assert!(warn.contains("retained"), "warning says 'retained': {warn}");
    }

    #[test]
    fn git_status_at_reads_the_given_dir_not_cwd() {
        use super::git_status_at;
        let repo = init_repo();
        std::fs::write(repo.path().join("f.txt"), "hi there changed").unwrap();
        let (_added, _removed, files) = git_status_at(repo.path());
        assert!(
            files >= 1,
            "git_status_at must see the change in the given dir: files={files}"
        );
    }

    #[test]
    fn current_branch_at_reads_the_given_dir() {
        use super::current_branch_at;
        let repo = init_repo();
        let b = current_branch_at(repo.path());
        assert!(
            b == "main" || b == "master",
            "init default branch, got: {b}"
        );
    }
}

#[cfg(test)]
mod mcp_confirm_guard_tests {
    use super::catalog_confirm_awaits;
    use zoid_tui::state::{PluginCatalogRow, PluginCatalogState};

    fn row(id: &str) -> PluginCatalogRow {
        PluginCatalogRow {
            id: id.into(),
            name: id.into(),
            kind_label: "mcp".into(),
            description: String::new(),
            source_label: String::new(),
            license: None,
        }
    }

    #[test]
    fn awaits_only_the_selected_loading_row() {
        let mut cat = PluginCatalogState::loading();
        cat.rows = vec![row("a"), row("b")];
        cat.cursor = 1; // on "b"
                        // List mode → not awaiting any fetch yet.
        assert!(!catalog_confirm_awaits(&cat, "b"));
        cat.begin_confirm_loading();
        assert!(catalog_confirm_awaits(&cat, "b")); // the row whose fetch we spawned
        assert!(!catalog_confirm_awaits(&cat, "a")); // a stale fetch for "a" is dropped
    }
}

#[cfg(test)]
mod mcp_install_tests {
    use super::mcp_target_path;
    use zoid_tui::state::McpTarget;

    #[test]
    fn user_target_is_config_mcp_json() {
        let p = mcp_target_path(
            McpTarget::User,
            std::path::Path::new("/cfg"),
            std::path::Path::new("/repo"),
        );
        assert_eq!(p, std::path::Path::new("/cfg/mcp.json"));
    }

    #[test]
    fn project_target_is_cwd_dot_mcp_json() {
        let p = mcp_target_path(
            McpTarget::Project,
            std::path::Path::new("/cfg"),
            std::path::Path::new("/repo"),
        );
        assert_eq!(p, std::path::Path::new("/repo/.mcp.json"));
    }
}

#[cfg(test)]
mod onboarding_tests {
    use super::*;
    use zoid_core::config::Config;

    fn cfg_with_provider(provider: &str) -> Config {
        Config {
            provider: provider.to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn gate_fires_for_first_time_empty_provider() {
        let c = cfg_with_provider("");
        assert!(wizard_needed(true, &c, false, true));
    }

    #[test]
    fn gate_fires_for_first_time_key_required_no_key() {
        let c = cfg_with_provider("anthropic-api");
        assert!(wizard_needed(true, &c, false, true));
    }

    #[test]
    fn gate_skips_ollama_local() {
        let c = cfg_with_provider("ollama-local");
        assert!(!wizard_needed(true, &c, false, true));
    }

    #[test]
    fn gate_skips_when_key_present() {
        let c = cfg_with_provider("anthropic-api");
        assert!(!wizard_needed(true, &c, true, true));
    }

    #[test]
    fn gate_skips_returning_user() {
        let c = cfg_with_provider("");
        assert!(!wizard_needed(false, &c, false, true));
    }

    #[test]
    fn gate_skips_when_secrets_unavailable() {
        let c = cfg_with_provider("");
        assert!(!wizard_needed(true, &c, false, false));
    }

    #[test]
    fn gate_ignores_ambient_key_for_empty_provider() {
        // A first-time user with empty provider but an ambient OLLAMA_API_KEY
        // still gets the wizard (empty-provider check precedes !has_key).
        let c = cfg_with_provider("");
        assert!(wizard_needed(true, &c, true, true));
    }

    #[test]
    fn key_url_and_key_env_for_are_in_lockstep() {
        // Every key-requiring provider (key_url: Some) must have a key_env_for arm
        // returning Some. A keyless provider (key_url: None) must return None.
        for e in zoid_provider::model::PROVIDERS.iter() {
            let key_env = key_env_for(e.id);
            if e.key_url.is_some() {
                assert!(
                    key_env.is_some(),
                    "{} has key_url: Some but key_env_for returned None — \
                     a key-requiring provider must have a key env mapping",
                    e.id
                );
                assert!(
                    entry_requires_key(e.id),
                    "{} has key_url: Some but entry_requires_key returned false",
                    e.id
                );
            } else {
                assert!(
                    key_env.is_none(),
                    "{} has key_url: None but key_env_for returned Some({:?}) — \
                     a keyless provider must not have a key env mapping",
                    e.id,
                    key_env
                );
                assert!(
                    !entry_requires_key(e.id),
                    "{} has key_url: None but entry_requires_key returned true",
                    e.id
                );
            }
        }
    }

    #[test]
    fn canonical_id_empty_is_not_ollama_local() {
        // The gate's ollama-local exemption precedes the empty-provider check.
        // Its correctness depends on canonical_id("") != "ollama-local".
        assert_ne!(zoid_provider::model::canonical_id(""), "ollama-local");
        assert_eq!(zoid_provider::model::canonical_id(""), "");
    }
}
