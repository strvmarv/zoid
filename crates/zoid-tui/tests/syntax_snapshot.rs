use ratatui::{backend::TestBackend, widgets::Paragraph, Terminal};
use zoid_syntax::Language;
use zoid_tui::highlight_lines;

const SAMPLE: &str = "\
fn main() {
    let name = \"zoid\";
    let n = 42; // answer
    greet(name, n);
}
";

fn draw(w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let lines = highlight_lines(SAMPLE, Language::Rust);
    terminal
        .draw(|f| f.render_widget(Paragraph::new(lines), f.area()))
        .unwrap();
    format!("{:#?}", terminal.backend().buffer())
}

#[test]
fn syntax_highlight_frame() {
    insta::assert_snapshot!(draw(100, 24));
}

#[test]
fn syntax_highlight_wide_frame() {
    insta::assert_snapshot!(draw(140, 24));
}
