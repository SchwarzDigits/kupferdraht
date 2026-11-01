# SSO login is SP-driven: Kupferdraht drives Wire's SSO endpoints, holds no SAML

Condensed from `docs/adrs/0043-sso-login-is-sp-driven.md`.

- **SSO login is `initiate_sso` / `initiate_sso_by_email` → the app opens
  the URL → `finalize_sso(cookie)` → `PendingLogin` → `register_device`.**
  The SAML exchange stays on Wire's server; Kupferdraht builds the
  initiate URL, discovers the SSO code through `/sso/settings`, and turns
  the captured `zuid` cookie into a session through the existing login
  pipeline.
- **Don't** put SAML parsing, signing or identity-provider configuration
  into Kupferdraht, and **don't** call `POST /sso/finalize-login` — that
  is the identity provider's callback, the client only catches the
  redirect.
- **Don't** add a second login pipeline for SSO: `LoginPipeline::finalize`
  is authentication-agnostic, `PendingLogin.password` is `None` for SSO,
  and the device-cap recovery works unchanged.
- Capturing the cookie off the redirect is the application's job
  (custom URL scheme, in-app browser); the CLI passes it by hand.
- The backend-driven (α) and SCIM-only (γ) variants are out of scope
  until a headless or SCIM-only consumer is confirmed; the Step 16
  `sso_id` field stays for α.
