//! zoid-tui — design tokens and ratatui render functions. Every view renders
//! from the `tokens` module (spec §16).

pub mod chat;
pub mod command;
pub mod config_view;
pub mod economy_view;
pub mod feedback_view;
pub mod help;
pub mod layout;
pub mod markdown;
pub mod motion;
pub mod objects;
pub mod onboarding;
pub mod overview;
pub mod palette;
pub mod question;
pub mod render;
pub mod route;
pub mod scrollbar;
pub mod state;
pub mod syntax_view;
pub(crate) mod text;
pub mod tokens;

#[cfg(feature = "web-capture")]
pub mod web_capture;

#[cfg(test)]
mod test_reg;

pub use economy_view::EconomyView;
pub use motion::{
    caret_on, ease_out_cubic, reveal_count, spinner_frame, zoom_reveal, Anim, MOTION_FPS,
};
pub use objects::{selectable_objects, Obj, ObjectKind};
pub use render::render_shell;
pub use scrollbar::{line_of_msg, msg_at_line, scrollbar_thumb};
pub use state::{DrawerId, Focus, Overlay, ShellState};
pub use syntax_view::{highlight_lines, syn_color};
