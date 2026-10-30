# Wire API Notes (v14 target)

This document is the human-readable navigator on top of
`reference/wire.swagger.json`. It covers every Wire HTTP or WebSocket
endpoint Kupferdraht currently uses or plans to use, with:

- The v14 path, method, and what it does.
- Required headers and query / path / body parameters.
- A minimal request / response JSON example.
- Documented status codes and Wire's label convention for errors.
- Notes on what changed between v5 (our first implementation
  target) and v14 (the current pinned target per ADR-0008).
- The module in Kupferdraht where the endpoint is (or will be) wired.

Ordering is roughly by category (auth → client → MLS key material
→ conversations → event stream → user directory), matching the
order a fresh Kupferdraht session would exercise them.

---

## Conventions

### Base URLs

A Wire deployment splits HTTP and WebSocket traffic across two
hosts, by convention:

- **HTTP API:** `https://nginz-https.<domain>`
- **WebSocket:** `wss://nginz-ssl.<domain>`

Every authenticated HTTP endpoint sits under `/v{N}/…` after the base
(e.g. `https://nginz-https.<domain>/v14/conversations/list`).
Three endpoints remain **unversioned** for backward compatibility:
`/login`, `/access/logout`, and `/await` — the legacy event stream,
which is **alive and load-bearing again**: while wire-server ships
ConsumableNotifications hard-disabled, it is the only live push
channel, and Kupferdraht's negotiated WebSocket transport uses it (Step
73, ADR-0019 — see the deviation section below).

### Request / response format

All HTTP endpoints default to `application/json;charset=utf-8`,
with two MLS-specific exceptions:

- `POST /v14/mls/commit-bundles` — body is raw TLS-serialized
  MLS concat (`commit || group_info || welcome?`), content-type
  `message/mls`.
- `POST /v14/mls/messages` — body is a single TLS-serialized
  `MlsMessage(PrivateMessage)`, content-type `message/mls`.

### Authentication

Three channels, each with a narrow scope:

| Channel | Used for | Obtained from |
|---|---|---|
| `Authorization: Bearer <access_token>` | All authenticated HTTP API calls | `POST /login` or `POST /v14/access` |
| `Cookie: zuid=<refresh_token>` | `POST /v14/access` (refresh), `POST /access/logout` | `POST /login` response `Set-Cookie` header |
| `?access_token=<access_token>` query parameter | WebSocket upgrades only (browser-compat channel) | Same as Bearer — duplicated into the URL |

**Client-scoped endpoints (`/mls/commit-bundles`,
`/mls/messages`, and likely more) expect the `Z-Client` header
set to the calling device's `client_id`.** nginx strips
client-supplied `Z-Client` headers; the header visible to the
backend is injected by nginx from the access token's
`client_id` claim. This means the access token must be **bound
to the client** via `POST /v14/access?client_id=<id>` after
client registration. See LEARNINGS.md §1.4.

### Error bodies

Wire's JSON error body shape:

```json
{ "code": 400, "label": "mls-stale-message", "message": "..." }
```

- **`code`** matches the HTTP status.
- **`label`** is a stable machine-readable error kind, namespaced
  by concern (`mls-*`, `conversation-*`, etc.). Kupferdraht's
  `KupferdrahtError::BackendRejected` surfaces this field verbatim.
- **`message`** is a human-readable detail line.

Not every error returns JSON. When nginx itself rejects (auth
middleware, rate limit, path-not-routed), you get a **plain HTML
403 / 404** with no JSON envelope — always capture the raw body
text as fallback.

### Status codes

- `200 OK` — standard success for GETs and token-yielding POSTs.
- `201 Created` — standard success for resource-creating POSTs
  (conversation, client, MLS commit, MLS message). `is_success()`
  covers both 200 and 201.
- `204 No Content` — some DELETE and update paths.
- `400 Bad Request` — shape-level issues plus most MLS-specific
  rejections (stale epoch, missing group info, etc.). Check the
  `label` field.
- `401 Unauthorized` — access token invalid/expired.
- `403 Forbidden` — authorization failure (token doesn't carry
  the claim the endpoint needs, or nginx denied at the edge).
- `404 Not Found` — resource doesn't exist on this user's behalf.
- `409 Conflict` — MLS epoch race, add-member-already-member,
  duplicate commit.
- `422 Unprocessable Entity` — openmls-validation failure
  (malformed commit bundle, etc.).
- `426 Upgrade Required` — WebSocket upgrade rejected.
- `429 Too Many Requests` — rate limit (label `too-many-requests`,
  observed on bursts of `DELETE /clients/{id}`, observations.md).
  Kupferdraht maps every `429` on the shared rejection path and on the token
  refresh (`POST /access`) to `KupferdrahtError::RateLimited`, reading
  `Retry-After` as delta-seconds
  (an HTTP-date is not parsed → no hint); the sync loops wait it out
  (Step 103).
- `533` — Wire-internal "proposal execution failed" status code
  (used by `/mls/messages` and `/mls/commit-bundles` when a
  commit's side effects — membership change, etc. — can't be
  applied).

---

## Endpoints

### 1. Server discovery

#### `GET /api-version` (unversioned)

Advertise supported API versions.

**Response shape (v14):**

```json
{
  "supported": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
  "development": [],
  "federation": true,
  "domain": "wire.com"
}
```

**Kupferdraht usage:** `kupferdraht-core/src/wire/http/mod.rs::api_version`.
Called once at `login` time before any authenticated call.
Client pins `CLIENT_SUPPORTED = &[14]` per ADR-0008; if the
server's `supported` list does not include 14, login fails with
`KupferdrahtError::Generic("server does not support API v14")`.

**v5 → v14:** identical shape. `development` may list beta
versions on a deployment running a pre-release wire-server (e.g. `[15]` while v14 is latest stable).

---

### 2. Authentication

#### `POST /login` (unversioned)

Exchange email + password (+ optional 2FA code) for an access
token and a refresh-token cookie.

**Query parameters:** `?persist=true` (we always pass this) —
returns a long-lived zuid cookie rather than a session cookie.

**Request body:**

```json
{
  "email": "alice@example.com",
  "password": "...",
  "label": "kupferdraht-cli",
  "verification_code": "123456"
}
```

- `email` XOR `handle` XOR `phone` — one of three identity
  channels. Kupferdraht only uses `email` today.
- `label` is the cookie label Wire displays in the user's
  device-list UI.
- `verification_code` is the 2FA code, only present on retry
  after a `403 code-authentication-required` response.

**Response 200:**

```json
{
  "user": "<uuid>",
  "access_token": "<opaque JWT-like token>",
  "expires_in": 900,
  "token_type": "Bearer"
}
```

Plus `Set-Cookie: zuid=<refresh-token>; …` header. Extract the
`zuid` value from the cookie for subsequent `/access` refreshes.

**Status codes:**

- `200` — success.
- `403 code-authentication-required` — 2FA code required.
- `403 code-authentication-failed` — 2FA code wrong.
- `403 invalid-credentials` (no label) — email/password wrong.

**Kupferdraht usage:** `kupferdraht-core/src/wire/http/auth.rs::login`.

**v5 → v14:** shape unchanged.

---

#### `POST /v14/access`

Exchange a refresh-token cookie for a new access token. Used
both on initial login (to rebind the token to a newly-registered
`client_id`) and on periodic refresh (~every 10 minutes, before
the 15-minute expiry).

**Query parameters:**

- `client_id=<id>` (optional but **effectively required** after
  client registration, see LEARNINGS.md §1.4) — binds the
  issued access token to this client. nginx subsequently injects
  `Z-Client: <id>` downstream for endpoints that require client
  scoping.

**Headers:**

- `Cookie: zuid=<refresh-token>` — only auth channel; no Bearer.

**Request body:** empty.

**Response 200:**

```json
{
  "user": "<uuid>",
  "access_token": "<new opaque token>",
  "expires_in": 900,
  "token_type": "Bearer"
}
```

May carry a new `Set-Cookie: zuid=<rotated-refresh-token>; …`.
Persist the new cookie if present; keep the old one otherwise.

**Status codes:**

- `200` — success.
- `403` — refresh cookie invalid / expired / revoked
  (server-side logout from another device, password change).
  Kupferdraht treats this as "session invalidated" and clears local
  state.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/auth.rs::rebind_access_token`. Called
once during `ClientBuilder::login` after client registration to
bind the token to the new `client_id`, and again by
`ClientBuilder::resume` when the persisted access token has aged
out. A cross-process rebind with the persisted `zuid` **works**
(live-confirmed against a Wire v14 backend with a ~15-h-old cookie —
CHANGELOG.md §51, superseding the earlier §24 belief that it failed,
which was a symptom of the §18 cookie collision). `resume` therefore
refreshes a stale token in place; a fresh `login` is only needed once
the `zuid` itself expires or is revoked.

Without `client_id` the call mints a **user-scoped** token for whoever
the cookie belongs to (`user` in the body). Two uses:
`access_from_sso_cookie` (SSO finalize, before any client exists) and,
in the browser, `access_with_browser_cookie` (Step 111b): the browser's
cookie jar carries `zuid` (`credentials: 'include'`, no explicit
`Cookie` header), and the server mode's `resume` needs a Wire token
before the account's databases open — the `client_id` a rebind would
name sits in them. Kupferdraht checks `user` against the account being
resumed; another user's cookie (one jar per backend domain) or a `403`
is `SessionExpired`.

**v5 → v14:** shape unchanged. Kalium's `AccessTokenApiV0`
through `V15` all delegate to the same V0 logic.

---

#### `POST /access/logout` (unversioned)

Revoke the current access token and its associated refresh
cookie.

**Headers:**

- `Authorization: Bearer <access_token>`
- `Cookie: zuid=<refresh-token>`

**Request body:** empty.

**Response 200:** empty body.

**Status codes:**

- `200` — success.
- `403` — token already expired or invalid. Kupferdraht treats this
  as "local cleanup wins" — logout returns exit 0 regardless.

**Kupferdraht usage:** `kupferdraht-core/src/wire/http/auth.rs::logout`
(not yet wired into a CLI subcommand).

**v5 → v14:** shape unchanged.

---

### 2b. SSO login (Step 17β — interactive / SP-driven)

Kupferdraht drives Wire's SP-side SSO endpoints; the SAML round-trip and signature
validation happen on Wire's server. Kupferdraht builds the initiate redirect URL and,
after the browser round-trip, finalises the session from the returned `zuid`
cookie (17b). **These `/sso/*` paths are treated as unversioned (spar service);
confirm vs. `/v14/sso/*` on live-probe.**

#### `GET | HEAD /sso/initiate-login/{idp}`

- `{idp}` — the SSO **code** identifying the team's configured IdP.
- Query: `success_redirect`, `error_redirect` — where Wire redirects the browser
  after the IdP round-trip (success carries the `zuid` cookie; failure carries an
  error code). Kupferdraht url-encodes both.
- `GET` returns the IdP redirect (opened in a browser). `HEAD` validates the code
  without initiating: **200** → code resolves to an IdP, **404** → unknown code.
- Kupferdraht: `HttpClient::sso_initiate_login_url` (builds the URL, pure) +
  `HttpClient::sso_code_exists` (`HEAD`, 200→true / 404→false).

#### `POST /sso/finalize-login` (17b — not called by the client)

The IdP→Wire callback: Wire receives the SAML assertion, sets the `zuid`
`Set-Cookie`, and redirects the browser to `success_redirect`. The client never
calls this directly — it catches the redirect and extracts the cookie.

#### `GET /sso/settings` (17c)

Returns the team's **default** SSO code — used for email-domain discovery
(single-IdP teams). Per-domain vs. team-default semantics: live-probe (17c).

**Open questions (live-probe):** endpoint versioning; whether the `zuid` is
delivered as a `?cookie=…` query on `success_redirect` or a `Set-Cookie` the app
reads; whether `POST /access` with no `client_id` mints the first token from a
bare `zuid`. See `docs/plan/17-saml-sso.md`.

---

### 3. Client (device) registration

#### `POST /v14/clients`

Register a new device for the authenticated user. Required
before any MLS operation because MLS identities are tied to a
device's `client_id`.

**Request body:**

```json
{
  "capabilities": ["legalhold-implicit-consent"],
  "class": "desktop",
  "cookie": "kupferdraht-cli",
  "label": "kupferdraht-cli",
  "lastkey": { "id": 65535, "key": "<base64 CBOR last-resort prekey>" },
  "mls_public_keys": {
    "ecdsa_secp256r1_sha256": "<base64 MLS signature public key>"
  },
  "model": "kupferdraht-cli",
  "password": "...",
  "prekeys": [{ "id": 0, "key": "<base64 CBOR Proteus prekey>" }, ...],
  "type": "permanent"
}
```

- `password` re-confirms the user's login password.
- `prekeys` + `lastkey` are **Proteus** prekeys — required by
  Wire's schema even though Kupferdraht is MLS-only (see ADR-0001
  and ADR-0010). Generated by Kupferdraht's own encoder
  (`encryption::prekeys`, Step 9; format below); uploaded once and
  never used for encryption.
- `capabilities` declares client behavior. `legalhold-implicit-
  consent` is required; `consumable-notifications` signals v9+
  event-stream support (see `/v14/events`).
- `mls_public_keys` may be set here OR later via
  `PUT /v14/clients/{client_id}`. Kalium splits: register
  client first (gets `client_id` back), then upload MLS public
  key as a separate PUT. Kupferdraht currently uses the two-step
  flow.

**Prekey CBOR format.** Each `key` is the base64 of a proteus
`PreKeyBundle` serialised as CBOR (RFC 8949): a 5-entry map with
integer keys 0–4 — version `1`, prekey id (`u16`), the public key
as a 1-entry map around the 32 raw key bytes, the identity key as
a 1-entry map around that same envelope, and `signature` = `null`
(Kupferdraht emits unsigned bundles). The last-resort `lastkey`
carries id `65535`. The format was observed from Wire's `proteus`
(GPL-3.0) at tag v2.1.1, the dependency Kupferdraht carried through
Step 8 (ADR-0010):
[`PreKeyBundle::encode`](https://github.com/wireapp/proteus/blob/b92dbc2d0c77105cae3911a7388acba05450a06d/src/internal/keys.rs#L307-L308),
[`PreKey::encode`](https://github.com/wireapp/proteus/blob/b92dbc2d0c77105cae3911a7388acba05450a06d/src/internal/keys.rs#L194-L195),
[`IdentityKey::encode`](https://github.com/wireapp/proteus/blob/b92dbc2d0c77105cae3911a7388acba05450a06d/src/internal/keys.rs#L52-L53) and
[`MAX_PREKEY_ID`](https://github.com/wireapp/proteus/blob/b92dbc2d0c77105cae3911a7388acba05450a06d/src/internal/keys.rs#L358). Kupferdraht's encoder
(`kupferdraht-core/src/encryption/prekeys.rs`) emits the same bytes:
checked through proteus-wasm's deserialiser until Step 9c and live
against Wire on 2026-05-05 (Step 9d). Nothing of proteus is copied;
the wire format itself is not protectable (ADR-0010 §Legal
considerations).

**Response 201:**

```json
{
  "id": "<16-hex-char client_id>",
  "type": "permanent",
  "class": "desktop",
  "time": "2026-04-22T12:00:00.000Z",
  "capabilities": ["legalhold-implicit-consent"],
  "cookie": "kupferdraht-cli",
  "label": "kupferdraht-cli",
  "model": "kupferdraht-cli",
  "mls_public_keys": {}
}
```

**Status codes:**

- `201` — created.
- `400` — prekey shape wrong, or required field missing.
- `403 too-many-clients` — user already has 7 devices
  registered. Call `DELETE /v14/clients/{id}` on an old device
  first or use `logout --purge-client`.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/clients.rs::register_client`, driven by
`client/login_pipeline.rs`.

**v5 → v14:** `capabilities` may now include
`consumable-notifications` (v9+). `mls_public_keys` format
hasn't changed.

---

#### `PUT /v14/clients/{client_id}`

Update a registered device. Kupferdraht uses this path to upload
the MLS public key after client registration (split from the
initial `POST /v14/clients` because the `client_id` isn't known
at registration time).

**Path parameters:** `client_id`.

**Request body (Kupferdraht's usage):**

```json
{
  "mls_public_keys": {
    "ecdsa_secp256r1_sha256": "<base64 signature public key>"
  }
}
```

**Response:** 200 (no body).

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/clients.rs::update_client_mls_public_key`
(uploads the freshly-generated MLS signature public key) plus
`update_client_capabilities` (PUTs `consumable-notifications` +
`legalhold-implicit-consent` per observations.md §22). Both run from
`client/login_pipeline.rs` immediately after `register_client`. Required before
the first `POST /v14/mls/key-packages/self/{id}` upload (observations.md §19) — without it Wire returns
`400 mls-protocol-error: "No key associated to the given identity
and signature scheme"`.

**v5 → v14:** shape unchanged. Key-type names
(`ecdsa_secp256r1_sha256`, `ed25519`, …) are the same enum
`MLSPublicKeyType` across versions.

---

#### `DELETE /v14/clients/{client_id}`

Deregister a device. Wire requires the account password for
permanent clients (kupferdraht-cli falls into this category).

**Path parameters:** `client_id`.

**Request body:**

```json
{ "password": "..." }
```

**Response 200:** empty body.

**Kupferdraht usage:** not yet wired — Kupferdraht does not currently
expose a delete-device path. Stale CLI clients are pruned via the
Wire UI when the per-user 7-device cap is hit.

**v5 → v14:** shape unchanged.

---

#### `GET /v14/clients`

List the authenticated user's registered devices. Kupferdraht does
not currently use this but production-UX cleanup (finding which
stale kupferdraht-cli clients can be pruned when the 7-slot limit is
hit) will need it.

---

### 4. MLS key material

#### `GET /v14/mls/public-keys`

Fetch the backend's external-sender public keys — used to
validate administrative Remove proposals that Wire itself
issues (see ADR-0004 and [`mls-deviations.md`](mls-deviations.md) W-5).

**Response 200:**

```json
{
  "removal": {
    "ed25519": "<base64>",
    "ecdsa_secp256r1_sha256": "<base64>",
    "ecdsa_secp384r1_sha384": "<base64>",
    "ecdsa_secp521r1_sha512": "<base64>"
  }
}
```

Kupferdraht picks the entry matching `MLS_PUBLIC_KEY_TYPE`
(`ecdsa_secp256r1_sha256` per our P-256 ciphersuite
choice — see CHANGELOG.md §13).

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/mls.rs::fetch_mls_backend_public_keys`.
Called once at login time and persisted as
`backend_mls_keys` row in `kupferdraht.db`; consumed by
`MlsClient::create_group` (Step 6a) to populate the
`external_senders` extension.

**v5 → v14:** shape unchanged; the map may grow with new
signature schemes over time.

---

#### `POST /v14/mls/key-packages/self/{client}`

Upload a fresh batch of our own KeyPackages so other users can
claim them to add us to groups.

**Path parameters:** `client` — our client id.

**Request body:**

```json
{
  "key_packages": [
    "<base64 TLS-serialized KeyPackage>",
    "..."
  ]
}
```

Kupferdraht currently uploads `INITIAL_KEY_PACKAGE_COUNT = 100` at
login time. Pool refresh is not yet implemented — once Volker
has been claimed into ~100 groups the pool will go empty and
peer invites of him will fail upstream with empty `key_packages`
arrays. Refill is on the backlog.

**Response 201:** empty body.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/mls.rs::upload_key_packages`, called
once at login time from `client/login_pipeline.rs::upload_initial_key_packages`.

**v5 → v14:** shape unchanged.

---

#### `POST /v14/mls/key-packages/claim/{user_domain}/{user}`

Claim one KeyPackage per client of a target user. Consumed by
the sender when building an Add commit.

**Path parameters:** `user_domain`, `user` (qualified-id pair).

**Query parameters:**

- `ciphersuite=<decimal>` **required**. Note: decimal, not hex
  (see `CHANGELOG.md` resolved `fix: claim_key_package passes
  ciphersuite as decimal, not hex`).
- `skip_own=<client_id>` — semantics depend on the target user:
    - **Self-claim** (`user == self`): the field **MUST** be set
      to the caller's `client_id`, otherwise Wire defensively
      returns an empty pool. Kupferdraht uses this for the
      self-bootstrap step in `Client::invite_members` so every
      other Wire client the user owns becomes a leaf in the
      same Add-commit (observations.md §27).
    - **Other-user claim** (`user != self`): the caller's
      `client_id` cannot appear in the target's pool, so
      `skip_own` is a functional no-op. Kupferdraht passes `None`
      for clarity.

**Response 200:**

```json
{
  "key_packages": [
    {
      "client": "<target's client_id>",
      "domain": "wire.com",
      "key_package": "<base64 TLS-serialized KeyPackage>",
      "key_package_ref": "<base64 hash reference>",
      "user": "<target's uuid>"
    }
  ]
}
```

Empty `key_packages: []` means the target's pool is empty — the
target's client hasn't recently topped up KeyPackages. Not a
Kupferdraht bug; users of infrequently-active clients regularly hit
this.

**Wire's full-device-coverage rule** (relevant when the claimed
KPs are about to feed an Add-commit): every device of every
member-user must end up as a leaf in the post-commit group.
A claim that returns fewer KPs than the target user has active
clients will trip `409 mls-client-mismatch` at commit-bundle
upload time (observations.md §27). Pool-refresh on the target side is
out-of-band — Kupferdraht can't fix it client-side.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/key_packages.rs::claim_key_packages`.
Driven by `Client::invite_members`'s self-bootstrap and
per-invitee claim loops.

**v5 → v14:** shape unchanged. `ciphersuite` query param exists
in every version ≥ v5; earlier versions may have taken it as a
body field.

---

#### `GET /v14/mls/key-packages/self/{client}/count`

Return the count of unclaimed KeyPackages currently on the
server for this client under a given ciphersuite. Kupferdraht uses it
since Step 63 to decide when to refill the pool
(`Client::replenish_key_packages`, auto-checked from the sync
surface).

**Query parameters:** `ciphersuite` — **required** (current
wire-server answers `400 Bad Request: Query parameter ciphersuite
is required` without it, live-observed 2026-08-13; the pinned
swagger does not mark it). Sent decimal (`?ciphersuite=2` for
P-256), same convention as the claim endpoint.

**Response 200:** `{ "count": 100 }`

---

### 5. Conversations

#### `POST /v14/conversations`

Create a new conversation (group or channel).

**Request body (what Kupferdraht actually sends, live-validated
2026-04-27 / -28):**

```json
{
  "name": "My Group",
  "protocol": "mls",
  "group_conv_type": "group_conversation",
  "qualified_users": []
}
```

Notes:

- `qualified_users` is intentionally empty for the zero-invitee
  flow (Step 6a). The first invite happens via Step 6b's
  `Room::invite` → `/v14/mls/commit-bundles` Add-commit, **not**
  via this body.
- `creator_client`, `access`, `access_role`, `conversation_role`
  are **not required** — Wire derives the creator from the
  Bearer token's `Z-Client` claim, and the team-default access
  policies kick in when the fields are omitted. The minimal
  4-field body above produces a working group on a Wire v14 backend.
- `group_conv_type` is `"group_conversation"` for groups,
  `"channel"` for channels (Kupferdraht does not target channels in
  scope today).
- The created MLS group starts at **epoch 0 with one leaf**, where Wire
  rejects application messages (`mls-protocol-error: "Application
  messages at epoch 0 are not supported"`). `create_room` therefore
  issues an **establishing commit** right after the local group create,
  advancing epoch 0 → 1 so the room is sendable immediately: it claims
  the creator's own remaining clients' KeyPackages (`skip_own` the
  current device) and Adds them, or — for a solo creator with no other
  device — issues a **self-update** commit. Both go to
  `POST /v14/mls/commit-bundles`. The earlier "inert until first invite"
  behavior (CHANGELOG.md §25) is superseded by observations.md §61;
  live-validated 2026-06-16.

**Response 201:**

```json
{
  "qualified_id": { "id": "<uuid>", "domain": "wire.com" },
  "type": 0,
  "protocol": "mls",
  "group_id": "<base64>",
  "epoch": 0,
  "cipher_suite": 2,
  "name": "My Group",
  "members": { "self": { … }, "others": [] }
}
```

`cipher_suite` is an integer — Wire's default is **2**
(P-256/ECDSA) per CHANGELOG.md §13.

**Status codes:**

- `201` — success.
- `400` — shape / permission / capability issue (error label
  details).
- `401` — access token expired (Wire's TTL is ~15 min).
  `ClientBuilder::resume` refreshes the token in place via an
  `/access` rebind with the persisted `zuid` (CHANGELOG.md §51); a
  fresh `login` is only needed once the `zuid` itself expires.
- `409` — name collision or similar.
- `533` — Wire-internal "commit execution failed".

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/conversations.rs::create_conversation`.

**v5 → v14:** `group_conv_type` field was added around v5/v6
to distinguish groups from channels. `add_permission` and
other channel-specific fields may have been added in v7+.

---

#### `POST /v14/conversations/list-ids`

Paginated listing of all conversation IDs the user is a member
of.

**Request body:**

```json
{ "paging_state": null, "size": 1000 }
```

**Response 200:**

```json
{
  "qualified_conversations": [
    { "id": "<uuid>", "domain": "wire.com" }
  ],
  "has_more": false,
  "paging_state": "<opaque>"
}
```

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/conversations.rs::list_conversation_ids`.

**v5 → v14:** shape unchanged.

---

#### `POST /v14/conversations/list`

Fetch conversation metadata for a list of IDs (obtained via
`list-ids`).

**Request body:**

```json
{
  "qualified_ids": [
    { "id": "<uuid>", "domain": "wire.com" },
    ...
  ]
}
```

**Response 200:**

```json
{
  "found": [<Conversation>, ...],
  "not_found": [<QualifiedId>, ...],
  "failed":    [<QualifiedId>, ...]
}
```

Each `Conversation` carries `qualified_id`, `type` (0=group,
1=self, 2=one-on-one, 3=channel), `protocol`, `group_id`,
`epoch`, `cipher_suite`, `name`, `members.self`, `members.others`.

The MLS **self conversation** (`type: 1`, `members.self` only) is
listed from the user's first `list-ids` on — at `epoch: 0` with a
`group_id` while nobody has established it. Unlike the 1:1 (Step 68
below) it is never hidden; it is the one room a fresh account's
snapshot carries without any `GroupInfo` behind it (live 2026-09-24,
Step 86 — see the discovery section below).

**Cap.** At most 1000 `qualified_ids` per request (Wire's schema).
`Client::sync_metadata` sends the `list-ids` result in chunks of that
size (Step 95c); a single oversized request was rejected, failing the
metadata sync of an account in more than 1000 conversations on every
cycle. A `found` entry whose `qualified_id` has an empty `id` or
`domain` is skipped; an empty `group_id` means no MLS group.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/conversations.rs::list_conversations`.

**v5 → v14:** the **path** changed at the v5 boundary. v0 uses
`/conversations/list/v2` (historical marker). v5+ uses
`/v{N}/conversations/list` as a regular versioned path. Kupferdraht
already uses the v5+ form.

---

#### `GET /v14/conversations/{cnv_domain}/{cnv}/groupinfo`

Fetch the current `GroupInfo` for an established MLS
conversation. Required input for `external-commit` join (Step
2b/1).

**Path parameters:** `cnv_domain`, `cnv`.

**Request body:** none.

**Response 200:** raw bytes, content-type `message/mls`. The
payload is the raw TLS-serialized `VerifiableGroupInfo` — **not**
wrapped in an `MlsMessage` envelope (W-1 in `mls-deviations.md`,
live 2026-04-25; Kupferdraht's parser tolerates both framings).

**Response 404 `mls-missing-group-info`** (`"The conversation has
no group information"`) while the conversation has no `GroupInfo`
on file — every conversation before its first commit bundle, and
in practice the never-established self conversation of every user
(live 2026-09-24, Step 86). An external commit is impossible in
that state; the client establishes the group itself.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/groups.rs::fetch_group_info`.
Consumer: `MlsClient::join_by_external_commit`.

**v5 → v14:** the endpoint exists from v5 onward (Kalium's
`ConversationApiV5` introduces it). v14 shape unchanged.

---

### 6. MLS commit + message upload

#### `POST /v14/mls/commit-bundles`

Upload a commit bundle. Used by external-commit rejoin (Step 3),
`Room::invite` (Step 6b), and Step 6d's `Room::kick`.

**Headers:**

- `Authorization: Bearer <token>`
- `Z-Client: <our client_id>` (auto-injected by nginx from
  token claim — **do not rely on client-supplied headers**).
- `Content-Type: message/mls`

**Request body:** raw bytes, concatenation of TLS-serialized
segments:

```
commit     : MlsMessage(PublicMessage | PrivateMessage)
group_info : MlsMessage(GroupInfo)           ← wrapped, not raw
welcome?   : MlsMessage(Welcome)             ← optional, Add-commits only
```

Per-flow segment count:

| Flow                         | Segments | Layout |
|------------------------------|----------|--------|
| External commit (rejoin)     | 2        | commit ‖ group_info |
| Add-commit (`Room::invite`)  | 3        | commit ‖ group_info ‖ welcome |
| Remove-commit (`Room::kick`) | 2        | commit ‖ group_info |

**Every segment MUST be MlsMessage-framed** (docs/observations.md and
LEARNINGS.md §1.5). Raw GroupInfo bytes cause `400 "Unknown
ContentType: N"`.

**Wire-format under MIXED_PLAINTEXT_WIRE_FORMAT_POLICY:**
handshake messages (commits, welcomes, group_info) ship as
`PublicMessage`; application messages remain `PrivateMessage`.
**Wire's commit-bundle endpoint can only inspect plaintext
commits** — it holds no group secrets and cannot decrypt
`PrivateMessage`-framed commits. Any handshake commit shipped
under `PURE_CIPHERTEXT_WIRE_FORMAT_POLICY` is folded into
`400 bad-request: "Missing commit"` regardless of epoch
(live-confirmed 2026-04-27 against epoch 0 from
`encryption/group_create.rs` and 2026-04-29 against an
externally-committed group at epoch 13, see observations.md §28).
`encryption/external_commit.rs` therefore explicitly sets
MIXED_PLAINTEXT on its `MlsGroupJoinConfig::builder()`;
`encryption/group_invite.rs::add_members_commit` back-fills the
same policy on legacy groups before emitting the commit.

**Response 201:** JSON with the list of events the commit
emitted (member-add events, welcome delivery, etc.) — Kupferdraht
currently ignores this.

**Status codes:**

- `201` — accepted.
- `400 mls-protocol-error` — openmls-level rejection. Notable
  sub-messages observed live:
    - `"Application messages at epoch 0 are not supported"` —
      not a commit-bundle error per se but a sibling rejection
      from `/mls/messages` against an inert epoch-0 group
      (CHANGELOG.md §25, closed by Step 6b).
    - `"Tried to add invalid LeafNode: Client identity does not
      match credential identity"` — emitted when an Add-commit
      carries a `force_self_update=true` path-update leaf. Use
      openmls's `add_members_without_update` instead (observations.md §26).
    - `"Missing commit"` — `PrivateMessage`-framed commit Wire's
      parser cannot inspect (no group secrets server-side).
      Cause is `PURE_CIPHERTEXT_WIRE_FORMAT_POLICY`, not the
      epoch or the presence/absence of a path-update — a Kupferdraht
      commit always lands in `PublicMessage` framing once
      MIXED_PLAINTEXT is set on the group (observations.md §28). The
      earlier "empty path" reading of this label was a
      misinterpretation of the symptom.
- `400 mls-*` — other openmls-level rejections (stale epoch,
  missing ratchet tree, bad signature, …).
- `409 mls-client-mismatch` — `"A proposal of type Add or
  Remove does not apply to the full list of clients for a
  user"`. Wire enforces that every device of every member-user
  has a leaf. `Client::invite_members` claims the caller's own
  KPs (`skip_own=current_device`) and adds them in the same
  commit to satisfy this; subsequent invites dedupe via leaf
  signature key (observations.md §27).
- `409 mls-stale-message` — epoch race (another commit landed
  first).
- `422` — malformed bundle.
- `533` — proposal execution failed server-side.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/groups.rs::upload_commit_bundle`,
driven by `MlsClient::join_by_external_commit`,
`MlsClient::add_members_commit`, and (future)
`MlsClient::remove_members_commit`.

**v5 → v14:** shape unchanged; response event types may have
been added over time.

---

#### `POST /v14/mls/messages`

Upload an MLS application message (e.g. an encrypted
`GenericMessage`).

**Headers:** same as `/mls/commit-bundles` (Bearer + Z-Client
from token claim + `message/mls`).

**Request body:** raw TLS-serialized `MlsMessage(PrivateMessage)`
— the ciphertext produced by
`openmls::MlsGroup::create_message`.

**Response 201:** `{ "events": [...] }` list of events the
message produced (delivery acks, subconversation updates). Body
ignored today.

**Status codes:** same as commit-bundles (400 mls-*, 409, 422, 533).

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/messages.rs::send_mls_message`,
driven by `Timeline::send_text` (Step 4).

**v5 → v14:** shape unchanged.

---

### 7. Event stream (WebSocket)

#### `GET /v14/events` (WebSocket, v9+) — **implemented (Step 62)**

ConsumableNotifications model (v9+). Long-lived WebSocket
delivering events; each event must be acknowledged with an
`EventAcknowledgeRequest` or it stays in the queue.

**URL:**
`wss://nginz-ssl.<domain>/v14/events?client=<id>&sync_marker=<uuid>&access_token=<token>`

**Query parameters:**

- `client=<client_id>` — our device id.
- `sync_marker=<uuid>` — a client-generated UUID that Wire uses
  to correlate this session to previous ones for backfill.
- `access_token=<token>` — WebSocket-compatible auth channel
  (Bearer header is stripped by nginx on upgrade; see
  LEARNINGS.md §1.6).

**Response:** `101 Switching Protocols`, then a stream of
WebSocket frames. Each frame is a JSON `ConsumableNotification`
envelope; we ack each one before it gets removed from the
queue. The normative frame/ack/reconnect contract is
`docs/backend-spec.md` §11.2–§11.5 (live-derived from Wire
a live backend during the Step-5d investigation).

**Kupferdraht usage:** implemented in Step 62 —
`kupferdraht-core/src/wire/websocket.rs` (upgrade, frame parse, ack
encode) + `kupferdraht-core/src/sync/push/` (connection state
machine behind `SyncTransport::WebSocket`); events feed the same
MLS-decrypt + `EventHandler` pipeline as the `/notifications`
drain. Acks are sent strictly after the cursor persist.

**v5 → v14:** **fundamental change**. v0–v8 used
`/await` (unversioned) with no ack protocol; events stayed in
the queue based on a server-side cursor the client could
rewind via `/notifications?since=<id>`. v9+ moved to per-event
ack + sync-marker to support reliable multi-device delivery.
See observations.md §15 and ADR-0008.

---

#### `GET /await` (WebSocket, v0–v8) — **load-bearing again (Step 73)**

Classic unversioned event stream. The April diagnosis ("dead for
new work — no usable frames") was a client-side struct mismatch:
cannon frames JSON in *binary* WebSocket frames (observations §68
corrects §15). Since wire-server 5.30 hard-disables
ConsumableNotifications upstream, this route is the one gundeck
actually pushes through — Kupferdraht's negotiated fallback since
Step 73 (ADR-0019), used as a doorbell for the REST drain since
Step 97b (ADR-0030). Full connect/auth/frame contract: the
"Step 73 deviation" section below.

---

#### `GET /v14/notifications`

REST backfill for missed events. Used on reconnect to catch up
on events delivered while the WebSocket was down.

**Query parameters:**

- `since=<uuid>` — the id of the last event we processed. Wire
  returns everything more recent.
- `client=<client_id>` — scope to this device.
- `size=<100..10000>` — page size.

**Response 200:**

```json
{
  "notifications": [<Event>, ...],
  "has_more": false,
  "time": "2026-04-22T12:00:00.000Z"
}
```

**Response 404 `not-found`** — `since` names a notification the
queue no longer holds (probed live 2026-09-28):
`{"code":404,"label":"not-found","message":"Some notifications not found"}`,
`application/json;charset=utf-8`. An unparseable `since` is a `400`
with a plain-text body ("Error parsing query parameter since failed:
invalid UUID").

**Kupferdraht usage:** `SyncService::drain_pending` — the polling drain and
the legacy `/await` socket's doorbell (ADR-0030). Only the labelled `404 not-found` on a
request with `since` resets the cursor (Step 102); any other `404` — a
proxy's error page — is `BackendRejected`.

**v5 → v14:** shape essentially unchanged. Event types inside
the response have grown over versions (team events, feature
config events, MLS events).

---

### 8. User directory

#### `POST /v14/list-users`

Resolve user identifiers to profile records. Takes either
qualified ids or qualified handles — Wire overloads the same
endpoint with two body shapes.

**Request body (qualified-id shape):**

```json
{
  "qualified_ids": [
    { "id": "<uuid>", "domain": "wire.com" }
  ]
}
```

**Request body (qualified-handle shape):**

```json
{
  "qualified_handles": [
    { "handle": "alice", "domain": "wire.com" }
  ]
}
```

**Response 200:**

```json
{
  "found": [<UserProfile>, ...],
  "failed": [<QualifiedId | QualifiedHandle>, ...]
}
```

Each `UserProfile` carries `qualified_id`, `name`, `handle`,
`accent_id`, `assets`, `team`, etc.

**Kupferdraht usage:**
`kupferdraht-core/src/wire/http/users.rs::list_users` (qualified-id
variant only — handle resolution is not used by Kupferdraht today).
Consumed by `Client::sync_metadata` and
`Client::refresh_members` to populate display names / handles
on non-self room members.

**v5 → v14:** shape unchanged. Kalium's `UserDetailsApiV0` is
the reference.

---

### 9. Self-profile

#### `GET /v14/self`

Fetch the authenticated user's own profile.

**Response 200:**

```json
{
  "id": "<uuid>",                      // deprecated; use qualified_id
  "qualified_id": { "id": "<uuid>", "domain": "wire.com" },
  "name": "Alice",
  "handle": "alice",
  "email": "alice@example.com",
  "accent_id": 3,
  …
}
```

**Kupferdraht usage:** `kupferdraht-core/src/wire/http/auth.rs::fetch_self`.
Called once at login time to populate the `self_users` row in
`kupferdraht.db` (`Account::display_name` / `handle` / `email` /
`domain` come from this).

**v5 → v14:** shape unchanged in the fields Kupferdraht reads.

---

## Endpoints Kupferdraht does not (yet) use

For completeness, the following appear in the v14 Swagger under
`paths` and are relevant to future Steps but unimplemented today:

- `GET /v14/conversations/{domain}/{id}/groupinfo` (already
  covered under Step 2b/1).
- `GET /v14/conversations/mls-self` — the user's MLS self
  conversation as an `OwnConversation`. Not needed: `list-ids` +
  `list` already carry it, epoch 0 included (Step 86).
- `POST /v14/mls/reset-conversation` — membership reset edge
  case; not in Kupferdraht's scope.
- `GET /v14/teams/{tid}/features/mls` — team MLS-enablement
  flag; needed if Kupferdraht ever supports team-scoped policy
  enforcement.
- `GET /v14/teams/{tid}/features/mlsE2EId` — e2ei-enablement
  flag; explicitly out of Kupferdraht's scope (ADR-0001).

See `reference/wire.swagger.json` for the authoritative spec of
any endpoint not covered above.

---

## Step 122 discovery: a guest link names its conversation only through its own backend

Guest links are issued by `POST /v14/conversations/{cnv}/code` (Step 8)
and redeemed by `POST /v14/conversations/join` with `{key, code,
password?}`. Measured on the test stage (2026-10-05), the issued `uri` is

```
https://account.<operator>/conversation-join/?key=<key>&code=<code>
```

— the operator's account page, with no domain: the host is the account
host, not the conversation's domain. Every code endpoint is unqualified
(operation ids `join-conversation-by-code-unqualified`,
`create-conversation-code-unqualified`, `remove-code-unqualified`;
`GET /conversations/join` and `POST /conversations/code-check` take no
domain either), so a redeem reaches the caller's own backend only, and a
code of another operator answers `404 no-conversation-code`.

`GET /v14/conversations/join?key=<key>&code=<code>` answers the
conversation a link names before the redeem (`ConversationCoverView`:
`{id, name?, has_password}`, `id` unqualified). `Client::join_via_link`
reads the id there and picks the room with that id after the redeem
(Step 122); the redeem answers `200` with an `Event`, or `204` for a
member already. The query carries the link's secrets — Kupferdraht logs the
path only and strips the URL from transport errors. Open: whether
another operator's link can be joined at all (`open.md` §74).

## Step 14a discovery: `/one2one-conversations` is GET-only on deployed wire-server

Confirmed live on **2026-05-21**.

The v14 swagger lists two routes for 1:1 conversations:

- `POST /v14/one2one-conversations` with body
  `NewOne2OneConv { name?, qualified_users, team?, users? }` —
  documented as "Create a 1:1 conversation".
- `GET /v14/one2one-conversations/{usr_domain}/{usr}` returning
  `MlsOne2OneConversation { conversation, public_keys }` —
  documented as "Get an MLS 1:1 conversation".

**Deployed wire-server serves only the GET route.** The POST route
returns `404 Not Found` at the nginx layer (HTML body, not Wire's
JSON error shape), meaning the path never reaches the Wire app
server. The 1:1 endpoint is server-side idempotent: the first GET
against a fresh `(self, peer)` pair registers the conversation,
every subsequent GET returns the existing one. Same call shape
for "create" and "fetch".

Kupferdraht models this as `Client::create_dm(peer) -> Arc<Room>`
calling `HttpClient::fetch_one_to_one_conversation(domain, uuid)`
internally. The Kalium reference (`ConversationApiV7`) drives the
same single GET, confirming the deployed behaviour.

The response wraps the conversation in
`{ conversation: OwnConversation, public_keys: MlsKeysByPurpose }`;
Kupferdraht discards the `public_keys` echo (already fetched at login
from `/v14/mls/public-keys`) and surfaces the flat
`OwnConversation` to the caller.

If Wire ever deploys the POST route (e.g. to support naming a 1:1
at creation time via the `name` field), Kupferdraht would expose it as
a sibling helper rather than replacing the GET-based default.

---

## Step 68 discovery: an MLS 1:1 stays hidden from `list-ids` until established

Observed live on **2026-08-12** against the own instance
(schwarz-dev-wire), root-caused during Step 68 (observations §66).

The one2one GET only **reserves** the conversation: the server
assigns the deterministic conversation id and the MLS `group_id`
and returns them (`epoch: 0`), but the conversation is **not**
included in `POST /v14/conversations/list-ids` responses. It
becomes visible there only after a member uploads the establishing
commit via `POST /v14/mls/commit-bundles` (claim the peer's key
packages, Add-commit) — the same call that gives the server its
first `GroupInfo`. Until then, other clients render the 1:1 as the
`mls-missing-group-info` failure case.

Consequence for the client contract: fetching the 1:1 and creating
only the local group dead-ends — the room never appears in any
subsequent metadata sync. `Client::create_dm` therefore establishes
the group when the fetched `epoch` is `0` (claim + Add-commit +
bundle upload, discarding the local group if any leg fails) and
joins via external commit when `epoch > 0` (another device or the
peer established first).

---

## Step 86 discovery: the self conversation is listed at epoch 0 but has no `GroupInfo` until a member establishes it

Observed live on **2026-09-24** against the own instance
(schwarz-dev-wire), test user 3, fresh device (observations §82).

The user's MLS self conversation (`type: 1`, one member) appears in
`POST /v14/conversations/list-ids` + `list` from the first sync on:
`epoch: 0`, a `group_id`, no name. It is **not** hidden while
unestablished (the 1:1 reservation above is). But
`GET …/groupinfo` answers `404 mls-missing-group-info` — the same
"never MLS-established" failure the 2026-08-24 `rooms-join-all`
re-probe logged for exactly this room (observations §70) — so an
external commit (`Client::join_room`'s default path) cannot enter
it.

Consequence for the client contract: at `epoch == 0` the first
device must **establish** the group (create the local group under
the listed `group_id`, self-claim the user's other devices' key
packages, Add-commit — or a self-update commit when alone — and
upload the bundle; the Step-31 `create_room` commit reused).
`POST /v14/mls/commit-bundles` accepts it and the conversation
advances to `epoch: 1`; from then on every other device of the user
joins by external commit like any other room. `Client::join_room`
branches on `RoomType::SelfRoom && epoch == 0`, `join_all_rooms`
inherits the branch, `Client::self_room` is the lookup.

---

## Step 73 deviation: the version-less legacy `/await` socket

ADR-0008 pins Kupferdraht to v14 routes exclusively — with this one
recorded exception (ADR-0019). While wire-server ships
`consumableNotifications: false` hard-coded (chart ≥ 5.30, PR #5116),
gundeck delivers every push through cannon's legacy WebSocket at
`wss://<nginz-ssl-base>/await` — a deliberately **version-less**
route (the versioned `/v{N}/await` variants 404 at nginx; verified
2026-04, observations §15). Live-verified 2026-08-18 (observations
§68):

- `?client=<device_id>` scopes the queue; auth rides an
  `Authorization: Bearer` header — Kupferdraht never puts the token in
  the URL.
- Each frame (binary, UTF-8 JSON) is one `/v14/notifications` item:
  `{"id", "payload": [event], "transient"}`; `id` lives in the REST
  cursor space, so socket delivery and REST backfill share one
  cursor.
- `"transient": true` marks live-only events (`conversation.typing`)
  that never appear in the REST queue.
- The socket is passive: no acks, no sync markers, no redelivery.
  Kupferdraht therefore uses it only as a doorbell (Step 97b, ADR-0030):
  connect first, then a REST `since=` drain; every durable frame
  triggers another drain, and only the drain advances the cursor —
  a drain before the connect would miss events stored before the
  socket registered, and a frame moving the cursor would make a
  missed push permanent.

## Cheat sheet: what changed between v5 and v14

A tight version of the "what broke" catalogue useful when
re-validating HTTP calls during the v14 upgrade:

| Concern | v5 | v14 |
|---|---|---|
| Event stream | `/await` + `/notifications`, no ack | `/v14/events` + `/v14/notifications`, per-event ack required |
| Consumable-notifications capability | not advertised | `ClientCapability.consumable-notifications` value exists |
| Access token client-binding | `client_id` query param optional | Effectively required for `/mls/commit-bundles` and `/mls/messages` |
| WebSocket auth | Bearer header accepted on some paths | **Only `?access_token=…` query param** reliably works |
| Most HTTP shapes (login, /clients, /mls/*, list-users, list-conversations) | — | **Unchanged** from v5 |
| Default ciphersuite in new conversations | Same (server-decided, currently 2) | Same |
| `POST /conversations/list` path | `/v5/conversations/list` | `/v14/conversations/list` (path convention is consistent) |

Where "Unchanged" is claimed, cross-check against the Swagger
before shipping — small additive fields (new optional
parameters, new enum values) regularly appear between minor
versions without breaking older clients.
