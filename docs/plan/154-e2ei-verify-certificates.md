# Step 154 — e2ei, reading: check the certificates of other members' devices

**Status: commissioned 2026-11-10** (Volker), from the e2ei measurement
(`docs/observations.md` §100): an identity provider is to be activated on the team's
backend, and a team that switches e2ei on makes Kupferdraht drop its enrolled
colleagues' messages. Kupferdraht's own devices stay on Basic credentials; their
enrollment is Step 155.

## Problem

Wire's end-to-end identity (e2ei, team feature `mlsE2EId`) gives each enrolled device
an X.509 certificate that binds its MLS signature key to the user's identity. Since
Step 135 (ADR-0027, "Senders with non-Basic (X.509) credentials", option D) Kupferdraht
drops every message signed by such a leaf, with a `warn`, because nothing it could
check named the sender. In a team with e2ei on, a Kupferdraht user sees none of the
enrolled colleagues' messages, while they see the Kupferdraht user's. The README says
so; the user sees nothing.

## Scope (fixed in phase 1)

1. **Check a member's certificate:** the chain to the team's trust anchor, the
   validity period, revocation, that the certificate names the sender (its user,
   client and domain, and the handle where Wire puts one), and that the leaf's
   signature key is the certificate's key.
2. **Show what checks out.** A message from a leaf whose certificate checks out is
   shown again, attributed to the certificate's identity; one that fails stays
   dropped and is logged, as today.
3. **A verification state on the surface,** per member or per message as phase 1
   decides (matrix-rust-sdk's nearest: `EncryptionInfo` and its shields): FFI, wasm,
   CLI.
4. **Where the trust anchor and revocation come from:** the ACME server named by
   `acmeDiscoveryUrl` and the `crlProxy` (observations §100), fetched and refreshed
   by Kupferdraht; the failure policy when they cannot be reached.

## Decisions phase 1 brings to Volker

- The trust model: the team's ACME certificate authority becomes a trust anchor beside
  the backend; who runs it, and what a changed `acmeDiscoveryUrl` means.
- Fail closed or open when revocation cannot be checked; expired certificates.
- An ADR that amends ADR-0027's X.509 decision, the matching `rules/` file, the
  CLAUDE.md out-of-scope line ("e2ei certificate enrollment — MLS BasicCredentials
  path only": checking others' certificates comes in, the own enrollment stays out
  until Step 155), NG-7, and a THREAT_MODEL section.

## Phase 1

- Wire's certificate profile and verification rules from the Swagger, RFC 9420
  (credentials), RFC 8555 (ACME), live observation on a backend with e2ei on, and
  Wire's repositories as protocol observation only (ADR-0009), each recorded in
  `docs/observations.md` / `docs/wire/kalium-readings.md`.
- openmls's support for X.509 credentials; an X.509 crate whose license `cargo deny`
  admits.
- A test environment: a backend with e2ei on, its ACME server and identity provider,
  and a Wire app with a certificate. Neither the local stack nor the dev backend has
  one today (§100).

## Out of scope

- Enrolling Kupferdraht's own devices (Step 155).
- Running an ACME server or an identity provider.

## TDD sketch

- Certificates from a test CA made in the tests: a valid chain shows the message with
  its identity; an unknown root, an expired or revoked certificate, an identity that
  names another client, and a signature key that is not the certificate's each keep it
  dropped.
- The verification state on each surface.
- Smoke against a backend with e2ei on and a Wire app enrolled.
