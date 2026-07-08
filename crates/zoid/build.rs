//! Embeds two compile-time facts for the `zoid` binary:
//!   * `ZOID_TARGET`      — the build target triple, so `zoid update` can pick
//!     the matching release asset at runtime (spec §2 component A).
//!   * `ZOID_BUILD_EPOCH` — the build's Unix-epoch seconds, so the build can
//!     refuse to launch once it is >30 days old (build-expiration spec §A).

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=ZOID_TARGET={target}");

    // Prefer a caller-provided SOURCE_DATE_EPOCH (reproducible / CI-pinnable);
    // otherwise stamp the current wall clock at build time.
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before UNIX_EPOCH")
                .as_secs()
        });
    println!("cargo:rustc-env=ZOID_BUILD_EPOCH={epoch}");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}
