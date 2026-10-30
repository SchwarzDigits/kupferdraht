# ADR-0029: Create a database key once — never replace a missing one

**Status:** Accepted
**Date:** 2026-09-27
**Deciders:** Markus Troßbach

## Context

Every Kupferdraht home has three whole-DB-encrypted databases (ADR-0021):
`global.db` (the device's account registry), and per account
`kupferdraht.db` (session, rooms, timeline, tasks, …) and `mls.db` (the
openmls state). Each is opened with its own 32-byte key, stored
directly in the platform keystore — no KEK, no on-disk wrapping — so a
database is readable only with exactly the key it was created with.
All three keys are resolved in one place: `load_or_create_key_locked`
→ `adopt_or_create_key`, inside the per-account lock (ADR-0024), with
the legacy-id adoption of ADR-0025.

When the keystore returns no key under the home-scoped id and none
under the legacy id, that code generates and stores a fresh key — also
when the database file already exists. The fresh key cannot open that
database, and should the real key reappear, it has been overwritten:
the account is permanently unusable on the device (session, history,
MLS groups). Keystores can report "not stored" when they are merely
locked or unavailable: the iOS Keychain before the first unlock after
a reboot, a host keystore whose backing service is not up yet, the
Linux kernel keyring after a reboot (open.md §71 B4). The
`SecureKeyStore` trait does not say which answer a locked store must
give — `get_secret` documents only "`None` if nothing is stored".

The production-readiness review of 2026-09-25 lists this as blocker
B5 (open.md §71). Recovery of an account whose key is gone for good —
discarding the local databases — is Step 78's local account purge.

## Options Considered

### Option A: Keep generating a key when none is found

The current behaviour.

**Pros:**
- No API change; a first run and a lost key take the same path.

**Cons:**
- A transiently missing key becomes permanent data loss, silently.
- The overwritten entry destroys the only way back if the keystore
  recovers.
- The app cannot tell the user what happened.

### Option B: Refuse with the existing `KupferdrahtError::Storage`

An existing database without a key fails as a storage error.

**Pros:**
- No data loss; nothing is overwritten.
- Not a breaking change — no new error variant.

**Cons:**
- The app cannot tell "retry after unlock" (keystore not ready) from a
  damaged database — both are `Storage`.

### Option C: Refuse with a dedicated `KupferdrahtError::KeyUnavailable`

An existing database without a key fails with its own variant that
names the database.

**Pros:**
- No data loss; nothing is overwritten.
- The app can react specifically: retry later, or offer to discard the
  local account when the key is gone for good.

**Cons:**
- A new error variant is source-breaking for exhaustive `switch` /
  `when` in Swift and Kotlin.

## Decision

**Option C.**

1. **A key is created only for a database file that does not exist
   yet.** Inside the locked critical section, an existing database
   with no key under either id (home-scoped or legacy) fails with
   `KupferdrahtError::KeyUnavailable`, naming the database; the keystore is
   not written. The legacy adoption of ADR-0025 is unchanged.
2. **The host keystore contract:** `SecureKeyStore::get_secret`
   returns `Err` when the store is locked or unavailable, and `None`
   only when the id was never stored or was deleted. Documented on the
   trait and in `docs/bindings.md`, with platform advice: iOS items
   with `kSecAttrAccessibleAfterFirstUnlock(ThisDeviceOnly)`; Android
   Keystore keys without user-authentication binding; the browser's
   IndexedDB keystore cannot be locked.
3. **Recovery is out of scope:** discarding a local account whose key
   is gone for good is Step 78. Until then the error text names the
   manual path (remove the account directory, log in again as a new
   device).

The deciding reason for C over B: the app's correct reaction depends
on which of the two it is, and only a dedicated variant lets it
choose.

## Consequences

### Positive

- A locked or not-yet-ready keystore no longer destroys an account —
  the next attempt after unlock succeeds.
- A key that is really gone surfaces as a named error instead of an
  unopenable database behind a misleading storage failure.
- One rule for all three databases and every target (native, wasm).

### Negative

- `BREAKING:` a new `KupferdrahtError` variant for FFI consumers with
  exhaustive matching.
- A key that is gone for good leaves the account stuck until Step 78
  ships a purge; until then the user removes the directory by hand.
- Host keystores that answer `None` while locked still fail — now
  visibly, but the app must implement the contract to benefit from
  "retry after unlock".

### Neutral

- A first run (no database, no key) is unchanged.
- The browser's fire-and-forget key persistence (open.md §71 H7) can
  still lose a fresh key; it now surfaces as `KeyUnavailable` instead
  of a silently replaced key.

## Implementation Notes

- The check sits in `storage/mod.rs::adopt_or_create_key`, after the
  legacy adoption and before `load_or_create_key` generates.
- Mirror the variant in the UniFFI error enum, the wasm error mapping
  and `kupferdraht-tabs` if it enumerates variants; the CLI prints the
  manual recovery path.

## References

- Builds on ADR-0024 — the decision lives inside its per-account
  critical section around first key creation.
- Builds on ADR-0025 — keeps its home-scoped ids and the legacy
  adoption (which already requires the database to exist).
- Constrained by ADR-0021 — the key opens the database directly (no
  KEK), so a lost key cannot be recovered and must never be replaced.
- Related: ADR-0022 / ADR-0023 (browser storage and keystore).
- `docs/plan/96-no-silent-key-replacement.md`; open.md §71 B5.

## Revision History

- 2026-09-27: Created (Accepted — Markus, decisions D1–D4 of Step 96)
