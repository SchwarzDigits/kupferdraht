# Step 139 — Service tokens from the token exchange on the browser surface

**Status: commissioned 2026-11-02** (from an embedder's spec "service tokens
from the token exchange on the browser surface", measured against the public
snapshot `4f30f02` of 2026-10-30; every claim re-verified on v0.1.1 the same
day). The embedder runs a service of its own beside the storage server and
the PIN service. It must answer only users who are logged in to Wire and
admitted by the operator — the question the storage server and the PIN
service ask, answered the same way: a short-lived token from the token
exchange (ADR-0036), verified offline against the exchange's key set, the
user in `sub`, the team in `team`. The Wire access token stays inside
Kupferdraht (ADR-0036: it acts for the user at the backend for up to 15
minutes), and only Kupferdraht talks to the exchange, so the embedder
cannot get such a token today.

## Problem

1. **The exchange client is generic, its surface is not.**
   `storage/remote/token_exchange.rs` posts any body to `<exchange>/v1/token`
   (`exchange`, `:63`); `fetch_storage_token` (`:49`) and `fetch_pin_token`
   (`:98`) only build the body (`{"audience": "storage", "publicKey": …}`,
   `{"audience": "pin"}`). `refusal` (`:131`) maps the answers — `401`
   `Unauthorized`, `403` `StorageAccessDenied`, `429` `RateLimited` with
   `Retry-After`, `503` and unreachable `ServiceUnavailable`, the rest
   `BackendRejected` — exactly the table the spec asks for. `buy_pin_token`
   (`wire_session.rs:93`) buys with the session's Wire token and renews it
   once on `Unauthorized`; recovery builds the session from
   `Arc::downgrade(client)` (`recovery/account.rs:132-133`). Nothing of this
   is reachable from JavaScript.
2. **The exchange exists in the server mode only.** `Kupferdraht.open`'s
   `storage.tokenExchangeUrl` (`wasm/manager.rs:176-218`) lands in the
   thread-local `ServerSettings` (`storage/remote/server.rs:67`,
   `use_storage_server` `:101`, `server_settings` `:158`); the local mode
   has none. `storage::remote` and `recovery` compile for wasm32 and tests
   only (`lib.rs:46-47`, `storage/mod.rs:10-11`).
3. **The exchange's limits are shared.** ADR-0036 names it (Consequences,
   "the exchange's limits count per instance"): the per-user limits that
   gate a service token also gate the renewal of the storage token
   (`keep_token_renewed`, a `429` is waited out). Page code calling for a
   token in a loop starves its own storage connection.
4. **Two audiences are Kupferdraht's own.** A PIN token in JavaScript would
   let page code spend the scarce PIN attempts (ADR-0035); a storage token
   is key-bound and useless to the page, but nothing else of Kupferdraht's
   should be mintable from outside.
5. **ADR-0036 says two audiences, tokens in memory only.** Decision 3 ("one
   audience per server"), the Implementation Notes (`:112`, the two request
   bodies) and the logging note (`:115`) describe today; a token kind that
   leaves Kupferdraht on the embedder's request is not written down, and
   THREAT_MODEL N8 (the surrounding page is not under Kupferdraht's
   control) does not say that such tokens are visible to page code by
   design.

## Decisions (phase 1, Volker)

1. **A cache per audience** — the spec says none is needed; the shared
   limit of Problem 3 says otherwise. One token per audience, served until
   half its lifetime like the storage token (`StorageTokenCache`,
   `token_cache.rs:14`), bought anew after that or after a refusal.
   Recommended: the cache.
2. **ADR-0036 revision 1.1 and `rules/0036-*.md`** in the same commit: a
   third token kind, bought for an audience the embedder names, handed to
   the page; the reserved audiences; the request body generalised to
   `{"audience": "<name>"}`. One sentence at THREAT_MODEL N8: service tokens
   cross the wasm boundary by the embedder's request and live in the
   embedder's trust domain. Recommended: both, phase 1.
3. **wasm only.** `serviceToken` is a method of the wasm `Client` class like
   `recovery()` (`wasm/manager.rs:1385`), no `pub` symbol in the core, so
   no FFI mirror; the tab-proxy parity test applies. Native would first
   need an exchange address in `ServerConfig` — out of scope, as the spec
   says. Recommended: wasm only.

## Scope

1. **Exchange client.** `fetch_token(exchange_url, permit_http, wire_token,
   audience)` with the body `{"audience": audience}`; `fetch_pin_token`
   becomes a call of it. The reserved audiences `storage` and `pin` and the
   empty string fail as `InvalidInput` before any request.
   `buy_token(exchange, session, audience)` in `wire_session.rs` with the
   one renewal; `buy_pin_token` calls it.
2. **wasm.** `Client.serviceToken(audience)` returns the exchange's answer
   unchanged, `{ token, expiresIn }` (seconds); `server_settings()` or
   `InvalidInput` ("no token exchange configured") in the local mode; the
   session from `Arc::downgrade(&self.inner)`; the cache of decision 1.
   Kupferdraht neither parses nor checks the JWT. The call fails
   `SessionExpired` once the client is gone, as the buyers do.
3. **Tabs.** `["Client", "serviceToken", "idempotent", null]` in
   `protocol.js` (a re-issue to a new leader buys a token again), `proxy.js`,
   `index.d.ts`; `tests/step_94_tab_proxy_parity.rs` pins the table.
4. **Docs.** `docs/bindings.md`: the server-mode section (the exchange may
   know further audiences; a service token is a bearer token without the
   storage token's `cnf` binding — a copy works until it expires, the
   lifetime is the exchange's to set) and the error list;
   `docs/api-mapping-matrix-rust-sdk.md` (Kupferdraht-specific, like
   `receipt_mode`); ADR-0036 and THREAT_MODEL per decision 2; CHANGELOG
   `[Unreleased]` Added. `docs/library.md` is unchanged (no native surface).
5. **Errors.** The existing variants, by name; no `AccessDenied`:
   `KupferdrahtError` is not `#[non_exhaustive]`, a new variant breaks
   every exhaustive match. The `StorageAccessDenied` doc line in
   `docs/library.md:1820` and `docs/bindings.md:360` names the exchange's
   admission for any audience.

## Out of scope

- A native `service_token`: no exchange configuration exists natively.
- A top-level `tokenExchangeUrl` for the local mode (the spec defers it).
- Parsing or verifying the JWT, its `aud`, `sub` or `team`: the embedder's
  service does that against the exchange's key set.
- The exchange's redirect policy: Step 137 scope 4 sets `Policy::none()` on
  `token_exchange::http_client` (`:121`); this step lands after it.

## TDD sketch

- `token_exchange.rs`: the request carries the Wire token as bearer and
  `{"audience": "cases"}` and nothing else (httpmock); `storage`, `pin` and
  `""` fail `InvalidInput` with no request recorded; the answers map as
  the `refusal` tests already pin for the storage token.
- `wire_session.rs`: `buy_token` renews the Wire token once on
  `Unauthorized` and succeeds (the `TokenRefresh` stub of the existing
  tests); a second refusal is `Unauthorized`; a gone client is
  `SessionExpired`.
- The cache: a second call within half the lifetime makes no request; one
  after it does; a refusal evicts.
- Browser suite: `serviceToken("cases")` in the local mode fails
  `InvalidInput`; through a thin tab the call reaches the leader (the Step
  94 driver); the tab-proxy table pins `serviceToken`.
- Doc pins: ADR-0036 names the reserved audiences, `rules/0036-*.md` and
  THREAT_MODEL N8 carry the new sentence.
- No CLI ("no CLI (browser surface only)", precedent Step 115). Gate incl.
  the wasm32 checks. Smoke: against the dev stage only if its exchange is
  configured with a third audience — `serviceToken("cases")` returns a JWT
  whose `sub` is the logged-in user; else "no smoke (no exchange with a
  third audience)".
