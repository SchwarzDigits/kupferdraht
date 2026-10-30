# Step 22 — Update email

> **Status:** merged on `step/22-update-email`. Closes the
> `updateEmail` FFI gap surfaced by the natrium-ion migration analysis.
> Verification-flow email change (`PUT /access/self/email`, `202`/`204`);
> local profile unchanged until confirmed. `a923ff4` (#9). Archived from PLAN.md.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 22" — current.
>
> **Branch / commit prefix:** `step/22-*`, `feat(step-22): …`.

---

## Step 22 — Update email

Request a change of the signed-in user's email. Mirrors the existing
self-profile mutators (`set_display_name`, `set_handle`), closing the
last profile-edit gap for the natrium drop-in (`Session.updateEmail`).

### Why

The natrium-ion migration analysis flagged `updateEmail` as a small
remaining gap: Kupferdraht exposed `set_display_name` and `set_handle` but no
email mutator. This adds it so natrium's `Session.updateEmail` maps 1:1
onto Kupferdraht.

### matrix-rust-sdk analog

`Account` 3PID/email management. Drift: matrix models email as a
third-party-identifier add/bind flow; Wire has a single
`PUT /access/self/email` endpoint with a server-side verification mail.

### Verification-flow semantics

Wire's email change is **not immediate**. `PUT /access/self/email` sends
a confirmation mail to the new address and returns `202 Accepted`; the
address only changes once the user clicks the link. Kupferdraht therefore
issues the request and returns on the `202`/`204` acknowledgement
**without** updating the local `Account` snapshot — `Client::account()`
still reports the old address until the user verifies and a later
`sync_metadata` refetches `/self`.

### Public API

```rust
impl Client {
    /// Request an email change. Returns once Wire has accepted the
    /// request (a verification mail is sent); does not change the local
    /// profile.
    pub async fn update_email(&self, email: &str) -> Result<(), KupferdrahtError>;
}
```

FFI: `ffi/admin.rs` `Client::update_email(email)` delegates to the core.

### Wire API v14 endpoints

- `PUT /v14/access/self/email` — body `{"email": "<addr>"}`
  (`EmailUpdate`). `202` (verification mail sent) or `204` → success.
  `400 invalid-email` and `409 key-exists` (address already in use)
  surface as `KupferdrahtError::BackendRejected` with the label in the body.

### RFC 9420 sections

None — account email management is Wire-transport-only, never touches
MLS.

### Wire Message Types

None — not a `GenericMessage`.

### Error mapping

Mirrors `set_handle`: non-2xx → `BackendRejected(body)`. The body carries
Wire's label (`invalid-email`, `key-exists`), which a consumer (e.g.
natrium's `UpdateEmailResult`) branches on. No typed `KupferdrahtError`
variant was added.

### Dependencies

- Step 2g (self-profile mutators `set_display_name` / `set_handle`) —
  same `wire/http/auth.rs` + `Client` pattern.

### Verification

- `wire/http/auth.rs` httpmock: body + path, `202`/`204` ok, `400`/`409`
  → `BackendRejected`.
- Full gate + Kotlin/Swift binding smoke (`update_email` present).
- Live (optional): confirm the `/v{N}/access/self/email` path versioning
  against a real backend.
