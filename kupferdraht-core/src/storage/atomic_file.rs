// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Crash-safe file replacement: temp file → fsync → rename over the
//! target → fsync the directory. Shared by the dev `FileKeyStore`
//! (Step 77: a torn key file bricks the database it keys) and the home id
//! file (Step 93: a torn `home.id` orphans every key of the home). Plain
//! `std::fs`, so it compiles on every target; the browser's own files
//! live in the OPFS pool (`storage::browser`) and never come through here.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically. `private` creates the file with mode
/// 0600 on Unix (secrets); otherwise the process umask applies. A failure
/// anywhere leaves `path` as it was and removes the temp file (best
/// effort); a stale `.tmp` from an earlier crash is discarded unread.
///
/// # Errors
/// The failing step's `io::Error`, with the path in the message.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let temp = temp_path(path);
    debug_assert!(temp != path, "temp path must differ from the target");
    remove_stale_temp(&temp)?;
    let written = write_temp_file(&temp, bytes, private).and_then(|()| rename_over(&temp, path));
    if written.is_err() {
        // Never leave a half-written file behind; the original error is
        // the one worth reporting.
        let _ = fs::remove_file(&temp);
    }
    written?;
    // fsync the directory so the rename itself is durable. A directory
    // cannot be opened as a file on Windows; the rename is left to the
    // OS's own metadata durability there.
    #[cfg(unix)]
    sync_parent_dir(path)?;
    Ok(())
}

/// `<path>.tmp` — the sibling the bytes are staged in.
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// A `.tmp` left by a crashed earlier write is discarded unread — it may
/// be truncated, world-readable, or another process's.
fn remove_stale_temp(temp: &Path) -> io::Result<()> {
    match fs::remove_file(temp) {
        Ok(()) => {
            tracing::debug!(temp = %temp.display(), "removed stale temp file");
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(with_path("remove stale", temp, &e)),
    }
}

/// Create the temp file fresh (`create_new` — a stale one was removed just
/// before), with the private mode set at creation on Unix when asked;
/// write and fsync so the bytes are on disk before the rename makes them
/// visible.
fn write_temp_file(temp: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(temp)
        .map_err(|e| with_path("create", temp, &e))?;
    file.write_all(bytes)
        .map_err(|e| with_path("write", temp, &e))?;
    file.sync_all().map_err(|e| with_path("fsync", temp, &e))
}

fn rename_over(temp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temp, path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("rename {} over {}: {e}", temp.display(), path.display()),
        )
    })
}

/// fsync the file's directory so the rename itself is durable.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> io::Result<()> {
    debug_assert!(path.parent().is_some(), "path has a parent directory");
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| with_path("fsync", dir, &e))
}

fn with_path(what: &str, path: &Path, e: &io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{what} {}: {e}", path.display()))
}
