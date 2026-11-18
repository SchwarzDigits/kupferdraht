# SSO login is SP-driven: Kupferdraht drives Wire's SSO endpoints, holds no SAML

Condensed from `docs/adrs/0043-sso-login-is-sp-driven.md`.

- **SSO login is `sso_login_url(redirect_url, provider)` → the app opens
  the URL → `login_with_sso_callback(callback_url)` → `PendingLogin` →
  `register_device`** (matrix-rust-sdk's shape, Step 162). The SAML
  exchange stays on Wire's server; Kupferdraht builds the start URL (the
  provider validated, or the backend's default from `/sso/settings`),
  derives Wire's redirect pair from the one URL, and reads the session
  cookie or the refusal out of the callback itself.
- **Do** refuse a redirect Wire would refuse — scheme not beginning with
  `wire`, over 140 bytes as Wire re-serializes it — before any request.
- **Don't** put SAML parsing, signing or identity-provider configuration
  into Kupferdraht, and **don't** call `POST /sso/finalize-login` — that
  is the identity provider's callback, the client only catches the
  redirect.
- **Don't** add a second login pipeline for SSO: `LoginPipeline::finalize`
  is authentication-agnostic, `PendingLogin.password` is `None` for SSO,
  and the device-cap recovery works unchanged.
- **Don't** let Wire's vocabulary out of `wire/`: its placeholders, page
  messages and endpoint names stay there; the public names are
  matrix-rust-sdk's.
- Receiving the callback is the application's job (a `wire…` URL scheme,
  an in-app browser, the popup's `postMessage` in the browser); the CLI
  passes the URL by hand.
- The backend-driven (α) and SCIM-only (γ) variants are out of scope
  until a headless or SCIM-only consumer is confirmed; the Step 16
  `sso_id` field stays for α.
