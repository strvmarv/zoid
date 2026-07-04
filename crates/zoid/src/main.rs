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
use ratatui::{layout::Rect, prelude::CrosstermBackend, text::Line, Terminal};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tui_textarea::{CursorMove, TextArea};
use ulid::Ulid;

mod obs;

use zoid::agent::{run_agent_turn_cancellable, AgentUpdate};
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

/// Delete the whole line the cursor sits on (§30, ⇧Delete). Column-independent
/// (snaps to the line head first) and collapses the buffer by one row in every
/// multi-line case, so a repeated ⇧Delete keeps eating lines upward.
///
/// The subtlety: `delete_line_by_end()` is not purely "clear to end of line" —
/// on a line with nothing left to clear it *eagerly merges the next line up*
/// (tui-textarea 0.7 textarea.rs:1206). Composing it with an unconditional
/// second merge therefore double-deletes on empty lines. We branch on the line's
/// emptiness (captured before mutating) and handle the three positions explicitly.
fn input_delete_line(textarea: &mut TextArea<'static>) {
    let (row, _) = textarea.cursor();
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
    let ledger = zoid_core::economy::token_ledger(&app.events);
    // Prompt-cache hit rate = cache-read as a % of input tokens (economy).
    let cache_hit_pct = if ledger.input == 0 {
        0
    } else {
        (ledger.cached * 100 / ledger.input).min(100) as u8
    };
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
        render_cache_pct: if s.cache_total == 0 {
            0
        } else {
            (s.cache_hits * 100 / s.cache_total).min(100) as u8
        },
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
    let msgs = conversation(&app.events);
    let hits = zoid_tui::chat::code_hits(&msgs, app.streaming, true, app.tz_offset_secs, width);
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

/// Whether a provider id needs an API key to be usable. Local Ollama (localhost)
/// does not; all remote HTTP flavors do. Hardcoded shortcut: `ollama-local` is
/// the only keyless `Available` provider today. Revisit against the registry if
/// `anthropic-cli`/`anthropic-sdk` (ambient auth, no API key) become selectable.
fn entry_requires_key(id: &str) -> bool {
    id != "ollama-local"
}

/// The secret env name a provider id needs, or `None` if it needs no key.
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
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
                zoid_provider::ollama::OllamaProvider::new(String::new()).with_base_url(base_url),
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
        match provider.fetch_model_info(&model).await {
            Ok(Some(info)) => {
                let _ = ui_tx
                    .send(AgentUpdate::ModelInfoFetched { model, info })
                    .await;
            }
            _ => {} // error or None → keep the static fallback
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
        "anthropic" => key_for("ANTHROPIC_API_KEY").map(|k| {
            Arc::new(zoid_provider::anthropic::AnthropicProvider::new(k).with_base_url(base_url))
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
/// resolved token `ceiling` — 0 disables compaction (`None`), else the
/// absolute token count `ceiling * pct / 100`.
///
/// Feeds `spawn_turn`'s live `TurnConfig.policy` (ACM-1), so the agent loop
/// actually records `ToolResultCompacted` events once `compact_threshold_pct`
/// is set above 0; the default (0) leaves existing chat behavior unchanged.
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
}

impl ProjectionCache {
    /// Refresh all projections iff the event count changed; a cheap no-op
    /// otherwise. Returns `true` when a recompute happened.
    fn refresh(&mut self, events: &[Event]) -> bool {
        if self.events_len == Some(events.len()) {
            return false;
        }
        self.msgs = conversation(events);
        self.window = zoid_core::context::context_window(events);
        self.churn = zoid_core::economy::churn_timeline(events);
        self.tasks = zoid_core::tasks::tasks(events);
        let ledger = zoid_core::economy::token_ledger(events);
        self.ledger_total = ledger.total;
        self.cached_total = ledger.cached;
        // Find the last Usage event's real input token count — the provider's
        // actual prompt size, far more accurate than the chars/4 estimate.
        self.last_input_tokens = events
            .iter()
            .rev()
            .find_map(|e| e.tokens.map(|t| t.input))
            .filter(|&t| t > 0);
        self.events_len = Some(events.len());
        true
    }

    /// Incrementally apply a single new event to the cached `msgs` projection.
    /// Handles `ModelDelta` (append text to the last assistant message) and
    /// `ToolCall` (append to its tool_calls) in O(1). Returns `true` when the
    /// event was applied incrementally. For all other event kinds, returns
    /// `false` — the caller must do a full `refresh` on the next frame.
    fn apply_streaming(&mut self, ev: &Event) -> bool {
        use zoid_core::event::EventKind;
        use zoid_core::projection::{ChatMsg, ToolCallRef};
        match &ev.kind {
            EventKind::ModelDelta { text } => {
                if let Some(ChatMsg::Assistant { text: t, .. }) = self.msgs.last_mut() {
                    t.push_str(text);
                } else {
                    self.msgs.push(ChatMsg::Assistant {
                        text: text.clone(),
                        tool_calls: Vec::new(),
                        ts: ev.ts,
                    });
                }
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                true
            }
            EventKind::ToolCall { id, name, args } => {
                if let Some(ChatMsg::Assistant { tool_calls, .. }) = self.msgs.last_mut() {
                    tool_calls.push(ToolCallRef {
                        id: id.clone(),
                        name: name.clone(),
                        args: args.clone(),
                    });
                }
                self.events_len = Some(self.events_len.unwrap_or(0) + 1);
                true
            }
            _ => false, // structural event — needs a full refresh
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
    fn refresh(
        &mut self,
        key: BodyKey,
        msgs: &[zoid_core::projection::ChatMsg],
        width: usize,
    ) -> bool {
        // Full no-op only when not streaming and nothing changed.
        if !key.streaming && self.key.as_ref() == Some(&key) && self.msg_count == msgs.len() {
            return true;
        }
        let view = ChatView {
            zoom: key.zoom,
            caret_on: key.caret,
            reveal: None,
            tz_offset_secs: key.tz,
        };
        // Incremental streaming: same message count, only the last message's
        // text is growing. Re-render just the last message and splice it in.
        if self.key.as_ref() == Some(&key)
            && self.msg_count == msgs.len()
            && self.msg_count > 0
            && key.streaming
        {
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
                key.streaming,
                width,
            );
            // conversation_view_indexed appends a trailing blank; we want it.
            self.body.extend(new_lines);
            // An incremental re-render is render work, not a pure cache hit.
            return false;
        }
        // Full rebuild.
        let (body, starts) =
            zoid_tui::chat::conversation_view_indexed(msgs, &view, key.streaming, width);
        self.body = body;
        self.msg_starts = starts;
        self.msg_count = msgs.len();
        self.key = Some(key);
        false
    }
}

struct App {
    session: SessionHandle,
    session_id: Ulid,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    /// Available mode profiles with the active one marked; drives the turn's
    /// system prompt. v1 holds only the default profile.
    profiles: zoid_core::agent_profile::AgentProfileRegistry,
    /// Skills the `invoke_skill` tool can load; also rendered as the menu the
    /// active mode's system prompt advertises.
    skills: std::sync::Arc<zoid_core::skill::SkillRegistry>,
    model: String,
    /// Economy config (spec §7.2), carried from `load_config()` so `run`'s
    /// per-frame `ContextPolicy` build (via `policy_from_config`) doesn't need
    /// its own copy of `main`'s `config` local.
    economy: zoid_core::config::EconomyConfig,
    /// Full resolved config + provenance, kept live so the config screen can
    /// display current values and so edits reload/re-render without a restart.
    config: zoid_core::config::Config,
    prov: zoid_core::config::Provenance,
    /// Encrypted secret store (None → unavailable this run; secret edits no-op
    /// with a stderr note). Shared with the provider credential lookup.
    secrets: Option<std::sync::Arc<zoid_core::secret::EncryptedDb>>,
    textarea: TextArea<'static>,
    streaming: bool,
    shell: zoid_tui::ShellState,
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
    /// One subagent at a time (spec §6): set while a `:delegate` dispatch (or a
    /// verb-picked task) is in flight; cleared when its `DelegationResult` lands.
    delegating: bool,
    /// The reply channel for an in-flight `ask_user` question (Task 11): `Some`
    /// while the question overlay is up. Dropping it (Esc-abort) makes the
    /// agent loop record a balanced "[user aborted]" result and end the turn.
    pending_answer: Option<tokio::sync::oneshot::Sender<zoid::agent::Answer>>,
    /// Cancellation token for the in-flight chat turn (`Some` while streaming).
    /// Firing it (Esc/Ctrl-C via `Action::CancelTurn`) makes the agent loop
    /// drain any pending tool calls and end the turn cleanly; cleared on
    /// `TurnComplete`.
    turn_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Dynamically-fetched model capabilities (from Ollama `/api/show` etc.),
    /// overriding the static MODEL_CAPS table. `None` until the first fetch
    /// lands (or when the provider doesn't support capability introspection).
    fetched_model_info: Option<zoid_provider::model::ModelInfo>,
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

    match zoid::cli::parse_args(std::env::args().skip(1)) {
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
        zoid::cli::Cli::Unknown(arg) => {
            eprintln!(
                "zoid: unrecognized argument '{arg}'\n\n{}",
                zoid::cli::help_text()
            );
            std::process::exit(2);
        }
        zoid::cli::Cli::Run => {}
    }

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

    let (config, prov) = load_config();
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
    // The Repo drawer only makes sense inside a git work tree; outside one it
    // showed a fabricated "main" branch and zero changes (§16, task #38). Detect
    // once at startup: populate + keep the drawer when present, drop it when not.
    let repo_present = in_git_repo();
    if repo_present {
        shell.branch = current_branch();
        shell.repo_name = Path::new(&root)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.clone());
        let (boot_added, boot_removed, boot_files) = git_status();
        shell.changes_added = boot_added;
        shell.changes_removed = boot_removed;
        shell.changes_files = boot_files;
    } else {
        shell.remove_drawer(zoid_tui::DrawerId::Repo);
    }
    shell.session_name = session_name;
    shell.model = model.clone();
    // Economy ⑤ denominator: config-derived (ZOID_CONTEXT_CEILING overrides via
    // config.economy.context_ceiling), else the model registry's default.
    // Constant for the process lifetime, so set once here rather than per frame.
    shell.ctx_ceiling = config
        .economy
        .context_ceiling
        .unwrap_or_else(|| zoid_provider::context_ceiling(&model));
    shell.ctx_ceiling_overridden = config.economy.context_ceiling.is_some();
    shell.provider = provider_label(provider_name, has_key);
    shell.cache_supported = zoid_provider::has_prompt_cache(&model);
    shell.cwd = root.clone();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    let skills = {
        let cfg_dir = resolve_config_dir(|k: &str| std::env::var(k).ok());
        let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
        let dirs = zoid::skill_import::resolve_skill_dirs(
            &config.skills.source_dirs,
            &cfg_dir,
            std::path::Path::new(&root),
            home.as_deref(),
        );
        std::sync::Arc::new(zoid::skill_import::build_registry(&dirs))
    };

    let mut app = App {
        session,
        session_id,
        events,
        provider,
        tools: Arc::new(zoid::invoke_skill::chat_tools(skills.clone())),
        profiles: zoid_core::agent_profile::AgentProfileRegistry::new(vec![
            zoid::agent::default_profile(),
        ]),
        skills,
        model,
        economy: config.economy,
        config: config.clone(),
        prov,
        secrets: secrets.clone(),
        textarea: make_input(TextArea::default()),
        streaming: false,
        shell,
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
        delegating: false,
        pending_answer: None,
        turn_cancel: None,
        fetched_model_info: None,
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

    // Fetch the active model's capabilities (context window, prompt cache) from
    // the provider's introspection endpoint. The static MODEL_CAPS table is the
    // fallback until this lands (or if the provider doesn't support it).
    spawn_model_info_fetch(
        app.provider.clone(),
        app.model.clone(),
        app.ui_tx.clone(),
    );

    let result = run(&mut terminal, &mut app, &mut ui_rx, &obs.state).await;

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

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    ui_rx: &mut mpsc::Receiver<AgentUpdate>,
    obs_state: &std::sync::Arc<std::sync::Mutex<obs::ObsState>>,
) -> Result<()> {
    let mut term_events = EventStream::new();

    let tick_period = std::time::Duration::from_millis(1000 / zoid_tui::motion::MOTION_FPS);
    let mut motion_tick = tokio::time::interval(tick_period);
    motion_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Off-load `git status` to a background task so the subprocess never blocks
    // the render loop (it previously ran synchronously on the loop every second,
    // hitching typing/scrolling). The loop reads the latest value non-blocking.
    let (git_tx, mut git_rx) = tokio::sync::watch::channel((0usize, 0usize, 0usize));
    // Only poll git when the Repo drawer is actually present; outside a repo the
    // stats are neither shown nor meaningful, so we skip the per-second `git`
    // subprocess entirely. The drawer's presence is the git-repo signal decided
    // at startup. The receiver still reads its initial (0, 0, 0).
    if app.shell.drawer(zoid_tui::DrawerId::Repo).is_some() {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tick.tick().await;
                let r = tokio::task::spawn_blocking(git_status)
                    .await
                    .unwrap_or((0, 0, 0));
                if git_tx.send(r).is_err() {
                    break; // receiver dropped — app is exiting
                }
            }
        });
    }

    loop {
        // Latest git status from the background watcher (non-blocking read).
        {
            let (a, r, f) = *git_rx.borrow_and_update();
            app.shell.changes_added = a;
            app.shell.changes_removed = r;
            app.shell.changes_files = f;
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
        app.shell.input_rows = app.textarea.lines().len().max(1) as u16;
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
        } else {
            Some(app.body_cache.refresh(
                BodyKey {
                    zoom,
                    width: body_w,
                    streaming,
                    caret: streaming && caret,
                    tz,
                },
                &app.proj.msgs,
                body_w,
            ))
        };

        // Tail-follow: when engaged, pin the viewport to the latest line before
        // drawing — this is what makes the view show the latest output on startup
        // and follow new events (including live streaming) as they append. Applied
        // after the cross-zoom anchor below, so following the tail wins over the
        // anchor. max_scroll mirrors render's clamp: body length (+ the in-flight
        // tool line) minus the visible conversation height.
        let active_extra = usize::from(app.shell.active_tool.is_some());
        // Scroll math reuses the active body's length (Overview dashboard or the
        // cached transcript), so the scrollbar/clamp work unchanged at every altitude.
        let body_len = if is_overview {
            app.overview_body.len()
        } else {
            app.body_cache.body.len()
        };
        let max_scroll = (body_len + active_extra)
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
        app.shell.busy = app.streaming || app.delegating;
        // Only a chat turn carries a cancellation token; delegation has none, so
        // Esc/Ctrl-C routes to CancelTurn only while this is true (and keeps its
        // normal focus behavior during a delegation).
        app.shell.cancellable = app.turn_cancel.is_some();
        app.shell.spinner = zoid_tui::tokens::glyph::SPINNER[zoid_tui::motion::spinner_frame(
            elapsed,
            80,
            zoid_tui::tokens::glyph::SPINNER.len(),
            app.shell.reduced_motion,
        )];

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
                        // A tool result ends the in-flight indicator for that tool.
                        if matches!(ev.kind, EventKind::ToolResult { .. }) {
                            app.shell.clear_active_tool();
                        }
                        // Incremental streaming: ModelDelta and ToolCall events
                        // append directly into the cached ChatMsg vec in O(1)
                        // instead of triggering a full O(n) conversation() fold
                        // on the next frame. Structural events (ToolResult,
                        // Usage, etc.) return false and get a full refresh.
                        if !app.proj.apply_streaming(&ev) {
                            // Structural event — invalidate the projection cache
                            // AND the body cache so both do a full rebuild on
                            // the next frame. Compaction events replace content
                            // in existing messages (same count) so the BodyCache's
                            // msg_count check would skip the rebuild without this.
                            app.proj.events_len = None;
                            app.body_cache.key = None;
                        }
                        app.events.push(*ev);
                    }
                    AgentUpdate::ToolStarted { name } => {
                        app.shell.set_active_tool(name);
                    }
                    AgentUpdate::TurnComplete => {
                        app.streaming = false;
                        app.shell.clear_active_tool();
                        app.pending_answer = None;
                        app.turn_cancel = None;
                        // Clear any lingering "cancelling…" hint now the turn ended.
                        app.shell.status_hint = None;
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
                            "main: AskUser received, raising Question overlay (choices={})",
                            choices.len()
                        );
                        app.shell.question =
                            Some(zoid_tui::question::QuestionState::new(question, choices));
                        app.shell.overlay = zoid_tui::state::Overlay::Question;
                        app.pending_answer = Some(reply);
                    }
                    AgentUpdate::ModelsFetched { provider, models } => {
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
                            // Live-apply the context ceiling (unless the user
                            // set an explicit override in config).
                            app.shell.ctx_ceiling = app
                                .config
                                .economy
                                .context_ceiling
                                .unwrap_or(info.context_window);
                            app.shell.ctx_ceiling_overridden =
                                app.config.economy.context_ceiling.is_some();
                            app.shell.cache_supported = info.prompt_cache;
                        }
                    }
                }
            }
            _ = motion_tick.tick(), if app.streaming || app.delegating || app.zoom_changed_at.is_some() => {
                // Wake to redraw the blinking caret or the activity spinner (which
                // animates while streaming OR delegating). Zoom is instant now (the
                // reveal animation was retired for cross-zoom anchoring), so
                // `zoom_changed_at` stays None; the guard is left in place harmlessly.
                // Idle + not-streaming never ticks.
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
    /// Unsigned int; empty / "(none)" / unparseable removes the key (ceilings).
    U64Unset,
    /// Percent clamped to 0..=100; unparseable is a no-op (compact at %).
    U8Pct,
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
        "context ceiling" => FieldTarget::Toml {
            key: "economy.context_ceiling",
            ty: TomlTy::U64Unset,
        },
        "compact at %" => FieldTarget::Toml {
            key: "economy.compact_threshold_pct",
            ty: TomlTy::U8Pct,
        },
        "token ceiling" => FieldTarget::Toml {
            key: "economy.token_ceiling",
            ty: TomlTy::U64Unset,
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
        "context ceiling" => ("economy.context_ceiling", opt_u64(econ.context_ceiling)),
        "auto-evict cold" => (
            "economy.auto_evict_cold",
            TomlValue::Bool(econ.auto_evict_cold),
        ),
        "compact at %" => (
            "economy.compact_threshold_pct",
            TomlValue::Int(econ.compact_threshold_pct as i64),
        ),
        "token ceiling" => ("economy.token_ceiling", opt_u64(econ.token_ceiling)),
        "reduced motion" => ("reduced_motion", TomlValue::Bool(app.config.reduced_motion)),
        _ => return None,
    })
}

/// The (label, kind) of the row under the config cursor, if any.
fn current_config_field(app: &App) -> Option<(&'static str, zoid_tui::config_view::FieldKind)> {
    app.shell
        .config_sections
        .get(app.shell.config_section)
        .and_then(|s| s.rows.get(app.shell.config_field))
        .map(|r| (r.label, r.kind.clone()))
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
    if current_config_field(app).map(|(l, _)| l) != Some("model") {
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
    let (c, p) = load_config();
    app.config = c;
    app.prov = p;
    app.economy = app.config.economy;
    refresh_config_sections(app);
    // Live-apply the bits the running UI caches (economy auto-applies on the
    // next turn via spawn_turn's policy_from_config(&app.economy, ...)).
    app.shell.reduced_motion = app.config.reduced_motion;
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
    app.shell.ctx_ceiling = app
        .config
        .economy
        .context_ceiling
        .unwrap_or_else(|| zoid_provider::context_ceiling(&app.model));
    app.shell.ctx_ceiling_overridden = app.config.economy.context_ceiling.is_some();
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
            let items = zoid_tui::palette::all_items(app.shell.mode);
            let n = zoid_tui::palette::selectable_matches(&items, &app.shell.palette.query).len();
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
                match palette_selected_command(&app.shell) {
                    // Parameterized command → enter inline Arg phase, stay open.
                    Some(cmd) => match zoid_tui::palette::arg_kind_for(&cmd) {
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
                    },
                    // No matching row → do nothing (overlay stays open).
                    None => {}
                }
            }
            zoid_tui::state::PaletteStage::Arg { kind, input } => {
                // Blank argument (empty or whitespace-only) is a no-op — cannot
                // rename to empty; stay in Arg. Trim so a padded entry stores a
                // clean name.
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
        Action::CmdlineChar(c) => app.shell.cmdline.buffer.push(c),
        Action::CmdlineBackspace => {
            app.shell.cmdline.buffer.pop();
        }
        Action::RunCommand(c) => {
            app.shell.close_overlay();
            return exec_command(app, c).await;
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
        Action::CancelTurn => {
            // Esc/Ctrl-C while a chat turn is streaming: fire the token. The
            // agent loop drains any pending tool calls and ends the turn
            // cleanly; the resulting TurnComplete clears streaming + the token.
            if let Some(cancel) = &app.turn_cancel {
                cancel.cancel();
                app.shell.status_hint = Some("cancelling…".into());
            }
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
                app.events = loaded;
                // Wholesale event-log replacement: reset the caches so they
                // can't serve the previous session's data at an equal length.
                app.proj = ProjectionCache::default();
                app.body_cache = BodyCache::default();
                app.shell.conversation_scroll = 0;
                app.shell.follow_tail = true; // jump to the latest of the loaded session
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
            if let (Some((label, kind)), Some(buffer)) =
                (current_config_field(app), app.shell.config_edit.clone())
            {
                match field_target(label, &kind) {
                    Some(FieldTarget::Secret) => {
                        if let Some(s) = &app.secrets {
                            use zoid_core::secret::SecretStore;
                            if let Err(e) = s.set(label, &buffer) {
                                eprintln!("zoid: secret set failed for {label}: {e}");
                            }
                        } else {
                            eprintln!("zoid: secret store unavailable; cannot set {label}");
                        }
                        refresh_config_sections(app);
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
            if let Some((label, _kind)) = current_config_field(app) {
                let write = match label {
                    "auto-evict cold" => Some((
                        "economy.auto_evict_cold",
                        !app.config.economy.auto_evict_cold,
                    )),
                    "reduced motion" => Some(("reduced_motion", !app.config.reduced_motion)),
                    _ => None,
                };
                if let Some((key, new)) = write {
                    apply_config_write(app, key, TomlValue::Bool(new), false);
                }
            }
        }
        Action::ConfigDrillOpen => {
            use zoid_tui::state::ConfigCol;
            if let Some((label, _)) = current_config_field(app) {
                app.shell.config_picker = match label {
                    "provider" => zoid_tui::config_view::provider_options(&app.config.provider),
                    "model" => zoid_tui::config_view::model_options(
                        &app.config.provider,
                        &app.config.model,
                    ),
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
            let label = current_config_field(app).map(|(l, _)| l).unwrap_or("");
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
                } else if label == "model" {
                    apply_config_write(app, "model", TomlValue::Str(id), false);
                    app.shell.config_picker.clear();
                    app.shell.config_col = ConfigCol::Fields;
                }
            }
        }
        Action::ConfigSaveToRepo => {
            use zoid_tui::config_view::FieldKind;
            if let Some((label, kind)) = current_config_field(app) {
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
            if let Some((label, kind)) = current_config_field(app) {
                if matches!(kind, FieldKind::Secret) {
                    if let Some(s) = &app.secrets {
                        use zoid_core::secret::SecretStore;
                        if let Err(e) = s.clear(label) {
                            eprintln!("zoid: secret clear failed for {label}: {e}");
                        }
                    } else {
                        eprintln!("zoid: secret store unavailable; cannot clear {label}");
                    }
                    refresh_config_sections(app);
                }
            }
        }
        Action::QuestionMove(d) => {
            if let Some(q) = &mut app.shell.question {
                let len = q.rows().len();
                q.selected = zoid_tui::palette::nav(q.selected, d, len);
            }
        }
        Action::QuestionChar(c) => {
            if let Some(q) = &mut app.shell.question {
                q.free_text.push(c);
            }
        }
        Action::QuestionBackspace => {
            if let Some(q) = &mut app.shell.question {
                q.free_text.pop();
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
            // balanced "[user aborted]" result and end the turn.
            app.pending_answer = None; // drop the Sender
            app.shell.question = None;
            app.shell.overlay = zoid_tui::state::Overlay::None;
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
        Action::Noop => {}
    }
    Ok(false)
}

/// Send the user's answer down the `ask_user` reply channel and close the
/// overlay. A no-op if the channel was already consumed/dropped (e.g. a
/// double-fire race), matching the other overlay-close handlers' style.
fn answer_question(app: &mut App, ans: zoid::agent::Answer) {
    if let Some(tx) = app.pending_answer.take() {
        let _ = tx.send(ans);
    }
    app.shell.question = None;
    app.shell.overlay = zoid_tui::state::Overlay::None;
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
            // New session: reset the caches (clear() may leave the same length).
            app.proj = ProjectionCache::default();
            app.body_cache = BodyCache::default();
            app.shell.conversation_scroll = 0;
            app.shell.follow_tail = true; // new session starts pinned to the latest
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
        Command::OpenConfig => {
            app.shell.overlay = zoid_tui::Overlay::Config;
            app.shell.config_section = 0;
            app.shell.config_field = 0;
            app.shell.config_edit = None;
            refresh_config_sections(app);
            Ok(false)
        }
        Command::ShowOverview => {
            app.shell.zoom = zoid_tui::state::Zoom::Overview;
            app.shell.conversation_scroll = 0;
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

fn spawn_turn(app: &mut App) {
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    let session_id = app.session_id;
    let profile = app.profiles.active();
    let menu = app.skills.menu();
    let mut turn_config = zoid::agent::chat_turn_config_with(profile, &menu);
    turn_config.policy = policy_from_config(&app.economy, app.shell.ctx_ceiling);
    // Mint a fresh cancellation token for this turn and keep a clone so
    // `Action::CancelTurn` (Esc/Ctrl-C) can fire it. Cleared on `TurnComplete`.
    let cancel = tokio_util::sync::CancellationToken::new();
    app.turn_cancel = Some(cancel.clone());
    tokio::spawn(async move {
        let _ = run_agent_turn_cancellable(
            turn_config,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            seed,
            model,
            ui,
            session_id,
            now_ms,
            cancel,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};
    use tui_textarea::TextArea;

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
        // context ceiling → economy.context_ceiling, uint-with-unset.
        assert_eq!(
            field_target("context ceiling", &FieldKind::Uint),
            Some(FieldTarget::Toml {
                key: "economy.context_ceiling",
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
        assert_eq!(base_url_write_for("anthropic-cli"), TomlValue::Unset); // Cli → clear base_url
    }

    #[test]
    fn write_config_file_round_trips_through_temp_dir() {
        use zoid_core::config::{parse_toml, TomlValue};
        let dir = tempfile::tempdir().unwrap();
        // Parent dir does not exist yet — write_config_file must create it.
        let path = dir.path().join("nested").join("config.toml");
        write_config_file(&path, "reduced_motion", TomlValue::Bool(true)).unwrap();
        let parsed = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.reduced_motion, Some(true));
        // A nested-table write preserves the earlier top-level key.
        write_config_file(&path, "economy.context_ceiling", TomlValue::Int(200_000)).unwrap();
        let parsed = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.reduced_motion, Some(true));
        assert_eq!(parsed.economy.context_ceiling, Some(200_000));
        // Unset removes the key again.
        write_config_file(&path, "economy.context_ceiling", TomlValue::Unset).unwrap();
        let parsed = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.economy.context_ceiling, None);
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
            profiles: zoid_core::agent_profile::AgentProfileRegistry::new(vec![
                zoid::agent::default_profile(),
            ]),
            skills: std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            model: "test-model".into(),
            economy: zoid_core::config::EconomyConfig::default(),
            config: zoid_core::config::Config::default(),
            prov: {
                use zoid_core::config::Source;
                zoid_core::config::Provenance {
                    provider: Source::Default,
                    base_url: Source::Default,
                    model: Source::Default,
                    context_ceiling: Source::Default,
                    auto_evict_cold: Source::Default,
                    compact_threshold_pct: Source::Default,
                    token_ceiling: Source::Default,
                    reduced_motion: Source::Default,
                }
            },
            secrets: None,
            textarea: make_input(TextArea::default()),
            streaming: false,
            shell: zoid_tui::ShellState::new(),
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
            delegating: false,
            pending_answer: None,
            turn_cancel: None,
            fetched_model_info: None,
        }
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
        let mut events = vec![mk("hello there friend")];
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

    /// `apply_models_fetched` replaces the OPEN model picker's options with the
    /// live list, seeding the selection cursor on the current model; an empty
    /// fetch result is a no-op (the static registry fallback is kept).
    #[tokio::test]
    async fn apply_models_fetched_replaces_open_model_picker() {
        let mut app = test_app().await;
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
        assert_eq!(current_config_field(&app).map(|(l, _)| l), Some("model"));
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
        let mut c = Config::default(); // provider = "ollama" (legacy) → ollama-cloud, base_url = None
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
    fn select_provider_ollama_local_is_ready_without_key() {
        let config = zoid_core::config::Config {
            provider: "ollama-local".to_string(),
            ..Default::default()
        };
        let (_provider, name, has_key) = select_provider(&config, &None);
        assert_eq!(name, "ollama");
        assert!(has_key, "ollama-local must be usable (ready) with no key");
    }
}
