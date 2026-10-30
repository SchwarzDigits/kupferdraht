# Step 110 — An account's keys from its seed; the login waits for the seed

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (Markus —
ADR-0035, PR #80 merged as `01c1491`). `a9de8d5`. Merged to `main` as PR
#85 (`800e381`); CI green. Smoke 2026-09-30 (schwarz-dev-wire, release
wasm, headless Chrome). Retrospective: `docs/LEARNINGS.md` §5.49. Archived
with this header.

## Problem

The browser draws its database keys at random and keeps them in IndexedDB;
an eviction loses them with the databases. ADR-0035 keeps the keys stored
exactly as natively but lets a new key of an account come from that
account's seed, and a login open nothing of an account whose seed this
browser does not hold. Everything else stays the native code.

## Decisions (2026-09-29/30, Markus — ADR-0035)

- **D1 — the one place a key is made.** `load_or_create_key_locked` →
  `adopt_or_create_key` is where every new database key is created. A home
  carries an account key source: `Random` (native, the default) or `Seed`
  (the browser). With `Seed`, a key of an account is HKDF-SHA-256 of the
  account's seed (`kupferdraht/v1/db-key/kupferdraht`, `kupferdraht/v1/db-key/mls`), stored
  under its native id; without the seed the creation refuses with the new
  `KupferdrahtError::RecoveryDataRequired` and writes nothing. Device-level keys
  (`global.db`) stay random. A derived key is deterministic, so a missing key
  of an existing database is derived again instead of refused (ADR-0029
  applies only without the seed). Loading is untouched.
- **D2 — the seed** is a keystore entry `kupferdraht.<home-id>.account-<account>-seed`
  (32 random bytes), written durably before any database of the account.
- **D3 — the login in two stages.** `ClientBuilder` authenticates (HTTP
  only), then assembles (keys, stores). A `Random` home assembles at once —
  native behaviour unchanged. A `Seed` home assembles only when the account's
  seed is there; otherwise the pending login waits: `register_device` refuses
  with `RecoveryDataRequired` unless asked for a first setup, which removes
  keys and databases the account has without a seed (builds before
  ADR-0035), creates the seed, assembles and registers. A retry after
  `TooManyDevices` keeps the seed.
- **D4 — the native device reuse stays the only device logic** (Step 40):
  recorded device, or a new one replacing it (`wipe_superseded_state`), or a
  first registration. The client remembers whether its device replaced the
  recorded one (`device_replaced`).
- **D5 — `cancel`** of a pending login drops its state and removes what a
  first setup of it created: seed, keys, database files.
- **D6 — registry.** With a `Seed` home `accounts()` lists only accounts whose
  seed is here; `resume` of an account without its seed refuses with
  `RecoveryDataRequired`.
- **D7 — `recovery_expected`**: while the seed is missing, the pending login
  asks the account's device list whether a device with the model
  `Kupferdraht (Web)` is registered. The browser registers with that model (110b).
- **D8 — out of Step 110:** `logout({ purgeLocal })` is the browser face of
  the local account purge that Step 78 plans for native; it comes with Step 78
  instead of a second deletion path here. `purgeDevice` + `password` exist
  natively already and are only bound in 110b.

## Core (native tests)

The browser behaviour lives in the shared core behind the `Seed` source, so
native tests drive it with an in-memory keystore and `httpmock`. The seed-home
switch and the new pending-login methods are `pub(crate)`: the UniFFI and
Rust APIs do not change; only `KupferdrahtError` gains `RecoveryDataRequired`
(additive, BREAKING for exhaustive matches).

### TDD (red first)

`storage/account_keys.rs` (unit, known-answer vectors computed independently):
- the `kupferdraht.db` and `mls.db` keys of seed `00 01 … 1f`;
- the seed id is home-scoped.

`storage/seed_keys_tests.rs` (unit, no HTTP):
- a seed home derives a new account key and stores it under the native id;
- the `global.db` key stays random in a seed home;
- without the seed, an account key is refused with `RecoveryDataRequired`
  and nothing is written;
- an existing database whose key is missing gets the derived key back;
- a stored key is loaded as is, not derived;
- a random home is unchanged.

`manager/seed_login_tests.rs` (unit, `httpmock`, the shared login mocks):
- a login without the account's seed opens and persists nothing;
- `register_device` without the seed refuses with `RecoveryDataRequired`, and
  the pending login stays usable;
- a first setup creates the seed, derives the keys and registers;
- a first setup removes keys and databases left without a seed;
- a later login with the seed present takes the native path and reuses the
  device;
- a retry after `TooManyDevices` keeps the seed;
- `cancel` after a failed first setup leaves nothing, and a new first setup
  works;
- `accounts()` lists only accounts with a seed;
- `resume` without the seed refuses with `RecoveryDataRequired`;
- `recovery_expected` follows the `Kupferdraht (Web)` devices;
- a device that replaced the recorded one is reported.

The login mocks move from `tests/common/mod.rs` into
`tests/common/login_mocks.rs` (httpmock only), shared by the integration
tests and, through `#[path]`, by these unit tests.

## Browser binding (red tests approved separately)

- `wasm::Kupferdraht::open` opens a `Seed` home; `PendingLogin.keys`,
  `recoveryExpected`, `cancel()`, `registerDevice({ newSetup })`;
  `Client.deviceReplaced`; `Client.logout({ purgeDevice, password })`.
- Every thrown error's `name` is its `KupferdrahtError` variant
  (`KupferdrahtError::variant_name`). `wasm_bindgen::JsError` cannot carry a
  name, so the entry points return `Result<_, JsValue>` with a named JS
  `Error` — the same thrown `Error` for JavaScript.
- The browser registers its device with the model `Kupferdraht (Web)`.
- The seed is made durable by the Step 104 flush before any database.
- `cancel` is synchronous: keystore deletions are not tracked writes, there
  is nothing to await (the tab proxy still returns a promise).
- `kupferdraht-tabs` mirrors every member; `bindings/web/opfs-smoke/` passes
  `newSetup: true`; `docs/bindings.md` describes the flow; browser smoke.

### TDD (red first)

- `error.rs` (unit): every variant is named as `Debug` spells it; the
  browser outcomes by name.
- `tests/step_110_browser_seed_surface.rs` (native pins): `Kupferdraht.open`
  opens a seed home; the pending login and client members; named errors;
  the smoke page asks for a first setup; `docs/bindings.md` names the flow.
- `tests/step_94_tab_proxy_parity.rs` (existing): the tab module already
  mirrors the new members — red until the binding exports them.
- `tests/step_110_browser_error_names.rs` (browser): a thrown error's `name`
  is its variant (`InvalidInput` for an unknown account).

## As built (2026-09-30)

- Core: `storage/account_keys.rs` (source, HKDF, seed, forgetting),
  `KeyId::owner()` (device or account key), the one creation point
  `load_or_create_key_locked` derives an account key in a `Seed` home;
  `durable_keys::create_seed_durably`; `storage/account_files.rs`
  (removing an account's databases). `ClientBuilder` carries the source
  into its home; `assemble_pending_login` waits without the seed
  (`awaits_seed`), `open_account_storage` is the shared opening.
  `establish_device_identity` returns `DeviceIdentity { replaced }`;
  `Client::device_replaced`. The browser side of the pending login lives in
  `manager/pending_login/first_setup.rs`; `Kupferdraht::accounts` / `resume`
  honour the seed.
- Browser: `wasm::Kupferdraht::open` opens a `Seed` home; `PendingLogin.keys`,
  `recoveryExpected`, `registerDevice({ newSetup })`, `cancel()`;
  `Client.deviceReplaced`, `logout({ purgeDevice, password })`; every
  entry point returns `Result<_, JsValue>` with a JS `Error` named after
  `KupferdrahtError::variant_name`. Browsers register as `Kupferdraht (Web)`.
  `kupferdraht-tabs` mirrors the members; `opfs-smoke` sets up with
  `newSetup: true` and gained a scripted login (`?scenario=login`).
- Gate 2026-09-30: fmt, 0 warnings, clippy (native all targets, wasm lib),
  1069 native tests, the browser suites (incl. the new error-name test),
  `cargo deny` — green.
- Smoke 2026-09-30 (schwarz-dev-wire, release wasm, headless Chrome, a
  local same-origin proxy, user 2): (1) fresh browser — `keys` missing,
  `recoveryExpected` false, first setup registered a `Kupferdraht (Web)`
  device, synced, joined the 1:1 by external commit, sent; (2) same
  browser — `keys` present, the device reused (`GET /clients/{id}`, no
  `POST /clients`); (3) second fresh browser — first setup registered a
  second device, its first send got a backend `500` (observations §92),
  resent fine on its next login; (4) third fresh browser —
  `recoveryExpected` **true**; (5) browser (3) after its device was deleted
  at the backend — replaced, `deviceReplaced` true, rejoined and sent.
  User 1 received all five sends. The smoke devices were deleted again
  (user 2 back at 5/7). No CLI command: the feature is browser-only.
- Retrospective: `docs/LEARNINGS.md` §5.49.

