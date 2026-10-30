# ADR-0041: Rename Sodium to Kupferdraht as a clean cut

**Status:** Accepted
**Date:** 2026-10-07
**Deciders:** Volker Zöpfel

## Context

The project was named Sodium. The name is heavily taken: the library built
as `libsodium.so` / `libsodium.dylib` / `libsodium.a`, the exact file name
of the NaCl library that Homebrew installs and many Android apps bundle.
A consumer app carrying both would hold two `libsodium.so` in one process,
and which one the loader resolves depends on the search path. `kupferdraht`
is free on crates.io, npm and GitHub under `reklov`. The TUI in its own
repository follows as `kupferdraht-tui`; natrium keeps its name as a
product of its own.

The name sits in three layers. Branding and build identity: crate names,
`[lib] name`, the UniFFI namespace, Gradle namespaces, the Swift module,
the wasm-pack output, the CLI binary, `SODIUM_*` environment variables, the
log target. Consumer API: `Sodium`, `SodiumBuilder`, `SodiumError` and
their Kotlin, Swift and TypeScript projections. Persisted identifiers: the
HKDF labels `sodium/v1/…`, `sodium.db` and `sodium.lock`, `~/.sodium`, the
keystore id prefix `sodium.`, the keyring service, the browser's OPFS base
directory, Web Locks lease and home-id entry, and the cookie label.
ADR-0025 and ADR-0035 declare the labels frozen: changing one orphans every
database keyed under it and every sealed recovery seed.

Constraints: version 0.1.0, `publish = false`, no release shipped. The
known installs are the author's development homes, smoke homes, natrium
test builds and browser storages on test instances. Shipped migrations are
immutable (refinery checksums, `migration_immutability`), and committed
binary fixtures carry content from before the rename.

## Options Considered

### Option A: Clean cut

Rename every layer, persisted identifiers and HKDF labels included. No
migration: an existing home, browser storage or keystore is wiped and
logged in afresh; devices register anew.

**Pros:**
- One name everywhere; no legacy string to explain in the storage layer
  or the keychain.
- The storage format is reset at the only moment this is cheap.
- Five known-answer vectors are recomputed once, independently.

**Cons:**
- Every existing install is unreadable; devices re-register on Wire.
- The "frozen" promise of ADR-0025 and ADR-0035 is broken once.
- BREAKING for natrium and the TUI.

### Option B: Compatible rename

Rename branding, build identity and the consumer API only. The
storage-format strings keep `sodium…` as frozen v1 constants, collected
in one module and documented.

**Pros:**
- Existing installs keep working; no re-login, no device churn.

**Cons:**
- The old name lives on forever in file names, keystore ids and the
  keychain of a project called Kupferdraht.
- Two names to explain to every future reader; the grep-cleanliness the
  charter relies on is gone.

## Decision

Option A. Before a release is the one moment a storage-format reset costs
nothing but a re-login of the author's own installs, and the libsodium
collision makes the rename itself necessary rather than cosmetic.

Four exceptions, each deliberate: shipped migration files keep their
bytes (nine comments mention Sodium) because refinery checksums and the
committed fixtures pin them; the step-91 fixture keeps its marker token
for the same reason; ADR-0006 names the real libsodium; the SQLite3MC
amalgamation is upstream.

The keystore id prefix grows from seven to twelve bytes, so the dev
`FileKeyStore`'s file-name budget caps the account domain at 153 bytes
instead of 158 (ADR-0025, revision entry).

The existing GitHub repository is renamed, not replaced: its history is
the provenance record of the independent implementation that
ADR-0016 rests on. The branch `archive/sodium`
marks the last state under the old name.

## Consequences

### Positive

- `libkupferdraht.*` cannot collide with the NaCl library.
- Consumers see one consistent name: `uniffi.kupferdraht`, the Swift
  module `Kupferdraht`, `kupferdraht.js`, `KUPFERDRAHT_*`.
- No legacy branch in key derivation or storage paths.

### Negative

- BREAKING for natrium (Gradle path, `buildSodiumNatives` tasks,
  `uniffi.sodium` imports, `import Sodium`, `libsodium.*`) and for the
  TUI, renamed to `kupferdraht-tui` in its own repository.
- Every development home, smoke home, browser storage and keystore is
  wiped; registered devices are replaced.
- Devices registered as `Sodium (…)` are no longer recognised by the
  model check of ADR-0040, so the renamed build never retires them;
  stale devices on test accounts go by hand.
- The labels were renamed once; after the first release they are frozen
  for good.
- Nine migration comments and one fixture token keep the old name.

### Neutral

- GitHub redirects `reklov/sodium` to `reklov/kupferdraht` until someone
  recreates the old name.
- Git history keeps the old name; CHANGELOG entries before this one were
  renamed with the rest of the prose.

## Implementation Notes

- Structural renames by `git mv`: the three crates,
  `bindings/web/kupferdraht-tabs`, the test files, the rule/ADR pairs 0016
  and 0023 (same file name on both sides).
- Mechanical rename of three case variants over every tracked text file
  except `vendor/` upstream sources, `reference/` and `Cargo.lock`; the
  whole procedure is a script, re-run over every pull request that lands
  before the rename does. A colleague's open branch takes the same
  mechanical rename first, then rebases.
- Known-answer vectors recomputed with an independent HKDF-SHA-256
  implementation that first reproduced all five old vectors.
- Follow-ups outside the repository: GitHub rename and
  `git remote set-url`, working directory move, natrium, the TUI.

## References

- Builds on ADR-0025 (keystore id scheme and prefix; the domain cap is
  amended by a revision entry), ADR-0023 (the lease `kupferdraht:instance`),
  ADR-0024 (`kupferdraht.lock`), ADR-0026 (`kupferdraht-tabs`,
  `kupferdraht.js`), ADR-0040 (the device model `Kupferdraht (Web)`).
- Constrained by ADR-0035: its frozen HKDF labels forced the choice
  between a clean cut and a compatible rename; the freeze resumes after
  this one rename (revision entry there).
- Related: ADR-0009 (naming model), ADR-0021 (the raw database key under
  the renamed label), ADR-0029 (key creation under the renamed ids),
  ADR-0016 (file renamed with the project), ADR-0006 (keeps the one
  mention of the real libsodium).

## Revision History

- 2026-10-07: Created
- 2026-10-07: Accepted (Volker, after the draft and the relations were reviewed in the rename session).
- 2026-10-07: Wording — "clean-room provenance" becomes "provenance record
  of the independent implementation"; ADR-0016's entry of the same day
  says why. Decision untouched.
