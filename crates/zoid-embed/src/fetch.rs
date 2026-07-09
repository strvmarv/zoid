//! Weight fetcher: download model files from pinned URLs via in-tree reqwest
//! (rustls), sha256-verify against pinned hashes, cache under `cache_dir`. NO
//! hf-hub dep (its native-tls drags OpenSSL, which fails musl — Phase-0).

use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct WeightPaths {
    pub config: PathBuf,
    pub tokenizer: PathBuf,
    pub weights: PathBuf,
}

/// Download-progress sink: `(label, bytes_downloaded, total_bytes)`. `total` is
/// `None` when the server sends no `Content-Length`. Called repeatedly (throttled)
/// during a download and once more at completion.
pub type ProgressFn<'a> = &'a mut dyn FnMut(&str, u64, Option<u64>);

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn verify_file(path: &Path, want_sha256: &str) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let got = sha256_hex(&bytes);
    if got != want_sha256 {
        bail!(
            "sha256 mismatch for {}: got {got}, want {want_sha256}",
            path.display()
        );
    }
    Ok(())
}

// Pinned artifacts for bge-small-en-v1.5. URLs resolve to the HF CDN; hashes
// pin exact bytes (verified against the pinned URLs; not downloaded by this
// crate's fast tests).
struct Artifact {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}
const ARTIFACTS: &[Artifact] = &[
    Artifact {
        file: "config.json",
        url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/config.json",
        sha256: "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750",
    },
    Artifact {
        file: "tokenizer.json",
        url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json",
        sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    },
    Artifact {
        file: "model.safetensors",
        url: "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/model.safetensors",
        sha256: "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad",
    },
];

pub fn ensure_weights(cache_dir: &Path, auto_download: bool) -> Result<WeightPaths> {
    ensure_weights_with_progress(cache_dir, auto_download, &mut |_, _, _| {})
}

/// Like [`ensure_weights`], but reports byte-level download progress through
/// `progress` (see [`ProgressFn`]). Downloads stream to a `.part` file, verify
/// their sha256 as bytes arrive, and only then atomically rename into place —
/// so an interrupted or corrupt download never leaves a bad cache file, and the
/// full artifact is never buffered in memory.
pub fn ensure_weights_with_progress(
    cache_dir: &Path,
    auto_download: bool,
    progress: ProgressFn<'_>,
) -> Result<WeightPaths> {
    std::fs::create_dir_all(cache_dir)?;
    let mut paths = Vec::new();
    for a in ARTIFACTS {
        let dest = cache_dir.join(a.file);
        if dest.exists() {
            // Cached artifact: verify integrity (a freshly downloaded one is
            // already verified in-stream below, so we never double-read it).
            verify_file(&dest, a.sha256)?;
        } else {
            if !auto_download {
                bail!("weight {} missing and auto_download=false", a.file);
            }
            let mut resp = reqwest::blocking::get(a.url)?.error_for_status()?;
            let total = resp.content_length();
            stream_verify(&mut resp, &dest, total, a.sha256, a.file, &mut *progress)?;
        }
        paths.push(dest);
    }
    Ok(WeightPaths {
        config: paths[0].clone(),
        tokenizer: paths[1].clone(),
        weights: paths[2].clone(),
    })
}

/// Stream `reader` into `dest`, hashing as we go, reporting progress, and
/// atomically renaming from a `.part` sidecar only after the sha256 matches.
/// Network-free (takes any `Read`) so it is unit-testable without a server.
fn stream_verify<R: Read>(
    mut reader: R,
    dest: &Path,
    total: Option<u64>,
    want_sha256: &str,
    label: &str,
    progress: ProgressFn<'_>,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last = std::time::Instant::now();
    progress(label, 0, total);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        // Throttle callbacks so a fast link doesn't spam the terminal.
        if last.elapsed() >= std::time::Duration::from_millis(200) {
            progress(label, downloaded, total);
            last = std::time::Instant::now();
        }
    }
    f.flush()?;
    progress(label, downloaded, total);
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != want_sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("sha256 mismatch for {label}: got {got}, want {want_sha256}");
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
    #[test]
    fn verify_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, b"corrupt").unwrap();
        assert!(verify_file(
            &p,
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
    }
    #[test]
    fn ensure_weights_refuses_download_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        // empty dir → first artifact missing → with auto_download=false must error,
        // never hit the network (offline / "use only if present" contract).
        let res = ensure_weights(dir.path(), false);
        assert!(
            res.is_err(),
            "auto_download=false with missing weights must refuse, not download"
        );
    }
    // Note: the cached-hit success path (auto_download=true or false with all
    // three artifacts present) is intentionally not tested here — it requires
    // verifying model.safetensors (133MB), which cannot be fixtured. That path
    // is covered by the #[ignore] smoke test instead.

    #[test]
    fn stream_verify_writes_and_reports_progress_on_match() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("blob.bin");
        let data = vec![7u8; 300_000];
        let want = sha256_hex(&data);
        let mut calls: Vec<(u64, Option<u64>)> = Vec::new();
        let mut progress = |_label: &str, done: u64, total: Option<u64>| calls.push((done, total));
        stream_verify(
            std::io::Cursor::new(data.clone()),
            &dest,
            Some(data.len() as u64),
            &want,
            "blob.bin",
            &mut progress,
        )
        .unwrap();
        // File landed with exact bytes; no .part sidecar left behind.
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!dest.with_extension("part").exists());
        // Progress was reported and the final call equals the full size.
        assert!(!calls.is_empty());
        assert_eq!(calls.last().unwrap().0, data.len() as u64);
    }

    #[test]
    fn stream_verify_rejects_mismatch_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("blob.bin");
        let res = stream_verify(
            std::io::Cursor::new(b"actual bytes".to_vec()),
            &dest,
            None,
            "0000000000000000000000000000000000000000000000000000000000000000",
            "blob.bin",
            &mut |_, _, _| {},
        );
        assert!(res.is_err(), "sha mismatch must error");
        // Neither the final file nor the .part sidecar survives a bad download.
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
    }
}
