// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `FileKeyStore` — file-backed `SecureKeyStore` for development and CI
//! environments where the desktop OS keyring is unavailable.
//!
//! On macOS specifically, the OS keychain ACL is bound to the calling
//! binary's code-signing identity. Each `cargo build` produces a new
//! binary hash → new ad-hoc signature → the keychain treats the
//! rebuilt binary as a different app and demands an interactive consent
//! dialog `SecItemCopyMatching` cannot show from a non-bundled CLI
//! process. The error surfaces as `errSecInteractionNotAllowed` ("User
//! interaction is not allowed") and is unfixable without a real
//! (non-ad-hoc) Developer ID signature.
//!
//! `FileKeyStore` sidesteps the issue: the database key for an id lives
//! at `<base>/<id>.key` (mode 0600, 32 raw bytes). No secure-enclave
//! protection — anyone with read access to the base directory recovers
//! the key. That is acceptable for **development only**; the OS-keyring
//! path is the production target on every desktop platform.
//!
//! Because the store is a plaintext downgrade, every construction
//! announces itself with one `tracing::warn!` naming the base directory
//! (Step 77): a leftover `KUPFERDRAHT_DEV_KEYSTORE` in a CI config or shell
//! profile must never switch a process to plaintext keys silently.
//!
//! Writes are atomic (Step 77): the secret goes to `<base>/<id>.key.tmp`
//! — created fresh with mode 0600 on Unix, never chmod'ed after the fact
//! — is fsynced, and is renamed over the target; the directory is fsynced
//! afterwards. A crash mid-write leaves the previous key file intact
//! rather than a truncated one, which would brick the `SQLite3MC` database
//! it keys. A stale `.tmp` from an earlier crash is discarded unread.
//!
//! Activated by `default_keystore` (`client/builder.rs`) and by
//! `kupferdraht-cli`'s `cli_keystore` when the `KUPFERDRAHT_DEV_KEYSTORE`
//! environment variable is set. The variable's value is the base
//! directory for key files (created with mode 0700 on Unix if missing;
//! an existing directory keeps its mode — it is the operator's).

use std::fs;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;

/// File-backed `SecureKeyStore`. The secret for an id lives at
/// `<base>/<id>.key` as raw bytes.
pub struct FileKeyStore {
    base: PathBuf,
}

impl FileKeyStore {
    /// Construct a `FileKeyStore` rooted at `base`. Does not touch the
    /// filesystem yet — `<base>` is created lazily on the first
    /// `set_secret` (mode 0700 on Unix).
    ///
    /// Emits one `WARN` event: the store keeps database keys in plaintext
    /// files, so its activation must be visible in every log — the
    /// accidental-leftover-env-var case is the threat (Step 77).
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        tracing::warn!(
            base = %base.display(),
            "FileKeyStore activated — development only: database keys are stored in \
             plaintext files under the base directory; production uses the OS keyring"
        );
        Self { base }
    }

    /// The key file of `id` under the base directory. Step 95c: the id
    /// is caller input (`FileKeyStore` is public) — an empty or
    /// path-unsafe one (`../x`) is refused instead of escaping the base.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` for an empty or path-unsafe `id`.
    fn key_path(&self, id: &str) -> Result<PathBuf, KupferdrahtError> {
        if !crate::storage::is_path_safe_id(id) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "FileKeyStore: refusing the path-unsafe keystore id {id:?}"
            )));
        }
        Ok(self.base.join(format!("{id}.key")))
    }
}

/// Create `base` (and missing parents) with mode 0700 on Unix. The mode is
/// set at creation, not chmod'ed after; an already-existing directory keeps
/// whatever mode its operator gave it.
fn create_private_dir(base: &Path) -> Result<(), KupferdrahtError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(base)
        .map_err(|e| KupferdrahtError::Generic(format!("mkdir -p {}: {e}", base.display())))
}

/// Write `secret` to `path` atomically (Step 77): temp file (0600 from
/// creation) → fsync → rename over `path` → directory fsync — the shared
/// primitive in `storage::atomic_file`. A failure leaves `path` as it was.
fn write_key_file(path: &Path, secret: &[u8]) -> Result<(), KupferdrahtError> {
    debug_assert!(
        path.extension().is_some_and(|ext| ext == "key"),
        "key files are named `<id>.key`"
    );
    crate::storage::write_atomically(path, secret, true)
        .map_err(|e| KupferdrahtError::Generic(e.to_string()))
}

impl SecureKeyStore for FileKeyStore {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        let path = self.key_path(&id)?;
        if !path.is_file() {
            return Ok(None);
        }
        let raw = fs::read(&path)
            .map_err(|e| KupferdrahtError::Generic(format!("read {}: {e}", path.display())))?;
        Ok(Some(raw))
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        let secret = Zeroizing::new(secret);
        let path = self.key_path(&id)?;
        create_private_dir(&self.base)?;
        write_key_file(&path, &secret)
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        let path = self.key_path(&id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KupferdrahtError::Generic(format!(
                "FileKeyStore delete {}: {e}",
                path.display()
            ))),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {

    use tempfile::TempDir;

    use super::*;

    const ID: &str = "account-alice";
    const SECRET_A: [u8; 32] = [0xA5; 32];
    const SECRET_B: [u8; 32] = [0x5A; 32];

    /// Step 95c: `FileKeyStore` is public, so the id is caller input. An
    /// empty id used to hit a `debug_assert!`; a path-unsafe one
    /// (`../evil`) escaped the base directory. Both are `InvalidInput`.
    #[test]
    fn empty_or_path_unsafe_ids_are_refused() {
        let dir = TempDir::new().expect("tempdir");
        let store = FileKeyStore::new(dir.path().join("keys"));
        for id in ["", "../evil", "a/b", ".."] {
            assert!(
                matches!(
                    store.get_secret(id.into()),
                    Err(KupferdrahtError::InvalidInput(_))
                ),
                "get_secret({id:?})"
            );
            assert!(
                matches!(
                    store.set_secret(id.into(), SECRET_A.to_vec()),
                    Err(KupferdrahtError::InvalidInput(_))
                ),
                "set_secret({id:?})"
            );
            assert!(
                matches!(
                    store.delete_secret(id.into()),
                    Err(KupferdrahtError::InvalidInput(_))
                ),
                "delete_secret({id:?})"
            );
        }
        assert!(
            !dir.path().join("evil.key").exists(),
            "nothing escaped the base"
        );
    }

    fn key_file(base: &Path) -> PathBuf {
        base.join(format!("{ID}.key"))
    }

    fn temp_file(base: &Path) -> PathBuf {
        base.join(format!("{ID}.key.tmp"))
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn set_secret_twice_reads_back_the_latest_at_mode_0600_without_tmp_residue() {
        let dir = TempDir::new().expect("tempdir");
        let store = FileKeyStore::new(dir.path());
        store
            .set_secret(ID.to_owned(), SECRET_A.to_vec())
            .expect("first set");
        store
            .set_secret(ID.to_owned(), SECRET_B.to_vec())
            .expect("second set");
        let read = store.get_secret(ID.to_owned()).expect("get");
        assert_eq!(read.as_deref(), Some(&SECRET_B[..]), "latest secret wins");
        assert_eq!(mode_of(&key_file(dir.path())), 0o600, "key file mode");
        assert!(
            !temp_file(dir.path()).exists(),
            "no `.tmp` residue after a successful write"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_tmp_from_a_crashed_write_is_replaced_not_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new().expect("tempdir");
        // Simulate a crash mid-write of an earlier process: a world-readable
        // temp file with garbage that must neither be read nor inherited.
        let stale = temp_file(dir.path());
        fs::write(&stale, b"garbage-from-a-crashed-write").expect("write stale tmp");
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).expect("chmod stale");
        let store = FileKeyStore::new(dir.path());
        store
            .set_secret(ID.to_owned(), SECRET_A.to_vec())
            .expect("set over stale tmp");
        let read = store.get_secret(ID.to_owned()).expect("get");
        assert_eq!(
            read.as_deref(),
            Some(&SECRET_A[..]),
            "fresh secret, not garbage"
        );
        assert!(!stale.exists(), "stale `.tmp` is gone after the write");
        assert_eq!(
            mode_of(&key_file(dir.path())),
            0o600,
            "0600 from creation, not inherited"
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_base_dir_is_created_with_mode_0700() {
        let root = TempDir::new().expect("tempdir");
        let base = root.path().join("nested").join("keystore");
        let store = FileKeyStore::new(&base);
        store
            .set_secret(ID.to_owned(), SECRET_A.to_vec())
            .expect("set creates the base dir");
        assert_eq!(
            mode_of(&base),
            0o700,
            "base dir mode as the module doc promises"
        );
        assert_eq!(mode_of(&key_file(&base)), 0o600, "key file mode");
    }

    #[test]
    fn get_secret_ignores_a_tmp_file_when_no_key_file_exists() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(temp_file(dir.path()), b"partial").expect("write tmp");
        let store = FileKeyStore::new(dir.path());
        let read = store.get_secret(ID.to_owned()).expect("get");
        assert_eq!(read, None, "a lone `.tmp` is not a key");
    }
}
