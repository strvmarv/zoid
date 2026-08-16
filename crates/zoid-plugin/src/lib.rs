//! Pure, IO-free plugin schema + planning for zoid (spec:
//! docs/superpowers/specs/2026-07-09-zoid-plugin-support-design.md).

pub mod bundled;
pub mod effect;
pub mod manifest;
pub mod plan;
pub mod provenance;
pub mod resolve;

pub use effect::{classify_config_key, Effect, RiskTier};
