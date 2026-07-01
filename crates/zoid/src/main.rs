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
#[allow(unused_imports)]
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
use zoid_provider::{default_model, default_provider};
use zoid_provider::Provider;
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

/// Wall-clock millis since the epoch — supplied by the binary (core stays clock-free).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build the message input with the tui-textarea cursor-line **underline**
/// disabled (spec §2.2/§9): the default underline clutters the calm box.
fn make_input(textarea: TextArea<'static>) -> TextArea<'static> {
    let mut textarea = textarea;
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea
}

/// Best-effort current branch from `.git/HEAD` (`ref: refs/heads/<name>`); "main" otherwise.
fn current_branch() -> String {
    std::fs::read_to_string(".git/HEAD")
        .ok()
        .and_then(|s| s.trim().strip_prefix("ref: refs/heads/").map(|b| b.to_string()))
        .unwrap_or_else(|| "main".into())
}

/// Up to N entries of the cwd for the Files drawer (names only, sorted).
fn cwd_files(limit: usize) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(".")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names.truncate(limit);
    names
}

struct App {
    session: SessionHandle,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    model: String,
    textarea: TextArea<'static>,
    streaming: bool,
    shell: zoid_tui::ShellState,
    ui_tx: mpsc::Sender<AgentUpdate>,
    /// Monotonic clock start for motion timing (Ⓡ2).
    started: std::time::Instant,
    /// When the altitude last changed, for the fold/unfold reveal (Ⓡ2).
    zoom_changed_at: Option<std::time::Instant>,
    /// Local UTC offset (seconds) for message-row HH:MM stamps, sampled once.
    tz_offset_secs: i32,
}

impl App {
    /// Append an event both durably (session actor) and to the in-memory log
    /// the UI renders from.
    async fn record(&mut self, kind: EventKind) -> Result<()> {
        let ev = Event::new(Ulid::new(), None, now_ms(), kind);
        self.session.append(ev.clone()).await?;
        self.events.push(ev);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = db_path()?;
    let session = SessionHandle::spawn(path.to_str().context("session DB path is not valid UTF-8")?)?;
    let events = session.snapshot().await?;

    let model = std::env::var("ZOID_MODEL").unwrap_or_else(|_| default_model().to_string());

    let mut shell = zoid_tui::ShellState::new();
    shell.branch = current_branch();
    shell.files = cwd_files(64);
    shell.reduced_motion = std::env::var("ZOID_REDUCED_MOTION").map(|v| !v.is_empty()).unwrap_or(false);

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    let mut app = App {
        session,
        events,
        provider: default_provider(),
        tools: Arc::new(zoid_tools::registry()),
        model,
        textarea: make_input(TextArea::default()),
        streaming: false,
        shell,
        ui_tx,
        started: std::time::Instant::now(),
        zoom_changed_at: None,
        tz_offset_secs: chrono::Local::now().offset().local_minus_utc(),
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
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &mut app, &mut ui_rx).await;

    // Restore the terminal on every exit path — drive through errors, don't bail.
    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
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
        app.shell.input_rows = app.textarea.lines().len().max(1) as u16;
        terminal.draw(|f| {
            let msgs = conversation(&app.events);
            let window = zoid_core::context::context_window(&app.events);
            let churn = zoid_core::economy::churn_timeline(&app.events);
            let ledger = zoid_core::economy::token_ledger(&app.events);
            // T10 (manual control: shell.policy / shell.economy_selected) is DEFERRED post-P3.
            // Until then the policy is the default (auto-evict-cold ON, no ceiling) and there is
            // no row selection — the drawer is read-only/observability-only.
            let policy = zoid_core::assembler::ContextPolicy::default();
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
                            &ChatView { zoom: app.shell.zoom, caret_on: caret, reveal: None, tz_offset_secs: app.tz_offset_secs },
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
            let view = ChatView { zoom: app.shell.zoom, caret_on: caret, reveal, tz_offset_secs: app.tz_offset_secs };
            render_shell(f, &app.shell, &economy, &msgs, &app.textarea, app.streaming, &view);
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
                    AgentUpdate::Appended(ev) => { app.events.push(ev); }
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
        Action::CmdlineBackspace => { app.shell.cmdline.buffer.pop(); }
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
        Action::Edit(key) => { app.textarea.input(key); }
        Action::Submit => {
            if app.streaming { return Ok(false); }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() { return Ok(false); }
            app.textarea = make_input(TextArea::default());
            app.shell.status_hint = None;
            app.record(EventKind::UserMessage { text }).await?;
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
            app.shell.objects.obj_selected = zoid_tui::palette::nav(app.shell.objects.obj_selected, d, n);
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
            let n = objs.get(sel).map(|o| zoid_tui::objects::verbs_for(o.kind).len()).unwrap_or(0);
            app.shell.objects.verb_selected = zoid_tui::palette::nav(app.shell.objects.verb_selected, d, n);
        }
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let prompt = zoid_tui::objects::verb_prompt(verb, obj);
                    // Queue (P4d): seed the input; P5 will dispatch it to a subagent.
                    app.textarea = make_input(TextArea::from(prompt.lines().map(String::from).collect::<Vec<_>>()));
                    app.shell.status_hint = Some("queued · runs as a subagent in P5".into());
                    app.shell.focus = zoid_tui::Focus::Input;
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
        Command::SwitchMode(m) => { app.shell.set_mode(m); Ok(false) }
        Command::OpenDrawer(id) => { app.shell.open_drawer(id); Ok(false) }
        Command::Unknown(_) => Ok(false),
    }
}

fn spawn_turn(app: &App) {
    let provider = app.provider.clone();
    let tools = app.tools.clone();
    let session = app.session.clone();
    let seed = app.events.clone();
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        let _ = run_agent_turn(provider, tools, session, seed, model, ui, now_ms).await;
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
        assert!(has_underline(&default), "default TextArea underlines the cursor line");
        // make_input turns it off.
        let plain = make_input(TextArea::from(vec!["hello".to_string()]));
        assert!(!has_underline(&plain), "make_input must disable the cursor-line underline");
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
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
}
