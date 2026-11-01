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

**Option β.** Kupferdraht drives Wire's SP-side SSO endpoints:
`initiate_sso(server, code, success_redirect, error_redirect)` returns
the URL to open; `initiate_sso_by_email(server, email, …)` discovers the
team's SSO code first; `finalize_sso(server, cookie, device_label)`
completes the login from the captured `zuid` cookie and returns a
`PendingLogin`, from which `register_device` yields the `Client` as with a
password login. SAML signing and validation stay on Wire's server;
Kupferdraht holds no identity-provider configuration and no SAML code.
Options α and γ are out of scope — later extensions that would not
change the β API; α is revisited only if a headless or SCIM-only consumer
is confirmed.

## Consequences

### Positive

- One login pipeline for both authentication methods; every later
  change to registration, device reuse or session persistence reaches
  SSO logins automatically.
- The public surface is three methods on `ClientBuilder` and their
  manager-aware twins on `Kupferdraht`, mirrored over UniFFI and in the
  browser (`initiateSso`, `initiateSsoByEmail`, `finalizeSso`), and two
  CLI commands (`sso initiate`, `sso finalize`).
- No cryptographic or protocol surface was added: the only new HTTP
  calls are the initiate URL (built, not called, except `HEAD` to
  validate a code), `GET /sso/settings` and the cookie-to-token exchange
  that resume already used.

### Negative

- SSO login is interactive by construction: it needs a browser the user
  can see and a way for the application to receive the redirect. The CLI
  therefore splits the flow into two invocations with the cookie passed
  by hand (`docs/cli.md` "sso initiate" / "sso finalize").
- The flow has not been verified end to end against a Wire backend with
  a configured identity provider; four facts are still assumptions
  pinned by tests (`docs/wire/api-notes.md` §2b): the `/sso/*` paths are
  unversioned, the success redirect carries the cookie as a query
  parameter, `POST /access` mints the first token from a bare `zuid`, and
  `/sso/settings` is the right discovery source. Recorded as an open
  test-coverage gap (`open.md` gap-B.36).

### Neutral

- The Step 16 `sso_id` registration field stays as it is: unused by β,
  available if α is ever taken up.

## Implementation Notes

Shipped as Step 17 (sub-steps 17a initiate, 17b finalize, 17c email
discovery; merged 2026-07-09). The reuse seam is
`client/login_pipeline.rs::finalize` with `client/pending_login.rs`
(`password: Option`); the cookie-to-token exchange is
`wire/http/auth.rs` (`access_from_sso_cookie`). The CLI's default redirect
URLs use Kupferdraht's own scheme (`docs/cli.md`). Naming follows
matrix-rust-sdk's `ClientBuilder` verbs (ADR-0009); the Wire endpoint
names stay inside `wire::*`.

## References

- Builds on ADR-0009 (matrix-rust-sdk as the API model) for the shape of
  the surface and on the Step 24 `PendingLogin` split for the device-cap
  recovery.
- Related: Step 16 (`sso_id` registration plumbing, option α's
  groundwork); `docs/plan/archive/17-saml-sso.md` (the decision record of
  2026-07-08 with the α/β/γ framework); `docs/wire/api-notes.md` §2b (the
  endpoints and the open live-probe questions); `docs/cli.md` "sso
  initiate" / "sso finalize"; `docs/bindings.md` (FFI and browser mirrors).

## Revision History

- 2026-11-01: Created — the decision of 2026-07-08 lifted out of the
  archived Step 17 plan into an ADR (open.md §61).
