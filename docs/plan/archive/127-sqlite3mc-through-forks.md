# Step 127 — SQLite3MC through the SchwarzDigits forks, not a root `[patch]`

**Status: MERGED 2026-10-31** — commissioned by Volker the same day,
after the first Rust embedder (a TUI; its open.md §21) reported that it had
to carry Kupferdraht's `[patch.crates-io]` entry itself. `613aac9`. Merged
to `main` as PR #3 (`70fc378`); CI green, the runners fetching the three
forks by tag. No surface change, no CLI command, no smoke (build glue).
Scope shipped: rusqlite, refinery and openmls_sqlite_storage from the
SchwarzDigits forks by git tag, the vendored stand-in and the root
`[patch]` gone, ADR-0042 with its rule. Archived with this header.

## Problem

Since Step 90 the SQLite3MC engine was reached through a root
`[patch.crates-io] libsqlite3-sys = { path = "vendor/libsqlite3-sys" }`.
Cargo honours a `[patch]` only in the root manifest of the build, so a
Rust consumer of `kupferdraht-core` had to repeat the entry, and nothing
told it to: neither README nor `docs/library.md` mentioned it.

Measured on 2026-10-31:

- A scratch crate depending on `kupferdraht-core` by git from the public
  repository resolved `libsqlite3-sys 0.35.0` from crates.io without the
  entry, and `vendor/libsqlite3-sys` with it.
- Against vanilla rusqlite 0.37 (`bundled`), Kupferdraht's exact keying
  batch (`PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32;
  PRAGMA key = "x'…'"`) returned `Ok`, the first read returned `Ok(0)`,
  and the file was written with a plaintext header and plaintext rows:
  SQLite ignores unknown pragmas. Only `SELECT sqlite3mc_version()` fails
  there ("no such function").

So a consumer without the line compiled, ran, and wrote unencrypted
databases; an existing encrypted home failed to open with "file is not a
database".

## Why not the smaller fix, and why not something else

- **Document the patch block and add a link-time guard** (an `extern "C"
  fn sqlite3mc_version` reference in `open_connection`, so a vanilla link
  fails at build time): cheap and loud, but every consumer keeps carrying
  the line. Rejected by Volker in favour of the structural fix.
- **A newer libsqlite3-sys**: 0.39.1 (2026-10-25) and rusqlite 0.41.1
  know only SQLCipher as a bundled encrypting engine. And the line is
  pinned to rusqlite 0.37 by `openmls_sqlite_storage` 0.3.0 anyway.
- **Upstream**: rusqlite PR #1726 (2025-08-21) proposed a
  `bundled-sqlite3mc` feature; the maintainer declined on 2025-08-23
  ("rusqlite cannot bundle all the SQLite forks… you can already link to
  your own SQLite fork"). A feature would have propagated through the
  graph; a `[patch]` does not.
- **SQLite3MC as a loadable extension over vanilla SQLite**: not
  possible; its documentation requires patches to the amalgamation.
- **libsqlite3-sys's non-bundled `SQLITE3_LIB_DIR` route**: rejected since
  Step 88 (observations §77: pkg-config is probed first and a system
  SQLite silently linked), and it is again a consumer-side setting.
- **Own crates under new names on crates.io**: refinery and
  openmls_sqlite_storage name `rusqlite` from crates.io, so both would
  have to be replaced, and publishing adds process. Not needed:
  `kupferdraht-core` is `publish = false` and consumed by git, and git
  dependencies are inherited transitively (the clean-machine build of
  2026-10-30 proved it for sqlite-remote-vfs).

## Decision (ADR-0042)

Three forks in the SchwarzDigits organisation, consumed as git
dependencies by tag, with the identical source spec at every site (Cargo
treats a different tag or rev of the same URL as a different source, and
two `libsqlite3-sys` collide on `links = "sqlite3"`):

| Fork | Base | Branch / tag | Change |
|---|---|---|---|
| SchwarzDigits/rusqlite | v0.37.0 | `sqlite3mc/0.37`, tag `v0.37.0-sqlite3mc-2.5.1` | `libsqlite3-sys`: `bundled` compiles `libsqlite3-sys/sqlite3mc/` (the amalgamation, `LICENSE`, `PROVENANCE.txt` copied from `vendor/sqlite3mc/`) with upstream's flag set plus `SQLITE_TEMP_STORE=2`; SQLCipher features refused; the wasm arm (`src/wasm.rs` re-exporting `sqlite-wasm-rs`, the two deprecated stubs, `sqlite3mc_wrap_vfs`); upstream's crate root unchanged in `src/native.rs` |
| SchwarzDigits/refinery | v0.9.1 | `sqlite3mc/0.9`, tag `v0.9.1-sqlite3mc` | `refinery-core` (and the examples) take `rusqlite` from the rusqlite fork by tag |
| SchwarzDigits/openmls | openmls-v0.9.0 | `sqlite3mc/sqlite_storage-0.3`, tag `sqlite_storage-v0.3.0-sqlite3mc` | `sqlite_storage`: `rusqlite` and `refinery` from the forks by tag; `openmls_traits = "0.6.0"` from crates.io instead of the workspace path (the consumer's openmls 0.9 resolves the same package; the graph must hold the trait crate once) |

The openmls and SQLite3MultipleCiphers forks of the organisation existed
before as Markus's staging area for upstream pull requests (openmls: six,
four merged; SQLite3MC: ten, nine merged); `main` of the openmls fork was
fast-forwarded to upstream on 2026-10-31 and keeps tracking it. The new
branch is a long-lived downstream branch beside the PR heads, never a PR
head itself.

Kupferdraht: the three dependencies by tag (dependency and
dev-dependency sites alike), the root `[patch]` and the workspace
`exclude` gone, `vendor/libsqlite3-sys/` deleted, `vendor/sqlite3mc/`
kept as the provenance pin and the wasm source (`SQLITE_WASM_RS_SOURCE_DIR`
unchanged), `deny.toml` allow-git extended, the ORT exclusion of the
stand-in removed.

## Tests (red first, 2026-10-31)

- `step_90_sqlite3mc_engine`: `lockfile_resolves_the_sqlite3mc_line_to_the_forks`
  (the six lock blocks carry the fork sources by tag; one `libsqlite3-sys`
  and one `rusqlite` in the graph; no `[patch.` in the root manifest; the
  stand-in directory gone; the manifest and `deny.toml` name the tags
  and URLs) and `forks_amalgamation_matches_the_provenance_record` (the
  fork's `sqlite3mc/*.{c,h}`, read off the checkout `cargo metadata`
  resolves, hash to `PROVENANCE.txt`'s pins).
- `step_91_wasm_build_glue`: `the_forks_sys_crate_carries_the_wasm_arm`
  (the resolved sys crate's manifest and `src/wasm.rs`, same symbols as
  before; the raw FFI call still never in `kupferdraht-core/src`).
- `step_111a_local_vfs_pins`: `allow-git` is a list now; the VFS URL must
  be in it.

The engine identity, at-rest layout and encrypted-header control tests
of Step 90 run unchanged against the fork's engine.

## Gate

The five plus the wasm32 check and clippy; the forks' tags must be on
GitHub before the lockfile can resolve (the fork branches were built and
probed locally first: native build, `sqlite3mc_version()` = "SQLite3
Multiple Ciphers 2.5.1", `sqlite_version()` = 3.53.4, default cipher
chacha20, `cargo check --target wasm32-unknown-unknown` of the sys crate).
`cargo about` regenerated `docs/THIRD_PARTY_LICENSES.html`;
THIRD-PARTY-NOTICES edited by hand (the forks are the same code).

Run on the pushed tags, 2026-10-31: 0 build warnings, fmt, clippy, every
test green including the three new pins, `cargo deny check` with only the
pre-existing duplicate-version warnings, wasm32 check and clippy. The
lockfile moved six packages to the git sources and gained one: `vcpkg`
0.2.15, a build dependency of the fork's full `libsqlite3-sys` that the
reduced stand-in had dropped (idle on the bundled path; recorded in
THIRD-PARTY-NOTICES).

## Follow-ups (outside this step)

- The TUI drops its `[patch.crates-io]` entry (Cargo warns "unused patch"
  until it does).
- The next rusqlite line (0.40, when `openmls_sqlite_storage` releases on
  it; openmls `main` is there already): rebase the three branches, and a
  rusqlite ≥ 0.38 can retire the wasm arm for its own `ffi-sqlite-wasm-rs`.
- The openmls fork's branch policy (a downstream branch beside the PR
  heads) is Markus's to confirm.
