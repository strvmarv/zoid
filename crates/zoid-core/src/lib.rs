//! zoid-core — the event-sourced spine: an append-only log, a SQLite store,
//! and pure projections over the log.

pub mod agent_profile;
pub mod assembler;
pub mod config;
pub mod context;
pub mod economy;
pub mod event;
pub mod projection;
pub mod secret;
pub mod session;
pub mod sessions;
pub mod store;
pub mod zoom;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
