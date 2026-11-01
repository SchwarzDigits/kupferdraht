# Step 128 — `crypto-bigint` 0.5 leaves the dependency graph (RUSTSEC-2026-0387)

**Status: commissioned 2026-11-01** (transferred from open.md §81, where the
advisory was tracked since 2026-10-30; the route below was measured on
2026-10-31 as a candidate and is now the step). Removes the `deny.toml`
ignore for RUSTSEC-2026-0387.

## Problem

RUSTSEC-2026-0387 (2026-10-23): `crypto-bigint` 0.5 can branch on secret
data in subtraction, zero counting, division, Montgomery multiplication and
limb equality, depending on compiler and CPU. Patched in 0.7.6; 0.7 is taken
only by the RustCrypto 0.14 line (`elliptic-curve` 0.14, `p256` 0.14,
`ecdsa` 0.17).

Kupferdraht reaches 0.5 through `p256` 0.13 from four places:
`openmls_rust_crypto` 0.6 (HPKE over `hpke-rs-rust-crypto` 0.7 — the
key-package and Welcome path of MLS_128_DHKEMP256_AES128GCM_SHA256_P256),
`openmls_basic_credential` 0.6 (the ECDSA P-256 signatures on MLS messages),
`voprf` 0.5 (the PIN service's OPRF, RFC 9497 over P-256) and the crate's own
`p256 = "0.13"` (receipt verification under `recovery/`). None of the three
upstream crates has a release on the 0.14 line. Accepted 2026-10-30 as a
`deny.toml` ignore with the reason recorded there: the crates cannot move
independently, and the alternative was no build until openmls follows. The
advisory concerns side channels an attacker must observe on the device
running Kupferdraht; nothing in the protocol or the stored data changes.

Upstream state (2026-10-31): openmls `main` moved its crypto crates to
`p256` 0.14 and `hpke-rs` 0.8 (PR #2249, merged 2026-10-02; the three
`hpke-rs` 0.8 manifest lines in #2261), unreleased. Since then `main` also
changed `openmls_traits` (async storage, ML-DSA, hybrid suites), so the next
upstream release is a migration, not a bump. `voprf` has not moved
(0.6.0-pre.1 of 2026-04-06 pins `p256` 0.13). `voprf-ng` 0.6.0 (2026-10-27,
an openmls maintainer's fork of that pre-release) is on the 0.14 line with
the identical RFC 9497 vector file and a mechanical diff otherwise;
`privacypass` 0.6 takes it. Trust: one maintainer, no release tags, no
audit.

## Route (measured 2026-10-31)

Since Step 127 (ADR-0042) the organisation's openmls fork carries a
downstream branch consumed by git tag; the same mechanism carries this
migration ahead of an openmls release:

1. An openmls fork branch from the `openmls-v0.9.0` tag with upstream
   commit 055a2b4b0 (PR #2249; the sources apply cleanly, conflicts only in
   five manifests and changelogs, PR side taken) and the three `hpke-rs`
   0.7 → 0.8 lines of e0b21a70a in `openmls_rust_crypto/Cargo.toml`;
   `openmls_traits` and `openmls_memory_storage` from crates.io 0.6.0
   instead of `workspace = true`. Both crates compile against traits 0.6.0
   and `cargo tree -i crypto-bigint@0.5` is empty for them. Branch and tag
   naming as in `vendor/README.md`'s recipe.
2. Kupferdraht: `p256 = { version = "0.14", features = ["oprf",
   "hash2curve", "ecdsa"] }` (the feature `voprf` became `oprf`),
   `voprf = { package = "voprf-ng", version = "0.6", … }` (same API names;
   `deterministic_blind_unchecked` under `danger`), `rand_core` 0.10 beside
   0.6, and `ed25519-dalek` gains its `rand_core` feature
   (`SigningKey::generate` needs it; `openmls_basic_credential` had unified
   it in so far).
3. Two code changes: `recovery/receipt.rs` encodes the verifying key with
   `to_sec1_point(true)` (`ecdsa` 0.17 dropped `to_encoded_point` on
   `VerifyingKey`), and `recovery/oprf.rs` gains a small adapter that
   implements `rand_core` 0.10's `TryRng` / `TryCryptoRng` over the
   caller's rand 0.8 RNG, because `voprf-ng`'s `blind` samples through
   `TryRng`.

Measured result: workspace and all targets compile, 1262 core tests pass
(PIN vectors, MLS flows), the wasm32 check passes, the lock holds
`crypto-bigint` 0.7.10 only, and `cargo deny` is green with the 0387 ignore
removed. More duplicate-crate warnings (`ed25519-dalek` 2 + 3, the
`aes` / `sha2` 0.10 + 0.11 families) until openmls releases.

## Scope

1. Phase 1, decisions before code: (a) the fork-branch policy with the
   maintainer of the organisation's openmls fork (open.md §82: the
   downstream branch sits beside upstream-PR heads); (b) `voprf-ng` versus
   an own RFC 9497 client over `p256` 0.14's `hash2curve` (blind, unblind,
   finalize — a small module, checked against the PIN-service interop
   vectors, open.md §77); (c) whether the duplicate-crate families are
   acceptable until the openmls release, or whether the step waits for it.
   No new ADR unless (b) chooses the own client; the fork line itself is
   ADR-0042's.
2. The fork branch and tag; `kupferdraht-core/Cargo.toml` on the new line;
   the two code changes; the `deny.toml` ignore removed; `vendor/README.md`
   and the third-party reports refreshed by the release recipe.
3. Live smoke, mandatory: the HPKE interop of the classical DHKEM-P256 path
   with `hpke-rs` 0.8 (upstream issue #2282 says only the PQ derivation
   changed — verify it: a Welcome from an official client decrypts, a key
   package it claims is usable), the ECDSA signing path with
   `ed25519-dalek` 3 beside 2 in the graph, and a PIN export / restore
   round trip (`recovery/`).

## Out of scope

- Following openmls `main`'s `openmls_traits` changes (async storage, new
  suites) — that is the next openmls release's migration, a step of its own.
- Any ciphersuite change (open.md gap-B.1).

## TDD sketch

`kupferdraht-core/tests/step_128_crypto_bigint_pins.rs`, in the idiom of
`step_76_openmls_upgrade.rs`: `Cargo.lock` holds no `crypto-bigint` below
0.7.6 — red; `deny.toml` carries no RUSTSEC-2026-0387 ignore — red; `p256`
is on the 0.14 line — red. Behaviour pins that already exist must stay
green: the RFC 9497 vectors of the PIN service, the receipt known-answer
tests (the SEC1 encoding of the verifying key is unchanged), the MLS flows
against a real openmls second device (`common::mls::PeerDevice`).
