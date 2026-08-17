//! Test-only support: build a `Registry` from the shipped `models.toml`.
//!
//! The `zoid-tui` library itself never parses TOML — `provider_options`,
//! `model_options`, and `build_sections` only take a `&Registry`. Tests,
//! however, need a realistic registry to exercise those functions, so this
//! module (compiled only under `#[cfg(test)]`) parses the shipped TOML via
//! `zoid-registry` (a dev-dependency) and hands back a `Registry`.

#![cfg(test)]

use zoid_model::Registry;

/// The shipped registry parsed from `crates/zoid-model/models.toml`.
/// Panics on parse failure (a corrupted build-time asset — a test-only path).
pub fn shipped() -> Registry {
    zoid_registry::parse::parse_shipped(include_str!("../../zoid-model/models.toml"))
        .expect("shipped models.toml must parse")
}