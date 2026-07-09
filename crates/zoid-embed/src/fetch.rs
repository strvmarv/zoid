//! Weight fetcher: download model files from pinned URLs via in-tree reqwest
//! (rustls), sha256-verify against pinned hashes, cache under `cache_dir`. NO
//! hf-hub dep (its native-tls drags OpenSSL, which fails musl — Phase-0).

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub struct WeightPaths {
    pub config: PathBuf,
    pub tokenizer: PathBuf,
    pub weights: PathBuf,
}

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
    std::fs::create_dir_all(cache_dir)?;
    let mut paths = Vec::new();
    for a in ARTIFACTS {
        let dest = cache_dir.join(a.file);
        if !dest.exists() {
            if !auto_download {
                bail!("weight {} missing and auto_download=false", a.file);
            }
            let bytes = reqwest::blocking::get(a.url)?.error_for_status()?.bytes()?;
            std::fs::write(&dest, &bytes)?;
        }
        verify_file(&dest, a.sha256)?;
        paths.push(dest);
    }
    Ok(WeightPaths {
        config: paths[0].clone(),
        tokenizer: paths[1].clone(),
        weights: paths[2].clone(),
    })
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
}
