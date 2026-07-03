use zoid::update::{asset_name, install_binary, is_newer, parse_sha256sums, verify_sha256};

#[test]
fn newer_when_core_version_increases() {
    assert!(is_newer("0.1.0", "0.1.1"));
    assert!(is_newer("0.1.0", "v0.2.0"));
    assert!(is_newer("v0.1.0", "1.0.0"));
}

#[test]
fn not_newer_when_equal_or_older_or_prerelease() {
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.2.0", "0.1.9"));
    // Same core version with a pre-release suffix is NOT an upgrade.
    assert!(!is_newer("0.1.0", "0.1.0-test"));
}

#[test]
fn asset_name_matches_pinned_archive_formats() {
    assert_eq!(
        asset_name("x86_64-unknown-linux-musl"),
        "zoid-x86_64-unknown-linux-musl.tar.gz"
    );
    assert_eq!(
        asset_name("aarch64-apple-darwin"),
        "zoid-aarch64-apple-darwin.tar.gz"
    );
    assert_eq!(
        asset_name("x86_64-pc-windows-msvc"),
        "zoid-x86_64-pc-windows-msvc.zip"
    );
}

#[test]
fn parse_sums_maps_filename_to_hex() {
    let text = "abc123  zoid-x86_64-unknown-linux-musl.tar.gz\n\
                def456 *zoid-x86_64-pc-windows-msvc.zip\n";
    let map = parse_sha256sums(text);
    assert_eq!(
        map.get("zoid-x86_64-unknown-linux-musl.tar.gz").unwrap(),
        "abc123"
    );
    // Leading '*' (binary mode) on the filename is stripped.
    assert_eq!(
        map.get("zoid-x86_64-pc-windows-msvc.zip").unwrap(),
        "def456"
    );
}

#[test]
fn verify_sha256_accepts_correct_and_rejects_tampered() {
    // echo -n "hello" | sha256sum
    let digest = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    assert!(verify_sha256(b"hello", digest).is_ok());
    assert!(verify_sha256(b"hello!", digest).is_err()); // tampered payload
}

#[cfg(unix)]
#[test]
fn extract_finds_zoid_binary_in_targz() {
    // Build a .tar.gz containing `<subdir>/zoid` in memory, then extract it.
    let mut gz = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let data: &[u8] = b"fake-zoid-binary";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "zoid-x86_64-unknown-linux-musl/zoid", data)
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    let bin = zoid::update::extract_binary(&gz).unwrap();
    assert_eq!(bin, b"fake-zoid-binary");
}

#[cfg(unix)]
#[test]
fn install_swaps_binary_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("zoid");
    std::fs::write(&target, b"old-binary").unwrap();
    install_binary(&target, b"new-binary").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new-binary");
    assert_eq!(
        std::fs::read(target.with_extension("bak")).unwrap(),
        b"old-binary"
    );
}
