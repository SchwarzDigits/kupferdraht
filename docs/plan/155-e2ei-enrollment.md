# Step 155 — e2ei, enrollment: a certificate for Kupferdraht's own devices

**Status: commissioned 2026-11-10** (Volker), with Step 154. It starts only after
Step 154 and when it is needed: when a team requires every device to be verified, or
a backend refuses devices without a certificate. The Swagger names no such backend
check: the grace period `verificationExpiration` is enforced by each client
(`docs/observations.md` §100).

## Problem

Without a certificate, a Kupferdraht device stays on Basic credentials. Wire's apps
then show the conversations it is in as not verified, and a team with e2ei on has a
member it cannot verify.

## Scope (fixed in phase 1)

1. **Enrollment by ACME** (RFC 8555) against the server named by `acmeDiscoveryUrl`,
   with Wire's challenges: a DPoP access token from the backend
   (`GET|HEAD /clients/{client}/nonce`, `POST /clients/{cid}/access-token`) and the
   login at the team's identity provider, which the embedder hosts (browser) or opens
   (native).
2. **The certificate in the MLS credential:** the leaf's signature key in the
   certificate request, the credential rotated in every group by a commit.
3. **Renewal** before expiry, and what the grace period means for Kupferdraht.
4. Surface: FFI, wasm, CLI; the embedder's part of the identity provider's login.

## Decisions phase 1 brings to Volker

- The CLAUDE.md out-of-scope line and NG-7 change; an ADR for the enrollment and the
  key's lifecycle; THREAT_MODEL (the identity provider's login, the DPoP token).
- Which identity provider protocol Wire's enrollment uses with the team's provider,
  and what the embedder must host for it.

## Out of scope

- Running an ACME server or an identity provider.

## TDD sketch

- The ACME exchange against a mock server, the DPoP and nonce endpoints against
  httpmock, the credential rotation in a group.
- Smoke against a backend with e2ei on, its ACME server and identity provider.
