# `vendor/` — third-party build inputs (Step 90, ADR-0021)

Not Kupferdraht code. These directories track upstream releases byte-for-byte
(or, for the stand-in, upstream source minus stripped build paths); the
hand-written-code rules of `CLAUDE.md` (no `unsafe`, clippy pedantic, …)
do not apply to them, and Kupferdraht's own license does not cover them — each
directory carries its upstream `LICENSE`.

| Directory | What | Upstream | License |
|---|---|---|---|
| `sqlite3mc/` | SQLite3 Multiple Ciphers amalgamation (`.c` + `.h`), the whole-DB encryption engine under `kupferdraht.db`, `mls.db`, `global.db` | utelle/SQLite3MultipleCiphers, pinned by version + SHA-256 in `PROVENANCE.txt` and verified against the release's signed `SHA256SUMS` | MIT (SQLite: public domain) |
| `libsqlite3-sys/` | a stand-in for `libsqlite3-sys` 0.35.0: upstream's `src/` (`src/native.rs`) and pre-generated `bundled` bindings, a `build.rs` reduced to the `bundled` path that compiles `../sqlite3mc/`; plus a Kupferdraht-authored **wasm arm** (`src/wasm.rs`, Step 91 / ADR-0022) that re-exports `sqlite-wasm-rs` on `wasm32-unknown-unknown`, stubs the two deprecated entry points rusqlite 0.37 still names, and wraps the one FFI call the browser path needs (`sqlite3mc_wrap_vfs`) | rusqlite/rusqlite | MIT |

## Why a `[patch.crates-io]` stand-in

The root `Cargo.toml` patches `libsqlite3-sys` to `vendor/libsqlite3-sys`,
so rusqlite's `bundled` feature — which `openmls_sqlite_storage` 0.3 forces
workspace-wide — compiles the SQLite3MC amalgamation instead of vanilla
SQLite, deterministically and without any environment probe. The
alternative, libsqlite3-sys's non-bundled `SQLITE3_LIB_DIR` route, probes
pkg-config first and silently links a system SQLite wherever a `sqlite3.pc`
is findable (`docs/observations.md` §77) — never use it here. No
`-DSQLITE_EXTRA_INIT=sqlite3mc_initialize`: SQLite3MC 2.5.x registers
itself, a second registration fails initialization.

**The wasm arm (Step 91, ADR-0022).** rusqlite 0.37 has no browser path of
its own (that arrived in 0.38; the 0.37 line is pinned by
`openmls_sqlite_storage` 0.3), so on `wasm32-unknown-unknown` the stand-in
is the path: `build.rs` returns early (no C compile here), `src/lib.rs`
switches to `src/wasm.rs`, which re-exports `sqlite-wasm-rs` — the same
amalgamation, compiled for wasm by that crate from `../sqlite3mc/` (the
root `.cargo/config.toml` sets `SQLITE_WASM_RS_SOURCE_DIR`; the crate's
bundled copy is byte-identical at 2.5.1 but never the pin). Kupferdraht-authored
code lives in that file on purpose: the two `unsafe` stubs
(`sqlite3_trace` / `sqlite3_profile`, deprecated, compiled out of the wasm
engine, never called) and the one `unsafe` FFI call
(`sqlite3mc_vfs_create`, behind the safe `sqlite3mc_wrap_vfs`) are engine
glue, and `kupferdraht-core`'s hand-written code stays free of `unsafe`
(CLAUDE.md). `kupferdraht-core/tests/step_91_wasm_build_glue.rs` pins the arm's
presence and that the raw call never appears in `kupferdraht-core/src`.

The engine is pinned at runtime, not trusted from the build:
`kupferdraht-core/tests/step_90_sqlite3mc_engine.rs` asserts the lockfile
(path source, no OpenSSL), `sqlite3mc_version()` against `PROVENANCE.txt`,
the encrypted-header control, and the at-rest byte layout.

## Maintenance recipe

1. **SQLite3MC release** — download the new amalgamation zip plus
   `SHA256SUMS` / `.sig` / `.pem`; verify (recipe in `PROVENANCE.txt`);
   replace `sqlite3mc_amalgamation.{c,h}` and `LICENSE` if it changed;
   rewrite `PROVENANCE.txt`; run the Step-90 pins and the full gate.
2. **rusqlite / libsqlite3-sys bump** — copy the new upstream
   `libsqlite3-sys` `src/` into `src/native.rs` and
   `sqlite3/bindgen_bundled_version.rs` into the stand-in, bump its
   `version`, re-check `build.rs` against upstream's `bundled` flag set,
   and re-check `src/wasm.rs` against the symbols the new rusqlite names
   (a rusqlite ≥ 0.38 could use its own `ffi-sqlite-wasm-rs` path and
   retire the arm). Kupferdraht stays on rusqlite 0.37 until
   `openmls_sqlite_storage` moves (it pins 0.37).
4. **sqlite-wasm-rs bump** — re-verify that the crate's bundled
   amalgamation still matches `../sqlite3mc/` (or that
   `SQLITE_WASM_RS_SOURCE_DIR` is still honoured) and that
   `sqlite3mc_vfs_create` is still in its bindings; run
   `scripts/wasm-test.sh`.
3. **Never** pass `SQLITE_EXTRA_INIT`; keep the engine-identity pins.
