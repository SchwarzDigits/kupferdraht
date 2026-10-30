# ADR-0002: No separation between cryptographic primitives and application logic

**Status:** Accepted
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel, reviewed by Markus Trossbach

---

## Context

A secure messaging library must decide where to draw the boundary between cryptographic primitives (MLS operations, key derivation, AEAD) and application logic (sessions, conversations, members, message handling). Two fundamentally different architectures are possible.

Wire's Kalium SDK draws this boundary at a language level. The cryptographic primitives live in a Rust library (Core Crypto) and the application logic lives in Kotlin (Kalium). The two communicate across a C FFI boundary: data is serialized on one side, passed as bytes, deserialized on the other. Judging by the repositories, this has historical reasons (Core Crypto predates Kalium) and some practical benefits (primitives can be updated independently of application code).

The same boundary could be drawn in Kupferdraht, or it could be avoided. Drawing it naturally would mean a thin Rust core exposing only primitives, with application logic in per-platform host languages (Kotlin, Swift, TypeScript). Avoiding it means keeping everything in a single Rust crate and exposing only the final application API to host languages via UniFFI.

The security-relevant observation: a language-level split forces plaintext to cross the FFI boundary. When Kalium encrypts an outgoing message, it must have the plaintext in the Kotlin heap before handing it to Core Crypto. When Kalium decrypts an incoming message, the plaintext returns to the Kotlin heap after Core Crypto processes it. The lifetime of sensitive data in the Kotlin heap is governed by JVM garbage collection, which provides no zeroization guarantees. Heap dumps, crash reports, and memory inspection tools can expose residual plaintext long after the application thinks it has been discarded.

The same data, if it never left the Rust side, could be bound to `Zeroizing` wrappers, cleared on `Drop`, and made Compiler-enforceable.

---

## Options Considered

### Option A: Follow the Wire architecture — Rust crypto core, host-language application logic

Kupferdraht would provide a thin Rust core with MLS primitives, and per-platform bindings (Kotlin, Swift, TypeScript for WASM) would each implement session, conversation, and message handling in their host language.

- **Pro:** Matches established industry pattern (Wire, Signal).
- **Pro:** Application logic can be written in the idiomatic style of each platform.
- **Con:** Plaintext crosses the FFI boundary on every encrypt and decrypt operation. Lifetime management in the host language cannot guarantee zeroization.
- **Con:** Application logic must be reimplemented per platform. Bugs are multiplied across implementations. Test coverage is tripled or quadrupled.
- **Con:** Type information is lost at the FFI boundary and reconstructed on both sides, often with subtle differences.

### Option B: Keep everything in a single Rust crate, expose only the application API via UniFFI

Kupferdraht would build both the cryptographic primitives and the application logic (sessions, conversations, members, messages, events) in Rust. UniFFI generates bindings for Kotlin, Swift, and WASM, but the consumer-facing API exposes only the final application concepts, not the primitives.

- **Pro:** Plaintext lifecycle stays entirely in the Rust heap. Zeroization is compile-time enforceable via `Zeroize` and `Drop`.
- **Pro:** One implementation of application logic, shared across all platforms. Bugs fixed once, tested once.
- **Pro:** Stronger type-level guarantees at the API surface. A locked session cannot be used to decrypt, because `LockedSession` and `UnlockedSession` are distinct types — the compiler enforces this.
- **Con:** Consumers cannot extend or customize application logic without modifying Kupferdraht itself. The API is less flexible than a primitives-only library.
- **Con:** The Rust crate becomes larger and requires domain understanding that a primitives-only library would not require.
- **Con:** Diverges from the pattern Wire uses, making future code-sharing with Wire harder.

---

## Decision

**We keep cryptographic primitives and application logic in a single Rust crate, exposing only the application API to host languages via UniFFI (Option B).**

The security benefit is the decisive factor. Messaging library users, including CivicSeal users communicating with government agencies, rely on the library's promise that plaintext has a bounded lifetime in memory. A language-level split makes that promise unprovable. A single-language implementation makes it a compile-time property.

The secondary benefit — single implementation of application logic across platforms — avoids a class of bugs that Kalium is structurally prone to. Wire has visibly struggled with platform-specific bugs where Android behavior diverges from iOS behavior in subtle ways. A shared Rust implementation removes that class of bug by construction.

The cost — less flexibility for consumers who want to customize application logic — is acceptable for Kupferdraht's target use case. Consumers like CivicSeal and Natrium wrap Kupferdraht's API into product-specific UX; they do not need to modify application logic at the library layer.

---

## Consequences

### Positive

- Plaintext lifecycle is bounded by the Rust runtime. `Zeroizing` wrappers and `Drop` implementations provide compiler-enforceable clearing of sensitive material.
- One shared implementation of session, conversation, member, and message handling across Android, iOS, macOS, Windows, Linux, and Web. Bugs are fixed once.
- Type-level API safety: `LockedSession` vs `UnlockedSession`, distinct message status types for MLS-level delivery vs application-level acknowledgement, distinct handle types for conversations belonging to different sessions. API misuse is a compile error, not a runtime bug.
- Consumer integration is simpler: import Kupferdraht, call the final API, no need to reimplement session handling.

### Negative

- Kupferdraht is not a primitives-only library. Consumers who want only MLS primitives (without Kupferdraht's session and conversation model) must use openmls directly.
- The Rust crate is larger than Wire's Core Crypto. Build times are longer, and the crate has more domain-specific code that must be maintained.
- Divergence from Wire's Kalium+Core Crypto architecture. Cross-project code sharing with Wire is limited to openmls and proteus.

### Neutral

- UniFFI still creates a language boundary between Kupferdraht (Rust) and the consumer (Kotlin, Swift, TypeScript). Plaintext that the consumer explicitly reads out of Kupferdraht — for displaying messages, for example — leaves the Rust heap at that point. This is outside Kupferdraht's protection scope (see `THREAT_MODEL.md` assumption A6).

---

## Implementation Notes

The Kupferdraht public API exposes application concepts (`Session`, `Conversation`, `Message`, observer callbacks) rather than MLS primitives. Consumers cannot directly call `mls_encrypt` or access a `RatchetTree`. These primitives exist inside Kupferdraht but are not part of the UniFFI-exported surface.

The one exception to "everything in Rust" is the `SecureKeyStore` trait, which is implemented on the host-language side (for access to Android Keystore, iOS Keychain, WebCrypto) and called back into Rust via UniFFI callback interface. This is a necessary exception because platform-specific secure key storage is not accessible from Rust directly. The callback only deals in opaque key handles, never in raw key material, so plaintext never crosses the boundary through this path.

---

## References

- `THREAT_MODEL.md` — Goal G6 (plaintext lifecycle in Rust heap) is the primary security goal this ADR supports
- `secure-storage.md` — describes how the `SecureKeyStore` callback boundary works in practice
- ADR-0001 — the openmls/proteus foundation this architecture is built on

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
