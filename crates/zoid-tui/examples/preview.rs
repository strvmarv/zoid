//! Headless UX preview: render any shell scene to plain text at an arbitrary
//! size and print it to stdout. Lets us (and CI) eyeball the modal frame at
//! widths the snapshot suite doesn't pin — no real terminal required.
//!
//!   cargo run -p zoid-tui --example preview -- [scene] [width] [height]
//!
//! scene ∈ { chat, files, palette, build, economy, syntax, summary, detail, objects, verbs }  (default: chat)
//! width/height default to 140×24 (wide enough to expose gutter bugs).

use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;

mod scenes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("chat");
    let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(140);
    let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(24);

    // scene: "syntax" — Ⓡ3 highlight demonstration (not a shell frame).
    if name == "syntax" {
        let sample = "fn main() {\n    let name = \"zoid\";\n    let n = 42; // answer\n    greet(name, n);\n}\n";
        let lines = zoid_tui::highlight_lines(sample, zoid_syntax::Language::Rust);
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(ratatui::widgets::Paragraph::new(lines), f.area()))
            .unwrap();
        let tens: String = (0..w)
            .map(|c| if c % 10 == 0 { '|' } else { ' ' })
            .collect();
        println!("scene={name}  size={w}x{h}");
        println!("{tens}");
        print!("{}", terminal.backend());
        return;
    }

    let (state, msgs, economy) = scenes::scene(name);
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let view = ChatView {
        zoom: state.zoom,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    };
    terminal
        .draw(|f| {
            let reg = scenes::shipped_registry();
            render_shell(f, &state, &economy, &reg, &msgs, None, &[], &input, false, &view);
        })
        .unwrap();

    // A ruler makes column drift obvious at a glance.
    let tens: String = (0..w)
        .map(|c| if c % 10 == 0 { '|' } else { ' ' })
        .collect();
    println!("scene={name}  size={w}x{h}");
    println!("{tens}");
    print!("{}", terminal.backend());
}
