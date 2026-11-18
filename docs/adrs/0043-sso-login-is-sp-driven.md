# ADR-0043: Drive Wire's SSO login from the client (SP-driven, option β)

**Status:** Accepted
**Date:** 2026-11-01 (decision taken 2026-07-08, Step 17; recorded as an ADR on 2026-11-01)
**Deciders:** Volker Zöpfel

## Context

Wire authenticates SSO users through SAML. The SAML exchange itself —
building the authentication request, validating the identity provider's
signed response — runs on Wire's server (the `spar` service); a client
takes part only at the edges: it starts the login by opening a Wire URL
for the configured identity provider, and it receives the result as the
`zuid` refresh cookie that Wire sets when the identity provider's response
has been accepted, followed by a redirect to a URL the client named. The
endpoints are `GET /sso/initiate-login/{code}` (with `success_redirect`
and `error_redirect`), `POST /sso/finalize-login` (the identity provider's
callback — never called by a client) and `GET /sso/settings` (the team's
default SSO code, for discovery by email domain). They are documented in
`docs/wire/api-notes.md` §2b.

Kupferdraht's password login already ran as a pipeline whose finalisation
is authentication-agnostic: `LoginPipeline::finalize` takes a
`LoginOutcome` and runs the device registration, the key-package upload
and the session persistence regardless of where the outcome came from;
`PendingLogin.password` was already an `Option`. The first consumer of
Kupferdraht's SSO surface was an embedding application whose own SSO API
has the shape "give me a URL to open; later, here is the cookie the
redirect carried; finish the login" (`ssoLogin(email)` /
`ssoLoginWithCode(code)` returning an `authorizationUrl`, then
`completeSSOLogin(cookie)`).

Three ways to bring SSO users into Kupferdraht were on the table in
July 2026; the choice shapes the public API (`ClientBuilder`, the
`Kupferdraht` manager, the FFI and browser mirrors, the CLI) and decides
whether any SAML handling enters the library.

## Options Considered

### Option α: the embedding backend owns SAML; Kupferdraht only plumbs `sso_id`

The embedding application's backend authenticates the user against the
identity provider itself and hands Kupferdraht a `UserSSOId` for
`POST /register` — the Step 16 registration plumbing already carries the
field.

**Pros:**
- No SSO code in Kupferdraht beyond the existing field; roughly test
  scaffolding only.
- A headless or server-side consumer needs no browser.

**Cons:**
- Only covers account creation, not the login of an existing SSO user;
  the session still has to come from Wire's SSO endpoints.
- Makes the embedding backend a SAML service provider in Wire's place —
  Wire's `spar` is the registered service provider at the identity
  provider, so this does not work without Wire's cooperation.

### Option β: Kupferdraht drives Wire's SSO endpoints (SP-driven, interactive)

Kupferdraht builds the `initiate-login` URL for a given SSO code (or
discovers the code from an email domain through `/sso/settings`), hands
it to the application to open in a browser, and finishes the login from
the `zuid` cookie the application captures off the success redirect:
mint an access token from the cookie (`POST /access`), read `/self`, and
run the existing pipeline from a `PendingLogin` without a password.

**Pros:**
- Matches Wire's actual SSO contract; no SAML in Kupferdraht, only
  redirect plumbing and the existing post-login pipeline.
- Matches the shape the first consumer already exposed, so the binding
  is a one-to-one mapping.
- Reuses `PendingLogin`: the device-cap recovery (`TooManyDevices` →
  `devices` / `delete_device` → `register_device`) works for SSO logins
  unchanged.

**Cons:**
- Needs a user-facing browser at login time; a headless consumer cannot
  use it.
- The application has to capture a cookie off a redirect — a platform
  detail (custom URL scheme, in-app browser) that Kupferdraht cannot do
  for it.
- About 200–400 lines plus the FFI and browser mirrors.

### Option γ: SCIM provisioning, normal password login

Users are provisioned into the Wire team by SCIM and log in with a
password; no SSO code in Kupferdraht, documentation only.

**Pros:**
- Nothing to build.

**Cons:**
- Not SSO: the identity provider does not authenticate the session, and
  a team that enforces SSO has no password to log in with.

## Decision

**Option β.** Kupferdraht drives Wire's SP-side SSO endpoints, in
matrix-rust-sdk's two-call shape since Step 162:
`sso_login_url(redirect_url, provider)` returns the URL to open — the
provider by its code, `None` for the backend's default; with a redirect URL
the browser comes back to it, without one (the browser) Wire's page reports
to the window that opened it. `login_with_sso_callback(callback_url)` reads
the outcome out of the URL the browser came back to — the session cookie,
or the reason of a refusal as `BackendRejected` — and returns a
`PendingLogin`, from which `register_device` yields the `Client` as with a
password login. SAML signing and validation stay on Wire's server;
Kupferdraht holds no identity-provider configuration and no SAML code.
Options α and γ are out of scope — later extensions that would not
change the β API; α is revisited only if a headless or SCIM-only consumer
is confirmed.

**Revised 2026-11-18 (Step 162).** Step 17 shipped β as
`initiate_sso(code, success_redirect, error_redirect)`,
`initiate_sso_by_email(email, …)` and `finalize_sso(cookie)`: the shape of the
first consumer's facade (Natrium, which wraps Kalium — `ssoLoginWithCode`,
`ssoLogin(email)`, `completeSSOLogin(cookie)`), which this ADR gave as a
reason. Its claim that the naming followed matrix-rust-sdk's verbs was wrong:
matrix-rust-sdk has `get_sso_login_url(redirect_url, idp_id)` and
`login_with_sso_callback(callback_url)`, which parses the callback itself.
Step 162 adopts that shape. One redirect URL: Kupferdraht derives Wire's
success and error redirects from it, each with a query parameter of its own
(`credential`, `reason`) that carries Wire's placeholder (`$cookie`,
`$label`), so the application never picks a cookie out of a redirect. The
email lookup goes: under v14 the email never selected the provider, only
the backend's default did, which `provider: None` now names. The start URL
also carries the home's cookie label (ADR-0025), as a password login does.

## Consequences

### Positive

- One login pipeline for both authentication methods; every later
  change to registration, device reuse or session persistence reaches
  SSO logins automatically.
- The public surface is two methods on `ClientBuilder` and their
  manager-aware twins on `Kupferdraht`, mirrored over UniFFI and in the
  browser (`ssoLoginUrl`, `loginWithSsoCallback`, which takes the Wire
  page's `postMessage` data; the cookie stays in the browser's jar), and two
  CLI commands (`sso url`, `sso callback`).
- No cryptographic or protocol surface was added: the only new HTTP
  calls are the initiate URL (built, not called, except `HEAD` to
  validate a code), `GET /sso/settings` and the cookie-to-token exchange
  that resume already used.
- Live-confirmed end to end on 2026-11-18 against a Wire 5.38.9 backend
  with Keycloak as identity provider (`docs/observations.md` §107): the
  `/sso/*` paths are unversioned, the success redirect carries the whole
  `Set-Cookie` line percent-encoded in Kupferdraht's parameter,
  `POST /access` mints the first token from it, and `/sso/settings`
  answers the default (`null` when none is set).

### Negative

- SSO login is interactive by construction: it needs a browser the user
  can see and a way for the application to receive the redirect. The CLI
  therefore splits the flow into two invocations with the callback URL
  passed by hand (`docs/cli.md` "sso url" / "sso callback").
- Wire accepts only redirect schemes beginning with `wire` and redirect
  URLs of at most 140 bytes as it re-serializes them
  (`docs/wire/kalium-readings.md` #30), so an application registers a
  `wire…` scheme for its callback. Step 17's CLI default
  (`kupferdraht://sso/done`) was refused (`invalid-schema`), and its cookie
  never travelled in the redirect, only in the browser's jar: before
  Step 162 the flow worked only with the cookie read out of the browser by
  hand.

### Neutral

- The Step 16 `sso_id` registration field stays as it is: unused by β,
  available if α is ever taken up.

## Implementation Notes

Shipped as Step 17 (sub-steps 17a initiate, 17b finalize, 17c email
discovery; merged 2026-07-09), reshaped as Step 162. The reuse seam is
`client/login_pipeline.rs::finalize` with `client/pending_login.rs`
(`password: Option`); the public pair is `client/sso_login.rs`; the redirect
pair, the callback and the page message are read in `wire/sso.rs`, the start
URL is built in `wire/http/sso.rs`; the cookie-to-token exchange is
`wire/http/auth.rs` (`access_from_sso_cookie`). The CLI's default redirect
is `wire-kupferdraht://sso` (`docs/cli.md`). Naming follows
matrix-rust-sdk (ADR-0009); Wire's endpoint names, placeholders and page
messages stay inside `wire::*`.

## References

- Builds on ADR-0009 (matrix-rust-sdk as the API model) for the shape of
  the surface and on the Step 24 `PendingLogin` split for the device-cap
  recovery.
- Related: Step 16 (`sso_id` registration plumbing, option α's
  groundwork); `docs/plan/archive/17-saml-sso.md` (the decision record of
  2026-07-08 with the α/β/γ framework); `docs/wire/api-notes.md` §2b (the
  endpoints); `docs/wire/kalium-readings.md` #30 (how Wire hands the
  outcome back); `docs/observations.md` §107 (the live check);
  `docs/wire/provenance-audit-2026-10.md` (the addendum on the Natrium
  channel); `docs/cli.md` "sso url" / "sso callback"; `docs/bindings.md`
  (FFI and browser mirrors).

## Revision History

- 2026-11-01: Created — the decision of 2026-07-08 lifted out of the
  archived Step 17 plan into an ADR (open.md §61).
- 2026-11-18: Step 162 — the surface reshaped to matrix-rust-sdk's
  `sso_login_url` / `login_with_sso_callback`, the naming claim corrected,
  the provenance through Natrium stated, the redirect rules and the live
  check recorded; option β unchanged.
