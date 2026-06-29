use anyhow::Result;

mod interop;
mod tui;

#[tokio::main]
async fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "tui".into());
    match mode.as_str() {
        "interop" => interop::run(),
        "tui" => tui::run().await,
        other => {
            eprintln!("unknown mode: {other}  (use `tui` or `interop`)");
            Ok(())
        }
    }
}
