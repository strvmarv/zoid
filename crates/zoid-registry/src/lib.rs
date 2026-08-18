//! Loads and merges the provider/model registry from TOML, and hosts the
//! refresh tool's fetch + reconcile logic. `zoid-model` stays dependency-free;
//! this crate owns all TOML/serde parsing and network I/O.

pub mod fetch;
pub mod merge;
pub mod parse;
pub mod raw;
pub mod refresh;

use anyhow::Result;
use std::path::Path;
use zoid_model::Registry;

/// Load the merged registry from the shipped and user TOML files.
/// A missing user file is treated as empty; a malformed user file falls back
/// to the shipped file alone (reported via the returned warning string).
pub fn load(shipped: &Path, user: &Path) -> Result<(Registry, Option<String>)> {
    let shipped_text = std::fs::read_to_string(shipped)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", shipped.display()))?;
    let shipped_reg = parse::parse_shipped(&shipped_text)?;

    let user_text = match std::fs::read_to_string(user) {
        Ok(t) => t,
        Err(_) => return Ok((shipped_reg, None)), // missing user file → shipped alone
    };
    match parse::parse_user(&user_text) {
        Ok(user_patch) => Ok((merge::merge(shipped_reg, user_patch), None)),
        Err(e) => Ok((
            shipped_reg,
            Some(format!(
                "ignoring malformed user registry {}: {e} (hidden/user rows dropped)",
                user.display()
            )),
        )),
    }
}
