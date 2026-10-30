// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `ProcessLock` — the advisory cross-process lock on a Kupferdraht directory
//! (Step 85). Native only; the browser has one instance per origin by
//! construction (ADR-0023, the Web Locks lease).
//!
//! The primitive is the standard library's advisory file lock
//! (`File::try_lock` — `flock(LOCK_EX | LOCK_NB)` on Unix, `LockFileEx`
//! on Windows). The kernel releases it when the holding process dies, so
//! a crash leaves nothing to clean up: the lock **file** stays on disk
//! but holds no lock. `flock` binds to the open file description, not to
//! the process — two handles in one process contend exactly like two
//! processes, which is what makes the in-process tests honest.
//!
//! Fail-fast by design: a held lock is `KupferdrahtError::AccountLocked`, the
//! caller retries. The holder records its pid in the file (best effort)
//! so a refusal can name it. Two lock files use it: `kupferdraht.lock`
//! (Step 85, ADR-0024 — its callers fail fast) and `sync.lock` (Step
//! 97d, ADR-0031 — `client/drain_lock.rs` retries it for a bounded time).

use std::fs::{self, File, TryLockError};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;

use crate::error::KupferdrahtError;

/// File name of the advisory lock inside a home or account directory.
pub(crate) const PROCESS_LOCK_FILE: &str = "kupferdraht.lock";

/// File name of the account's drain lock (Step 97d, ADR-0031): one
/// cursor-advancing dispatch per account at a time.
pub(crate) const SYNC_LOCK_FILE: &str = "sync.lock";

/// An exclusive advisory lock on a `kupferdraht.lock` file, held until dropped.
#[derive(Debug)]
pub(crate) struct ProcessLock {
    file: File,
}

impl ProcessLock {
    /// Try to take the lock file at `path` (a `KupferdrahtHome` lock path —
    /// `home_lock` / `account_lock`) without waiting. `scope` names what
    /// is being locked for the error message (`"account <id>"`,
    /// `"home <path>"`).
    ///
    /// # Errors
    /// `KupferdrahtError::AccountLocked` if another handle holds the lock;
    /// `KupferdrahtError::Storage` if the parent directory does not exist or
    /// the lock file cannot be created or locked.
    pub(crate) fn try_acquire(path: &Path, scope: &str) -> Result<Self, KupferdrahtError> {
        debug_assert!(!scope.is_empty(), "scope names what is locked");
        debug_assert!(
            path.file_name()
                .is_some_and(|name| name == PROCESS_LOCK_FILE || name == SYNC_LOCK_FILE),
            "lock paths come from KupferdrahtHome"
        );
        if !path.parent().is_some_and(Path::is_dir) {
            return Err(KupferdrahtError::Storage(format!(
                "{scope}: lock directory for {} does not exist",
                path.display()
            )));
        }
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| {
                KupferdrahtError::Storage(format!("{scope}: open {}: {e}", path.display()))
            })?;
        match file.try_lock() {
            Ok(()) => {
                record_holder(&mut file, path);
                Ok(Self { file })
            }
            Err(TryLockError::WouldBlock) => Err(KupferdrahtError::AccountLocked(describe_holder(
                &mut file, scope,
            ))),
            Err(TryLockError::Error(e)) => Err(KupferdrahtError::Storage(format!(
                "{scope}: lock {}: {e}",
                path.display()
            ))),
        }
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        // Closing the file releases the lock anyway; the explicit unlock
        // makes the release point visible. Nothing to do on failure.
        let _ = self.file.unlock();
    }
}

/// Record this process's pid in the lock file so a refused process can
/// name the holder. Best effort: the lock is held regardless.
fn record_holder(file: &mut File, path: &Path) {
    let pid = std::process::id().to_string();
    let written = file
        .set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)))
        .and_then(|_| file.write_all(pid.as_bytes()))
        .and_then(|()| file.flush());
    if let Err(e) = written {
        tracing::debug!(path = %path.display(), error = %e, "could not record the lock holder pid");
    }
}

/// `"<scope> (held by pid N)"`, or `"(holder unknown)"` when the holder
/// has not written its pid (yet) — runtime data, never asserted.
fn describe_holder(file: &mut File, scope: &str) -> String {
    let mut content = String::new();
    let pid = file
        .read_to_string(&mut content)
        .ok()
        .and_then(|_| content.trim().parse::<u32>().ok());
    match pid {
        Some(pid) => format!("{scope} (held by pid {pid})"),
        None => format!("{scope} (holder unknown)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn lock_path(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join(PROCESS_LOCK_FILE)
    }

    #[test]
    fn second_acquire_on_the_same_path_is_refused_typed_and_names_the_holder() {
        let dir = TempDir::new().expect("tempdir");
        let _first = ProcessLock::try_acquire(&lock_path(&dir), "account alice").expect("first");
        assert!(lock_path(&dir).is_file(), "the lock file is created");
        let err = ProcessLock::try_acquire(&lock_path(&dir), "account alice")
            .expect_err("a second handle must be refused while the first is held");
        match err {
            KupferdrahtError::AccountLocked(msg) => {
                assert!(msg.contains("account alice"), "names the scope: {msg}");
                let pid = std::process::id().to_string();
                assert!(msg.contains(&pid), "names the holder pid {pid}: {msg}");
            }
            other => panic!("expected AccountLocked, got {other:?}"),
        }
    }

    #[test]
    fn dropping_the_lock_frees_it_for_the_next_acquire() {
        let dir = TempDir::new().expect("tempdir");
        let first = ProcessLock::try_acquire(&lock_path(&dir), "account alice").expect("first");
        drop(first);
        let _second =
            ProcessLock::try_acquire(&lock_path(&dir), "account alice").expect("free after drop");
        assert!(
            lock_path(&dir).is_file(),
            "the lock file stays on disk; only the kernel lock is released"
        );
    }

    #[test]
    fn locks_of_different_directories_are_independent() {
        let a = TempDir::new().expect("tempdir a");
        let b = TempDir::new().expect("tempdir b");
        let _lock_a = ProcessLock::try_acquire(&lock_path(&a), "home a").expect("lock a");
        let _lock_b = ProcessLock::try_acquire(&lock_path(&b), "home b")
            .expect("a lock in another directory must not interfere");
    }

    #[test]
    fn missing_directory_is_a_storage_error_not_a_panic() {
        let root = TempDir::new().expect("tempdir");
        let missing = root.path().join("does-not-exist").join(PROCESS_LOCK_FILE);
        let err =
            ProcessLock::try_acquire(&missing, "account alice").expect_err("no directory, no lock");
        assert!(
            matches!(err, KupferdrahtError::Storage(_)),
            "expected Storage, got {err:?}"
        );
    }
}
