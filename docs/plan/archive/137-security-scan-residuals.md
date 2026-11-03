# Step 137 — Security-scan residuals: URL path segments, the CLI's cookie, error bodies, the token exchange's redirects, the case dispatch's cold cache

**Status: MERGED 2026-11-02** — `08d0e58` on
`step/137-security-scan-residuals`, merged to `main` as PR #17 (`9dd79e0`);
gate and CI green; smoked 2026-11-02 against the self-hosted stack (a
guest link redeemed with the key on stdin and `--key` refused; a Welcome
and a case snapshot in one notifications page folding on the invitee), no
`sso finalize` smoke (the stack has no IdP). Scope shipped: `path_segment`
at every dynamic path site plus `qualified_segments`; `sso finalize
--cookie-stdin` / `join-via-link --key-stdin` or the TTY prompt;
`redact::bounded_detail` at three sites; `Policy::none()` on the token
exchange; the room list refreshed right after a Welcome (ADR-0039
revision 1.1 — item 5 turned out worse than written below: the message
was lost, not only unauthorized); pins in
`tests/step_137_security_scan_residuals.rs`, `wire/http/mod.rs`,
`wire/http/auth.rs`, `storage/remote/token_exchange.rs`, `redact.rs` and
the CLI; retrospective LEARNINGS §5.74. Archived 2026-11-02 with this
header. (Commissioned 2026-11-02 from the review of the external
security scan of 2026-11-02; `docs/plan/135-*` and `136-*` hold its other
consequences). Step 132 merged the same day (PR #10) and closed the scan's
transport findings — no redirect on `HttpClient`, `ws://` refused without
`permit_http` natively and in the browser, a handshake deadline and pong
tracking on the push socket, the `assetToken` line in `docs/bindings.md`.
These five are what it did not cover. None is exploitable; each is a few
lines, so they ship as one step with one gate, like 132.

## Problem

1. **URL path segments (scan finding M5).** `HttpClient::path_segment`
   (Step 95c) guards four call sites — the asset downloads, the registered
   client id, the property key — while twenty other endpoints format
   backend- or consumer-supplied ids into the path unchecked
   (`wire/http/assets.rs` `delete_asset`; `clients.rs` ×5;
   `conversations.rs` ×8; `sso.rs` ×2; `groups.rs`, `key_packages.rs`,
   `mls.rs` and `one_to_one.rs` one each — the scan named sixteen and
   missed the last four), and `auth.rs` formats the client id into a
   query string (`/access?client_id=`). No peer reaches them (the dispatch
   boundary refuses `/`, `\`, `.`, `..` and control characters in ids),
   and a traversal lands on the backend that issued the id, so this is
   defence in depth; the 95c archive's "as built" line claims more than
   the code does.
2. **`--cookie` in argv (M1).** `sso finalize` takes the `zuid` refresh
   cookie as `--cookie <value>`, and `join-via-link` the link's `key`;
   both land in `ps` and the shell history, against `docs/cli.md`'s own
   rule for passwords (TTY prompt or `--password-stdin`).
3. **Unbounded error bodies (L6).** `reject_from_response`
   (`wire/http/mod.rs`) reads the whole response body and embeds it
   verbatim in `BackendRejected`, which crosses UniFFI and wasm into host
   logs; Step 101 bounded only the login path.
4. **The token exchange follows redirects (M4, second site).**
   `storage/remote/token_exchange.rs`'s `http_client` builds its own
   reqwest client with the default policy (`Policy::limited(10)`); Step
   132 set `Policy::none()` on `HttpClient`'s two builders only. The
   exchange POSTs a JSON body under the Wire token as Bearer, and a
   307 / 308 re-sends it. reqwest 0.12's wasm `ClientBuilder` has no
   `redirect()` — the browser's `fetch` follows on its own — which Step
   132 noted for `HttpClient` and which holds here too.
5. **The case dispatch's cold cache (M3).** `dispatch_inbound_case`
   (`sync/dispatch/case.rs`) evaluates the ADR-0018 receive-time admin
   check with `client.room`, the cache-only lookup: a room the cache does
   not hold yet persists `sender_authorized = false` for good. Every pass
   refreshes the list before dispatch, but a Welcome and a case snapshot
   in the same drain (the ADR-0020 invite flow) meet a cold cache, and
   `room_or_load` does not know a room the store has not seen either. Step
   132's item 10 hydrated `Client::rooms()`, not this site. Not reproduced
   live. Reproduced red in orientation, and worse than written here: the
   message never reaches the admin check. `decrypt_to_application`
   resolves the room to its group through the cache (`group_id_for_room`),
   so a message after its Welcome in the same page fails as `no local
   group_id`, is reported through `on_sync_error`, and queues a heal of
   the room just joined — the ADR-0020 re-broadcast is lost.

## Scope

1. `path_segment` on every dynamic path parameter in `wire/http/` (the
   `?client_id=` query value included), or once beside `HttpClient::url` —
   decide in orientation; the 95c archive line is corrected to what is
   built. Decided: per site, as at the four Step 95c sites — a joined path
   cannot tell an id with `/` from a legitimate second segment — with
   `qualified_segments` for the five sites that split `<uuid>@<domain>`.
2. `sso finalize` reads the cookie from a TTY prompt without echo, or from
   stdin with `--cookie-stdin`; `--cookie` goes. The same for
   `join-via-link`'s key. `docs/cli.md` follows.
3. `reject_from_response` caps `detail` (about 1 KiB) and strips control
   characters before it formats `BackendRejected`. Decided: one helper,
   `redact::bounded_detail`, at the three sites of the class —
   `reject_from_response`, `rebind_rejection` and the token exchange's
   `refusal`; the body is still read whole (the request timeout bounds it).
4. `token_exchange::http_client` gets `Policy::none()` natively; the wasm
   builder keeps the note `HttpClient` carries.
5. `dispatch_inbound_case` reads the members for the ADR-0018 check from
   the store, or the drain refreshes the room list after a Welcome before
   the next dispatch — decide in orientation after reproducing the
   Welcome-plus-case drain. Decided: the refresh right after the Welcome,
   in `dispatch_mls_welcome` (the store cannot help — the room's members
   come with the list, and the decrypt needs the room before the case
   dispatch does); ADR-0039 decision 1 reworded (revision 1.1).

## Out of scope

- Dropping `asset_token` from the FFI `AssetMeta` record: Step 132 chose
  the `docs/bindings.md` line (N3 puts logging on the consumer); the field
  stays until a consumer asks.
- X.509 leaves (Step 135), payload caps (Step 136), file modes (Step 134).

## TDD sketch

One red test per item: a hostile id is refused at every path site (the
`hostile_asset_path_segments_are_refused_before_any_request` table,
generalised); the CLI no longer accepts `--cookie` and `--cookie-stdin`
reads one line; a 1 MiB error body yields a bounded `BackendRejected`
without control characters; an httpmock 302 and 307 on the token exchange
are refused; a case snapshot dispatched for a room the cache lacks is
authorized per the store (or after the refresh). The CLI change is item 2
itself. Gate; smoke: one `sso finalize` through the new prompt against the
dev stage where an IdP is configured, else "no smoke (no backend surface
changes)".
