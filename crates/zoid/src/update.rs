//! `zoid update`: anonymous, checksum-verified self-replace against the public
//! releases repo (spec §2 component B). Pure core (version compare, asset
//! selection, checksum, sums parsing) + a thin network/filesystem shell.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Public distribution repo that holds the GitHub Releases. Source stays private.
const RELEASES_REPO: &str = "strvmarv/zoid";

/// cargo-dist publishes its unified checksums file under this name (coreutils
/// `<hex>  <filename>` format). NOT `SHA256SUMS`.
const CHECKSUMS_ASSET: &str = "sha256.sum";

/// The build target triple, embedded by `build.rs`.
pub fn build_target() -> &'static str {
    env!("ZOID_TARGET")
}

/// Parse "v0.1.0" / "0.1.0" / "0.1.0-test" into a (major, minor, patch) core
/// triple, ignoring a leading 'v' and any `-prerelease` suffix.
fn parse_core(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let core = v.split('-').next().unwrap_or(v);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is a strictly newer release than `current`. A pre-release
/// sharing the same core version is not considered newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_core(current), parse_core(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// The published asset filename for a build target triple. Archive formats are
/// pinned in cargo-dist config (unix `.tar.gz`, windows `.zip`).
pub fn asset_name(target: &str) -> String {
    if target.contains("windows") {
        format!("zoid-{target}.zip")
    } else {
        format!("zoid-{target}.tar.gz")
    }
}

/// Parse a coreutils-format checksums file ("<hex>  <filename>") into a map of
/// filename → lowercase hex digest. A leading '*' on the filename is stripped.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(hex), Some(name)) = (parts.next(), parts.next()) {
            map.insert(name.trim_start_matches('*').to_string(), hex.to_lowercase());
        }
    }
    map
}

/// Verify `bytes` hashes to `expected_hex` (SHA-256). Error on mismatch.
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let got = Sha256::digest(bytes);
    let got_hex = got.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if got_hex == expected_hex.to_lowercase() {
        Ok(())
    } else {
        bail!("checksum verification failed (expected {expected_hex}, got {got_hex})")
    }
}

/// Extract the `zoid` binary bytes from a downloaded `.tar.gz` archive (unix).
#[cfg(unix)]
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.file_name().and_then(|s| s.to_str()) == Some("zoid") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("no `zoid` binary found in release archive")
}

/// Extract the `zoid.exe` binary bytes from a downloaded `.zip` archive (windows).
#[cfg(windows)]
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let base = file.name().rsplit(['/', '\\']).next().unwrap_or("");
        if base == "zoid.exe" {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("no `zoid.exe` binary found in release archive")
}

/// Atomically replace the binary at `target` with `new_bin`, keeping `<target>.bak`.
#[cfg(unix)]
pub fn install_binary(target: &Path, new_bin: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("target has no parent dir"))?;
    let tmp = dir.join(".zoid-update.tmp");
    std::fs::write(&tmp, new_bin).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    let bak = target.with_extension("bak");
    if target.exists() {
        std::fs::rename(target, &bak)
            .with_context(|| format!("backing up {}", target.display()))?;
    }
    std::fs::rename(&tmp, target).with_context(|| format!("installing {}", target.display()))?;
    Ok(())
}

/// Windows variant: a running `.exe` cannot be overwritten in place, but it can
/// be renamed out of the way first.
#[cfg(windows)]
pub fn install_binary(target: &Path, new_bin: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("target has no parent dir"))?;
    let tmp = dir.join("zoid-update.tmp.exe");
    std::fs::write(&tmp, new_bin).with_context(|| format!("writing {}", tmp.display()))?;
    let bak = target.with_extension("bak");
    if bak.exists() {
        let _ = std::fs::remove_file(&bak);
    }
    if target.exists() {
        std::fs::rename(target, &bak)
            .with_context(|| format!("backing up {}", target.display()))?;
    }
    std::fs::rename(&tmp, target).with_context(|| format!("installing {}", target.display()))?;
    Ok(())
}

/// Entry point for `zoid update`.
pub async fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = build_target();
    let exe = std::env::current_exe().context("resolving current executable path")?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("zoid-updater/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest");
    let rel: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .context("could not reach releases repo")?
        .error_for_status()
        .context("releases API returned an error")?
        .json()
        .await?;

    let latest = rel["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow!("release has no tag_name"))?;
    if !is_newer(current, latest) {
        println!("zoid is already up to date (v{current})");
        return Ok(());
    }

    let want = asset_name(target);
    let assets = rel["assets"]
        .as_array()
        .ok_or_else(|| anyhow!("release has no assets"))?;
    let find = |name: &str| -> Option<String> {
        assets.iter().find_map(|a| {
            if a["name"].as_str() == Some(name) {
                a["browser_download_url"].as_str().map(String::from)
            } else {
                None
            }
        })
    };
    let asset_url = find(&want).ok_or_else(|| anyhow!("no release asset for {target}"))?;
    let sums_url = find(CHECKSUMS_ASSET)
        .ok_or_else(|| anyhow!("release has no {CHECKSUMS_ASSET} checksums file"))?;

    println!("updating zoid {current} -> {latest} ({want})...");
    let archive = client
        .get(&asset_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let sums = client
        .get(&sums_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let expected = parse_sha256sums(&sums)
        .get(&want)
        .cloned()
        .ok_or_else(|| anyhow!("{want} missing from {CHECKSUMS_ASSET}"))?;
    verify_sha256(&archive, &expected)
        .context("aborting: refusing to install an unverified binary")?;

    let bin = extract_binary(&archive)?;
    install_binary(&exe, &bin).with_context(|| format!("cannot replace {}", exe.display()))?;

    println!(
        "zoid updated to {latest} (previous binary kept as {}.bak)",
        exe.display()
    );
    Ok(())
}
