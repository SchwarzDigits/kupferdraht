# Step 96 — A missing key for an existing database is an error, never a new key

**Status: MERGED 2026-09-28** — commissioned 2026-09-27 (Markus — blocker
B5 of the production-readiness review, `open.md` §71), owner OK
2026-09-28 (Volker). `785df54` (ADR-0029 + rule), `13b6736`; review fix
`a8019cb`. Merged to `main` as PR #60 (`89092e6`); CI green. Smoke
2026-09-27 against schwarz-dev-wire (release CLI). Scope shipped: an
existing database without a key is `KeyUnavailable`, never a new key;
the `SecureKeyStore` contract requires `Err` for a locked store. B4 (the
Linux Secret Service backend) stays open in `open.md` §71. Archived with
this header.

## Problem (verified in code 2026-09-27)

All three databases (`global.db`, `kupferdraht.db`, `mls.db`) get their key
through `storage::load_or_create_key_locked` → `adopt_or_create_key`.
When the keystore returns no key under the home-scoped id and none
under the legacy id, a fresh key is generated and stored —
**also when the database file already exists.** That database was
written under a key the keystore no longer returns, so:

- the fresh key cannot open it — the account is unusable on this
  device, and the real key, should it come back, has been overwritten;
- a keystore that answers "not stored" while it is merely **locked or
  unavailable** turns a transient state into permanent data loss
  (iOS Keychain before the first unlock after a reboot, a host keystore
  whose backing service is not up yet, the Linux kernel keyring after a
  reboot — B4);
- the `SecureKeyStore` contract does not say which answer a locked or
  unavailable store must give — `get_secret` documents only "`None` if
  nothing is stored".

## Decisions (Markus, 2026-09-27)

- **D1 — Never create a key for a database that exists.** Inside the
  locked critical section, an existing database without a key (neither
  id) is an error. A key is generated only for a database file that
  does not exist yet.
- **D2 — A dedicated error: `KupferdrahtError::KeyUnavailable(String)`**
  (the message names the database). The app's reaction differs from a
  storage fault: retry later (after unlock), or — if the key is gone for
  good — discard the local account and log in again. A new variant is a
  source-breaking change for exhaustive `switch`/`when` in Swift and
  Kotlin (`BREAKING:` in the changelog).
- **D3 — The host keystore contract.** `get_secret` returns `Err` when
  the store is locked or unavailable; `None` only when the id has
  definitely never been stored (or was deleted). Documented on the trait
  and in `docs/bindings.md`, with the platform advice: iOS
  `kSecAttrAccessibleAfterFirstUnlock(ThisDeviceOnly)`; Android Keystore
  without user-authentication binding; browser — IndexedDB is not
  lockable.
- **D4 — Recovery is not in this step.** Discarding a local account
  whose key is gone for good is Step 78 (local account purge). Until
  then `KeyUnavailable` documents the manual path (remove the account
  directory, log in again as a new device).
- **ADR-0029.** D1 + D3 define a contract every host keystore relies
  on — ADR-0029 ("a database key is created once, never replaced"),
  written with `new-adr` before the red tests.

## Scope

- `storage/mod.rs::adopt_or_create_key`: existing database + no key →
  `KeyUnavailable`; the legacy adoption (ADR-0025) stays.
- `KupferdrahtError::KeyUnavailable` + FFI (UniFFI error enum), wasm
  mapping, `kupferdraht-tabs` error serialisation if it enumerates variants.
- Trait docs (`keystore/mod.rs`), `docs/bindings.md`, `docs/library.md`,
  THREAT_MODEL (the keystore paragraph), CHANGELOG (`BREAKING:`).
- CLI: a readable message for `KeyUnavailable` in `status` / `sync`.

## TDD (sketch)

- a database exists, the keystore has no key → `KeyUnavailable`, and
  the keystore still has no key afterwards (nothing was written);
- a keystore whose `get_secret` returns `Err` (locked) → the error
  surfaces, nothing is written;
- no database, no key → a key is created (first login unchanged);
- a database with a legacy-id key → adopted (ADR-0025 unchanged);
- per database: `global.db`, `kupferdraht.db`, `mls.db`.

## As built (2026-09-27)

- `storage/mod.rs`: `adopt_or_create_key` generates only when the
  database does not exist; `adopt_legacy_key` adopts or refuses with
  `KeyUnavailable` (payload: database path + keystore id + the two ways
  out). `KupferdrahtError::KeyUnavailable(String)`; the UniFFI error enum
  carries it without extra code, wasm and `kupferdraht-tabs` pass errors as
  text.
- The `get_secret` contract is documented on the trait,
  `docs/bindings.md`, `docs/library.md` and THREAT_MODEL.
- CLI: exit code **4** for `KeyUnavailable` (not in the plan above —
  scripts can tell "retry after unlock" from a failure, as with
  `AccountLocked` = 3).
- **Test harness change:** the Step-85 child process parked inside the
  account lock at its first `set_secret` — reached, for a home whose
  database existed, only through the key replacement this step
  forbids. It now parks at the re-read under the lock
  (`GatedKeyStore::on_second_read`).
- TDD: `tests/step_96_no_silent_key_replacement.rs` — 4 red → green, 1
  pin (contract-following keystore). Gate 2026-09-27: 942 passed.
- Smoke 2026-09-27 (release CLI, dev keystore of smoke home 1): the
  account key hidden → `status` exits 4 with the message, no key file
  written; restored (byte-identical) → `status` and `sync` work.
- Review fix-ups (2026-09-28): the way out in the `KeyUnavailable` payload
  is per database (`global.db` → the home's registry file, the account
  databases → the account directory; the earlier text sent every case to
  the account directory); `tests/step_96_no_silent_key_replacement_wasm.rs`
  pins the refusal and the unchanged first run over OPFS (added to
  `scripts/wasm-test.sh`); the BREAKING note covers Rust consumers matching
  `KupferdrahtError` exhaustively.

## Out of scope

- The Linux Secret Service backend (B4, deferred).
- Discarding a local account (Step 78).
