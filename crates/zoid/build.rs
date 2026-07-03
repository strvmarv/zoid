//! Embeds the build target triple so `zoid update` can select the matching
//! release asset at runtime (spec §2 component A).

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=ZOID_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
