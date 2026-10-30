# Step 27 — Change password

> **Status:** committed on `step/27-change-password`. Closes the gap
> between `docs/backend-spec.md` §5.7 (which freezes `PUT /self/password`
> as part of the backend contract) and the Kupferdraht implementation.
> HTTP + `Client` + FFI + CLI landed; httpmock-tested, full gate green.
> `HttpClient::update_self_password` + `Client::change_password(old, new)` +
> FFI + `kupferdraht-cli change-password` (TTY prompts). All `zuid` cookies
> invalidated on success; in-memory session untouched. Single-account live
> smoke (change pw → re-login → change back) is optional and destructive —
> not yet run. Archived from PLAN.md.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 24" — original draft number, collided with the merged Step 24
>   (two-phase login / device-limit recovery); renumbered to 27 on
>   2026-06-14 (next free after the merged Steps 25–26).
> - "Step 27" — current.
>
> **Branch / commit prefix:** `step/27-*`, `feat(step-27): …`.

---

## Step 27 — Change password

Change the signed-in user's password. Completes the self-credential
mutators (`set_display_name`, `set_handle`, `update_email`) and is the
client half of the initial-password flow specified in
`docs/backend-spec.md` §6.2: a team admin sets an initial password,
the user logs in and replaces it with this call.

### Why

`docs/backend-spec.md` §5.7 (resolving its Open Question Q4) specifies
`PUT /v14/self/password` as part of the backend contract Kupferdraht runs
against. Without a client-side caller, spec and implementation drift:
the endpoint would be specified, served, and never exercised by the
only intended client. This step adds the caller so the contract is
covered end-to-end (library + CLI + tests).

### matrix-rust-sdk analog

`Account::change_password(new_password, auth_data)`. Drift: matrix
re-authenticates via the interactive-auth `auth_data` parameter; Wire
carries the re-authentication as `old_password` in the request body.
Kupferdraht follows Wire's shape with matrix-rust-sdk naming:
`change_password(old_password, new_password)`.

### Session semantics

Per the backend contract (Wire-observed; backend-spec §5.7), a
successful password change **invalidates all `zuid` cookies** of the
user — every device's persisted session dies; access tokens MAY remain
valid until their ~15-min expiry. Kupferdraht therefore:

- returns success on `200` without touching the in-memory session
  (the current access token keeps working until expiry);
- does NOT attempt to re-login or refresh the `zuid` — the next
  `ClientBuilder::resume` past token expiry will fail its `/access`
  rebind and surface the normal re-login path (CHANGELOG.md §51/§58
  machinery, no new code);
- documents in the CLI output that other devices will need to log in
  again.

### Public API

```rust
impl Client {
    /// Change the account password. Wire invalidates every device's
    /// refresh cookie on success — all sessions (including this one,
    /// once the current access token expires) must re-login.
    pub async fn change_password(
        &self,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), KupferdrahtError>;
}
```

FFI: `ffi/admin.rs` `Client::change_password(old, new)` delegates to
the core, alongside `update_email`.

CLI: `kupferdraht change-password` — prompts for old and new password
(no passwords as bare CLI args; reuse the login prompt machinery),
prints the all-devices-must-relogin notice.

### Wire API v14 endpoints

- `PUT /v14/self/password` — body
  `{"old_password": "<old>", "new_password": "<new>"}`
  (`PasswordChange`, both fields 6–1024 chars in Wire's schema).
  `200` → success. `403 invalid-credentials` (old password wrong) and
  `400` (policy violation) surface as `KupferdrahtError::BackendRejected`
  with the label in the body.

### RFC 9420 sections

None — password management is Wire-transport-only, never touches MLS.
Registered devices, key packages, and group state all survive a
password change.

### Wire Message Types

None — not a `GenericMessage`.

### Error mapping

Mirrors `update_email`: non-2xx → `BackendRejected(body)`; consumers
branch on the label (`invalid-credentials`). No typed `KupferdrahtError`
variant.

### Dependencies

- Step 2g / Step 22 (self-profile mutators) — same
  `wire/http/auth.rs` + `Client` + CLI pattern.

### Verification

- `wire/http/auth.rs` httpmock: path + body shape, `200` ok,
  `403 invalid-credentials` / `400` → `BackendRejected`.
- Full gate + Kotlin/Swift binding smoke (`change_password` present).
- Live (optional, destructive on the test account's credential):
  change password, confirm old `zuid` rebind fails (`403`), re-login
  with the new password, change back.
