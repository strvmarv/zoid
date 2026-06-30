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
pub mod syntax_view;
pub mod motion;
pub mod objects;

pub use render::render_shell;
pub use state::{DrawerId, Focus, Mode, Overlay, ShellState};
pub use economy_view::EconomyView;
pub use syntax_view::{highlight_lines, syn_color};
pub use motion::{caret_on, ease_out_cubic, reveal_count, zoom_reveal, Anim, MOTION_FPS};
pub use objects::{selectable_objects, Obj, ObjectKind};
