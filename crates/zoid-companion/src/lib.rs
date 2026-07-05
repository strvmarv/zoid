//! Optional localhost companion server for a running zoid session: a live
//! metrics dashboard plus a single agent-pushed HTML card, streamed over SSE.
//! Runs entirely on std threads — no tokio.

pub mod hub;
pub mod snapshot;

pub use hub::{CompanionHub, Frame};
pub use snapshot::{DashboardSnapshot, TierRow};
