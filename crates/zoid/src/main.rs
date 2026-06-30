mod input;

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

use input::{classify, KeyAction};
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{transcript, Role};
use zoid_core::session::SessionHandle;
use zoid_provider::{default_model, default_provider};
use zoid_provider::{CompletionRequest, Message, MsgRole, Provider, ProviderEvent};
use zoid_tui::chat::render_chat;

const SYSTEM_PROMPT: &str = "You are zoid, a terminal coding assistant. Be concise and precise.";

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

    /// Build the provider request from the current transcript.
    fn request(&self) -> CompletionRequest {
        let messages = transcript(&self.events)
            .into_iter()
            .map(|t| Message {
                role: match t.role {
                    Role::User => MsgRole::User,
                    Role::Assistant => MsgRole::Assistant,
                },
                content: t.text,
                tool_calls: Vec::new(),
                tool_name: None,
            })
            .collect();
        CompletionRequest {
            model: self.model.clone(),
            system: Some(SYSTEM_PROMPT.to_string()),
            messages,
            max_tokens: 4096,
            tools: vec![],
        }
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
    // Long-lived delta channel; each provider turn clones the sender.
    let (delta_tx, mut delta_rx) = mpsc::channel::<ProviderEvent>(256);

    loop {
        let turns = transcript(&app.events);
        terminal.draw(|f| render_chat(f, &turns, &app.textarea, app.streaming))?;

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
                                if app.streaming { continue; } // ignore submits mid-stream
                                let text = app.textarea.lines().join("\n");
                                if text.trim().is_empty() { continue; }
                                app.textarea = TextArea::default();
                                app.record(EventKind::UserMessage { text }).await?;
                                app.streaming = true;

                                let req = app.request();
                                let provider = app.provider.clone();
                                let tx = delta_tx.clone();
                                tokio::spawn(async move {
                                    let _ = provider.stream(&req, tx.clone()).await;
                                    // Always terminate the turn: providers that end their stream without an
                                    // explicit Done (e.g. a truncated/timed-out SSE response) must not leave
                                    // the UI stuck in `streaming`. A redundant Done in the normal case is
                                    // harmless (it just sets streaming=false again).
                                    let _ = tx.send(ProviderEvent::Done).await;
                                });
                            }
                        }
                    }
                    Some(Ok(_)) => { /* resize/mouse/etc: redraw on next loop */ }
                    Some(Err(_)) | None => return Ok(()),
                }
            }
            Some(pe) = delta_rx.recv() => {
                match pe {
                    ProviderEvent::TextDelta(s) => {
                        app.record(EventKind::ModelDelta { text: s }).await?;
                    }
                    ProviderEvent::Usage(_) => { /* token ledger lands in P3 */ }
                    ProviderEvent::Error(msg) => {
                        app.record(EventKind::AssistantMessage { text: format!("{} {msg}", zoid_tui::tokens::glyph::WARNING) }).await?;
                        app.streaming = false;
                    }
                    ProviderEvent::Done => { app.streaming = false; }
                    ProviderEvent::ToolCall(_) => { /* agent loop wires this up in P1b */ }
                }
            }
        }
    }
}
