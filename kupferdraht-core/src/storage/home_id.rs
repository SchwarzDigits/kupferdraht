// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The home id (Step 93, ADR-0025) — `base/home.id`, the 32-hex identifier
//! every keystore id of a home and its Wire login cookie label carry.
//!
//! An identifier, not a secret: it sits in plaintext next to the
//! databases, inside the keystore ids and inside the login label Wire
//! sees. What it must be is **stable** — a regenerated id orphans every
//! key of the home — so a malformed file is a recoverable error that is
//! never rewritten. Creation runs under the home's process lock
//! (ADR-0024): two first opens of one fresh home agree on one id, the
//! loser is refused typed and reads the winner's id on retry; an existing
//! id is read lock-free. On wasm the file lives in the OPFS pool (one
//! instance per origin, ADR-0023 — no lock); the byte primitive is per
//! target, the decision logic is this one path.

use std::path::Path;

use crate::error::KupferdrahtError;

/// File name of the home id inside the home (native).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const HOME_ID_FILE: &str = "home.id";
/// A v4 UUID in its simple (hyphen-less) form.
const HOME_ID_LEN: usize = 32;

/// Read the home's id, creating it on the first open of a fresh home.
///
/// # Errors
/// `KupferdrahtError::Storage` — the base directory cannot be created, the
/// file is unreadable or malformed, or the id cannot be written;
/// `KupferdrahtError::AccountLocked` — another process is creating the id at
/// this moment (retry: the retry reads the winner's id).
#[cfg(target_arch = "wasm32")]
pub(crate) fn resolve_home_id(_base: &Path) -> Result<String, KupferdrahtError> {
    // Step 111a: the browser's home id is a keystore entry, resolved when
    // the storage is opened (`storage::browser`).
    super::browser::browser_home_id()
}

/// Read the home's id, creating it on the first open of a fresh home
/// (native: `base/home.id`).
///
/// # Errors
/// As above.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_home_id(base: &Path) -> Result<String, KupferdrahtError> {
    let path = base.join(HOME_ID_FILE);
    std::fs::create_dir_all(base)
        .map_err(|e| KupferdrahtError::Storage(format!("mkdir {}: {e}", base.display())))?;
    if let Some(id) = read_home_id(&path)? {
        return Ok(id);
    }
    tracing::debug!(home = %base.display(), "first open of this home: creating its id");
    let _lock = super::ProcessLock::try_acquire(
        &base.join(super::lock::PROCESS_LOCK_FILE),
        &format!("home {}", base.display()),
    )?;
    // Re-read under the lock: the loser of a race adopts the winner's id.
    if let Some(id) = read_home_id(&path)? {
        return Ok(id);
    }
    let id = generate_home_id();
    debug_assert!(is_home_id(&id), "generated ids are 32 lowercase hex");
    write_bytes(&path, format!("{id}\n").as_bytes())?;
    tracing::info!(home = %base.display(), home_id = %id, "home id created");
    Ok(id)
}

/// A fresh random home id — a v4 UUID in its 32-hex form.
#[must_use]
pub(crate) fn generate_home_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The Wire login cookie label of the home `home_id`: `kupferdraht-<home-id>`.
/// Wire keeps one refresh cookie per `(user, label)`; one label per home
/// is what keeps two homes of one user from evicting each other.
#[must_use]
pub(crate) fn cookie_label(home_id: &str) -> String {
    debug_assert!(is_home_id(home_id), "labels derive from a home id");
    format!("kupferdraht-{home_id}")
}

/// The label of a session that has no home (a builder without
/// `base_dir`): a fresh id per builder — such a session persists no
/// device either, so it never resumes a cookie.
#[must_use]
pub(crate) fn ephemeral_cookie_label() -> String {
    cookie_label(&generate_home_id())
}

/// Exactly 32 lowercase hex characters.
#[must_use]
pub(crate) fn is_home_id(id: &str) -> bool {
    id.len() == HOME_ID_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(not(target_arch = "wasm32"))]
/// The persisted id, or `None` when the file does not exist.
///
/// # Errors
/// `KupferdrahtError::Storage` when the file is unreadable or malformed —
/// runtime data: never a panic, never auto-repaired.
fn read_home_id(path: &Path) -> Result<Option<String>, KupferdrahtError> {
    let Some(bytes) = read_bytes(path)? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|_| malformed(path))?;
    let id = text.trim_end_matches(['\n', '\r']);
    if !is_home_id(id) {
        return Err(malformed(path));
    }
    Ok(Some(id.to_owned()))
}

#[cfg(not(target_arch = "wasm32"))]
fn malformed(path: &Path) -> KupferdrahtError {
    KupferdrahtError::Storage(format!(
        "{}: malformed home id (expected 32 lowercase hex characters); the file is never \
         rewritten — a new id would orphan every key of this home",
        path.display()
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, KupferdrahtError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(KupferdrahtError::Storage(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

/// Atomic on native (temp file → fsync → rename), so a crash never leaves
/// a truncated id behind.
#[cfg(not(target_arch = "wasm32"))]
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), KupferdrahtError> {
    super::write_atomically(path, bytes, false)
        .map_err(|e| KupferdrahtError::Storage(format!("write home id: {e}")))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_32_lowercase_hex_and_distinct() {
        let a = generate_home_id();
        let b = generate_home_id();
        assert!(is_home_id(&a), "{a}");
        assert!(is_home_id(&b), "{b}");
        assert_ne!(a, b);
    }

    #[test]
    fn only_32_lowercase_hex_is_a_home_id() {
        assert!(is_home_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_home_id("0123456789ABCDEF0123456789ABCDEF"), "uppercase");
        assert!(!is_home_id("0123456789abcdef0123456789abcde"), "31 chars");
        assert!(!is_home_id("0123456789abcdef0123456789abcdefg"), "33 chars");
        assert!(!is_home_id(""), "empty");
    }

    #[test]
    fn resolve_creates_the_file_once_and_reads_it_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("home");
        let first = resolve_home_id(&base).expect("first open creates");
        assert_eq!(
            std::fs::read_to_string(base.join(HOME_ID_FILE)).expect("file"),
            format!("{first}\n")
        );
        let second = resolve_home_id(&base).expect("second open reads");
        assert_eq!(first, second);
        assert!(
            !base.join(format!("{HOME_ID_FILE}.tmp")).exists(),
            "no temp residue"
        );
    }

    #[test]
    fn a_malformed_file_is_a_storage_error_naming_it_and_stays_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(HOME_ID_FILE);
        std::fs::write(&path, "ABC\n").expect("write");
        let err = resolve_home_id(dir.path()).expect_err("malformed");
        assert!(
            matches!(&err, KupferdrahtError::Storage(msg) if msg.contains(HOME_ID_FILE)),
            "{err:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "ABC\n");
    }

    #[test]
    fn a_trailing_newline_is_tolerated_but_nothing_else_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(HOME_ID_FILE);
        std::fs::write(&path, "0123456789abcdef0123456789abcdef\r\n").expect("write");
        assert_eq!(
            resolve_home_id(dir.path()).expect("crlf tolerated"),
            "0123456789abcdef0123456789abcdef"
        );
        std::fs::write(&path, " 0123456789abcdef0123456789abcdef\n").expect("write");
        assert!(
            resolve_home_id(dir.path()).is_err(),
            "leading space rejected"
        );
    }

    #[test]
    fn ephemeral_labels_differ_per_call_and_carry_the_prefix() {
        let a = ephemeral_cookie_label();
        let b = ephemeral_cookie_label();
        assert!(a.starts_with("kupferdraht-") && a.len() == "kupferdraht-".len() + HOME_ID_LEN);
        assert_ne!(a, b);
    }
}
