//! Verifies the workspace version bump and the build.rs target embed.

#[test]
fn workspace_version_is_semver() {
    // Durable check: the crate version is set and looks like `MAJOR.MINOR.PATCH`
    // (numeric parts). Pinning a literal version broke on every release bump.
    let v = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = v.split('.').collect();
    assert_eq!(parts.len(), 3, "expected MAJOR.MINOR.PATCH, got {v:?}");
    assert!(
        parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "non-numeric version part in {v:?}"
    );
}

#[test]
fn build_target_is_embedded() {
    // Set by crates/zoid/build.rs via `cargo:rustc-env=ZOID_TARGET`.
    assert!(!env!("ZOID_TARGET").is_empty());
}
