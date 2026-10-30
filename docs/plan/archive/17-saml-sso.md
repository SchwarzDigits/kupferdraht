# Step 17 — SSO login (interactive / SP-driven, option β)

> **Status:** MERGED to `main` — `83167c4`, PR #45 (merge `3e1f109`,
> 2026-07-09). Scope decided 2026-07-08; implements **option β**: Kupferdraht
> drives Wire's SP-side SSO endpoints (`/sso/initiate-login/{idp}` + the
> `zuid` cookie from `/sso/finalize-login`), returning a redirect URL the
> consumer opens and finalising the session from the returned cookie —
> 17a/17b/17c (lib + CLI + FFI + wasm + docs). This matches the SSO API
> Natrium already exposes (`ssoLogin`/`ssoLoginWithCode` → `authorizationUrl`,
> then `completeSSOLogin(cookie)`). Options **α** (backend-driven / "SAML in the
> background" / headless) and **γ** (SCIM-only) stay **out of scope** —
> later extensions that do not change the β API. Prior decision-framework
> (α/β/γ sketches) archived at the bottom. Live verification pending (as of
> 2026-07-16) — at fold time this file's own status still read "implemented,
> pending E2E … not yet committed — verifying end-to-end with natrium + the
> test app before the commit", stale against its own commit/merge, and the
> "Open questions (confirm via live-probe)" section below is unresolved.

---

## Decision (2026-07-08)

- **β chosen.** Natrium's existing SSO surface is exactly the SP-driven shape:
  `ssoLogin(email)`/`ssoLoginWithCode(code)` return an `authorizationUrl` the app
  opens in a browser, then `completeSSOLogin(cookie)` finishes login. SAML XML
  signing/validation stays on Wire's server; Kupferdraht only does redirect plumbing +
  reuses its existing post-login pipeline.
- **α/γ out of scope.** α reuses the Step-16 `sso_id` `/register` plumbing (thin,
  no β-API change); γ needs no SSO code (normal password login). Revisit when a
  headless / SCIM-only consumer is confirmed.

## Flow (β)

```
ssoLoginWithCode(code) ──► initiate: build /sso/initiate-login/{code}?success_redirect&error_redirect
ssoLogin(email)        ──► discover code (GET /sso/settings) ─┘        │
                                          authorizationUrl ◄───────────┘
app opens authorizationUrl → user auths at IdP → Wire POST /sso/finalize-login
      → Set-Cookie zuid → 302 to success_redirect ( <success_redirect>?cookie=… )
                                       │  (failure → error_redirect?errorCode=…)
completeSSOLogin(cookie) ─► finalize: zuid → POST /access (mint access token) → GET /self
                                    → PendingLogin(password: None) → register_device → Client
```

Wire endpoints (v14 swagger, `reference/wire.swagger.json`):
- `GET|HEAD /sso/initiate-login/{idp}` — `{idp}` = SSO code (path); optional query
  `success_redirect`, `error_redirect`. `HEAD` validates a code without initiating.
- `POST /sso/finalize-login` — IdP→Wire callback (sets `zuid`, redirects to
  `success_redirect`); **the client never calls this** — it catches the redirect.
- `GET /sso/settings` — the team's default SSO code (single-IdP discovery).

**Reuse seam** (from the login-pipeline map): `LoginPipeline::finalize` is
auth-agnostic — it takes a `LoginOutcome` regardless of origin. SSO skips
`authenticate()` (password), seeds `LoginOutcome` from the `zuid` cookie, and runs
phases 3–6 unchanged. `PendingLogin.password` is already `Option` (`None` for SSO).

## Sub-steps (each its own commit on `sso`: lib + CLI + FFI parity + docs)

### 17a — SSO initiate (`ssoLoginWithCode` half)
- **HTTP** (`wire/http/sso.rs`, new):
  - `sso_initiate_login_url(code, success_redirect, error_redirect) -> String` —
    build `/sso/initiate-login/{code}?success_redirect&error_redirect` (pure).
  - `sso_code_exists(code) -> bool` — `HEAD /sso/initiate-login/{code}` (200→true,
    404→false).
- **Client/FFI/CLI**: `Kupferdraht::initiate_sso(code, redirects…) -> String`,
  validating format + existence (404 → mapped error); FFI mirror + `kupferdraht-cli sso initiate`.
- **TDD (red first):**
  - [ ] `sso_initiate_login_url` builds the unversioned `/sso/initiate-login/{code}`
    path with `success_redirect`/`error_redirect` as url-encoded query pairs.
  - [ ] `sso_code_exists` → `true` on HEAD 200.
  - [ ] `sso_code_exists` → `false` on HEAD 404.
  - [ ] non-2xx/non-404 HEAD → `BackendRejected` (label preserved) / `Network`.
  - [ ] empty code → `InvalidInput` (precondition).

### 17b — SSO finalize (`completeSSOLogin`)
- **HTTP**: mint the first access token from the `zuid` cookie (`POST /access`,
  no `client_id`) → `LoginOutcome`; `GET /self` → `SelfUser`.
- **Pipeline**: `finalize_sso(cookie) -> PendingLogin` — seed `LoginOutcome`,
  skip `authenticate()`, reuse phases 3–6. `register_device()` then works as-is.
- **TDD (red first):**
  - [ ] `zuid` cookie → `POST /access` mints access token (no `client_id`).
  - [ ] rotated `zuid` from the `/access` Set-Cookie is what gets persisted.
  - [ ] `GET /self` populates `SelfUser`; assembled `PendingLogin.password == None`.
  - [ ] invalid/expired cookie → `SessionExpired` / `Unauthorized`.

### 17c — Email discovery (`ssoLogin(email)`)
- **HTTP**: `GET /sso/settings` → default code; map absent/none → SSO-not-available.
- **Client/FFI/CLI**: `Kupferdraht::initiate_sso_by_email(email)` = discover code → 17a.
- **TDD (red first):**
  - [ ] `/sso/settings` with a default code → that code drives initiate.
  - [ ] `/sso/settings` with no default → error mapped to "SSO not available".
  - [ ] (live-probe) per-domain vs. team-default discovery semantics.

## Open questions (confirm via live-probe; assumptions documented in tests)

1. **Versioning:** are `/sso/*` endpoints unversioned (spar service, assumed here)
   or under `/v14/…`? Tests assert **unversioned**; adjust on live-probe.
2. **Cookie hand-off:** does the `success_redirect` actually carry the `zuid` as a
   query param (`?cookie=…`, per Natrium docs) or as a `Set-Cookie` the app reads?
   Impacts whether the app or Kupferdraht extracts it.
3. **Access without client:** confirm `POST /access` (no `client_id`) mints the
   first token from a bare `zuid` (as the wasm-W6 cookie flow suggests).
4. **Discovery source:** `/sso/settings` team-default vs. per-domain enterprise
   discovery (domain-registration endpoints).

Record answers in `docs/wire/api-notes.md` §"Step 17 discovery".

---

## Archive — the α/β/γ decision framework (pre-2026-07-08)

Retained for context. β is chosen; α/γ are out-of-scope extensions.

- **α — Natrium-backend owns SAML; Kupferdraht just plumbs `sso_id`.** Backend
  authenticates against the IdP, hands Kupferdraht a `UserSSOId` for `POST /register`.
  Step-16 plumbing already exists; ~50–100 LoC test scaffolding. No SAML in Kupferdraht.
- **β — Kupferdraht drives Wire's `/sso/initiate-login` + `/sso/finalize-login`.**
  SP-driven redirect flow; ~200–400 LoC + FFI. **← chosen.**
- **γ — SCIM provisioning (external).** Users log in via the normal `login` path;
  no new Kupferdraht code. Documentation-only.

## Cross-references
- Natrium plan: `natrium-ion/natrium-core/docs/sso-login-plan.md`.
- Natrium stubs: `Natrium.kt:147-163`; types `session/SessionResults.kt`.
- Reuse seam: `client/login_pipeline.rs` (`finalize`), `client/pending_login.rs`
  (`password: Option`), `wire/http/auth.rs` (`login`/`rebind_access_token`, `zuid`).
- Step 16 `sso_id` plumbing (α groundwork): `wire/dto.rs`, `wire/http/registration.rs`.
