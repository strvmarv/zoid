//! Render a shell scene to a faithful colored HTML fragment for the marketing
//! site. Reuses the shared scene fixtures and the feature-gated converter.
//!
//!   cargo run -p zoid-tui --features web-capture --example web_capture -- [scene] [w] [h]

mod scenes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Print the number of frames in a scene's sequence.
        Some("--count") => {
            let name = args.get(1).map(String::as_str).unwrap_or("context-economy");
            println!("{}", scenes::scene_seq(name).len());
        }
        // Print one frame of a scene's sequence: --frame <i> <scene> [w] [h]
        Some("--frame") => {
            let i: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(0);
            let name = args.get(2).map(String::as_str).unwrap_or("context-economy");
            let w: u16 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(160);
            let h: u16 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(40);
            let frames = scenes::render_shell_scene_seq(name, w, h);
            let buf = frames
                .get(i)
                .unwrap_or_else(|| panic!("frame {i} out of range (have {})", frames.len()));
            print!("{}", zoid_tui::web_capture::buffer_to_html(buf));
        }
        // Single still (back-compat): <scene> [w] [h]. Default size ≥ MIN.
        _ => {
            let name = args.first().map(String::as_str).unwrap_or("chat");
            let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(160);
            let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(40);
            let buf = scenes::render_shell_scene(name, w, h);
            print!("{}", zoid_tui::web_capture::buffer_to_html(&buf));
        }
    }
}
