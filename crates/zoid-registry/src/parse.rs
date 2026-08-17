//! TOML → Registry (filled in Task 4).
use anyhow::Result;
use zoid_model::{Registry, RegistryPatch};

pub fn parse_shipped(_text: &str) -> Result<Registry> {
    Ok(Registry::default())
}

pub fn parse_user(_text: &str) -> Result<RegistryPatch> {
    Ok(RegistryPatch::default())
}