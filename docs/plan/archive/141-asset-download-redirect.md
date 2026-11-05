# Step 141 — Asset downloads follow cargohold's redirect

**Status: MERGED 2026-11-05** — `a8fb122` on
`step/141-asset-download-redirect`, merged to `main` as PR #31
(`9e1a11c`); gate green (its first run found
`step_135_x509_leaves_dropped` reading THREAT_MODEL's version as an `f32`:
0.10 parsed as 0.1 < 0.8; now compared as `(major, minor)`), CI green;
smoked 2026-11-04 on the wire-server-rs stack with
`kupferdraht-smoke.sh --throwaway`, 18/18 against the Rust cargohold and
18/18 against the Haskell cargohold 5.38.9 (nginz logs the `302` for both;
`CARGOHOLD_UPSTREAM` switched by Volker between the runs). Scope shipped
as planned below: `download_asset` follows one `302` / `303` / `307` hop
by hand (`wire/http/asset_redirect.rs`) — a GET without the bearer token
or the `Asset-Token`, an absolute `https` location (`http` with
`permit_http`), no userinfo, no second hop; `301` / `308` stay refused;
a refusal names host and status only; THREAT_MODEL 0.10. Pins: the five
unit tests in `wire/http/asset_redirect.rs`,
`wire/http/asset_redirect_tests.rs`,
`tests/step_141_asset_download_redirect.rs`; retrospective LEARNINGS
§5.79. The regression shipped first in v0.1.1, not in v0.2.0 as the text
below says: Step 132's `0818b6f` is in both tags, and the smoke had last
passed with v0.1.0 (CHANGELOG and observations §96 corrected on
archiving). No PLAN.md row (proposed and built the same day). Archived
2026-11-05 with this header. (Proposed 2026-11-04 by Volker from the
wire-server-rs Docker stack: `docker/scripts/kupferdraht-smoke.sh
--throwaway` fails its `download-asset` check since v0.2.0, against the
Haskell cargohold 5.38.9 and the Rust cargohold alike; 18/18 with v0.1.0
on 2026-10-31. Regression of Step 132 (M2, `0818b6f`, released in
v0.2.0). Ordered before 130b; ships in v0.2.1.)

**Taken 2026-11-04 (Volker), with the red tests:** THREAT_MODEL 0.10
names the redirect rule and its one exception (Trust Model); no
allow-list for the hop's target; a refusal of the hop names its host and
status only — the asset store's error body is dropped, since S3 echoes
the signature in it. No new CLI command: `download-asset` is the
consumer that regressed.

## Problem

1. **Wire answers a local asset download with a redirect.**
   `GET /v14/assets/{asset_domain}/{asset_id}` returns `302 Found` with
   `Location:` a pre-signed S3 URL on cargohold's `s3DownloadEndpoint`
   (path-style, signed in the query, valid `downloadLinkTTL` seconds — 300
   in the chart). The bytes never come from the API host. wire-server-rs
   keeps this behaviour in its Rust cargohold; its contract test
   (`docker/tests/cargohold-contract.sh`) pins it for both implementations.
2. **Step 132 stopped following it.** Both reqwest clients got
   `redirect::Policy::none()` (`wire/http/mod.rs`, `HttpClient::with_timeouts`)
   on the premise "Wire's API issues none" — the code comment there states
   it. The native download path, `HttpClient::download_asset`
   (`wire/http/assets.rs`, transfer client), treats every non-2xx as a
   rejection, so a received file can no longer be opened:
   `BackendRejected("download_asset 302 Found: ")`. This affects the CLI,
   the TUI and every FFI consumer. Uploads are fine (`POST /assets`
   answers 201). The browser build is not affected:
   `download_asset_bytes` runs on `fetch`, which follows redirects by
   itself.
3. **The spec says otherwise.** `docs/backend-spec.md` §15.2 states
   "`200` raw ciphertext bytes" — the premise of Step 132 came from there.
   The httpmock tests serve 200 directly, so no test caught the
   regression.

## Scope

1. **One redirect, on the native download path only.** `download_asset`
   keeps `Policy::none()` on the transfer client and follows by hand: on
   `302` (also `303` and `307`, which mean the same for a body-less GET)
   with a `Location` header:
   - the location must parse as an absolute URL (`url::Url`); a relative
     or unparsable location, or one with userinfo, is refused;
   - `https` is required; `http` only with `permit_http`, as for the base
     URL (`checked_base_url`);
   - the second request is a plain GET on the transfer client with **no**
     `Authorization`, **no** `Asset-Token` and no other headers — the URL
     carries its own signature (no cookie either: the reqwest clients keep
     no cookie store);
   - exactly one hop: a 3xx from the target is a `BackendRejected` as
     today; `301` and `308` from the API stay rejected (cargohold answers
     302);
   - the target host may differ from the API host (the S3 endpoint is a
     deployment choice, e.g. `assets.example.com`); see Security for the
     allow-list question;
   - the transfer idle timeout (Step 98) covers both hops;
   - logs name the target host only, never the query (it is a credential
     for its lifetime).
2. A `200` from the API host stays accepted (federated assets, other
   backends).
3. Every other path keeps `Policy::none()`: login, `/access`, the token
   exchange (Step 137) do not change.
4. **The code comment in `with_timeouts` is corrected:** Wire's API does
   redirect, on asset downloads; the policy stays `none` and
   `download_asset` follows its one hop by hand.
5. Docs in the same change:
   - `docs/backend-spec.md` §15.2: "`302` with `Location` a time-limited
     pre-signed URL (the normal case for local assets) or `200` with the
     bytes; the client fetches the location without credentials".
   - `docs/wire/api-notes.md`: a new asset-download section with the live
     observation (status, `Location` host = `s3DownloadEndpoint`, TTL),
     and the browser note below; the dated observation in
     `docs/observations.md`.
   - `docs/wire/asset-protocol.md` Q6 "Download response": the redirect
     beside the raw bytes.
   - `docs/THREAT_MODEL.md` 0.10: the Trust Model names the redirect rule
     of Step 132 and its one exception.
   - `docs/cli.md` (`download-asset`) and `docs/library.md`
     (`AssetSummary::download`): one sentence each.
   - `CHANGELOG.md` under Fixed.

## Security

- **Integrity does not depend on the hop.** The ciphertext is checked
  against the message's SHA-256 before AES-256-CBC decryption
  (`encryption/asset_crypto.rs`, `IntegrityCheck`; ADR-0011). The digest
  travels inside the MLS-encrypted message, so a redirect to a foreign
  host can withhold or corrupt the file, never substitute it, and the 302
  comes from the authenticated backend the client already trusts with
  the ciphertext.
- **No credential reaches the target.** Neither the bearer token nor the
  `Asset-Token` is sent on the second request. reqwest would drop
  `Authorization` on a cross-host hop by itself, but not the custom
  `Asset-Token` header — one reason to follow by hand instead of a
  `Policy::custom`.
- **No downgrade:** an `http` location is refused without `permit_http`.
- **No body re-send:** the hop is a GET without body; login and `/access`
  keep `Policy::none()`, which was M2's point.
- **What remains:** a malicious backend can make the client issue one
  credential-free GET to an `https` URL of its choice when the user opens
  an attachment — blind (the answer goes into the sink and fails the
  digest check, never back to the server). Accepted; the backend chooses
  the asset host by design.

## Phase-1 questions (taken, see above)

1. **THREAT_MODEL.** `docs/THREAT_MODEL.md` has no statement on redirects
   today; M2 lives in the archived Step 132 plan and the code comment in
   `wire/http/mod.rs`. Add one (0.10): "the client follows no redirect of
   the API, except one credential-free GET hop on an asset download,
   integrity by the digest in the MLS message" — recommended — or keep it
   in the code comment and backend-spec only.
2. **Allow-list for the hop target** (a configured asset host, or the host
   seen on the first download per backend)? Recommendation: no — Wire
   deployments name the asset host freely, and the digest check already
   covers integrity.

## Out of scope

- The browser build: `fetch` follows the redirect itself. Whether the S3
  endpoint answers CORS for the app origin, and what the browser does with
  the custom `Asset-Token` header on the cross-origin hop, is a deployment
  matter; this step only notes it in `docs/wire/api-notes.md` as an open
  observation.
- Uploads (no redirect), range requests, resuming a download.

## TDD sketch

- httpmock: the API mock answers 302 with `Location` on a second mock
  server; `download_asset` streams the second server's body into the sink.
  The second mock asserts the request carries no `Authorization` and no
  `Asset-Token`.
- The same for 303 and 307; a 301 or 308 from the API is a
  `BackendRejected` and the target sees no request.
- The scheme check is a unit test on the location check: the httpmock
  servers are plain `http` and the API mock itself needs `permit_http`, so
  "`http` refused without `permit_http`" cannot be an integration test;
  with `permit_http` the hop is followed (the integration tests above).
- A 302 whose target answers 302 again → `BackendRejected`; a third server
  sees no request.
- A 302 without `Location`, with a relative or unparsable one, or with
  userinfo → error, no second request.
- A `200` from the API host still streams (the existing tests stay green).
- Login and `/access` still do not follow a 302 or 307 (the Step 132 tests
  stay green).
- Gate; smoke: `docker/scripts/kupferdraht-smoke.sh --throwaway` against
  the wire-server-rs stack, 18/18 including `download-asset`, once with the
  Haskell and once with the Rust cargohold (`CARGOHOLD_UPSTREAM`).

## Red tests (2026-11-04)

- `wire/http/asset_redirect.rs` — `redirect_target(location, permit_http)`
  stubbed with `todo!()`, five unit tests: an absolute `https` location is
  the target; `http` only with `permit_http`; relative, scheme-relative,
  unparsable, `ftp`, `file`, `data`, `ws` refused; userinfo refused; a
  refusal never repeats the query. The module is test-only until
  `download_asset` calls it (a dead stub fails `deny(warnings)` in the
  library build the integration tests link).
- `wire/http/asset_redirect_tests.rs` — `download_asset` against two
  httpmock servers: 302 / 303 / 307 followed without `Authorization`,
  `Asset-Token` or cookie; a second 3xx refused, a third server untouched;
  the hop's refusal names status and host, never the signature S3 echoes
  in its error body; a silent asset store fails after the transfer idle
  timeout. Two pins already green: 301 / 308 not followed, a missing,
  relative, unparsable or userinfo `Location` refused before any hop.
- `tests/step_141_asset_download_redirect.rs` — the consumer's path
  (`Timeline::asset` → `download` / `save_to`) on a joined client with a
  seeded AES-256-CBC file: both decrypt through the redirect; other bytes
  behind the redirect fail `IntegrityCheck`. All three red with the live
  error, `BackendRejected("download_asset 302 Found: ")`.
- The Step 132 tests (login does not follow 302 / 307) stay as they are.
