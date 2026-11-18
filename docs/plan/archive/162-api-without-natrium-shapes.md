# Step 162 — The API without Natrium's shapes: SSO, typing and the email change in matrix form

**Status: MERGED 2026-11-18** — `004c454` on `step/162-api-without-natrium-shapes`,
merged to `main` as PR #103 (`4b315d3`); locally fmt, 0 build warnings, clippy native
and wasm (both wasm lines), 950 library and step tests, the CLI's 32, the browser suite
(18 suites, 89 tests); CI green on the first run. Smoke 2026-11-18 on the reference
machine (Wire 5.38.9, Keycloak as SAML identity provider): 12 ok, 0 failed
(observations §107). Scope as shipped: `sso_login_url` / `login_with_sso_callback`,
`Room::typing_notice`, `request_email_change` across Rust, FFI, browser and CLI
(BREAKING); SSO works against Wire for the first time; the email change sends the
session cookie Wire wants. Retrospective: LEARNINGS 5.102.

Commissioned 2026-11-17 (Volker), from an audit of the public API's
provenance the same day. Three surfaces were built as a "drop-in" that "maps 1:1"
onto Natrium's facade during the Natrium migration of May and June 2026, and
Natrium wraps Kalium. The October provenance audit
(`docs/wire/provenance-audit-2026-10.md`) compared the tree with Kalium only, so
these three survived it. Whether Natrium's names are Kalium's own was not settled;
after this step it no longer matters for the API.

## Problem

1. **SSO (ADR-0043).** `ClientBuilder::initiate_sso(code, success_redirect,
   error_redirect)`, `initiate_sso_by_email(email, …)` and
   `finalize_sso(sso_cookie) -> PendingLogin`, their twins on `Kupferdraht`, the
   FFI and the browser (`initiateSso`, `initiateSsoByEmail`, `finalizeSso`), and the
   CLI's `sso initiate` / `sso finalize`. The sub-steps of Step 17 are named after
   Natrium's `ssoLoginWithCode`, `completeSSOLogin` and `ssoLogin(email)`
   (`docs/plan/archive/17-saml-sso.md`); ADR-0043 gives the one-to-one mapping as a
   reason. The verbs repeat Wire's endpoints (`/sso/initiate-login`,
   `/sso/finalize-login` — the latter is never called by a client), and the
   argument is Wire's `zuid` cookie, which the application has to pick out of the
   redirect itself. ADR-0043 says the naming "follows matrix-rust-sdk's
   `ClientBuilder` verbs"; it does not: matrix-rust-sdk has
   `get_sso_login_url(redirect_url, idp_id)` and
   `login_with_sso_callback(url_or_query)`, a callback the SDK parses itself.
2. **Typing.** `Timeline::send_typing_started()` / `send_typing_stopped()`,
   `Client::send_typing_started(room_id)` / `send_typing_stopped(room_id)` and the
   FFI mirror, chosen so that "natrium maps 1:1" its `sendTypingStarted` /
   `sendTypingStopped` (`docs/plan/archive/21-typing-indicators.md`). The browser
   already says `startTyping` / `stopTyping`. matrix-rust-sdk has
   `Room::typing_notice(typing: bool)`, with its own auto-stop.
3. **The email change.** `Client::update_email(email)`, FFI `update_email`, browser
   `updateEmail`, CLI `email-update`, added so that "natrium's
   `Session.updateEmail` maps 1:1" (`docs/plan/archive/22-update-email.md`); it
   breaks the house pattern of `set_display_name`. The call only starts Wire's
   verification: the address changes once the user follows the mail.

## Scope

1. **SSO in matrix form**: a URL for the browser and a login from the callback.
   `sso_login_url(redirect_url, provider)` with the provider as a code or an email
   address, and `login_with_sso_callback(callback_url) -> PendingLogin`, which reads
   the session cookie out of the callback itself; the error redirect's parameters
   become an error, not a URL the application has to read. On the manager, the
   FFI, the browser (where the cookie stays in the browser's jar) and the CLI
   (`sso url`, `sso callback`). `PendingLogin` and the device-cap recovery stay.
2. **Typing in matrix form**: `Room::typing_notice(typing: bool)` (and the
   manager-level twin the browser needs), the 11-second auto-stop unchanged; the
   browser's `startTyping` / `stopTyping` follow.
3. **The email change named for what it does**: `request_email_change(email)`
   (phase 1 may choose `set_email`), browser and CLI following.
4. **Docs.** ADR-0043 revised (the surface, the naming claim corrected, the
   provenance through Natrium stated) with its `rules/` file;
   `docs/wire/provenance-audit-2026-10.md` gains an addendum on the Natrium channel
   (Steps 17, 21 and 22) and its resolution; `docs/api-mapping-matrix-rust-sdk.md`,
   `docs/library.md`, `docs/bindings.md`, `docs/cli.md`, the tabs `index.d.ts`;
   `CHANGELOG.md` **BREAKING**. Released with Step 163 as 0.6.0.

### Phase 1

- **The callback.** Whether Wire's success redirect carries the cookie as a query
  parameter is still an assumption (`docs/wire/api-notes.md` §2b, open.md
  gap-B.36). Settle it live (the reference machine, if an identity provider is
  configured there) before the callback parser is written; otherwise the step keeps
  a cookie argument beside the URL form.
- **One redirect or two.** matrix-rust-sdk takes one redirect URL; Wire takes a
  success and an error redirect. Whether Kupferdraht derives the error redirect
  from the one URL (a marker parameter) or takes both.
- `request_email_change` or `set_email`.

### Phase 1 — results (2026-11-17/18)

- **The callback, settled from spar's source and live** (`docs/wire/kalium-readings.md`
  #30, `docs/observations.md` §107): the cookie was never a query parameter of its
  own. spar replaces a `$cookie` placeholder in the success URL with the whole
  `Set-Cookie` line, a `$label` in the error URL with the reason; redirect schemes
  must begin with `wire`, each URL at most 140 bytes as spar re-serializes it.
  Step 17's flow therefore worked only with the cookie read out of the browser by
  hand. The reference machine got Keycloak (team "SSO Referenz") for this step, so
  the smoke covers the login end to end.
- **One redirect URL** (Volker): `sso_login_url(redirect_url, provider)`;
  Kupferdraht derives the pair with parameters of its own, `credential=$cookie`
  and `reason=$label`. The CLI's default is `wire-kupferdraht://sso`; the URL comes
  back by prompt or stdin.
- **The provider is optional** — `None` takes the backend's default; the email
  lookup goes (under v14 it only ever found the default).
- **The browser** takes no redirect: `ssoLoginUrl(apiBaseUrl, provider?)` for a
  popup, `loginWithSsoCallback(…, message, …)` with the page's `postMessage` data;
  the cookie stays in the browser's jar (Volker).
- **`request_email_change`** (Volker).
- **The start URL names the home's cookie label** (ADR-0025), as a password login.
- **No Wire vocabulary in Kupferdraht's own names, words, tests or functions**
  (Volker): Wire's placeholders, page messages and endpoint names stay in `wire/`.
- **The CLI's `typing`** sends the start and, `--seconds` later, the stop in one
  process: a `false` sends only after a `true` of the same process, as in
  matrix-rust-sdk.

### Found on the way

- **The email change never reached Wire** (smoke, 2026-11-18): `PUT
  /access/self/email` wants the `zuid` cookie beside the token; Kupferdraht sent
  only the token since Step 22 (`403` "Missing cookie"). Fixed here, test first.
- **The redirect's length** counts as spar re-serializes it (`$` as `%24`), not
  as written — the check measures that way.
- **The browser's old `initiateSso`** could never have worked: it passed an empty
  WebSocket URL, which the scheme check refuses. The start URL no longer checks
  the socket.
- **The CLI's SSO commands ignored `login --ca-file`'s authorities**; they read
  them now and take `--ca-file` themselves.
- **An SSO user cannot delete a device through the CLI**: `logout --purge-device`
  and `device-delete` want a non-empty password, and an SSO user has none. Not
  fixed here.

## Out of scope

- Wire vocabulary elsewhere in the public API: Step 163.
- SSO variants α and γ of ADR-0043.

## TDD sketch

- `sso_login_url` for a code and for an email address builds the same URL the
  current `initiate_sso` does (httpmock for the code probe and the settings lookup).
- `login_with_sso_callback` with a success callback yields a `PendingLogin`; with
  an error callback the mapped error; with a callback that carries no cookie
  `InvalidInput`.
- `typing_notice(true)` sends `started`, `typing_notice(false)` `stopped`, the
  auto-stop fires as before.
- `request_email_change` sends the same request `update_email` does.
- FFI and browser parity tests name the new symbols; the old names are gone.
- Smoke: login, typing and the email request on the reference machine; SSO only if
  an identity provider is configured there.
