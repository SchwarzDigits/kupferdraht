# Step 1 — Client bootstrap

> **Status:** merged — 1a `7b5b2fa` (live 2026-04-22), 1b `63e4396` (live 2026-04-23), 1c `580466c` (live 2026-04-23) decomposed into 1c.1 `1b14b5a` · 1c.2 `9046414` · 1c.3 `1f2d1ef` · 1c.4 `9668913` · 1c.5 `d67290b` · 1c.6 `42c0c9f` + live-fix `11622b9` (observations.md §19), 1d (live 2026-04-30, devices / delete / rename). Archived from PLAN.md.
>
> **Sub-steps:** 1a, 1b, 1c (1c.1–1c.6); + opportunistic 1d (Device management).
>
> **Historical names:**
> - "Step 1" — current. (No renames; introduced as Step 1 in PLAN.md v1.)
>
> **Branch / commit prefix:** `step/1-*`, `feat(step-1): …`, `chore(step-1): …`.

---

## Step 1 — Client bootstrap

Authenticate a user, register a device, materialize a
`Client` backed by a SQLite store. Everything every
subsequent step builds on.

Decomposed into **three sub-steps** so the foundation
lands in three small, independently-testable pieces
rather than one monolithic 2–3 day push. Each sub-step
produces a runnable state against live Wire staging.

### matrix-rust-sdk analog

`matrix_sdk::Client::builder().homeserver_url(...)` →
`matrix_sdk::Client` via `ClientBuilder::login`. Kupferdraht
mirrors the pattern with `ClientBuilder::server(...).login(email,
password)`.

Source references in `reference/matrix-rust-sdk/`:

- `crates/matrix-sdk/src/client/builder/mod.rs` — builder
  pattern for the client.
- `crates/matrix-sdk/src/authentication/matrix/mod.rs` — login
  flow shape (adapted, not copied — Wire auth differs).

### Public API (Rust + UDL)

```rust
// kupferdraht-core/src/client/builder.rs
pub struct ClientBuilder { /* ... */ }

impl ClientBuilder {
    pub fn new() -> Self;
    pub fn server(self, config: ServerConfig) -> Self;
    pub async fn login(
        self,
        email: String,
        password: String,
        second_factor_code: Option<String>,
    ) -> Result<Arc<Client>, KupferdrahtError>;
    pub async fn resume(
        self,
        user_id: String,
    ) -> Result<Option<Arc<Client>>, KupferdrahtError>;
}

// kupferdraht-core/src/client/mod.rs
pub struct Client { /* ... */ }

impl Client {
    pub fn info(&self) -> ClientInfo;
    pub fn account(self: Arc<Self>) -> Arc<Account>;
    pub async fn logout(&self, purge_device: bool) -> Result<(), KupferdrahtError>;
}

pub struct ClientInfo {
    pub user_id: String,
    pub device_id: String,
    pub server: ServerConfig,
    pub self_user: SelfUser,
    pub mls_public_key: String,
    pub api_version: u32,
}
```

UDL surface:

```idl
namespace kupferdraht {};

interface ClientBuilder {
    constructor();
    ClientBuilder server(ServerConfig config);

    [Async, Throws=KupferdrahtError]
    Client login(string email, string password, string? second_factor_code);

    [Async, Throws=KupferdrahtError]
    Client? resume(string user_id);
};

interface Client {
    ClientInfo info();
    Account account();

    [Async, Throws=KupferdrahtError]
    void logout(boolean purge_device);
};

dictionary ClientInfo {
    string user_id;
    string device_id;
    ServerConfig server;
    SelfUser self_user;
    string mls_public_key;
    u32 api_version;
};

dictionary ServerConfig {
    string api_base_url;
    string websocket_url;
    string title;
};

dictionary SelfUser {
    string id;
    string? handle;
    string name;
    string? email;
};

[Error]
enum KupferdrahtError {
    "InvalidCredentials", "SecondFactorRequired", "InvalidSecondFactor",
    "NotLoggedIn", "Network", "BackendRejected", "Encoding",
    "MlsOperation", "KeyPackageClaimFailed", "NoKeyPackagesForUser",
    "CommitRejected", "InvalidInput", "Storage", "Environment", "Generic",
};
```

### Sub-steps

| Sub-step | Scope | Wire API v14 | RFC 9420 | Store additions |
|---|---|---|---|---|
| 1a | HTTP infra, version negotiation, email/password login with optional 2FA, self-profile fetch. Pure transport — no MLS, no Proteus. | `GET /api-version`, `POST /login?persist=true`, `GET /v14/self` | — | `session`, `self_user`, `server_config` |
| 1b | Device registration with Proteus prekeys (schema requirement only — never used for messaging) + **mandatory access-token rebind**. Closes the Z-Client gap that bit us in the first pass (LEARNINGS §1.4). | `POST /v14/clients`, `PUT /v14/clients/{client_id}`, `POST /v14/access?client_id=<id>` | §7 (Credentials) | `devices` |
| 1c | MLS identity: P-256 signing key, `BasicCredential`, `[Basic, X509]` leaf capabilities, KeyPackage batch upload, backend removal-key fetch, `SqliteStorageProvider` wired into openmls. | `POST /v14/mls/key-packages/self/{client_id}`, `GET /v14/mls/public-keys` | §7.2 (Capabilities), §10 (KeyPackage), §10.2 (KeyPackage signature, `ECDSA_SECP256R1_SHA256`) | openmls tables via `SqliteStorageProvider::run_migrations()` |

Each sub-step ends in a branch commit and a CLI smoke:

- **After 1a:** `kupferdraht-cli login ...; kupferdraht-cli status`
  shows `user_id`, API version `14`, self-profile. No
  `client_id` yet.
- **After 1b:** `kupferdraht-cli status` additionally shows
  `device_id`. `Z-Client`-scoped calls succeed (verify by
  hitting any cheap client-scoped endpoint in the next
  sub-step's dev loop).
- **After 1c:** `kupferdraht-cli status` shows the
  `mls_public_key` prefix. A fresh run re-uses persisted
  openmls state without re-uploading KeyPackages.

### Wire API v14 endpoints

Consolidated list (each assigned to a sub-step in the
table above):

- `GET /api-version` — version negotiation
  (§1 of `docs/wire/api-notes.md`).
- `POST /login?persist=true` — email/password + optional 2FA
  (§2). Returns access token + `Set-Cookie: zuid=<refresh>`.
- `GET /v14/self` — self-profile enrichment (§9).
- `POST /v14/clients` — Proteus prekey bundle (required by
  schema, see below) + MLS capabilities + initial MLS
  public key. Returns `client_id` (§3).
- `PUT /v14/clients/{client_id}` — MLS public-key upload as a
  separate call if not inlined into `POST /v14/clients` (§3).
- `POST /v14/mls/key-packages/self/{client_id}` — initial
  batch of 100 KeyPackages (§4).
- `GET /v14/mls/public-keys` — Wire's external-sender
  removal keys. Kupferdraht reads the `ecdsa_secp256r1_sha256`
  entry (§4, ADR-0004).
- `POST /v14/access?client_id=<id>` — **mandatory** rebind of
  the access token to the freshly-registered device. Without
  this, every subsequent client-scoped call returns
  `400 "Header Z-Client is required"` (LEARNINGS §1.4, §2).

### RFC 9420 sections

- §7 — Credentials, `BasicCredential`.
- §7.2 — Capabilities (we declare `[Basic, X509]` in the
  LeafNode capability set).
- §10 — KeyPackage structure.
- §10.2 — KeyPackage signature (we sign with
  `ECDSA_SECP256R1_SHA256`).

### Wire Message Types

None — Step 1 is pre-messaging.

### Store additions (`storage/schema.rs`)

First migration creates:

- `session` table — user_id, access_token_ct, refresh_token_ct,
  cookie_label, api_version, access_token_issued_at, plus the
  encryption metadata columns kept empty in Step 1, used from
  Step 7 onward.
- `devices` table — device_id, mls_public_key.
- `self_user` table — id, handle, name, email_ct.
- `server_config` table — api_base_url, websocket_url, title.

Plus openmls's own tables via `SqliteStorageProvider::run_migrations()`.

### Implementation notes

- **Proteus prekeys at `POST /v14/clients`.** Wire's schema
  declares `prekeys` and `lastkey` as non-nullable — a client
  record cannot be created without a Proteus prekey payload.
  Generated via `proteus-wasm`, uploaded once, never used for
  encryption (ADR-0001, CLAUDE.md §Proteus). Kupferdraht does not
  implement Proteus message flows.
- **Ciphersuite and credentials** set as cross-cutting
  constants (`MLS_CIPHERSUITE`, `MLS_SIGNATURE_SCHEME`,
  `MLS_PUBLIC_KEY_TYPE`, `LEAF_CAPABILITIES`) in
  `encryption/mod.rs`. Reused by Steps 3–6.
- **Access-token lifecycle:** proactive refresh in `resume`
  if stored token is older than 10 minutes. Reactive 401
  retry wrapper lives in `wire/http.rs` and applies to every
  authenticated call (unified with Step 5's WebSocket
  reconnect).
- `Client` is returned as `Arc<Client>` so consumers can
  share it across tasks. `!Sync` constraints sit on
  `MlsClient` inside `encryption/` — `Client` itself is
  `Send + Sync`.

### TDD starter tests

1a:
- `ClientBuilder::server` carries config through to
  `ClientBuilder::login`.
- `login` with invalid credentials returns
  `KupferdrahtError::InvalidCredentials`.
- `login` with `SecondFactorRequired` response propagates
  the typed variant.
- `GET /v14/self` after successful login parses into
  `SelfUser` with `id`, `handle`, `name`.
- `Store::save_session` + `Store::load_session` round-trip.
- `storage::schema::migrations` run idempotently on an
  existing DB.

1b:
- `POST /v14/clients` with a `proteus_prekeys` + `lastkey`
  payload yields a `201 Created` and a `client_id`.
- Missing `prekeys` or `lastkey` is rejected by Wire with a
  schema error — regression guard so we don't accidentally
  drop them.
- Immediately after `POST /v14/clients`, `POST /v14/access?client_id=<id>`
  succeeds; the **next** client-scoped call (any `/v14/…`
  with `Z-Client` implied) returns 2xx, not 400.
- `devices` table gains one row keyed by the new
  `client_id`.

1c:
- 100 KeyPackages are generated, TLS-serialized, and
  accepted by `POST /v14/mls/key-packages/self/{client_id}`.
- Each KeyPackage's leaf declares
  `[CredentialType::Basic, CredentialType::X509]` in its
  capability set.
- Each KeyPackage is signed under `ECDSA_SECP256R1_SHA256`.
- `GET /v14/mls/public-keys` returns an
  `ecdsa_secp256r1_sha256` entry; Kupferdraht stores it ready
  for Step 3's external-senders extension wiring.
- `Client::info()` returns the expected fields after a
  successful login end-to-end (integration test, mocked
  HTTP).
- A subsequent process start reloads the openmls state
  from `kupferdraht.db` without re-uploading KeyPackages.

### CLI command (step 1)

```
kupferdraht-cli login    -b <staging|production> -e <email> [--password-stdin]
kupferdraht-cli status
kupferdraht-cli resume
kupferdraht-cli logout   [--purge-device] [--password-stdin]
```

- No `-p/--password` flag (secrets in argv leak). Password
  read from TTY or `--password-stdin`.
- `logout --purge-device` also issues
  `DELETE /v14/clients/{id}` to free a Wire device slot.

### Smoke test

```bash
echo "$PASSWORD" | kupferdraht-cli login -b staging -e "$EMAIL" --password-stdin
kupferdraht-cli status
kupferdraht-cli logout
```

`kupferdraht-cli status` must show `user_id`, `device_id`,
`mls_public_key` prefix (first 12 chars), API version `14`.

---

## Step 1d — Device management (opportunistic)

Originally tracked in PLAN.md's separate "Opportunistic — Steps 1d and 2g" block alongside Step 2g. Hoisted here so the Step-1 file stays the single home for everything device-registration-shaped. The opportunistic framing — "lands when a slot opens up between Steps 6e and 7a, not gating for any later Step" — is preserved in PLAN.md §Suggested implementation order.

`Wire`'s 7-device-per-user cap is a real friction point: when
the cap is hit, `POST /v14/clients` (Step 1b) fails and the
only current remediation is to delete a device via Wire's web
UI. `Client::devices()` + `delete_device()` solves that
end-to-end from Kupferdraht.

#### matrix-rust-sdk analog

`Client::devices()`, `Client::delete_devices(...)`,
`Client::rename_device(...)`. Kupferdraht maps these directly onto
Wire's `/v14/clients` endpoints.

#### Public API

```rust
impl Client {
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError>;
    pub async fn delete_device(&self, device_id: String) -> Result<(), KupferdrahtError>;
    pub async fn rename_device(&self, device_id: String, label: String) -> Result<(), KupferdrahtError>;
}

pub struct DeviceInfo {
    pub device_id: String,
    pub label: Option<String>,
    pub model: Option<String>,
    pub class: String,                  // desktop/mobile/tablet/legacyImplicit
    pub registered_at_ms: i64,
    pub is_self: bool,
}
```

#### Wire API v14 endpoints

- `GET    /v14/clients` — list this user's registered devices.
- `DELETE /v14/clients/{id}` — revoke a device by id.
  Wire requires the password in the JSON body for the
  caller's own non-MLS devices; Kupferdraht prompts for it
  via `--password-stdin` (consistent with `logout
  --purge-device`).
- `PUT    /v14/clients/{id}` — update label.

#### CLI commands

```
kupferdraht-cli devices
kupferdraht-cli device-delete --id <client-id> [--password-stdin]
kupferdraht-cli device-rename --id <client-id> --label "MacBook Pro M2"
```

#### Smoke test

```bash
kupferdraht-cli devices                        # lists current cap usage
kupferdraht-cli device-delete --id <oldest>    # frees a slot
kupferdraht-cli device-rename --id <self> --label "Kupferdraht CLI dev"
```

### CLI commands

**Step 1 — client:**

| Command | Options |
|---|---|
| `login` | `-b/--backend <staging\|production>`, `-e/--email`, `--password-stdin` |
| `status` | — |
| `resume` | — |
| `logout` | `--purge-device`, `--password-stdin` |
