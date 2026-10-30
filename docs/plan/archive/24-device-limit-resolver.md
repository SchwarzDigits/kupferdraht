# Step 24 — Two-phase login (device-limit recovery)

> **Status:** merged on `step/24-device-limit-resolver`. Closes the
> device-limit handling gap (POC spec §4 „Limit-Handling: Spezieller
> Resolver-Mechanismus") from the natrium migration, via a two-phase login
> rather than a separate resolver object. `too-many-clients` recovery without
> re-login via `PendingLogin` (list + delete a device, retry); `password` now
> optional on `POST`/`DELETE /clients`. `14bb698` (#11). Archived from PLAN.md.
>
> **Sub-steps:** none.
>
> **Branch / commit prefix:** `step/24-*`, `feat(step-24): …`.

---

## Step 24 — Two-phase login

Login is split into two phases so a consumer can recover from Wire's per-user
device cap **without re-authenticating**:

- **Phase 1 — `login`**: version negotiation, `POST /login` + `GET /self`, and
  per-account storage setup. Returns a `PendingLogin` that already holds a
  valid session. **Authentication** errors (wrong password, missing/invalid
  2nd-factor code) surface here.
- **Phase 2 — `PendingLogin::register_device`**: `POST /clients`, MLS identity
  (key packages), backend removal-key fetch, finalize → a ready `Client`. The
  **device cap** (`400 too-many-clients` → `KupferdrahtError::TooManyDevices`)
  surfaces here.

Because the session is captured in phase 1, the recovery loop runs entirely on
the already-authenticated handle — no second login.

### Why

The POC statement of work (§4) requires recovery for the device limit. Wire
allows at most 7 devices per user; registering an 8th fails with
`400 too-many-clients`. natrium-ion exposes a recovery flow (list devices,
remove one, retry). A one-shot `login` that returns a `Client` can only throw
on the cap, leaving the caller with no authenticated handle to recover on. The
two-phase split keeps the authenticated session in hand across the retry.

The shape is also **SSO-ready**: the recovery surface
(`register_device` / `devices` / `delete_device`) operates on the captured
session, independent of *how* the session was obtained. A future SSO login
(Step 17) produces the same `PendingLogin`, and the recovery works unchanged.

### matrix-rust-sdk analog

None directly — Matrix has no per-user device cap. The two-phase shape is a
Wire-driven extension; `PendingLogin` mirrors the existing `PendingClient` /
`PendingAccount` intermediate-handle pattern.

### Public API

```rust
impl ClientBuilder {
    // Phase 1 — was `-> Arc<Client>`.
    pub async fn login(self, email: String, password: String,
        second_factor_code: Option<String>) -> Result<Arc<PendingLogin>, KupferdrahtError>;
}

impl Kupferdraht {
    // Phase 1 + registry binding on success (inside register_device).
    pub async fn login(&self, server: ServerConfig, email: String, password: String,
        second_factor_code: Option<String>) -> Result<Arc<PendingLogin>, KupferdrahtError>;
}

impl PendingLogin {
    pub fn user_id(&self) -> &str;
    pub fn email(&self) -> &str;
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError>;
    pub async fn delete_device(&self, device_id: &str) -> Result<(), KupferdrahtError>;
    // Phase 2 → ready Client. Retryable on TooManyDevices; single-use on success.
    pub async fn register_device(&self) -> Result<Arc<Client>, KupferdrahtError>;
}
```

`login` returns `PendingLogin` (one path — the prior one-shot `login -> Client`
is gone). The manager-level `PendingLogin` additionally writes the device
registry row and persists the session inside `register_device` (the writes
`Kupferdraht::login` used to make inline), so a registered account is immediately
listable via `Kupferdraht::accounts` and resumable via `Kupferdraht::resume`.

### Mechanism

`PendingLogin` holds the captured phase-1 state. The consumable bits (the login
pipeline, `LoginOutcome`, `SelfUser`) live behind a `Mutex<Option<…>>`;
`register_device` takes them, runs phase 2, and on failure **restores** them so
the caller can retry — on success they are consumed (a second call returns
`InvalidInput`). `devices` / `delete_device` use a dedicated `HttpClient` and
the user-scoped login token, so they need no lock.

### Credential model (SSO-ready)

The device-registration / deletion password is `Option`. Wire's
`NewClient.password` is required only for the 2nd, 3rd, … client of a password
account (not the first, and not for password-less accounts, which verify via
`verification_code`); `DeleteClient.password` is likewise optional. The
password is threaded as `Option<&str>` end-to-end and the JSON field is omitted
when `None`. Today's password login always passes `Some`; a future SSO login
sets `None` (and, in Step 17, the sibling `verification_code`) — same shape, no
reshape.

### FFI

`ffi/manager.rs`: `Kupferdraht::login` returns the `PendingLogin` `uniffi::Object`
(`register_device` → `Client`, `devices`, `delete_device`, `user_id`, `email`).
`KupferdrahtError::TooManyDevices` rides the existing `uniffi::Error` derive.

### CLI

`kupferdraht-cli login … [--delete-device <id>]`: phase 1 (`login`) then phase 2
(`register_device`). On `TooManyDevices`, without the flag it prints the
registered device list; with it, deletes that device and retries
`register_device` — one invocation, no second login. The live-validation path
(reproducible only with 7 registered devices).

### Dependencies

- Step 1b/1d (client registration, `devices` / `delete_device`).
- Step 16 (`Kupferdraht` manager, `PendingAccount` / `PendingClient` handle pattern).

### Tests

- `login_pipeline`: pure `map_too_many_clients` (label → `TooManyDevices`).
- `wire/http/clients`: `register_client` / `delete_client` omit the password
  field when `None`.
- Happy-path login + `register_device` across the integration suite (re-chained
  from the prior one-shot form).
- Phase separation: authentication failures (invalid credentials, 2FA-required)
  surface from `login`; device-registration / key-package / public-keys
  failures surface from `register_device`.
- `register_device` is single-use after a successful registration.
- The `TooManyDevices` recovery loop needs live auth → covered by the CLI live
  path, like the rest of the login pipeline.

### Verification

- Full gate: `cargo fmt --check`, build 0 warnings, `clippy -D warnings`,
  `cargo test --workspace`, `cargo deny`.
- Binding smoke: Kotlin + Swift see `login -> PendingLogin`,
  `PendingLogin.registerDevice` / `devices` / `deleteDevice`, `TooManyDevices`.
- Live (7 devices): `login` → `register_device` → `too-many-clients` → device
  list → `--delete-device <id>` → retry → `Client`.
