# ADR-0001: Use openmls and proteus directly instead of CoreCrypto

**Status:** Accepted
**Date:** 2026-04-20
**Deciders:** Volker Zöpfel, reviewed by Markus Trossbach

---

## Context

Kupferdraht provides MLS-based end-to-end encrypted group communication for the Wire backend protocol, as an independent alternative to Wire's Kotlin Multiplatform SDK (Kalium). An early technical decision was which cryptographic library to build on.

Wire publishes **CoreCrypto**, a Rust library that bundles MLS, Proteus, and an encrypted keystore in a single package. CoreCrypto is what Wire's own mobile and desktop clients use internally. Building on CoreCrypto would be the path of least resistance: a single dependency, maintained by Wire, already proven in production.

When we attempted to integrate CoreCrypto as a Cargo git dependency (tag `v9.3.3`, the latest release at the time), the build failed.

The root cause was not a single bug but a structural property of how Wire distributes CoreCrypto. The library is published as a git repository, not as a versioned package on `crates.io`. Its internal dependencies include Wire-internal crates (notably `rust-pki`) referenced by *moving git branches* rather than by fixed version. Within Wire's own build environment, internal coordination ensures these branches match the CoreCrypto release. Outside that environment — for a consumer like us — the branches have drifted since the CoreCrypto tag was cut.

Concretely, `rust-pki` introduced a breaking API change between the CoreCrypto release and the current branch state (`Option<Vec<X>>` return type became `Vec<X>`), which caused a compile failure in CoreCrypto itself. A second failure was caused by two conflicting versions of `vergen_lib` entering the dependency graph.

Both failures could be patched by pinning Wire's internal crate versions ourselves — perhaps half a day's work. But that patch would have to be reapplied with every CoreCrypto update, and the drift would continue. We would effectively be reconstructing parts of Wire's internal CI configuration in our own build, without being part of it.

The underlying observation is that CoreCrypto is not designed to be consumed as a public library by third parties. It is an internal code collection that Wire assembles for its own clients. The public git URL is incidental, not an interface contract.

---

## Options Considered

### Option A: Use CoreCrypto with manual dependency pinning

Fork the dependency tree, pin every Wire-internal crate to the versions that match a given CoreCrypto release, maintain those pins as Cargo patches.

- **Pro:** Matches what Wire's own clients use. Minimal divergence from Wire's implementation.
- **Con:** Requires redoing the pinning work on every CoreCrypto upgrade. Fragile. Creates a structural dependency on Wire's internal CI output without being part of it.
- **Con:** Hard to onboard new contributors — the build environment is subtly broken for anyone who isn't already familiar with the patch set.
- **Con:** Does not change the GPL licensing situation (see Consequences).

### Option B: Use openmls and proteus directly

CoreCrypto itself is built on two independent lower-level libraries:

- **openmls** — the canonical reference implementation of MLS (RFC 9420). Apache-2.0 licensed, cleanly published on `crates.io`, used by Wire itself as its MLS core.
- **proteus** — Wire's pre-MLS encryption library for 1:1 chats. Wire-maintained, single crate, GPL-3.0 licensed.

Depend on these two directly and build the thin integration layer ourselves that CoreCrypto would otherwise provide.

- **Pro:** Stable, publicly consumed dependencies. No branch drift. Easy to onboard contributors.
- **Pro:** Wire-line behavior against the Wire backend is identical — openmls produces RFC-9420-conformant MLS messages, which is exactly what CoreCrypto produces.
- **Pro:** Closer to the long-term target of moving off GPL entirely. Only proteus remains GPL; openmls is Apache-2.0.
- **Con:** More code to write on our side. CoreCrypto provides helpers for three Wire-specific MLS extensions (external senders, identity format, required capabilities) that we must implement ourselves. Estimated 50–100 lines, not on the critical path for early steps.
- **Con:** We diverge from Wire's client codebase. Any future alignment effort becomes harder.

### Option C: Re-implement MLS from scratch

Not considered seriously. openmls is the mature, audited reference implementation. Re-implementation would be a multi-year effort with no meaningful benefit.

---

## Decision

**We use openmls and proteus directly (Option B).**

The structural fragility of Option A outweighs its apparent simplicity. A build that depends on Wire-internal coordination without being part of it is a liability, not an asset. Option B costs us roughly 15–20% additional implementation time across the early steps, which is bounded and one-time. Option A's cost is recurring and grows with every Wire release.

Option B also aligns better with the longer-term architectural direction. We eventually want to remove the GPL dependency (proteus) and move to a fully Apache-2.0 stack. Starting with openmls as the MLS core makes that future step simpler; starting with CoreCrypto would require undoing the coupling later.

---

## Consequences

### Positive

- Stable, publicly consumed dependencies. No branch drift, no coordination with Wire's internal CI.
- Lower barrier to contribution. A new developer can clone the repo and build without specialized knowledge of Wire-internal crate versions.
- Clearer licensing picture. openmls is permissively licensed, which preserves our options for future license changes.
- Closer to the target of Apache-2.0-only. Achievable in a later step by replacing proteus with a permissively licensed equivalent.

### Negative

- We write and maintain the thin layer between openmls/proteus and Kupferdraht's public API that CoreCrypto would otherwise provide. Estimated 200–400 additional lines of code over the lifetime of the early implementation steps.
- We diverge from Wire's client codebase. If Wire later ships bug fixes specific to their CoreCrypto wrapper, we need to reimplement them ourselves.
- We must implement three Wire-specific MLS extensions ourselves (details in Implementation Notes).

### Neutral

- Wire-line compatibility is unchanged. The Wire backend does not observe any difference between a CoreCrypto-based client and a direct-openmls client, because both produce RFC-9420-conformant MLS messages.
- proteus remains a GPL dependency until a replacement is introduced. This was true under Option A as well; the decision here does not change the GPL situation.

---

## Implementation Notes

Three Wire-specific MLS integration points that require our own implementation rather than being provided by CoreCrypto:

- **External Senders Extension** (RFC 9420 §12.1.8). Wire registers the backend as an external sender in every MLS group. See `docs/wire/mls-deviations.md` W-5 for the full rationale and mechanism. Required at Step 3 (group creation) and Step 6 (event stream).
- **Wire Identity Format.** Wire uses a specific domain-qualified identity string format in MLS credentials. Required at Step 1 (client registration).
- **Required Capabilities Extension.** Wire specifies a list of client capabilities that all group members must support. Required at Step 3 (group creation).

Time impact estimated at 15–20% across Steps 1–6, with the largest single increment in Step 1 (login and client registration) going from approximately one day to two or three days.

---

## References

- `docs/wire/mls-deviations.md` W-5 — detail on the External Senders integration
- `THREAT_MODEL.md` — security goals that this decision supports (G1: correctly propagate MLS guarantees)
- [openmls on crates.io](https://crates.io/crates/openmls)
- [RFC 9420: Messaging Layer Security](https://www.rfc-editor.org/rfc/rfc9420)

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR, superseding the `corecrypto-blocker.md` note |
| 1.1 | 2026-09-15 | `docs/archive/corecrypto-blocker.md` removed (Volker): its rationale was already fully absorbed into Context at 1.0 — the note added only a German stakeholder summary, retrievable via git history. `THREAT_MODEL.md` now points here instead. No content change. |
| 1.2 | 2026-10-07 | Context wording: "a replacement for" Kalium becomes "an independent alternative to" — Kupferdraht is a client library beside Wire's, not a substitute Wire ships or endorses (CLAUDE.md: "a Wire-compatible client, not a Wire replacement"). No content change. |
