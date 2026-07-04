//! zoid library surface: the terminal-free agent loop, reused by the binary
//! and exercised by integration tests against a fake provider (spec §13).

pub mod agent;
pub mod cli;
pub mod invoke_skill;
pub mod subagent;
pub mod update;
pub mod worktree;
