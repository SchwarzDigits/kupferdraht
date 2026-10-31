# ADR-0042: Bundle SQLite3MC through forked rusqlite, refinery and openmls_sqlite_storage, not through a root `[patch]`

**Status:** Accepted
**Date:** 2026-10-31
**Deciders:** Volker Zöpfel

## Context

ADR-0021 adopted SQLite3 Multiple Ciphers (SQLite3MC) as the whole-database
encryption engine and reached it through the build glue of Step 90: a
`libsqlite3-sys` stand-in in `vendor/`, routed by a root
`[patch.crates-io]` so that rusqlite's `bundled` feature compiles the
vendored amalgamation instead of vanilla SQLite. ADR-0022 added a wasm arm
to that stand-in. The glue was correct for every build of the workspace
itself — the CLI, the Kotlin, Swift and browser bindings all build there.

It was wrong for a Rust consumer of `kupferdraht-core`. Cargo honours a
`[patch]` only in the root manifest of the build; a dependency's patch is
not inherited. The first embedder, a TUI, found that out and carried the
entry itself since its Step 16, but neither README nor `docs/library.md`
told anyone to. Since the repository went public on 2026-10-30 any Rust
project can depend on it, and on 2026-10-31 the failure mode was measured:
a scratch crate depending on `kupferdraht-core` by git resolved crates.io's
`libsqlite3-sys`; against that engine Kupferdraht's keying batch (`PRAGMA
cipher`, `PRAGMA plaintext_header_size`, `PRAGMA key`) returned `Ok`, the
verifying first read returned `Ok`, and the file was written with a
plaintext header and plaintext rows — SQLite ignores unknown pragmas. Only
`SELECT sqlite3mc_version()` fails there. A consumer without the line
compiled, ran and wrote unencrypted databases without an error.

Constraints. A dependency graph holds one `libsqlite3-sys` (its `links =
"sqlite3"`), and `openmls_sqlite_storage` 0.3.0 and `refinery` 0.9 both
name `rusqlite` from crates.io, which pins the line to rusqlite 0.37.
Upstream declines to bundle SQLite forks: rusqlite PR #1726 proposed a
`bundled-sqlite3mc` feature on 2025-08-21 and was turned down two days
later; a newer libsqlite3-sys (0.39.1, 2026-10-25) still knows only
SQLCipher. SQLite3MC cannot run as a loadable extension over vanilla
SQLite (its documentation requires patches to the amalgamation).
libsqlite3-sys's non-bundled `SQLITE3_LIB_DIR` route was rejected in Step
88 (it probes pkg-config first and silently links a system SQLite;
observations §77) and would again be a consumer-side setting.
`kupferdraht-core` is `publish = false` and consumed by git, and Cargo
resolves a git dependency's own git dependencies transitively (the
clean-machine build of the publication day proved it for
sqlite-remote-vfs, ADR-0035). The SchwarzDigits organisation already holds
forks of openmls and SQLite3MultipleCiphers as the staging area for its
upstream pull requests.

## Options Considered

### Option A: Keep the `[patch]`, document it, fail loudly without it

Add a "Consuming from Rust" section with the patch block to copy, and
reference the C symbol `sqlite3mc_version` from `open_connection` so a
build against vanilla SQLite fails at link time instead of writing
plaintext.

**Pros:**
- One day of work; the trap becomes a build error at once.
- Nothing moves out of the tree.

**Cons:**
- Every consumer keeps carrying a line that is not theirs, and has to
  move it with every bump.
- The consumer's manifest still decides whether Kupferdraht's storage is
  encrypted.

### Option B: Forks consumed as git dependencies by tag

Fork rusqlite (whose repository carries `libsqlite3-sys`), refinery and
openmls in the SchwarzDigits organisation; one branch per upstream line
with manifest changes only; `kupferdraht-core` depends on the three by
tag, and the forks name each other by the same tags.

**Pros:**
- A consumer needs the one `kupferdraht-core` line; Cargo inherits the
  rest. Features and sources propagate through the graph, a `[patch]`
  does not.
- A second, crates.io rusqlite in a consumer's graph fails the build
  (`links` collision) — loud, not silent.
- The stand-in leaves `vendor/`; the sys crate lives where it belongs and
  the compliance analyzer no longer needs an exclusion for it.

**Cons:**
- Three fork branches to rebase on every bump Kupferdraht takes.
- The openmls fork, until now PR heads only, gains a long-lived downstream
  branch.
- Consumers cannot combine Kupferdraht with a crates.io rusqlite of their
  own; they take the fork.

### Option C: Own crates under new names on crates.io

Publish a renamed rusqlite + sys pair, and replace refinery and
openmls_sqlite_storage (both name `rusqlite` from crates.io) by own code
or renamed forks.

**Pros:**
- No git dependencies; registry checksums for everything.

**Cons:**
- Renames ripple through three crates and their consumers; publishing
  adds release process for code that is upstream's.
- Unnecessary: nothing in the chain is published to crates.io.

### Option D: Upstream the feature

Revive rusqlite PR #1726.

**Cons:**
- Declined by the maintainers with a policy reason ("cannot bundle all
  the SQLite forks"); not in Kupferdraht's hands.

## Decision

Option B. The three forks, consumed by tag, with the identical source
spec at every site:

| Fork | Base | Branch, tag | Change |
|---|---|---|---|
| github.com/SchwarzDigits/rusqlite | v0.37.0 | `sqlite3mc/0.37`, `v0.37.0-sqlite3mc-2.5.1` | `libsqlite3-sys`: `bundled` compiles the SQLite3MC amalgamation in `libsqlite3-sys/sqlite3mc/` (copied from `vendor/sqlite3mc/` with its `LICENSE` and `PROVENANCE.txt`) with upstream's flag set plus `SQLITE_TEMP_STORE=2`; SQLCipher features refused; on `wasm32-unknown-unknown` the crate re-exports `sqlite-wasm-rs` (the former stand-in's wasm arm); upstream's crate root unchanged in `src/native.rs` |
| github.com/SchwarzDigits/refinery | v0.9.1 | `sqlite3mc/0.9`, `v0.9.1-sqlite3mc` | `refinery-core` and the examples take `rusqlite` from the rusqlite fork by tag |
| github.com/SchwarzDigits/openmls | openmls-v0.9.0 | `sqlite3mc/sqlite_storage-0.3`, `sqlite_storage-v0.3.0-sqlite3mc` | `sqlite_storage` takes `rusqlite` and `refinery` from the forks by tag and `openmls_traits = "0.6.0"` from crates.io instead of the workspace path, so a consumer on the crates.io openmls 0.9 line holds the trait crate once |

On this rusqlite fork `bundled` means SQLite3MC, always; there is no
feature a consumer could leave off. Versions stay upstream's, so the
lockfile reads rusqlite 0.37.0 and libsqlite3-sys 0.35.0 with git
sources. `vendor/sqlite3mc/` stays in Kupferdraht as the provenance pin
(signed release sums) and as the source `sqlite-wasm-rs` compiles for the
browser; a test reads the fork's copy off the resolved checkout and
asserts it byte-identical. No `[patch]` anywhere in the workspace; a test
pins that, the six lock sources and the single rusqlite of the graph.

The rationale that carries it: the engine of an encrypted store must not
depend on a line in somebody else's manifest. Git dependencies are the
one mechanism Cargo inherits that does not require publishing, and the
forks are upstream's code with manifest changes, rebased per release.

## Consequences

### Positive

- A Rust consumer integrates with one dependency line; the trap of
  2026-10-31 cannot recur, and the one way to end up with two SQLites is
  a build error.
- The build glue is three readable manifests and one `build.rs` block
  instead of a reduced copy of a crate in `vendor/`.
- The ORT exclusion for the stand-in and the workspace `exclude` go.

### Negative

- Three branches to rebase and retag per bump (rusqlite ships eight
  releases in sixteen months; Kupferdraht takes one every few). The
  recipe is in `vendor/README.md` and each fork's README.
- The openmls fork carries a downstream branch beside the upstream PR
  heads; its branch policy is to be agreed with the fork's other user.
- A git tag is immutable by convention only; the committed `Cargo.lock`
  pins the commit, and the provenance of these crates is a commit hash,
  not a registry checksum.
- An application that keeps its own SQLite beside Kupferdraht must take
  rusqlite from the fork at Kupferdraht's tag.

### Neutral

- Same engine, same amalgamation bytes, same versions, same at-rest
  format; nothing changes at runtime.
- Option A's link-time guard is not added: with the forks, vanilla SQLite
  is unreachable without a `[patch]` of the consumer's own making.
- cargo-about lists the forks like any crate; THIRD-PARTY-NOTICES names
  the fork beside the upstream repository.

## Implementation Notes

- Every site that names a fork uses the same `git` URL and the same
  `tag` — Cargo treats another tag or a `rev` of the same URL as a
  different source, which would be a second rusqlite.
- Bump order: rusqlite fork (rebase, tag) → refinery and openmls branches
  (their manifests name the rusqlite tag; rebase, tag) →
  `kupferdraht-core/Cargo.toml` (three tags) → gate. An SQLite3MC release
  replaces `vendor/sqlite3mc/` and the fork's copy in the same bump.
- Kupferdraht stays on rusqlite 0.37 until `openmls_sqlite_storage`
  releases on a newer line (openmls `main` is on 0.40); a rusqlite ≥ 0.38
  can retire the fork's wasm arm for its own `ffi-sqlite-wasm-rs`.
- Pins: `kupferdraht-core/tests/step_90_sqlite3mc_engine.rs`
  (`lockfile_resolves_the_sqlite3mc_line_to_the_forks`,
  `forks_amalgamation_matches_the_provenance_record`),
  `step_91_wasm_build_glue.rs` (`the_forks_sys_crate_carries_the_wasm_arm`),
  `deny.toml` `allow-git`.
- Consumer documentation: `docs/library.md` "Consuming from Rust".

## References

- Builds on ADR-0021 "Adopt SQLite3MC with ChaCha20-Poly1305 as the
  whole-DB encryption engine" — the engine; this ADR replaces only its
  build glue (the `[patch.crates-io]` stand-in).
- Builds on ADR-0022 "Persist browser data in SQLite3MC over OPFS" — the
  wasm arm moves from the stand-in into the fork's sys crate, unchanged.
- Constrained by ADR-0035 "Store an account's browser databases on
  sqlite-remote-vfs" — the precedent for a SchwarzDigits git dependency
  in `deny.toml`.
- Related: ADR-0016 (GPL-3.0-only; the forks stay MIT / MIT OR Apache-2.0).
- rusqlite PR #1726 "Added sqlite3mc (bundled) support", declined
  2025-08-23. SQLite3 Multiple Ciphers, "Architecture overview": the
  amalgamation patches. Step 127, `docs/plan/archive/127-sqlite3mc-through-forks.md`.

## Revision History

- 2026-10-31: Created and accepted (Volker) — Step 127
