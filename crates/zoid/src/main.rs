use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, prelude::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::Path;
use zoid_core::projection::{transcript, Turn};
use zoid_core::store::EventStore;
use zoid_tui::chat::render_chat;

/// Resolve the session DB path: `$ZOID_DB` if set, else `./.zoid/session.db`.
fn db_path() -> String {
    if let Ok(p) = std::env::var("ZOID_DB") {
        return p;
    }
    let dir = Path::new(".zoid");
    let _ = std::fs::create_dir_all(dir);
    dir.join("session.db").to_string_lossy().into_owned()
}

fn main() -> Result<()> {
    // Boot: open the log, replay it into the current transcript.
    let store = EventStore::open(&db_path())?;
    let turns = transcript(&store.load_all()?);

    // Enter the TUI.
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &turns);

    // Restore the terminal regardless of how `run` ended.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run<B: Backend>(terminal: &mut Terminal<B>, turns: &[Turn]) -> Result<()> {
    loop {
        terminal.draw(|f| render_chat(f, turns))?;
        if let CEvent::Key(key) = event::read()? {
            let quit = key.code == KeyCode::Char('q')
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
            if quit {
                return Ok(());
            }
        }
    }
}
