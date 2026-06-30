use anyhow::{Context, Result};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event as CEvent, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
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
use zoid_provider::{default_model, default_provider};
use zoid_provider::Provider;
use zoid_tools::Tool;
use zoid_tui::layout::compute;
use zoid_tui::render_shell;
use zoid_tui::route::{palette_selected_command, route_key, route_mouse};

/// Resolve the session DB path: `$ZOID_DB` if set, else `./.zoid/session.db`.
fn db_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ZOID_DB") {
        return Ok(PathBuf::from(p));
    }
    let dir = Path::new(".zoid");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("session.db"))
}

/// Wall-clock millis since the epoch — supplied by the binary (core stays clock-free).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    let mut app = App {
        session,
        events,
        provider: default_provider(),
        tools: Arc::new(zoid_tools::registry()),
        model,
        textarea: TextArea::default(),
        streaming: false,
        shell,
        ui_tx,
    };

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &mut app, &mut ui_rx).await;

    // Restore the terminal on every exit path — drive through errors, don't bail.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    ui_rx: &mut mpsc::Receiver<AgentUpdate>,
) -> Result<()> {
    let mut term_events = EventStream::new();

    loop {
        terminal.draw(|f| {
            let msgs = conversation(&app.events);
            render_shell(f, &app.shell, &msgs, &app.textarea, app.streaming);
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
        Action::Newline => app.textarea.insert_newline(),
        Action::Edit(key) => { app.textarea.input(key); }
        Action::Submit => {
            if app.streaming { return Ok(false); }
            let text = app.textarea.lines().join("\n");
            if text.trim().is_empty() { return Ok(false); }
            app.textarea = TextArea::default();
            app.record(EventKind::UserMessage { text }).await?;
            app.streaming = true;
            spawn_turn(app);
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
