# ADR-0036: Admit browser storage connections with short-lived, key-bound tokens from a token exchange

**Status:** Accepted
**Date:** 2026-09-29
**Deciders:** Markus Troßbach

## Context

ADR-0035 puts the browser's databases on a `sqlite-remote-server`. Such a server is reachable from the internet, and
without a gate anyone could store databases on it. Whether someone may use it is a question about their messaging
account: are they a member of a team the operator serves? The same question comes up for the PIN service that seals
the seed (ADR-0035), which must not receive the account's access token, because that token acts for the user at the
backend for up to 15 minutes.

The storage server keeps connections open for hours and reconnects on its own, and a browser WebSocket can send
neither an `Authorization` header nor any credential other than the URL, a subprotocol or a cookie. The account's
access token expires after minutes. The storage server should also keep working while the messaging backend is
briefly unavailable.

`sqlite-remote-vfs` and `sqlite-remote-server` (v0.1.0) support an opaque token in the login (`Hello`), requested from
a token source before every connect; the server verifies it offline as a JWT against a JSON Web Key Set, requires a
`cnf` key binding equal to the connection's public key, and ends a connection when its token expires.

## Options Considered

### Option A: A proxy that checks the access token on every connection

**Pros:**
- The VFS stays unaware.

**Cons:**
- The token must travel in the URL or a cookie and is stale on the next reconnect; the storage server depends on the
  backend at every reconnect; the proxy sees access tokens.

### Option B: A pass cookie issued by a gate

**Pros:**
- The VFS stays unaware; reconnects carry the cookie.

**Cons:**
- Fragile: third-party cookie rules, same-site deployment constraints.

### Option C: Admit keys on the server (an enrolment list)

**Pros:**
- No token per connection; reconnects always work.

**Cons:**
- State on the storage server; revocation needs list maintenance; a server change beyond the protocol.

### Option D: A token exchange issues short-lived JWTs bound to the storage-login key

**Pros:**
- The storage server verifies offline and never talks to the backend; the admission lives in one place; a copied
  token is useless without the key; revocation takes at most a token lifetime.
- The same exchange serves the PIN service with a token of its own audience.

**Cons:**
- One more service to operate, which sees access tokens.

### Option E: The storage server checks the access token itself

**Pros:**
- No extra service.

**Cons:**
- Couples the storage server to the backend and hands it access tokens.

## Decision

**Option D.**

1. **Where the token comes from.** In server mode Kupferdraht asks the token exchange for a storage token once the account
   is authenticated and before it opens a database: the account's access token as bearer, the audience `storage`,
   and the public storage-login key. It keeps the token in memory only, hands it to the VFS through its token source
   on every connect, renews it at half its lifetime and after a refusal, with a current access token.
2. **What the token is.** An EdDSA JWT of the exchange: `aud` is the storage server's own name, `cnf.jwk` the
   storage-login key, a short lifetime (one hour by default). The storage server verifies it offline against the
   exchange's key set and compares `cnf` with the key of the connection.
3. **One audience per server.** The PIN service gets tokens of its own audience (`pin`), without a key, since a
   browser restoring its recovery data has no key yet. Neither server accepts the other's tokens, and only the
   exchange ever sees the account's access token.
4. **Admission is the exchange's.** Which teams and users may use the servers, and how many tokens and keys a user
   gets, is configured there. Kupferdraht reports a refusal as `StorageAccessDenied`, waits out a rate limit, and reports
   an unreachable exchange as `ServiceUnavailable`.
5. **Expiry and revocation.** The storage server ends a connection when its token expires; the VFS reconnects with a
   fresh token. A token refused while running stops the instance with `onEvicted("access")`.

## Consequences

### Positive

- The storage server and the PIN service do not depend on the backend at runtime and never see an access token.
- One place decides admission, by team and by user.
- A copied storage token is useless; a user who loses access is out after one token lifetime.

### Negative

- A token exchange must be operated, and it sees access tokens.
- Without the exchange, a client cannot connect again once its token expired.
- The exchange's limits count per instance.
- The audiences must be configured consistently: the exchange's storage audience must equal the storage server's
  name, its PIN audience the PIN service's name.

### Neutral

- Local mode has no gate.

## Implementation Notes

- `Kupferdraht.open` server-mode storage options carry `tokenExchangeUrl`; requests are `POST <url>/v1/token` with
  `{"audience": "storage", "publicKey": "<base64url>"}` or `{"audience": "pin"}`; the answer is
  `{"token", "expiresIn"}`.
- The VFS token source is synchronous: it returns the cached token; Kupferdraht refreshes in the background.
- Tokens, like access tokens, never appear in logs, errors or `Debug` output.

## References

- Builds on ADR-0035 (pull request #80).
- `sqlite-remote-vfs` / `sqlite-remote-server` v0.1.0 (token in `Hello`, JWKS, `cnf` binding).

## Revision History

- 2026-09-29: Created (Accepted — Markus)
