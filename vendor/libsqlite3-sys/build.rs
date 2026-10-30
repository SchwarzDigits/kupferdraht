//! Build script of Kupferdraht's stand-in for `libsqlite3-sys` 0.35.0 (Step 90,
//! ADR-0021).
//!
//! Reduced to the `bundled` path of the upstream script (rusqlite/rusqlite,
//! MIT): compile the SQLite3MC amalgamation vendored in `../sqlite3mc/`
//! (pinned by its `PROVENANCE.txt`) with the SQLite feature flags upstream
//! uses — verbatim, so the engine runs under the compile options every
//! `bundled` rusqlite build has — then copy the pre-generated bindings.
//!
//! Deliberately absent: the pkg-config / vcpkg / bindgen / OpenSSL paths
//! (SQLite3MC needs no external crypto library), and the
//! `-DSQLITE_EXTRA_INIT=sqlite3mc_initialize` flag CHANGELOG §51 needed for
//! 2.3.5: as of 2.5.x the amalgamation's `sqlite3_initialize` calls
//! `sqlite3mc_initialize` itself, and a second registration would fail
//! initialization instead of being a no-op. The engine identity is pinned at
//! runtime instead (`tests/step_90_sqlite3mc_engine.rs`:
//! `sqlite3mc_version()`, encrypted-header control).

use std::env;
use std::path::Path;

fn main() {
    // wasm32-unknown-unknown (Step 91): no C compile here — sqlite-wasm-rs
    // builds the amalgamation for wasm; src/wasm.rs re-exports its bindings.
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32")
        && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("unknown")
    {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let sqlite3mc_dir = Path::new(&manifest_dir).join("../sqlite3mc");
    let amalgamation = sqlite3mc_dir.join("sqlite3mc_amalgamation.c");
    let header = sqlite3mc_dir.join("sqlite3mc_amalgamation.h");
    assert!(
        amalgamation.exists() && header.exists(),
        "{} / {} missing — vendor/sqlite3mc must hold the pinned SQLite3MC amalgamation (see vendor/README.md)",
        amalgamation.display(),
        header.display()
    );

    std::fs::copy(
        "sqlite3/bindgen_bundled_version.rs",
        Path::new(&out_dir).join("bindgen.rs"),
    )
    .expect("copy the pre-generated bindings into OUT_DIR");

    println!("cargo:include={}", sqlite3mc_dir.display());
    println!("cargo:rerun-if-changed={}", amalgamation.display());
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed=sqlite3/bindgen_bundled_version.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let mut cfg = cc::Build::new();
    cfg.file(&amalgamation)
        // upstream libsqlite3-sys 0.35.0 `bundled` flag set, verbatim
        .flag("-DSQLITE_CORE")
        .flag("-DSQLITE_DEFAULT_FOREIGN_KEYS=1")
        .flag("-DSQLITE_ENABLE_API_ARMOR")
        .flag("-DSQLITE_ENABLE_COLUMN_METADATA")
        .flag("-DSQLITE_ENABLE_DBSTAT_VTAB")
        .flag("-DSQLITE_ENABLE_FTS3")
        .flag("-DSQLITE_ENABLE_FTS3_PARENTHESIS")
        .flag("-DSQLITE_ENABLE_FTS5")
        .flag("-DSQLITE_ENABLE_JSON1")
        .flag("-DSQLITE_ENABLE_LOAD_EXTENSION=1")
        .flag("-DSQLITE_ENABLE_MEMORY_MANAGEMENT")
        .flag("-DSQLITE_ENABLE_RTREE")
        .flag("-DSQLITE_ENABLE_STAT4")
        .flag("-DSQLITE_SOUNDEX")
        .flag("-DSQLITE_THREADSAFE=1")
        .flag("-DSQLITE_USE_URI")
        .flag("-DHAVE_USLEEP=1")
        .flag("-DHAVE_ISNAN")
        .flag("-D_POSIX_THREAD_SAFE_FUNCTIONS")
        .warnings(false);

    // Encrypted-engine hygiene: temporary tables/indices stay in memory so
    // no plaintext spill reaches disk (upstream sets the same flag for its
    // bundled-sqlcipher path; Android's bundled SQLite uses 3).
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "android" {
        cfg.flag("-DSQLITE_TEMP_STORE=3");
    } else {
        cfg.flag("-DSQLITE_TEMP_STORE=2");
    }
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        cfg.flag("-DHAVE_LOCALTIME_R");
    }

    // Upstream's escape hatch, kept verbatim: extra `-D…` defines from the
    // environment. Kupferdraht's default build passes none — the compile-time
    // default cipher (chacha20) is pinned by `tests/step_90_sqlite3mc_engine.rs`.
    if let Ok(extras) = env::var("LIBSQLITE3_FLAGS") {
        for extra in extras.split_whitespace() {
            if extra.starts_with("-D") || extra.starts_with("-U") {
                cfg.flag(extra);
            } else if extra.starts_with("SQLITE_") {
                cfg.flag(format!("-D{extra}"));
            } else {
                panic!("Don't understand {extra} in LIBSQLITE3_FLAGS");
            }
        }
    }
    println!("cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS");

    cfg.compile("sqlite3");
    println!("cargo:lib_dir={out_dir}");
}
