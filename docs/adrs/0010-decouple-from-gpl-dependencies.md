# ADR-0010: Decouple Kupferdraht from GPL dependency stack

**Status:** Accepted
**Date:** 2026-04-23
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht currently depends on three GPL-3.0 crates from the `wireapp/proteus` repository:

- `proteus-wasm v2.1.0`
- `proteus-traits v2.1.0`
- `cbor-codec v0.7.1`

These are the only GPL-3.0 dependencies in Kupferdraht's transitive graph. Every other dependency (254 crates at the time of this ADR) is under a permissive license: Apache-2.0, MIT, BSD, ISC, or similar. This includes all of Kupferdraht's cryptographic foundation — `openmls`, `openmls_*`, `curve25519-dalek`, `ed25519-dalek`, `x25519-dalek`, `aes-gcm`, `ring`, `rustls`, the RustCrypto stack, Mozilla's UniFFI, the Tokio ecosystem, and everything else.

The consequence of the three GPL crates is that **Kupferdraht as a distributed artifact is currently bound to GPL-3.0** — any downstream consumer who embeds Kupferdraht must also be GPL-3.0. This constrains Kupferdraht's licensing options before any strategic decision about its own license can even be made.

### Why this is a technical decision, not a license decision

The right license for Kupferdraht itself is a strategic question that depends on factors outside this ADR's scope: downstream consumer requirements, the broader Wire ecosystem position, the relationship between Kupferdraht and parallel projects like Natrium and CivicSeal.

The dependency on GPL crates, however, is a **technical constraint** that limits any future strategic decision. As long as the three Wire GPL crates are in the graph, Kupferdraht cannot be Apache-2.0 even if the strategic decision were to go that way. The technical constraint forecloses a strategic option.

This ADR therefore separates the two concerns:

- **Now:** Remove the technical constraint. Eliminate GPL from the dependency graph.
- **Later:** Make the strategic license decision in a separate ADR with full information.

By doing the decoupling work first, both options remain open. A future Apache-2.0 decision is feasible. A future decision to stay GPL-3.0 (or move to LGPL-3.0) is also feasible — and would be the result of strategic preference, not technical lock-in.

### Why Kupferdraht needs Proteus at all

The Wire backend requires every registered client to upload Proteus prekeys at `POST /clients`. This is enforced by the backend's contract, not by Kupferdraht's design. Even an MLS-only client cannot register without satisfying this requirement.

The critical observation: **Kupferdraht never uses Proteus for encryption**. Proteus prekeys are generated once, serialized in Wire's expected CBOR format, uploaded to the backend, and never referenced again. The Double Ratchet protocol that Proteus implements is never executed by Kupferdraht. All actual messaging goes through MLS via openmls.

This reduces the effective surface of "what Proteus does for Kupferdraht" to a small, well-defined set of operations:

- Curve25519 identity keypair generation
- Curve25519 prekey generation (a fixed number of ephemeral keypairs with an ID each)
- Ed25519 signing key generation and signing the identity key
- CBOR serialization in the exact byte-for-byte format the Wire backend expects

All of these operations can be implemented using crates already in Kupferdraht's dependency graph:

- `curve25519-dalek` (Apache-2.0/MIT) for Curve25519 operations
- `ed25519-dalek` (BSD-3-Clause) for Ed25519 operations
- `ciborium` or `serde_cbor` (Apache-2.0/MIT) for CBOR serialization

### Legal considerations for the replacement

The Wire prekey format is a protocol specification — a data format the Wire backend expects over the wire. Protocol specifications and data formats are not copyrightable under EU law (SAS Institute ruling) and the equivalent US doctrine. Implementing the format is a question of interoperability, not derivation.

Implementing the prekey generation and CBOR serialization from the observable wire format — matching what the Wire server accepts — is legally equivalent to implementing an HTTP client that follows RFC 7230. It is not a derivative work of Proteus the codebase; it is an independent implementation of an interoperable format.

---

## Options Considered

### Option A: Keep the Proteus dependency

Do nothing. Accept that Kupferdraht is locked to GPL-3.0 by transitive dependency.

- **Pro:** No implementation work. The current code path already works.
- **Con:** Forecloses strategic license options. Even if Kupferdraht's owners decide an Apache-2.0 posture is right, that decision becomes infeasible without first doing the dependency work.
- **Con:** Kupferdraht remains dependent on Wire's licensing policy for a protocol-level feature it does not even use.
- **Con:** Conflates technical constraint with strategic decision.

### Option B: Replace Proteus with an existing alternative Double Ratchet crate

Crates like `double-ratchet-2` and `ratchet-x2` exist under MIT license. They implement the Double Ratchet algorithm in Rust.

- **Pro:** Proven implementations, publicly maintained.
- **Con:** None of them use Wire's exact serialization format. Wrapping them to produce Wire-compatible prekeys requires implementing the Wire CBOR format on top — the same work as Option C with an extra dependency and API mismatch on top.
- **Con:** Kupferdraht does not need the Double Ratchet at all. Importing a full Double Ratchet implementation to use only the key-generation parts is over-dependency.

### Option C: Implement the minimal prekey generation and CBOR serialization directly

Write a small internal module in `kupferdraht-core/src/encryption/prekeys.rs` (naming TBD during implementation) that generates Curve25519 identity and prekey pairs, signs them with Ed25519, and serializes them in Wire's expected CBOR format. Use the cryptographic primitives already in Kupferdraht's dependency graph.

- **Pro:** Minimal surface. Only what Kupferdraht actually needs.
- **Pro:** No new dependencies. Uses crates already in the graph.
- **Pro:** Kupferdraht's dependency graph becomes fully GPL-free, which is a precondition for **any** future license decision other than GPL.
- **Pro:** Code is auditable — one small module that does one well-defined thing, with clear boundaries.
- **Con:** Implementation and careful validation effort. Estimated 200–400 lines of Rust plus interoperability tests against the live Wire backend.
- **Con:** We own the correctness of the CBOR byte-for-byte match with Wire's expectation.

### Option D: Ask Wire for a commercial Proteus license

Obtain an Apache-2.0 or commercial license from Wire for the three crates.

- **Pro:** No implementation work.
- **Con:** Creates an ongoing licensing relationship with Wire that Kupferdraht is specifically designed to avoid.
- **Con:** Wire has no established dual-license offering for these specific crates. Negotiating one is an indefinite, open-ended process.
- **Con:** Even if successful, Kupferdraht's license posture becomes "permissive with Wire's permission" rather than "permissive independently". Strategically weaker.

---

## Decision

**We implement the minimal prekey generation and CBOR serialization ourselves (Option C). The three Wire crates (`proteus-wasm`, `proteus-traits`, `cbor-codec`) are removed from `Cargo.toml` once the replacement is in place.**

**Kupferdraht's own `LICENSE` file is not changed by this ADR.** The strategic license decision is explicitly deferred to a future ADR. This ADR establishes the technical precondition; it does not pre-empt the strategic choice.

The decisive factors:

- Kupferdraht only uses a tiny fraction of Proteus — the prekey generation part — and ignores the Double Ratchet entirely. Carrying a GPL dependency for a feature we do not use is an unnecessary cost.
- The implementation surface is small and well-bounded.
- Removing the GPL constraint preserves all future license options. Keeping it forecloses several of them.
- All required cryptographic primitives are already in the dependency graph. No new external dependencies are needed.

---

## Consequences

### Positive

- Kupferdraht's dependency graph becomes free of GPL code.
- The strategic license decision can be made on its merits rather than constrained by technical lock-in.
- Downstream consumers no longer face GPL contamination via Kupferdraht's transitive dependencies — though Kupferdraht's own license still applies until a separate decision is made.
- Kupferdraht is no longer dependent on Wire's licensing policy for any component of its dependency graph.
- `cargo-deny` can enforce a strict permissive-only allow-list from this point forward, preventing future GPL drift through new dependencies.

### Negative

- Implementation and validation effort: 200–400 lines of Rust, plus interoperability tests against a live Wire v14 backend to confirm the CBOR output is byte-for-byte acceptable to the backend.
- Kupferdraht now owns the correctness of the prekey format. Any future change Wire makes to their Proteus prekey format must be replicated in Kupferdraht. In practice this is unlikely — the format has been stable for years — but it is a maintenance responsibility.
- We diverge further from Wire's client codebase. Not a new consequence (we diverged at ADR-0001), but strengthened.

### Neutral

- No user-visible behavior change. Prekey generation is an internal registration step. The Wire backend cannot distinguish a Kupferdraht client using the new implementation from one using the old `proteus-wasm` crate, because the bytes on the wire are identical.
- No performance change. Prekey generation happens once at client registration.
- **Kupferdraht's own license is unchanged by this ADR.** A separate ADR will address the strategic license decision once the technical precondition is in place.

---

## Implementation Notes

### Prerequisites

Before starting Step 9, cargo-deny must already be installed and configured (task before Step 2). At that point cargo-deny permits GPL-3.0 only as a specific exception for the three Wire crates. After Step 9 completes, those exceptions are removed.

### Scope of the new module

- Curve25519 identity keypair generation
- N Curve25519 prekey pairs (N is the Wire-specified batch size, confirmed via `reference/wire.swagger.json` and live validation)
- Ed25519 signing keypair and signatures over prekey public keys where Wire expects them
- CBOR serialization matching Wire's expected format exactly
- Minimal public API: one function that produces the JSON body for `POST /clients` prekey fields

### Validation

Interoperability tests against a live Wire v14 backend confirm:

- A client registered using the new implementation is accepted by the Wire backend.
- Other Wire clients (wire-android, wire-desktop, iOS) can establish MLS groups that include the Kupferdraht client without error.
- The prekey format matches byte-for-byte what `proteus-wasm` would have produced for the same inputs.

### Cleanup

- Remove `proteus-wasm`, `proteus-traits`, `cbor-codec` from `Cargo.toml` and `Cargo.lock`.
- Remove the three exception entries from `deny.toml`.
- Tighten `deny.toml` to the strict Apache-2.0/MIT/BSD/ISC/MPL-2.0 allow-list.
- Regenerate `docs/THIRD_PARTY_LICENSES.html` via `cargo about`.
- Write an announcement note for `CHANGELOG.md` documenting the before/after dependency state and the fact that the license decision is intentionally deferred.

### Out of scope for this ADR

- Changing Kupferdraht's own `LICENSE` file.
- Changing the `license` field in any workspace `Cargo.toml`.
- Updating CLAUDE.md or README.md license posture.

These changes are gated by a future strategic license-decision ADR.

---

## References

- ADR-0001 — original decision to use `openmls` + `proteus` directly. This ADR narrows the Proteus dependency further, toward zero.
- `reference/wire.swagger.json` — Wire API v14, authoritative reference for the `POST /clients` prekey format.
- `docs/THREAT_MODEL.md` — unaffected by this change. The cryptographic guarantees remain intact; only the implementation source changes.
- EU Directive 2009/24/EC Art. 1(2) — data formats and interfaces are not protectable under software copyright.

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-23 | Initial ADR (Apache-2.0 migration baked in) |
| 1.1 | 2026-04-23 | Decoupled technical decision from strategic license decision. Step 9 now removes GPL dependencies but does not change Kupferdraht's own license. License decision deferred to a future ADR. |
| 1.2 | 2026-07-23 | Deferred license decision resolved by ADR-0016 (GPL-3.0-only). This ADR's dependency rules are unchanged and re-affirmed there. |
| 1.3 | 2026-10-08 | Wording: validation targets named as a live Wire v14 backend, not Wire's hosted service. Decision unchanged. |
