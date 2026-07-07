//! Optional localhost companion server for a running zoid session: a single
//! agent-pushed HTML card, streamed over SSE. Runs entirely on std threads —
//! no tokio.

pub mod hub;
pub mod server;

pub use hub::{CompanionHub, Frame};
pub use server::{start, CompanionServer, CSP};
