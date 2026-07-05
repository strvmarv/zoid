//! Render a shell scene to a faithful colored HTML fragment for the marketing
//! site. Reuses the shared scene fixtures and the feature-gated converter.
//!
//!   cargo run -p zoid-tui --features web-capture --example web_capture -- [scene] [w] [h]

mod scenes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("chat");
    let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(140);
    let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(24);
    let buf = scenes::render_shell_scene(name, w, h);
    print!("{}", zoid_tui::web_capture::buffer_to_html(&buf));
}
