//! Encrypted secret store (spec 2026-07-01-config-screen-design.md §3).
//! Threat model: HYGIENE at rest — defeats casual exposure (cat/grep/git/
//! screen-share/backup), NOT a same-uid local attacker. The app key lives in a
//! separate 0600 file, never in the DB, so a copied DB can't be decrypted.

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use rusqlite::{params, Connection};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStatus {
    Set { from_env: bool },
    NotSet,
}

pub trait SecretStore {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, val: &str) -> Result<()>;
    fn clear(&self, name: &str) -> Result<()>;
    fn status(&self, name: &str) -> SecretStatus;
}

/// Encrypted-DB backend. Env var (same name) wins on read.
pub struct EncryptedDb {
    conn: Connection,
    cipher: XChaCha20Poly1305,
}

impl EncryptedDb {
    /// Open the store at `db_path`, loading (or creating, 0600) the 32-byte app
    /// key at `key_path`. `db_path` may be an existing zoid.db (the `secrets`
    /// table is created by `EventStore::open`, but we also ensure it here so the
    /// store is usable standalone in tests).
    pub fn open(db_path: &str, key_path: &Path) -> Result<Self> {
        let key = load_or_create_key(key_path)?;
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS secrets (
                name TEXT PRIMARY KEY, ciphertext BLOB NOT NULL,
                nonce BLOB NOT NULL, created_ts INTEGER NOT NULL);",
        )?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        Ok(Self { conn, cipher })
    }

    fn stored(&self, name: &str) -> Option<String> {
        let row: Option<(Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT ciphertext, nonce FROM secrets WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let (ct, nonce) = row?;
        if nonce.len() != 24 {
            return None;
        }
        let pt = self.cipher.decrypt(XNonce::from_slice(&nonce), ct.as_ref()).ok()?;
        String::from_utf8(pt).ok()
    }
}

impl SecretStore for EncryptedDb {
    fn get(&self, name: &str) -> Option<String> {
        // env wins
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        self.stored(name)
    }

    fn set(&self, name: &str, val: &str) -> Result<()> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, val.as_bytes())
            .map_err(|e| anyhow::anyhow!("encrypt failed: {e}"))?;
        self.conn.execute(
            "INSERT INTO secrets (name, ciphertext, nonce, created_ts) VALUES (?1,?2,?3,?4)
             ON CONFLICT(name) DO UPDATE SET ciphertext=?2, nonce=?3, created_ts=?4",
            params![name, ct, nonce.as_slice(), 0i64],
        )?;
        Ok(())
    }

    fn clear(&self, name: &str) -> Result<()> {
        self.conn.execute("DELETE FROM secrets WHERE name = ?1", params![name])?;
        Ok(())
    }

    fn status(&self, name: &str) -> SecretStatus {
        if std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false) {
            return SecretStatus::Set { from_env: true };
        }
        if self.stored(name).is_some() {
            SecretStatus::Set { from_env: false }
        } else {
            SecretStatus::NotSet
        }
    }
}

/// Load the 32-byte app key, or generate + persist it at 0600 on first use.
fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            return Ok(k);
        }
        eprintln!(
            "zoid: secret key at {} is {} bytes (expected 32); regenerating — previously stored secrets will be unreadable",
            path.display(),
            bytes.len()
        );
    }
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts
        .open(path)
        .with_context(|| format!("writing key file {}", path.display()))?;
    f.write_all(&k)
        .with_context(|| format!("writing key file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, String, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db").to_str().unwrap().to_string();
        let key = dir.path().join("secret.key");
        (dir, db, key)
    }

    #[test]
    fn round_trip_encrypts_and_decrypts() {
        let (_d, db, key) = tmp();
        let s = EncryptedDb::open(&db, &key).unwrap();
        s.set("MY_KEY", "sk-abc123").unwrap();
        assert_eq!(s.get("MY_KEY").as_deref(), Some("sk-abc123"));
        assert!(matches!(s.status("MY_KEY"), SecretStatus::Set { from_env: false }));
        s.clear("MY_KEY").unwrap();
        assert_eq!(s.get("MY_KEY"), None);
        assert!(matches!(s.status("MY_KEY"), SecretStatus::NotSet));
    }

    #[test]
    fn key_file_is_0600_and_ciphertext_is_not_plaintext() {
        let (_d, db, key) = tmp();
        let s = EncryptedDb::open(&db, &key).unwrap();
        s.set("K", "secret-value").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let raw: Vec<u8> = s
            .conn
            .query_row("SELECT ciphertext FROM secrets WHERE name='K'", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.windows(6).any(|w| w == b"secret"));
    }

    #[test]
    fn corrupt_nonce_returns_none_not_panic() {
        let (_d, db, key) = tmp();
        let s = EncryptedDb::open(&db, &key).unwrap();
        s.set("K", "v").unwrap();
        // Corrupt the nonce column to a wrong length; must NOT panic.
        s.conn
            .execute("UPDATE secrets SET nonce = ?1 WHERE name = 'K'", params![vec![0u8; 5]])
            .unwrap();
        assert_eq!(s.get("K"), None);
    }
}
