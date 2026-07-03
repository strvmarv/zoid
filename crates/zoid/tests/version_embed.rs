//! Verifies the workspace version bump and the build.rs target embed.

#[test]
fn workspace_version_is_embedded_semver() {
    // `zoid` inherits `version.workspace = true`, so CARGO_PKG_VERSION is the
    // embedded workspace version. Assert it is a real 3-part semver rather than
    // pinning an exact release number (which drifts on every version bump).
    let v = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = v.split('.').collect();
    assert_eq!(parts.len(), 3, "expected MAJOR.MINOR.PATCH, got {v:?}");
    for p in &parts {
        // Strip any pre-release/build suffix on the patch component (e.g. "0-rc1").
        let num = p.split(['-', '+']).next().unwrap_or(p);
        assert!(num.parse::<u64>().is_ok(), "non-numeric version component in {v:?}");
    }
    assert_ne!(v, "0.0.0", "workspace version was never bumped from the default");
}

#[test]
fn build_target_is_embedded() {
    // Set by crates/zoid/build.rs via `cargo:rustc-env=ZOID_TARGET`.
    assert!(!env!("ZOID_TARGET").is_empty());
}
