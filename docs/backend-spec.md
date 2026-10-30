# Kupferdraht Backend — Functional Specification

Status: DRAFT — specification only; implementation decisions follow in a
separate step.

This document specifies a standalone backend that replaces the Wire
backend for Kupferdraht clients entirely. It defines *what* the backend must
do, observable at its API surface — not *how* it is built. No storage
engine, language, framework, or deployment technology is prescribed
here.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

### How to read this specification

**This file is the complete, self-contained contract.** A backend can
be built from it alone, in a separate repository, with no access to the
Kupferdraht client source. Everything required to interoperate is stated
here inline.

**External normative references** — the only documents a backend author
must additionally obtain (all public standards):

- **RFC 9420** (The Messaging Layer Security Protocol) — the MLS
  protocol. This spec cites the exact sections needed and reproduces
  only Wire's *deviations* from it (§10.7). The pinned MLS ciphersuite
  is `MLS_128_DHKEMP256_AES128GCM_SHA256_P256` (IANA MLS ciphersuite
  `0x0002`); use any RFC 9420 implementation that supports it.
- **RFC 2046** §5.1 (`multipart/mixed`) — asset upload body (§15).
- **RFC 6265** (HTTP cookies) — the `zuid` refresh cookie (§5).
- **RFC 2119** — the requirement keywords above.

**Reference client.** Kupferdraht (the MLS client this backend serves) is
the conformance oracle: a correct backend is one an unmodified Kupferdraht
client works against by changing only its base URL (§20). The Kupferdraht
source is *not* required reading.

**Normativity rule.** Only the inline text of this document is
normative. Any mention of `observations.md §N` / `CHANGELOG.md` / `open.md`, `ADR-NNNN`, `docs/wire/*`,
`docs/plan/*`, `PLAN.md`, a "Kupferdraht Step N", or a Kupferdraht source path /
symbol (`*.rs`, `MlsClient`, `ClientBuilder`, …) is **non-normative
rationale** — it records *why* a rule exists or *where the reference
client implements it*, and is safe to ignore when building a backend.
Where this document and the Wire v14 Swagger differ, **this document
wins** (the Swagger is known to diverge from deployed wire-server — e.g. §10.7
W-1).

---

## 1. Purpose and Scope

### 1.1 Goal

Provide a backend such that an unmodified Kupferdraht client works against
it by changing only its base URL. The backend is **wire-protocol
compatible with the subset of the Wire v14 API that Kupferdraht consumes**
(paths, JSON shapes, status codes, error labels, content types,
WebSocket framing), plus a provisioning surface that Wire provides
out-of-band (admin account management).

### 1.2 Compatibility contract

- **Drop-in:** Every endpoint in §5–§15 MUST match the Wire-compatible
  behavior specified inline in this document. The contract was derived
  from Kupferdraht's live-validated observations against Wire v14 deployments;
  those observations are reproduced here, so this document — not the
  Wire Swagger — is the controlling reference. Where this document and
  the Wire v14 Swagger differ, this document wins (the Swagger is known
  to diverge from deployed wire-server — e.g. §10.7 W-1).
- **Subset:** Endpoints of the Wire v14 API outside this document are
  out of scope. Requests to unimplemented paths MUST return `404`
  (clients treat unrouted paths as 404).
- **Wire deviations:** The backend MUST reproduce Wire's deviations
  from RFC 9420, specified in full in §10.7 (W-1 … W-9). These are the
  Wire-specific behaviors that are *not* in RFC 9420 and cannot be
  derived from it — they are reproduced here normatively.

### 1.3 In scope

- Multi-tenant (team-based) identity, authentication, and session
  management, including cross-process session resume and email change
  (§4, §5)
- Account provisioning: self-registration and admin-created accounts,
  selectable per team (§6)
- Device (client) registration including the Proteus prekey contract (§7)
- MLS key material: removal keys, key-package pool (§8)
- Conversations: group and 1:1, MLS protocol only; rename, leave,
  delete-for-everyone, typing notifications (§9)
- MLS messaging: commit bundles, application messages, group info,
  server-side removals via external sender (§10)
- Event delivery: per-device queues, WebSocket `ConsumableNotifications`
  with per-event acknowledgment, REST backfill (§11, §12)
- User directory and self-profile (§13)
- Feature configs and user properties (§14)
- Asset upload/download/delete (§15)
- Admin API for deployment, team, and account management (§16)

### 1.4 Out of scope

- Federation between backend deployments and toward Wire's hosted service.
  Qualified IDs are kept wire-format compatible, but every conversation
  is confined to the deployment's single domain (§4.1). Cross-*team*
  conversation within the deployment is in scope, gated by policy
  (§4.3).
- Proteus messaging. Proteus prekeys are stored on client registration
  (backend contract, §7.2) but never served for encryption sessions.
- Voice / video / calling, link-preview generation, legal hold (the
  capability string is accepted, the feature does not exist — §7.4),
  e2ei certificate enrollment, SAML/SSO (deferred with Kupferdraht Step 17),
  asset previews.
- Push notification gateways (APNs/FCM). Delivery is via the event
  stream only. MAY be added later without changing this spec's surface.
- Any client other than Kupferdraht. Compatibility with Wire's own clients
  is a non-goal.

---

## 2. Definitions

| Term | Meaning |
|---|---|
| Deployment | One backend installation serving exactly one DNS domain (§4.1). |
| Tenant / team | An organizational unit within the deployment with its own auth, registration, feature config, and reach policy. Every user belongs to exactly one team (§4.1). |
| Reach check | The policy gate deciding whether two users of different teams may discover/contact each other (§4.3). |
| User | An account within exactly one team. Identified by UUID + deployment domain. |
| Client / device | A registered cryptographic device of a user. Identified by an opaque hex ID, unique per user. |
| Qualified ID | `{uuid, domain}` pair identifying a user or conversation. |
| MLS group | The RFC 9420 group backing a conversation. The backend tracks `group_id`, epoch, and member leaves. |
| External sender | The backend's own MLS signature key, embedded in every group's `external_senders` extension, used to issue server-side Remove proposals/commits (§10.5). |
| Event | A JSON object with a `type` field, fanned out to per-device queues (§12). |
| Notification | The deliverable unit on the event stream: an envelope with an `id` and a `payload` array of events (§11). |

---

## 3. General Protocol Conventions

### 3.1 Versioning

- All endpoints except those listed below are served under the path
  prefix `/v14`. The backend MUST accept exactly version 14;
  `CLIENT_SUPPORTED = &[14]` on the Kupferdraht side.
- Unversioned endpoints (matching Kupferdraht's call sites):
  `GET /api-version`, `POST /register`, `POST /activate`,
  `POST /activate/send`.
- `GET /api-version` (unauthenticated) MUST return:

```json
{
  "supported": [14],
  "development": [],
  "federation": false,
  "domain": "<the deployment domain, §4.2>"
}
```

The backend MAY list additional versions in `supported` in the future;
it MUST always include `14`.

### 3.2 Content types

- JSON endpoints: `application/json` both ways.
- MLS endpoints (`POST /v14/mls/messages`,
  `POST /v14/mls/commit-bundles`): request body is raw bytes with
  `Content-Type: message/mls`; responses are JSON.
- Group info (`GET …/groupinfo`): raw bytes response (§10.4).
- Assets: `multipart/mixed; boundary=frontier` upload, raw-byte
  download (§15).

### 3.3 Error model

Every application-level error response MUST be a JSON object:

```json
{ "code": <http-status-int>, "label": "<machine-label>", "message": "<human text>" }
```

- `label` is the contract; clients switch on it (`message` is
  free-form and MUST NOT be relied on). `code` equals the HTTP status.
- The backend SHOULD return this JSON envelope for *all* errors;
  clients tolerate non-JSON error bodies (e.g. edge-proxy HTML) but a
  conformant backend SHOULD always emit the envelope.

**Authoritative label catalog** — every label this spec uses, its HTTP
status, and meaning. The endpoint sections are the source of which
labels apply where; this table is the closed set:

| Label | Status | Meaning | Defined in |
|---|---|---|---|
| `invalid-credentials` | 403 | Bad email/password, wrong `old_password`, or dead/superseded `zuid` | §5.2, §5.7, §5.3 |
| `code-authentication-required` | 403 | 2FA required, `verification_code` absent (code emailed) | §5.2 |
| `code-authentication-failed` | 403 | 2FA code wrong/expired | §5.2 |
| `invalid-code` | 404 | Activation code wrong/expired | §6.1 |
| `invalid-email` | 400 | Syntactically invalid email | §5.6 |
| `key-exists` | 409 | Email already registered in the deployment | §5.6, §6.1 |
| `handle-exists` | 409 | Handle taken in the deployment | §5.5 |
| `too-many-clients` | 403 | User at `max_clients_per_user` | §7.1 |
| `client-not-found` | 404 | Unknown/deleted client id | §7.3 |
| `mls-protocol-error` | 400 | MLS validation failure (bad bundle, wrong ciphersuite, identity/key-package/lifetime, missing MLS key — W-9) | §8.2, §10.2, §10.3 |
| `mls-stale-message` | 409 | Commit/message at a superseded epoch (retry) | §10.2, §10.3 |
| `mls-client-mismatch` | 409 | Add-commit lacks full-device coverage of an added user | §10.2 |
| `not-connected` | 403 | Reach policy denies contact (1:1 / claim) | §9.5 |
| `no-conversation` | 404 | Unknown conversation, or caller not a member | §9.6 |
| `invalid-op` | 403 | Permission denied (role/policy) or cross-domain reference | §3.4, §9.4, §9.7 |

A `422` (unprocessable) is returned for structurally malformed MLS
bodies that fail to deserialize at all (vs. `400 mls-protocol-error`
for parse-OK-but-invalid); a `533` is reserved for Wire-internal
proposal-execution failure (clients treat both as transient/fatal per
the endpoint). These two carry the envelope but no stable label.

### 3.4 Qualified identifiers

- Users and conversations are addressed as
  `{"id": "<uuid>", "domain": "<deployment-domain>"}` in JSON, and as
  path segments `…/{domain}/{uuid}` in URLs.
- UUIDs MUST be lowercase RFC 4122 textual form.
- Any request that references a domain other than the deployment's
  domain MUST be rejected with `403 invalid-op` (no federation, §1.4).
  Exception: none — assets, users, and conversations all carry the
  single deployment domain.

### 3.5 Pagination

Two models exist, matching Wire:

- **Conversation IDs** (`POST /v14/conversations/list-ids`):
  request `{paging_state?, size}`; response
  `{qualified_conversations: [...], has_more: bool, paging_state: "<opaque>"}`.
  `paging_state` is an opaque token; the client chains requests until
  `has_more` is false. `size` cap: 1000.
- **Notifications** (`GET /v14/notifications`): cursor is the last seen
  notification `id` passed as `since`; response carries `has_more`
  (§11.4).

### 3.6 Idempotency and concurrency

- `GET /v14/one2one-conversations/{d}/{u}` is idempotent-create (§9.5).
- `PUT /v14/clients/{id}` with identical body is a no-op (200).
- MLS commit bundles are NOT idempotent: concurrent commits to the same
  group race; exactly one wins per epoch, the loser receives
  `409 mls-stale-message` and is expected to retry against the new
  epoch (§10.3).

---

## 4. Tenancy Model

Mechanism and policy are deliberately separated: the *mechanism* (one
domain, teams inside it, cross-team communication technically possible,
team membership visible) is fixed by this section; *whether* two teams
may interact is per-team policy (§4.3). This mirrors how Wire itself
hosts populations with different login procedures (e.g. a SAML-backed
organization and self-registered private users) on one backend while
letting them converse.

### 4.1 Deployment = domain, tenant = team

A **deployment** is one backend installation serving exactly **one**
DNS domain (e.g. `chat.example.org`). All qualified IDs of the
deployment's users and conversations carry that domain — Kupferdraht's
qualification handling, @-mention resolution (which qualifies handles
with the account's own domain), key-package claims, and asset domains
all stay within it unchanged.

A **tenant is a team** within the deployment:

- Every user belongs to **exactly one team**. There are no team-less
  users (Wire's "personal users" are modeled as members of an ordinary
  team, e.g. a `citizens` team with open registration).
- Each team has a UUID, generated at team creation (§16.1). It is
  surfaced as the `team` field of `GET /v14/self` (§5.5) — therefore
  non-null for every user — and addressed by the conversation-delete
  endpoint (§9.7). Kupferdraht resolves the team for `delete_room` via
  `conversation.team` with fallback to `self.team` (Step 23).
- Team membership carries a role: `admin` or `member` (§16.2). Team
  admins manage the team's users and configuration; the `admin` role
  also gates delete-for-everyone (§9.7).
- **Handles (`@handle`) MUST be unique per deployment** (= per domain).
  This is what `qualified_handles` lookup and @-mention resolution
  require, and — together with visible team membership — what prevents
  display-name spoofing across team boundaries.
- Email addresses MUST be unique per deployment (login carries no team
  context; the email alone must resolve the account).
- Identity assurance is attested by team membership, not by handle or
  display name: `UserProfile.team` (§13.2) is set by the backend and
  cannot be chosen by the user. Clients render it to answer "is this
  really a member of organization X?". Login strength (§4.4
  `second_factor_required`, later SAML) protects each team's *own*
  accounts; message confidentiality is MLS and identical for everyone.

### 4.2 Deployment resolution

One domain per deployment: the backend MUST verify that the HTTP `Host`
header matches the deployment's configured hostname and reject others
(`404`). `GET /api-version` returns the deployment domain (§3.1).
Unauthenticated endpoints (`/register`, `/activate`, `/login`) operate
deployment-wide; the team context comes from the account (login) or
from the registration mode (§6).

### 4.3 Reach policy (cross-team interaction)

Cross-team communication is a **policy decision, not an architectural
one**. Each team carries:

| Policy | Type | Default | Effect |
|---|---|---|---|
| `discoverable_by` | `none` \| `all` \| `[team-ids]` | `none` | Which teams' members can resolve this team's members by handle (§13.2) and open 1:1s / claim key packages for them. |
| `contactable_by` | `none` \| `all` \| `[team-ids]` | `none` | Which teams' members may initiate contact: open a 1:1 (§9.5), claim key packages (§8.3), add this team's members to conversations (§10.2). |

Normative rules (the **reach check** referenced throughout this spec):

1. Members of the **same team** always reach each other.
2. User A may *contact* user B iff B's team lists A's team in
   `contactable_by` (or `all`).
3. User A may *resolve* user B (directory lookup, §13.2) iff B's team
   lists A's team in `discoverable_by`/`contactable_by`, **or** A and B
   share a conversation (this exception keeps @-mention resolution
   working inside an existing cross-team conversation regardless of
   discovery policy).
4. Once a conversation exists, message delivery within it is never
   policy-filtered — policy gates the *establishment* of contact, not
   an established MLS group.
5. Denied reach MUST be indistinguishable from nonexistence where the
   surface allows it (`failed` arrays, `404`), and `403 not-connected`
   where Kupferdraht expects an error (1:1 creation, §9.5).

Hermetic isolation is the default (`none`/`none`): a fresh team can
talk only to itself. The federation-like openness of e.g. Matrix is the
other extreme (`all`/`all`). Both are configurations, not spec changes.

Assets are capability-addressed deployment-wide (§15); key packages,
events, and feature configs are scoped as defined in their sections.

### 4.4 Per-team configuration

Each team carries at minimum the following configuration, managed by
its team admins or the deployment operator (consumed by §5, §6, §14):

| Setting | Type | Default | Used by |
|---|---|---|---|
| `registration_mode` | `open` \| `invite` \| `admin-only` | `admin-only` | §6 |
| `second_factor_required` | bool | false | §5.2 |
| `feature_configs` | JSON per feature | see §14.1 | §14 |
| `discoverable_by` / `contactable_by` | see §4.3 | `none` | §4.3 |

At most **one team per deployment** MAY have
`registration_mode: open` — it is the team that anonymous
`POST /register` calls join (§6.1).

Deployment-level configuration (operator-set):

| Setting | Type | Default | Used by |
|---|---|---|---|
| `max_clients_per_user` | int | 7 | §7.3 |
| `asset_max_size_bytes` | int | 26214400 (25 MiB) | §15 |
| `key_package_low_watermark` | int | 30 | §8.3 (informational) |
| `zuid_ttl` | duration | 28 days | §5.1 |
| `device_purge_enabled` | bool | false | §11.6 |
| `device_purge_inactivity` | duration | 180 days | §11.6 |

---

## 5. Authentication and Sessions

### 5.1 Token model

Two credentials, exactly as Wire issues them:

- **Access token** — bearer token, sent as
  `Authorization: Bearer <token>`. Lifetime: 900 seconds (the value
  MUST be returned in `expires_in`). Carries at minimum: user ID,
  expiry, and — after rebind (§5.3) — the client ID.
- **Refresh cookie `zuid`** — `Set-Cookie: zuid=<opaque>; HttpOnly;
  Path=/access; Secure`. Rotated (new value, old invalidated) on every
  successful `POST /v14/access`. Invalidating events: logout, password
  change, deletion of the bound client, admin deactivation.

The backend MUST treat a rotated-away `zuid` value as dead: a client
process presenting a superseded cookie gets `403`.

**Cross-process resume (CHANGELOG.md §51/§58):** Kupferdraht persists the most
recent `zuid` and refreshes a stale access token in place via
`POST /v14/access` on process restart — live-verified against Wire with
a ~15 h old cookie. The backend MUST honor this: the *latest* `zuid` of
a session remains valid across arbitrary client downtime until its own
TTL elapses or an invalidating event occurs. The `zuid` TTL MUST be
operator-configurable with a default of at least 28 days; it MUST NOT
be shorter than 24 hours. Only when the `zuid` is dead does the client
fall back to `login`.

### 5.2 Login

`POST /v14/login?persist=true` — unauthenticated.

Request:
```json
{ "email": "...", "password": "...", "label": "<cookie label>", "verification_code": "<optional 2FA>" }
```

Responses:
- `200` + `Set-Cookie: zuid=…`:
  ```json
  { "user": "<uuid>", "access_token": "...", "expires_in": 900, "token_type": "Bearer" }
  ```
- `403 invalid-credentials` — unknown email or wrong password. The
  backend MUST NOT distinguish the two cases.
- `403 code-authentication-required` — the account's team requires a
  second factor (§4.4) and `verification_code` was absent. The backend MUST send the code to
  the account email upon this rejection (Wire behavior: the client
  retries login with the code).
- `403 code-authentication-failed` — code present but wrong/expired.

Accounts in `pending-activation` state MUST NOT be able to log in
(`403 invalid-credentials`).

### 5.3 Token refresh and client binding (rebind)

`POST /v14/access?client_id=<id>` — requires `Cookie: zuid=<value>`.
The `client_id` query parameter is optional; when present, the issued
access token MUST be bound to that client.

- `200` + rotated `Set-Cookie: zuid=…` + same JSON shape as login.
- `403` — cookie missing, expired, superseded, or `client_id` does not
  belong to the cookie's user. For a dead/superseded `zuid` the label
  is `invalid-credentials` (§3.3) and is load-bearing: since Kupferdraht
  Step 39 the client switches on it to recognize a dead session and
  fall back to `login` (§5.1) instead of retrying.

**Client identity injection (replaces Wire's nginx Z-Client header,
W-8):** for every authenticated request, the backend MUST derive the
acting client ID **from the access-token claim only**. Any
client-supplied `Z-Client` (or similar) header MUST be ignored.
Endpoints that require a bound client (`POST /v14/mls/messages`,
`POST /v14/mls/commit-bundles`,
`POST /v14/mls/key-packages/self/{id}`, the event stream) MUST reject
tokens without a client binding with `403`.

### 5.4 Logout

`POST /v14/access/logout` — requires Bearer token and `zuid` cookie.
Invalidates the cookie (and SHOULD invalidate the access token).
Returns `200`.

### 5.5 Self profile

- `GET /v14/self` → `200`:
  ```json
  {
    "qualified_id": {"id": "...", "domain": "..."},
    "name": "...", "handle": "...|null", "email": "...",
    "accent_id": 0, "assets": [], "locale": "...",
    "team": "<uuid of the user's team, never null — §4.1>"
  }
  ```
- `PUT /v14/self` with `{"name": "..."}` → `200`. Name length 1–128.
- `PUT /v14/self/handle` with `{"handle": "..."}` → `200`;
  `409 handle-exists` if taken within the deployment (§4.1). Handle
  charset: `[a-z0-9_.-]{2,256}` (Wire's rule; lowercase-only matches
  Kupferdraht's lowercased mention map).

Profile changes SHOULD emit a `user.update` event to the devices of
the user and of users sharing a conversation (Kupferdraht currently ignores it
gracefully; see §12.3).

### 5.6 Email change (verification flow)

`PUT /v14/access/self/email` — Bearer (note the `/access` prefix; the
endpoint sits in the session scope, matching Wire).

Request: `{ "email": "<new address>" }`

- `202` — verification mail sent to the **new** address; the account's
  email is NOT changed yet. The backend MUST deliver a confirmation
  code/link out-of-band (same delivery duty as §6.1) and MUST apply the
  change only upon confirmation. `204` MAY be returned when the backend
  applies the change without verification (team-config option;
  Kupferdraht treats both as success — Step 22).
- `400 invalid-email` — syntactically invalid address.
- `409 key-exists` — address already in use within the deployment.
- Pending email changes MUST expire (max 7 days) and a newer request
  MUST supersede an older pending one.
- Confirmation transport: reuse of the `/activate`-style code flow is
  RECOMMENDED; the exact confirmation endpoint is an implementation
  choice since Kupferdraht never calls it (the user confirms out-of-band).

### 5.7 Password change

`PUT /v14/self/password` — Bearer. Wire-compatible shape (v14 Swagger
`change-password`); called by the reference client since Kupferdraht Step 27
(`docs/plan/archive/27-change-password.md`, merged 2026-06 —
`Client::change_password`).

Request: `{ "old_password": "...", "new_password": "..." }`

- `200` — password changed. All `zuid` cookies of the user are
  invalidated (§5.1); outstanding access tokens MAY remain valid until
  their ~15-min expiry. Every client must re-`login` afterwards;
  registered devices and MLS state survive.
- `403 invalid-credentials` — `old_password` does not match.
- `400` — `new_password` violates the password policy.

Password policy (uniform for §5.7, §6.1 registration, and §16.2
admin-set passwords): minimum length 8, maximum 1024 (Wire's schema
floor is 6; this spec deliberately requires 8). The backend MUST NOT
impose composition rules beyond length (NIST-style guidance; length
over complexity).

This endpoint is also the second half of the **initial-password flow**
(§6.2): a team admin sets an initial password out-of-band, the user
logs in and replaces it here. There is no forced-change flag — Kupferdraht
has no UI to satisfy one; communicating "change your password after
first login" is an organizational duty of the team admin. Self-service
"forgot password" reset is NOT in scope; a lost password is recovered
via admin reset (§16.2). If self-service reset is wanted later, Wire's
`POST /password-reset` + `/password-reset/complete` email flow is the
compatibility template.

---

## 6. Account Provisioning

Provisioning is governed per team by `registration_mode` (§4.4).
All paths produce identical accounts; Kupferdraht cannot tell them apart.
Which team a new account joins:

- `POST /register` without `team_code` → the deployment's single
  `registration_mode: open` team (§6.1a).
- `POST /register` with `team_code` → the team the invitation was
  issued for (§6.1b); account born active.
- Admin-created → the team of the creating team admin (§6.2).

### 6.1 Self-registration (`registration_mode: open` or `invite`)

`POST /register` — unauthenticated. The backend follows Wire's
`NewUser` contract; the relevant request fields are:

```json
{ "name": "...", "email": "...", "password": "...",
  "team_code": "<invitation code, optional>", "label": "...?" }
```

The presence of `team_code` selects between two outcomes, **both
Wire-compatible** (Kupferdraht sends the same endpoint; only the body and
the follow-up differ):

**(a) Open registration — no `team_code`.** Allowed only when a team
with `registration_mode: open` exists (§4.4); the account joins it.
- `201`:
  ```json
  { "qualified_id": {...}, "name": "...", "email": "...",
    "status": "pending-activation", "accent_id": 0, "locale": "..." }
  ```
  The backend MUST send an activation email containing a short
  numeric/alphanumeric code to the given address. Email delivery
  mechanism is an implementation concern; the *requirement* to deliver
  a code out-of-band is part of this spec. The user activates via
  `POST /activate` (below).
- `403` — no `registration_mode: open` team exists.

**(b) Invite-based registration — `team_code` present.** The code was
issued by `POST /admin/v1/.../invitations` (§16.2) and delivered to the
invitee's email by the backend. The account joins the **invitation's
team** (not the open team) and — because possession of the code already
proves control of that mailbox — is born **`active`** with **no second
activation round-trip** (matches Wire: an invited user logs in
immediately after `/register`).
- `201`: same body shape with `"status": "active"`.
- `403` — `team_code` unknown, expired, or revoked.
- `409 key-exists` — the invited email is already a registered account.
- The invited email in the code MUST match the `email` in the body; a
  mismatch is `400`.

Common to both:
- `400` — malformed body, password policy violation (§5.7).
- `409 key-exists` — email already registered in this deployment.
- The backend MUST accept (and MAY log) an `X-Forwarded-For` header on
  this endpoint; it MUST NOT require it (Kupferdraht sends a placeholder).

> **Kupferdraht status (2026-06):** `ClientBuilder::register` sends only
> `{name, email, password}` (the `team_code` field exists in the
> `NewUserRequest` DTO but is never set), so Kupferdraht today drives path
> (a) only. Path (b) needs a future Kupferdraht step to thread `team_code`
> through and to add a "register returns an already-active account"
> variant to `PendingClient` (today `register` always yields a pending
> client awaiting `activate`). The **backend contract for (b) is frozen
> here** so that step lands on a stable surface — same discipline as
> Step 27 / §5.7. Until then the backend serves (b) for any
> Wire-compatible client and operators can provision via the admin
> invitation endpoints + open registration.

`POST /activate` — unauthenticated. (Path (a) only; invited accounts
are already active.)

Request: `{ "code": "...", "email": "...", "dryrun": false }`
- `200` → `{ "email": "...", "first": true }`; account transitions to
  `active`.
- `404 invalid-code` — wrong or expired code. Codes MUST expire (max
  validity 7 days) and MUST be invalidated after a bounded number of
  failed attempts (max 3, Wire behavior).

`POST /activate/send` — unauthenticated. Request: `{ "email": "..." }`.
Re-sends the code. `200` even if throttled; `429` MAY be used for rate
limiting. MUST NOT reveal whether the email exists (`200` either way
or uniform error).

### 6.2 Admin-created accounts (`registration_mode: admin-only`, always available)

Accounts created via the Admin API (§16.2) join the creating team
admin's team and are born `active` (no email round-trip) with an
admin-set initial password. The initial password is an ordinary valid
password (no forced-change flag — Kupferdraht has no UI for one); the user
replaces it after first login via `PUT /v14/self/password` (§5.7).

### 6.3 Account lifecycle states

`pending-activation → active → suspended → deleted`

- `suspended` (admin action): all sessions invalidated; login returns
  `403 invalid-credentials`.
- `deleted` (admin action): irreversible. The backend MUST remove the
  user's leaves from all MLS groups via external-sender removal
  (§10.5), delete key packages and devices, and tombstone the UUID
  (never reused).

---

## 7. Clients (Devices)

### 7.1 Registration

`POST /v14/clients` — Bearer (user-scoped token suffices).

Request (Kupferdraht's shape):
```json
{
  "password": "<optional, see below>",
  "prekeys": [ { "id": 0, "key": "<b64>" }, ... ],
  "lastkey": { "id": 65535, "key": "<b64>" },
  "type": "permanent",
  "class": "desktop|phone|tablet|legalhold",
  "label": "...", "model": "...",
  "cookie": "<cookie label>",
  "capabilities": ["consumable-notifications", "legalhold-implicit-consent"],
  "mls_public_keys": { "ecdsa_secp256r1_sha256": "<b64>" }
}
```

**Password optionality (Kupferdraht Step 24):** `password` is **optional**
and is omitted from the JSON when absent. The backend MUST require it
for the **2nd and later** client of a password account (re-auth before
adding a device) and MUST accept its absence for the **first** client
(and for SSO accounts, which have no password). This matches Wire's
actual behavior, not just the Swagger.

- `201`:
  ```json
  { "id": "<hex client id>", "type": "permanent", "class": "...",
    "time": "<ISO8601>", "label": "...", "model": "...",
    "mls_public_keys": {...} }
  ```
- `400` — malformed prekeys or body.
- `403 too-many-clients` — user already has
  `max_clients_per_user` permanent clients. Kupferdraht's two-phase login
  (Step 24) recovers by listing devices (`GET /v14/clients`), deleting
  a stale one (`DELETE /v14/clients/{id}`), and retrying — all existing
  endpoints; no new backend surface.
- Wrong/missing `password` where required → `403 invalid-credentials`.

Client IDs MUST be unique per user, generated server-side, lowercase
hex, and stable for the client's lifetime.

### 7.2 Proteus prekey contract

The backend MUST accept and persist `prekeys` and `lastkey` at
registration. It MUST NOT require them to be refreshed and MUST NOT
expose endpoints that consume them — they exist only because Kupferdraht
fulfills Wire's registration contract. (No Proteus session endpoints
are in scope.)

### 7.3 Client management

- `GET /v14/clients` → `200` array of client objects
  (`id, type, class, label, model, time, last_active?`).
- `GET /v14/clients/{id}` → `200` single object; `404` unknown.
- `PUT /v14/clients/{id}` → `200`. Accepts any subset of:
  - `{"mls_public_keys": {"ecdsa_secp256r1_sha256": "<b64 raw P-256 public key>"}}`
    — registers the device's MLS signature key. The key MUST be
    registered (by this PUT **or** at `POST /v14/clients`, §7.1) before
    the first key-package upload; an upload without it is rejected per
    §10.7 W-9. Re-registering the same key is a no-op; registering a
    *different* key for an already-registered scheme SHOULD be rejected
    with `400`. (Kupferdraht's reference flow uses this PUT immediately
    before the key-package upload.)
  - `{"capabilities": [...]}` — replaces the capability set.
  - `{"label": "..."}`.
- `DELETE /v14/clients/{id}` with `{"password": "..."}` → `200`.
  `password` is **optional** here too (Step 24), required only for a
  password account deleting its own device; omitted for SSO accounts
  and for the device-limit recovery flow where the just-registered
  session deletes a stale device. Deleting a client MUST: invalidate
  its sessions and cookies, delete its key-package pool, remove its
  leaf from every MLS group via external-sender removal (§10.5), and
  drop its event queue.

### 7.4 Capabilities

The backend MUST accept the capability strings
`consumable-notifications` and `legalhold-implicit-consent`. The event
stream (§11) MUST only be served to clients that have registered
`consumable-notifications`. `legalhold-implicit-consent` is stored and
otherwise ignored (no legal-hold feature exists).

---

## 8. MLS Key Material

### 8.1 Backend removal keys (external sender)

`GET /v14/mls/public-keys` — Bearer.

```json
{
  "removal": {
    "ed25519": "<b64>",
    "ecdsa_secp256r1_sha256": "<b64>",
    "ecdsa_secp384r1_sha384": "<b64>",
    "ecdsa_secp521r1_sha512": "<b64>"
  }
}
```

- The backend MUST hold one long-lived signature key pair per listed
  scheme, **per deployment** (groups may span teams, so removal keys
  cannot be team-scoped), and serve the public halves here.
- The `ecdsa_secp256r1_sha256` key is the one embedded as
  `external_senders` in every group (W-5) and used to sign server-side
  removals (§10.5). The other schemes MUST be present in the response
  (Kupferdraht parses the full struct) but MAY be unused.
- These keys MUST be stable across restarts; rotating them invalidates
  every existing group's ability to verify server removals (rotation
  procedure: out of scope, flag as operational risk).

### 8.2 Key-package upload

`POST /v14/mls/key-packages/self/{client_id}` — Bearer, bound client
matching `{client_id}`.

Request: `{ "key_packages": ["<b64 of a TLS-serialized RFC 9420 KeyPackage (§10), wrapped MLSMessage(key_package)>", ...] }`

- `201` on success.
- `400 mls-protocol-error` with message
  `"No key associated to the given identity and signature scheme"` if
  the client has not registered its MLS public key first (§10.7 W-9).
- `400 mls-protocol-error` for key packages that fail validation.

Validation the backend MUST perform per key package (W-refs → §10.7):
1. Parses as `MLSMessage(key_package)` carrying an RFC 9420 `KeyPackage`
   (§10); signature and structure valid per the RFC.
2. Ciphersuite is `0x0002` (MLS_128_DHKEMP256_AES128GCM_SHA256_P256,
   W-6). Other suites → `400 mls-protocol-error`.
3. Credential is a `BasicCredential` whose identity equals
   `{user-uuid}:{client-id}@{deployment-domain}` for the authenticated
   client (W-4).
4. Signature verifies against the client's registered
   `ecdsa_secp256r1_sha256` key.
5. Leaf-node capabilities include credential types Basic and X509
   (W-7) — the backend SHOULD validate, MUST NOT require more.
6. Lifetime: the backend MUST accept lifetimes of at least 90 days
   (W-3) and MUST reject already-expired key packages.

The pool size per client is unbounded by the client contract; the
backend SHOULD cap storage at 200 per client (drop oldest on overflow
is NOT allowed — reject with `400` instead, since clients track which
private keys they hold).

### 8.3 Key-package claim

`POST /v14/mls/key-packages/claim/{user_domain}/{user_uuid}?ciphersuite=2[&skip_own=<client-id>]`
— Bearer.

- `ciphersuite` is **decimal** (`2`), not hex (observations.md §27).
- Semantics: atomically remove-and-return **one** key package per
  registered MLS-capable client of the target user.
  - `skip_own=<id>` MUST exclude that client from the claim. Kupferdraht
    always passes it when claiming its own user.
  - Each key package MUST be claimable exactly once (atomic across
    concurrent claims).
- `200`:
  ```json
  { "key_packages": [
      { "user": "<uuid>", "domain": "...", "client": "<id>",
        "key_package": "<b64>", "key_package_ref": "<b64>" } ] }
  ```
  `key_package_ref` is the RFC 9420 KeyPackageRef (hash) of the
  returned package.
- A client whose pool is empty contributes no entry; an entirely empty
  `key_packages: []` is a valid `200` response (Kupferdraht treats it as
  pool depletion; the eventual commit fails with
  `409 mls-client-mismatch`, §10.3).
- `404` — target user unknown, or the reach check (§4.3) denies the
  claimer contact with the target user (indistinguishable from
  nonexistence by design).

### 8.4 Key-package count

`GET /v14/mls/key-packages/self/{client_id}/count?ciphersuite=2` —
Bearer. `200` → `{ "count": <int> }`. (Kupferdraht plans pool refresh on
this; serve it from day one.)

---

## 9. Conversations

All conversations are MLS (`"protocol": "mls"`); the backend MUST
reject `proteus`/`mixed` protocol values with `400`.

### 9.1 Conversation object

The shape Kupferdraht parses (subset of Wire's `Conversation`):

```json
{
  "qualified_id": {"id": "...", "domain": "..."},
  "type": 0,
  "protocol": "mls",
  "group_id": "<b64 MLS group id>",
  "epoch": 0,
  "epoch_timestamp": "<ISO8601|null>",
  "cipher_suite": 2,
  "name": "...|null",
  "creator": "<uuid>",
  "members": {
    "self": { "qualified_id": {...}, "conversation_role": "wire_admin" },
    "others": [ { "qualified_id": {...}, "conversation_role": "wire_member" } ]
  },
  "group_conv_type": "group_conversation|null",
  "access": ["invite"], "access_role": ["team_member", "non_team_member"],
  "team": "<owning team uuid (creator's team) — SHOULD be set; MAY be null>",
  "message_timer": null,
  "receipt_mode": 0,
  "last_event": "0.0", "last_event_time": "1970-01-01T00:00:00.000Z"
}
```

`type`: `0` regular group, `1` self-conversation (§9.8), `2` 1:1,
`3` channel. The backend
MUST track `epoch` authoritatively (advanced only by accepted commits)
and report it consistently here and in group info.

### 9.2 Create

`POST /v14/conversations` — Bearer.

Request (Kupferdraht's shape):
`{ "name": "...", "protocol": "mls", "group_conv_type": "group_conversation", "qualified_users": [] }`

- `201` → conversation object. Backend generates `qualified_id` and a
  unique `group_id`, sets `epoch: 0`, membership = creator only
  (role `wire_admin`), owning `team` = the creator's team.
- `qualified_users` MUST be empty for MLS creation (members join via
  MLS commits); non-empty → `400`.
- The MLS group itself is created client-side; at creation time the
  backend knows the group only administratively. The group becomes
  cryptographically real when the first commit bundle arrives (§10.2).
- Emits `conversation.create` to the creator's other devices (§12.3).

### 9.3 List

- `POST /v14/conversations/list-ids` — paginated IDs of all
  conversations the user is a member of (§3.5).
- `POST /v14/conversations/list` with
  `{ "qualified_ids": [ ... max 1000 ... ] }` → `200`:
  ```json
  { "found": [ <conversation objects> ], "not_found": [ ... ], "failed": [ ... ] }
  ```
  `not_found`: unknown or not-a-member. `failed`: reserved (always
  empty in single-backend operation, field MUST exist).

### 9.4 Conversation roles, rename, and membership

**Conversation-role model (Wire-faithful).** Each member holds a
per-conversation role (distinct from the team role of §4.1). A role
grants a set of *actions* (Wire's `Action` enum). The backend MUST
track each member's `conversation_role` and authorize the actions
below against it. Two built-in roles (`RoleName`, `wire_` prefix
reserved):

| Role | Actions granted |
|---|---|
| `wire_admin` | all: `add_conversation_member`, `remove_conversation_member`, `modify_conversation_name`, `modify_conversation_message_timer`, `modify_conversation_receipt_mode`, `modify_conversation_access`, `modify_other_conversation_member`, `leave_conversation`, `delete_conversation`, `modify_add_permission` |
| `wire_member` | `leave_conversation` only (Wire default) |

- The creator gets `wire_admin` (§9.2); members added by an Add-commit
  get `wire_member` by default.
- Custom roles MAY be added later; the action-based check below is the
  stable contract, not the two role names.
- Authorization is the **backend's** responsibility. Kupferdraht performs no
  client-side permission check and does not currently parse
  `conversation_role` (Open Question Q5's deferred client part); it
  surfaces whatever status the backend returns.

**Rename** — `PUT /v14/conversations/{cnv_domain}/{cnv}/name` with
`{"name": "..."}`:
- `200` if the caller's role grants `modify_conversation_name` (i.e.
  `wire_admin` by default). Emits `conversation.rename` (§12.3).
- `403 invalid-op` if the caller is a member without the action.
- `404` if the caller is not a member.

**Membership** — `DELETE /v14/conversations/{cd}/{c}/members/{ud}/{u}`
→ `200` (`204` if the user is already absent):
- Removing **self** = leaving; granted by `leave_conversation` (every
  member, including `wire_member`).
- Removing **another member** requires `remove_conversation_member`
  (`wire_admin` by default); `403 invalid-op` otherwise.
- This `DELETE` endpoint always drives the **backend-initiated**
  removal path (§10.5 path 2): the backend emits an external-sender
  Remove **proposal**, which a remaining member auto-commits — clients
  cannot self-remove in MLS (observations.md §31), and even an admin kick goes
  through the proposal path here because the kicked member is not the
  caller's own leaf to commit synchronously in this REST call. (A
  client that wants to kick by building its own commit uses
  `POST /mls/commit-bundles` directly, §10.5 path 1.) The `DELETE`
  returns once the proposal is queued; the membership change
  materializes asynchronously via the resulting member commit's
  `conversation.mls-message-add` (§10.5), so it is eventually
  consistent, not synchronous.

### 9.5 1:1 conversations

`GET /v14/one2one-conversations/{peer_domain}/{peer_uuid}` — Bearer.

- Idempotent-create: the first call by either party creates the
  conversation (type `1`, deterministic per user pair); every later
  call by either party returns the same object.
- The create is a **reservation**: while the MLS group is
  unestablished (`epoch == 0`, no `GroupInfo` on file) the
  conversation is **omitted from `/conversations/list-ids`** (§9.3).
  It joins the listing once a member uploads the establishing commit
  bundle (§10.2). Live-verified 2026-08-14 (observations §67);
  clients that only mirror the group locally dead-end.
- `200`:
  ```json
  { "conversation": <conversation object>, "public_keys": <same shape as §8.1> }
  ```
- `404` — peer unknown in this deployment.
- `403 not-connected` — the reach check (§4.3) denies the caller
  contact with the peer.
- No connection-request workflow is in scope: Kupferdraht never calls Wire's
  connection endpoints. The reach policy (§4.3) replaces it — within
  one team (or where `contactable_by` allows it) any user may open a
  1:1 (resolves former Open Question Q6).

### 9.6 Typing notifications

`POST /v14/conversations/{cnv_domain}/{cnv}/typing` — Bearer.

Request: `{ "status": "started" | "stopped" }`

- `200`, no body. Other `status` values → `400`.
- `404 no-conversation` — unknown conversation or caller not a member.
- Effect: fan out a `conversation.typing` event (§12.2) to all member
  devices except the sender's devices — **transient delivery only**
  (§11.1): pushed to currently connected event-stream sockets, never
  enqueued durably, never visible in REST backfill, no acknowledgment
  expected.
- Typing is plaintext REST, deliberately outside MLS (matches Wire;
  Kupferdraht Step 21). The backend SHOULD rate-limit per
  (client, conversation) — at most 1 event per second is sufficient.
- The backend does NOT auto-expire `started` states; deriving "who is
  typing now" (including timeout of stale `started` signals) is client
  business (Kupferdraht handles auto-stop client-side).

### 9.7 Delete conversation for everyone

`DELETE /v14/teams/{team_id}/conversations/{conv_id}` — Bearer.
`team_id` and `conv_id` are **unqualified** UUIDs (Wire's team scope;
Kupferdraht Step 23). `team_id` MUST equal the conversation's owning team
(§9.1); any other value → `404`.

- `200` — conversation deleted for all members.
- `403` (`invalid-op`) — caller lacks the required role (§ below).
- `404` — conversation unknown. Kupferdraht treats this as idempotent
  success; the backend MUST keep returning plain `404` for already
  deleted conversations (no tombstone special-casing needed).
- Authorization (resolves former Open Question Q10, matching Wire):
  the caller MUST hold the `admin` role (§4.1) in the conversation's
  owning team. Conversation-level `wire_admin` alone does NOT suffice.
- Effect, atomic in this order of observability:
  1. Emit `conversation.delete` (§12.2) to every member device
     (durable, queued).
  2. Remove the conversation and its MLS group state (roster,
     GroupInfo, epoch). No external-sender removal commits are issued —
     the group ceases to exist; clients drop local state on the event.
  3. Subsequent requests referencing the conversation (`/groupinfo`,
     `/mls/messages` for its group, rename, typing) → `404`; MLS
     messages for the deleted `group_id` → `404` or
     `400 mls-protocol-error`.
- 1:1 and self-conversations are NOT deletable this way (`403`) —
  Wire's surface is team-group-scoped; Kupferdraht offers `delete_room` for
  group conversations only.

---

### 9.8 Self conversation

Every user has exactly one MLS self conversation (type `1`,
`members.self` only) — Wire's notes-to-self room, whose MLS members are
the user's own devices. Kupferdraht uses it as the private cross-device
channel (personal task list, read position, drafts).

- The backend MUST create it server-side (deterministic id, an MLS
  `group_id`) and MUST list it in `/conversations/list-ids` +
  `/conversations/list` from the user's first listing on — **including
  while unestablished** (`epoch == 0`, no `GroupInfo`). Unlike the 1:1
  reservation (§9.5) it is never hidden. Live-verified 2026-09-24
  (observations §82).
- `GET …/groupinfo` on it MUST answer `404 mls-missing-group-info`
  until the first commit bundle arrives (§10.4).
- The backend MUST accept the establishing commit bundle (§10.2) from
  any of the user's devices — an Add-commit over the user's other
  devices' key packages (self-claim, §8.3) or a self-update commit when
  the device is alone, exactly as for a freshly created group — and
  advance the epoch. Every other device then joins by external commit
  (§10.2).
- Not deletable (§9.7).

## 10. MLS Messaging

This is the security-critical core. The backend acts as the MLS
**Delivery Service** (RFC 9420 §16) with Wire's extensions: it
validates handshake messages, serializes commit ordering per group,
stores the latest GroupInfo, fans out messages, and issues
external-sender removals. The backend MUST NOT possess group encryption
keys; application message content is opaque to it.

### 10.1 Group state the backend MUST track per conversation

- `group_id`, current `epoch`.
- The member roster at leaf granularity: which (user, client) pairs
  hold a leaf, derived from validated commits (identity per W-4).
- The per-member `conversation_role` (§9.4): creator → `wire_admin`,
  users added by an Add-commit → `wire_member`. Used to authorize
  rename and member-removal.
- Any **pending external Remove proposal** the backend has emitted but
  no member has committed yet (§10.5 path 2), so it can be re-delivered
  to members reconnecting later.
- The latest `GroupInfo` (uploaded with each commit bundle).
- The `external_senders` extension contents (backend removal key, W-5)
  — the backend MUST verify at group registration (first commit bundle)
  that the group embeds the deployment's removal key, and reject
  otherwise with `400 mls-protocol-error`.

### 10.2 Commit bundles

`POST /v14/mls/commit-bundles` — Bearer with client binding,
`Content-Type: message/mls`.

Body: the in-order byte concatenation of complete `MLSMessage`
structures, parsed sequentially (§10.6), per §10.7 W-2
(`X: MLSMessage(wire_format)`):

| Bundle | Segments |
|---|---|
| External-commit join | `commit: MLSMessage(public_message)` ‖ `group_info: MLSMessage(group_info)` |
| Add-commit | `commit: MLSMessage(public_message)` ‖ `group_info: MLSMessage(group_info)` ‖ `welcome: MLSMessage(welcome)` |
| Remove-/update-commit | `commit: MLSMessage(public_message)` ‖ `group_info: MLSMessage(group_info)` |

Every segment MUST be `MLSMessage`-framed, **including the GroupInfo**
(unlike the raw `GroupInfo` returned by `/groupinfo`, §10.7 W-1); a raw
(unwrapped) GroupInfo segment MUST be rejected with `400` (message like
`"Unknown ContentType: N"`, §10.7 W-2). Handshake messages arrive as
`public_message` (Wire's MIXED_PLAINTEXT policy) — the backend relies
on this to read commit contents; `private_message` commits MUST be
rejected with `400 mls-protocol-error`.

Processing (atomic per group; the backend MUST serialize commit
processing per group):

1. Parse all segments; structural failure → `422`.
2. Epoch check: commit epoch == current epoch, else
   `409 mls-stale-message`.
3. Sender check: sender leaf belongs to the authenticated client
   (identity per W-4); external commits validated per RFC 9420 against
   the stored GroupInfo.
4. Proposal validation: Adds reference key packages claimed from this
   backend (§8.3); each added client's key package MUST have been
   claimed by the committing user. Removes reference existing leaves.
   The reach check (§4.3) is enforced at claim time, so a commit can
   only add users the committer was allowed to contact — a commit
   referencing unclaimed key packages fails here with
   `400 mls-protocol-error`.
5. **Full-device coverage (Wire rule):** an Add-commit that adds a user
   MUST add *all* MLS-capable clients of that user (every client with a
   registered MLS public key). Partial coverage →
   `409 mls-client-mismatch`.
6. On acceptance:
   - epoch += 1; roster updated; GroupInfo replaced.
   - The commit message is fanned out as a
     `conversation.mls-message-add` event (§12.2) to every member
     device **except the sender device**.
   - The Welcome (if present) is fanned out as
     `conversation.mls-welcome` to exactly the devices joined by this
     commit.
   - Conversation membership (administrative roster, §9.1 `members`)
     is updated to match the MLS roster at user granularity.
7. Response `201`:
   ```json
   { "events": [ <the events generated, same shape as §12.2> ], "time": "<ISO8601>" }
   ```
   (Kupferdraht reads only success/failure; the `events` array MUST be
   present and MAY be empty.)

Failure labels: `409 mls-stale-message` (epoch race),
`409 mls-client-mismatch` (coverage), `400 mls-protocol-error`
(validation), `422` (deserialization), `403` (sender not a member /
not bound client).

### 10.3 Application messages

`POST /v14/mls/messages` — Bearer with client binding,
`Content-Type: message/mls`.

Body: one TLS-serialized `MlsMessage(PrivateMessage)`.

- The backend MUST verify: known `group_id`, sender is a member device
  of the group, message epoch is the current epoch (older →
  `409 mls-stale-message`; the label is load-bearing — since Kupferdraht
  Step 37 the client switches on it to re-sync and resend instead of
  surfacing an error), and `epoch >= 1` — application messages at
  epoch 0 MUST be rejected with `400 mls-protocol-error`, message
  `"Application messages at epoch 0 are not supported"` (CHANGELOG.md §25,
  observations.md §61).
- Content is opaque ciphertext; the backend MUST NOT attempt to decrypt
  and MUST fan it out verbatim as `conversation.mls-message-add` to all
  member devices except the sender device.
- `201` → `{ "events": [], "time": "<ISO8601>" }`.

### 10.4 Group info

`GET /v14/conversations/{cnv_domain}/{cnv}/groupinfo` — Bearer.

- `200`, body: the stored RFC 9420 `GroupInfo` struct (§12.4.3.2),
  **raw TLS-serialized, NOT wrapped in an `MLSMessage` envelope**
  (§10.7 W-1). Content type `message/mls`. (openmls deserializes this
  as `VerifiableGroupInfo`; on the wire it is a bare `GroupInfo`.)
- Requester MUST be a member of the conversation, OR be joining: Kupferdraht
  fetches group info to build external commits. Access rule: any
  authenticated user of the deployment who is listed in the conversation's
  administrative roster (§9.1) — external joins of non-members are only
  reachable via guest links (deferred, §17). Non-members → `404`.
- `404 mls-missing-group-info` if the group has no GroupInfo yet (no
  commit bundle received) — the self conversation (§9.8) sits in this
  state for every user until one of their devices establishes it.

The GroupInfo MUST include the `ratchet_tree` extension (clients join
by external commit without out-of-band tree delivery).

### 10.5 Server-side removals (external sender)

There are **two distinct removal paths**; only the second uses the
external sender:

**(1) Member-initiated kick** — a `wire_admin` removes *another* member
(§9.4, `remove_conversation_member`). The removing client builds an
ordinary Remove **commit** and uploads it via
`POST /v14/mls/commit-bundles` (§10.2). The backend processes it like
any commit; no external sender involved. (Kupferdraht's member-kick path,
`group_kick.rs`, produces this two-segment bundle.)

**(2) Backend-initiated removal** — the backend must remove a leaf that
no member is committing on its own: self-leave via
`DELETE …/members/{self}` (§9.4; the leaver cannot commit their own
removal — openmls rejects `CannotRemoveSelf`, observations.md §31, and has
already discarded the group locally), client deletion (§7.3), user
deletion / suspension (§6.3).

The external sender is a **non-member**: under RFC 9420 it MAY sign
*proposals* but MUST NOT author commits (commits are members-only).
Therefore the backend MUST NOT attempt to build a "server commit" — the
removal is a **Remove proposal**, committed by a remaining member.
(Live-confirmed against a Wire v14 backend: a self-leave delivers a
backend-signed Remove *proposal* to remaining members, not a ready-made
commit — `observations.md` §59.)

For each affected group the backend MUST:

1. Construct a Remove **proposal** (RFC 9420 §12.1.3) referencing the
   affected leaf/leaves, signed with the deployment's
   `ecdsa_secp256r1_sha256` external-sender key, framed as
   `MLSMessage(public_message)` with `Sender = external(0)` — index 0
   into the group's `external_senders` extension (RFC 9420 §12.1.8;
   §10.7 W-5). An external sender signs proposals only; it never
   commits.
2. Treat it as a **pending** group change: the proposal does **not**
   advance the epoch and does **not** by itself alter the roster. Fan
   it out as `conversation.mls-message-add` to all remaining member
   devices, and **persist it** so it is re-delivered to members that
   are offline now (§11.1 durable queue) and survives until committed.
3. A remaining member validates the external-sender signature against
   the embedded `external_senders` key and **auto-commits** it; the
   resulting member commit arrives via `POST /v14/mls/commit-bundles`
   and is processed by §10.2 (epoch += 1, roster update, GroupInfo
   replaced, fan-out). The epoch advances **only then**. Concurrent
   auto-commits race normally: one wins, the others get
   `409 mls-stale-message` and drop their now-redundant commit.
4. The roster update in §10.2 is what finally drops the removed leaf;
   the removal commit is the last event delivered to a removed device
   for that group.

**Edge cases the backend MUST handle:**
- **No remaining members** (the leaver was the last member, or the
  group is now empty): there is no one to commit. The backend drops the
  group state directly — no proposal is emitted (no recipient). This
  also covers self-leave of a solo/last member.
- **All remaining members offline:** the proposal stays pending and is
  re-delivered on reconnect (step 2); the removal materializes when a
  member next comes online and commits. Removal is eventually
  consistent, not synchronous with the `DELETE`.

**Client obligation this depends on:** members MUST auto-commit
validated external Remove proposals — without it the removal never
takes effect in a group of such clients. This is a decided Kupferdraht
obligation: ADR-0004 chose to auto-commit administrative proposals
(Option B), and `docs/wire/mls-deviations.md` W-5 lists it as a Kupferdraht
responsibility.

> **RESOLVED (Kupferdraht Step 28, merged 2026-06).** An audit of 2026-06-14
> had found the reference client queued inbound external Remove
> proposals but discarded them on the next outbound commit (no
> `commit_to_pending_proposals`, no signature validation, no test).
> Step 28 (`docs/plan/archive/28-external-proposal-autocommit.md`)
> closed the gap: Kupferdraht now validates and auto-commits external Remove
> proposals (`encryption/external_removal.rs`, including self-remove
> handling and tests), so path (2) removals materialize as specified
> above.

The external-sender key never encrypts and never reads application
traffic — its sole use is signing these Remove proposals (and, for
guest/invite-link flows, External-Init admissions, which are out of
scope here).

### 10.6 MLS framing primitives (RFC 9420)

All MLS objects on the wire are RFC 9420 structures. The backend needs
an RFC 9420 implementation supporting ciphersuite `0x0002` (§10.7 W-6).
Two framing facts used throughout this section:

- **`MLSMessage`** (RFC 9420 §6) is the envelope:
  `ProtocolVersion(=mls10) || WireFormat || content`. The `WireFormat`
  discriminant values are: `0x0001` public_message, `0x0002`
  private_message, `0x0003` welcome, `0x0004` group_info, `0x0005`
  key_package. Each `MLSMessage` is **self-delimiting** under
  TLS-presentation encoding (all variable-length fields are
  length-prefixed), so a sequence of concatenated `MLSMessage`s is
  parsed by deserializing one, then continuing at the next byte.
- **`Sender`** (RFC 9420 §6.1) on a handshake message is one of
  `member(LeafIndex)`, `external(SenderIndex)`, `new_member_proposal`,
  `new_member_commit`. Wire's backend removals use
  `external(index)` (§10.7 W-5).

### 10.7 Wire MLS deviations (W-1 … W-9) — normative

Wire's MLS *cryptography* is RFC 9420-conformant. What deviates is the
delivery/policy layer: framing on specific endpoints, a mandatory
extension, identity encoding, and HTTP conventions. These are **not**
derivable from RFC 9420 and are reproduced here in full; the backend
MUST implement them exactly.

| ID | One-line | Full text |
|---|---|---|
| W-1 | `/groupinfo` returns a raw `GroupInfo`, not `MLSMessage`-framed | below |
| W-2 | Commit bundle = concatenated segments, each `MLSMessage`-framed | below |
| W-3 | Accept peer leaf-node lifetimes up to ≥ 90 days | below |
| W-4 | `BasicCredential.identity` = `{uuid}:{client_id}@{domain}` | below |
| W-5 | `external_senders` extension mandatory; backend drives removals through it | below |
| W-6 | Ciphersuite pinned to `0x0002` (P-256) end to end | below |
| W-7 | Leaves declare `[basic, x509]` credential capabilities | below |
| W-8 | Acting client derived from the access-token claim, never a header | below |
| W-9 | MLS signature key registered (via `PUT /clients/{id}`) before key-package upload | below |

**W-1 — `GET …/groupinfo` returns a raw `GroupInfo`.** The response
body is a bare TLS-serialized RFC 9420 `GroupInfo` struct (RFC 9420
§12.4.3.2), **not** wrapped in an `MLSMessage` envelope (so the leading
bytes are the `GroupInfo`'s `group_context`, not a `WireFormat`
discriminant). The `GroupInfo` MUST include the `ratchet_tree`
extension so external-commit joiners need no out-of-band tree. Content
type `message/mls`. (Asymmetry with W-2 is deliberate; see W-2.)

**W-2 — commit bundle is a segment concatenation, each segment
`MLSMessage`-framed.** `POST /v14/mls/commit-bundles` body is the
in-order byte concatenation of complete `MLSMessage` structures (parsed
sequentially per §10.6):

```
commit     : MLSMessage(public_message | private_message)   (required)
group_info : MLSMessage(group_info)                          (required — WRAPPED, unlike W-1)
welcome    : MLSMessage(welcome)                             (optional; present on Add-commits)
```

The trap: `/groupinfo` (W-1) returns a **raw** `GroupInfo`, but in the
commit bundle the `GroupInfo` segment MUST be wrapped as
`MLSMessage(group_info)`. A raw (unwrapped) `GroupInfo` segment MUST be
rejected (Wire answers `400` with a message like
`"Unknown ContentType: N"` for whatever byte lands at the wire-format
position). Handshake messages ship as `public_message` (Wire's
MIXED_PLAINTEXT policy, §10.2).

**W-3 — wide leaf-node lifetime acceptance.** Wire clients (iOS/Android/
desktop) emit leaf-node `Lifetime`s with windows of ~84 days and more.
When validating a commit/external-commit, the backend MUST accept the
lifetimes of **pre-existing** peer leaves in the ratchet tree even if
they exceed a default acceptance window — a joiner/committer cannot
renegotiate a peer's lifetime. RFC 9420 §7.5 leaves the acceptance
window to application policy; Wire's policy is "wide" (accept ≥ 90-day
windows). The committer's own newly-published leaf is still validated
normally.

**W-4 — `BasicCredential.identity` carries `{uuid}:{client_id}@{domain}`.**
RFC 9420 (§5.3) treats `BasicCredential.identity` as opaque bytes.
Wire packs a UTF-8 triple:

```
{uuid}:{client_id}@{domain}
```

`{uuid}` = the user UUID (no domain); `{client_id}` = the device's
lowercase-hex id from `POST /v14/clients`; `{domain}` = the deployment
domain. The backend MUST validate, on every key package (§8.2) and
every commit (§10.2), that a leaf's credential identity equals
`{uuid}:{client_id}@{domain}` for the authenticated user+device. To map
a leaf to a user (roster), strip `:{client_id}` → `{uuid}@{domain}`
(multiple devices of one user collapse to one member).

**W-5 — `external_senders` extension mandatory; the backend is that
external sender.** RFC 9420 (§12.1.8) makes the External Senders
mechanism optional; Wire makes it mandatory. Every MLS group MUST embed
an `external_senders` group-context extension whose entry carries the
deployment's removal signature key (the `ecdsa_secp256r1_sha256` value
from `GET /v14/mls/public-keys`, §8.1) and a `BasicCredential`. The
backend obligations:
- On first commit bundle for a group, verify the embedded
  `external_senders` entry matches the deployment removal key; reject
  with `400 mls-protocol-error` otherwise.
- To remove a leaf server-side, sign a Remove **proposal** with that
  key, framed `MLSMessage(public_message)` with `Sender = external(0)`
  (index 0 into the group's `external_senders` list). A member then
  commits it (§10.5). Per RFC 9420 an external sender MAY sign
  proposals (and external-init) but MUST NOT author commits, read, or
  send application messages — it is never in the ratchet tree.
- **Client obligation (normative for the reference client, stated here
  so a backend can rely on it):** members validate an inbound
  `external(0)`-signed Remove proposal against the embedded key and
  **auto-commit** it; the resulting member commit is what advances the
  epoch and effects the removal (§10.5). Without client auto-commit a
  backend-initiated removal never materializes.

**W-6 — ciphersuite pinned to `0x0002`.** The entire deployment uses
`MLS_128_DHKEMP256_AES128GCM_SHA256_P256` (IANA `0x0002`): P-256 /
AES-128-GCM / SHA-256, signature scheme `ecdsa_secp256r1_sha256`. The
backend MUST reject key packages / groups of any other ciphersuite
(§8.2). There is no negotiation. (Rationale: iOS Secure Enclave backs
only NIST-curve signing keys.) Public keys on the wire (key-package
signature key, `mls_public_keys`, removal key) are the **raw P-256
public key** (uncompressed point), base64 where carried in JSON;
signatures use the ciphersuite's `ecdsa_secp256r1_sha256` scheme as
RFC 9420 defines for that ciphersuite.

**W-7 — leaves declare `[basic, x509]` credential capabilities.** Every
leaf node (in key packages, group creation, external-commit joins)
MUST list both `basic` and `x509` in its `Capabilities.credentials`,
even though Kupferdraht uses only `BasicCredential` — Wire's e2ei-enrolled
groups carry a `RequiredCapabilities` extension listing `x509`, and a
leaf that omits `x509` from its capabilities fails leaf-node validation
when claimed into such a group. The backend MUST accept (and SHOULD
require) leaves declaring at least `[basic, x509]`.

**W-8 — acting client from the token, never a header.** Wire injects a
`Z-Client: <client_id>` header at its edge from the access-token's
client claim and strips any client-supplied `Z-Client`. The backend
MUST derive the acting device for every authenticated request **from
the access-token binding only** (§5.3) and MUST ignore client-supplied
`Z-Client`/similar headers. Endpoints needing a bound client
(`POST /v14/mls/messages`, `/commit-bundles`,
`/key-packages/self/{id}`, the event stream) reject token-without-client
with `403`.

**W-9 — register the MLS signature key before key-package upload.**
Before `POST /v14/mls/key-packages/self/{client_id}` is accepted, the
device's MLS signature public key MUST already be registered, via
`PUT /v14/clients/{client_id}` with body
`{"mls_public_keys":{"ecdsa_secp256r1_sha256":"<base64 raw P-256
public key>"}}` (or supplied at `POST /v14/clients`, §7.1). If absent,
key-package upload MUST be rejected with exactly
`400 mls-protocol-error` message
`"No key associated to the given identity and signature scheme"`.

---

## 11. Event Delivery

### 11.1 Queue model

- The backend maintains **one durable FIFO event queue per registered
  client** (device) with the `consumable-notifications` capability.
- Every event (§12) is enqueued to each recipient device's queue as
  part of the action that caused it (no lost-wakeup window between
  state change and enqueue).
- Queue entries (notifications) carry a unique, time-ordered `id`
  (UUID format; MUST be monotonically sortable per queue in delivery
  order).
- An entry is removed **only** by client acknowledgment (§11.3) or
  device deletion. Individual unacknowledged events MUST NOT be
  silently dropped or TTL-expired out of a live device's queue — an MLS
  `Commit` or `Welcome` lost from the queue breaks that device's ratchet
  permanently (it can never catch up). The queue is therefore
  **all-or-nothing**: kept in full, or the whole device is purged
  (§11.6). There is no "drop oldest" cap.
- Delivery guarantees: at-least-once, per-queue FIFO order. After ack,
  an event MUST NOT be delivered again. Cross-queue ordering is
  unspecified.
- **Transient events** (`conversation.typing` only, §9.6) bypass the
  durable queue entirely: they are pushed to currently connected
  sockets (same frame format, §11.2), are lost if the device is
  offline, MUST NOT appear in REST backfill (§11.4), and impose no
  acknowledgment obligation — an ack whose `delivery_tag` covers a
  transient delivery is simply a no-op for it.

### 11.2 WebSocket stream

> **Reference-client status (2026-08, Step 62):** Kupferdraht connects this
> stream on native targets when the consumer opts into
> `SyncTransport::WebSocket`; the default remains the REST backfill
> (§11.4) drain, which also stays the reconnect-gap fallback. The
> framing below was live-derived from a Wire v14 backend while the stream
> was still unimplemented client-side and remains the normative
> contract (the Step-62 client was built against this section).

`GET /v14/events?client=<client-id>&sync_marker=<uuid>&access_token=<token>`
— upgraded to WebSocket (`101`).

- **Auth via `access_token` query parameter** (Wire strips Authorization
  headers on upgrade; clients therefore send the token in the query —
  the backend MUST accept it there; it MAY additionally accept the
  Bearer header).
- `client` MUST match the token's bound client; mismatch or missing
  binding → `401` before upgrade.
- One active socket per device queue; a second connection for the same
  client MUST terminate the first.
- After upgrade the backend delivers queued notifications in order,
  then continues streaming new ones as they are enqueued.

Frame format — JSON, one `ConsumableNotification` per WebSocket
frame. The backend MAY deliver frames as text or as binary frames
carrying the same UTF-8 JSON (the 2026-04 capture sent text;
current wire-server sends binary — live-observed 2026-08-12); clients
MUST accept both:

```json
{
  "type": "event",
  "data": {
    "delivery_tag": <int>,
    "event": { "id": "<notification-uuid>", "payload": [ <event objects> ] }
  }
}
```

plus the marker frame (when the queue has drained past the
synchronization point):

```json
{ "type": "synchronization", "data": { "delivery_tag": <int>, "marker_id": "<the sync_marker from the query>" } }
```

- `sync_marker` is a client-generated UUID. The backend MUST emit the
  `synchronization` frame after all notifications that were already
  queued at connection time have been sent. Clients use this to detect
  "caught up".
- **The marker rides the durable queue** (live-observed 2026-08-12 on
  current wire-server; the 2026-04 capture carried no
  `delivery_tag`): it consumes a `delivery_tag` and MUST be acked like
  an event (§11.3) — unacked markers block further delivery once the
  broker's prefetch window fills. A reconnecting client MAY therefore
  receive **stale markers from earlier connections** first; it MUST
  ack them and MUST NOT treat any marker other than its own echo as
  its catch-up signal.
- Heartbeat: the backend MUST respond to WebSocket ping frames and
  SHOULD send pings at an interval ≤ 30 s.

### 11.3 Acknowledgment

The client sends JSON text frames:

```json
{ "type": "ack", "data": { "delivery_tag": <int>, "multiple": <bool> } }
```

- `multiple: true` acknowledges all deliveries up to and including
  `delivery_tag` on this connection; `false` exactly one.
- On ack the backend MUST remove the corresponding notification(s) from
  the device queue.
- `delivery_tag` is connection-scoped, assigned by the backend,
  strictly increasing from 1 per connection.
- Unacked deliveries are redelivered (same notification `id`, new
  `delivery_tag`) on the next connection.

### 11.4 REST backfill

`GET /v14/notifications?client=<id>&size=<n>[&since=<notification-id>]`
— Bearer with matching client binding.

- Returns queued (unacknowledged) notifications **without consuming
  them**:
  ```json
  { "notifications": [ { "id": "...", "payload": [ ... ] } ],
    "has_more": <bool>, "time": "<ISO8601>" }
  ```
- `since` is exclusive; omitted = from queue head.
- **Stale cursor:** if `since` references a notification the queue no
  longer has (already acked/purged), respond `404` with the standard
  error envelope (§3.3) — *not* a notifications page. The client treats
  this as "cursor lost" and performs a full resync (reconnects the
  event stream with a fresh `sync_marker`, §11.2/§11.5).
- `size`: 100–1000, default 100.
- A pull-only client (the reference client today, §17) never
  acknowledges: it tracks its position solely via its persisted `since`
  cursor and resumes from there. Its server-side queue therefore
  shrinks only through device deletion / purge (§11.6) — this is the
  expected steady state, not an error.

### 11.5 Reconnect contract (client view, normative for the backend)

1. Client connects with a fresh `sync_marker` UUID.
2. Backend replays all unacked notifications (FIFO), then sends the
   `synchronization` marker frame, then streams live.
3. Client acks as it processes; on disconnect, unacked items remain.
4. REST backfill (§11.4) MAY be used by the client as an alternative
   drain; acks still only happen on the socket (REST reads are
   non-consuming).

### 11.6 Inactive-device purge

A device that never reconnects accumulates events forever (§11.1: no
per-event drop is possible without breaking MLS). The only MLS-safe way
to reclaim that storage is to purge the **whole device**.

- The backend provides a **device-inactivity purge**, configurable per
  deployment (§4.4):
  - `device_purge_enabled` — **default `false`** (keep-forever is the
    default; a returning device then catches up losslessly).
  - `device_purge_inactivity` — inactivity window before purge,
    **default 180 days**; counted from the device's last successful
    event-stream connection or authenticated request.
- A purge is exactly a **device deletion** (§7.3): it removes the
  device's leaf from every MLS group via external-sender removal
  (§10.5), deletes its key-package pool, invalidates its sessions, and
  drops its event queue. There is no lighter "evict queue, keep device"
  variant — a device returning without the missed `Commit`s would be a
  live-but-broken member.
- **Client recovery is graceful** (verified against Kupferdraht): a purged
  device's `zuid` dies, so resume fails and the user re-logs in;
  `reuse_existing_device_if_valid` sees the gone device (`404
  client-not-found`) and registers a fresh one. The user loses only the
  old device's local history, not the account.
- **Removal dependency:** a purge triggers a §10.5 path-2 removal,
  which materializes once a remaining member auto-commits the external
  Remove proposal (the client obligation in §10.7 W-5). The reference
  client implements this auto-commit (live-verified, §10.5); a backend
  relying on purge in a group of clients that do *not* auto-commit
  would leave a ghost leaf, so the purge defaults off.
- Operators monitor queue growth via the per-device queue-depth read
  (§16.4) regardless of whether the purge is enabled.

---

## 12. Event Catalog

### 12.1 Envelope

Events ride inside notification `payload` arrays. Every event object
has at minimum:

```json
{ "type": "<event-type>", "qualified_conversation": {...}?, "qualified_from": {...}?, "time": "<ISO8601>", "data": ... }
```

**Two intentional, Wire-faithful envelope quirks the backend MUST
reproduce verbatim (clients depend on them; do NOT "normalize"):**
- For `conversation.mls-message-add`, the MLS bytes live at
  `event.data.data` — i.e. `data` is an object whose `data` key holds
  the base64 `MLSMessage` (nesting is deliberate).
- For `conversation.mls-welcome`, `event.data` is a **bare base64
  string** (the `MLSMessage(welcome)`), not an object — the one event
  whose `data` is not an object.

### 12.2 Events the backend MUST emit (consumed by Kupferdraht today)

| Type | Trigger | Recipients | `data` |
|---|---|---|---|
| `conversation.mls-message-add` | §10.2 commit, §10.3 app message, §10.5 server removal | all member devices except sender device | `{ "data": "<b64 MlsMessage>", "epoch": <int> }` plus `qualified_conversation`, `qualified_from`, `subconv: null` |
| `conversation.mls-welcome` | Welcome segment of an Add-commit | exactly the newly added devices | `"<b64 MlsMessage(Welcome)>"` (string), plus `qualified_conversation` of the joined conversation |
| `conversation.typing` | §9.6 typing POST | member devices except sender's, **transient — live sockets only (§11.1)** | `{ "status": "started"\|"stopped" }` plus `qualified_conversation`, `qualified_from` |
| `conversation.delete` | §9.7 delete-for-everyone | all member devices (durable) | none required; `qualified_conversation` identifies the deleted conversation |
| `feature-config.update` | team feature config changed (§14.1) | all devices of the team's members | `{ "name": "<feature-name>" }` — **without** the new value; clients re-fetch `/feature-configs` (ADR-0012) |
| `user.properties-set` | property PUT (§14.2) | all devices of that user | `{ "key": "<property-key>", "value": <json> }` |
| `user.properties-delete` | property DELETE (§14.2) | all devices of that user | `{ "key": "<property-key>" }` |

### 12.3 Events the backend SHOULD emit (Wire emits them; Kupferdraht currently discards unknown types gracefully)

`conversation.create`, `conversation.rename`,
`conversation.member-join`, `conversation.member-leave`,
`user.update`, `user.client-add`, `user.client-remove`.

Emitting these keeps the door open for future Kupferdraht steps without a
backend change. Unknown-type events MUST be tolerated by clients per
Kupferdraht's envelope rule, so emitting them is safe. Since Step 118 Kupferdraht
refreshes its room list when it receives one of them (any event it does
not process itself) instead of on every sync pass: a backend that omits
them leaves room names, member lists and new rooms without a Welcome
stale for up to 5 minutes (Kupferdraht's safety-net refresh).

**`conversation.member-join` / `conversation.member-leave` are
ADVISORY for MLS conversations (Q9).** Wire emits them alongside the
MLS commit (`conversation.mls-message-add`); the backend SHOULD do the
same, emitted to the conversation's member devices when a commit
(§10.2) or external-sender removal (§10.5) changes the roster. But the
**authoritative** membership change for an MLS conversation is the MLS
commit, not these events. The backend MUST keep its administrative
roster (§10.2 step 6) consistent with the MLS-derived roster; clients
MUST derive cryptographic membership from MLS group state, never from
these advisory events (a `member-join` may arrive before the recipient
has processed the commit that actually adds the leaf). Kupferdraht derives
membership from openmls state + `sync_metadata`; since Step 118 these
events only tell it to run `sync_metadata` — which is the correct
handling, not a gap.

---

## 13. User Directory

### 13.1 Self — see §5.5.

### 13.2 Batch lookup

`POST /v14/list-users` — Bearer. Exactly one of:

- `{ "qualified_ids": [ {"id","domain"}, ... ] }`
- `{ "qualified_handles": [ {"handle","domain"}, ... ] }`

`200`:
```json
{ "found": [ <UserProfile> ], "failed": [ <echo of unresolvable inputs> ] }
```

UserProfile:
```json
{ "qualified_id": {...}, "name": "...", "handle": "...|null",
  "accent_id": 0, "assets": [], "team": "<uuid of the user's team — MUST be set (§4.1)>",
  "legalhold_status": "no_consent" }
```

- Cap: 10 inputs per request (Wire-observed; reject more with `400`).
- Inputs with a foreign domain go to `failed` (single deployment
  domain, §3.4).
- Resolution is gated by the reach check (§4.3): an input resolves iff
  the target is in the requester's own team, the target's team is
  discoverable/contactable by the requester's team, or requester and
  target share a conversation (keeps cross-team @-mention resolution
  working). Denied inputs go to `failed`, indistinguishable from
  nonexistent users.
- `team` in UserProfile is backend-attested (§4.1) — it is the identity
  anchor clients display; the backend MUST NOT let users influence it.

---

## 14. Feature Configs and User Properties

### 14.1 Feature configs (team-level, read-only for clients)

`GET /v14/feature-configs` — Bearer. `200`: one JSON object whose keys
are feature names. The backend MUST serve at least:

```json
{
  "fileSharing":      { "status": "enabled|disabled", "config": null },
  "mls":              { "status": "enabled", "config": {
                          "defaultProtocol": "mls",
                          "allowedCipherSuites": [2],
                          "defaultCipherSuite": 2,
                          "supportedProtocols": ["mls"] } },
  "conferenceCalling": { "status": "disabled" },
  "selfDeletingMessages": { "status": "disabled", "config": { "enforcedTimeoutSeconds": 0 } }
}
```

- Unknown extra features MAY be present (Kupferdraht stores them as opaque
  blobs).
- Values come from the requesting user's team configuration (§4.4);
  changes via the Admin API (§16.3) MUST emit `feature-config.update`
  with the feature name to that team's members (§12.2).
- `mls.status` MUST be `enabled` and the ciphersuite lists MUST pin
  suite 2 — this backend has no other mode.

### 14.2 User properties (per-user key-value store)

- `GET /v14/properties` → `200` JSON array of key strings.
- `GET /v14/properties/{key}` → `200` raw JSON value; `404` unknown key.
- `PUT /v14/properties/{key}` with a JSON body → `200`; creates or
  replaces; emits `user.properties-set` to the user's devices
  (including the writer's — Kupferdraht dedupes).
- `DELETE /v14/properties/{key}` → `200`; emits
  `user.properties-delete`.
- Constraints: max 256 keys per user, max 64 KiB per value, key charset
  printable ASCII without `/`. The key `WIRE_RECEIPT_MODE` (integer
  `0`/`1`) is the one Kupferdraht reads today; the store is generic.

---

## 15. Assets

Content-addressable encrypted blob store. The backend never sees
plaintext: clients upload AES-256-CBC ciphertext; keys and SHA-256
digests travel inside MLS messages (ADR-0011). The backend's integrity
duty is transport-level only.

### 15.1 Upload

`POST /v14/assets` — Bearer.

- Body: `multipart/mixed; boundary=frontier`. The backend MUST accept
  the literal boundary `frontier`; it MAY accept arbitrary boundaries
  (Wire requires `frontier`; Kupferdraht always sends it).
  - Part 1: `Content-Type: application/json` —
    `{ "public": false, "retention": "persistent|eternal|expiring|volatile", "convId"?: "...", "filename"?: "...", "filetype"?: "..." }`
  - Part 2: `Content-Type: application/octet-stream` — ciphertext
    bytes. An optional `Content-MD5` header (base64 MD5 of the bytes)
    MUST be verified when present; mismatch → `400`.
- Size cap: deployment `asset_max_size_bytes` (§4.4); exceeded → `413`.
- `201`:
  ```json
  { "key": "<asset-uuid>", "domain": "<deployment-domain>",
    "token": "<opaque asset token>", "expires": "<ISO8601|absent>" }
  ```
- The backend MUST generate a fresh random `token` per asset when
  `public: false`. Possession of `(key, token)` grants download — the
  token is the capability; it travels E2E-encrypted inside the MLS
  message.

### 15.2 Download

`GET /v14/assets/{asset_domain}/{asset_id}` — Bearer +
`Asset-Token: <token>` header for private assets.

- `200` raw ciphertext bytes.
- `404` — unknown asset, or wrong/missing token (MUST NOT distinguish).
- `asset_domain` MUST equal the deployment domain (§3.4).
- Any authenticated user of the deployment with the correct token may
  download — there is no per-conversation or per-team ACL (capability
  model, matches Wire; the token travels only inside MLS-encrypted
  messages, so possession implies conversation membership).

### 15.3 Delete

`DELETE /v14/assets/{asset_domain}/{asset_id}` — Bearer.

- `200`/`204`. Only the uploader may delete; others → `403`.
- `404` unknown.

### 15.4 Retention

`retention` values and backend obligations:
- `persistent` / `eternal` — keep until deleted by uploader or uploader
  account deletion.
- `volatile` / `expiring` — backend MAY garbage-collect after the
  served `expires` timestamp; MUST serve `expires` in the upload
  response when it will expire the asset.

Kupferdraht currently always sends `persistent`.

---

## 16. Admin API

Administrative surface for operators. **Not Wire-compatible, not used
by Kupferdraht** — namespaced under `/admin/v1`. Two principal kinds:

- **Deployment operator** — NOT a messaging user (separate credential
  space, e.g. operator bearer tokens). Manages the deployment and its
  teams.
- **Team admin** — a messaging user holding the `admin` role in their
  team (§4.1). Team admins manage their own team's users and
  configuration; the same role gates delete-for-everyone (§9.7). How a
  team admin authenticates to `/admin/v1` is an implementation choice.

This section specifies required capabilities, not final JSON shapes
(those are settled in the implementation step; the messaging API above
is the compatibility-frozen part).

### 16.1 Deployment and team management (deployment operator)

- Configure the deployment: domain, deployment-level settings (§4.4).
  Deployment setup MUST generate the external-sender key set (§8.1).
- Create team: name, initial config (§4.4), initial team admin.
  Creating a team generates its team UUID (§4.1).
- Update any team's config including reach policy (§4.3); enforce the
  at-most-one-open-team rule (§4.4).
- Suspend/delete a team (delete = irreversible: all member accounts
  deleted per §6.3 semantics, including MLS removals §10.5).
- List teams.

### 16.2 User management (team admin, own team only)

- Create user: name, email, password → account born `active` in the
  admin's team (§6.2).
- Grant/revoke the team `admin` role.
- List/search team users; read account status.
- Suspend / reactivate / delete user (semantics §6.3; delete triggers
  MLS removals §10.5).
- Reset password: set a new password, invalidating all sessions and
  cookies (clients must re-login; registered devices and MLS state
  survive).
- **Manage team invitations** (Wire-compatible surface, mirrors Wire's
  `/teams/{tid}/invitations`):
  - Create invitation: `{email}` → the backend generates a `team_code`,
    binds it to `(team, email)`, and emails it to the invitee. The code
    is what the invitee passes to `POST /register` as `team_code`
    (§6.1b). Invitations MUST expire (default 14 days, Wire's value).
  - List / get / revoke pending invitations.
  - Check by email (`HEAD`-style existence query) — mirrors Wire's
    `/teams/invitations/by-email`, lets an admin avoid duplicate
    invites.
  - An accepted invitation (account registered with the code) MUST be
    consumed — single use.
  These endpoints live under `/admin/v1` (non-Wire-compatible admin
  namespace), but their *semantics and the resulting `team_code`* are
  Wire-shaped so the invitee-facing `/register` flow stays a drop-in.

### 16.3 Team configuration (team admin, own team only)

- Read/update the team settings of §4.4 including feature configs
  (§14.1). Feature-config updates MUST emit `feature-config.update`
  events to the team's members. Reach-policy changes (§4.3) SHOULD be
  restricted to the deployment operator (a team widening its own
  `contactable_by` only affects who may contact *it*, so team-admin
  control is acceptable; widening `discoverable_by` exposes members —
  operator sign-off recommended).

### 16.4 Operational reads

- Per-user device list with key-package pool counts (supports the
  `key_package_low_watermark` monitoring concern).
- Event-queue depth and last-activity timestamp per device (detects
  dead devices pinning storage; informs the §11.6 inactivity purge).

---

## 17. Deferred Surface (planned by Kupferdraht, specify-later)

These are referenced by `PLAN.md` but blocked or deferred on the Kupferdraht
side. They are listed so path/shape decisions don't paint us into a
corner; they are NOT part of the initial backend scope:

- Invite-based registration client path (§6.1b): thread `team_code`
  through `ClientBuilder::register` and add an active-on-register
  `PendingClient` variant. Backend contract already frozen (§6.1b,
  §16.2); a future Kupferdraht step (analogous to Step 27) supplies the
  client half. No plan file opened yet — kept open per Q3.
- Conversation-role awareness (§9.4): parse Wire's per-member
  `conversation_role` into `RoomMember` (today only `qualified_id` is
  decoded) and expose it, so a client can pre-check `modify_*` actions
  (e.g. hide rename for `wire_member`) instead of relying on the
  backend `403`. UX-only; backend authorization is already complete
  per Q5. No plan file opened.
- **WebSocket event stream (§11.2/§11.3/§11.5) — implemented in Kupferdraht
  Step 62 (2026-08).** The reference client connects `GET /v14/events`
  on native targets when the consumer opts into
  `SyncTransport::WebSocket` (`wire/websocket.rs`, `sync/push/`);
  the default remains the pull drain of `GET /v14/notifications` with
  a persisted `since` cursor, and the wasm build is pull-only. A
  backend without the WebSocket endpoint still passes §20 conformance
  for pull-only consumers, but push mode — including receiving
  `conversation.typing` (transient, socket-only — §11.1) — requires
  the §11.2 surface.
- Guest links: `POST/DELETE /v14/conversations/{id}/code`,
  `POST /v14/conversations/join` (Kupferdraht Step 8 redeem pending).
- Key backup (Kupferdraht Step 12 — no v14 endpoints exist; if Kupferdraht
  adopts it, this backend defines new endpoints at that time). The
  Dehydrated Device (Step 13) was dropped 2026-09-25.
- SAML/SSO (Kupferdraht Step 17).
- MLS subconversations (`subconv` is delivered as `null` in §12.2;
  no subconversation endpoints).

---

## 18. Non-Functional Requirements

### 18.1 Security

- TLS mandatory on every endpoint; no plaintext HTTP listener in
  production deployments.
- Password storage: memory-hard KDF (argon2id-class); this is a
  requirement on observable behavior in the sense that no API may ever
  return a password or password hash.
- Access tokens MUST be unforgeable and verifiable per request;
  `zuid` cookies MUST be high-entropy opaque values (≥ 128 bit).
- The backend MUST NOT log message ciphertext, asset ciphertext,
  tokens, cookies, passwords, or key-package private material (it never
  holds the latter).
- The external-sender private keys (§8.1) are the backend's most
  sensitive secret: compromise allows arbitrary member removal (DoS,
  not confidentiality loss). They MUST be storable in an
  operator-provided secret store; the API contract is only that they
  are stable (§8.1).
- Rate limiting REQUIRED on: `/login`, `/register`, `/activate`,
  `/activate/send` (per-IP and per-account). Limits are
  operator-configurable; on excess return `429`.

### 18.2 Consistency and atomicity

- Per-group commit serialization (§10.2) is a hard requirement —
  two commits for the same epoch MUST NOT both succeed.
- Key-package claim is atomic remove-and-return (§8.3) — a package MUST
  NOT be claimable twice.
- State change + event enqueue (§11.1) MUST be atomic from the client's
  observation: no accepted commit/message without its events eventually
  appearing in every recipient queue.

### 18.3 Durability

- Acknowledged-as-accepted writes (2xx responses) MUST survive backend
  restart: accounts, devices, prekeys, key packages, group state,
  GroupInfo, event queues, assets, properties, configs.
- Event queues are durable storage, not best-effort memory (§11.1).

### 18.4 Limits summary (defaults, operator-tunable unless fixed)

| Limit | Value | Fixed by protocol? |
|---|---|---|
| API version | 14 | yes |
| Ciphersuite | 0x0002 only | yes (W-6) |
| Access-token TTL | 900 s | Kupferdraht assumes ~15 min — keep |
| `zuid` cookie TTL | 28 days (min 24 h) | Kupferdraht's resume relies on long-lived `zuid` (§5.1) |
| Typing rate limit | 1/s per client+conversation | no |
| Clients per user | 7 | no |
| Key packages per upload | ≤ 100 per request | no |
| Key-package pool cap | 200/client | no |
| `list-users` batch | 10 | Wire-observed |
| `conversations/list` batch | 1000 | Wire-observed |
| Notification page size | 100–1000 | Wire-observed |
| Asset size | 25 MiB | no |
| Property value | 64 KiB | no |

---

## 19. Open Questions

Decisions deliberately left open or made provisionally — to be settled
before or during the implementation step.

**Resolved (2026-06-12):**

- **Q1 — Tenancy model.** RESOLVED: deployment = one domain, tenant =
  team within it, cross-team reach is per-team policy (§4). Rationale:
  hard domain-per-tenant isolation cannot model populations with
  different login procedures that still converse (e.g. a SAML-backed
  organization and self-registered users); Kupferdraht's mention resolution
  qualifies handles with the account's own domain, so multi-domain
  communication would break @-mentions. Mechanism (one domain, teams,
  visible team membership) is fixed; reach is policy (§4.3) with
  hermetic-by-default settings.
- **Q2 — Handle scope.** RESOLVED: unique per deployment (= per
  domain), exactly the scope `qualified_handles` lookup and @-mention
  resolution require. Anti-spoofing across teams is carried by
  backend-attested team membership (§4.1, §13.2), not by the handle
  alone. Handles are protocol-irrelevant otherwise (not used in auth or
  MLS identity).
- **Q6 — Connection requests.** RESOLVED: no connection workflow; the
  reach policy (§4.3) replaces it. Within a team, 1:1s are always
  possible; across teams iff `contactable_by` allows (§9.5,
  `403 not-connected` otherwise).
- **Q4 — Initial passwords.** RESOLVED: the spec adds the
  Wire-compatible `PUT /v14/self/password` (§5.7). Admin-created
  accounts get an ordinary initial password (§6.2) which the user
  replaces after first login; no forced-change flag, no self-service
  "forgot password" reset (admin reset per §16.2 covers recovery).
- **Q10 — Delete-for-everyone authorization.** RESOLVED: requires the
  team `admin` role in the conversation's owning team (§9.7), matching
  Wire's team-admin gating now that real team roles exist (§4.1).
- **Q3 — Invite mechanism.** RESOLVED: adopt Wire's full mechanism
  rather than the simplified email-allowlist — stay Wire-compatible and
  keep future options open. A team admin creates an invitation (§16.2),
  the backend emails a `team_code`, and the invitee registers via
  `POST /register` with that code (§6.1b) → born active, no second
  activation round-trip. Wire's `team_code` field already exists in
  Kupferdraht's `NewUserRequest` DTO (unused today), so the `/register`
  surface needs no shape change; threading it through
  `ClientBuilder::register` + the active-on-register `PendingClient`
  variant is a future Kupferdraht step. The backend contract is frozen now
  (§6.1b, §16.2). Email-allowlist provisioning remains available as an
  operator convenience layered on the same endpoints, not as the
  primary mechanism.
- **Q5 — Rename authorization.** RESOLVED: Wire-faithful — rename
  requires the caller's `conversation_role` to grant the
  `modify_conversation_name` action (`wire_admin` by default;
  `wire_member` cannot rename). The backend tracks per-member roles
  (§9.4, §10.1) and enforces; member-removal follows the same
  action model (`remove_conversation_member`). No Kupferdraht change is
  required — it reads no role and surfaces the backend's `403`. An
  optional future Kupferdraht UX step (parse `conversation_role` into
  `RoomMember` so the client can pre-check) is noted in §17; the
  backend authorization stands on its own regardless.

- **Q7 — Removal form.** RESOLVED: the backend emits an external-sender
  Remove **proposal**, committed by a remaining member — not a
  "server-built commit", which is not a valid RFC 9420 construct (an
  external sender is a non-member and may sign proposals but never
  commit). Verified against Kupferdraht's openmls 0.8 integration: inbound
  external proposals are parsed and queued via `store_pending_proposal`
  (`encryption/messages.rs`); a member then commits, and that member
  commit advances the epoch via the normal §10.2 path. The proposal
  does not advance the epoch by itself; removal is eventually
  consistent (§10.5). Member-initiated kicks remain ordinary member
  commits (§10.5 path 1). **Client-side caveat (VERIFIED 2026-06-14):**
  the auto-commit of validated external proposals — a decided Kupferdraht
  obligation (ADR-0004) — is **not implemented**; Kupferdraht queues the
  proposal then discards it (`clear_pending_proposals`), so path (2)
  removals never materialize in a Kupferdraht-only group. Pre-existing
  Kupferdraht bug, tracked as Step 28; see §10.5 and §17. (Step 28 has since
  merged — the reference client now auto-commits, §10.7 W-5. Step 71
  closed the residual race: a user Add/Remove commit no longer clears
  queued external Removes — peer-targeting ones ride that commit.)
- **Q8 — Unbounded event queues.** RESOLVED: individual events can never
  be safely dropped (an MLS `Commit`/`Welcome` lost from a live queue
  breaks the ratchet permanently), so the only reclaim is whole-device
  purge. The backend provides a device-inactivity purge (§11.6),
  **disabled by default** (keep-forever; a returning device catches up
  losslessly), operator-enabled per deployment with a default 180-day
  inactivity window (§4.4 `device_purge_enabled` /
  `device_purge_inactivity`). A purge is a device deletion (§7.3 →
  §10.5 removal); client recovery is graceful (re-login → fresh device,
  verified). The purge triggers a §10.5 path-2 removal, which the
  reference client now auto-commits (§10.7 W-5, merged); it stays
  default-off because a deployment with non-auto-committing clients
  would leak ghost leaves.
- **Q9 — `conversation.member-join/leave` events.** RESOLVED: SHOULD
  emit from day one, matching Wire (which emits them for MLS convs too),
  but **advisory only** (§12.3). The authoritative MLS membership change
  is the commit (`conversation.mls-message-add`); clients derive
  cryptographic membership from MLS state, never from these events.
  Kupferdraht ignores them and derives from openmls + `sync_metadata` —
  correct, not a gap.

**Open:**

- **Q11 — Per-team SAML/SSO.** The tenancy model is built so that a
  team can later carry a SAML IdP configuration instead of password
  login (§4.4). Deferred together with Kupferdraht Step 17 — the login
  endpoint surface for SSO accounts is unspecified until then.

---

## 20. Conformance

A backend is conformant when an **unmodified Kupferdraht client** completes
the full lifecycle against it by changing only its base URL. Kupferdraht is
the reference oracle; its live smoke scripts (`scripts/smoke-baseline.sh`
and per-feature smokes) exercise the surface end to end. There are no
separate conformance vectors — interop with the reference client is the
test.

### 20.1 Reference end-to-end transcript

The choreography a fresh client runs; a backend MUST support it in this
order. (`→` = client request; effects in parentheses.)

1. `GET /api-version` → confirm `14` ∈ `supported` (§3.1).
2. `POST /v14/login?persist=true` → access token + `zuid` cookie (§5.2).
3. `POST /v14/clients` (prekeys + `mls_public_keys`) → `client_id`
   (§7.1). 2nd+ device needs `password`; first device may omit it.
4. `POST /v14/access?client_id=<id>` (cookie auth) → access token now
   **bound** to the device (§5.3; required before any `/mls/*` — §10.7
   W-8).
5. `PUT /v14/clients/{id}` `{mls_public_keys:{ecdsa_secp256r1_sha256}}`
   → register the MLS signature key (§10.7 W-9), unless already sent in
   step 3.
6. `GET /v14/mls/public-keys` → the deployment removal key for
   `external_senders` (§8.1, §10.7 W-5).
7. `POST /v14/mls/key-packages/self/{id}` → upload key packages (§8.2).
8. `GET /v14/feature-configs`, `GET /v14/properties` → server config
   (§14).
9. `POST /v14/conversations` → create a group at epoch 0 (§9.2); the
   first `POST /v14/mls/commit-bundles` commit takes it to epoch 1
   (§10.2) — an Add-commit (with invitees) or, for a solo creator, a
   self-update commit — application messages before epoch 1 are
   rejected (§10.3).
10. `POST /v14/mls/messages` → send (§10.3); recipients receive
    `conversation.mls-message-add` on the event stream.
11. Drain `GET /v14/notifications` with a persisted `since` cursor
    (§11.4) — the path the reference client exercises today. The
    WebSocket stream with per-event acks (§11.2/§11.3) is the frozen
    future push path (§17); a backend without it currently still
    passes §20 conformance.
12. Server-driven removal (`DELETE …/members/{self}`): the backend
    emits an external-sender Remove **proposal**; a remaining member
    auto-commits it (§10.5, §10.7 W-5) — live-confirmed against Wire.

### 20.2 Conformance limits

The numeric limits a conformant backend honors are tabulated in §18.4
(API version 14, ciphersuite `0x0002`, token TTLs, batch caps, asset
size, etc.). The MLS deviation set §10.7 (W-1…W-9) is the conformance
core for the MLS endpoints.

## 21. Document Maintenance

This specification freezes the **client-observable contract**. Changes
to §5–§15 after implementation starts require a corresponding
compatibility check against the Kupferdraht client (smoke tests). The Admin
API (§16) and the open questions (§19) are expected to evolve.

Provenance (non-normative): derived from Kupferdraht's wire layer and
live-validation notes through merged Step 26; freshness re-check
against merged Steps 27–42 on 2026-07-03 (revision note below).
Historical source basis: `kupferdraht-core/src/wire/http/*`,
`docs/wire/api-notes.md`, `docs/wire/mls-deviations.md` (W-1…W-9),
`docs/wire/asset-protocol.md`, `docs/adrs/` (ADR-0004, ADR-0008,
ADR-0011, ADR-0012), `docs/observations.md` / `CHANGELOG.md`,
`PLAN.md`.

Revision note (2026-06-11): updated for Steps 21–23 and CHANGELOG.md §51/§58 — typing notifications (§9.6, transient delivery §11.1), email
change (§5.6), delete-for-everyone with the tenant team ID (§4.1, §9.7,
`conversation.delete` and `conversation.typing` in §12.2, `team` now
non-null in `/self`), and long-lived `zuid` for cross-process resume
(§5.1, §18.4).

Revision note (2026-06-12): tenancy model restructured after Q1/Q2
discussion — deployment = one domain, tenant = team within it
(replacing tenant = domain with hard isolation and the synthetic team
UUID). Cross-team reach is per-team policy with hermetic defaults
(§4.3); handles unique per deployment; team membership is the
backend-attested identity anchor; real team roles (admin/member) gate
delete-for-everyone and the team-scoped Admin API. Resolves Q1, Q2,
Q6, Q10; adds Q11 (per-team SAML). Same day: Q4 resolved — added
Wire-compatible `PUT /v14/self/password` (§5.7) with uniform password
policy; initial-password flow via §6.2 + §5.7.

Revision note (2026-06-13): Q3 resolved — adopt Wire's full invitation
mechanism (`team_code` on `POST /register` §6.1b + admin invitation
management §16.2) rather than a simplified email allowlist, to stay
Wire-compatible and keep future options open. Invited accounts are born
active (no activation round-trip); `team_code` already present in
Kupferdraht's `NewUserRequest` DTO (unused), so the backend contract is
frozen now and the client path is a deferred Kupferdraht step (§17). Q5
resolved — Wire-faithful conversation-role/action model (§9.4): rename
needs `modify_conversation_name` (`wire_admin` default), backend tracks
per-member roles (§10.1) and enforces; Kupferdraht needs no change (reads no
role, surfaces `403`), optional client role-awareness deferred (§17).

Revision note (2026-06-14): Q7 resolved against Kupferdraht's openmls 0.8
integration — §10.5 rewritten. Server-side removal is an external-sender
Remove *proposal* committed by a remaining member (not a server commit,
which RFC 9420 disallows for a non-member external sender). The proposal
does not advance the epoch; the member's commit does (§10.2). Added
edge-case handling (no remaining members → drop group; offline members →
pending re-delivery), pending-proposal group state (§10.1).

Revision note (2026-06-14, main re-sync): re-checked spec against merged
Steps 24–26.
- **Step 24 (two-phase login / device-limit recovery):** `password` is
  optional on `POST /clients` and `DELETE /clients/{id}` (§7.1, §7.3) —
  required for 2nd+ device of a password account, omitted for the first
  device / SSO / device-limit recovery. No new endpoints.
- **Steps 25 (delivery status) & 26 (Location/Knock):** no backend
  impact — both are opaque MLS application messages (§10.3); delivery
  status is client-local + a `Confirmation(DELIVERED)` GenericMessage.
- **Q7 auto-commit VERIFIED MISSING:** audit confirms Kupferdraht stores then
  `clear_pending_proposals`-discards inbound external Remove proposals;
  no `commit_to_pending_proposals`, no test. ADR-0004 decided it
  (assigned to "Step 6"), never built. Pre-existing correctness bug →
  new **Kupferdraht Step 28** (`docs/plan/archive/28-external-proposal-autocommit.md`);
  §10.5, §17 updated.
- **Numbering fix:** the change-password plan drafted as "Step 24"
  collided with the merged Step 24; renumbered to **Step 27**
  (`docs/plan/archive/27-change-password.md`).

Revision note (2026-06-14, Q8): unbounded event queues resolved. No
per-event drop is MLS-safe (a lost `Commit`/`Welcome` breaks the
ratchet), so reclaim is whole-device purge only (§11.1). Added §11.6
device-inactivity purge — **disabled by default**, operator-enabled per
deployment, 180-day default window (§4.4 `device_purge_enabled` /
`device_purge_inactivity`). Purge = device deletion (§7.3 → §10.5),
client recovery verified graceful; gated on Step 28 to avoid ghost
leaves.

Revision note (2026-06-14, Q9): member-join/leave events resolved —
SHOULD emit (Wire does, for MLS convs too) but **advisory only**; the
MLS commit is the authoritative membership change, clients derive
membership from MLS state (§12.3). Kupferdraht's ignore-and-derive handling
is correct. With Q9 resolved, only Q11 (per-team SAML, deferred with
Kupferdraht Step 17) remains open.

Revision note (2026-07-03): freshness re-check against merged Steps
27–42; no endpoint, shape, or query-parameter divergence found — the
changes below are status prose, not contract changes.
- **§5.7** — Step 27 merged: the reference client now calls
  `PUT /v14/self/password`.
- **§10.5 / §17** — Step 28 merged: the VERIFIED-GAP callout is
  resolved; Kupferdraht validates and auto-commits external Remove
  proposals (`encryption/external_removal.rs`), so §10.5 path (2)
  removals materialize. The stale §17 bullet is removed (history stays
  in Q7).
- **§11.2 / §11.4 / §17 / §20.1** — the WebSocket push stream is marked
  "prototyped, deferred with Step 5": the reference client is pull-only
  via `GET /v14/notifications` with a persisted `since` cursor and
  never acknowledges; its queue is reclaimed only by device purge
  (§11.6). The socket frame/ack contract stays frozen for the future
  push step; a backend without it currently still passes §20
  conformance, and transient `conversation.typing` delivery is
  unreachable for a pull-only client.
- **§5.3 / §10.3** — the `403 invalid-credentials` (Step 39) and
  `409 mls-stale-message` (Step 37) labels are now load-bearing: the
  client switches on them (session death → re-login; stale epoch →
  re-sync and resend). Both labels were already in the §3.3 catalog.
- Steps 29/30/32/33 (module decompositions), 38 (in-order commit
  processing), 40 (device-reuse key fix), 41/42 (build config, KMP
  bindings) verified contract-neutral: no wire-layer surface change.
