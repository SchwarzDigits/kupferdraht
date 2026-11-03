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
3. **One audience per server, and further audiences for the embedder's services.** The PIN service gets tokens of
   its own audience (`pin`), without a key, since a browser restoring its recovery data has no key yet. Neither
   server accepts the other's tokens, and only the exchange ever sees the account's access token. The further
   audiences are decision 6.
4. **Admission is the exchange's.** Which teams and users may use the servers, and how many tokens and keys a user
   gets, is configured there. Kupferdraht reports a refusal as `StorageAccessDenied`, waits out a rate limit, and reports
   an unreachable exchange as `ServiceUnavailable`.
5. **Expiry and revocation.** The storage server ends a connection when its token expires; the VFS reconnects with a
   fresh token. A token refused while running stops the instance with `onEvicted("access")`.
6. **Service tokens on the embedder's request (revision 1.1, Step 139).** A third token kind: on
   `serviceToken(audience)` Kupferdraht buys a token of the audience the embedder names — the session's access token
   as bearer, the body `{"audience": "<name>"}`, no key — and hands the exchange's answer to the page unchanged, for
   a service the embedder runs beside the storage server and the PIN service that must answer only users logged in
   to Wire and admitted by the operator. The audiences `storage` and `pin` are reserved: Kupferdraht buys them for
   itself and refuses them on request before any request leaves (`InvalidInput`), as it refuses the empty audience
   and a call in local mode, which has no exchange. One token per audience is kept and served until half its
   lifetime, bought anew after that or after a refusal, because the exchange's per-user limits are shared with the
   storage token's renewal. Kupferdraht neither parses nor checks the token; the embedder's service verifies it
   offline against the exchange's key set (`sub` the user, `team` the team). Without a key binding it is a bearer
   token: a copy works until it expires, and the lifetime is the exchange's to set.

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
- A service token is a bearer token without `cnf`: handed to the page on the embedder's request, it lives in the
  embedder's trust domain like the plaintexts the page displays (THREAT_MODEL N8), and a copy works until it
  expires.
- The browser build's exchange client has no request timeout and no redirect policy (open.md §84); the
  service-token path shares the client and the gap.

### Neutral

- Local mode has no gate.

## Implementation Notes

- `Kupferdraht.open` server-mode storage options carry `tokenExchangeUrl`; requests are `POST <url>/v1/token` with
  `{"audience": "storage", "publicKey": "<base64url>"}` for the storage token and `{"audience": "<name>"}` for every
  other audience (`pin`, and the audiences of `serviceToken`); the answer is `{"token", "expiresIn"}`
  (`ExchangedToken`).
- The VFS token source is synchronous: it returns the cached token; Kupferdraht refreshes in the background.
  `storage/remote/service_tokens.rs` keeps the service tokens, one per audience, until half their lifetime.
- Tokens — storage, PIN and service tokens alike — never appear in logs, errors or `Debug` output, like access
  tokens.

## References

- Builds on ADR-0035 (pull request #80).
- `sqlite-remote-vfs` / `sqlite-remote-server` v0.1.0 (token in `Hello`, JWKS, `cnf` binding).

## Revision History

- 2026-09-29: Created (Accepted — Markus)
- 2026-11-03: Revision 1.1 (Step 139; Volker Zöpfel) — a third token kind: service tokens of an audience the embedder names, bought on `serviceToken(audience)` with `{"audience": "<name>"}` and handed to the page unchanged; `storage` and `pin` reserved; one token per audience until half its lifetime because the exchange's limits are shared; not parsed or checked by Kupferdraht. Decision 6 added, decision 3 names the further audiences, the Implementation Notes generalise the request body; nothing else changes.
