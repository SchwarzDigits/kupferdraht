//! Kupferdraht's stand-in for `libsqlite3-sys` 0.35.0 (Step 90, ADR-0021) —
//! `src/lib.rs` of the upstream crate (rusqlite/rusqlite, MIT) minus the
//! OpenSSL link shim. The bindings are upstream's pre-generated `bundled`
//! set; the C behind them is the SQLite3MC amalgamation vendored in
//! `../sqlite3mc/` (see `build.rs`).

// wasm32-unknown-unknown (Step 91): the bindings come from sqlite-wasm-rs
// (the same amalgamation, compiled for wasm); see src/wasm.rs.
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod wasm;
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use wasm::*;

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
mod native;
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
pub use native::*;
