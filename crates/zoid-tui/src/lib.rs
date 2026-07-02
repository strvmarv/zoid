//! zoid-tui — design tokens and ratatui render functions. Every view renders
//! from the `tokens` module (spec §16).

pub mod chat;
pub mod command;
pub mod economy_view;
pub mod layout;
pub mod markdown;
pub mod motion;
pub mod objects;
pub mod palette;
pub mod render;
pub mod route;
pub mod state;
pub mod syntax_view;
pub(crate) mod text;
pub mod tokens;

pub use economy_view::EconomyView;
pub use motion::{caret_on, ease_out_cubic, reveal_count, zoom_reveal, Anim, MOTION_FPS};
pub use objects::{selectable_objects, Obj, ObjectKind};
pub use render::render_shell;
pub use state::{DrawerId, Focus, Mode, Overlay, ShellState};
pub use syntax_view::{highlight_lines, syn_color};
