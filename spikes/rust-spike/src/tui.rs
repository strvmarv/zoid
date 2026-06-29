//! R1 — TUI render floor + ceiling (Rust side, ratatui + crossterm + tokio).
//! Exercises: semantic-zoom toggle (ceiling), drawer + Tab focus + mouse-select
//! (framework floor), live async streaming redraw (concurrency).

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use tokio::sync::mpsc;

struct Turn {
    role: &'static str,
    summary: String,
    lines: Vec<String>,
}

#[derive(PartialEq)]
enum Focus {
    Transcript,
    Drawer,
}

struct App {
    turns: Vec<Turn>,
    zoomed_out: bool,
    drawer_open: bool,
    focus: Focus,
    selected: Option<usize>, // index into the *flat* rendered line list of the transcript
    transcript_area: Rect,   // last drawn inner area, for mouse hit-testing
    streaming: bool,
}

impl App {
    fn new() -> Self {
        let turns = vec![
            Turn {
                role: "user",
                summary: "refactor auth to use JWT".into(),
                lines: vec!["› refactor auth to use JWT instead of the legacy token check".into()],
            },
            Turn {
                role: "zoid",
                summary: "read auth.cs, edited 2 files, tests pass".into(),
                lines: vec![
                    "zoid  read auth.cs, api/tokens.cs (412 lines)".into(),
                    "      ● edited auth.cs        +12 -4".into(),
                    "      ● edited api/tokens.cs  +27 -9".into(),
                    "      ✓ 48 tests passed".into(),
                ],
            },
            Turn {
                role: "user",
                summary: "add rate limiting".into(),
                lines: vec!["› now add rate limiting to the public endpoints".into()],
            },
        ];
        Self {
            turns,
            zoomed_out: false,
            drawer_open: true,
            focus: Focus::Transcript,
            selected: None,
            transcript_area: Rect::default(),
            streaming: false,
        }
    }
}

pub async fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;

    let mut app = App::new();
    let mut events = EventStream::new();
    let (tx, mut rx) = mpsc::channel::<String>(64);

    let res = loop {
        terminal.draw(|f| ui(f, &mut app))?;

        tokio::select! {
            maybe_ev = events.next() => {
                match maybe_ev {
                    Some(Ok(ev)) => {
                        if handle_event(&mut app, ev, &tx) { break Ok(()); }
                    }
                    Some(Err(e)) => break Err(anyhow::anyhow!(e)),
                    None => break Ok(()),
                }
            }
            Some(tok) = rx.recv() => {
                push_token(&mut app, &tok);
            }
        }
    };

    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    res
}

/// Returns true if the app should quit.
fn handle_event(app: &mut App, ev: Event, tx: &mpsc::Sender<String>) -> bool {
    match ev {
        Event::Key(k) => match k.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('z') => app.zoomed_out = !app.zoomed_out,
            KeyCode::Char('d') => app.drawer_open = !app.drawer_open,
            KeyCode::Tab => {
                app.focus = if app.focus == Focus::Transcript {
                    Focus::Drawer
                } else {
                    Focus::Transcript
                };
            }
            KeyCode::Char('s') => start_stream(app, tx),
            _ => {}
        },
        Event::Mouse(m) => {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                // hit-test the click against the transcript inner area
                let a = app.transcript_area;
                if m.column >= a.x
                    && m.column < a.x + a.width
                    && m.row >= a.y
                    && m.row < a.y + a.height
                {
                    app.focus = Focus::Transcript;
                    app.selected = Some((m.row - a.y) as usize);
                }
            }
        }
        _ => {}
    }
    false
}

fn start_stream(app: &mut App, tx: &mpsc::Sender<String>) {
    if app.streaming {
        return;
    }
    app.streaming = true;
    app.turns.push(Turn {
        role: "zoid",
        summary: "(streaming…)".into(),
        lines: vec!["zoid  ".into()],
    });
    let tx = tx.clone();
    tokio::spawn(async move {
        for word in "I'll add a token-bucket limiter per IP, return 429 with Retry-After, and cover it with tests .".split(' ') {
            tokio::time::sleep(std::time::Duration::from_millis(90)).await;
            if tx.send(format!("{word} ")).await.is_err() {
                break;
            }
        }
    });
}

fn push_token(app: &mut App, tok: &str) {
    if tok == ". " {
        app.streaming = false;
    }
    if let Some(last) = app.turns.last_mut() {
        if let Some(line) = last.lines.last_mut() {
            line.push_str(tok);
        }
        last.summary = "streamed a plan".into();
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let vert = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(1),    // body
        Constraint::Length(1), // status
    ])
    .split(f.area());

    // title bar
    let zoom = if app.zoomed_out { "overview" } else { "detail" };
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::styled("  CHAT", Style::default().fg(Color::Cyan)),
        Span::raw(format!("   altitude:{zoom}   opus-4.8   152k/200k")),
    ]);
    f.render_widget(Paragraph::new(title), vert[0]);

    // body: transcript (+ drawer)
    let body = if app.drawer_open {
        Layout::horizontal([Constraint::Min(20), Constraint::Length(34)]).split(vert[1])
    } else {
        Layout::horizontal([Constraint::Min(20)]).split(vert[1])
    };

    render_transcript(f, app, body[0]);
    if app.drawer_open {
        render_drawer(f, app, body[1]);
    }

    // status
    let status = Line::from(Span::styled(
        " q quit · z zoom · d drawer · Tab focus · s stream · click=select ",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(status), vert[2]);
}

fn render_transcript(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Transcript;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" transcript ")
        .border_style(border_style(focused));
    let inner = block.inner(area);
    app.transcript_area = inner;
    f.render_widget(block, area);

    // Build the flat line list (semantic zoom = which projection we render).
    let mut lines: Vec<Line> = Vec::new();
    for turn in &app.turns {
        if app.zoomed_out {
            lines.push(summary_line(turn));
        } else {
            for (i, l) in turn.lines.iter().enumerate() {
                let style = if turn.role == "user" {
                    Style::default().fg(Color::Cyan)
                } else if i == 0 {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(l.clone(), style)));
            }
        }
    }

    // selection highlight (object-first floor)
    if let Some(sel) = app.selected {
        if let Some(line) = lines.get_mut(sel) {
            *line = line.clone().style(
                Style::default()
                    .bg(Color::Rgb(22, 51, 92))
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn summary_line(turn: &Turn) -> Line {
    let glyph = if turn.role == "user" { "›" } else { "•" };
    Line::from(vec![
        Span::styled(format!("{glyph} "), Style::default().fg(Color::Cyan)),
        Span::raw(turn.summary.clone()),
    ])
}

fn render_drawer(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Drawer;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" ⑤ context ")
        .border_style(border_style(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        Line::from("● auth.cs       18k ███"),
        Line::from("  tokens.cs     12k ██░"),
        Line::from(Span::styled(
            "  docs/auth.md   6k cold",
            Style::default().fg(Color::Red),
        )),
        Line::from("  system+tools  22k lock"),
        Line::from(""),
        Line::from(Span::styled(
            "[x] evict cold → -6k",
            Style::default().fg(Color::Cyan),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
