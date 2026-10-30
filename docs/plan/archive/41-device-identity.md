# Step 41 — Device identity: per-install cookie label + real device names

> **Status:** MERGED to `main` — content landed inside PR #35 (`a16fe70`,
> squash-merge, no separate merge commit), 2026-06-24, bundled with Step 42.
> This step's own branch commit (`12c29ce`) was never merged directly — it
> is unreachable from `main`; only its content, folded into PR #35, shipped.
> Scope: fixes the post-Step-39 bug where a fresh login's session died after
> ~15–30 min whenever a second device shared the account (per-install cookie
> label) + platform-aware device class/model/label + a real device name from
> natrium via FFI. Cross-repo root-cause analysis: natrium's
> `natrium-core/docs/token-refresh-after-login.md` (§0). No dated live-smoke
> record in this file — its own Verification section lists a live two-device
> check as a to-do ("log two devices into one account; the first device's
> session survives the second's login"), not a logged result.
>
> **Branch / commit prefix:** `step/41-*`, `fix(step-41): …`.

---

## The bug (Part A — the fix)

Wire ties the persistent refresh `zuid` to `(user, cookie_label)`. The label was
**not** unique per device:

- `POST /login` sent a constant `DEFAULT_COOKIE_LABEL = "kupferdraht-cli"`.
- `register_client` sent a per-**user** label (`"kupferdraht-{user_id}"`).

So two devices of one account collided on the same `(user, label)` slot: device 2's
login replaced device 1's cookie, and device 1's next refresh (`rebind`) got
`403 invalid-credentials` → `SessionExpired` → `LoggedOut`. The ~15-min delay was
just the access-token TTL — device 1 only noticed at its first refresh.

**Fix:** a **per-install** unique cookie label, generated once and persisted in the
keystore, reused for every login of every account on the install — so each device
owns its own Wire cookie slot and the two `zuid`s coexist. The `device_id` can't
serve as the label: Wire only assigns it during `register_client`, *after* login.

- `client/builder.rs::install_cookie_label(keystore)` — reads `kupferdraht.install-id`
  from the keystore; on miss, generates `uuid::Uuid::new_v4().simple()`, persists
  it, and returns `"kupferdraht-{id}"`. Device-wide (not per account).
- `ClientBuilder::login` computes it and threads `cookie_label` into
  `LoginPipeline::start`; the pipeline passes it to **both** `http.login`
  (login body) and `http.register_client`.
- Removed the constant `DEFAULT_COOKIE_LABEL` and the per-user `cookie_label(uid)`.

## Part B — platform-aware device class / model / label

Devices were indistinguishable in the user's device list (identical class/model/
label), which made the device-cap UX painful. `wire/http/clients.rs` now derives
these at compile time via `cfg!`:

- `device_class()` → `"phone"` on iOS/Android, else `"desktop"`.
- `device_model()` → `"Kupferdraht (iOS)"` / `"Kupferdraht (Android)"` / `"Kupferdraht (macOS)"`
  / `"Kupferdraht"`.
- `build_register_client_body` uses `class = device_class()`, `model =
  device_model()`, and `label = device_label.unwrap_or(device_model())`.

## Part C/D — real device name from natrium via FFI

The human-readable device name is supplied by the host app and flows through:

- Kupferdraht: `Kupferdraht::login` gained `device_label: Option<String>`, forwarded to
  `ClientBuilder::device_label()`. FFI `Kupferdraht::login` exposes the same param
  (UniFFI binding regenerated).
- natrium: `NatriumPlatform.deviceName` (expect + actuals) — iOS
  `UIDevice.currentDevice.name`, Android `Build.MODEL`, JVM `null` (falls back to
  the platform `device_model()`). `Natrium.login` passes `platform.deviceName`
  through `KupferdrahtEngine.login` → `Kupferdraht.login`.

When `device_label` is `None`, registration falls back to the platform
`device_model()` (Part B), so the CLI / JVM still get a sensible label.

## TDD / verification

- `client/builder.rs` tests:
  `install_cookie_label_is_stable_for_one_install` (same keystore → same label,
  `kupferdraht-` prefix) and `install_cookie_label_is_unique_per_install` (two
  keystores → distinct labels — the (user, label) collision that caused the bug).
- `wire/http/clients.rs` tests: `build_register_client_body_has_wire_required_fields`
  (cookie + label + class==`device_class()` + model==`device_model()`), and
  `build_register_client_body_label_falls_back_to_platform_model` (None → label ==
  `device_model()`).
- `wire/http/auth.rs`: `build_login_body_carries_cookie_label`.
- CI gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace --all-targets` (all green; 368 lib +
  integration tests).
- natrium: `buildKupferdrahtNatives` (binding regen) then compile jvm / iosArm64 /
  android — all green.
- Live: log two devices into one account; the first device's session survives the
  second's login (no `403` at the next refresh), and each device shows its own
  name in the device list.
