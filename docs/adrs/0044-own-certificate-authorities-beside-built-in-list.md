# ADR-0044: Add an app's own certificate authorities to the built-in list, on the builders

**Status:** Accepted
**Date:** 2026-11-11
**Deciders:** Volker Zöpfel

## Context

Kupferdraht's native TLS connections trust only the Mozilla list compiled in through
`webpki-roots`: reqwest with `rustls-tls`, tokio-tungstenite with
`rustls-tls-webpki-roots`. No extra root can be given, and the system store and
`SSL_CERT_FILE` play no part. A self-hosted wire-server — the supported target —
behind a company's own certificate authority cannot be reached, and neither can
the local wire-server-rs stack from Kupferdraht's own builds, which invites
workarounds up to switching the check off. The browser is not affected: `fetch`
and its WebSocket check with the browser's list.

The WebSocket takes a custom root store only as a rustls `ClientConfig`
(`Connector::Rustls`), which needs the `rustls` crate directly; CLAUDE.md forbade
"direct `ring` / `rustls` / `openssl` outside what openmls/proteus pull in
transitively". THREAT_MODEL A4 asks for certificate validation, "ideally with
pinning"; there is no pinning.

## Options Considered

### Option A: Anchors added to the built-in list, on the builders

`ClientBuilder::add_root_certificates` and the same on `Kupferdraht`'s builder
(matrix-rust-sdk's name), PEM text, for every native TLS path: both reqwest
clients and the WebSocket over a rustls config of the Mozilla list plus the
anchors. The app passes them at every start; nothing is stored.

**Pros:**
- A company CA works, with no change for anyone else.
- matrix-rust-sdk's shape (ADR-0009).
- A CA rotation needs no migration; fits managed app configurations.

**Cons:**
- A direct rustls dependency.
- The app passes the anchors at every start.
- An anchor holds for every connection of the app, not one server.

### Option B: Anchors as a field of `ServerConfig`, stored per server

**Pros:**
- Scoped to one server; a resume finds them in `ServerConfigRecord`.

**Cons:**
- A schema migration.
- A rotated CA stays stale in the store until the app changes it.
- Not matrix-rust-sdk's shape.

### Option C: The system store (`rustls-platform-verifier`)

**Pros:**
- A company CA distributed by device management works without configuration.

**Cons:**
- Trusts whatever the device has installed; gives up the explicit configuration.

### Option D: Switching the check off (`danger_accept_invalid_certs`)

**Pros:**
- Reaches any server.

**Cons:**
- No protection against a man in the middle. Never, not even for tests.

### Option E: Anchors for HTTP only

**Pros:**
- No direct rustls dependency (reqwest's `add_root_certificate`).

**Cons:**
- The WebSocket fails against a company CA; the sync falls back to polling with
  no live push and no typing indicators.

### Option F: A WebSocket library on reqwest

**Pros:**
- One TLS configuration for both transports.

**Cons:**
- A new dependency and a rewrite of the push transport (ADR-0019).

## Decision

Option A. The anchors are added, never in place of the Mozilla list, and nothing
turns the check off; without them nothing changes. PEM, one or more certificates;
PEM without a certificate is `InvalidInput` at build. FFI:
`Kupferdraht.openWithRootCertificates(baseDir, keystore, pem)`; CLI:
`login --ca-file`, kept in the CLI's home. `rustls` and `webpki-roots` may be used
directly for the transports' TLS trust configuration only — in the versions
reqwest and tokio-tungstenite already pull in, with the `ring` provider — never
for cryptography of Kupferdraht's own; CLAUDE.md's rule says so. A certificate
the trust does not cover is `KupferdrahtError::CertificateRejected` where a
connection to a host is first made: the API version, a login, an authenticated
call, the asset store's redirect, the WebSocket.

## Consequences

### Positive

- Company deployments and the local stack are reachable without a workaround.
- A refused certificate is recognisable as one.
- Kupferdraht's own builds can test against the local stack.

### Negative

- A direct rustls dependency that must stay on reqwest's rustls version.
- The app passes the anchors at every start (from its managed configuration).
- An anchor is trusted for every connection of the app, not one server.
- No pinning yet (THREAT_MODEL A4); a later step when a customer asks.
- `CertificateRejected` is a new enum variant: BREAKING, released with 0.6.0.

### Neutral

- No effect in the browser.
- Request sites after the first contact keep `Network`.

## Implementation Notes

`wire/tls.rs` (`TrustAnchors`: the PEM parse, the reqwest certificates, the
WebSocket's `ClientConfig`, the walk to rustls' refusal through nested
`io::Error`s); `HttpClient::with_trust` for both clients;
`EventSocket::connect_trusting` and `connect_with_bearer_timeout(…, trust)`;
`ManagerConfig` carries the PEM to every per-account `ClientBuilder`. Step 156
(`docs/plan/156-own-certificate-authorities.md`).

## References

- Builds on ADR-0009 (the API after matrix-rust-sdk: `add_root_certificates`).
- Related: ADR-0019 (the push transport whose sockets take the anchors), ADR-0035
  (the browser's storage path, unaffected), ADR-0022 (browser persistence).

## Revision History

- 2026-11-11: Created and accepted (Volker Zöpfel, Step 156).
