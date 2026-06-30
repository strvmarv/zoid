//! Headless UX preview: render any shell scene to plain text at an arbitrary
//! size and print it to stdout. Lets us (and CI) eyeball the modal frame at
//! widths the snapshot suite doesn't pin — no real terminal required.
//!
//!   cargo run -p zoid-tui --example preview -- [scene] [width] [height]
//!
//! scene ∈ { chat, files, palette, cmdline, build }  (default: chat)
//! width/height default to 140×24 (wide enough to expose gutter bugs).

use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::ChatMsg;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Mode, Overlay, ShellState};

fn seeded() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("what's causing the 500?".into()),
        ChatMsg::Assistant {
            text: "an unwrapped lookup in the handler.".into(),
            tool_calls: vec![],
        },
    ]
}

fn scene(name: &str) -> (ShellState, Vec<ChatMsg>) {
    let mut s = ShellState::new();
    s.files = vec!["Cargo.toml".into(), "src".into(), "README.md".into()];
    match name {
        "files" => {
            s.toggle_drawer(DrawerId::Files);
        }
        "palette" => {
            s.overlay = Overlay::Palette;
            s.palette.query = "build".into();
        }
        "cmdline" => {
            s.overlay = Overlay::CommandLine;
            s.cmdline.buffer = "build".into();
        }
        "build" => {
            s.set_mode(Mode::Build);
            return (s, vec![]);
        }
        _ => {} // "chat" / default
    }
    (s, seeded())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("chat");
    let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(140);
    let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(24);

    let (state, msgs) = scene(name);
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &state, &msgs, &input, false))
        .unwrap();

    // A ruler makes column drift obvious at a glance.
    let tens: String = (0..w).map(|c| if c % 10 == 0 { '|' } else { ' ' }).collect();
    println!("scene={name}  size={w}x{h}");
    println!("{tens}");
    print!("{}", terminal.backend());
}
