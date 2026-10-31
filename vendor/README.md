# `vendor/` — third-party build inputs (Step 90, ADR-0021; Step 127, ADR-0042)

Not Kupferdraht code. This directory tracks an upstream release
byte-for-byte; the hand-written-code rules of `CLAUDE.md` (no `unsafe`,
clippy pedantic, …) do not apply to it, and Kupferdraht's own license does
not cover it — it carries its upstream `LICENSE`.

| Directory | What | Upstream | License |
|---|---|---|---|
| `sqlite3mc/` | SQLite3 Multiple Ciphers amalgamation (`.c` + `.h`), the whole-DB encryption engine under `kupferdraht.db`, `mls.db`, `global.db` | utelle/SQLite3MultipleCiphers, pinned by version + SHA-256 in `PROVENANCE.txt` and verified against the release's signed `SHA256SUMS` | MIT (SQLite: public domain) |

## Where the engine is compiled

Natively, by the `libsqlite3-sys` of the SchwarzDigits rusqlite fork
(github.com/SchwarzDigits/rusqlite, branch `sqlite3mc/0.37`, consumed by
tag — ADR-0042): its `bundled` feature compiles a copy of this
amalgamation that lives in the fork at `libsqlite3-sys/sqlite3mc/`, with
upstream's `bundled` flag set plus `SQLITE_TEMP_STORE=2`. refinery and
`openmls_sqlite_storage` come from sibling forks on that rusqlite
(github.com/SchwarzDigits/refinery, github.com/SchwarzDigits/openmls branch
`sqlite3mc/sqlite_storage-0.3`), so the graph holds one rusqlite. No root
`[patch]`, no system SQLite, no pkg-config, no OpenSSL. The earlier
`[patch.crates-io]` stand-in that lived here (Steps 90–126) is gone: a
`[patch]` is honoured only in the root manifest of the build, so every
Rust consumer of `kupferdraht-core` had to repeat it, and one that did not
linked vanilla SQLite and wrote plaintext databases without an error.

On `wasm32-unknown-unknown` the fork's sys crate re-exports
`sqlite-wasm-rs`, which compiles the amalgamation for wasm from THIS
directory: the root `.cargo/config.toml` sets `SQLITE_WASM_RS_SOURCE_DIR`
to `vendor/sqlite3mc/` (the crate's bundled copy is byte-identical at
2.5.1 but never the pin).

This directory stays the pin for both: `kupferdraht-core/tests/step_90_sqlite3mc_engine.rs`
asserts the SHA-256 of both files against `PROVENANCE.txt`, that the
fork's copy (read off the resolved checkout) is byte-identical to them,
the lockfile sources (the three forks by tag, no OpenSSL, no `[patch]`),
`sqlite3mc_version()` against `PROVENANCE.txt`, the encrypted-header
control and the at-rest byte layout.

## Maintenance recipe

1. **SQLite3MC release** — download the new amalgamation zip plus
   `SHA256SUMS` / `.sig` / `.pem`; verify (recipe in `PROVENANCE.txt`);
   replace `sqlite3mc_amalgamation.{c,h}` and `LICENSE` if it changed;
   rewrite `PROVENANCE.txt`. Then copy the same four files into the
   rusqlite fork's `libsqlite3-sys/sqlite3mc/`, tag the fork
   `v<rusqlite>-sqlite3mc-<mc>`, retag refinery and the openmls branch on
   the new rusqlite tag (their manifests name it), move the three tags in
   `kupferdraht-core/Cargo.toml`, and run the Step-90 pins and the full
   gate.
2. **rusqlite / libsqlite3-sys bump** — rebase the fork's `sqlite3mc/`
   branch onto upstream's release tag (the fork's `README.md` has the
   recipe: re-check `build.rs` against upstream's `bundled` block and
   `src/wasm.rs` against the symbols the new rusqlite names; a rusqlite
   ≥ 0.38 could use its own `ffi-sqlite-wasm-rs` path and retire the
   arm), tag, then the refinery and openmls branches as in 1. Kupferdraht
   stays on rusqlite 0.37 until `openmls_sqlite_storage` moves (0.3.0
   pins 0.37; openmls `main` is on 0.40, unreleased).
3. **sqlite-wasm-rs bump** — re-verify that the crate's bundled
   amalgamation still matches `sqlite3mc/` (or that
   `SQLITE_WASM_RS_SOURCE_DIR` is still honoured) and that
   `sqlite3mc_vfs_create` is still in its bindings; bump it in the fork's
   `libsqlite3-sys/Cargo.toml` too; run `scripts/wasm-test.sh`.
4. **Never** pass `SQLITE_EXTRA_INIT=sqlite3mc_initialize` (2.5.x registers
   itself; a second registration fails initialization); keep the
   engine-identity pins.
