# Kupferdraht Threat Model

**Version:** 0.6 (Draft) **Author:** Volker Zöpfel **Date:** 2026-10-08 **Status:** working draft, for review by Markus Troßbach

------

## Purpose

Kupferdraht is a Rust library for MLS-based end-to-end encrypted group communication, designed as an independent alternative to Wire's Kotlin Multiplatform SDK (Kalium). This document describes what Kupferdraht guarantees under which assumptions, what it explicitly does not guarantee, and which attack scenarios the design addresses.

The focus is the shipped state (PLAN.md; Steps 1–92 merged as of 2026-09-25): the document describes what is built, not what was planned. Long-term architectural questions are collected in the appendix and do not block implementation.

Audience: developers on the Kupferdraht team, reviewers (BSI context), product teams that build systems such as CivicSeal or Natrium on Kupferdraht.

------

## What Kupferdraht is, and what it is not

Kupferdraht is a Rust library that generates Kotlin and Swift bindings through Mozilla UniFFI 0.29; the web bindings are a hand-written wasm-bindgen surface (ADR-0015). The cryptographic core builds on [openmls](https://github.com/openmls/openmls) 0.9 (Apache 2.0, crates.io) for MLS (RFC 9420). The Proteus prekeys Wire's registration requires come from an own, GPL-free module, generated once (Ed25519, Wire-exact CBOR); they are uploaded and forgotten, never used to encrypt, and proteus itself is no longer a dependency (ADR-0010). For the history see ADR-0001 (`docs/adrs/0001-use-openmls-and-proteus-directly.md`).

Like Kalium, Kupferdraht provides the fundamental application logic for secure group communication as functionality: session management, conversations, members, messages, event streams, join links. That goes beyond bare crypto primitives; it is what an embedding product needs to build communication without having to understand MLS details itself.

What Kupferdraht does *not* do: transport messages (the consumer's HTTP client does), manage users (the embedding system's identity provider does), or model product business logic (cases, vault, delivery receipts, deadlines, roles all live in CivicSeal or comparable layers on top, not in Kupferdraht).

Kupferdraht is therefore exclusively a client library. The server-side **Dehydrated Device** once planned was dropped on 2026-09-25 (PLAN.md Step 13); the web-login and recovery need it was meant to cover is being met differently, and the shape will be recorded here once it is decided.

### No crypto/logic split, deliberately

Kupferdraht differs from Wire's architecture in one deliberate point: there is no separation between the crypto core and the application logic of the kind Wire draws between Core Crypto and Kalium. At Wire an FFI boundary runs there: Core Crypto in Rust, Kalium in Kotlin. Data is serialised at that boundary, handed over, deserialised.

That separation has security costs: plaintext has to cross the FFI boundary, because Kalium knows the application logic and Core Crypto the primitives. Type information is lost. Lifecycle management (zeroization, ownership) becomes a matter of convention instead of a compiler guarantee.

Kupferdraht keeps application logic and crypto primitives in the same Rust crate. Plaintext comes into being in the Rust heap, is processed, encrypted and zeroized there, with no detour across a language boundary. UniFFI generates the bindings for Kotlin, Swift and WASM, but the consumer boundary lies *outside* the application logic, not in the middle of it. Zeroization is thus a compiler guarantee (through Rust `Drop` and `zeroize`), not a convention.

This is not architectural dogma but a security-relevant decision.

### Web as WASM only

Kupferdraht ships for the web *exclusively* as a WASM artifact. There is no JavaScript port, and none will be developed; this is a hard architectural decision taken for security reasons.

Plain JavaScript in the browser would have direct access to the DOM and could be manipulated by every other script component on the same page. WASM runs in a separate execution environment with its own memory, which JavaScript cannot read directly. This is no absolute protection: the hand-over between WASM and JavaScript across the UniFFI interface remains a boundary that plaintext necessarily crosses as soon as a message is to be displayed. But it raises the attack surface structurally and makes a *single* Rust code base across all platforms possible.

Embedding web consumers therefore need a build pipeline that loads WASM. Static HTML pages without a WASM runtime cannot use Kupferdraht.

------

## Trust Model

Three actors:

**The user** is the person whose identity and communication Kupferdraht protects. Trust in Kupferdraht comes from code review, reproducible builds and open-source inspection.

**The embedding product** (Natrium, the CivicSeal client, other integrators) calls Kupferdraht correctly: it keeps the API contract, handles plaintext responsibly after decryption, and does not serialise session handles into persistent storage.

**The server** is the counterpart in the Wire backend or a compatible MLS delivery infrastructure. Kupferdraht does *not* trust the server with message content. Kupferdraht trusts the server with administrative operations such as group-membership management; that relationship is formalised through MLS mechanisms and visible to every participant (see `wire/mls-deviations.md` W-5). **Limitation (ADR-0027):** under BasicCredentials the server is also the key directory: it distributes the KeyPackages with which devices are added to groups. A malicious server can therefore inject a device with an arbitrary identity (a "ghost device") that then reads along and sends MLS-authentically "as" that identity. This is the same trust model as official Wire without e2ei; the countermeasures are e2ei certificates (a non-goal) and comparing the MLS thumbprints per device. Since Step 95d Kupferdraht lists the devices of every room with their MLS thumbprint (comparable to the "MLS thumbprint" of the official apps) and reports every change in a room's devices against the last stored state, so an injected device becomes visible; it is not prevented. The statement "no trust for content" therefore holds only against a server that does not inject devices.

**Foreign identifiers are data, not paths.** Envelope ids (`message_id`), sender and user ids and asset identifiers are attacker-controlled input: a malicious peer or a compromised backend can choose them freely. No foreign string becomes a path fragment: checked at the dispatch boundary (path-unsafe ids are discarded like malformed envelopes), at every path site (`storage::is_path_safe_id`) and when the account identity is materialised from server responses (Step 80, 2026-09-11; LEARNINGS §5.23 "validate the sink").

------

## Security Goals

Under the assumptions of the following section, Kupferdraht guarantees:

**G1 — Pass MLS guarantees through correctly.** Forward secrecy (FS), post-compromise security (PCS), sender authentication and plausible deniability per RFC 9420. Deviations only for Wire compatibility (External Senders extension, identity format, capabilities list). Since Step 95a (ADR-0027) sender authentication means: every inbound application message is attributed to the leaf openmls authenticated as its signer, never to the envelope the server wrote (`qualified_from` / `from_client`, now a cross-check only). Exception until the X.509 follow-up step: signers with an e2ei X.509 credential are still attributed through the envelope and are thus not authenticated. What is authenticated is *which leaf* sent, not *whom* it belongs to (see Trust Model, server).

**G2 — Private key material does not leave the Kupferdraht process and is key-protected at rest.** Identity signing keys, KeyPackage private keys and MLS group state live as software keys in `mls.db`, whole-DB-encrypted (G3) under a key that lives exclusively in the platform keystore; without keystore access they cannot be read. Hardware-bound *signing* (Secure Enclave; the ciphersuite `0x0002`/P-256 was chosen for it, LEARNINGS §1.2) is a possible extension through an SE-backed `Signer`, not today's state; KEM/HPKE keys would stay in `mls.db` even then, because openmls decapsulates internally. Structurally, the API gives consumer code no raw private keys, only opaque handles.

**G3 — Non-key material is persisted encrypted at rest.** The entire local database is whole-DB-encrypted with SQLite3MC in the `chacha20` scheme (ChaCha20-Poly1305, RFC 8439: ChaCha20 for the page data, a 16-byte Poly1305 tag per page for integrity; ADR-0021). The database key is keystore-protected and does not leave the Kupferdraht process. Every database file (kupferdraht.db, mls.db, global.db) is isolated under its own key. **Accepted deviation:** ChaCha20-Poly1305 is not on the lists of BSI TR-02102-1 (the 2026-01 version recommends AES exclusively; §3.2 recommends no dedicated stream ciphers). The defence is the state of the art (RFC 8439, TLS 1.3, WireGuard), not a BSI listing; SQLite3MC's AES-based `sqlcipher` scheme stays compiled in as the documented fallback path (a new decision plus a database rewrite, not a consumer switch). This deviation was taken deliberately (ADR-0021, decider Volker Zöpfel, 2026-09-16) and must nowhere be rephrased as "TR-02102-compliant".

**G4 — Plausible zeroization.** At session end, on leaving a group or on restore, Kupferdraht zeroizes key material in memory (Rust `zeroize`). The local database is whole-DB-encrypted (G3), so freed pages leave no plaintext behind; a separate `VACUUM` is not needed for that. Forward secrecy applies in the protocol *and* in local storage.

**G5 — Resistance to API misuse.** The public API is type-safe against common crypto and application-logic mistakes. Conversation handles cannot be mixed up between sessions by accident. MLS delivery and user acknowledgement are separate types (`Submitted` vs. `Acknowledged`) so that a consumer cannot confuse them: receipt by a server or a queue is never evidence of user perception.

**G6 — The plaintext lifecycle stays in the Rust heap.** Plaintext messages come into being, are processed, encrypted and zeroized inside a single Rust runtime. There is no internal FFI boundary plaintext would have to cross. (The consumer boundary lies outside the application logic; as soon as a decrypted message is returned to the consumer, A6 applies.)

**G7 — Recovery is named honestly.** Today Kupferdraht provides **no** recovery path for identity loss: the mnemonic backup (Step 12) waits for an endpoint in Wire's API (observations §62), the server-side Dehydrated Device was dropped (2026-09-25). Until a replacement is decided, N6 applies without restriction: losing the device means a new MLS identity without history (Wire's multi-device property). Once the replacement is settled this goal is reformulated, not silently set back to "two paths".

------

## Non-Goals

**N1 — Protection of a compromised device.** OS compromise (kernel access, keyloggers, forensic inspection in the unlocked state) cannot be intercepted by any library. Kupferdraht reduces the risk through keystore use but does not rule it out.

**N2 — Metadata protection.** The server sees `group_id`, `sender_id`, `timestamp` and message volume. Kupferdraht encrypts content, not routing information. A deliberate trade-off against queryability. An attacker with access to the server database or to local storage can reconstruct social graphs.

**N3 — Protection against a faulty consumer.** Kupferdraht trusts the embedding product. A consumer that writes plaintext to logs or persists session handles wrongly lies outside Kupferdraht's defence.

**N4 — Identity management.** Kupferdraht binds cryptographic identities to public keys, not to people. The binding public key ↔ person belongs to the identity provider (Wire ID as an interim, later the EUDI Wallet).

**N5 — Transport security.** TLS and certificate validation are the HTTP client's job. Kupferdraht assumes that the transport layer prevents network MITM.

**N6 — Passphrase recovery.** Without keystore access (or without the passphrase in passphrase mode), local data is cryptographically unrecoverable. Not a bug but design: any way around it would be a back door with the same effect.

**N7 — Protection against state actors with hardware access.** No library technique helps against kernel-level compromise or physical access to an unlocked device. Mitigation is operational (ephemeral sessions, auto-delete) and belongs at the product level.

**N8 — Protection against malicious code in the browser origin.** Kupferdraht runs in the WASM sandbox, but the surrounding web page is not under Kupferdraht's control. An XSS attack on the web consumer compromises plaintexts as soon as they are handed from the WASM boundary to the JavaScript side (for display in the DOM). Mitigation lies with the web consumer: Content Security Policy, Subresource Integrity, strict origin separation.

------

## Assumptions

The guarantees hold only under these assumptions:

**A1 — Process isolation by the OS.** The Android/iOS app sandbox, macOS/Linux user permissions and Windows user accounts separate processes reliably.

**A2 — Keystore integrity.** Android Keystore, iOS Keychain/Secure Enclave, macOS Keychain, Windows Credential Manager and WebCrypto non-extractable keys (in the WASM environment) are not compromised. Device unlock is enforced.

**A3 — Cryptography is implemented correctly.** openmls (MLS) and `curve25519-dalek`/`ed25519-dalek` (prekey generation, ADR-0010) are correct implementations of their primitives. Kupferdraht adds no crypto primitives of its own.

**A4 — Transport security prevents MITM.** TLS with certificate validation (ideally with pinning) is enabled in the HTTP client.

**A5 — The identity provider is trustworthy.** The IdP in use binds public keys authentically to user identities. Kupferdraht's authentication hangs directly on this assumption.

**A6 — The consumer keeps the API contract.** Plaintext is not logged after decryption, session handles are not serialised into persistent storage, error paths are handled.

------

## Keystore architecture

This is the central concrete architectural point for Step 4. Here Kupferdraht departs most clearly from Kalium.

### Categories of private key material

Three classes of secret material:

1. **Long-lived identity keys** (ECDSA-P256 signing), unique per user device, rotated only on re-registration.
2. **Medium-lived KeyPackage keys** (DHKEM-P256), consumed on joining a group, rotated on top-up.
3. **Short-lived MLS epoch secrets**, rotating with every commit, forward-secure within the protocol.

Plus non-key material: plaintext messages, group state (ratchet tree, pending proposals, epoch context), attachment metadata, contact caches.

### Platform mapping

- **Android**: AndroidKeyStore with hardware backing (TEE/StrongBox) for AES and NIST curves
- **iOS**: Keychain with Secure Enclave for P-256 and AES
- **macOS**: Keychain Services with hardware backing on Apple silicon
- **Web**: WebCrypto with `extractable: false`, persisted in IndexedDB (exclusively in the WASM environment); the databases themselves live since Step 91 as SQLite3MC files in the Origin Private File System (OPFS)
- **Windows (later)**: Credential Manager, optionally with DPAPI or TPM

### Keystore: no KEK wrapping any more

The platform keystore (`SecureKeyStore`) is a plain secret store: Kupferdraht stores the database key of the whole-DB encryption (G3) **directly** under an id; no KEK, no on-disk wrapping envelope (see `open.md` §53). On desktop the `keyring` crate backs it (Keychain / Credential Manager), on mobile and web a UniFFI callback keystore (iOS Keychain, Android Keystore, WebCrypto/IndexedDB). **Linux desktop is currently not covered:** there the `keyring` crate uses only the kernel keyring (keyutils), which lives in memory and is lost on reboot, after which the local databases can no longer be opened. Secret Service is deferred for now as `open.md` §71 B4. **A missing key is never replaced (ADR-0029):** if a database exists but the keystore returns no key, Kupferdraht reports `KeyUnavailable` and writes nothing. A merely locked keystore (iOS before the first unlock) thus no longer destroys an account. Host keystores must return an error in the locked state, not "not present".

Whether MLS signature keys should additionally sign hardware-bound (Secure Enclave) is a separate, still open question; the earlier "Curve25519 mismatch / KEK wrapping" rationale is obsolete (see `open.md` §54).

**Home id (Step 93, ADR-0025):** every keystore entry carries the id of the home (`kupferdraht.<home-id>.global-db`, `kupferdraht.<home-id>.account-<user>`, `kupferdraht.<home-id>.account-<user>-mls`), as does the Wire login label `kupferdraht-<home-id>`. The home id (`base/home.id`, 32 hex) is an **identifier, not a secret**: it lies in plaintext next to the databases, appears in the keystore ids and is known to the server as the cookie label; knowing it opens nothing without keystore access. What it must be is *stable*: a freshly generated id orphans every key of the home; a damaged file is a recoverable error (`KupferdrahtError::Storage`) and is never overwritten. The home-scoped ids are an isolation boundary between homes on a shared keystore (no shared database key, no race for the first key creation, no mutual eviction of refresh cookies), not an additional confidentiality boundary: whoever can read the keystore can still open everything.

### Encryption at rest

Kupferdraht encrypts every local SQLite database completely (whole-DB) with SQLite3MC (SQLite3 Multiple Ciphers) in the `chacha20` scheme and raw-key mode: ChaCha20-Poly1305 (sqleet format), a raw 32-byte key as the entire security parameter, no KDF, no salt, no HMAC subkey in the trust path (ADR-0021, replacing SQLCipher from ADR-0013). There is **no** field-level encryption any more; sensitive columns lie as plaintext *inside* the encrypted pages. Every database file has its own, independently generated key, which lives directly in the platform keystore (`keystore::SecureKeyStore`, exactly 32 bytes; a 48-byte `key ‖ salt` blob from the SQLCipher era is rejected, no migration path, ADR-0021 I3) and is zeroized in memory (G4). The first 32 bytes of the SQLite header stay plaintext (`PRAGMA plaintext_header_size = 32`, set before `PRAGMA key`; SQLite3MC expects cipher parameters before the key) so that iOS can identify a WAL database in the shared container; everything after that, page 1 included, is neither readable nor migratable without the key. The key is verified on open by a first read; a wrong key is a recoverable error (`KupferdrahtError::Storage`), never a panic. The linked engine is pinned at runtime (`sqlite3mc_version()`, the encrypted header as the check): a wrongly linked SQLite is a red test, not a plaintext file.

**BSI deviation (deliberate, ADR-0021):** ChaCha20-Poly1305 is not listed in BSI TR-02102-1 (2026-01), see G3. The AES-based `sqlcipher` scheme (AES-256-CBC, random page IV, HMAC-SHA512, encrypt-then-MAC, every primitive on the BSI lists) stays compiled into SQLite3MC and is the documented path should TR-02102 compliance become a requirement.

In the browser the same at-rest model applies since Step 91 (ADR-0022, replacing part 2 of ADR-0015): the same SQLite3MC amalgamation, the same raw key, the same files, in the Origin Private File System (OPFS) over the `sahpool` VFS, exclusively in a dedicated Web Worker; the database key stays in the WebCrypto/IndexedDB keystore. The IndexedDB record store with plaintext index keys (ADR-0015 part 2) no longer exists. Details in ADR-0021, ADR-0022 and `docs/library.md`.

------

## Auth modes

Three modes in the API, distinguished at the type level:

**DeviceBound**: the default. Keys protected by the keystore only. No user passphrase. The user is signed in as long as they pass device unlock. Corresponds to Signal/Wire today.

**PasswordProtected**: keystore *plus* user passphrase (Argon2id). The passphrase-backed `SecureKeyStore` releases the key material only after the passphrase is entered. Session timeout and auto-lock are first class. The strongest mode against OS compromise: an attacker with kernel access has the keystore side but not the passphrase side.

**PasswordOnly**: no keystore, only a passphrase-derived key. For web contexts without a persistent keystore API, or for users who do not trust the device. Hardware-independent, but weaker against brute force.

### Type-level distinction

The type-level contract against API misuse that Kalium structurally lacks lives in separate types for easily confused states, such as `Submitted` vs. `Acknowledged` for MLS delivery and user acknowledgement (G5). There is **no** `LockedSession`/`UnlockedSession` split: the store is opened ready (whole-DB-keyed), a decrypt needs no separate unlock step. An optional passphrase mode is a property of a passphrase-backed keystore, not a client state.

The three auth modes stay distinguished at the type level (`DeviceBound` complete, `PasswordProtected` / `PasswordOnly` as interface stubs) so that a backup path can dock on later without breaking changes.

------

## Key backup (Step 12, architectural preview — blocked)

A mnemonic-based key backup is the intended recovery path. Its implementation is Step 12 and blocked as long as Wire v14 offers no backup endpoints (observations §62); the fundamental decisions below continue to apply so that the API shape stays compatible.

**A separate backup passphrase.** Not the account passphrase: changing the account passphrase must not render the backup worthless. BIP39-style mnemonic (12 words, 128 bits of entropy) plus an encrypted binary container.

**What is restored:** identity keys and the current KeyPackage keys. The user can sign in as the same user and is recognised by others. Group state is *not* restored; groups are rejoined via external commit.

**No way around the passphrase.** No server backup, no social recovery, no admin override. Any way around it would be a back door.

Rough API shape:

```rust
pub struct BackupPassphrase([String; 12]);

impl Client {
    pub fn export_key_backup(&self, p: &BackupPassphrase) -> Result<BackupBlob, Error>;
}

pub fn restore_key_backup(blob: &BackupBlob, p: &BackupPassphrase)
    -> Result<Arc<Client>, Error>;
```

------

## Dehydrated Device — dropped (2026-09-25)

Up to version 0.3 this document foresaw a server-side **Dehydrated Device** (DD) as a second recovery path and web-login enabler, with its own server trust profile, team deployment policy and enclave questions. Volker dropped the idea on 2026-09-25 (PLAN.md Step 13; prose in `docs/plan/archive/13-dehydrated-device.md`) because the need is being met differently. What remains: the separation of MLS delivery and user acknowledgement (`Submitted` vs. `Acknowledged`, G5), a general principle, not a DD artifact. The replacement is described here once it is decided.

------

## Known Limitations

**L1 — Metadata leak on storage inspection.** An attacker with storage access but without keystore access sees plaintext metadata. A deliberate trade-off against queryability.

**L2 — WASM protection ends at the JavaScript boundary.** The plaintext lifecycle is protected inside the WASM instance. As soon as a decrypted value is returned to the JavaScript side (which display requires), it lies in the JavaScript heap and is for the web consumer's architecture to protect. WASM offers no protection against XSS or compromised scripts in the same origin (see N8).

**L3 — License.** Kupferdraht itself is GPL-3.0-only (ADR-0016); the dependency graph is permissive (ADR-0010 replaced proteus with an own prekey module, `cargo-deny` enforces the allow-list). The earlier concern "proteus propagates GPL onto Kupferdraht" is thus moot: the GPL posture is a free decision, not a forced one.

**L4 — Operator abuse of the External Senders authority** (see `wire/mls-deviations.md` W-5). Visible (the extension list is public) but not preventable. Standard MLS behaviour.

**L5 — removed** (DD dropped 2026-09-25).

**L6 — Asset content authenticity inherited from MLS** (Step 11, ADR-0011). Asset ciphertext is encrypted with AES-256-CBC, an unauthenticated mode. The integrity of an asset relies on the SHA-256 hash carried inside `Asset.RemoteData.sha256`, which itself is delivered through an MLS-encrypted, MLS-authenticated `GenericMessage`. An attacker who can substitute an asset blob in transit cannot do so undetected: the receiver computes SHA-256 over the downloaded ciphertext and compares it against the hash, surfacing a mismatch as `KupferdrahtError::IntegrityCheck`. An attacker who can substitute the entire `GenericMessage` would need to forge MLS sender authentication, which is the same threat model that already protects every text message.

The construction is cryptographically sound at the protocol level, but it does inherit the MLS layer's properties rather than achieving authenticated encryption locally. A future Wire protocol revision to AES-GCM (or AES-CBC + HMAC-SHA256 in encrypt-then-MAC framing) would strengthen this further; Kupferdraht will adopt such a revision when Wire's clients standardise it. Until then, Kupferdraht emits only `AES_CBC` outbound; inbound `AES_GCM`-flagged assets are decoded but `download()` returns `InvalidInput` (warn-and-skip per ADR-0011 §AES-GCM toleration).

------

## Appendix: long-term questions

Not blocking for the current implementation phase. To be picked up before the affected steps or at product decisions.

**A1 / A2 — removed** (enclave strategy and policy negotiation belonged to the DD, dropped 2026-09-25).

**A3 — Post-EUDI-Wallet migration** (future): migrating existing MLS groups from Wire ID to an EUDI Wallet identity. Primarily an identity-provider topic; Kupferdraht must make the migration path possible.

**A4 — Mobile PWA WebCrypto** (future): which keystore path on mobile WebCrypto? Restrictions compared with desktop WebCrypto?

**A5 — Attachment handling** (V2): large binaries as AES-GCM blobs in SQLite are not ideal. Separate file storage with a per-attachment key is plausible but not specified. **Resolved 2026-05-05 by ADR-0011** — assets travel through Wire's `/v14/assets` REST endpoint AES-256-CBC-encrypted with a per-asset key, MLS reference carries `(otr_key, sha256, asset_id, asset_token, asset_domain)`. See L6 for the authenticity model. Implemented as PLAN.md Step 11.

------

## Versions

| Version | Date       | Change                                                       |
| ------- | ---------- | ------------------------------------------------------------ |
| 0.1     | 2026-04-21 | First draft                                                  |
| 0.2     | 2026-04-21 | Application logic made explicit; no crypto/logic split as a security decision; cut down to the implementation focus; long-term questions moved to the appendix |
| 0.3     | 2026-04-21 | Delivery semantics in the DD made precise (`DeliveryStatus` API, MLS delivery vs. application-receipt separation); web = WASM as a hard architectural decision; N8 (browser-origin attacks) added; L2 made precise at the WASM boundary; O1 removed (a leftover of the wrong DD model from v0.1) |
| 0.4     | 2026-09-25 | Step 82: brought to the shipped state — header (UniFFI 0.29, wasm-bindgen), G2 honest (software keys in `mls.db`, SE signing as an option), trust boundary for foreign identifiers (Step 80), Dehydrated Device dropped (G7 → "no recovery path today", DD/policy sections replaced by a stub, L5/A1/A2 removed), proteus references replaced by ADR-0010 (intro, A3, L3), key backup as Step 12 (blocked) |
| 0.5     | 2026-09-26 | Step 95a: G1 sender authentication on the MLS-authenticated leaf (ADR-0027, envelope as a cross-check only, X.509 exception named); Trust Model: server as key directory under BasicCredentials (ghost device) named |
| 0.6     | 2026-10-08 | Translated from German to English for publication; content unchanged except the positioning in Purpose ("independent alternative", 2026-10-07). The German original stays in the private history. |

------

*Changes, contradictions and clarifications are welcome.*
