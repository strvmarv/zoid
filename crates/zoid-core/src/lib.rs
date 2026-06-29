//! zoid-core — the event-sourced spine: an append-only log, a SQLite store,
//! and pure projections over the log.

pub mod event;
pub mod projection;
pub mod session;
pub mod store;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
