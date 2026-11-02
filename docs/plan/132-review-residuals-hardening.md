# Step 132 — Review residuals: transport, platform and threading hardening

**Status: commissioned 2026-11-01** (transferred from open.md §71 — the
residuals of the production-readiness review of 2026-09-25 that are code,
each re-verified in the code on 2026-11-01; the review's blockers and
findings were closed by Steps 95–106 — plus, added 2026-11-01, the code
gaps of open.md's former known-gaps list and its §73). None is
exploitable; each is a line to a few dozen lines, the removal-flush queue
the largest, so they ship as one step with one gate.

## Problem

1. **M2 — redirects.** `HttpClient::new` (`wire/http/mod.rs`) builds reqwest
   with the default redirect policy: up to ten redirects, https → http
   included, and a 307 / 308 re-sends the login body with the password to
   the host it was redirected to. Only the configured backend can issue the
   redirect, so it takes a hostile or misconfigured server.
2. **M3 — `ws://`.** `wire/websocket.rs` accepts `ws` and `wss` alike and
   never consults `permit_http`; the CLI derives `wss://` from the required
   https `--api-url`, a library consumer can pass `ws://` in `ServerConfig`.
3. **M1 — WebSocket liveness.** `connect_async` runs without a timeout;
   pongs are discarded (`Message::Pong(_) => continue`), so liveness rests
   on a failed ping send and a hung connection is noticed one ping interval
   late. The polling fallback covers delivery.
4. **M4 — Windows names.** `is_path_safe_id` (`storage/home.rs`) refuses
   separators, control characters, `.` / `..` and over-long ids, but passes
   `:`, the reserved device names (`CON`, `NUL`, …) and a trailing dot or
   space. Windows is not a supported target yet; the check is cheap.
5. **H5 — blocking work in async code.** No `spawn_blocking` anywhere in
   the core; `replenish_key_packages` (`client/key_packages.rs`) generates
   up to 100 key packages inline. UniFFI polls on the caller's thread, so a
   Kotlin consumer calling from the main thread risks an ANR (derived from
   UniFFI's internals, not measured). `docs/bindings.md` states a thread
   rule only for `onLog` and for the browser worker.
6. **H6 residual.** UniFFI generates the Kotlin `toString` / Swift
   `description` of records itself, so `AssetMeta.assetToken` prints there
   although the Rust `Debug` redacts it; a consumer logging the record
   leaks a short-lived download token (N3 of the threat model puts that on
   the consumer).
7. **B7 / B9 residual.** `Client::send_bytes` (`timeline/asset_send.rs`,
   the browser flavour) emits no `TimelineDiff::Append` for its own row —
   `send_file` does, through `emit_outgoing_asset_append` — so a browser
   sender sees its file only on the next timeline read. Custom `Store`
   implementations stay non-atomic until they override the combined writes
   of Step 105 (the shipped SQLite store is atomic): a doc line, not code.
8. **Keystore before URL (former gap-B.26).** `ClientBuilder::login`
   resolves the home and the keystore (and writes the cookie label) before
   `HttpClient::new` validates the URL scheme, so on a Keychain-less macOS
   sandbox the `http_hardening` rejection test blocks on the Keychain for
   about 30 minutes instead of failing fast (observations §76;
   `KUPFERDRAHT_DEV_KEYSTORE` is the workaround).
9. **`ConvType 3` (former gap-B.33).** `wire/dto.rs` maps conversation
   type 3 to `Channel` and `group_conv_type` is never read. Wire treats
   type 3 as a pending-connection 1:1 and signals channels through
   `group_conv_type: "channel"` (`docs/wire/provenance-audit-2026-10.md`).
   Consumers keying on `RoomType::Channel` see the wrong rooms.
10. **Cold room cache after `resume` (former gap-B.35).** The wasm
    `Client.rooms()` / `messages()` read the in-memory cache, which is
    empty after `resume()` until the first `syncOnce`: `rooms()` answers
    `[]` and `messages(roomId)` fails "unknown room" for a room the store
    holds (seen live 2026-09-25 by a promoted leader tab); the doc comment
    says "read from the store". Native has `room_or_load` (Step 14g).
11. **AES-256-CBC known-answer vectors (former gap-B.12)** are absent;
    they come from an independent tool (`openssl enc`), never from a Wire
    repository (ADR-0011 revision 1.2).
12. **Ciphersuite mismatch is silent (NG-25).** Measured live on
    2026-11-02 (observations §94): a self-hosted backend with
    `defaultCipherSuite: 1` surfaces as `room-invite` "Wire returned no
    KeyPackages … (pool exhausted or user has no devices?)". The backend's
    `mls.config.defaultCipherSuite` is stored and never compared with
    `MlsClient::MLS_CIPHERSUITE`; a backend on another suite fails on the
    first group add with no hint.
13. **The removal-flush queue dies with the client (former open.md §73).**
    The Step-74 flush (`sync/removal_flush.rs`) commits backend Remove
    proposals at the end of a sync cycle and keeps a failed room queued in
    the `Client`'s memory; after a restart the queue is empty while the
    proposal still sits in the room's MLS state, and every send there fails
    `GroupStateError(PendingProposal)` until another Remove arrives (found
    by the Step-117 browser smoke, 2026-10-01; native has the same queue).

## Scope

1. Redirect policy: none, or same-origin https only — decide in
   orientation; "none" is the simplest and matches Wire's API, which does
   not redirect. A 3xx to another host is an error, a 307 never re-sends
   the login body.
2. `ws://` is refused unless `permit_http` is set, like `http://`, with the
   same error shape.
3. A connect timeout on `connect_async` (the `HttpClient` connect timeout
   as the value); pong tracking: a ping without a pong within one interval
   closes the socket so the reconnect path runs.
4. `is_path_safe_id` refuses `:`, the reserved names case-insensitively
   with and without an extension, and a trailing dot or space.
5. `replenish_key_packages` generates on `spawn_blocking` natively (the
   wasm target has no blocking pool and keeps the inline path).
   `docs/bindings.md` gains the threading contract: every call may block
   for storage and crypto, so call from a background dispatcher, never the
   main thread; the browser runs in a worker already.
6. `docs/bindings.md`: the `assetToken` line (do not log `AssetMeta` as a
   whole).
7. `send_bytes` emits the `Append` like `send_file`; the `Store` trait doc
   names the atomicity expectation.
8. `ClientBuilder::login` constructs the HTTP client (scheme check) before
   it touches the home or the keystore.
9. Type 3 maps to the pending-connection 1:1 shape, `group_conv_type:
   "channel"` to `Channel`; verified live in this step's smoke before the
   mapping changes; `docs/library.md` notes it for consumers.
10. The wasm `rooms()` / `messages()` hydrate from the store on a cold
    miss (`load_rooms_from_store` once after `resume`, or `room_or_load`),
    and the doc comment says what they read.
11. Known-answer tests for AES-256-CBC from `openssl enc` vectors.
12. At login and resume, a `defaultCipherSuite` other than ours is a
    typed error naming both suites (NG-25).
13. The removal-flush queue is rebuilt at client start from the groups'
    stored pending proposals (or the send path commits pending proposals
    before it creates the message — decide in orientation after
    reproducing why the first flush failed).

## Out of scope

- Measuring the ANR risk on a device; the contract is stated and the one
  generation site moved.
- Windows as a target (Step 20e's support-tier ADR).
- The review's documentation claims (Step 133) and local forward secrecy
  (Step 134).

## TDD sketch

One red test per item, in the module's test file or in
`tests/step_132_review_residuals.rs`: a redirect is refused (httpmock 302
and 307); `ws://` is refused without `permit_http`; the `is_path_safe_id`
table; the native `send_bytes` path emits `Append` (the Step-105 fixtures);
doc pins for the threading contract and the token line in
`docs/bindings.md`; the builder refuses an `http://` URL before any keystore
call (an `InMemoryKeyStore` that panics on first use); the `ConvType`
table; a wasm-shaped `rooms()` after `resume` on a store with rooms (native
test of the shared path); the AES-CBC vectors; a mismatching
`defaultCipherSuite` fixture is refused at login; a client restarted with a
stored pending Remove proposal flushes it on its first cycle. The pong timeout needs a scripted socket — the
websocket test module's fixture if it has one, else a minimal
`tokio-tungstenite` server in the test. Gate; the smoke is one connect to the
dev stage over the new timeout path and the `ConvType 3` check against a
live v14 backend, since no backend surface changes.
