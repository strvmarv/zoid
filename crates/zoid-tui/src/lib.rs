//! zoid-tui — design tokens and ratatui render functions. Every view renders
//! from the `tokens` module (spec §16).

pub mod tokens;
pub mod chat;
pub mod state;
pub mod layout;
pub mod command;
pub mod palette;
pub mod route;
pub mod render;
pub mod economy_view;

pub use render::render_shell;
pub use state::{DrawerId, Focus, Mode, Overlay, ShellState};
pub use economy_view::EconomyView;
