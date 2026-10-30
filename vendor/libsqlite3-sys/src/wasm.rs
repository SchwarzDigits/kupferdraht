//! wasm32-unknown-unknown arm of the stand-in (Step 91).
//!
//! rusqlite 0.37 has no browser path of its own (that arrived with rusqlite
//! 0.38's `ffi-sqlite-wasm-rs`); Kupferdraht's line is pinned to 0.37 by
//! openmls_sqlite_storage 0.3. So the stand-in — already Kupferdraht's engine glue
//! (ADR-0021) — supplies the path: on this target `libsqlite3_sys::*` IS
//! `sqlite_wasm_rs::*`, the same SQLite3MC amalgamation compiled for wasm.
//!
//! The only additions are two stubs for entry points SQLite marks deprecated
//! and sqlite-wasm-rs compiles out (`SQLITE_OMIT_DEPRECATED`): rusqlite 0.37
//! still declares safe wrappers around them (`Connection::trace` /
//! `Connection::profile`, both `#[deprecated]`), so the names must resolve.
//! Kupferdraht never calls them; the stubs are no-ops returning the "previous
//! callback" as null, exactly what SQLite returns when none was set.

use core::ffi::{c_char, c_void, CStr};

pub use sqlite_wasm_rs::*;

/// Deprecated `sqlite3_trace` (compiled out of the wasm engine) — inert stub.
///
/// # Safety
/// No-op; never dereferences its arguments.
pub unsafe fn sqlite3_trace(
    _db: *mut sqlite3,
    _x_trace: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    _arg: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

/// Deprecated `sqlite3_profile` (compiled out of the wasm engine) — inert stub.
///
/// # Safety
/// No-op; never dereferences its arguments.
pub unsafe fn sqlite3_profile(
    _db: *mut sqlite3,
    _x_profile: Option<unsafe extern "C" fn(*mut c_void, *const c_char, sqlite3_uint64)>,
    _arg: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

/// Wrap the registered VFS `vfs_name` with SQLite3MC's cipher VFS
/// (`multipleciphers-<vfs_name>`) and optionally make that the default.
///
/// SQLite3MC only auto-wraps the VFS that was default when it initialised
/// (sqlite-wasm-rs's in-memory VFS); a VFS registered afterwards — the OPFS
/// `sahpool` — carries no cipher layer, and `PRAGMA key` on it fails with
/// "Encryption is not supported by the VFS". This is the one FFI call the
/// browser path needs; it lives here, in the engine glue, so kupferdraht-core's
/// hand-written code stays free of `unsafe`. Returns SQLite's result code.
#[must_use]
pub fn sqlite3mc_wrap_vfs(vfs_name: &CStr, make_default: bool) -> core::ffi::c_int {
    // SAFETY: `vfs_name` is a valid NUL-terminated string for the duration of
    // the call; SQLite3MC copies what it needs and never retains the pointer.
    unsafe { sqlite3mc_vfs_create(vfs_name.as_ptr(), core::ffi::c_int::from(make_default)) }
}
