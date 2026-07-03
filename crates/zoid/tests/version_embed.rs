//! Verifies the workspace version bump and the build.rs target embed.

#[test]
fn workspace_version_is_0_1_0() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
}

#[test]
fn build_target_is_embedded() {
    // Set by crates/zoid/build.rs via `cargo:rustc-env=ZOID_TARGET`.
    assert!(!env!("ZOID_TARGET").is_empty());
}
