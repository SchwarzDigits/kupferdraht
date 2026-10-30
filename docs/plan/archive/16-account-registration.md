# Step 16 — Account registration (personal + SSO plumbing)

> **Status:** merged — 16a–16d merged 2026-05-22; library + CLI end-to-end. Live-smoke deferred (register creates real accounts). CHANGELOG.md §45. Sub-steps: 16a `a2f5b9d` · 16b `7464cb8` · 16c `88c9362` · 16d `81102d5`. Archived from PLAN.md.
>
> **Sub-steps:** 16a (`POST /register` HTTP layer — personal flow with
> `name + email + password`, `UserSSOId` field plumbed but unused
> here), 16b (activation flow — `POST /activate` with `email_code`,
> resend via `POST /activate/send`), 16c
> (`ClientBuilder::register(...)` → `PendingClient` → `.activate(code)`
> → `Arc<Client>` builder shape), 16d (CLI `kupferdraht-cli register` +
> `kupferdraht-cli activate`).
>
> **Branch / commit prefix:** `step/16*-*`, `feat(step-16*): …`,
> `chore(step-16*): …`.

---

## Step 16 — Account registration

Kupferdraht's auth surface has so far covered login (existing account →
session) and device registration (`POST /v14/clients` for the
authenticated user). Account creation — turning a fresh person
into a Wire user — has been out of scope; first-pass focus was
the messaging loop. Natrium (Kupferdraht's KMP consumer) creates Wire
users in production today and needs a Kupferdraht-side surface for it.

Step 16 introduces the personal-account registration flow:
`POST /register` with `email + password + name`, followed by an
activation step that converts the pending account into an active
one. The Wire endpoint also accepts a `sso_id` field — Step 16
**plumbs** that field through but does not implement any SAML or
SSO-specific behaviour. The full SAML workflow lands in Step 17
(separately scoped because the right shape depends on Natrium's
actual SAML flow — see Step 17's plan doc for the α / β / γ
decision tree).

### matrix-rust-sdk analog

- `Client::register(request) async -> Result<Session>` — exists in
  matrix-rust-sdk for fresh account creation against the user-API
  endpoint. Kupferdraht's `ClientBuilder::register(...)` matches the
  shape modulo the builder-vs-Client placement, consistent with
  how Kupferdraht handles `login` (on `ClientBuilder`, not `Client`).
- Matrix has no separate activation step in the same shape —
  Matrix' `/register` returns the session directly. Wire splits
  the two: registration creates a pending user, activation
  (separate endpoint, separate user input — the email-delivered
  code) converts it. Step 16's `PendingClient` zwischenstand has
  no direct matrix analog and is recorded as a Wire-extension in
  the api-mapping doc.

### Sub-steps

| Sub-step | Scope | Wire API | Value |
|---|---|---|---|
| 16a | `HttpClient::register_user(NewUser { name, email, password, sso_id?, ... }) async -> Result<RegisterResponse>`. Wire endpoint accepts a single `NewUser` body whose only required field is `name`; Kupferdraht's personal flow always supplies `email + password + name`. `sso_id: Option<UserSSOId>` is exposed as an optional input but not wired anywhere by Step 16 — Step 17 picks it up. Response carries the new user's id + activation state. | `POST /register` (unversioned) | high |
| 16b | `HttpClient::activate(email_code) async -> Result<()>` plus `HttpClient::request_activation_resend(email)` for test/manual repeats. Activation converts the pending account into an active one — required before any subsequent `login`. | `POST /activate`, `POST /activate/send` | high |
| 16c | `ClientBuilder::register(name, email, password) -> Result<PendingClient>`; `PendingClient::activate(code) -> Result<Arc<Client>>`. The two-step shape mirrors `LockedClient`/`unlock` from Step 7c — caller can hold the `PendingClient` across UI states (e.g. "we sent you a code, enter it"). Single-shot convenience `ClientBuilder::register_and_activate(name, email, password, code)` provided too for non-UI consumers (test runners). | (composes 16a + 16b) | high |
| 16d | `kupferdraht-cli register --email <e> --name <n>` (password via `--password-stdin` or interactive prompt, same shape as `login`). `kupferdraht-cli activate --code <c>` as the second step. Single-shot `kupferdraht-cli register --email <e> --name <n> --code <c>` for staging-script use. | (composes) | medium |

### Public API

```rust
// 16a — HTTP layer (kupferdraht-core/src/wire/http/registration.rs)
impl HttpClient {
    pub async fn register_user(
        &self,
        body: NewUserRequest,
    ) -> Result<RegisterResponse, KupferdrahtError>;

    pub async fn activate(
        &self,
        email_code: &str,
    ) -> Result<(), KupferdrahtError>;

    pub async fn request_activation_resend(
        &self,
        email: &str,
    ) -> Result<(), KupferdrahtError>;
}

pub struct NewUserRequest {
    pub name: String,
    pub email: Option<String>,
    pub password: Option<String>,
    pub sso_id: Option<UserSSOId>,  // 16-plumbed, Step-17-consumed
    pub team_code: Option<String>,  // out of scope for 16; deferred
    pub locale: Option<String>,     // optional, default "en"
    pub label: Option<String>,      // device label hint, default "kupferdraht-cli"
}

pub struct UserSSOId {
    pub subject: Option<String>,
    pub scim_external_id: Option<String>,
    pub tenant: Option<String>,
}

pub struct RegisterResponse {
    pub user_id: String,
    pub status: AccountStatus,   // expected: "pending-activation" after register
    pub email: Option<String>,
    pub name: String,
}

// 16c — Builder (kupferdraht-core/src/client/builder.rs extension)
impl ClientBuilder {
    pub async fn register(
        self,
        name: String,
        email: String,
        password: String,
    ) -> Result<Arc<PendingClient>, KupferdrahtError>;

    pub async fn register_and_activate(
        self,
        name: String,
        email: String,
        password: String,
        email_code: String,
    ) -> Result<Arc<Client>, KupferdrahtError>;
}

pub struct PendingClient { /* opaque */ }
impl PendingClient {
    pub fn user_id(&self) -> &str;
    pub fn email(&self) -> &str;

    pub async fn activate(
        self: Arc<Self>,
        email_code: &str,
    ) -> Result<Arc<Client>, KupferdrahtError>;

    pub async fn resend_activation_email(
        &self,
    ) -> Result<(), KupferdrahtError>;
}
```

### Wire Message Types

None — Step 16 is the auth/identity surface, not the messaging
surface. No `GenericMessage` involvement.

### Implementation notes

**16a — `POST /register` is unversioned, like `/login`.**
The full Wire path is `https://prod-nginz-https.wire.com/register`
(no `/v14/` prefix), consistent with the other unversioned auth
endpoints (`/login`, `/access/logout`). Response is HTTP 201 with
a JSON body matching `User` schema, plus an `AccountStatus` field
that should read `"pending-activation"` after a fresh register
with an email but no `email_code`.

Wire's full `NewUser` schema (per `reference/wire.swagger.json`
component `NewUser`) carries 19 optional fields. Step 16 wires
only the personal-flow subset (`name + email + password + locale
+ label`) and threads `sso_id` through for Step 17. Everything
else (`team`, `team_code`, `picture`, `assets`, `managed_by`,
`accent_id`, `invitation_code`, etc.) is **deferred** — flagged
in the api-mapping doc as Wire-extension fields not yet exposed.

**16a — `sso_id` plumbing without SAML implementation.** The
`UserSSOId` struct is exposed as `Option<UserSSOId>` on
`NewUserRequest`. Step 16 forwards it verbatim to Wire's
`POST /register` body if present. Step 16 does NOT:
- implement any SAML XML / signature handling,
- expose `/sso/initiate-login` / `/sso/finalize-login` (those
  are Step 17 territory),
- exercise the SSO path in live-smoke (no SSO IdP configured for
  Kupferdraht's staging tests).
The plumbing is here so Step 17 can wire the field through
without re-touching `register_user`.

**16b — Activation endpoint.** `POST /activate` body is
`{ "code": "<email-delivered code>" }`; response 200 (no body) on
success, `404 invalid-code` on wrong/expired code. `POST /activate/send`
body is `{ "email": "<email-of-pending-account>" }`; resends the
activation email. Both are unversioned.

**16c — `PendingClient` follows the `LockedClient` pattern.**
Step 7c introduced `LockedClient` as an intermediate state between
"opened from persistent storage but DEK still locked" and "fully
unlocked Client". The same shape applies to register-then-activate:
`register` returns a `PendingClient` that holds the pending
account's id and email but has no MLS state, no session token —
just the registration handle. `activate(code)` completes the
loop by calling `POST /activate` and then immediately running the
existing login pipeline (since the account now has credentials
that can produce an access token via `POST /login`).

The `register_and_activate` convenience does both in one call —
useful for non-interactive consumers like test runners and CLI
scripts that have the code in hand. It returns `Arc<Client>`
directly, skipping the `PendingClient` intermediate.

**16c — No `LockedClient`-style serialisation.** `PendingClient`
is **in-memory only**; it does not persist across process restarts.
A consumer that crashes after `register` but before `activate`
must re-call `register` (Wire returns `409 key-exists` then; the
consumer should fall back to `request_activation_resend(email)`
and try a fresh code).

**16d — CLI password handling matches `login`.** `--password-stdin`
reads from stdin (same flag name as `login` for muscle-memory);
without the flag, the CLI uses `rpassword::prompt_password`
interactively. Activation code is similarly read either via
`--code <c>` or prompted interactively. Test scripts use
`--code` directly.

### Live-smoke posture

**Live-smoke against Wires staging only**, not production.
`https://staging-nginz-https.zinfra.io/register` (and friends) is
the only sanctioned target for Step 16's automated tests. Reasons:

- Production registrations create **real** Wire accounts that
  count against quotas, send real activation emails, and
  pollute the user namespace.
- Staging account cleanup is loose enough that test accounts
  do not need explicit teardown (compared to production where
  each account would need a `DELETE /v14/self`).
- The activation email at staging is delivered to a Kupferdraht-team
  mailbox (or whatever staging-side test-mode Wire offers — see
  Open Question 1 below) so the live-smoke can read the code.

CI configuration must hard-block Step 16 tests against any
`*.wire.com` URL — an explicit `ServerConfig` guard or a feature
flag is appropriate.

### Audit closures

None. Step 16 is new functionality, not audit cleanup. No entries
in `v1-api-ergonomics-audit.md` or `v1-known-simplifications.md`
correspond to register / activate (the audit was about polishing
the existing surface, not adding new surface).

### Out of scope — explicitly _not_ in Step 16

- **SAML / SSO flow** (Wires `/sso/initiate-login`,
  `/sso/finalize-login`, `/sso/metadata`) — Step 17 territory.
- **Team creation** at register time — `NewUser.team` and
  `NewUser.team_code` not exposed; consumer who wants to create
  a Wire team uses Wires web UI (or a future Step).
- **Invitation-code flow** (`NewUser.invitation_code`) — when an
  existing team admin invites a user, the invitee submits the
  code at register time. Deferred to a follow-up sub-step or
  Step if Natrium needs it.
- **SCIM provisioning** (`managed_by = "scim"`) — out of Kupferdraht
  scope entirely; SCIM happens server-side between an external
  identity system and Wire, Kupferdraht is not in the loop. Kupferdraht
  picks up SCIM-provisioned users at login time.
- **`/provider/register`** for Wire bot / service-provider
  accounts — different scope, different Step if it ever lands.
- **Account deletion** (`DELETE /v14/self`) — symmetric inverse
  of register, but a separate concern. Out of scope here.
- **Self-profile editing post-registration** beyond what already
  exists (`Client::set_display_name`, `Client::set_handle`).

### TDD starter tests

16a:
- `register_user({ name, email, password })` POSTs to `/register`
  with the expected JSON body and parses the response into
  `RegisterResponse`.
- `register_user` on a duplicate email returns `409 key-exists`
  surfaced as `KupferdrahtError::BackendRejected` with the Wire label
  preserved.
- `register_user` with `sso_id: None` produces a body that omits
  the `sso_id` field entirely (not `null`).
- `register_user` with `sso_id: Some(...)` produces a body that
  includes `sso_id: { subject, scim_external_id, tenant }` —
  pure plumbing verification; no behaviour beyond pass-through.
- Response carries `AccountStatus::PendingActivation` (or
  whatever the post-register status decodes to — to be confirmed
  by live-probe).

16b:
- `activate("good-code")` POSTs `{"code": "good-code"}` to
  `/activate` and returns `Ok(())` on 200.
- `activate("bad-code")` returns `KupferdrahtError::BackendRejected`
  with Wire label `invalid-code`.
- `request_activation_resend("user@example.com")` POSTs
  `{"email": "user@example.com"}` to `/activate/send` and
  returns `Ok(())` on 200.

16c:
- `ClientBuilder::new().server(...).register(name, email, pw)`
  returns `Arc<PendingClient>` with `.user_id()` and `.email()`
  reflecting the input.
- `pending.activate("good-code")` returns `Arc<Client>` whose
  `user_id()` matches `pending.user_id()` byte-for-byte.
- `pending.activate("bad-code")` returns an error but leaves
  `pending` re-usable for a retry (no consumed-self semantics
  on bad-code path; `Arc<PendingClient>` is shared).
- `ClientBuilder::register_and_activate(...)` returns
  `Arc<Client>` directly on success; on error at either step
  surfaces the failing step's error variant.

16d:
- `kupferdraht-cli register --email <e> --name <n> --password-stdin`
  reads password from stdin and prints the pending user's id on
  success.
- `kupferdraht-cli activate --code <c>` activates the most-recently-
  registered pending account (loads from a temp file persisted
  by `register`, or refuses if absent and asks for `--email`
  + `--code` together).
- The single-shot `kupferdraht-cli register --email <e> --name <n>
  --code <c>` runs both steps and prints the final user_id.

### CLI commands

```
kupferdraht-cli register --email <e> --name <n>                    # interactive password
kupferdraht-cli register --email <e> --name <n> --password-stdin   # stdin password
kupferdraht-cli activate --code <c>                                # activate pending account
kupferdraht-cli register --email <e> --name <n> --code <c>         # single-shot (script use)
```

Flat subcommand convention per Kupferdraht CLI style (audit §7.3
subgroup pass remains a separate global restructure).

### Open questions

1. **Activation code delivery in staging.** Wire production
   delivers the activation code via real e-mail. Staging may
   have a test-mode endpoint that returns the code directly in
   the register response (e.g. a hidden field, or a separate
   `GET /v14/i/activation-code/{email}` internal endpoint).
   Without it, automated live-smoke for 16b requires reading an
   actual mailbox. To probe during 16a implementation: register
   a test account on staging, observe the response payload
   carefully, and check whether the staging Wire docs expose a
   developer-mode activation-code retrieval. Record observations
   in `docs/wire/api-notes.md` §"Step 16 discovery".
2. **`AccountStatus` enum mapping.** Wire's `AccountStatus`
   has values `active | suspended | deleted | ephemeral |
   pending-invitation` (per swagger). Step 16 expects a
   freshly-registered account to be in one of these — most
   likely `ephemeral` (account exists but not yet activated)
   or a new variant. To confirm by live-probe during 16a.
3. **PendingClient persistence (or not).** Step 16 ships
   `PendingClient` as in-memory only. If Natrium needs a
   "register on device A, activate on device B" flow, the
   pending state needs persistence — out of scope here, deferred
   unless requirement emerges.
4. **CLI activation-context handoff.** When `register` and
   `activate` are two separate CLI invocations, the second
   needs to know the email of the pending account. Options:
   (a) persist the pending email in a temp file written by
   `register`, (b) require `--email` on `activate`, (c) only
   support the single-shot `register --code` form. To decide
   during 16d.

### Smoke test

```bash
# Staging only — do NOT run against production!
export KUPFERDRAHT_REGISTER_TARGET=staging

# Two-step flow
kupferdraht-cli register --email "alice+$(date +%s)@example.com" --name "Alice" --password-stdin
# (CLI prints pending user_id; activation email lands in test mailbox)
kupferdraht-cli activate --code "<code-from-email>"
# (CLI prints active user_id; identical to pending user_id)

# Single-shot flow
kupferdraht-cli register --email "bob+$(date +%s)@example.com" --name "Bob" --code "$KNOWN_CODE"
# (only useful when KNOWN_CODE is pre-known — staging test-mode
#  or pre-seeded fixture; otherwise the email must arrive first)

# Verify the new account works end-to-end
kupferdraht-cli login --email "alice+...@example.com" --backend staging
kupferdraht-cli rooms       # empty
kupferdraht-cli sync        # ok
```

### Threat model

Password handling is the only sensitive surface added. The
existing login pipeline already handles passwords correctly
(no logging, zeroized on drop, transmitted under TLS).
`register` adopts the same posture: `password: String` is
zeroized via `Zeroizing` wrapper before the HTTP body is built;
no plaintext password persists in `PendingClient` (Wire holds
the password server-side once registered).

`sso_id` plumbing carries no secrets — `subject`, `tenant`, and
`scim_external_id` are identity references that an SSO IdP
already considers non-confidential (they appear in SAML
assertions delivered through browsers). No special handling
beyond standard String semantics.
