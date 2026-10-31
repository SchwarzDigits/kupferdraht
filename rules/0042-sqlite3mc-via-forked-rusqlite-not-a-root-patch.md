# SQLite3MC through the SchwarzDigits forks, not a root `[patch]`

Condensed from `docs/adrs/0042-sqlite3mc-via-forked-rusqlite-not-a-root-patch.md`.

Replaces the build-glue bullet of `rules/0021-sqlite3mc-engine-chacha20.md`
and the stand-in bullet of
`rules/0022-browser-persistence-sqlite3mc-over-opfs.md` (2026-10-31, Step 127).

- **Take `rusqlite`, `refinery` and `openmls_sqlite_storage` from the
  SchwarzDigits forks as git dependencies by tag** —
  github.com/SchwarzDigits/rusqlite (its `libsqlite3-sys` bundles the
  SQLite3MC amalgamation; `bundled` always means SQLite3MC there),
  github.com/SchwarzDigits/refinery and github.com/SchwarzDigits/openmls
  (branch `sqlite3mc/sqlite_storage-<line>`). Dependency and
  dev-dependency sites alike.
- **The same `git` URL and `tag` at every site, in Kupferdraht and inside
  the forks** — another tag or a `rev` of the same URL is a second source,
  and the graph holds one `libsqlite3-sys` (`links = "sqlite3"`).
- **Don't** reach the engine through a `[patch]` — Cargo honours it in the
  root manifest only; a consumer without it links vanilla SQLite and
  writes plaintext without an error. No `[patch]` in the workspace at all.
- **Don't** let a crates.io `rusqlite` into the graph; a consumer with its
  own SQLite takes the fork at Kupferdraht's tag (`docs/library.md`
  "Consuming from Rust").
- **Keep `vendor/sqlite3mc/` as the pin:** PROVENANCE-verified bytes, the
  wasm source (`SQLITE_WASM_RS_SOURCE_DIR`), and the test that reads the
  fork's copy off the resolved checkout and asserts it byte-identical.
- **Bump in order:** rusqlite fork → refinery and openmls branches →
  `kupferdraht-core/Cargo.toml` → gate; an SQLite3MC release replaces the
  vendored amalgamation and the fork's copy together (`vendor/README.md`).
