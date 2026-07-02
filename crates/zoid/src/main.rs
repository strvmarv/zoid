use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event as CEvent, EventStream,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use ratatui::{layout::Rect, prelude::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tui_textarea::TextArea;
use ulid::Ulid;

use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::conversation;
use zoid_core::session::SessionHandle;
use zoid_provider::Provider;
use zoid_provider::{default_model, default_provider};
use zoid_tools::Tool;
use zoid_tui::chat::ChatView;
use zoid_tui::layout::compute;
use zoid_tui::render_shell;
use zoid_tui::route::{palette_selected_command, route_key, route_mouse};

/// Duration of the zoom fold/unfold line-reveal animation (Ⓡ2, T5).
const ZOOM_ANIM_MS: u64 = 160;

/// Pure DB-path resolver (env injected for testing). Precedence:
/// `$ZOID_DB` > `$XDG_DATA_HOME/zoid/zoid.db` > `$HOME/.local/share/zoid/zoid.db`.
fn resolve_db_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("ZOID_DB") {
        return PathBuf::from(p);
    }
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".local/share"));
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
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".config"));
    base.join("zoid")
}

/// Pure secret-key-path resolver (env injected for testing), mirroring
/// `resolve_db_path`'s precedence: `$XDG_DATA_HOME/zoid/secret.key` >
/// `$HOME/.local/share/zoid/secret.key`.
fn resolve_secret_key_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".local/share"));
    base.join("zoid").join("secret.key")
}

/// Load config from files + env, in precedence order (user-global < project <
/// local < env). Missing files are skipped (empty layer); a malformed file is
/// skipped with a stderr note (non-fatal — the process still starts).
fn load_config() -> (zoid_core::config::Config, zoid_core::config::Provenance) {
    use zoid_core::config::{merge, parse_toml, PartialConfig, Source};
    let env = |k: &str| std::env::var(k).ok();
    let cfg_dir = resolve_config_dir(env);
    let read = |p: PathBuf| -> Option<PartialConfig> {
        let text = std::fs::read_to_string(&p).ok()?;
        match parse_toml(&text) {
            Ok(pc) => Some(pc),
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
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() {
            if n > 0 {
                envp.economy.context_ceiling = Some(n);
            }
        }
    }
    if std::env::var("ZOID_REDUCED_MOTION")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        envp.reduced_motion = Some(true);
    }
    layers.push((Source::Env, envp));
    merge(&layers)
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

/// Build the message input with the tui-textarea cursor-line **underline**
/// disabled (spec §2.2/§9): the default underline clutters the calm box.
fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
    let mut textarea = textarea;
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea
}

/// Canonical repo/cwd root as a string (best-effort absolute path).
fn repo_root() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
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

/// Build the economy `ContextPolicy` (spec §7.2) from the loaded config's
/// `[economy]` table, resolving `compact_threshold_pct` (0–100) against the
/// resolved token `ceiling` — 0 disables compaction (`None`), else the
/// absolute token count `ceiling * pct / 100`.
fn policy_from_config(
    econ: &zoid_core::config::EconomyConfig,
    ceiling: u64,
) -> zoid_core::assembler::ContextPolicy {
    let compact_threshold = if econ.compact_threshold_pct == 0 {
        None
    } else {
        Some(ceiling.saturating_mul(econ.compact_threshold_pct as u64) / 100)
    };
    zoid_core::assembler::ContextPolicy {
        token_ceiling: econ.token_ceiling,
        auto_evict_cold: econ.auto_evict_cold,
        compact_threshold,
    }
}

/// Best-effort current branch from `.git/HEAD` (`ref: refs/heads/<name>`); "main" otherwise.
fn current_branch() -> String {
    std::fs::read_to_string(".git/HEAD")
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix("ref: refs/heads/")
                .map(|b| b.to_string())
        })
        .unwrap_or_else(|| "main".into())
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

/// Working-tree change stats via `git diff --numstat` (unstaged) + `--cached`
/// (staged). Best-effort — any failure yields zeros.
fn git_status() -> (usize, usize, usize) {
    let run = |args: &[&str]| -> String {
        std::process::Command::new("git")
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

struct App {
    session: SessionHandle,
    session_id: Ulid,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    model: String,
    /// Economy config (spec §7.2), carried from `load_config()` so `run`'s
    /// per-frame `ContextPolicy` build (via `policy_from_config`) doesn't need
    /// its own copy of `main`'s `config` local.
    economy: zoid_core::config::EconomyConfig,
    textarea: TextArea<'static>,
    streaming: bool,
    shell: zoid_tui::ShellState,
    ui_tx: mpsc::Sender<AgentUpdate>,
    /// Monotonic clock start for motion timing (Ⓡ2).
    started: std::time::Instant,
    /// Last time the repo drawer's git changes line was refreshed (cadence-gated
    /// to ~1/sec so the event-driven run loop doesn't shell out to `git` on
    /// every keystroke / streaming tick).
    last_git_refresh: std::time::Instant,
    /// When the altitude last changed, for the fold/unfold reveal (Ⓡ2).
    zoom_changed_at: Option<std::time::Instant>,
    /// Local UTC offset (seconds) for message-row HH:MM stamps, sampled once.
    tz_offset_secs: i32,
    /// Epoch-millis the active session started (resumed session's `created_ts`,
    /// or boot time for a freshly-created one) — feeds the session drawer's
    /// live "dur" label.
    session_started_ms: i64,
    /// Session ids backing the resume-session picker rows (index-aligned with
    /// `shell.sessions`), populated when `Command::ResumeSessionPicker` opens it.
    session_ids: Vec<Ulid>,
    /// One subagent at a time (spec §6): set while a `:delegate` dispatch (or a
    /// verb-picked task) is in flight; cleared when its `DelegationResult` lands.
    delegating: bool,
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

    let session = SessionHandle::spawn(
        path.to_str()
            .context("session DB path is not valid UTF-8")?,
    )?;

    // Auto-resume the most-recently-touched session for this repo, else create one.
    let sessions = session
        .list_sessions(Some(root.clone()))
        .await
        .unwrap_or_default();
    let (session_id, session_name, session_started_ms) = if let Some(s) = sessions.first() {
        session.touch_session(s.id, boot_ts).await.ok();
        (s.id, s.name.clone(), s.created_ts)
    } else {
        let id = Ulid::new();
        let name = derive_session_name(None, boot_ts, tz_offset_secs);
        session
            .new_session(id, name.clone(), root.clone(), boot_ts)
            .await?;
        (id, name, boot_ts)
    };
    let events = session.snapshot_session(session_id).await?;

    // Task 10/12 consume _prov.
    let (config, _prov) = load_config();
    let model = if config.model.is_empty() {
        default_model().to_string()
    } else {
        config.model.clone()
    };
    let secret_key = resolve_secret_key_path(|k| std::env::var(k).ok());
    let secrets = zoid_core::secret::EncryptedDb::open(&path.to_string_lossy(), &secret_key)
        .map(std::sync::Arc::new)
        .ok(); // None → secrets unavailable this run (non-fatal)

    // Provider + credential from config.provider + the secret store (env wins
    // inside SecretStore::get). No key found → fall back to the offline
    // FakeProvider so the binary always runs; `provider_label` below mirrors
    // this exact selection so the drawer never disagrees with reality.
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
    let (provider, provider_name, has_key): (Arc<dyn Provider>, &str, bool) =
        match config.provider.as_str() {
            "anthropic" => match key_for("ANTHROPIC_API_KEY") {
                Some(k) => (
                    Arc::new(zoid_provider::anthropic::AnthropicProvider::new(k)),
                    "anthropic",
                    true,
                ),
                None => (default_provider(), "anthropic", false),
            },
            _ => match key_for("OLLAMA_API_KEY") {
                Some(k) => (
                    Arc::new(zoid_provider::ollama::OllamaProvider::new(k)),
                    "ollama",
                    true,
                ),
                None => (default_provider(), "ollama", false),
            },
        };

    let mut shell = zoid_tui::ShellState::new();
    shell.branch = current_branch();
    shell.reduced_motion = config.reduced_motion;
    shell.repo_name = Path::new(&root)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.clone());
    let (boot_added, boot_removed, boot_files) = git_status();
    shell.changes_added = boot_added;
    shell.changes_removed = boot_removed;
    shell.changes_files = boot_files;
    shell.session_name = session_name;
    shell.model = model.clone();
    // Economy ⑤ denominator: config-derived (ZOID_CONTEXT_CEILING overrides via
    // config.economy.context_ceiling), else the model registry's default.
    // Constant for the process lifetime, so set once here rather than per frame.
    shell.ctx_ceiling = config
        .economy
        .context_ceiling
        .unwrap_or_else(|| zoid_provider::context_ceiling(&model));
    shell.provider = provider_label(provider_name, has_key);
    shell.cwd = root.clone();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    let mut app = App {
        session,
        session_id,
        events,
        provider,
        tools: Arc::new(zoid_tools::registry()),
        model,
        economy: config.economy,
        textarea: make_input(TextArea::default()),
        streaming: false,
        shell,
        ui_tx,
        started: std::time::Instant::now(),
        last_git_refresh: std::time::Instant::now(),
        zoom_changed_at: None,
        tz_offset_secs,
        session_started_ms,
        session_ids: Vec::new(),
        delegating: false,
    };

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    // Kitty keyboard protocol: lets the terminal report ⇧⏎ distinctly from ⏎ so
    // route.rs can map Shift+Enter → newline. Degrade gracefully — only push the
    // flags when supported; otherwise the Alt+⏎ fallback stands.
    let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if kbd_enhanced {
        // Best-effort: a failed push just means ⇧⏎ falls back to Alt+⏎ — it must
        // not skip the terminal restore below, so don't propagate with `?`.
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &mut app, &mut ui_rx).await;

    // Restore the terminal on every exit path — drive through errors, don't bail.
    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    result
}

/// Whether a zoom fold/unfold animation is still in flight (not yet past
/// `ZOOM_ANIM_MS`, and not short-circuited by reduced-motion).
fn zoom_animating(app: &App) -> bool {
    matches!(app.zoom_changed_at, Some(t0) if t0.elapsed().as_millis() < ZOOM_ANIM_MS as u128)
        && !app.shell.reduced_motion
}

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    ui_rx: &mut mpsc::Receiver<AgentUpdate>,
) -> Result<()> {
    let mut term_events = EventStream::new();

    let tick_period = std::time::Duration::from_millis(1000 / zoid_tui::motion::MOTION_FPS);
    let mut motion_tick = tokio::time::interval(tick_period);
    motion_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if app.last_git_refresh.elapsed() >= std::time::Duration::from_secs(1) {
            let (a, r, f) = git_status();
            app.shell.changes_added = a;
            app.shell.changes_removed = r;
            app.shell.changes_files = f;
            app.last_git_refresh = std::time::Instant::now();
        }
        let ledger = zoid_core::economy::token_ledger(&app.events);
        let window = zoid_core::context::context_window(&app.events);
        app.shell.session_tokens = ledger.total;
        app.shell.ctx_used = window.total_tokens;
        app.shell.duration = fmt_duration(app.session_started_ms, now_ms());
        app.shell.input_rows = app.textarea.lines().len().max(1) as u16;
        terminal.draw(|f| {
            let msgs = conversation(&app.events);
            let window = zoid_core::context::context_window(&app.events);
            let churn = zoid_core::economy::churn_timeline(&app.events);
            let ledger = zoid_core::economy::token_ledger(&app.events);
            // T10 (manual control: shell.policy / shell.economy_selected) is DEFERRED post-P3.
            // Until then the policy is config-derived (economy.rs) and there is no row
            // selection — the drawer is read-only/observability-only.
            let policy = policy_from_config(&app.economy, app.shell.ctx_ceiling);
            let economy = zoid_tui::EconomyView::build(&window, &churn, &ledger, &policy, 0);
            let elapsed = app.started.elapsed().as_millis() as u64;
            let caret = zoid_tui::motion::caret_on(elapsed, 1000, app.shell.reduced_motion);
            // Measure total lines (which re-runs conversation_view — tree-sitter in
            // Detail) ONLY while a zoom animation is actually in flight; on every
            // ordinary frame `reveal` is None and we skip the second build entirely.
            let reveal = match app.zoom_changed_at {
                Some(t0) => {
                    let elapsed_ms = t0.elapsed().as_millis() as u64;
                    if elapsed_ms < ZOOM_ANIM_MS && !app.shell.reduced_motion {
                        let total_lines = zoid_tui::chat::conversation_view(
                            &msgs,
                            &ChatView {
                                zoom: app.shell.zoom,
                                caret_on: caret,
                                reveal: None,
                                tz_offset_secs: app.tz_offset_secs,
                            },
                            app.streaming,
                        )
                        .len();
                        zoid_tui::motion::zoom_reveal(
                            total_lines,
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
            let view = ChatView {
                zoom: app.shell.zoom,
                caret_on: caret,
                reveal,
                tz_offset_secs: app.tz_offset_secs,
            };
            render_shell(
                f,
                &app.shell,
                &economy,
                &msgs,
                &app.textarea,
                app.streaming,
                &view,
            );
        })?;

        tokio::select! {
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
                        if handle_action(app, route_mouse(&app.shell, &layout, me)).await? {
                            return Ok(());
                        }
                    }
                    Some(Ok(_)) => { /* resize: redraw next loop */ }
                    Some(Err(_)) | None => return Ok(()),
                }
            }
            Some(update) = ui_rx.recv() => {
                match update {
                    AgentUpdate::Appended(ev) => {
                        if matches!(ev.kind, EventKind::DelegationResult { .. }) {
                            app.delegating = false;
                            app.shell.status_hint = None;
                        }
                        app.events.push(*ev);
                    }
                    AgentUpdate::TurnComplete => { app.streaming = false; }
                }
            }
            _ = motion_tick.tick(), if app.streaming || zoom_animating(app) => {
                // Wake to redraw the blinking caret or the in-flight zoom reveal. The
                // budget: this branch is disabled when nothing is animating, so an
                // idle app never ticks.
            }
        }
    }
}

async fn handle_action(app: &mut App, action: zoid_tui::route::Action) -> Result<bool> {
    use zoid_tui::route::Action;
    use zoid_tui::Overlay;
    match action {
        Action::Quit => return Ok(true),
        Action::SwitchMode => app.shell.toggle_mode(),
        Action::FocusNext => app.shell.focus_next(),
        Action::FocusRegion(f) => app.shell.focus = f,
        Action::OpenPalette => {
            app.shell.overlay = Overlay::Palette;
            app.shell.palette = Default::default();
        }
        Action::OpenCommandLine => {
            app.shell.overlay = Overlay::CommandLine;
            app.shell.cmdline = Default::default();
        }
        Action::CloseOverlay => app.shell.close_overlay(),
        Action::ToggleDrawer(id) => app.shell.toggle_drawer(id),
        Action::PaletteMove(d) => {
            let items = zoid_tui::palette::all_items(app.shell.mode);
            let n = zoid_tui::palette::selectable_matches(&items, &app.shell.palette.query).len();
            app.shell.palette.selected = zoid_tui::palette::nav(app.shell.palette.selected, d, n);
        }
        Action::PaletteChar(c) => {
            app.shell.palette.query.push(c);
            app.shell.palette.selected = 0;
        }
        Action::PaletteBackspace => {
            app.shell.palette.query.pop();
            app.shell.palette.selected = 0;
        }
        Action::PaletteRun => {
            let cmd = palette_selected_command(&app.shell);
            app.shell.close_overlay();
            if let Some(c) = cmd {
                return exec_command(app, c).await;
            }
        }
        Action::CmdlineChar(c) => app.shell.cmdline.buffer.push(c),
        Action::CmdlineBackspace => {
            app.shell.cmdline.buffer.pop();
        }
        Action::RunCommand(c) => {
            app.shell.close_overlay();
            return exec_command(app, c).await;
        }
        Action::ScrollConversation(d) => {
            let next = app.shell.conversation_scroll as i32 + d;
            app.shell.conversation_scroll = next.max(0) as u16;
        }
        Action::ZoomIn => {
            let before = app.shell.zoom;
            app.shell.zoom_in();
            if app.shell.zoom != before {
                app.zoom_changed_at = Some(std::time::Instant::now());
            }
        }
        Action::ZoomOut => {
            let before = app.shell.zoom;
            app.shell.zoom_out();
            if app.shell.zoom != before {
                app.zoom_changed_at = Some(std::time::Instant::now());
            }
        }
        Action::Newline => app.textarea.insert_newline(),
        Action::Edit(key) => {
            app.textarea.input(key);
        }
        Action::Submit => {
            if app.streaming || app.delegating {
                return Ok(false);
            }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() {
                return Ok(false);
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
        // Object-first picker (P4d ④): pick an object, then a verb scoped to
        // it. Picking a verb composes a prompt and queues it into the input
        // (see the `VerbPick` arm below) — dispatch to a subagent is P5.
        Action::OpenObjects => {
            app.shell.overlay = zoid_tui::Overlay::Objects;
            app.shell.objects = Default::default();
        }
        Action::ObjectMove(d) => {
            let n = zoid_tui::objects::selectable_objects(&conversation(&app.events)).len();
            app.shell.objects.obj_selected =
                zoid_tui::palette::nav(app.shell.objects.obj_selected, d, n);
        }
        Action::ObjectPick => {
            // Advance to the verb picker — but only if there's an object to act
            // on (otherwise the verb picker would show "(no object)").
            if !zoid_tui::objects::selectable_objects(&conversation(&app.events)).is_empty() {
                app.shell.overlay = zoid_tui::Overlay::Verbs;
                app.shell.objects.verb_selected = 0;
            }
        }
        Action::VerbBack => {
            // Step back to the object picker (keeps the object selection).
            app.shell.overlay = zoid_tui::Overlay::Objects;
        }
        Action::VerbMove(d) => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let sel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            let n = objs
                .get(sel)
                .map(|o| zoid_tui::objects::verbs_for(o.kind).len())
                .unwrap_or(0);
            app.shell.objects.verb_selected =
                zoid_tui::palette::nav(app.shell.objects.verb_selected, d, n);
        }
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let task = zoid_tui::objects::verb_prompt(verb, obj);
                    app.shell.close_overlay();
                    start_delegation(app, task); // dispatches (P5) — closes P4d's "queued"
                    return Ok(false);
                }
            }
            app.shell.close_overlay();
        }
        Action::SessionMove(d) => {
            app.shell.session_selected =
                zoid_tui::palette::nav(app.shell.session_selected, d, app.shell.sessions.len());
        }
        Action::SessionPick => {
            if app.streaming || app.delegating {
                app.shell.status_hint = Some("finish the current turn first".into());
                app.shell.close_overlay();
                return Ok(false);
            }
            if let Some(&sid) = app.session_ids.get(app.shell.session_selected) {
                app.session.touch_session(sid, now_ms()).await.ok();
                app.session_id = sid;
                app.events = app.session.snapshot_session(sid).await.unwrap_or_default();
                app.shell.conversation_scroll = 0;
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
            }
            app.shell.close_overlay();
        }
        Action::Noop => {}
    }
    Ok(false)
}

async fn exec_command(app: &mut App, cmd: zoid_tui::command::Command) -> Result<bool> {
    use zoid_tui::command::Command;
    match cmd {
        Command::Quit => Ok(true),
        Command::SwitchMode(m) => {
            app.shell.set_mode(m);
            Ok(false)
        }
        Command::OpenDrawer(id) => {
            app.shell.open_drawer(id);
            Ok(false)
        }
        Command::NewSession => {
            if app.streaming || app.delegating {
                app.shell.status_hint = Some("finish the current turn first".into());
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
            app.events.clear();
            app.shell.conversation_scroll = 0;
            Ok(false)
        }
        Command::RenameSession(name) => {
            if name.is_empty() {
                // Seed the command line so the user types the name.
                app.shell.overlay = zoid_tui::Overlay::CommandLine;
                app.shell.cmdline.buffer = "rename ".into();
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
            app.shell.session_selected = 0;
            app.shell.overlay = zoid_tui::Overlay::Sessions;
            Ok(false)
        }
        Command::Delegate(task) => {
            start_delegation(app, task);
            Ok(false)
        }
        Command::Unknown(_) => Ok(false),
    }
}

/// Dispatch `task` to a single subagent (spec §6). One at a time. Non-trivial:
/// runs in an isolated git worktree (falls back to cwd if not a repo); its
/// DelegationResult folds back as a card. (Trivial edits use the normal inline
/// chat path — this is the explicit delegate path only.)
fn start_delegation(app: &mut App, task: String) {
    if app.streaming || app.delegating {
        app.shell.status_hint = Some("busy · one subagent at a time".into());
        return;
    }
    if task.trim().is_empty() {
        app.shell.status_hint = Some("usage: :delegate <task>".into());
        return;
    }
    app.delegating = true;
    app.shell.status_hint = Some(format!("{} delegating…", zoid_tui::tokens::glyph::RUNNING));

    // Create the isolated worktree up front so a genuine failure (a real repo
    // where worktree creation failed) can surface a hint; "not a git repo"
    // falls back to the process cwd silently (isolation isn't possible there).
    let wt = if Path::new(".git").exists() {
        match zoid::worktree::create_worktree(Path::new("."), &format!("sub-{}", Ulid::new())) {
            Ok(w) => Some(w),
            Err(_) => {
                app.shell.status_hint = Some(format!(
                    "{} worktree failed — running in the main tree",
                    zoid_tui::tokens::glyph::WARNING
                ));
                None
            }
        }
    } else {
        None // not a git repo: run in the process cwd, isolation not possible
    };
    let cwd = wt
        .as_ref()
        .map(|w| w.path().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let provider = app.provider.clone();
    let session = app.session.clone();
    let session_id = app.session_id;
    let seed = app.events.clone(); // context for construction (B3)
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        let res = zoid::subagent::run_subagent(
            &task,
            &seed,
            &zoid_core::agent_profile::AgentProfile::builtin(),
            provider,
            cwd,
            model,
            session.clone(),
            session_id,
            ui.clone(),
            now_ms,
        )
        .await;
        // WorktreeGuard `wt` drops here → worktree cleaned up (isolation preserved
        // even on failure — the main copy never saw the subagent's edits).
        drop(wt);

        let (branch, summary, ok) = match res {
            Ok(r) => (r.branch, r.summary, r.ok),
            Err(e) => (String::new(), format!("delegation failed: {e}"), false),
        };
        // Record the outcome on the MAIN branch, tagged to this session, so
        // conversation() folds it (Plan 2 seam: untagged events land in the nil
        // session and never surface).
        let ev = Event::new(
            Ulid::new(),
            None,
            now_ms(),
            EventKind::DelegationResult {
                branch,
                summary,
                ok,
            },
        )
        .with_session(session_id);
        let _ = session.append(ev.clone()).await;
        let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    });
}

fn spawn_turn(app: &App) {
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    let session_id = app.session_id;
    tokio::spawn(async move {
        let _ = run_agent_turn(
            zoid::agent::chat_turn_config(),
            provider,
            tools,
            session,
            seed,
            model,
            ui,
            session_id,
            now_ms,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};
    use tui_textarea::TextArea;

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
            context_ceiling: Some(200_000),
            auto_evict_cold: false,
            compact_threshold_pct: 80,
            token_ceiling: Some(50_000),
        };
        let p = policy_from_config(&econ, 200_000);
        assert!(!p.auto_evict_cold);
        assert_eq!(p.token_ceiling, Some(50_000));
        assert_eq!(p.compact_threshold, Some(160_000)); // 80% of 200k
                                                          // 0% disables compaction
        let econ0 = zoid_core::config::EconomyConfig {
            compact_threshold_pct: 0,
            ..econ
        };
        assert_eq!(policy_from_config(&econ0, 200_000).compact_threshold, None);
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
            events: Vec::new(),
            provider: Arc::new(zoid_provider::FakeProvider::new(Vec::new())),
            tools: Arc::new(Vec::new()),
            model: "test-model".into(),
            economy: zoid_core::config::EconomyConfig::default(),
            textarea: make_input(TextArea::default()),
            streaming: false,
            shell: zoid_tui::ShellState::new(),
            ui_tx,
            started: std::time::Instant::now(),
            last_git_refresh: std::time::Instant::now(),
            zoom_changed_at: None,
            tz_offset_secs: 0,
            session_started_ms: 0,
            session_ids: Vec::new(),
            delegating: false,
        }
    }

    /// Regression for the busy-guard bug: `Action::Submit` must be a no-op
    /// while a delegation is in flight (`app.delegating`), symmetric with
    /// `start_delegation`'s `app.streaming || app.delegating` check. Before the
    /// fix, `Submit` only checked `app.streaming`, so a chat turn could be
    /// submitted while a subagent delegation was running — both turns would
    /// then race to send `AgentUpdate::TurnComplete` on the same `ui_tx`,
    /// letting the subagent's completion clear `app.streaming` mid-chat-turn.
    #[tokio::test]
    async fn submit_is_noop_while_delegating() {
        let mut app = test_app().await;
        app.delegating = true;
        app.textarea = make_input(TextArea::from(vec!["hello".to_string()]));

        let quit = handle_action(&mut app, zoid_tui::route::Action::Submit)
            .await
            .unwrap();

        assert!(!quit, "Submit must not signal quit");
        assert!(
            app.delegating,
            "delegating flag must be untouched by a blocked Submit"
        );
        assert!(
            !app.streaming,
            "streaming must stay false — no turn was spawned"
        );
        assert!(
            app.events.is_empty(),
            "no UserMessage should be recorded while delegating"
        );
        // The textarea must be left alone (not cleared) since nothing was submitted.
        assert_eq!(app.textarea.lines(), &["hello".to_string()]);
    }

    /// Regression for I-1: `Action::SessionPick` must be a no-op while a
    /// delegation is in flight, symmetric with `Submit`/`start_delegation`'s
    /// `app.streaming || app.delegating` guard. Before the fix, `SessionPick`
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

        app.delegating = true;
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
            app.delegating,
            "delegating flag must be untouched by a blocked SessionPick"
        );
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("finish the current turn first"),
            "blocked SessionPick should surface the busy hint"
        );
    }

    /// Regression for I-1: `Command::NewSession` must be a no-op while a
    /// delegation is in flight, mirroring the `SessionPick`/`Submit` guard.
    #[tokio::test]
    async fn new_session_is_noop_while_delegating() {
        let mut app = test_app().await;
        let original_session_id = app.session_id;
        app.delegating = true;

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
            app.delegating,
            "delegating flag must be untouched by a blocked NewSession"
        );
        assert_eq!(
            app.shell.status_hint.as_deref(),
            Some("finish the current turn first"),
            "blocked NewSession should surface the busy hint"
        );
    }
}
