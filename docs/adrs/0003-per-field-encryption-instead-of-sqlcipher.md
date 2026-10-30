# ADR-0003: Per-field AES-GCM encryption with DEK instead of SQLCipher

**Status:** Superseded by [ADR-0013](0013-sqlcipher-engine.md) (2026-06)
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel

> **Superseded.** This ADR chose per-field AES-GCM encryption over
> SQLCipher. ADR-0013 reverses that decision: Kupferdraht now encrypts the
> **whole** database with SQLCipher (AES-256-CBC + HMAC-SHA512), and the
> per-field DEK/KEK scheme described below has been removed from the
> code. The Context and trade-off analysis remain on record for the
> reasoning; the Decision no longer reflects the implementation.

---

## Context

Kupferdraht persists sensitive data locally: message contents, group state (ratchet trees, pending proposals, epoch context), conversation metadata, and contact caches. Persistent storage requires both a storage engine (for SQL queries, transactions, indexes) and an encryption scheme (to protect data at rest).

The widely-used approach for local-encrypted storage in secure messaging is **SQLCipher** — an extension of SQLite that encrypts every database page with AES-256-CBC + HMAC-SHA512. Wire's Kalium uses SQLCipher. Signal Desktop uses SQLCipher. WhatsApp Desktop uses SQLCipher. The Rust ecosystem supports it directly via `rusqlite` with the `bundled-sqlcipher` feature.

An alternative is **per-field encryption**: use plain SQLite as the storage engine, and have the application layer encrypt specific columns (content, metadata containing sensitive information) before writing, decrypting on read. Non-sensitive columns used for queries (`group_id`, `sender_id`, `timestamp`) stay plaintext, so normal SQL queries work without decryption.

Both approaches can meet the threat model's requirement that stored data is unreadable without keystore access. The question is which approach fits Kupferdraht's cross-platform requirements and operational needs better.

---

## Options Considered

### Option A: SQLCipher with a database-level encryption key

Standard industry approach. Database file is fully encrypted. Application treats SQLite as normal. The encryption key lives in the platform keystore and is passed to SQLCipher on database open via `PRAGMA key`.

- **Pro:** Proven industry pattern. Minimal new code on Kupferdraht's side.
- **Pro:** Everything is encrypted, including metadata — an attacker with raw database file access learns nothing about group structure or message timing.
- **Con:** SQLCipher has limited support on WASM targets. Emscripten ports exist but are not well-maintained; cross-platform support is fragile.
- **Con:** All-or-nothing encryption. Cannot inspect or query a field without decrypting it. Query performance requires SQLCipher to load and decrypt pages on demand.
- **Con:** Single key for the entire database. Rotating the key requires re-encrypting every page. Partial compromise (one user's data exposed) forces full rotation.
- **Con:** The database key itself becomes a single point of failure. If an attacker obtains it, they obtain everything.

### Option B: Per-field AES-256-GCM encryption with a Data Encryption Key (DEK)

Plain SQLite as storage engine. Sensitive columns are encrypted in the application layer with a DEK. Each encrypted field is stored as a triple: `<field>_ct` (ciphertext), `<field>_nonce` (per-row random nonce), `<field>_tag` (authentication tag). Query-relevant columns stay plaintext. The DEK itself is AES-wrapped by a KEK in the platform keystore.

- **Pro:** Works uniformly across all platforms, including WASM (plain SQLite via sql.js, WebCrypto for AES-GCM operations).
- **Pro:** Granular rotation. The DEK can be replaced without touching the KEK or vice versa. Compromise recovery is more flexible.
- **Pro:** Query columns remain plaintext, so normal SQL queries (e.g., "messages in group X ordered by timestamp") execute without decryption overhead.
- **Pro:** Forces explicit acknowledgement at the schema level of what is sensitive vs. what is metadata. The `_ct`/`_nonce`/`_tag` suffix is visible and reviewable.
- **Con:** More code to write and maintain on Kupferdraht's side. Every read and write of a sensitive field must go through an encryption helper.
- **Con:** Metadata (group_id, sender_id, timestamp) is stored plaintext. An attacker with raw database file access can reconstruct social graphs even without keystore access.
- **Con:** Larger storage footprint per row (three columns instead of one, plus nonce and tag overhead).

### Option C: Application-layer encryption on top of plain SQLite, encrypting full rows

A hybrid: use plain SQLite, but encrypt entire rows as opaque blobs. The application decrypts every row on read, serializes and encrypts on write.

- **Pro:** No plaintext metadata.
- **Con:** Breaks SQL queries entirely. Every query requires decrypting all rows to evaluate predicates.
- **Con:** Effectively reimplements SQLCipher badly.

---

## Decision

**We use per-field AES-256-GCM encryption with a DEK (Option B).**

The decisive factor is cross-platform uniformity. Kupferdraht must run identically on Android, iOS, macOS, Windows, Linux, and Web. SQLCipher's WASM support is fragile, and working around that would mean having a different storage architecture on Web than on native platforms — which is exactly what we wanted to avoid with the "one Rust crate" decision (ADR-0002).

The granular rotation benefit is secondary but real. Being able to rotate the DEK without rotating the KEK gives us more flexibility in response to partial compromises or routine key hygiene.

The metadata-leak trade-off (Option B's main weakness) is acknowledged and documented in `THREAT_MODEL.md` as limitation L1. For Kupferdraht's use case, preserving query capability is more valuable than metadata encryption — and any consumer who needs metadata encryption can overlay their own scheme on top of Kupferdraht's storage.

---

## Consequences

### Positive

- Uniform storage architecture across all six target platforms.
- DEK and KEK can be rotated independently.
- Query performance is unaffected by encryption — sensitive column decryption only happens when content is actually read.
- Clear separation at the schema level between what is sensitive (encrypted) and what is queryable metadata (plaintext).
- No bundled C dependency (SQLCipher) in the build. Faster build times, smaller binary.

### Negative

- More encryption/decryption code in Kupferdraht. Every read and write of a sensitive field goes through a helper that manages nonce generation and tag verification.
- Group IDs, sender IDs, and message timestamps are visible in the raw database file. An attacker who obtains the database file without the keystore can reconstruct the social graph and message timing, even though they cannot read content. This is documented as L1 in `THREAT_MODEL.md`.
- Slightly larger storage footprint — each encrypted field costs an extra 28 bytes (12-byte nonce + 16-byte tag).

### Neutral

- The MLS protocol state (stored via `openmls_sqlite_storage`) is encrypted as a single blob per group using this scheme, since openmls's internal structure does not benefit from field-level query access.

---

## Implementation Notes

Schema convention: sensitive fields use three columns — `<field>_ct`, `<field>_nonce`, `<field>_tag`. Non-sensitive metadata columns remain plaintext for SQL query use. The convention is applied consistently across all Kupferdraht-owned tables.

The DEK is 32 bytes, wrapped by the KEK, stored in the keystore. On session start it is unwrapped into a `Zeroizing<[u8; 32]>` buffer held in memory. On session end the buffer is zeroized (automatically via `Drop`) and `VACUUM` is run to overwrite freed database pages.

Full details in `secure-storage.md`.

---

## References

- `secure-storage.md` — implementation reference that operationalizes this decision
- `THREAT_MODEL.md` — goals G2 (key material in keystore), G3 (non-key material field-encrypted), G4 (zeroization), and limitation L1 (metadata leak)
- ADR-0002 — single-Rust-crate architecture that makes uniform cross-platform storage necessary

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
