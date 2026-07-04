//! zoid-core — the event-sourced spine: an append-only log, a SQLite store,
//! and pure projections over the log.

pub mod agent_profile;
pub mod skill;
pub mod assembler;
pub mod band;
pub mod compaction;
pub mod config;
pub mod context;
pub mod economy;
pub mod event;
pub mod eviction;
pub mod projection;
pub mod retrieval;
pub mod secret;
pub mod session;
pub mod sessions;
pub mod store;
pub mod tasks;
pub mod zoom;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
