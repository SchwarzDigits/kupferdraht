# kupferdraht-core — Library API Reference

This document is the canonical reference for consuming `kupferdraht-core`
as a Rust library. The bindings layer (Kotlin / Swift / WASM via
UniFFI) re-exposes the same surface; bindings-specific notes live in
`docs/bindings.md`.

The public API is **shaped after matrix-rust-sdk** (ADR-0009). Every
type and method here has either a direct matrix-rust-sdk equivalent
or a deliberate, documented Wire-specific extension. The full mapping
lives in [`docs/api-mapping-matrix-rust-sdk.md`](api-mapping-matrix-rust-sdk.md);
add an entry there whenever you add a public symbol.

**Freshness rule.** Per `CLAUDE.md` §README and Documentation, this
file is updated in the same commit as any change to the public Rust
API. An out-of-date `library.md` is a review defect, not a follow-up
task.

---

## Module layout

Feature-based (matrix-rust-sdk style), not Kalium-style lifecycle-scope
nesting (ADR-0009). The real layout — see also `docs/ARCHITECTURE.md`
§3 for the responsibility-oriented view:

```
kupferdraht-core/src/
├── lib.rs                  — crate root: module declarations + the public
│                              pub-use re-exports
├── error.rs                 — KupferdrahtError
├── time.rs                  — now_ms() wall-clock shim (native SystemTime /
│                              wasm js_sys::Date), pub(crate)
├── client/                  — Client, ClientBuilder, ServerConfig, Account,
│   │                          PendingClient; decomposed (Step 29) across
│   │                          builder.rs, account.rs, login_pipeline.rs,
│   │                          resume.rs, session.rs, sync.rs, rooms.rs,
│   │                          membership.rs, timeline_read.rs, profile.rs,
│   │                          typing.rs, mentions.rs, message_lookup.rs,
│   │                          server_configs.rs, device.rs, http_calls.rs,
│   │                          pending*.rs
│   └── mod.rs                — the Client struct + cross-module accessors
├── rooms/                   — Room, RoomType, RoomMember, RoomAccess, JoinLink
├── timeline/                 — Timeline + one child module per interaction
│   │                          (Step 30 split): text, reaction, confirmation,
│   │                          delete, knock, location, typing, asset_send,
│   │                          asset_receive, lookup, ephemeral (Step 89:
│   │                          send + expiry sweep + timers)
│   └── mod.rs                — Timeline, MessageHandle, the pub-use surface
├── sync/                     — EventHandler, SyncSettings, SyncSummary
│                                (SyncService is pub(crate) since Step 14b)
│   └── dispatch/               — one inbound-event handler per interaction
│                                (Step 32 split): text, reaction, confirmation,
│                                delete, edit, knock, location, asset, config,
│                                ephemeral (Step 89)
├── encryption/                — pub(crate): MlsClient (openmls wrapper),
│                                KupferdrahtProvider (openmls storage provider,
│                                one transaction per MLS operation),
│                                asset AES-256-CBC crypto, Proteus prekeys
├── identity/                  — UserId — the one typed identifier (§Identifiers)
├── manager/                   — Kupferdraht (device-level, multi-account manager),
│                                KupferdrahtBuilder, PendingAccount, manager-level
│                                PendingLogin
├── keystore/                  — SecureKeyStore trait + KeyringStore /
│                                FileKeyStore / InMemoryKeyStore /
│                                IndexedDbKeyStore (wasm32-only)
├── storage/                   — Store + GlobalStore traits, every record
│   │                            type (records.rs), db_key.rs, home.rs
│   │                            (KupferdrahtHome on-disk layout)
│   ├── sqlite/                  — SqliteStore (the store on every target),
│   │                            one CRUD submodule per table
│   ├── migrations/              — refinery *.sql migrations (V1 → V27 today)
│   └── browser.rs               — wasm32-only: the instance lease, the home
│                                id, one sqlite-remote-vfs per database
│                                (Step 111, ADR-0035)
├── wire/                       — pub(crate): Wire HTTP API, verbatim Wire
│   │                            field names, transport only
│   ├── dto.rs, notification_dto.rs, proto.rs, version.rs
│   └── http/                    — HttpClient split by resource: auth,
│                                clients, conversations, messages, mls,
│                                groups, key_packages, assets, users, sso, …
├── ffi/                        — UniFFI wrappers over Client/Room/Timeline/…
│                                — native/mobile only (cfg'd out on wasm32)
└── wasm/                       — hand-written wasm-bindgen binding layer,
                                parallel to ffi/ — wasm32 only (Steps 44–59)
```

The public re-exports from `lib.rs` are the supported API surface —
nine `pub use` statements re-exporting roughly 30 symbols:

- `client::{Account, Client, ClientBuilder, FileSharingPolicy, JoinSummary, PendingClient, ReceiptMode, ResyncSummary, ServerConfig}`
- `error::KupferdrahtError`
- `identity::UserId`
- `manager::{PendingAccount, PendingLogin, Kupferdraht, KupferdrahtBuilder}`
- `rooms::{JoinLink, Room, RoomAccess, RoomMember, RoomType}`
- `storage::{AccountEntry, GlobalStore, StorageProvider}`
- `sync::{EventHandler, Message, MessageBody, SyncSettings, SyncSummary}`
- `time::now_ms`
- `timeline::{AssetSummary, MessageHandle, Subscription, Timeline}`

A second tier is `pub` inside `storage::` without a crate-root
re-export — reachable as `kupferdraht::storage::X`, not `kupferdraht::X` —
because the CLI and integration tests persist / restore through them
explicitly: `Store`, `SqliteStore`, `SessionRecord`,
`DeviceRecord`, `BackendMlsKey`, `SelfUserRecord`, `ServerConfigRecord`,
`SyncStateRecord`, `MentionRecord`, `TimelineEventRecord`,
`AttachmentRecord`, `RoomRecord`, `RoomMemberRecord`,
`FeatureConfigEntry`, `UserPropertyEntry`, `ReactionRecord`,
`ReadReceiptRecord`, `DeliveryReceiptRecord`, `MaybeSendSync`; the
standalone openers `open_default_store` / `open_default_global_store`;
and the `db_key` submodule (`DbKey`, `generate_db_key`,
`KEY_LEN` / `SALT_LEN` / `MATERIAL_LEN`).

Everything else — the Wire HTTP transport (`wire::HttpClient`, the DTO
structs) and the openmls glue (`encryption::MlsClient`, the `MLS_*`
constants) — lives in `pub(crate) mod wire` and `pub(crate) mod
encryption`, reachable only from inside `kupferdraht-core`. Integration
tests and `kupferdraht-cli` drive these through the curated facade above;
`ffi/` (native/mobile) and `wasm/` (browser) are the two binding
layers over the same facade — see `docs/bindings.md`.

---

## Identifiers

Identifiers on the public API are plain `&str` (borrow) / `String`
(owned) — with **one** typed exception: [`UserId`](#userid), the
qualified user id returned by `Client::user_id()`. Room id, device id,
and message id stay plain strings. Ids are compared verbatim — no case
folding, no zero padding (Wire emits lowercase UUIDs; a sender's user id
is matched exactly against the stored one, Step 95a/95b).

This is a narrower outcome than two earlier proposals. `CHANGELOG.md`
§43 (Step 15) proposed `kupferdraht::ids::{UserId, RoomId, DeviceId,
MessageId, QualifiedUserId}` as soft typed wrappers (newtype around
`String`, no parse-time format validation) across every identifier;
`CHANGELOG.md` §49 rejected that plan as a half-measure (type-tagging
without validation delivers only half of what typed-ID systems usually
bundle) and shipped a narrower `&str`-ergonomics pass instead (the
retained, superseded plan doc is `docs/plan/archive/15-typed-identifiers.md`).
Step 19 (`docs/plan/archive/19-user-id-type.md`) later shipped the
single typed exception — `identity::UserId` — scoped to user ids only,
closing the bare/qualified column seam and the nullable-`self_user.domain`
fragility that had made "is this me?" comparisons and `resume` error-prone.
See `docs/ARCHITECTURE.md` §8 for the architecture-level summary.

| Identifier   | Format                          | Notes |
|--------------|---------------------------------|-------|
| User id      | `<uuid>@<domain>` (typed: [`UserId`](#userid)) | Qualified form. UUID is RFC 4122; domain is the deployment's domain (e.g. `example.com`). `UserId::new` / `UserId::parse` guarantee the domain is always present — there is no bare/domain-less `UserId`. |
| Room id      | `<uuid>@<domain>` (plain `&str`/`String`) | Same shape as user id but distinct namespace (Wire conversations); not wrapped in a typed newtype. |
| Device id    | hex string (Wire-defined), plain `&str`/`String` | Wire's `client_id`. Not a UUID — currently a 16-char hex blob. |
| Message id   | UUID (Wire convention), plain `&str`/`String` | `GenericMessage.message_id`. No schema constraint server-side. |

**Sender of an inbound message (Step 95a, ADR-0027).** Every
`sender_user_id` / `sender_device_id` Kupferdraht hands out for an inbound
MLS message — `Message`, `TimelineItem`, `AssetSummary`, a task's
`creator_user_id`, the case gate — is the identity of the MLS leaf
that signed the message, parsed from its Wire `BasicCredential`
(`{uuid}:{client_id}@{domain}`). The Wire event envelope
(`qualified_from` / `from_client`) is not trusted for it: a backend
that lies there is logged (`warn!`) and ignored. Two edges: a signer
with an e2ei X.509 credential is still attributed by the envelope
until X.509 identity parsing lands (such senders are not
authenticated), and a message whose Basic credential is not in Wire's
shape is dropped. What this authenticates is *which device leaf* sent
a message — not *who owns* that leaf: under BasicCredentials the
backend distributes the keys, so an injected device would carry
whatever identity the backend gave it.

Mixing room/device/message identifiers of different kinds is a
runtime error at the HTTP boundary, not a compile-time error (only
user ids are type-tagged). Pass-through is `&str` for read-only
inspection and `String` for ownership transfer; the convention is
consistent across `Client`, `Room`, `Timeline`, and `Account`.

---

## UserId

The one typed identifier on the public API (`identity::UserId`,
re-exported at the crate root). Everything else — room id, device id,
message id — stays a plain `&str` / `String` (see [Identifiers](#identifiers)
above).

```rust
pub struct UserId(/* private */);

impl UserId {
    pub fn new(localpart: &str, domain: &str) -> Self;
    pub fn parse(s: &str) -> Result<Self, KupferdrahtError>;
    pub fn localpart(&self) -> &str;
    pub fn domain(&self) -> &str;
    pub fn as_str(&self) -> &str;
}
// impl fmt::Display — writes the canonical `<localpart>@<domain>` string.
```

`UserId` is a validated string newtype (the matrix-rust-sdk
`OwnedUserId` equivalent) carrying the canonical Wire user id
`<localpart>@<domain>`. The shared invariant — exactly one `@`,
non-empty localpart, non-empty domain — is upheld by both
constructors, so every `UserId` value carries a domain; there is no
`Option<domain>` and no bare/qualified seam to reconcile.
**Path safety is `parse`'s stricter, own layer** (Step 80): `parse`
additionally rejects path metacharacters, while `new` stays
infallible for pre-separated parts — ids that become path
components are independently refused at the materialization sites
(`open_default_store` / `open_mls_provider`), so an unchecked `new`
can never shape `base/accounts/<id>`.

| Method | Returns | Notes |
|---|---|---|
| `UserId::new(localpart, domain)` | `Self` | Infallible — build from already-separated parts the caller has checked. The caller guarantees non-empty, `@`-free parts; the precondition is `debug_assert!`-checked, not runtime-validated. Use `from_parts` for parts from runtime data. |
| `UserId::from_parts(localpart, domain)` | `Result<Self, KupferdrahtError>` | Step 95c — the checked form of `new` for parts that come from runtime data (Wire's `QualifiedId { id, domain }` in an event, a mention, a response). `KupferdrahtError::InvalidInput` when a part is empty or contains `@`. |
| `UserId::parse(s)` | `Result<Self, KupferdrahtError>` | Parse a flat `<localpart>@<domain>` string (a DB column value, a CLI argument). `KupferdrahtError::InvalidInput` unless `s` is exactly one `@` with a non-empty localpart and a non-empty domain, **both free of path metacharacters** (`/`, `\`, exact `.`/`..`, control characters, overlength — Step 80: the id is joined into the on-disk account layout, and the backend supplying it is not trusted with path structure). |
| `UserId::localpart()` | `&str` | The part before `@` — the Wire user UUID today (not type-guaranteed to be a UUID). |
| `UserId::domain()` | `&str` | The part after `@` — the Wire backend domain. Always present. |
| `UserId::as_str()` | `&str` | The canonical qualified string `<localpart>@<domain>`. |

`UserId` also implements `Clone`, `PartialEq`, `Eq`, `Hash`, `Debug`,
and `Display` (writes the wrapped string verbatim), so it can be used
directly as a `HashMap` key or in equality comparisons — the property
that made "is this me?" a plain `==` once every owner-identity column
(`session.user_id`, `self_user.id`, `devices.user_id`) was unified
under this type (Step 19).

The broader `kupferdraht::ids::*` typed-identifiers plan (`RoomId`,
`DeviceId`, `MessageId`, `QualifiedUserId`) covering every identifier
was rejected — see [Identifiers](#identifiers) above and `CHANGELOG.md`
§49. `UserId` is the one typed exception, not the first of a family.

---

## ClientBuilder

Construct a `Client` by supplying a backend address and credentials.

```rust
use kupferdraht::{ClientBuilder, ServerConfig};

let server = ServerConfig {
    api_base_url: "https://nginz-https.example.com".into(),
    websocket_url: "wss://nginz-ssl.example.com".into(),
    title: "Example backend".into(),
};

let client = ClientBuilder::new()
    .server(server)
    .login(
        "alice@example.com".into(),   // Wire account email
        "supersecret".into(),         // password
        None,                         // Option<String> 2FA code
    )
    .await?;
```

### `ClientBuilder::new() -> ClientBuilder`

Construct an empty builder. Calling `login` without first calling
`server` returns `KupferdrahtError::InvalidInput`.

### `ClientBuilder::server(self, ServerConfig) -> ClientBuilder`

Attach the backend addressing tuple. See `ServerConfig` below.

### `ClientBuilder::base_dir(self, PathBuf) -> ClientBuilder`

The persistence root. On `login` / `resume` the SDK opens (and creates
as needed) the per-account `base_dir/accounts/<id>/kupferdraht.db` (via the
default `StorageProvider`) and `mls.db` under the same directory. The
concrete store does its own encryption — it fetches its key from the
keystore and opens **ready**; there is no separate unlock step.
Resolving `base_dir` also reads `base_dir/home.id` — the home id
(Step 93, ADR-0025), created once under the home lock on the first open
— which every keystore id of the home and its login cookie label carry;
see [Keystore](#keystore).

Without `base_dir` (and no `storage_provider`), persistence is
in-memory only — fine for tests and one-shot CLI invocations; the
openmls `KeyPackage` private halves are lost on `Client` drop.

Reach the SDK-opened store afterwards via `Client::store() ->
Option<Arc<dyn Store>>` for direct reads the high-level API doesn't
wrap. `kupferdraht::storage::open_default_store(base_dir, keystore,
account_id)` opens the same per-account default store standalone.

### `ClientBuilder::storage_provider(self, Arc<dyn StorageProvider>) -> ClientBuilder`

Override the app-data + registry backend with a consumer
implementation (e.g. a server-side backend for cloud bots). When unset,
the SDK uses the default `SQLite` provider rooted at `base_dir`. The
provider opens **ready** stores (`open_global` + `open_store`); the
concrete store owns its own encryption (the trait makes no key
assumption). `base_dir` is still required for `mls.db`, which is not yet
pluggable.

**Atomic event writes (Step 105).** An event whose timeline row has a
dependent row is written through one `Store` call:
`insert_timeline_event_with_mentions` (text), `…_with_attachment`
(asset), `…_with_task` (task + fold row) and
`apply_message_edit_with_mentions` (edit). Each returns whether the row
was fresh / the edit applied; an existing row is reported, not an error,
and nothing is written then. Their default bodies write in sequence and
are **not** atomic — they keep custom stores compiling; a custom store
that can should override them with one transaction, because an inbound
notification is replayed after a failure (at-least-once, ADR-0032) and a
half-written event would be skipped as a duplicate.

A store can also group a drained page (Step 109, ADR-0037):
`begin_page`, `savepoint(index)`, `rollback_to_savepoint(index)`,
`commit_page` and the synchronous `rollback_page`. Their defaults group
nothing — every write commits on its own, which stays correct. A store
that overrides them must override all five, make its writes join the
open page, and make writes from outside the drain wait until the page
ends; `SqliteStore` does, and on remote storage that turns three commits
per notification into three per page.

The `Kupferdraht` manager (`Kupferdraht::builder().storage_provider(..)`) is the
device-level entry point for the multi-account / custom-backend path.

The CLI uses `base_dir = $KUPFERDRAHT_HOME`.

### `ClientBuilder::permit_http(self) -> ClientBuilder`

Opt in to plain-HTTP base URLs. Default is strict:
`ClientBuilder::login` rejects any `ServerConfig.api_base_url` whose
scheme is not `https://` with `KupferdrahtError::InvalidInput`. Only the
`httpmock`-backed integration tests call `permit_http` — production
callers (including `kupferdraht-cli`) must not.

### `ClientBuilder::account(self, account_id: impl Into<String>) -> ClientBuilder`

Select which account `ClientBuilder::resume` (see
[Login returns a ready Client](#login-returns-a-ready-client) below)
reconstructs. `resume` requires this to be set and returns
`KupferdrahtError::InvalidInput` otherwise; registry-based account
selection (picking from every account known to the device) lives on
`Kupferdraht::resume`, not here — `ClientBuilder::resume` is the
registry-less, single-account path (e.g. the CLI, which already knows
which account it wants).

### `ClientBuilder::device_label(self, label: impl Into<String>) -> ClientBuilder`

Human-readable device name shown in Wire's device list (Step 41),
e.g. the OS device name. When unset, a platform-derived default is
used. Threaded into `POST /v14/clients` during `register_device`
(phase 2 of login). The `Kupferdraht` manager exposes the same knob as the
`device_label: Option<String>` parameter on `Kupferdraht::login` /
`Kupferdraht::finalize_sso` — set for a manager-driven login, `None` falls
back to the same platform default.

### `ClientBuilder::login(self, email, password, second_factor_code) -> Arc<PendingLogin>`

End-to-end login, split into **two phases** so a consumer can recover
from Wire's per-user device cap without re-authenticating (see
`PendingLogin` below). Each step is a method on the internal
`LoginPipeline`.

**Phase 1 — `login`** (authentication; returns `PendingLogin`):

1. `GET /api-version` — version negotiation against `CLIENT_SUPPORTED = [14]`.
2. `POST /v{N}/login?persist=true` — credentials + persistent cookie
   label (Wire requires a non-empty `label`; see `observations.md` §16).
   Pick up the `zuid` refresh cookie from `Set-Cookie` (reqwest's
   cookie store does not reliably forward Wire's `Path=/access`-scoped
   cookie; see `observations.md` §18).
3. `GET /v{N}/self` — fetch profile, including the federated
   `qualified_id = {id, domain}` pair.

**Phase 2 — `PendingLogin::register_device`** (device + MLS bring-up;
returns `Arc<Client>`):

4. `POST /v{N}/clients` — register a fresh device with a Proteus
   prekey bundle (Wire schema requirement; ADR-0001). The per-user cap
   surfaces here as `KupferdrahtError::TooManyDevices`.
5. `POST /v{N}/access?client_id=<id>` — rebind the access token so
   nginx injects the `Z-Client` claim on every subsequent
   client-scoped call.
6. **MLS identity bring-up.** Create `MlsClient` (signature keypair
   + `BasicCredential` with Wire identity), `PUT /v{N}/clients/{id}`
   with `mls_public_keys` (required **before** the upload; see
   `observations.md` §19), generate `INITIAL_KEY_PACKAGE_COUNT = 100`
   TLS-serialized `KeyPackage`s, upload them via
   `POST /v{N}/mls/key-packages/self/{id}`, then `GET /v{N}/mls/public-keys`
   to capture Wire's backend external-sender removal key
   (ADR-0004).

Authentication errors (wrong password, missing/invalid 2nd-factor code)
surface from phase 1; device-registration / MLS errors surface from
phase 2. On success, `register_device` returns an `Arc<Client>` whose
`access_token()` returns the **rebound** token, not the initial
`/login` response token, and whose `mls_signature_public_key()` /
`backend_removal_key_ecdsa_p256()` expose the MLS artefacts generated in
phase 6.

**In-session token refresh.** Wire access tokens expire after
~15 min. The returned `Client` refreshes itself automatically: any
authenticated call that receives `401` triggers one cookie-auth
`POST /v{N}/access` rebind and replays the request with the rotated
bearer, transparently to the caller. The retry lives in the HTTP send
funnel, so every authenticated method inherits it without per-call
wrapping; N concurrent `401`s collapse to a single rebind. The rotated
`zuid` and the new access token are re-persisted to the session store
(when one is attached), so a later `resume` uses the current cookie.
`access_token()` therefore returns the *current* token, which changes
over the `Client`'s lifetime. The distinct `KupferdrahtError::Unauthorized`
variant surfaces only when a `401` cannot be recovered — during the
pre-login phase, or after a refreshed token is itself rejected.
**Exception:** streaming asset upload/download bypass the funnel
(non-cloneable body / custom streaming response), so a `401` mid-asset
is *not* auto-retried — it surfaces and the next ordinary call
refreshes.

Errors map to typed `KupferdrahtError` variants — see `error.rs`.

> **Language bindings (Kotlin / Swift / Android).** The FFI surface is a
> separate wrapper layer (`kupferdraht-core/src/ffi/`); foreign consumers reach
> the SDK through it (`Kupferdraht`, `Client`, `parse_user_id`, …) rather than
> the native types above. See `docs/bindings.md`.

### SSO login (Step 17β): `initiate_sso` / `initiate_sso_by_email` / `finalize_sso`

Interactive, SP-driven SSO. `initiate_sso(code, success_redirect,
error_redirect) -> String` validates the SSO code against the backend
(`InvalidInput` when unknown) and returns the interactive initiate URL to
open in a browser; `initiate_sso_by_email(email, success_redirect,
error_redirect) -> String` resolves the backend's default SSO code via
`GET /sso/settings` first (`InvalidInput` when the backend has none).
After the IdP redirect, `finalize_sso(sso_cookie) -> Arc<PendingLogin>`
mints an access token from the captured `zuid` cookie — no password —
and returns the same phase-1 `PendingLogin` handle as password login;
`register_device` (phase 2, incl. device-limit recovery) is unchanged.
`finalize_sso` surfaces `SessionExpired` when the cookie is already
revoked or rotated. All three require `ClientBuilder::server`.
matrix-rust-sdk analog: `Client::login_sso` (redirect-closure model —
see the mapping doc; Wire's SP-driven flow splits it into
initiate/finalize around the `zuid` cookie). Manager-level twins exist
on `Kupferdraht` (`initiate_sso` / `initiate_sso_by_email` / `finalize_sso`
with the manager's server wiring); the **manager trio** is FFI-mirrored
(`ffi::Kupferdraht`) and bound on wasm (`initiateSso` / `initiateSsoByEmail`
/ `finalizeSso`) — the builder trio reaches foreign code only through
the manager, since there is no FFI `ClientBuilder`.

### `ClientBuilder::register(self, name, email, password) -> Arc<PendingClient>`

Step 16c. POSTs `/register` (unversioned) against the configured
Wire backend and returns an `Arc<PendingClient>` carrying the new
account's `user_id` (from `qualified_id.id`), the supplied email,
and a `Zeroizing<String>`-wrapped password for the post-activate
login.

The `PendingClient` is **in-memory only** — a process crash between
`register` and `activate` requires re-calling `register` (Wire then
returns `409 key-exists`).

Driver for `kupferdraht-cli register`. Library consumers (Natrium) who
keep the `PendingClient` alive across the email-delivery window
should prefer this entry point to `ClientBuilder::activate` since it
avoids re-supplying the password.

### `ClientBuilder::activate(self, email, code, password) -> Arc<Client>`

Step 16d. Process-symmetric counterpart to
[`PendingClient::activate`](#pendingclient) for callers that lost the
in-memory `PendingClient` (typically: a fresh `kupferdraht-cli activate`
process). POSTs `/activate` and then drives the same login pipeline
`ClientBuilder::login` runs, returning a ready `Arc<Client>` — just
like `login` and `resume`.

---

## PendingClient

Step 16c. Intermediate handle returned by `ClientBuilder::register`.
Carries the new account's identity (`user_id`, `email`) plus the
in-memory password needed to complete the post-activate login.

### `PendingClient::user_id(&self) -> &str`

The new account's Wire UUID (from `/register`'s `qualified_id.id`).

### `PendingClient::email(&self) -> &str`

The email the caller passed at `register` time, echoed verbatim.

### `PendingClient::activate(self: Arc<Self>, code: &str) -> Arc<Client>`

Redeem the email-delivered code and complete the loop. POSTs
`/activate` and then runs the full login pipeline, returning a
fully-usable `Arc<Client>`. On error the `Arc<PendingClient>` remains
valid for retry via an additional `Arc`-clone.

### `PendingClient::resend_activation_email(&self) -> ()`

POST `/activate/send` to ask Wire to re-deliver the activation
email. Useful when the original message expired or never arrived.

---

## Kupferdraht (device manager)

Device-level, multi-account entry point. Sits above per-account
`Client`s: owns the device-wide `GlobalStore` registry (openable at
app start without unlocking any account), lists accounts for a
picker, and builds the per-account `Client` on login or resume —
registering the account as it goes.

### `Kupferdraht::open(base_dir: PathBuf, keystore: Arc<dyn SecureKeyStore>) -> Result<Kupferdraht, KupferdrahtError>`

Open the device manager rooted at `base_dir`, backed by the shipped
default `SQLite` `StorageProvider` (encrypted `global.db` registry
plus per-account stores). Shorthand for
`Kupferdraht::builder().base_dir(base_dir).keystore(keystore).build()`. For
a custom backend (e.g. a cloud-bot server), use `Kupferdraht::builder()`
(see [`KupferdrahtBuilder`](#kupferdrahtbuilder) below) directly. Errors with
`KupferdrahtError::Storage` if the registry cannot be opened.

### `Kupferdraht::permit_http(self) -> Self`

Permit plain-HTTP server URLs for every account this manager builds
(local dev / tests); production deployments use HTTPS only. Mirrors
`ClientBuilder::permit_http`, threaded through every internal
`ClientBuilder` the manager constructs.

### `Kupferdraht::accounts(&self) -> Result<Vec<AccountEntry>, KupferdrahtError>`

The device account registry — listable without unlocking any
account. Backs an account-picker UI. `KupferdrahtError::Storage` on a
registry read failure.

### `Kupferdraht::active_account(&self) -> Result<Option<String>, KupferdrahtError>`

The account id last marked active, if any — the account a
`register_device` / `PendingAccount::activate` / `Kupferdraht::resume`
call most recently wrote as active in the registry. `None` before any
account has been logged in / resumed on this device.
`KupferdrahtError::Storage` on a registry read failure.

### `Kupferdraht::home_id(&self) -> &str`

The id of this home (Step 93, ADR-0025): 32 lowercase hex characters
persisted in `base_dir/home.id`, created exactly once — under the
home's process lock — on the first open of a fresh home and read on
every later open. An identifier, not a secret: every keystore id of
the home (`kupferdraht.<home-id>.global-db`, `….account-<id>`,
`….account-<id>-mls`) and the login cookie label derive from it, so
two homes sharing one keystore never share an entry. Never edit or
delete the file — a regenerated id orphans every key of the home; a
malformed file is a recoverable `KupferdrahtError::Storage` at open and is
never rewritten.

### `Kupferdraht::cookie_label(&self) -> String`

The Wire login cookie label of this home, `kupferdraht-<home-id>` (Step 93,
replaces the Step-41 per-install label that lived in the keystore).
Wire keeps one refresh cookie per `(user, label)`; one label per home is
what keeps two homes of the same user — kupferdraht-cli and kupferdraht-tui on
one keychain, say — from evicting each other's `zuid` (observations
§83). Sent as `label` on `POST /login` and as `cookie` on
`POST /clients`; recorded in the session row (`SessionRecord.cookie_label`)
at login, so a session still holding a pre-Step-93 label is diagnosable
(`kupferdraht-cli status`). A `ClientBuilder` without `base_dir` uses an
ephemeral label per build.

### `Kupferdraht::login(&self, server, email, password, second_factor_code, device_label) -> Result<Arc<PendingLogin>, KupferdrahtError>`

Phase 1 of login: authenticate against `server` and return a
manager-aware `PendingLogin` (see below). Call
`PendingLogin::register_device` to register the device, write the
registry row (marking the account active), and obtain a ready,
resumable `Client`. `device_label` is the same optional
human-readable device name as `ClientBuilder::device_label` (Step
41) — `None` falls back to a platform-derived default. The
authenticate/register-device split lets a consumer recover from
Wire's per-user device cap (`TooManyDevices` from `register_device`)
without re-authenticating. Authentication failures
(`InvalidCredentials`, `SecondFactorRequired`, …) surface here;
device-cap / registration failures surface from `register_device`.

### `Kupferdraht::resume(&self, account_id: &str) -> Result<Arc<Client>, KupferdrahtError>`

Resume `account_id` from disk — no Wire login. The server config is
read from the per-account store; `account_id` is validated against
the device registry first (`InvalidInput` if unregistered) and then
against what the resumed session reports (`InvalidInput` on a
mismatch). **Step 39:** at most one live `Client` per account is
kept — a concurrent or repeated `resume` for an already-live account
returns the cached `Arc<Client>` instead of racing a second `zuid`
rebind against the first; a torn-down instance (every other `Arc` has
been dropped) is rebuilt. Marks the account active in the registry on
success. Otherwise surfaces whatever `ClientBuilder::resume` returns.

### KupferdrahtBuilder

```rust
let kupferdraht = Kupferdraht::builder()
    .base_dir(base_dir)
    .keystore(keystore)
    // .storage_provider(custom_provider)  // optional override
    // .permit_http()                       // local dev / tests only
    .build()?;
```

The way to inject a custom `StorageProvider` (e.g. a server-side
backend for cloud bots) into `Kupferdraht`; `Kupferdraht::open` is sugar over
the common case of this builder with the shipped default provider.

| Method | Returns | Notes |
|---|---|---|
| `KupferdrahtBuilder::base_dir(self, PathBuf)` | `Self` | **Required.** The persistence root — hosts `home.id` (the home id, Step 93), the default `global.db` and the per-account `accounts/<id>/` directories. `build` resolves the home first (creates the directory and, on a first open, the id under the home lock). |
| `KupferdrahtBuilder::keystore(self, Arc<dyn SecureKeyStore>)` | `Self` | **Required.** The platform keystore. The only place the manager hands out the keystore; the default provider self-fetches its keys from it. |
| `KupferdrahtBuilder::storage_provider(self, Arc<dyn StorageProvider>)` | `Self` | Override the storage backend (global + per-account stores). When unset, the shipped `SQLite` provider rooted at `base_dir` is used. |
| `KupferdrahtBuilder::permit_http(self)` | `Self` | Permit plain-HTTP server URLs (local dev / tests). |
| `KupferdrahtBuilder::build(self)` | `Result<Kupferdraht, KupferdrahtError>` | Build the `Kupferdraht` manager, opening the device-level registry via the (default or custom) provider. `KupferdrahtError::InvalidInput` if `base_dir` / `keystore` are missing; `KupferdrahtError::Storage` if the registry cannot be opened. |

### `Kupferdraht::register(&self, server, name, email, password) -> PendingAccount`

Manager-level registration. Reuses the manager's `base_dir` /
keystore / storage-provider wiring (unlike `ClientBuilder::register`,
which is the registry-less CLI path). `register` alone writes nothing
to the device registry.

### `PendingAccount`

Manager-aware counterpart to [`PendingClient`](#pendingclient),
returned by `Kupferdraht::register`. Same accessors (`user_id`, `email`,
`resend_activation_email`); cheap to clone.

`PendingAccount::activate(&self, code) -> Arc<Client>` redeems the
activation code and then performs the same writes `Kupferdraht::login`
does: the device-registry `AccountEntry` (marked active) and the
session rows — so the account is immediately listable via
`Kupferdraht::accounts` and resumable via `Kupferdraht::resume`. A failed
activation leaves the registry untouched and the handle valid for a
retry.

### `PendingLogin` (Step 24)

The phase-1 handle returned by `ClientBuilder::login` / `Kupferdraht::login`
(see [`login`](#clientbuilderloginself-email-password-second_factor_code---arcpendinglogin)).
It holds the authenticated session; `register_device` runs phase 2 and
yields a `Client`.

| Method | Returns | Notes |
|---|---|---|
| `PendingLogin::user_id()` / `email()`            | `&str` | Readable identity of the authenticated session. |
| `PendingLogin::register_device()`                | `Result<Arc<Client>, KupferdrahtError>` | Phase 2: register the device + MLS identity → ready `Client`. On `TooManyDevices` the handle stays valid for a retry; **single-use on success** (a second call → `InvalidInput`). |
| `PendingLogin::devices()`                         | `Result<Vec<DeviceInfo>, KupferdrahtError>` | List the registered devices (`GET /v{N}/clients` via the held login token). All `is_self == false` — no device is bound to this session yet. |
| `PendingLogin::delete_device(device_id)`          | `Result<(), KupferdrahtError>` | Remove a device to free a slot (`DELETE /v{N}/clients/{id}`). |

**Device-limit recovery.** Wire caps a user at 7 devices; the 8th
`register_device` surfaces the typed `KupferdrahtError::TooManyDevices`
(distinct from `BackendRejected`). Because the session was captured in
phase 1, recovery runs on the **same** handle — no second login:
`devices()` → `delete_device(id)` → `register_device()` again.

The manager-level `PendingLogin` additionally writes the device-registry
`AccountEntry` (marked active) and the session rows inside a successful
`register_device` — the same writes a full login makes — so the account
is immediately listable via `Kupferdraht::accounts` and resumable via
`Kupferdraht::resume`.

In-memory only (like `PendingAccount`); the held login token can expire on
a very long recovery (`Unauthorized`) — re-enter via a fresh `login`. The
recovery surface is auth-agnostic: SSO login (Step 17β, shipped) returns
the same handle from `finalize_sso` and reuses it unchanged. No
matrix-rust-sdk analog (Wire extension).

---

## ServerConfig

Wire-server addressing tuple.

```rust
pub struct ServerConfig {
    pub api_base_url: String,    // HTTPS, e.g. https://nginz-https.example.com
    pub websocket_url: String,   // WSS,   e.g. wss://nginz-ssl.example.com
    pub title: String,           // Human-readable, e.g. "Example backend"
}
```

This is **Kupferdraht-specific**. matrix-rust-sdk uses a single
`homeserver_url`. Wire pairs HTTP and WebSocket on different
hostnames, so a single URL is not enough. See the mapping doc for
the full justification.

---

## Client

Authenticated session. Returned wrapped in `Arc<Client>` so consumers
can share it across tokio tasks. `Client` is `Send + Sync`.

### Identity getters

| Method                         | Returns           | Notes |
|---|---|---|
| `user_id()`                    | `Option<&UserId>` | Wire user id (`<uuid>@<domain>`). Always `Some` post-login; `Option` mirrors matrix-rust-sdk. |
| `device_id()`                  | `Option<&str>`    | Hex device id from `POST /v14/clients`. Same `Option` contract. |
| `access_token()`               | `Option<String>`  | Owned. The **current** access token — the post-login rebind, and thereafter the most recent in-session refresh (auto-rotated on `401`; see *In-session token refresh* above). |
| `refresh_token()`              | `Option<String>`  | Owned. Wire's `zuid` refresh-cookie value, snapshotted the same way as `access_token()`. Wire-specific extension — matrix-rust-sdk has no equivalent; `Option` for symmetry with `access_token()`, always `Some` post-login. The CLI persists this into `SessionRecord.refresh_token` so a later `kupferdraht-cli sync` can drive `ClientBuilder::resume` without the password. |
| `account()`                    | `Arc<Account>`    | User profile snapshot. See `Account` below. |

### Backend getters

| Method                         | Returns      | Notes |
|---|---|---|
| `homeserver_url()`             | `&str`       | matrix-rust-sdk's `Client::homeserver` analog. |
| `websocket_url()`              | `&str`       | Wire-specific. Used by `SyncService` (Step 5+). |
| `server_title()`               | `&str`       | Wire-specific. UI affordance. |
| `api_version()`                | `u32`        | Wire-specific. Always `14` per ADR-0008. |

### Choosing the receive transport (Step 62)

`SyncSettings.transport` switches the loop variants (`sync`,
`sync_with_callback`, `sync_stream`, FFI `start_sync`) between
`SyncTransport::WebSocket` — live push, sub-second receive, the only
way transient `conversation.typing` events can arrive, and **the
default on native targets since Step 118 (ADR-0033)** — and
`SyncTransport::Polling` (drains `GET /v14/notifications` in a loop;
the explicit choice, and the default of the browser build, whose live
sync has its own socket). Kotlin and Swift name the transport anyway
(UniFFI records carry no enum defaults): pass `WebSocket`.
Since Step 73 (ADR-0019) the WebSocket variant **auto-negotiates the
concrete socket per connection**: servers with the
`consumableNotifications` feature enabled get `GET /v14/events`
(per-event acks, ADR-0017); everything else — every current
wire-server, which ships the feature hard-disabled — gets the
legacy cannon `/await` socket (Bearer-header auth, client keepalive
pings). That socket has no redelivery, so since Step 97b (ADR-0030)
it is only a doorbell: a REST `since=` drain runs after every
(re)connect and after every durable frame (frames during a running
drain coalesce), and only the drain dispatches and advances the
cursor; transient frames (typing) dispatch directly. The room list is
fetched once at start and then only when an event changed it or it is
older than 5 minutes (Step 118) — a reconnect alone fetches nothing.

Since Step 118 the push loop never depends on the socket: a connect
that fails drains anyway, the loop connects at most once per push
pace (`poll_interval`, 10 s when unset — the polling loop's pause) — a long-lived connection
reconnects at once, an unreachable or flapping one every 10 s with a
drain each time — and a legacy socket without a frame for a minute is
renewed (a ping only fails on a dead write; a socket whose frames a
proxy holds back answers it). Consumers simply take the default:

```rust
client.sync(SyncSettings::default()).await?; // push on native targets
```

`server_supports_push_events()` remains public — it reads the cached
`consumableNotifications` feature config (login fetch +
`feature-config.update` events, ADR-0012; an absent entry reports
`false`) and now also drives the per-connection negotiation; the CLI
prints which socket a `sync-stream` session negotiated. The
WebSocket transport is native-only; on wasm the loop variants return
an error until the browser transport lands.

### Reconnect contract (Step 79)

Written down because it was only ever implied. The loop variants
(`sync`, `sync_with_callback`, `sync_stream`, FFI `start_sync`)
**return on a hard error by design** — a dropped socket, a failed
drain, an expired session. A `429` (`RateLimited`) is not one: the
loops wait the backend's `Retry-After` and carry on (Step 103). There is
**no network observer in core** (OS network state is platform territory)
and **no background
scheduler** (the core has no lifecycle to hang one on):

- **Reconnect / backoff is the consumer's loop.** Restart the loop
  when it returns (kupferdraht-tui: 1 s → 60 s exponential backoff); the
  persisted cursor makes the restart lossless — the backend replays
  every unacknowledged notification first (`docs/backend-spec.md`
  §11.5). `SessionExpired` is the one terminal error: re-login instead
  of retrying.
- **A send that fails while the network is down stays a `Failed` row**
  (Step 25a) — nothing in core retries it. On recovery the consumer
  re-drives the rows explicitly: `Room::failed_sends()` lists them,
  `Timeline::resend_failed()` re-drives them oldest-first and stops at
  the first hard error so a stuck message is never overtaken (the
  matrix-rust-sdk strict-order rule), `Timeline::resend(message_id)`
  re-drives one. The hook is the first successful iteration after the
  restart (a `SyncListener` / `sync_with_callback` summary, or the
  stream yielding again):

  ```rust
  for room in client.rooms().await {
      if !room.failed_sends().await?.is_empty() {
          let summary = room.timeline()?.resend_failed().await?;
          tracing::info!(room = %room.id(), ?summary, "resent after reconnect");
      }
  }
  ```

- **The stale-epoch self-heal is the only retry inside core** — one
  `resync_room` + re-encrypt inside every send (`ARCHITECTURE.md`
  §5.6). It covers epoch races, not outages.

### Rooms

| Method                                                | Returns                               | Notes |
|---|---|---|
| `sync_once(SyncSettings)`                             | `Result<SyncSummary, KupferdrahtError>`    | **Step 14b.** One iteration of the matrix-rust-sdk-shaped sync family. Composes a conversations-metadata refresh + one `/v14/notifications` drain + cursor advance, dispatches every queued event through the registered `EventHandler`s. **Step 118:** the metadata refresh runs only when the room list is stale — on the first pass, after a drain that handed over an event Kupferdraht does not process itself or a Welcome (then in the same pass, after the drain), and when the queue was drained elsewhere or the backend lost the cursor — and, as a safety net, when the list is older than 5 minutes; an idle pass is the drain alone. Returns `SyncSummary { rooms_updated, events_dispatched }`. `poll_interval` is ignored — one-shot never sleeps. Concurrent invocations return `KupferdrahtError::SyncAlreadyRunning`. |
| `sync(SyncSettings)`                                  | `Result<(), KupferdrahtError>`             | **Step 14b.** Loops `sync_once` with `poll_interval` backoff between empty drains (10 s when unset — one value without a socket on every target since Step 118 — `Duration::ZERO` for none; a `429` is waited out). Returns only on hard error; the caller drives termination by dropping the future. Replaces the Step-5d `Client::sync().await?.run_polling(...)` pattern. **Step 62:** `SyncSettings.transport` selects `Polling` (default) or the live `/v14/events` WebSocket — see "Choosing the receive transport" below. |
| `sync_with_callback<F>(SyncSettings, F)`              | `Result<(), KupferdrahtError>`             | **Step 14b.** Same loop as `sync`, plus a per-iteration callback `F: Fn(SyncSummary) -> ControlFlow<()>`. UDL-friendly entry point — UniFFI exports `Fn` closures but not `impl Stream`. Returning `ControlFlow::Break(())` exits the loop. |
| `sync_stream(SyncSettings)`                           | `BoxStream<'static, Result<SyncSummary, KupferdrahtError>>` | **Step 14b.** Same loop surfaced as an async `Stream` of `SyncSummary`. Dropping the stream cancels the underlying polling task. Rust-side ergonomic; UDL surface deferred (callers from foreign code use `sync_with_callback`). |
| `add_event_handler(Arc<dyn EventHandler>)`            | `()`                                   | **Step 5c.** Register an `EventHandler` to receive per-event callbacks once a `sync*` iteration is running. Handlers added before `sync()` are honored on every future service run; handlers added after are picked up by the next frame the running service dispatches (the snapshot is reloaded per frame). matrix-rust-sdk analog: `Client::add_event_handler`. |
| `sync_metadata()`                                     | `Result<(), KupferdrahtError>`             | **Lightweight metadata-only sync** — hits `POST /v14/conversations/list-ids` + `POST /v14/conversations/list` + `POST /v14/list-users` and replaces the in-memory rooms snapshot, **without** draining `/v14/notifications`. Since Step 99 a conversation of a kind Kupferdraht does not know (a new `type` or `protocol`) or a user profile that does not decode is logged and skipped — it no longer fails the whole sync. Use over `sync_once` when battery / latency matters, or in background-startup paths where the per-event MLS-decrypt loop is too expensive — typically followed by a `sync_once` once foreground attention arrives. Originally Step 2's public sync surface; retained alongside the matrix-rust-sdk-shaped family as a deliberate Kupferdraht addition for three-tier composability (`load_rooms_from_store` / `sync_metadata` / `sync_once`). matrix-rust-sdk has no analog — Wire extension. CLI surface: `kupferdraht-cli sync-metadata`. |
| `rooms()`                                             | `Vec<Arc<Room>>`                      | In-memory snapshot. Empty before the first sync / hydrate. |
| `joined_rooms()`                                      | `Vec<Arc<Room>>`                      | **Step 14c.** Rooms the caller has joined. In Kupferdraht's current model every cached room is joined, so this is semantically equivalent to `rooms()`; the two methods are distinguished for forward-compatibility once invitation materialization lands. Sync (`std::sync::RwLock`). matrix-rust-sdk analog: `Client::joined_rooms`. |
| `invited_rooms()`                                     | `Vec<Arc<Room>>`                      | **Step 14c.** Rooms the caller has been invited to but has not yet joined. Returns an empty `Vec` today — Kupferdraht does not yet materialize pending invitations as `Room` objects (Kupferdraht-current limitation, tracked outside 14c). Sync. matrix-rust-sdk analog: `Client::invited_rooms`. |
| `rooms_filtered<F>(F)`                                | `Vec<Arc<Room>>`                      | **Step 14c.** Filter the cached rooms by a closure `F: Fn(&Room) -> bool`. Replaces ad-hoc iteration through `rooms()`. matrix-rust-sdk uses a typed `RoomListFilter`; Kupferdraht takes a closure to stay UDL-friendly and avoid filter-variant churn. |
| `self_room()`                                         | `Option<Arc<Room>>`                   | **Step 86.** Local-cache lookup of the caller's self conversation (`room_type == SelfRoom` — Wire's notes-to-self room, one MLS group of the user's own devices; the private cross-device channel later features ride: personal task list, `LastRead`, drafts). Sync, no network; `None` before the first sync / hydrate. The room may still be at epoch 0 (nobody established it) — `join_room` / `join_all_rooms` bring it online. No matrix-rust-sdk analog (Matrix has no self room). |
| `room(&str)`                                          | `Option<Arc<Room>>`                   | Single-room lookup by qualified id. **Step 15:** migrated from `room_id: String` to `&str` to drop the caller-side `.clone()` ceremony (closes `v1-api-ergonomics-audit.md §4.1`). |
| `room_or_load(&str)`                                  | `Option<Arc<Room>>`                   | **Step 14g.** Cache-first lookup with lazy store-hydrate fallback. Tries `room` first; on `None`, calls `load_rooms_from_store` and retries. **Step 15:** migrated from `String` to `&str`. |
| `unread_counts()`                                     | `Result<HashMap<String, u32>, KupferdrahtError>` | **Step 67.** Per-room unread counts in one grouped query — built for room lists (no n+1). Unread = inbound, not redacted, newer than the device-local read marker; **no marker → everything inbound counts**. Rooms without unread messages are absent (treat missing as 0). `Room::unread_count()` is the scoped sibling; `Timeline::mark_as_read` advances the marker (also when `ReceiptMode::Off` suppresses the outbound confirmation — the gate governs the wire signal, not the local read position). Cross-device read-position sync rides the self-conversation (later step). matrix-rust-sdk analog: `Room::unread_notification_counts` (Kupferdraht: single total, no mention split yet). |
| `create_room(name: &str)`                             | `Result<Arc<Room>, KupferdrahtError>`      | **Step 6a.** Create a new MLS conversation on Wire (`POST /v14/conversations` with `protocol="mls"`, no invitees) and the matching local MLS group. Builds the group at epoch 0, then immediately establishes it to epoch 1 with one commit (Wire rejects application messages at epoch 0). Refreshes the rooms cache via `sync_metadata` and returns the freshly-cached `Arc<Room>`. Zero-invitee path only — see `Client::invite_members` (below) to add members afterwards. `name` must be non-empty. |
| `find_message_room(&str)`                             | `Result<Vec<String>, KupferdrahtError>`    | Reverse lookup — given a `message_id` or edit-event id, return the rooms it lives in. Reads `timeline_events` only; returns empty when the message hasn't been synced locally. Powers the CLI's optional-`--id` sugar on `mark-read` / `react` / `delete-message` / `edit` / `reply`. Composite-PK schema permits multi-room matches but UUIDv4 makes that astronomically unlikely. |
| `find_message_by_id(message_id: &str)`                | `Result<Option<TimelineItem>, KupferdrahtError>` | **Step 34.** Resolve a single timeline item by id (stable `message_id` or edit-event id, same resolution as `find_message_room`) from the local store — no Wire call. Lightweight: the returned item's overlay collections (`reactions`, `read_by`, `delivered_to`) are empty; use `Room::messages` for a fully hydrated item. Primary use: resolving a reply's parent for a quote preview. `None` when the id isn't stored locally or no store is attached. |
| `load_rooms_from_store()`                             | `Result<(), KupferdrahtError>`             | Hydrate the in-memory cache from `kupferdraht.db` without hitting Wire. Requires `ClientBuilder::base_dir`. |
| `join_room(&str)`                                     | `Result<(), KupferdrahtError>`             | **Step 3.** External-commit join for one room. Fetches `GroupInfo`, runs openmls's `ExternalCommitBuilder`, uploads the commit bundle to `/v14/mls/commit-bundles`, and bumps the room's epoch to `server_epoch + 1` both in the cache and in `kupferdraht.db`. Requires `ClientBuilder::base_dir` so the MLS state persists across process restarts. **Step 86:** the never-established self conversation (`RoomType::SelfRoom` at `epoch == 0`) is **established** instead — Wire serves no `GroupInfo` before a member's first commit (`404 mls-missing-group-info`, live 2026-09-24): local group under the snapshot's `group_id`, the user's other devices Added (their Welcome rides the bundle, Step 69 absorbs it) or a self-update commit when this is the only device, epoch taken from openmls. A rejected bundle discards the local group so a later `join_all_rooms` retries (stale-snapshot race: another device established first → the next sync shows `epoch > 0` → external join). Non-self rooms are unchanged. |
| `sync_members(&str)`                                  | `Result<Vec<RoomMember>, KupferdrahtError>` | **Step 3** (renamed in **Step 14b** from `refresh_members` per finding §41). Re-derives the member list from the local MLS group's leaves, backfills `display_name` / `handle` via `/list-users`, and swaps the result into the in-memory and persisted `room_members` snapshot. Since Step 72a the rebuild merges each member's `RoomMemberRole` from the conversation-metadata snapshot (leaves carry no role) — a resync never strips administrators. Available the moment `join_room` has populated the group. matrix-rust-sdk uses this exact name. |
| `join_all_rooms()`                                    | `Result<JoinSummary, KupferdrahtError>`    | **Step 3.** Iterates the cached rooms, skips ones whose `group_id` is already in openmls storage (cheap local probe), and runs `join_room` on the rest. Per-room failures go into `JoinSummary.failed` rather than aborting the outer call. **Step 86:** through `join_room`'s branch a fresh device heals the self conversation automatically — established when it is at epoch 0, external-joined otherwise; either way it counts under `joined`. |
| `resync_existing_rooms()`                             | `Result<ResyncSummary, KupferdrahtError>`  | **Step 7e.** Walk every cached room whose `group_id` is already known locally and re-join any whose persisted MLS leaf belongs to a *prior* `device_id` of the same `user_id` (the failure mode after an interactive re-`login`, CHANGELOG.md §29). Rooms with no local group at all are out of scope (that's `join_all_rooms`). Per-room failures land in `ResyncSummary.failed`. |
| `resync_room(room_id: &str)`                          | `Result<(), KupferdrahtError>`             | **Step 7e.** Per-room recovery primitive `resync_existing_rooms` drives: discard the stale local MLS group state for `room_id` and re-join it via the unchanged Step-3 external-commit path (`join_room`). `InvalidInput` if `room_id` is not cached, has no `group_id`, or has no local MLS group to discard. |

### Membership: invite / kick (Step 6b / Step 6d)

Add or remove members from an existing room's MLS group via an
Add-commit / Remove-commit uploaded to `/v14/mls/commit-bundles`.
Complements the zero-invitee `create_room` (Step 6a). Both mutate the
room's epoch and member list, then **Step 14e** returns the
freshly-cached `Arc<Room>` — the same handle `Client::room(room_id)`
yields from then on (`Arc::ptr_eq`-verified).

| Method | Returns | Notes |
|---|---|---|
| `Client::invite_members(room_id: &str, user_ids: &[String])` | `Result<Arc<Room>, KupferdrahtError>` | **Step 6b.** Claims `KeyPackage`s for every listed invitee (`POST /v14/mls/key-packages/claim/{domain}/{uuid}`) plus the caller's own other devices — Wire requires every device of every member-user to have a leaf, so the self-claim is mandatory (see `observations.md`) — builds an Add-commit, uploads it, merges on success, then re-syncs metadata. `user_ids` are qualified `<uuid>@<domain>` strings, the same shape `Room::members().user_id` exposes. `InvalidInput` if `user_ids` is empty, an entry is malformed, or the room has no `group_id`. `BackendRejected` if Wire's claim pool is empty or the commit bundle is rejected. Since Step 71, a queued backend external Remove targeting a peer rides this commit (one epoch advance covers both) instead of being dropped. |
| `Client::kick_members(room_id: &str, user_ids: &[String])` | `Result<Arc<Room>, KupferdrahtError>` | **Step 6d.** Removes every leaf whose credential identity matches one of `user_ids` in a single Remove-commit, uploads it, merges on success, then re-syncs metadata. **Self-kick is intentionally rejected** — removing your own last device needs the proper logout / device-revocation lifecycle (a later Step), not this path. `InvalidInput` if `user_ids` is empty, the caller is in the list, or no leaf matches any listed user. Since Step 71, a queued backend external Remove targeting a peer rides this commit; an external Remove of a leaf the kick also targets is deduped by openmls. |
| `Room::invite(user_ids: &[String])`   | `Result<Arc<Room>, KupferdrahtError>` | Thin `Weak<Client>` wrapper delegating to `Client::invite_members`. `InvalidInput` if the owning `Client` has been dropped. |
| `Room::kick(user_ids: &[String])`     | `Result<Arc<Room>, KupferdrahtError>` | Thin `Weak<Client>` wrapper delegating to `Client::kick_members`. Same self-kick rejection as above. |

Wire-side, a kick emits `conversation.mls-message-add` to the removed
devices' notification streams plus `member-leave` to the rest of the
room; consumers handle both through their existing event handlers.

### Conversation access (Step 8b / Step 14d)

Wire's `access` / `access_role` conversation fields — which join
modes a conversation permits and which user classes may use them.
matrix-rust-sdk has no direct analog: Matrix' room access is
state-event-driven (`m.room.join_rules`), while Wire's `access` /
`access_role` are first-class `Conversation` fields.

| Method | Returns | Notes |
|---|---|---|
| `Room::access_modes()`   | `Vec<String>` | **Step 8b.** Cached snapshot of Wire's `access` array (`"invite"`, `"code"`, `"link"`, `"private"`). Empty when the room was hydrated from the local store without a fresh `sync_metadata` round-trip. |
| `Room::access_roles()`   | `Vec<String>` | **Step 8b.** Cached snapshot of Wire's `access_role` array (`"team_member"`, `"non_team_member"`, `"guest"`, `"service"`). Same fresh-on-sync semantics as `access_modes`. |
| `Room::access()`         | `Result<RoomAccess, KupferdrahtError>` | **Step 14d.** Fetch the **live** `(access, access_role)` pair via a single `POST /v{N}/conversations/list` and project it into a typed `RoomAccess`. Replaces the removed `Client::fetch_room_access(...) -> Result<(Vec<String>, Vec<String>)>`. Use when the cached `access_modes()` / `access_roles()` snapshot may be stale. First method on `Room` that issues a Wire HTTP call. |
| `Room::set_access(access: Vec<String>, access_role: Vec<String>)` | `Result<Arc<Room>, KupferdrahtError>` | **Step 8b.** `PUT /v{N}/conversations/{domain}/{cnv}/access` — replace both arrays. `Room::create_join_link` requires `access` to include `"code"`; guest-link redeemers also need `access_role` to include `"guest"` (typically `"non_team_member"` too). **Step 14e:** returns the refreshed `Arc<Room>`. Delegates to `Client::set_room_access`. |
| `Room::open_to_guests()` | `Result<Arc<Room>, KupferdrahtError>` | **Step 8b.** Convenience over `set_access` with the canonical guest-enabling configuration: `access=["invite","code"]`, `access_role=["team_member","non_team_member","guest"]`. Most Kupferdraht-created conversations default to `access=["invite"]` (members only); call this once as the prerequisite for `Room::create_join_link`. |
| `Client::set_room_access(room_id: &str, access: &[String], access_role: &[String])` | `Result<Arc<Room>, KupferdrahtError>` | **Step 8b.** The `Client`-level primitive `Room::set_access` delegates to. `InvalidInput` when `room_id` is unqualified, either list is empty, or the room is missing from the cache after refresh. `BackendRejected` on `403 access-denied` / `403 invalid-op` / `403 action-denied` (unauthorized caller) or `404 no-conversation`. |

```rust
pub struct RoomAccess {
    pub modes: Vec<String>,  // Wire's `access` array
    pub roles: Vec<String>,  // Wire's `access_role` array
}
```

`RoomAccess` (Step 14d) is the typed projection `Room::access()`
returns, replacing a bare `(Vec<String>, Vec<String>)` tuple.

### Timeline (Step 4)

| Symbol                                                | Returns                               | Notes |
|---|---|---|
| `Room::timeline()`                                    | `Result<Arc<Timeline>, KupferdrahtError>`  | Cheap construction. Each call returns a fresh `Arc<Timeline>` carrying an `Arc<Client>` plus the room id; safe to drop after a single send. |
| `Timeline::room_id()`                                 | `&str`                                | The room id this `Timeline` is bound to — the same string the owning `Room::id()` returned at construction time. |
| `Timeline::send_text(body)`                           | `Result<MessageHandle, KupferdrahtError>`  | Persist a `timeline_events` row as `Sending` (emitting its `Append`), encrypt a `GenericMessage(Text)` under the local MLS group, POST the resulting `MlsMessage(PrivateMessage)` to `/v14/mls/messages`, and settle the row to `Submitted` or `Failed` (`UpdateAt`) — see "Send lifecycle (Step 97a)". Requires the room's MLS group to be in local storage (`join_room` first). |
| `Timeline::send_text_auto_join(body)`                 | `Result<MessageHandle, KupferdrahtError>`  | **Step 14g.** Eager auto-join + send: probes the local openmls store via `Client::has_mls_group`; on a missing group, calls `Client::join_room` first, then proceeds with `send_text`. Closes `v1-api-ergonomics-audit.md §1.2` — plain `send_text` fails confusingly when the room is cached but the openmls store has no group yet. Use this for "just send, set up if needed"; keep `send_text` for callers that want to decline the join (e.g. handle an invite gracefully) or want to typed-match `KupferdrahtError::MlsGroupMissing` themselves. `InvalidInput` when the room is not cached or has no `group_id` on its metadata snapshot. |
| `Timeline::send_knock(hot_knock: bool)`               | `Result<MessageHandle, KupferdrahtError>`  | **Step 26.** Send a `GenericMessage(Knock)` (nudge / poke) over the same MLS encrypt path as `send_text`. `hot_knock` is Wire's double-tap flag; plain knocks pass `false`. Persists a `timeline_events` row with `kind='knock'`, `body='true'`/`'false'`. |
| `Timeline::send_location(latitude: f32, longitude: f32, name: Option<String>, zoom: Option<i32>)` | `Result<MessageHandle, KupferdrahtError>`  | **Step 26.** Send a `GenericMessage(Location)` (map point). `name` is an optional place label, `zoom` an optional map zoom level. Persists a `timeline_events` row with `kind='location'` and the coordinates packed as JSON in `body`. |
| `Timeline::send_ephemeral_text(body: String, ttl: Duration)` | `Result<MessageHandle, KupferdrahtError>`  | **Step 89.** Send a self-deleting text: `GenericMessage(Ephemeral { expire_after_millis: ttl, text })` over the same MLS encrypt path as `send_text` (which stays untouched — a plain text never expires). Official clients render a countdown and delete their copy locally; Kupferdraht's own row (`kind='text'`, `expires_at_ms = sent_at_ms + ttl`) self-redacts after `ttl` too. `ttl` must be 1 s … 365 days (`InvalidInput` otherwise, nothing uploaded). No mentions / quote inside the wrapper in V1. On an upload failure the row is persisted as `Failed` **without** an expiry stamp — the timer starts on the backend ack only, so the body stays available for a resend (the chosen TTL is persisted for exactly that, V28). See "Ephemeral messages" below for the timer contract. |
| `Timeline::resend(message_id: String)`                | `Result<MessageHandle, KupferdrahtError>`  | **Step 79.** Re-drive one failed send (`DeliveryStatus::Failed`, Step 25a — since Step 97a also an abandoned `Sending` row, which reads as `Failed`) from its persisted row under the **same `message_id`** (matrix-rust-sdk's `SendHandle::unwedge` shape — the transaction id survives; receivers dedupe on the id if the first attempt did reach the backend behind a 5xx proxy). Covers text incl. reply / mentions (re-read from the local parent and the `mentions` table), ephemeral text (re-wrapped with the original TTL; `expires_at_ms` stamped and the timer armed on success), knock, location, and assets (the blob already sits on `/v14/assets` — only the MLS reference travels again). Re-encrypts at the current epoch, uploads once (the stale-epoch self-heal applies), flips the row to `Submitted`, re-stamps `sent_at_ms` to the resend instant (the row moves to where recipients see it), emits `TimelineDiff::UpdateAt`. `InvalidInput` for an unknown id, a row that is not a failed send, a deleted row, or a **task / case / marker snapshot** (a stale full-state snapshot could regress every receiver's fold — re-send the current state via `update_task` / `set_case` / `set_task_list`). A transport error propagates and leaves the row `Failed` for the next attempt. |
| `Timeline::resend_failed()`                           | `Result<ResendSummary, KupferdrahtError>`  | **Step 79.** Re-drive every failed send of the room, **oldest first**, stopping at the first hard error (a stuck message must not be overtaken); snapshot rows are skipped, rows behind the stop stay `Failed` — the pass is re-runnable. Never errors as a whole once the rows could be read; the outcome is the summary. The room-scoped analog of matrix-rust-sdk's `SendQueue::respawn_tasks_for_rooms_with_unsent_requests`. |
| `ResendSummary { resent, skipped, stopped_at, remaining }` | (struct)                          | **Step 79.** `resent: Vec<MessageHandle>` oldest first; `skipped: Vec<String>` the snapshot rows (`task` / `case` / `task_list_marker`) left `Failed`; `stopped_at: Option<(message_id, error)>` the hard error the pass stopped at; `remaining: u32` the failed rows behind it that were not attempted (snapshot rows included — the next pass classifies them). |
| `Room::failed_sends()`                                | `Result<Vec<TimelineItem>, KupferdrahtError>` | **Step 79.** The room's failed sends — live `Failed` rows, fully overlaid, oldest first — so a consumer wires "sync came back → resend" in three lines (see "Reconnect contract"). Includes the snapshot rows `resend` refuses (nothing hides). matrix-rust-sdk analog: the `SendingFailed` local echoes a `RoomSendQueue::subscribe()` yields. |
| `Timeline::send_task(draft: TaskDraft, with_fallback: Option<bool>)` | `Result<MessageHandle, KupferdrahtError>`  | **Step 60a (ADR-0014).** Send a task: a `TaskPayload` in `GenericMessage` field 262000, *outside* the content oneof. `with_fallback: Some(true)` (normal chats) also sets the `Text` variant as human-readable fallback (assignee mention included, UTF-16 offsets; the checkbox reflects the status — `☑` done, `☒` cancelled); `Some(false)` (dedicated task-list rooms) sends the bare payload official clients ignore; `None` derives the default from the room's task-list marker (Step 75, ADR-0020: marked → off, unmarked → on) — an explicit choice always wins. The library generates the stable `task_id` and stamps `status: Open`. Persists a `timeline_events` row with `kind='task'` and the full state packed as JSON in `body`. Rejects an empty `title` with `InvalidInput`. |
| `Timeline::update_task(task_id: &str, patch: TaskPatch, with_fallback: Option<bool>)` | `Result<MessageHandle, KupferdrahtError>`  | **Step 60b.** Read the current folded state, apply the **set-only** patch (unset fields keep their value; clearing is not expressible in V1), and send the resulting **full state** as a new task message — LWW on every receiver, `Done → Open` reopens, any transition is legal. `InvalidInput` for an unknown `task_id` or an empty patched title. |
| `Room::tasks()` | `Result<Vec<FoldedTask>, KupferdrahtError>` | **Step 60b.** The folded task list of this room: one entry per `task_id`, latest full state wins (LWW in arrival order — the local proxy for the event stream's server order). A redacted task message (`MessageDelete` wipes the body) drops out of the fold — the prior state resurfaces; a task whose only message was deleted disappears. **Step 60c:** reads the materialized `tasks` table (fold-on-write, no per-read re-fold); ordering is `sort_key` first (manually moved tasks, lexicographic), then due date ascending (deadlines before none), then priority descending, then first-seen. |
| `Room::is_task_list()` | `Result<bool, KupferdrahtError>` | **Step 75 (ADR-0020).** Whether the room is marked as a dedicated task list: fold-on-read over `timeline_events` rows with `kind='task_list_marker'` — any member's last write wins in event order, redacted rows drop out, no rows means unmarked. |
| `Room::set_task_list(marked: bool)` | `Result<(), KupferdrahtError>` | **Step 75 (ADR-0020).** Send the full marker state (`TaskListMarker`, `GenericMessage` field 262002 — bare payload, no text fallback, official clients see nothing) and fold it locally. Any member may flip it; no admin gate. Marker rows surface as `TimelineItemKind::TaskListMarker { marked }` — the audit trail of the deliberately forgeable flag; consumers render a system line or hide repeats. `Client::invite_members` re-broadcasts the union of case snapshot and/or marker plus every task's current folded state once (the late-join MUST; all tasks, done/cancelled included, always without fallback). |
| `Timeline::move_task(task_id: &str, after: Option<String>, before: Option<String>, with_fallback: Option<bool>)` | `Result<MessageHandle, KupferdrahtError>` | **Step 60c.** Move a task in the room's manual order: computes a fractional `sort_key` strictly between the two neighbors (LexoRank principle, lowercase `a`–`z`, lexicographic) and sends it as an ordinary full-state update — a move touches exactly one task. `after`/`before` name neighbor `task_id`s; `None` means the corresponding end; **both `None` moves the task to the front of the manual order** — the bootstrap for a room with no keyed task yet. Manual ordering opts in per task: keyless tasks sort after all keyed ones, and a referenced neighbor must already carry a key. `InvalidInput` for unknown ids, a keyless neighbor, or an impossible slot. |
| `Client::my_tasks()` | `Result<Vec<RoomTask>, KupferdrahtError>` | **Step 60c.** Cross-room "my open work": tasks assigned to this account with status `Open` or `InProgress`, across every room in the local replica, in the ADR-0014 ordering. Index-backed read of the materialized `tasks` table — no timeline scan. |
| `Client::delegated_tasks()` | `Result<Vec<RoomTask>, KupferdrahtError>` | **Step 60c.** Cross-room "what I delegated": tasks this account created and assigned to someone else — any status, so progress (`done`) stays visible. Unassigned and self-assigned tasks are not delegated. |
| `MessageHandle { message_id, sent_at_ms }`            | (struct)                              | UUID v4 picked client-side at send time + the wall-clock at which Kupferdraht finished assembling the row. The `message_id` is the same id Wire echoes back on the sync stream and the recipient's client surfaces in quote / edit / reply references. |

### Assets (Step 11)

File-attachment send + receive per ADR-0011. AES-256-CBC over
Wire's `/v14/assets` REST endpoint, MLS-protected reference.

| Symbol                                                | Returns                               | Notes |
|---|---|---|
| `Timeline::send_file(&Path, &str)`                    | `Result<MessageHandle, KupferdrahtError>`  | **Step 11f.** Encrypt the file (AES-256-CBC + SHA-256, streamed), upload to `POST /v14/assets`, then send the MLS reference via the same `encrypt_and_upload` pipeline `send_text` uses. Consults `Client::file_sharing()` first (Step 10e gate) — `Disabled` rejects with `InvalidInput`, `Enabled` proceeds. |
| `Timeline::send_file_stream(source: Arc<dyn AssetSource>, filename: &str, mime_type: &str)` | `Result<MessageHandle, KupferdrahtError>` | **Step 36.** Streaming counterpart of `send_file`: reads the plaintext from a foreign [`AssetSource`](#assetsource) byte stream instead of a filesystem path, so a platform without a real path (e.g. Android's `content://` picker `InputStream`) can send without a temp-file copy of the plaintext. Same chunked, never-fully-buffered memory profile. `filename` is the display name — the stream carries no path to derive it from. Same error surface as `send_file`, plus any `KupferdrahtError` `AssetSource::read_chunk` returns while reading. |
| `Timeline::asset(&str)`                               | `Result<Option<AssetSummary>, KupferdrahtError>` | **Step 11h.** Look up the persisted asset for `message_id` in this timeline's room. `None` when no row matches (text-only message or not yet synced). |
| `AssetSummary::download()`                            | `Result<Vec<u8>, KupferdrahtError>`        | **Step 11g.** Lazy-cache the encrypted ciphertext at `~/.kupferdraht/<account_id>/assets/<message_id>.enc`, verify SHA-256 over the bytes, unpack the DEK envelope on `otr_key`, decrypt via AES-256-CBC. Capped at 64 MB — for larger assets use `save_to`. Returns `InvalidInput` when `encryption_algo = "AES_GCM"` (ADR-0011 §AES-GCM toleration). |
| `AssetSummary::save_to(&Path)`                        | `Result<(), KupferdrahtError>`             | **Step 11g.** Same pipeline as `download` but streams the plaintext to a filesystem path — suitable for multi-megabyte assets. |

```rust
pub struct AssetSummary {
    // Public metadata for UI display:
    pub message_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sender_user_id: String,
    pub room_id: String,

    // Asset locator + encryption flag for diagnostics:
    pub asset_id: String,
    pub asset_token: Option<String>,
    pub asset_domain: String,
    pub encryption_algo: String,   // "AES_CBC" | "AES_GCM"
    pub cache_path: Option<String>,
    // sha256 + otr_key_ct + Weak<Client> are crate-private
    // (security material — kept off the public surface).
}
```

`MessageBody::Asset(Box<AssetSummary>)` and
`TimelineItemKind::Asset(Box<AssetSummary>)` are the two
public-facing surfaces consumers see — event handler callbacks
get an `Asset` body; the timeline read API
(`Timeline::items` / `TimelineDiff::Append`) surfaces the
`Asset` kind. Both variants are boxed to keep enum stack size
compact next to the smaller `Text` variants.

### AssetSource

```rust
#[uniffi::export(with_foreign)]
pub trait AssetSource: Send + Sync {
    fn read_chunk(&self, max_len: u32) -> Result<Vec<u8>, KupferdrahtError>;
}
```

**Step 36.** Foreign-implemented byte source for
`Timeline::send_file_stream`. The app feeds the plaintext in chunks
(e.g. from an Android `content://` `InputStream`) instead of handing
Kupferdraht a filesystem path, so no temp copy of the plaintext is needed.
`read_chunk` is called repeatedly with `max_len` set to Kupferdraht's
encrypt chunk size (`STREAM_CHUNK`, 64 KiB); return an **empty**
`Vec` to signal end-of-input. Any `Err` aborts the send before
anything reaches Wire. Exported as a `#[uniffi::export(with_foreign)]`
callback interface (the same shape as `SecureKeyStore`), so mobile
embedders can implement it in Kotlin / Swift.

### JoinSummary

Returned by [`Client::join_all_rooms`]. Counters + a per-failure list:

```rust
pub struct JoinSummary {
    pub attempted: u32,          // rooms iterated (incl. already-joined)
    pub joined: u32,             // successfully joined this call
    pub already_joined: u32,     // skipped — group already in openmls storage
    pub failed: Vec<(String, String)>,  // (room_id, error_detail)
}
```

Since Step 86 an established self conversation (epoch 0 → 1 by this
device's commit, see `join_room`) counts under `joined` like an
external-commit join.

### ResyncSummary

Returned by `Client::resync_existing_rooms` (Step 7e). Flattened
counters plus a per-room failure list, mirroring `JoinSummary` above:

```rust
pub struct ResyncSummary {
    pub attempted: u32,          // cached rooms inspected (incl. ones with no local MLS state — skipped, not counted below)
    pub already_current: u32,    // persisted leaf already matches the current device_id
    pub resynced: u32,           // successfully rejoined via a fresh external commit this call
    pub failed: Vec<(String, String)>,  // (room_id, error_detail)
}
```

### Mentions (Step 6-II-g)

| Method                                                  | Returns                          | Notes |
|---|---|---|
| `Timeline::send_text_with_mentions(body, mentions)`     | `Result<MessageHandle>`          | Send a `Text` with `Text.mentions` populated. UTF-16 offsets must match `body`'s code units. matrix-rust-sdk analog: included in `Timeline::send` content. |
| `Client::resolve_mentions_in_body(body)`                | `Result<Vec<Mention>>`           | Scan the body for `@<handle>` tokens, resolve each via `POST /v{N}/list-users` (qualified-handles shape), return the matched mentions with UTF-16 offsets. Handles Wire can't resolve are silently skipped. |
| `Store::replace_mentions(room_id, message_id, mentions)` | `Result<()>`                    | Replace the mention set for one message wholesale (DELETE + per-mention INSERT in one tx). Mentions belong to a body version; on edit the inbound `Text.mentions` overrides. |
| `TimelineItemKind::Text::mentions: Vec<Mention>`        | (field)                          | Populated via overlay in `Client::messages`. Per item, mentions are filtered + projected from the new `mentions` sibling table. |

### Read receipts (Step 6-II-f)

| Method                                                  | Returns                          | Notes |
|---|---|---|
| `Timeline::mark_as_read(item_id: String)`               | `Result<Option<MessageHandle>>`  | Send a `Confirmation { type: READ, first_message_id }` for a single message. **Step 10f gate**: when `Client::receipt_mode()` returns `Off`, this returns `Ok(None)` without sending — peers respect the same setting account-wide. Wire-side ack is per-batch but Kupferdraht's first cut sends one id per call. matrix-rust-sdk analog: `Timeline::send_single_receipt`. |
| `Store::apply_read_receipts(room_id, reader_user_id, message_ids, confirmed_at_ms)` | `Result<usize>` | Per-batch `INSERT OR IGNORE` into `read_receipts`. Returns the count of fresh inserts. Idempotent on redelivery. |
| `Store::load_read_receipts_for_room(room_id)`           | `Result<Vec<ReadReceiptRecord>>` | All receipts for a room. `Client::messages` calls this once per call and overlays per-message `read_by` lists. |
| `TimelineItem::read_by: Vec<String>`                    | (field)                          | Reader user-ids that have confirmed READ on this item, sorted + deduplicated. |

### Delivery receipts (Step 25b)

Mirror of read receipts for `Confirmation { type: DELIVERED }`. Kupferdraht
**auto-sends** a delivery confirmation when an inbound content message
(text/asset) arrives — but **only in 1:1 rooms** (group delivery would be
N² traffic) and **ungated** by the read-receipt setting (unlike READ).
Best-effort: a failed confirmation never breaks receiving the message. The
**receive** side applies an incoming DELIVERED in any room type.

| Method / field                                          | Returns                          | Notes |
|---|---|---|
| `Store::apply_delivery_receipts(room_id, confirmer_user_id, message_ids, confirmed_at_ms)` | `Result<usize>` | Per-batch `INSERT OR IGNORE` into `delivery_receipts`. Idempotent. |
| `Store::load_delivery_receipts_for_room(room_id)`       | `Result<Vec<DeliveryReceiptRecord>>` | Overlaid onto per-message `delivered_to` lists, like `read_by`. |
| `TimelineItem::delivered_to: Vec<String>`               | (field)                          | Confirmer user-ids that have confirmed DELIVERED on this item, sorted + deduplicated. Empty in group rooms. |

**Consumer mapping.** Kupferdraht exposes the raw lists; the consumer composes a
scalar bubble status with an "any" rule: `Failed` → `Sending` → `Read`
(`read_by` non-empty) → `Delivered` (`delivered_to` non-empty) → `Sent`
(`Submitted`).
Kupferdraht does not compute this scalar (matrix-rust-sdk-idiomatic: local
send-state + receipt lists, app aggregates).

### Reactions (Step 6-II-e)

| Method                                                  | Returns                          | Notes |
|---|---|---|
| `Timeline::toggle_reaction(item_id: String, emoji: String)` | `Result<MessageHandle>`        | Toggle an emoji on the parent message. Wire's payload is the caller's CURRENT FULL SET of emojis on this message after the toggle (comma-separated CSV); receivers replace the sender's set in full. matrix-rust-sdk analog: `Timeline::toggle_reaction`. |
| `Store::replace_user_reactions(room_id, canonical_message_id, sender_user_id, emojis, sent_at_ms)` | `Result<()>` | DELETE + INSERT-per-emoji in one transaction; mirrors Wire's "replace the user's full set" semantic. |
| `Store::load_reactions_for_room(room_id)` | `Result<Vec<ReactionRecord>>`     | All rows for a room. `Client::messages` calls this once per room and overlays per-message groups onto returned `TimelineItem`s. |
| `Store::canonical_message_id(room_id, target_id)` | `Result<Option<String>>`         | Resolves either a stable PK or a V12 chain pointer to the row's stable PK. Reactions are stored under the stable PK so per-message aggregation works across edit chains. |
| `TimelineItem::reactions: Vec<ReactionGroup>`           | (field)                          | Aggregated by emoji (one entry per distinct emoji, `senders` deduplicated). |

### Delete (Step 6-II-d)

| Method                                                 | Returns                          | Notes |
|---|---|---|
| `Timeline::redact(item_id: &str)`                       | `Result<MessageHandle>`          | Delete-for-everyone the message keyed by `item_id`. Wire-side emits `MessageDelete { message_id }`; locally `Store::apply_message_delete` clears the body and stamps `redacted_at_ms`. **Step 15:** migrated from `String` to `&str` for caller-ergonomics symmetry with the other Timeline send paths. **Step 95b (ADR-0028):** `InvalidInput` without uploading when the local row of `item_id` was sent by another user — delete-for-everyone is the sender's (any own device counts; an id unknown locally goes out as before; the receiver-side ephemeral expiry is a separate internal path). The own-message check runs **first**, before the room's MLS group is resolved — for an unknown room it decides by the local row alone. matrix-rust-sdk analog: `Timeline::redact`. |
| `Store::apply_message_delete(room_id, target_message_id, redacted_at_ms)` | `Result<bool>` | Idempotent UPDATE: matches via `(message_id = ?target OR current_edit_event_id = ?target)` so a delete targeting an already-edited message via its latest chain id still works. Returns `true` when a row was retired, `false` for "parent missing" or "older delete redelivered". |
| Inbound authorization (Step 95b, ADR-0028) | — | An inbound `MessageDelete` applies only when its MLS-authenticated sender (ADR-0027) is the target's original **user** (any of their devices), **or**, for an **ephemeral** target, the self user (another own device syncing a receiver-side expiry) **or** any member when the target is the self user's own message (receivers enforce the sender's timer, observations §74). A self-sent delete of another member's ordinary message is ignored, as the official webapp does (observations §91). A third party's delete — of a normal message, someone else's ephemeral, a task carrier or a case snapshot — is dropped with a `warn!`: no row change, no diff, no fold change. No admin override (Wire parity). The target is resolved in the same room (`Store::load_timeline_event_in_room`). |
| `TimelineItem::redacted: bool`                          | (field)                          | `true` when `redacted_at_ms` is set. The `kind` field is then `TimelineItemKind::Redacted`. |
| `TimelineItemKind::Redacted`                            | (variant)                        | Body is hidden; `reply_to`, `mentions`, `edited_at_ms` are dropped from the projection. The row itself stays in `Room::messages` so reply chains pointing at it still resolve. |

### Ephemeral messages (Step 89)

Wire's `Ephemeral` wrapper (`GenericMessage` field 18) is supported
for the `Text` inner arm in both directions. The contract:

| Symbol | Notes |
|---|---|
| `Timeline::send_ephemeral_text(body, ttl)` | Send side — see the Timeline table above. |
| `TimelineItem::expires_at_ms: Option<i64>` / `Message::expires_at_ms: Option<i64>` | Set only for an ephemeral message: the wall-clock (ms) at which the local timer elapses. The body stays an ordinary `TimelineItemKind::Text` / `MessageBody::Text` — the TTL is a decoration, so consumers that render text keep working; a countdown UI reads the stamp. |
| Timer start (V1) | **Dispatch/send time.** Receive: `received_at_ms + ttl`, stamped when the row is inserted; send: `sent_at_ms + ttl` once the upload succeeded. Official Wire receivers start the countdown only when the message is actually *viewed* (viewport), which a headless SDK cannot observe — Kupferdraht therefore starts earlier, never later: it may tombstone a message an official client would still show, never the reverse (observations §74, `docs/wire/message-types.md`). A view-triggered start (e.g. on `mark_as_read`) is a possible V2. |
| Expiry | **Local tombstone, plus a receiver-side broadcast.** When the stamp passes, the row is tombstoned exactly like an inbound `MessageDelete` (`Store::expire_ephemeral_messages`: body cleared, `redacted_at_ms` stamped, `expires_at_ms` kept) and every subscriber receives a `TimelineDiff::UpdateAt` whose item is `redacted == true`, `kind == Redacted`, `expires_at_ms == Some(..)`. Reply chains keep resolving. For an **inbound** row Kupferdraht then sends one `MessageDelete` for the message into the room — the delete official senders wait for before purging their own copy (observations §74) — best-effort: a rejected upload is logged and not retried, the local tombstone is the contract. An expired **own** row sends nothing (official senders rely on the receiver's delete; Kupferdraht's own copy is already tombstoned). Not mirrored: the `DeleteForMe` into the self-conversation for the receiver's other devices (Step 86). |
| When the sweep runs | (1) a per-message native `tokio` timer armed at send / receive / resume; (2) at the end of every dispatch cycle (REST drain or push frame) — so a polling consumer is at most one interval late; (3) on `Client::load_rooms_from_store` (resume), which also re-arms the timers of still-pending rows. On wasm32 only (2) and (3) run — there is no task spawner; the sweep itself is the same SQLite sweep since Step 91; see `docs/bindings.md`. |
| Failed send + resend (Step 79) | A rejected ephemeral send persists the body **and the chosen TTL** (`timeline_events.ephemeral_ttl_ms`, V28) but no stamp. `Timeline::resend` re-wraps the row in the same `Ephemeral` envelope with that TTL — never as a permanent text — and stamps `expires_at_ms = resend instant + ttl` on success, arming the timer then. Inbound rows carry no TTL column (their stamp is derived at dispatch; an inbound row is never resent). |
| `Store::expire_ephemeral_messages(now_ms)` / `Store::pending_ephemeral_expiries(now_ms)` | The two storage primitives behind the sweep and the resume re-arm (default no-op for custom `Store` backends; the shipped store implements them on every target). `timeline_events.expires_at_ms` is the V27 column, with a partial index over live ephemeral rows. |

### Edit (Step 6-II-c)

| Method                                                 | Returns                          | Notes |
|---|---|---|
| `Timeline::edit(item_id: String, new_body: String)`     | `Result<MessageHandle>`          | Replace the body of `item_id` with `new_body`. Wire-side emits `MessageEdit { replacing_message_id, text }`; locally `Store::apply_message_edit` updates the existing `timeline_events` row in place and stamps `edited_at_ms`. **Step 95b (ADR-0028):** `InvalidInput` without uploading when the local row of `item_id` was sent by another user (every receiver would reject the edit; any device of the self user counts as own; an id unknown locally goes out as before). The own-message check runs **first**, before the room's MLS group is resolved. Consumers must surface this error — an app that discards the result (`.map(\|_\| ())`) turns it into a silent no-op. matrix-rust-sdk analog: `Timeline::edit`. |
| `Store::apply_message_edit(room_id, replacing_message_id, new_body, edited_at_ms)` | `Result<bool>` | Idempotent UPDATE: the row is mutated only when the supplied `edited_at_ms` is strictly newer than the persisted one (or the row was never edited). Returns `true` when a row was actually updated, `false` for "parent missing locally" or "older edit redelivered". |
| Inbound authorization (Step 95b, ADR-0028) | — | An inbound `MessageEdit` applies only when its MLS-authenticated sender (ADR-0027) is the target's original **user** (any of their devices) and the target is in the same room; anyone else's edit is dropped with a `warn!`. |
| `TimelineItem::edited_at_ms: Option<i64>`               | (field)                          | Wall-clock at which the most recent edit was applied. `None` for unedited items. |
| `TimelineDiff::UpdateAt`                                | (variant)                        | Emitted by both the outbound `Timeline::edit` and the inbound receive path so subscribers see the local row mutation. The `index` field is `u32::MAX` today — a placeholder until per-room ordered indices land in a future sub-step; consumers should match by `item.item_id`. |

### Quote / Reply (Step 6-II-b)

| Method                                                    | Returns                                       | Notes |
|---|---|---|
| `Timeline::send_reply(parent_id: String, body: String)`   | `Result<MessageHandle, KupferdrahtError>`          | Send a `Text` whose `Text.quote.quoted_message_id == parent_id`. Wire's other clients render this as a quote/reply UI. matrix-rust-sdk analog: `Timeline::send_reply`. |

`TimelineItemKind::Text::reply_to` is populated on both the
sending side (from the `parent_id` passed to `send_reply`) and
the receiving side (from `Text.quote.quoted_message_id`). The
parent row may legitimately not be in the local store — the
column is a soft reference; UI rendering looks up the parent
at render time and falls back to "(quoted message
unavailable)".

`Message::reply_to_id` (in the `EventHandler` callback shape)
mirrors the same value so consumers driving their own
event-handling pipeline can read it. The same holds for
`Message::expires_at_ms` (Step 89) and `TimelineItem::expires_at_ms`.

### Typing indicators (Step 21)

Typing is a plaintext `conversation.typing` event, **not** a
`GenericMessage` — it never touches MLS, is not persisted, and is
not echoed to the sender. Both directions are supported.

| Method                                                      | Returns                 | Notes |
|---|---|---|
| `Timeline::send_typing_started()`                           | `Result<(), KupferdrahtError>` | POST `conversation.typing` `started` for the room and auto-enqueue a `stopped` after an 11-second window. Deduped per room — repeat calls while one is in flight are no-ops, so it is safe to call on every keystroke. matrix-rust-sdk analog: `Room::typing_notice(true)`. |
| `Timeline::send_typing_stopped()`                           | `Result<(), KupferdrahtError>` | POST `stopped`, but only if a `started` is outstanding (a no-op otherwise). With the auto-stop this guarantees exactly one `stopped` per `started`. matrix-rust-sdk analog: `Room::typing_notice(false)`. |
| `Client::send_typing_started(room_id)` / `Client::send_typing_stopped(room_id)` | `Result<(), KupferdrahtError>` | The qualified-room-id-keyed forms `Timeline` delegates to. |

**Receive side.** Inbound `conversation.typing` events drive a
per-room set of currently-typing users. When the set changes,
every registered `EventHandler` fires `on_typing(TypingEvent)`
with the *current* set (an empty `typing_users` means the last
typist stopped). The set is ephemeral and is cleared at the start
of each sync session (the orphan safety net for a sender that
vanished without a `stopped`). Wire's backend runs no typing
timer; the only timer is Kupferdraht's send-side auto-stop.

> ⚠️ **Transport caveat — receive needs the WebSocket transport.**
> Wire delivers `conversation.typing` as a *transient* event over
> the live `/v14/events` WebSocket only; it is **not** persisted in
> the `/v14/notifications` pull queue (`docs/backend-spec.md` §11).
> Native targets receive it on the push transport — the default of
> every `sync*` loop variant since Step 118 (ADR-0033), and of
> `kupferdraht-cli sync-stream` — and the browser through its own push
> socket (Steps 115, 115b). Consumers that choose
> `SyncTransport::Polling` miss inbound typing. **Send works
> everywhere** (immediate REST POST). The 11-s auto-stop timer is
> native-only (`tokio::spawn` is cfg'd out on wasm; the browser
> prototype manages start/stop client-side).

```rust
pub struct TypingEvent {
    pub room_id: String,
    pub typing_users: Vec<String>, // qualified ids, sorted
}

// EventHandler gains (default no-op so existing handlers compile):
fn on_typing(&self, _event: TypingEvent) {}
```

### Timeline read API (Step 6-II-a)

| Method                                                    | Returns                                       | Notes |
|---|---|---|
| `Client::messages(room_id, &MessagesOptions)`             | `Result<Vec<TimelineItem>, KupferdrahtError>`     | Paginated read of `timeline_events` for `room_id`. Oldest-first. `MessagesOptions::all()` (`limit: None`) returns everything. `with_limit(N)` returns the N newest items (still oldest-first; `Some(0)` legitimately returns zero items — there is no 0-as-no-limit sentinel). `before(message_id)` truncates at that cursor. **Local-only read; no Wire HTTP.** matrix-rust-sdk analog: `Room::messages`. |
| `Room::messages(&MessagesOptions)`                        | `Result<Vec<TimelineItem>, KupferdrahtError>`     | Thin Weak<Client> wrapper that delegates to `Client::messages`. |
| `Timeline::items()`                                       | `Result<Vec<TimelineItem>, KupferdrahtError>`     | Convenience: `Client::messages` with `MessagesOptions::all()`. matrix-rust-sdk analog: `Timeline::items`. |
| `Timeline::subscribe(Arc<dyn TimelineListener>)`          | `Subscription`                                | Register a listener that fires for every `TimelineDiff` emitted on this room. Step 6-II-a only emits `TimelineDiff::Append` (outbound and inbound text); later sub-steps add `UpdateAt` / `RemoveAt` / `Reset`. **Step 14f**: returns a `#[must_use] Subscription` handle. Drop it (or call `.unsubscribe()` explicitly) to remove the listener; hold it to keep the listener alive. |
| `Client::add_timeline_listener(Arc<dyn TimelineListener>)`| `Subscription`                                | Lower-level: register a listener that observes diffs across **all** rooms. `Timeline::subscribe` wraps this with a per-room filter. Same `Subscription` lifecycle. |
| `Subscription::unsubscribe(self)`                         | `()`                                          | **Step 14f**. Explicit form of "drop the handle to unregister". Consumes `self`; `Drop` fires inline. Equivalent to `drop(subscription)`. |

### TimelineItem / TimelineItemKind / TimelineDiff

```rust
pub struct TimelineItem {
    pub item_id: String,             // GenericMessage.message_id
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub sent_at_ms: i64,
    pub kind: TimelineItemKind,
    pub edited_at_ms: Option<i64>,   // Step 6-II-c
    pub redacted: bool,              // Step 6-II-d
    pub reactions: Vec<ReactionGroup>, // Step 6-II-e
    pub read_by: Vec<String>,        // Step 6-II-f
    pub delivered_to: Vec<String>,   // Step 25b
    pub delivery_status: DeliveryStatus,
    pub expires_at_ms: Option<i64>,  // Step 89 — Some only for an ephemeral message
}

pub enum DeliveryStatus {
    Sending,     // persisted, upload in flight in this process (Step 97a); an abandoned one reads as Failed
    Submitted,   // sent locally (uploaded to Wire), awaiting the echo
    Received,    // arrived via the sync stream
    Failed,      // an outbound send failed; row kept so it doesn't vanish (Step 25a) — recoverable via Timeline::resend (Step 79)
}

pub enum TimelineItemKind {
    Text { body: String, reply_to: Option<String>, mentions: Vec<Mention> },
    Redacted,
    Asset(Box<AssetSummary>),                                            // Step 11
    Knock { hot_knock: bool },                                           // Step 26
    Location { latitude: f32, longitude: f32, name: Option<String>, zoom: Option<i32> }, // Step 26
    Task(Box<TaskItem>),                                                 // Step 60a (ADR-0014)
}

// Step 60a (ADR-0014) — tasks are event-sourced: every task message
// carries the full state of one task; `TaskDraft` is the send-side
// input (the library generates `task_id` and stamps `status: Open`).
pub struct TaskDraft {
    pub title: String,
    pub description: Option<String>,     // links are plain text inside it
    pub assignee: Option<String>,        // qualified user id `<uuid>@<domain>`
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
}

pub struct TaskItem {
    pub task_id: String,                 // stable UUID — deliberately NOT the message_id
    pub title: String,
    pub description: Option<String>,
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub status: TaskStatus,              // unknown wire values decode as Open
    pub priority: Option<TaskPriority>,
    pub sort_key: Option<String>,        // fractional index — consumed in Step 60c
}

pub enum TaskStatus   { Open, InProgress, Done, Cancelled }
pub enum TaskPriority { Low, Medium, High, Urgent }

// Step 60b — set-only partial change; None keeps the current value.
// Clearing an optional field is deliberately not expressible (nested
// Option does not cross the FFI boundary); the wire format supports
// it, so a clearing API can follow without a schema change.
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
    pub status: Option<TaskStatus>,      // any transition legal; Done → Open reopens
    pub sort_key: Option<String>,
}

// Step 60b — one task after folding its event log (LWW per task_id).
pub struct FoldedTask {
    pub item: TaskItem,
    pub creator_user_id: String,         // sender of the task's first message
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub latest_message_id: String,       // message carrying the current state
}

// Step 60c — the cross-room view element of Client::my_tasks() /
// Client::delegated_tasks().
pub struct RoomTask {
    pub room_id: String,                 // qualified room id `<uuid>@<domain>`
    pub task: FoldedTask,
}

pub enum TimelineDiff {
    Append { room_id: String, item: TimelineItem },
    UpdateAt { room_id: String, index: u32, item: TimelineItem },
    RemoveAt { room_id: String, index: u32 },
    Reset { room_id: String, items: Vec<TimelineItem> },
}

pub trait TimelineListener: Send + Sync {
    fn on_update(&self, diff: TimelineDiff);
}
```

Step 6-II-a populates only the plain-text happy path:
`TimelineItemKind::Text { body, reply_to: None, mentions: [] }`,
`edited_at_ms: None`, `redacted: false`, `reactions: []`,
`read_by: []`. The remaining fields are populated by 6-II-b
through 6-II-g.

`DeliveryStatus` is persisted as a string in
`timeline_events.delivery_status`; an unknown stored value (a DB
written by a newer Kupferdraht) degrades to `Submitted` with a `warn!`
rather than failing the room read.

**Send lifecycle (Step 97a).** Every send that creates its own row —
text (incl. reply, mentions), ephemeral text, knock, location, file,
task, case, task-list marker — writes that row as `Sending` **before**
the upload and emits its `Append`; after the upload the row is settled
to `Submitted` or `Failed` and an `UpdateAt` follows (an ephemeral
send gets its expiry there — only a delivered send has a timer). A
file's row and attachment are written after the blob upload, before
the MLS send. While the upload runs the row reads `Sending`; a
`Sending` row whose send is no longer in flight in this process — the
future was dropped (a Kotlin coroutine cancel), or the process
restarted — reads as `Failed` and is resendable under the same
`message_id` (`Timeline::resend`, `Room::failed_sends`). An upload
that succeeded is `Ok` even if writing its status afterwards fails:
the row then stays `Sending`, reads as `Failed`, and a resend goes out
under the same id (receivers dedupe on it) — never an error that
invites a second send under a new id. Edits, deletes, reactions and
receipts have no row of their own and are unchanged. Old binaries read
a `sending` row as `Submitted`.

Three consequences are part of the contract (Step 107):

- **"In flight" is per `Client`.** Each `Client` knows only its own
  sends. Two `Client`s on one home (e.g. a TUI and a CLI at the same
  time) read each other's live `Sending` rows as `Failed`;
  `failed_sends` lists them there, and a resend from that `Client`
  uploads the message a second time. Receivers dedupe on the id, so
  nobody sees it twice; the original send's status update then finds
  the row already `Submitted`. The browser is not affected — one tab
  hosts the only `Client` (ADR-0026).
- **A cancelled send emits no settling diff.** Dropping the future
  cannot run async work, so no `UpdateAt` follows; a subscriber keeps
  the item at `Sending` until it re-reads. After cancelling a send,
  re-read (`Room::messages`, `Room::failed_sends`) — the row reads
  `Failed` there.
- **An ephemeral send's timer starts at the persist.** `sent_at_ms` is
  stamped before the upload, so the local expiry
  (`sent_at_ms + ttl`) counts the upload time too.

A row being resent is left out of `failed_sends` and cannot be resent
a second time until that resend ends; `find_message_by_id` reads an
abandoned `Sending` row as `Failed` like every other reader.

### DM convenience (Step 14a)

| Method                                                | Returns                                  | Notes |
|---|---|---|
| `Client::create_dm(peer: &str)`                       | `Result<Arc<Room>, KupferdrahtError>`         | `GET /v{N}/one2one-conversations/{domain}/{uuid}` — server-side idempotent: first call against a fresh `(self, peer)` pair registers the 1:1, subsequent calls return the existing one. `peer` is qualified `<uuid>@<domain>` (single string per finding §42), split at the HTTP boundary. The fetch only reserves the conversation (hidden from `list-ids` until established, Step 68 — `docs/wire/api-notes.md`), so the call branches: local group present → resume; `epoch == 0` → establish (claim the peer's key packages, Add-commit, bundle upload; any failure discards the local group so a retry starts clean); `epoch > 0` → external-commit join. Then refreshes the local rooms snapshot so the returned `Arc<Room>` is the same handle `Client::room(id)` and `Client::dm_room(peer)` will yield. Unqualified `peer` (no `@`) is rejected as `InvalidInput`. The swagger lists a separate POST `/one2one-conversations` route; deployed wire-server does not serve it (nginx 404, recorded in `docs/wire/api-notes.md`) — Kupferdraht uses GET as the canonical surface. matrix-rust-sdk analog: `Client::create_dm`. |
| `Client::dm_room(peer: &str)`                         | `Option<Arc<Room>>`                      | Local-cache lookup — no network. Returns the cached `Arc<Room>` whose `room_type == DirectMessage` AND whose `counterpart.user_id == peer`. A group room that happens to include the peer as a member is **not** returned. Sync method (no `await`) — Step 14a switched the rooms-cache from `tokio::sync::RwLock` to `std::sync::RwLock` so `dm_room` matches matrix-rust-sdk's sync `get_room`/`get_dm_room` signature without breaking other accessors. No `get_` prefix per Rust API Guidelines C-GETTER (finding §41 verb taxonomy). matrix-rust-sdk analog: `Client::get_dm_room` (renamed). |
| `Client::dm_with(peer: &str)`                         | `Result<Arc<Room>, KupferdrahtError>`         | Wire-extension convenience: calls `dm_room(peer)` first; on `Some`, returns it without any HTTP traffic; on `None`, falls through to `create_dm(peer)`. Two concurrent `dm_with(peer)` calls against the same peer de-duplicate via a per-peer `tokio::sync::Mutex` map: the second caller waits for the in-flight POST, then observes the freshly-cached room when it re-checks `dm_room`. Exactly one POST per `(self, peer)` pair regardless of concurrent caller count. matrix-rust-sdk has no analog — the lookup-or-create pattern is dominant for Wire's first-class 1:1 endpoint (finding §42). |

### Lifecycle (Step 6e)

| Method                                                | Returns                   | Notes |
|---|---|---|
| `Client::leave_room(room_id: &str)`                   | `Result<(), KupferdrahtError>` | **Step 6e-1.** `DELETE /v{N}/conversations/{domain}/{id}/members/{self_d}/{self_uuid}`. Wire's external-sender produces the MLS Remove commit server-side; the leaving client never builds a self-Remove (openmls's `CannotRemoveSelf` is a hard guard rail — see observations.md §31). Locally discards the persisted MLS group via `MlsClient::discard_group` and re-syncs metadata. matrix-rust-sdk analog: `Room::leave`. |
| `Room::leave()`                                       | `Result<(), KupferdrahtError>` | Thin Weak<Client> wrapper that delegates to `Client::leave_room`. |
| `Client::delete_room(room_id: &str)`                  | `Result<(), KupferdrahtError>` | Delete a team conversation for **all** members (team-admin action, distinct from leaving). `DELETE /v{N}/teams/{tid}/conversations/{cid}`. The owning team id is read from the conversation's `team` field (fetched via `list_conversations`), falling back to the signed-in user's own team (`GET /self`) since Wire's conversation-list response often omits `team`. An account in no team yields `InvalidInput`. `403` surfaces as `BackendRejected`; a `404` (already gone) is idempotent success. Refreshes the cache via `sync_metadata` so the room drops out. No matrix-rust-sdk analog (Wire extension). |
| `Room::delete()`                                      | `Result<(), KupferdrahtError>` | Thin Weak<Client> wrapper that delegates to `Client::delete_room`. |
| `Client::logout(purge_device: bool, password: Option<&str>)` | `Result<(), KupferdrahtError>` | **Step 6e-2.** `POST /v{N}/access/logout` to revoke the `zuid` refresh cookie. If `purge_device == true`, **first** issues `DELETE /v{N}/clients/{device_id}` with the password (matches Step 1d's body schema); the trailing cookie revocation is then **best-effort** — deleting the device invalidates the session server-side, so Wire rejects the still-held token ("Invalid zauth token", live-verified 2026-07-19) and that failure is logged, not returned. Without `purge_device` a failing `/access/logout` stays an error. Caller is responsible for any local-state cleanup (the library does not own the SqliteStore lifetime via `&self`). matrix-rust-sdk analog: `Client::logout`. |

### Profile mutators (Step 2g)

| Method                                                | Returns                   | Notes |
|---|---|---|
| `set_display_name(&str)`                              | `Result<(), KupferdrahtError>` | `PUT /v{N}/self` with `{"name": "..."}`. matrix-rust-sdk analog: `Account::set_display_name`. Kupferdraht puts the mutator on `Client` for transport-locality (`Account` stays a frozen snapshot). The local `Account` returned by `Client::account()` is **not** updated — call `sync_once` to refresh. |
| `set_handle(&str)`                                    | `Result<(), KupferdrahtError>` | `PUT /v{N}/self/handle` with `{"handle": "..."}`. Wire enforces global uniqueness — collisions surface as `BackendRejected("update_self_handle 409: handle-exists")`. matrix-rust-sdk analog: `Account::set_handle`. |
| `update_email(&str)`                                  | `Result<(), KupferdrahtError>` | `PUT /v{N}/access/self/email` with `{"email": "..."}`. **Verification flow** — Wire sends a confirmation mail and returns `202`; the address changes only after the user clicks the link, so the local `Account` is **not** updated (refetch via `sync_metadata` once verified). `400 invalid-email` / `409 key-exists` (in use) surface as `BackendRejected` with the label in the body. matrix-rust-sdk analog: `Account` 3PID/email management (Drift). |
| `change_password(old: &str, new: &str)`               | `Result<(), KupferdrahtError>` | `PUT /v{N}/self/password` with `{"old_password","new_password"}` (Step 27). On success Wire invalidates **every** device's refresh cookie — all sessions must re-`login` (devices + MLS state survive). Deliberately does **not** touch the in-memory session: the current access token works until expiry, then `resume`'s rebind fails into re-login. `403 invalid-credentials` (wrong old password) / `400` (policy) surface as `BackendRejected`. matrix-rust-sdk analog: `Account::change_password` (Drift: Wire carries re-auth as `old_password`, not interactive-auth `auth_data`). |
| `rename_room(room_id: &str, new_name: &str)`          | `Result<Arc<Room>, KupferdrahtError>` | `PUT /v{N}/conversations/{domain}/{id}/name`. Wire emits a `conversation.rename` event to all members. Step 14e: after the PUT succeeds the local cache is refreshed via `sync_metadata` and the freshly-cached `Arc<Room>` (carrying the new name) is returned. Same handle `Client::room(id)` yields (`Arc::ptr_eq`-verified). matrix-rust-sdk analog: `Room::set_name` (Kupferdraht also exposes a `Room::set_name` thin wrapper that delegates here and returns `Arc<Self>`). |

### Devices (Step 1d)

| Method                                                | Returns                                | Notes |
|---|---|---|
| `devices()`                                           | `Result<Vec<DeviceInfo>, KupferdrahtError>` | `GET /v{N}/clients`. Lists every device registered for the authenticated user (max 7). Each entry has `device_id`, `class`, `label`, `model`, `registered_at`, `last_active`, `is_self`. matrix-rust-sdk analog: `Client::devices`. |
| `delete_device(&str device_id, &str password)`        | `Result<(), KupferdrahtError>`              | `DELETE /v{N}/clients/{id}` with `{"password": "..."}`. Wire requires the user's password — the access token alone is not sufficient. **Refuses** to delete the current device (use `Client::logout(purge_device=true)` once Step 6e ships). matrix-rust-sdk analog: `Client::delete_devices`. |
| `rename_device(&str device_id, &str label)`           | `Result<(), KupferdrahtError>`              | `PUT /v{N}/clients/{id}` with `{"label": "..."}`. Updates the user-visible device name. matrix-rust-sdk analog: `Client::rename_device`. |
| `key_package_count()`                                 | `Result<u64, KupferdrahtError>`             | **Step 63.** Remaining unclaimed MLS key packages for this device (`GET /v{N}/mls/key-packages/self/{client}/count`). Peers claim one per invite; at zero the device can no longer be added to new conversations (existing rooms keep working). Wire extension — matrix-rust-sdk has no key-package pool. |
| `replenish_key_packages()`                            | `Result<u32, KupferdrahtError>`             | **Step 63.** Tops the pool back up to 100 when it has fallen below 50 (the halving heuristic Wire clients use); returns the number uploaded, `0` = healthy. The sync surface runs this automatically at most once per 24 h (stamp in `sync_state`, warn-and-continue — a failing check never breaks sync); call directly to force it. |

### DeviceInfo

```rust
pub struct DeviceInfo {
    pub device_id: String,           // hexadecimal Wire client_id
    pub class: Option<String>,       // "desktop" | "phone" | "tablet" | "legacyImplicit"
    pub label: Option<String>,
    pub model: Option<String>,
    pub registered_at: Option<String>, // RFC 3339 / Wire `time` field
    pub last_active: Option<String>,   // RFC 3339 / Wire `last_active` field
    pub is_self: bool,                 // true when device_id == client.device_id()
}
```

### MLS-identity getters

| Method                             | Returns | Notes |
|---|---|---|
| `mls_signature_public_key()`       | `&[u8]` | Raw ECDSA-P256 uncompressed signature public key (65 bytes). Lookup key for `MlsClient::resume_with_storage`; the CLI persists a base64 encoding of this into `devices.mls_public_key`. |
| `backend_removal_key_ecdsa_p256()` | `&[u8]` | Raw (base64-decoded) removal key from `GET /v14/mls/public-keys`. Embedded into every MLS group's `external_senders` extension so admin-driven Remove proposals signed by Wire validate (ADR-0004). |

### Room devices & MLS thumbprints (Step 95d)

A room's devices are the leaves of its MLS group. Each carries the Wire
identity of its credential and the **MLS thumbprint** of its signature
key — the RFC 7638 JWK thumbprint (SHA-256, base64url, no padding) that
official Wire apps show as "MLS Thumbprint" in the device details, so a
user can compare a device across clients. Under `BasicCredential`s the
backend distributes the keys (ADR-0027): an injected device cannot be
prevented, but this surface makes it **visible**.

| Method / type | Returns | Notes |
|---|---|---|
| `Room::devices()` | `Result<Vec<RoomDevice>>` | One entry per leaf, own device included, sorted. `InvalidInput` when the room has no local MLS group. |
| `Client::own_device_thumbprint()` | `Result<String>` | This device's thumbprint — what it shows up with in every room. |
| `Client::subscribe_device_changes(listener: Arc<dyn RoomDevicesListener>)` | `Subscription` | One call per room whose device set changed, **for every room** (matrix-rust-sdk analog: `Encryption::devices_stream`). Drop the handle to stop. |
| `RoomDevice { user_id, device_id, credential, thumbprint }` | record | `user_id` / `device_id` `None` for an e2ei X.509 leaf (identity not parsed yet) or a malformed Basic one; `thumbprint` `None` when the key is not a P-256 SEC1 point. |
| `DeviceCredential::{Basic, X509}` | enum | The leaf's credential type. |
| `RoomDevicesChange { room_id, added, removed }` | record | A re-keyed device appears as removed (old leaf) **and** added (new leaf). |
| `RoomDevicesListener::on_devices_changed(change)` | — | Runs synchronously on the task that observed the change — return fast. |
| `Store::load_room_devices(room_id)` / `replace_room_devices(room_id, devices)` | `Result<Option<Vec<RoomDevice>>>` / `Result<()>` | The persisted baseline (V29 `room_devices`). Default bodies (no baseline → no change ever reported) keep custom stores compiling. |

**When a change is reported.** Wherever a room's MLS group changes epoch
— a merged inbound commit, a Welcome, a (re)join incl. the self-healing
rejoin, an own invite / kick, a committed backend removal — Kupferdraht diffs
the current leaves against the set it persisted for the room, persists
the new set, and reports a difference. The first observation of a room
records the baseline silently — read `devices()` for the initial set.
To keep that first observation from swallowing a real change,
`sync_once` records a baseline for every cached room that has none
**before** it drains notifications (rooms joined by an older build,
rooms created mid-session, a missed call site): the baseline is then the
pre-drain state and the drain's changes are reported. Because the baseline is persisted,
a change that happened while no client ran is reported at the room's
next epoch change. Official Wire clients have no such signal without
e2ei (they re-evaluate on epoch changes only for e2ei verification) —
this is a Kupferdraht addition, local only, invisible to other clients. A
local "verified" trust store (mark a device verified, room shield,
key-change warning) is deliberately not part of this step.

### Server-administered configuration (Step 10c + 10e)

**Raw accessors** (Step 10c):

| Method                                       | Returns                                            | Notes |
|---|---|---|
| `feature_configs()`                          | `Result<Vec<FeatureConfigEntry>, KupferdrahtError>`     | Snapshot of every team-feature config persisted to the V18 `feature_configs` table at the last successful login (`LoginPipeline::fetch_server_configs`) or `feature-config.update` event. Empty when no `SqliteStore` is attached. matrix-rust-sdk analog: `Account::data_settings`. |
| `feature_config(name: &str)`                 | `Result<Option<FeatureConfigEntry>, KupferdrahtError>`  | Look up one config by Wire's exact key (`fileSharing`, `mls`, `mlsE2EId`, …). `Ok(None)` when the key is absent or no store is attached. |
| `user_properties()`                          | `Result<Vec<UserPropertyEntry>, KupferdrahtError>`      | Snapshot of every user-property entry in V18's `user_properties` table. Wire keys are open-ended (`WIRE_RECEIPT_MODE`, `labels`, `webapp`, …). |
| `user_property(key: &str)`                   | `Result<Option<UserPropertyEntry>, KupferdrahtError>`   | One property by Wire's exact key. `Ok(None)` when absent. |

**Typed getters** (Step 10e) — projections of the most-consumed raw entries:

| Method                       | Returns                                       | Notes |
|---|---|---|
| `file_sharing()`             | `Result<FileSharingPolicy, KupferdrahtError>`       | Project `feature_configs.fileSharing` into [`FileSharingPolicy`]. Returns `Enabled` when the entry is missing. Backs the Step 11 file-attachment pre-flight check. |
| `receipt_mode()`             | `Result<ReceiptMode, KupferdrahtError>`             | Project `user_properties.WIRE_RECEIPT_MODE` into [`ReceiptMode`]. Returns `On` when the property is unset. Backs Step 10f's `Timeline::mark_as_read` gate. |

`PUT /v{N}/properties/{key}` and `DELETE /v{N}/properties/{key}` are
intentionally **not** exposed in V1 per ADR-0012 §Out of scope —
Kupferdraht reads server-administered configuration; the user sets and
clears properties via Wire's other clients. Step 10d's
`user.properties-set` / `user.properties-delete` event handlers keep
the persisted snapshot fresh.

### FeatureConfigEntry / UserPropertyEntry

```rust
pub struct FeatureConfigEntry {
    pub name: String,             // Wire's exact key — "fileSharing", "mls", ...
    pub status: Option<String>,   // "enabled" | "disabled" | None when feature is config-only
    pub config_json: String,      // raw `config` object as JSON; "{}" when absent
    pub last_synced_at_ms: i64,
}

pub struct UserPropertyEntry {
    pub key: String,              // Wire's exact key — "WIRE_RECEIPT_MODE", "labels", ...
    pub value: String,            // raw response body, opaque to Kupferdraht
    pub last_synced_at_ms: i64,
}

pub enum FileSharingPolicy {
    /// Team policy disables attachments outright. Step 11's send
    /// path rejects the action client-side.
    Disabled,
    /// Attachments allowed. Default when login could not populate
    /// the entry (ADR-0012 §Decision: permissive default).
    Enabled,
}

pub enum ReceiptMode {
    /// User has read receipts disabled in their Wire settings.
    /// `Timeline::mark_as_read` skips outbound `Confirmation`
    /// emission.
    Off,
    /// Read receipts enabled (default).
    On,
}
```

### Example

```rust
let user_id = client.user_id().unwrap();
let device_id = client.device_id().unwrap();
let api_version = client.api_version();
let display_name = client.account().display_name();
println!("logged in as {display_name} ({user_id} / {device_id}, v{api_version})");
```

---

## Login returns a ready Client

`ClientBuilder::resume` / `::activate` return a usable `Arc<Client>`
directly; `::login` returns a `PendingLogin` whose `register_device`
returns the `Arc<Client>` (the two-phase split, Step 24). Either way
there is no `LockedClient` / unlock split (removed in storage-1h): the
concrete store owns its own encryption and is opened **ready** by the
`StorageProvider` during login — it fetches the per-account key from the
`SecureKeyStore` (under `account-<id>`) and keys the whole-DB connection
itself.

**`resume` refreshes a stale access token.** Wire access tokens expire
after ~15 min, so a persisted session is usually past its TTL by the
next process start. Rather than force a re-login, `resume` rebinds the
token in place with the persisted `zuid` (cookie-auth
`POST /v{N}/access`), swaps the fresh token + rotated `zuid` into the
session, and re-persists — so a restart resumes silently as long as the
`zuid` is still valid. A dead `zuid` (its own, much longer TTL elapsed
or revoked) surfaces a `BackendRejected`, the signal to re-run `login`.
(Live-confirmed against a Wire v14 backend with a ~15-h-old `zuid`; see
`CHANGELOG.md` §58.)

A future **passphrase mode** (Steps 8 / 9) is a feature of a
passphrase-backed `SecureKeyStore` — the keystore is "locked" until the
passphrase is provided to *it* — not a client-level state. So the client
surface stays flat: usable as soon as `login` returns.

`ClientBuilder::keystore(Arc<dyn SecureKeyStore>)` injects a custom
keystore — the typical use is `InMemoryKeyStore` in tests. When unset,
`permit_http()` doubles as the test-mode signal: it swaps the desktop
`KeyringStore` default for `InMemoryKeyStore`.

## Keystore

The platform keystore stores the database key — a raw 32-byte
ChaCha20 key (ADR-0021) — directly — no KEK, no on-disk wrapped
envelope (the wrap/unwrap envelope was removed; see `open.md` §53). A
pre-Step-90 48-byte `key ‖ salt` blob of the SQLCipher era is rejected
like any other length (ADR-0021 I3: hard 32, no transition path). Each database
file has its **own** key, and every id carries the **home id**
(Step 93, ADR-0025): `kupferdraht.db` under `kupferdraht.<home-id>.account-<id>`,
`mls.db` under `kupferdraht.<home-id>.account-<id>-mls`, and the device
registry `global.db` under `kupferdraht.<home-id>.global-db`. The home id is
`base_dir/home.id` (32 lowercase hex, created once under the home lock;
`Kupferdraht::home_id`). The keystore is device-wide — one keychain service,
one `KUPFERDRAHT_DEV_KEYSTORE` directory — while a home is the unit of a
Kupferdraht installation, so two homes on one keystore never touch the same
entry: no shared database key, no first-key race between homes
(open.md gap-B.34), and a future purge of one home cannot reach
another's keys. The trait + implementations live in
`kupferdraht::keystore`; the key type + generation in
`kupferdraht::storage::db_key`. The SDK reads / writes these keys
internally during login / resume — there are no keystore
symbols on `Client` or `ClientBuilder` beyond `ClientBuilder::keystore`.

**Homes keyed before Step 93** (keystore-wide ids `global-db`,
`account-<id>`, `account-<id>-mls`) are adopted once: when the
home-scoped id is absent, the legacy id exists **and the database that
key opens already exists**, the open copies the secret under the
home-scoped id (inside the same locked critical section as a first-time
creation). A fresh database always gets a fresh key. The legacy entry
is **kept** — Kupferdraht cannot know whether another home on the same
keystore still needs it — and so is the retired `kupferdraht.install-id`;
deleting them is a manual cleanup once every home has been opened by a
Step-93 build. The login label cannot be adopted (the cookie slot is
Wire-side): such a home takes its per-home label at its next login
(device reuse, no new device slot).

### `SecureKeyStore` trait

```rust
#[uniffi::export(with_foreign)]
pub trait SecureKeyStore: Send + Sync {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError>;
    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError>;
    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError>;
}
```

A plain secret store. Kupferdraht stores the device-level `global.db` key
under `kupferdraht.<home-id>.global-db` and two keys per account under
`kupferdraht.<home-id>.account-<account_id>` and
`kupferdraht.<home-id>.account-<account_id>-mls` (Step 93); the `kupferdraht.`
prefix keeps the entries recognisable in a keystore shared with other
software (the `KeyringStore` service prefix, the `FileKeyStore`
directory, or a consumer's app namespace still scope the store as a
whole). Ids are ASCII, at most 89 bytes plus the account's domain —
within every backend's limits (Windows Credential Manager user name
513; the dev `FileKeyStore` file name 255, which caps the domain at 153
bytes). `get_secret` returns `None` for an absent id (the normal
first-launch case); `delete_secret` is the wipe primitive — dropping a
key renders the matching database permanently unreadable.

The trait is exported as a `#[uniffi::export(with_foreign)]` callback
interface (Step 20a), so mobile / Web embedders can implement it in
Kotlin / Swift. That dictates two signature choices: `id` is an owned
`String` (UniFFI marshals callback-method arguments by value, not as
`&str`), and the secret is `Vec<u8>` / `Option<Vec<u8>>` rather than a
fixed-size `[u8; 32]` (UniFFI callback interfaces don't marshal
fixed-size byte arrays). This is the lowest-common-denominator the
platform-specific embedders (iOS Keychain, Android Keystore, Web
SubtleCrypto) can implement directly. The 32-byte length is validated
internally at the call boundary (a foreign keystore returning the wrong
number of bytes surfaces as `KupferdrahtError::Generic`, never a panic).

### `KeyringStore`

Desktop default. Stores the database key directly in the OS keyring
(`keyring` crate ≥ 3.0; routes to the macOS Keychain or the Windows
Credential Manager), base64-encoded under the service prefix. **Linux:
only the kernel keyring (keyutils) today** — it is in memory and lost on
reboot (and when the kernel's persistent keyring expires), after which
the local databases cannot be opened again. Not for production use on
Linux until `open.md` §71 B4 (Secret Service) is done. Construct with a service prefix:

```rust
let store = kupferdraht::keystore::KeyringStore::new("kupferdraht");
```

Mobile embedders implement `SecureKeyStore` directly from their
platform language via the UniFFI callback shape; the **web target does
not** — it ships the built-in `IndexedDbKeyStore` (below). CI / scratch
installs use `InMemoryKeyStore`. Local-dev environments where the OS keychain is
unavailable (unsigned `cargo build` binaries on macOS) can opt into
`FileKeyStore` via the `KUPFERDRAHT_DEV_KEYSTORE=<dir>` env override (raw
32-byte key material per id at `<dir>/<id>.key`, mode 0600; `<dir>` is
created with mode 0700 on Unix, an existing directory keeps its mode).
The store is a plaintext downgrade, so it is loud and crash-safe
(Step 77): constructing one emits a `WARN` naming the directory — a
leftover env var in a CI config or shell profile never switches a
process to plaintext keys silently — and every write goes temp file
(0600 from creation) → fsync → rename → directory fsync, so a crash
mid-write leaves the previous key file intact rather than a truncated
one that would brick the database it keys; a stale `.tmp` is discarded
unread.

### `InMemoryKeyStore`

Process-scoped. Keeps every secret in a `Mutex`-guarded `HashMap`. Two
distinct `InMemoryKeyStore` instances do not share state, so a key
stored in one is invisible to another — integration tests rely on that
to model "wrong machine / wiped keychain" scenarios.

```rust
let store: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
```

### `IndexedDbKeyStore` (wasm32 only)

Browser default (Steps 46 + 58). Persists key material in IndexedDB,
encrypted via WebCrypto; `open()` preloads all secrets so the sync
`SecureKeyStore` trait can be served over the async IndexedDB API
(write-through on `set_secret`, `persist_all()` flushes). Worker-capable
since Step 58: WebCrypto is resolved via `js_sys::global()`, so the
store works from both `Window` and `WorkerGlobalScope`. Constructed
internally by `wasm::Kupferdraht::open` — web consumers never build it
directly. Exported at `keystore::IndexedDbKeyStore` under
`#[cfg(target_arch = "wasm32")]`.

### `storage::db_key`

```rust
#[derive(Clone)]
pub struct DbKey {
    key: Zeroizing<[u8; 32]>,  // raw ChaCha20 key — secret, wiped on drop
}
// key() accessor;
// from_keystore_bytes(&[u8]) -> Result<Self, KupferdrahtError> — runtime
//   length check, the one place the material-length invariant is
//   enforced: exactly 32 bytes; any other length (the pre-Step-90
//   48-byte blob included) is `KupferdrahtError::Generic`, never a panic;
// to_keystore_bytes() -> Zeroizing<Vec<u8>> — the 32-byte form;
// generate_db_key() -> DbKey.
```

`DbKey` is the per-database key material — the raw 32-byte key alone,
`Zeroizing` (ADR-0006, THREAT_MODEL G4). Two length constants are `pub`
alongside it (`KEY_LEN = 32`, `MATERIAL_LEN = KEY_LEN`) — `MATERIAL_LEN`
is the blob size a foreign `SecureKeyStore` stores. It is fed to
SQLite3MC's `chacha20` scheme as `PRAGMA key = "x'<64 hex>'"` after
`PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32` (the
parameters go BEFORE the key — SQLite3MC's contract), and the key is
verified by a first read at open. There is no salt: with a raw key the
scheme runs no KDF, so the 256-bit key is the whole security parameter
(ADR-0021; the former `SALT_LEN` / `salt()` left with Step 90). The
former per-field AES-256-GCM primitives (`encrypt_field` /
`decrypt_field` / `EncryptedField`) and the `column_migration` one-shot
transcrypt were removed once whole-DB encryption landed — sensitive
columns are now stored plaintext *inside* the encrypted database.

---

## Account

User-profile snapshot loaded at login time from `GET /v14/self`.
Mirrors `matrix_sdk::Account` (read accessors only at Step 1c —
mutating accessors land with later Wire features).

```rust
let account: Arc<Account> = client.account();
account.user_id();        // &str
account.display_name();   // &str  — Wire's "name" field
account.handle();         // Option<&str>
account.email();          // Option<&str>
```

Field-name mapping Wire ↔ matrix-rust-sdk:

| Wire DTO (`/self`)     | `Account` getter   | matrix-rust-sdk |
|---|---|---|
| `id`                   | `user_id()`        | `Account::user_id()` |
| `name`                 | `display_name()`   | `Account::get_display_name()` (async) |
| `handle`               | `handle()`         | (no exact analog) |
| `email`                | `email()`          | (3PID, separate flow) |

`Account::domain() -> &str` is not a separate Wire DTO field — it
projects `self.user_id().domain()`, i.e. the Wire deployment domain
the account belongs to (the deployment's domain). Always present,
since every [`UserId`](#userid) carries a domain by construction; no
`Option` and no separate nullable-domain column behind it (Step 19
retired the old nullable `self_user.domain` storage column).

`Account` is currently a snapshot — no live refresh. A future
`account.fetch()` method will reload from `/v14/self` (out of Step 1
scope).

---

## KupferdrahtError

Typed error enum. All public async methods return
`Result<T, KupferdrahtError>`. The enum in `error.rs` is authoritative;
current variants:

| Variant                | When raised |
|---|---|
| `InvalidCredentials`   | Wire `403 invalid-credentials` on login. |
| `SecondFactorRequired` | Wire `403 code-authentication-required`. |
| `InvalidSecondFactor`  | Wire `403 code-authentication-failed`. |
| `NotLoggedIn`          | Operation requires an authenticated client. |
| `Unauthorized`         | Access token expired or rejected mid-session (Wire `401`); distinct from the terminal `SessionExpired`. |
| `SessionExpired`       | The `zuid` refresh cookie is no longer valid — re-login required. **Terminal for the sync family**: `sync_once` / `sync` / `sync_stream` return it and the FFI background loop maps it to `SyncListener::on_session_expired` (fires at most once, loop already stopped). Also raised by `finalize_sso` on a revoked/rotated cookie. (Step 39) |
| `TooManyDevices`       | Wire's 7-device cap on `register_device`; the `PendingLogin` handle stays valid for recovery. (Step 24) |
| `AccountLocked(String)` | Another Kupferdraht process is inside the account's critical section — the token refresh + persist, or the first-time creation of a database key (the payload names the scope and the holder's pid when the lock file recorded one). Since Step 97d also from a sync call (`sync_once`, the sync loops) when another `Client` or process on the account kept the drain lock (`sync.lock`) for more than 5 s (scope `sync of account <id>`). **Retryable**: the holder finishes in milliseconds; nothing was changed on this side. Native only. (Steps 85, 97d) |
| `KeyUnavailable(String)` | A database (`global.db`, `kupferdraht.db` or `mls.db`) exists but the keystore returns no key for it; nothing was written to the keystore (the payload names the database and the keystore id). A locked or not-yet-ready keystore succeeds on a later attempt; a key that is gone for good leaves the database unreadable — the payload names the way out (`global.db` → the home's registry file; `kupferdraht.db` / `mls.db` → the account directory). Kupferdraht never replaces the key of an existing database. (Step 96, ADR-0029) |
| `RecoveryDataRequired(String)` | Browser only (ADR-0035): the home derives an account's keys from the account's seed, and this browser holds none for the account (the payload). A login opens nothing of it; `register_device` refuses unless asked for a first setup; `Kupferdraht::resume` refuses. Recoverable — restore recovery data (Step 112) or set up the browser as a new device. Native homes never raise it. (Step 110) |
| `HeldElsewhere(String)` | Browser only (ADR-0035): another instance holds the database (the payload) — its open was refused with `SQLITE_BUSY`. Take it over on request, or close the other one. (Step 111a) |
| `StorageFull(String)` | Browser only (ADR-0035): the storage quota is exhausted (`SQLITE_FULL`); the commit was rolled back. (Step 111a) |
| `Evicted` | Browser only (ADR-0023, ADR-0035): this instance stopped — another tab or (server mode) another browser took over, or the storage no longer admits the user; every further store operation fails. Terminate the worker. (Step 111a; reasons `device` / `access` Step 111b) |
| `NothingStored(String)` | Browser server mode only (ADR-0035): the storage server has no such database (the payload) — `resume` opens without creating (`SQLITE_CANTOPEN`). Log in again; the login creates the databases anew. (Step 111b) |
| `StorageAccessDenied(String)` | Browser server mode only (ADR-0035): the token exchange does not admit the user (`403`), or the storage server refused the storage token (`SQLITE_AUTH`). (Step 111b) |
| `ServiceUnavailable(String)` | Browser server mode only (ADR-0035): the storage server or the token exchange cannot be reached, or the exchange is overloaded (`503`); while the storage server is unreachable every operation of the account fails with it at once, until the storage heals itself (Step 113, ADR-0038). Retry later. (Step 111b) |
| `WrongPin { attempts_remaining }` | Browser only (ADR-0035): the PIN does not open the recovery data; `attempts_remaining` is how many more attempts the PIN service allows before its limit, when it said so. (Step 112) |
| `TooManyAttempts { retry_after_secs }` | Browser only (ADR-0035): the PIN service's attempt limit is reached (by default 5 per hour, 12 per day per user); try again after `retry_after_secs`. (Step 112) |
| `InvalidRecoveryData { reason }` | Browser only (ADR-0035): the bytes cannot be opened as recovery data — `reason` is `notRecoveryData`, `unsupported` (an unknown format version, a key version the PIN service retired, an Argon2id cost beyond what a browser gives) or `otherAccount` (refused before the PIN service is asked). (Step 112) |
| `SyncAlreadyRunning`   | A sync iteration is already in flight on this `Client` (concurrent `sync_once`/`sync`). (Step 14b) |
| `Network`              | Transport-layer failure (DNS, TLS, timeout). Native timeouts (Step 98): 10 s to connect; an API call 30 s end to end; an asset download 30 s without a byte (no overall bound); an asset upload has no bound of its own — it fails when the connection fails, and the app can cancel the send. |
| `BackendRejected`      | Non-2xx HTTP response; carries body text for diagnostics. |
| `RateLimited { retry_after_secs }` | The backend answered `429`; `retry_after_secs` is its `Retry-After` in seconds when it sent one (an HTTP-date or no header → `None`). The loop variants (`sync`, `sync_with_callback`, `sync_stream`, FFI `start_sync`, the push loop) wait it out — `Retry-After`, else `poll_interval`, bounded to 1–60 s — and carry on; every other call, `sync_once` included, returns it. (Step 103) |
| `Encoding`             | JSON / TLS / Protobuf parse failure. |
| `IntegrityCheck`       | A cryptographic integrity check failed on the asset path (SHA-256 / MAC / AEAD-tag mismatch). Migration checksum failures surface as `Storage`, not here. |
| `InvalidInput`         | Caller-supplied argument violates a precondition. |
| `MlsGroupMissing`      | No MLS group loaded for the conversation — `Client::join_room` first. |
| `MlsStaleMessage`      | Outbound MLS epoch is stale — resync the conversation and retry. (Step 37) |
| `MlsOutOfOrder`        | Inbound MLS message epoch mismatch — the room resyncs to the current epoch when the backend confirms it fell behind (heal, not retry; the message itself is not re-processed); otherwise the message is skipped. (Steps 38/64/106) |
| `Storage`              | SQLite or migration failure. |
| `Environment`          | Filesystem, missing env var, etc. |
| `Generic`              | Catch-all; carries the inner message. |

(The formerly documented `MlsOperation` variant does not exist — openmls
failures surface through the typed `Mls*` variants above or `Generic`.)

---

## Storage

Per-account SQLite database, encrypted **whole-file at rest** via the
vendored SQLite3MC engine in its `chacha20` scheme (ChaCha20-Poly1305, a
raw 32-byte key from the platform keystore, no KDF/salt in the trust
path; ADR-0021 — an accepted deviation from BSI TR-02102-1's AES-only
lists, see THREAT_MODEL G3). The first 32 bytes of the SQLite header are
kept plaintext (`PRAGMA plaintext_header_size = 32`) so iOS can identify
a WAL-mode DB; the page content cannot be read or migrated without the
key, and a wrong key fails at open (`KupferdrahtError::Storage`). Sensitive
columns are stored as plaintext inside the encrypted pages — there is no
second per-field encryption layer. Databases written by the SQLCipher
engine (before Step 90) are not migrated — start from a fresh home.

### `StorageEngineInfo` and `storage_engine()`

```rust
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageEngineInfo {
    pub engine: String,             // "SQLite3 Multiple Ciphers"
    pub version: String,            // "2.5.1"
    pub cipher: String,             // "chacha20"
    pub plaintext_header_size: u32, // 32
}
// Store::storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError>
//   — default `Ok(None)` (custom backends); SqliteStore reads it
//   off the live connection (`sqlite3mc_version()`, `PRAGMA cipher`,
//   `PRAGMA plaintext_header_size`).
// Client::storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError>
//   — the same through the client's app-data store; `None` without one.
```

What is actually linked and in effect, never a constant — the source of
`kupferdraht-cli status`'s `storage:` line. FFI-mirrored on the `Client`
object (`docs/bindings.md`).

**wasm32 target** (Step 91, ADR-0022; on sqlite-remote-vfs since Step
111, ADR-0035): the same `SqliteStore` / `SqliteGlobalStore` /
`openmls_sqlite_storage` over the same vendored SQLite3MC, compiled for
wasm by `sqlite-wasm-rs`, each database opened through its own
registration of `sqlite-remote-vfs` with SQLite3MC encrypting above it —
in its local mode (Step 111a) an IndexedDB database
`sqlite-remote-vfs-local/<home-id>/<path>`; in its server mode (Step 111b,
`storage: { mode: "server", url, tokenExchangeUrl }` on `Kupferdraht.open`) an
account's `kupferdraht.db` and `mls.db` live on a `sqlite-remote-server` under
the account's storage-login key (derived from its seed; the VFS gets a
signer over it) and an instance id stable per home and account (a reload
takes its own lease back, Step 111d), opened with a storage token the token
exchange issues for the Wire session and Kupferdraht renews in the background;
`global.db` stays local. Every operation on `kupferdraht.db` (`SqliteStore`) and
`mls.db` (the MLS provider) first checks the VFS's reported failure
(`storage::check_database`): a takeover by another instance stops the
instance (`BrowserEviction::Device`), a refused token is
`StorageAccessDenied`, and while the server is unreachable one cheap read
lets the VFS heal the database in place or the operation fails at once
with `ServiceUnavailable` (Step 113, ADR-0038); the account's state
(`online` / `reconnecting`) reaches the binding's `onStorage`. The wasm-only `pub` items in
`storage::`: `open_browser_storage(keystore, take_over)` (async; checks
the **dedicated worker** and **cross-origin isolation**, takes or takes
over the origin's instance lease, resolves the home id — the keystore
entry `BROWSER_HOME_ID_ENTRY`, `kupferdraht.home-id` — and returns it; called by
`wasm::Kupferdraht::open`), `prepare_browser_database(path)` (async; registers
a database's VFS before its synchronous open — the stores and
`open_default_store` need it first), `browser_database_vfs(path)` (the
encrypting VFS name of a prepared database, for a raw connection),
`browser_storage_evicted()` / `wait_for_browser_eviction()` (the eviction
flag and its future, which yields the `BrowserEviction` — `Tab`, `Device`
or `Access`; Step 92, ADR-0023, Step 111b) and `BROWSER_INSTANCE_LEASE`. Journal mode is the rollback
journal on wasm (the VFS provides no shared memory); everything else —
keying batch, key verification, engine identity, migrations, the Step-89
sweep — is identical to native. The pre-Step-91 IndexedDB record store and
MLS snapshot are gone; OPFS data of builds before Step 111 is left where it
is.

### Two processes on one home — the cross-process lock (Step 85, native)

Two Kupferdraht processes may share a home (kupferdraht-tui and kupferdraht-cli, say).
The databases take care of themselves: every native connection waits up
to 5 s for a concurrent writer (`busy_timeout`) instead of failing
`database is locked`, and every writing transaction starts `IMMEDIATE`
so the wait also covers one that reads first (Step 97d). Two things
cannot be shared through the database
and run under an advisory file lock (`std::fs::File::try_lock`; the
kernel releases it with the process, so a crash leaves nothing to clean
up):

- **the token refresh + persist** — Wire invalidates the `zuid` on every
  `/access`, so two processes rebinding the same cookie would kill the
  second one's session. Under the lock the persisted session is re-read
  first and a rotation another process already persisted is **adopted**
  instead of rebound (`Client` on a `401`, and `ClientBuilder::resume`
  when the persisted token has aged out);
- **the first-time creation of a database key** — `load_or_create_key`
  is get-then-set; two first initialisations would each generate a key
  and the second `set_secret` would orphan the database created under
  the first. An existing key never takes the lock (the common case is
  lock-free); an absent one is re-checked under the lock before
  generating.

Lock files: `base/kupferdraht.lock` (the device-registry key) and
`base/accounts/<id>/kupferdraht.lock` (the account's keys and its token
refresh) — per home, per account, never machine-wide, so two consumers
with two homes never meet. The posture is **fail-fast**: a held lock
surfaces as `KupferdrahtError::AccountLocked` (payload: the scope and the
holder pid when known), the caller retries; nothing blocks, no lock is
held for the process lifetime. A session without a home (a custom
`StorageProvider` and no `base_dir`) takes no lock. On the web the
Web Locks lease of ADR-0023 makes a second instance impossible, so the
browser build has none of this. Not covered: two *different* homes on
one keystore racing the first-ever creation of the same keystore id
(the ids are keystore-wide — open.md gap-B.34).

**Two `Client`s or processes syncing one account (Step 97d, ADR-0031).**
Only one cursor-advancing dispatch runs per account at a time: each
page of `GET /notifications` is fetched without a lock, then
dispatched under `base/accounts/<id>/sync.lock` (plus an in-process
mutex) — if another `Client` or process advanced the cursor in
between, the page is refetched instead of dispatched twice. Unlike
`kupferdraht.lock`, a held `sync.lock` is waited for (up to 5 s, then
`AccountLocked`), and it is never held across a network call: healing
a room, 1:1 delivery receipts and config re-fetches run after the page.
An MLS message from the next epoch while an own commit is being
uploaded is not healed around: the drain waits for this process's
commit to merge, or — the commit belongs to another `Client` — stops
before the message and picks it up on a later sync; a pending commit
that holds the drain back for 60 s is cleared and the message heals as
before. A message from an older epoch (e.g. a redelivered commit) is
skipped, never healed. A message from a later epoch heals the room only
when the backend's conversation record confirms the local group fell
behind; otherwise it is skipped and reported through `on_sync_error`. A
storage error while processing an MLS message never heals — the sync
returns it and the next one retries the notification (Step 106,
ADR-0034).

**At-least-once receive (Step 97e, ADR-0032).** An inbound
notification's MLS state is committed only after its data is stored in
`kupferdraht.db`, and the cursor only after that. A crash in between replays
the notification — nothing is lost, nothing is dispatched twice (the
handlers are idempotent per message id; a message whose key was already
used is skipped quietly). A failure to store an inbound message's data
now **ends the sync call with `KupferdrahtError::Storage`** (retryable; the
notification stays queued) instead of being reported through
`on_sync_error` and skipped.

**Per page (Step 109, ADR-0037).** A fetched page (up to 100
notifications) commits as a whole, not notification by notification.
What your handlers and listeners hear about a page — `on_message`,
`on_room_invited`, timeline diffs, device changes — arrives after the
page is committed, never for data that is rolled back afterwards, and
never twice. A notification that fails keeps the ones before it; the
next sync starts at the failed one. A send, or any other write, that
happens while a page is being processed waits for it: milliseconds
locally, a few hundred milliseconds on remote storage.

### `SqliteStore`

`SqliteStore::open(path, key)` opens the store **ready**: it applies the
32-byte key (`PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size =
32; PRAGMA key = "x'…'"`, then a first read verifies the key), switches to
WAL, sets the multi-process pragmas (`busy_timeout = 5000`, explicit
`synchronous = FULL` — Step 85, on `global.db` and `mls.db` alike) and
runs the migrations at construction. The
concrete store owns its encryption — there is no separate unlock step,
and the `Store` trait makes no key assumption. Re-opens are idempotent
(refinery skips already-applied migrations), and a migration run is
atomic (Step 87): all pending migrations and their history rows commit
in one transaction, so a failure or crash mid-run leaves the database
untouched and the next open simply retries.

```rust
// Normally the SDK / default provider does this for you, fetching the
// key from the keystore.
let store = kupferdraht::storage::SqliteStore::open(std::path::Path::new(&path), &key_bytes)?;
```

`SqliteStore::lock(&self) -> Result<ConnGuard<'_>, KupferdrahtError>`
acquires the underlying `rusqlite::Connection` directly (RAII guard,
hold it briefly). Exposed for integration tests that assert on raw
column shapes — not part of the matrix-rust-sdk-shaped public API.
`KupferdrahtError::Storage` if the inner mutex is poisoned.

### `Store` trait

```rust
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Store: MaybeSendSync {
    async fn save_session(&self, session: &SessionRecord) -> Result<(), KupferdrahtError>;
    async fn load_session(&self, user_id: &str) -> Result<Option<SessionRecord>, KupferdrahtError>;
    // … ~46 async methods in total; the trait in
    // `storage/records.rs` is authoritative.
}
```

The trait has grown well beyond the original session/device/rooms core;
its method families cover: sessions, devices, backend MLS keys,
rooms + members, timeline events (incl. edits/redactions and the
Step-79 resend flip `mark_message_resent`, guarded to live `failed`
rows), mentions, reactions, read + delivery receipts, sync-state
cursors, self-user and server-config snapshots, feature configs, user
properties, asset attachments, and the browser's recovery state
(`save_recovery_state` / `load_recovery_state`, Step 112: when the current
recovery data was made and the PIN's local verifier — one row, V30; no
default bodies, a custom store implements them), and `close` (Step 114:
the browser's purge closes the database before deleting it; later
operations refuse with `SessionExpired`; a default body that does
nothing). Consult `storage/records.rs` for signatures — this section
is the map, not the territory.

**`MaybeSendSync`** (Steps 44+): the supertrait bound. On native it is a
blanket alias for `Send + Sync`; on `wasm32` it is empty (single-threaded
JS runtime, `async_trait(?Send)`), which is what lets one `Store` trait
serve both targets. `GlobalStore` (the device-registry trait in
`storage/global.rs` — accounts list, active account, generic settings)
carries the same bound; it is not otherwise documented here. (The
login cookie label is **not** in `GlobalStore` — it derives from the
home id in `base_dir/home.id` (Step 93, `Kupferdraht::cookie_label`; the
Step-41 keystore entry `kupferdraht.install-id` is retired) and is recorded
per session in `SessionRecord.cookie_label`.)

**No pre-key session peek.** An earlier `SqliteStore::peek_session_user_id()`
helper (reading the session row's `user_id` before the per-account key
was known) was removed in Step 19c
(`docs/plan/archive/19-user-id-type.md`) once whole-DB SQLCipher
encryption landed: every column, including `session.user_id`, lives
behind the per-database key now, so there is nothing to read before
the key is supplied — a "peek before key" primitive is no longer
possible, let alone needed. The replacement is structural, not a new
method: `ClientBuilder::resume` takes the account id explicitly via
`.account(id)` (`KupferdrahtError::InvalidInput` if unset), and
`Kupferdraht::resume(account_id)` takes it as a plain argument sourced
from the device-level `GlobalStore` registry (`Kupferdraht::accounts()`).
Account selection happens before the per-account store is ever
opened, so the store never needs to expose an unauthenticated read.

### `SessionRecord`

```rust
pub struct SessionRecord {
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub cookie_label: Option<String>,
    pub api_version: u32,
    pub access_token_issued_at_ms: i64,
}
```

### `DeviceRecord`

```rust
pub struct DeviceRecord {
    pub user_id: String,
    pub client_id: String,
    pub mls_public_key: Option<String>,   // base64; populated at login from Step 1c.6 onward
}
```

### `SelfUserRecord`

```rust
pub struct SelfUserRecord {
    pub id: String,               // qualified <localpart>@<domain> (Step 19) — no separate domain column
    pub handle: Option<String>,
    pub name: String,
    pub email: Option<String>,
}
```

The user's own profile as loaded at login from `GET /v14/self`,
persisted so a later process can reconstruct `Account` without
hitting the backend.

### `ServerConfigRecord`

```rust
pub struct ServerConfigRecord {
    pub api_base_url: String,
    pub websocket_url: String,
    pub title: String,
}
```

The `ServerConfig` values Kupferdraht booted with, persisted so
`ClientBuilder::resume` can rebuild `HttpClient` without being
re-told which backend to talk to. Singleton row.

### `SyncStateRecord`

```rust
pub struct SyncStateRecord {
    pub sync_marker_uuid: Option<String>,
    pub last_event_id: Option<String>,
    pub last_connected_at_ms: Option<i64>,
}
```

Singleton row, one per device — Step 5b reads / writes this around
the notifications-drain loop so a reconnect resumes from the last
persisted Wire cursor instead of re-reading already-acked events.

### `MentionRecord`

```rust
pub struct MentionRecord {
    pub message_id: String,
    pub room_id: String,
    pub user_id: String,
    pub start: i32,
    pub length: i32,
}
```

One `(message, span, user)` tuple (Step 6-II-g). Mentions belong to a
specific version of the body; on edit they are replaced wholesale via
`Store::replace_mentions`.

### `TimelineEventRecord`

```rust
pub struct TimelineEventRecord {
    pub message_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub kind: String,
    pub body: String,                    // GenericMessage.Text.content plaintext, not MLS ciphertext
    pub sent_at_ms: i64,
    pub delivery_status: String,
    pub reply_to_id: Option<String>,     // Step 6-II-b — soft reference, quote parent
    pub edited_at_ms: Option<i64>,       // Step 6-II-c
    pub redacted_at_ms: Option<i64>,     // Step 6-II-d — body cleared once set
    pub expires_at_ms: Option<i64>,      // Step 89 — ephemeral timer (V27); survives the tombstone
}
```

One row of `timeline_events` — the record shape behind every
`TimelineItem` the public read API (`Client::messages`,
`Timeline::items`) projects. `Store::insert_timeline_event` /
`Store::load_timeline_events` / `Store::load_timeline_event` are the
primary read/write surface (`Store::load_timeline_event_in_room` is the
room-scoped form — rows are keyed `(message_id, room_id)`, so one id can
exist in two rooms; Step 95b, with a default body that filters the
room-agnostic lookup, so custom stores keep compiling); `find_message_by_id` and
`find_message_room` resolve either the stable `message_id` or a
chained edit-event id to the same underlying row.

### `AttachmentRecord`

```rust
pub struct AttachmentRecord {
    pub message_id: String,
    pub room_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sha256: Vec<u8>,
    pub otr_key_ct: Vec<u8>,             // AES key, stored verbatim — protected by whole-DB encryption, no second wrap
    pub asset_id: String,
    pub asset_token: Option<String>,
    pub asset_domain: String,
    pub encryption_algo: String,         // "AES_CBC" | "AES_GCM"
    pub cache_path: Option<String>,      // Some once the encrypted blob is locally cached
    pub created_at_ms: i64,
}
```

Persisted asset attachment (Step 11d, ADR-0011 §Storage shape),
anchored to a `timeline_events` row by `(message_id, room_id)`. The
public-facing projection consumers see is `AssetSummary` (see
[Assets](#assets-step-11) above); `AttachmentRecord` is the raw
storage shape `Store::insert_attachment` / `get_attachment` /
`list_attachments_for_room` operate on.

### `CaseSnapshot` / `CaseState` / `CasePriority` (Step 72a, ADR-0018)

```rust
pub struct CaseSnapshot {          // full state of one case
    pub revision: u64,             // LWW fold key (Step 72b), monotonic per (room, case_ref)
    pub case_ref: String,          // stable per-room case identity
    pub external_ref: Option<String>,   // file reference / leading system
    pub title: Option<String>,
    pub case_type: Option<String>, // deployment-defined classification
    pub state: CaseState,          // Open | Active | Waiting | Resolved | Closed | Cancelled
    pub state_detail: Option<String>,
    pub assignee: Option<String>,  // "<uuid>@<domain>"
    pub due_at_ms: Option<i64>,
    pub priority: Option<CasePriority>, // Low | Normal | High | Urgent
    pub opened_at_ms: Option<i64>,
    pub closed_at_ms: Option<i64>,
    pub authority_ref: Option<String>,  // tenant or public authority
    pub extensions: HashMap<String, String>, // display-only, no semantics
}
```

Carried by `GenericMessage` field 262001 (no text fallback — plain
Wire clients discard the payload). Unknown `state`/`priority` wire
values decode to `Open`/`None` so newer-client snapshots stay
visible. **Read API (Step 72b):** `Room::case() ->
Result<Option<CaseSnapshot>>` — fold-on-read over the room's
persisted case rows: admin-authorized snapshots (sender was a
conversation administrator at receive time; flag captured in the
row) last-writer-win by `revision`, non-admin rows contribute
display-only `extensions` (wholesale replace), redacted rows drop
out. Case rows surface as `TimelineItemKind::Case` in `messages()`
and as `MessageBody::Case { snapshot, sender_authorized }` in the
`EventHandler` callback. **Send (Step 72c):** `Room::set_case(patch:
CasePatch) -> Result<CaseSnapshot>` — applies the set-only patch
onto the fold, bumps `revision`, sends the full snapshot (client-side
admin gate mirrors the receive fold; `extensions` in the patch
replace wholesale), and folds locally. `Client::invite_members`
re-broadcasts the current snapshot right after the Add-commit merges
(ADR-0018 MUST — the invitee can only decrypt post-commit epochs;
best-effort, a failed re-send warns and self-heals on the next
change). CLI: `case-set` / `case-show`. Both FFI-mirrored.

### `Room` / `RoomMember` / `RoomType`

```rust
pub struct Room { /* fields private */ }
impl Room {
    pub fn id(&self) -> String;                               // "<uuid>@<domain>"
    pub fn name(&self) -> String;                             // empty for unnamed DMs
    pub fn room_type(&self) -> RoomType;
    pub fn epoch(&self) -> u64;
    pub fn counterpart(&self) -> Option<RoomMember>;          // only 1:1 DMs
    pub async fn members(&self) -> Result<Vec<RoomMember>, KupferdrahtError>;
    pub fn group_id_bytes(&self) -> Option<Vec<u8>>;          // raw MLS group_id
    pub fn group_id_b64(&self) -> Option<&str>;               // as Wire delivers it
    pub async fn join(&self) -> Result<(), KupferdrahtError>;                       // Step 3
    pub async fn sync_members(&self) -> Result<Vec<RoomMember>, KupferdrahtError>;   // Step 3, renamed in Step 14b
}

pub struct RoomMember {
    pub user_id: String,                 // "<uuid>@<domain>"
    pub is_self: bool,
    pub display_name: Option<String>,    // backfilled via /v14/list-users
    pub handle: Option<String>,
    pub role: RoomMemberRole,            // Step 72a: Administrator | Member
}

pub enum RoomType { DirectMessage, Group, Channel, SelfRoom }
```

`RoomType` has stable stringification via `as_str` /
`from_str_persisted` for the `rooms.room_type` column. An unknown
value on read is skipped silently so a schema extension can't break
an older binary.

### `BackendMlsKey`

Per-backend MLS public key, keyed by `(purpose, signature_scheme)`.
Today Kupferdraht stores exactly one row, `("removal", "ecdsa_secp256r1_sha256")`,
populated at login from `GET /v14/mls/public-keys` (ADR-0004).

```rust
pub struct BackendMlsKey {
    pub purpose: String,           // "removal"
    pub signature_scheme: String,  // "ecdsa_secp256r1_sha256"
    pub public_key: Vec<u8>,       // raw bytes, base64-decoded from Wire
}
```

### `RoomRecord` / `RoomMemberRecord`

Flat, backend-agnostic shapes that `Client::sync_once` writes
into the `rooms` + `room_members` tables. Consumers that only want
room metadata can drive the `Store` trait directly; otherwise use
`Client::rooms` which returns the richer `Room` type.

```rust
pub struct RoomRecord {
    pub room_id: String,                      // "<uuid>@<domain>"
    pub group_id: Option<String>,             // base64 MLS GroupId; None until Step 3
    pub name: String,
    pub room_type: String,                    // "direct_message" | "group" | "channel" | "self_room"
    pub epoch: i64,
    pub counterpart_user_id: Option<String>,  // only 1:1 DMs
    pub last_synced_at_ms: i64,
}

pub struct RoomMemberRecord {
    pub room_id: String,
    pub user_id: String,                 // "<uuid>@<domain>"
    pub is_self: bool,
    pub display_name: Option<String>,
    pub handle: Option<String>,
}
```

---

## Error handling pattern

```rust
use kupferdraht::{ClientBuilder, ServerConfig, KupferdrahtError};

let result = ClientBuilder::new().server(server).login(email, pw, None).await;
match result {
    Ok(client) => { /* ... */ }
    Err(KupferdrahtError::InvalidCredentials) => eprintln!("wrong email or password"),
    Err(KupferdrahtError::SecondFactorRequired) => eprintln!("2FA code required"),
    Err(KupferdrahtError::Network(detail)) => eprintln!("transport error: {detail}"),
    Err(other) => eprintln!("login failed: {other}"),
}
```

The error enum implements `std::error::Error + Display + Debug`
through `thiserror` so it integrates with `?` and `anyhow` in
consumer code.

**Runtime data is never asserted (Step 95c).** Everything a peer, the
backend, a stored row or the app can shape fails as a typed error, or
is skipped, and never panics:

- **Your arguments.** An empty credential or id, a malformed room or
  user id, or an empty task id is `InvalidInput` at the entry point.
  `FileKeyStore` also refuses a path-unsafe keystore id (`../x`).
- **Backend responses.** An empty token, client id, removal key or
  `/self` id is `Encoding`. A conversation without an id is skipped, an
  empty `group_id` means no MLS group, and an empty team means none.
  `conversations/list` is fetched in chunks of Wire's 1000-id cap.
- **Peer payloads.** A task without `task_id` is ignored (its text
  fallback shows), a case without `case_ref` is dropped, and a mention
  with an unusable user id is dropped. An inbound self-deletion timer is
  capped at 1 year (a non-positive one expires at once). A `set_case` on a case at revision `u64::MAX` is
  `InvalidInput`.
- **Stored state.** A session whose tokens were cleared after
  `SessionExpired` resumes to `SessionExpired`, a damaged device row is
  `Storage`, and a group stored in the old `PURE_CIPHERTEXT` wire format
  refuses commits with `InvalidInput` (re-join the room).

If a panic still happens, the MLS lock recovers, and the FFI sync loop
reports it through `on_error` and stops (`docs/bindings.md`).

---

## Join links (Step 8)

Wire's reusable guest-link mechanism. Owners of a conversation issue
a code/key pair via `Room::create_join_link`; anyone holding the
resulting URL (and the password, if one was set) can redeem it via
`Client::join_via_link`. The redeemer is added to the Wire
conversation member list and locally external-commit-joins the MLS
group.

### `JoinLink`

```rust
pub struct JoinLink {
    pub url: String,
    pub password_protected: bool,
}
```

Kupferdraht-public projection of Wire's `ConversationCodeInfo`. `url` is
the link the backend issues — its operator's account page with the
link's `key` and `code` in the query
(`https://account.<operator>/conversation-join/?key=…&code=…`; the
conversation's domain is not in it); `password_protected` mirrors Wire's
`has_password`.

### `Room::create_join_link`

```rust
impl Room {
    pub async fn create_join_link(
        &self,
        password: Option<String>,
    ) -> Result<JoinLink, KupferdrahtError>;
}
```

Issues a new link or returns the existing one. `password` is
`None` for an open link or `Some("…")` (≥ 8 chars per Wire's
schema) for a password-protected one. Routes to
`POST /v{N}/conversations/{cnv}/code`.

### `Room::current_join_link`

```rust
impl Room {
    pub async fn current_join_link(&self) -> Result<Option<JoinLink>, KupferdrahtError>;
}
```

Fetches the conversation's current link or `Ok(None)` when none is
on file. Routes to `GET /v{N}/conversations/{cnv}/code`. Wire's
`404 no-conversation-code` surfaces as `Ok(None)` so callers don't
need to special-case it.

### `Room::revoke_join_link`

```rust
impl Room {
    pub async fn revoke_join_link(&self) -> Result<(), KupferdrahtError>;
}
```

Idempotent revoke — `Ok(())` whether or not a code existed. Routes
to `DELETE /v{N}/conversations/{cnv}/code`.

### Client-level guest-link methods

The `Room::create_join_link` / `Room::current_join_link` /
`Room::revoke_join_link` methods above are thin `Weak<Client>`
wrappers; the `Client`-level primitives they delegate to are also
public (Step 8), for callers that hold a `room_id` string rather than
an `Arc<Room>`:

| Method | Returns | Notes |
|---|---|---|
| `Client::create_join_link(room_id: &str, password: Option<&str>)` | `Result<JoinLink, KupferdrahtError>` | Issue (or fetch the existing) reusable guest-link for `room_id` via `POST /v{N}/conversations/{cnv}/code`. `password` is `None` for an open link or `Some("…")` (≥ 8 chars per Wire's schema; Kupferdraht only enforces non-empty and lets Wire reject the rest). `InvalidInput` if `room_id` is unqualified or `password` is `Some("")`. |
| `Client::current_join_link(room_id: &str)` | `Result<Option<JoinLink>, KupferdrahtError>` | Fetch the conversation's current guest link via `GET /v{N}/conversations/{cnv}/code`, or `Ok(None)` when none is on file (Wire's `404 no-conversation-code` is absorbed). `InvalidInput` if `room_id` is unqualified. |
| `Client::revoke_join_link(room_id: &str)` | `Result<(), KupferdrahtError>` | Revoke the conversation's current guest link via `DELETE /v{N}/conversations/{cnv}/code`. Idempotent — revoking a non-existent code is `Ok(())`. `InvalidInput` if `room_id` is unqualified. |

All three take the qualified `<uuid>@<domain>` `room_id` and split
off the unqualified UUID internally — Wire's code-link endpoints take
the local id only, unlike the rename / member-management routes.

### `Client::join_via_link`

```rust
impl Client {
    pub async fn join_via_link(
        &self,
        code: String,
        key: String,
        password: Option<String>,
    ) -> Result<Arc<Room>, KupferdrahtError>;
}
```

`code` and `key` are the link's query parameters. The redeem (Steps 8,
122):

1. `GET /v{N}/conversations/join?key&code` — the conversation the
   link names (its id). A link the backend does not know — another
   operator's, or a revoked one — fails here with
   `BackendRejected(404 no-conversation-code)`, before anything is
   redeemed: a redeem only ever reaches the own backend's
   conversations.
2. `POST /v{N}/conversations/join` with `{key, code, password?}` —
   Wire adds the caller to the conversation member list (`204` for a
   member already).
3. `Client::sync_metadata` — the room list now holds the room; the
   call returns the room with the id from step 1.
4. `Client::join_room` (the same Step 3 external-commit path
   `RoomJoin` uses) MLS-joins the room — unless its group is here
   already.

A room list that lacks the room after the redeem is `Generic` (the
join happened; it is not the caller's input). **Breaking (Step 122):**
the `domain` parameter is gone — the link does not carry it, and the
conversation id finds the room.

`password` is `Some("…")` when the link is password-protected and
must be supplied; mismatch surfaces as
`BackendRejected(403 invalid-conversation-password)`.
`409 guest-links-disabled` surfaces when Wire has the team-wide
guest-link feature toggled off.

---

## Logging (Step 117)

Kupferdraht logs through `tracing`. It installs no subscriber of its own and
writes to no console or stream: the product decides where the logs go.

| Item | Notes |
|---|---|
| `logging::init_logging(level: LogLevel, listener: Arc<dyn LogListener>) -> Result<(), KupferdrahtError>` | Installs Kupferdraht's `tracing` subscriber on the first call and hands every event of Kupferdraht's own targets (`kupferdraht::…`) at `level` or above to `listener`; events of dependencies are not forwarded. A later call replaces level and listener (a debug switch at runtime). `InvalidInput` when another `tracing` subscriber is installed already — a Rust embedder that brings its own (the CLI's `RUST_LOG` one) keeps it and needs no listener. |
| `LogListener::on_log(&self, record: LogRecord)` | Runs on the thread that logged — return fast. `Send + Sync`. |
| `LogRecord { level, target, message }` | `message` is the event's message with its other fields appended as ` key=value`. |
| `LogLevel::{Error, Warn, Info, Debug, Trace}` | matrix-rust-sdk's vocabulary. |

The same API is exported to Kotlin and Swift (`initLogging`, the
`LogListener` callback interface) and backs the browser's `Kupferdraht.open({
logLevel, onLog })` (`docs/bindings.md`). Secrets do not reach it: the
`Debug` of secret-carrying types is redacted (Step 101).

## Time helper

```rust
pub fn now_ms() -> i64
```

Returns the current wall-clock time as milliseconds since the UNIX
epoch. The canonical source used by every `kupferdraht-core` module and
re-exported at the crate root so `kupferdraht-cli` (and any other
consumer that needs a wire-format timestamp for `SessionRecord` /
`TimelineEvent` / similar) reads from the same function instead of
duplicating the `SystemTime` boilerplate.

Saturates at `i64::MAX` on overflow (not reachable in practice
before year 292 million); returns `0` if the system clock is set
before the epoch.

---

## Async contract

Every method that touches the network or the SQLite store is
`async`. On native targets Kupferdraht uses `tokio` 1.x as the runtime;
consumers must provide a `tokio` runtime (typically `#[tokio::main]`).

`Client` is `Send + Sync` — share it across tasks via `Arc`.
Internal MLS state (`MlsClient`) is `Send + !Sync` (rusqlite's
`Connection` is `!Sync`); it is held strictly inside `encryption/`
and never crosses an await boundary held by reference.

**wasm32 target.** The crate is dual-target since Steps 44–59
(`pub mod wasm` is the browser binding layer; see `docs/bindings.md`).
On wasm there is no tokio: futures run on the JS microtask queue
(`spawn_local`), the `Send + Sync` bounds relax to `MaybeSendSync`,
and three behaviors differ from native — `sync_stream` returns a
single `Err(Generic(...))` (use the wasm `startSync` binding instead),
the typing auto-stop timer is absent (`tokio::spawn` cfg'd out), and
`sync_once` additionally flushes MLS IndexedDB snapshots after each
drain.

---

## Mapping to matrix-rust-sdk

For every public symbol, the matrix-rust-sdk equivalent (or the
documented reason for divergence) lives in
[`docs/api-mapping-matrix-rust-sdk.md`](api-mapping-matrix-rust-sdk.md).

Adding a new public symbol without a corresponding mapping entry
is a review defect. The mapping doc is the contract enforcement
mechanism for ADR-0009.

---

## Versioning

`kupferdraht-core` follows the workspace version (`Cargo.toml`
`workspace.package.version`). The public API is not yet stable —
breaking changes are expected pre-1.0. The stability commitment is
decided with the support-tier ADR that precedes the release cut
(PLAN.md Step 20e).

When a breaking change is unavoidable, update both this document
and the mapping doc in the same commit as the code change.
