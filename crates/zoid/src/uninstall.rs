//! `zoid uninstall` — remove zoid's on-disk footprint.
//!
//! By default this deletes the app's data (sessions DB + secrets, config, and
//! the model cache); with `--purge` it also removes the binary. It is
//! deliberately conservative: it prints exactly what it will remove, requires a
//! typed confirmation, and refuses to delete a directory whose final path
//! component isn't `zoid` (a guard against a misresolved path pointing at
//! something unrelated).

use anyhow::Result;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The concrete locations a zoid install occupies (see `main.rs` resolvers).
pub struct Targets {
    /// `…/zoid` data dir — sessions DB, encryption key, encrypted secrets.
    pub data_dir: PathBuf,
    /// `…/zoid` config dir — `config.toml`.
    pub config_dir: PathBuf,
    /// `…/zoid` cache dir — downloaded model weights.
    pub cache_dir: PathBuf,
    /// The zoid executable itself (only removed with `--purge`).
    pub binary: PathBuf,
}

/// Entry point: run against real stdin/stdout.
pub fn run(targets: Targets, purge: bool) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    run_with_io(targets, purge, &mut stdin.lock(), &mut stdout)
}

/// Testable core: I/O is injected so the confirmation flow can be exercised
/// without a real terminal.
pub fn run_with_io(
    targets: Targets,
    purge: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let data_targets: [(&str, &Path); 3] = [
        ("session & secrets data", &targets.data_dir),
        ("configuration", &targets.config_dir),
        ("model cache", &targets.cache_dir),
    ];

    writeln!(out, "This permanently removes zoid's data:")?;
    for (label, p) in data_targets {
        let note = if p.exists() { "" } else { "  (not present)" };
        writeln!(out, "  - {label}: {}{note}", p.display())?;
    }
    if purge {
        let note = if targets.binary.exists() {
            ""
        } else {
            "  (not present)"
        };
        writeln!(out, "  - binary: {}{note}", targets.binary.display())?;
    }
    writeln!(out)?;
    write!(out, "Type 'uninstall' to confirm: ")?;
    out.flush()?;

    let mut line = String::new();
    input.read_line(&mut line)?;
    if line.trim() != "uninstall" {
        writeln!(out, "Aborted — nothing was removed.")?;
        return Ok(());
    }

    for (label, p) in data_targets {
        match remove_zoid_dir(p) {
            Ok(true) => writeln!(out, "removed {label}")?,
            Ok(false) => {}
            Err(e) => writeln!(out, "warning: could not remove {}: {e}", p.display())?,
        }
    }

    if purge {
        if targets.binary.exists() {
            match std::fs::remove_file(&targets.binary) {
                Ok(()) => writeln!(out, "removed binary")?,
                // A running executable can't delete itself on Windows; degrade
                // to an instruction rather than failing the whole uninstall.
                Err(e) => writeln!(
                    out,
                    "warning: could not remove binary {} ({e}); delete it manually",
                    targets.binary.display()
                )?,
            }
        }
        writeln!(out, "\nzoid uninstalled.")?;
    } else {
        writeln!(
            out,
            "\nzoid data removed. The binary remains at {} — delete it and drop it from PATH to finish, or re-run with --purge.",
            targets.binary.display()
        )?;
    }
    Ok(())
}

/// Remove a directory, but only if its final component is `zoid` (guard against
/// a misresolved path). Returns `Ok(false)` when the path doesn't exist.
fn remove_zoid_dir(p: &Path) -> Result<bool> {
    if !p.exists() {
        return Ok(false);
    }
    if p.file_name().and_then(|n| n.to_str()) != Some("zoid") {
        anyhow::bail!("refusing to remove {} — not a zoid directory", p.display());
    }
    std::fs::remove_dir_all(p)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_targets(root: &Path, binary: PathBuf) -> Targets {
        let d = root.join("data/zoid");
        let c = root.join("config/zoid");
        let k = root.join("cache/zoid");
        for p in [&d, &c, &k] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("marker"), b"x").unwrap();
        }
        Targets {
            data_dir: d,
            config_dir: c,
            cache_dir: k,
            binary,
        }
    }

    #[test]
    fn abort_when_not_confirmed_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let t = mk_targets(tmp.path(), tmp.path().join("zoid"));
        let (data, config, cache) = (
            t.data_dir.clone(),
            t.config_dir.clone(),
            t.cache_dir.clone(),
        );
        let mut out = Vec::new();
        run_with_io(t, false, &mut "no\n".as_bytes(), &mut out).unwrap();
        assert!(data.exists() && config.exists() && cache.exists());
        assert!(String::from_utf8(out).unwrap().contains("Aborted"));
    }

    #[test]
    fn confirmed_removes_data_dirs_but_keeps_binary_without_purge() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("zoid");
        std::fs::write(&bin, b"ELF").unwrap();
        let t = mk_targets(tmp.path(), bin.clone());
        let (data, config, cache) = (
            t.data_dir.clone(),
            t.config_dir.clone(),
            t.cache_dir.clone(),
        );
        let mut out = Vec::new();
        run_with_io(t, false, &mut "uninstall\n".as_bytes(), &mut out).unwrap();
        assert!(!data.exists() && !config.exists() && !cache.exists());
        assert!(bin.exists(), "binary must survive without --purge");
        assert!(String::from_utf8(out).unwrap().contains("binary remains"));
    }

    #[test]
    fn purge_also_removes_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("zoid");
        std::fs::write(&bin, b"ELF").unwrap();
        let t = mk_targets(tmp.path(), bin.clone());
        let mut out = Vec::new();
        run_with_io(t, true, &mut "uninstall\n".as_bytes(), &mut out).unwrap();
        assert!(!bin.exists(), "binary must be gone with --purge");
    }

    #[test]
    fn guard_refuses_non_zoid_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("important");
        std::fs::create_dir_all(&bogus).unwrap();
        let err = remove_zoid_dir(&bogus).unwrap_err();
        assert!(err.to_string().contains("not a zoid directory"));
        assert!(bogus.exists(), "guarded path must be left intact");
    }
}
