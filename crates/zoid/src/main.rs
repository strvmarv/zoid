use anyhow::{Context, Result};
use crossterm::{
    event::{Event as CEvent, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{prelude::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tui_textarea::TextArea;
use ulid::Ulid;

use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid::input::{classify, KeyAction};
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::conversation;
use zoid_core::session::SessionHandle;
use zoid_provider::{default_model, default_provider};
use zoid_provider::Provider;
use zoid_tools::Tool;
use zoid_tui::chat::render_chat;

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

struct App {
    session: SessionHandle,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    model: String,
    textarea: TextArea<'static>,
    streaming: bool,
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

    let mut app = App {
        session,
        events,
        provider: default_provider(),
        tools: Arc::new(zoid_tools::registry()),
        model,
        textarea: TextArea::default(),
        streaming: false,
    };

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &mut app).await;

    // Restore the terminal on every exit path — drive through errors, don't bail.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut term_events = EventStream::new();
    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(256);

    loop {
        let msgs = conversation(&app.events);
        terminal.draw(|f| render_chat(f, &msgs, &app.textarea, app.streaming))?;

        tokio::select! {
            maybe_term = term_events.next() => {
                match maybe_term {
                    Some(Ok(CEvent::Key(key))) => {
                        match classify(key) {
                            KeyAction::Quit => return Ok(()),
                            KeyAction::ToggleMode => { /* Build mode arrives in P6 — no-op */ }
                            KeyAction::Newline => { app.textarea.insert_newline(); }
                            KeyAction::Edit => { app.textarea.input(key); }
                            KeyAction::Submit => {
                                if app.streaming { continue; }
                                let text = app.textarea.lines().join("\n");
                                if text.trim().is_empty() { continue; }
                                app.textarea = TextArea::default();
                                app.record(EventKind::UserMessage { text }).await?;
                                app.streaming = true;

                                let provider = app.provider.clone();
                                let tools = app.tools.clone();
                                let session = app.session.clone();
                                let seed = app.events.clone();
                                let model = app.model.clone();
                                let ui = ui_tx.clone();
                                tokio::spawn(async move {
                                    let _ = run_agent_turn(provider, tools, session, seed, model, ui, now_ms).await;
                                });
                            }
                        }
                    }
                    Some(Ok(_)) => { /* resize/mouse/etc: redraw on next loop */ }
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
