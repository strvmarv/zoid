//! zoid library surface: the terminal-free agent loop, reused by the binary
//! and exercised by integration tests against a fake provider (spec §13).

pub mod agent;
pub mod agent_import;
pub mod catalog;
pub mod cli;
pub mod eventlog;
pub mod expiry;
pub mod github_fetch;
pub mod invoke_skill;
pub mod mode_import;
pub mod mode_wizard;
pub mod plugin_install;
pub mod skill_import;
pub mod spawn_subagent;
pub mod startup;
pub mod subagent;
pub mod uninstall;
pub mod update;
pub mod wake_timer;
pub mod worktree;
