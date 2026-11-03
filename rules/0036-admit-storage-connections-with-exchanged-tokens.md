# Storage access: short-lived, key-bound tokens from a token exchange

Condensed from `docs/adrs/0036-admit-storage-connections-with-exchanged-tokens.md`.

- **In server mode, get a storage token before opening a database:**
  `POST <tokenExchangeUrl>/v1/token` with the account's access token as
  bearer, `{"audience": "storage", "publicKey": <storage-login key>}`. Keep
  it in memory only; hand it to the VFS token source on every connect;
  renew at half its lifetime and after a refusal.
- **Never send the account's access token to the storage server or the
  PIN service.** The PIN service gets its own token (`{"audience": "pin"}`,
  no key); only the exchange sees access tokens.
- **Don't decide admission in Kupferdraht.** Map the exchange's answers:
  `403` → `StorageAccessDenied`, `429` → wait `Retry-After`, unreachable →
  `ServiceUnavailable`; a token refused while running → `onEvicted("access")`.
- **A service token on request only (`serviceToken(audience)`):** buy
  `{"audience": "<name>"}` with the session's Wire token (renew it once on
  a `401`) and hand the exchange's answer to the page unchanged; keep one
  per audience and serve it until half its lifetime — the exchange's
  limits are shared with the storage token's renewal. **Never** buy
  `storage` or `pin` on request (`InvalidInput` before any request, as for
  the empty audience); the local mode has no exchange (`InvalidInput`).
  Don't parse or verify the token — the embedder's service does; it is a
  bearer token without `cnf`.
- **Never log tokens** — storage, PIN, service or access — nor put them in
  errors or `Debug` output.
