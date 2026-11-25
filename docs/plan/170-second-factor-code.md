# Step 170 — The second-factor code: request it, ask for it, send it

**Status: commissioned 2026-11-25** (Volker): Kupferdraht supports the team feature
`sndFactorPasswordChallenge` fully. After Step 169.

## Problem

A login may be refused with `403 code-authentication-required` when the team requires
a second factor; so may `POST /clients`. Kupferdraht accepts a code at login
(`verification_code` in the login body, `KupferdrahtError::SecondFactorRequired` /
`InvalidSecondFactor`, `wire/http/auth.rs`) but can neither request one nor send one
with the device registration. The backend sends the code only on `POST
/verification-code/send` — Wire's flow, which brig-rs implements too.

## Scope

1. **Requesting.** On `code-authentication-required` from the login or from `POST
   /clients`, `POST /v{N}/verification-code/send` with `{"action":"login","email":…}`.
   Other actions (`create_scim_token`, `delete_team`) only if Kupferdraht ever offers
   them — it does not today.
2. **Asking.** The CLI reads the code interactively, or from `--verification-code` for
   scripts; the FFI surfaces it as an error the app answers by calling again with the
   code (`PendingLogin` already carries a second step for the pending login; the
   device registration gets the same shape). Phase 1 decides error type against
   callback and records it (an ADR if it changes the login's public shape).
3. **Sending.** The login and `POST /clients` repeat with `verification_code`; one code
   may serve both while the server accepts it. Phase 1 reads Wire's and brig-rs's rules
   (validity, single use) and records them in `docs/wire/api-notes.md`.
4. **Errors told apart:** `code-authentication-failed` (wrong or expired; offer a new
   request), too many attempts, the rate limit of `verification-code/send` (name the
   wait). Neither the password nor the code is ever logged.
5. **SSO users** get no code (Wire exempts them): a pin that the SSO login path never
   asks.
6. **Docs:** `docs/backend-spec.md` P-9 (Step 166's target paragraph becomes the
   present tense), `docs/library.md`, `docs/bindings.md`, `docs/cli.md`,
   `docs/api-mapping-matrix-rust-sdk.md` (matrix-rust-sdk's UIAA as the analog),
   `CHANGELOG.md`.

## Out of scope

- Enabling the feature for a team from Kupferdraht; the SCIM-token and team-delete
  actions.

## TDD sketch

- httpmock: `403 code-authentication-required` → one `POST /verification-code/send`
  with the login's email → the login repeats with the code and succeeds; the same for
  `POST /clients`; `code-authentication-failed` surfaces as `InvalidSecondFactor` with
  the offer to request anew; a `429` on `send` names the wait — red;
- the SSO path sends no `verification-code/send` — red;
- no log line contains the password or the code (a pin on the login's tracing) — red.

Smoke on wire-dev with a throwaway team: the feature unlocked and enabled through the
admin tool (or galley's internal endpoint), the code read from Inbucket's API
(`localhost:9000`), the team deleted and the mailboxes purged afterwards
(`tests/stack/purge-mailboxes.sh`); a separate, switchable check in
`kupferdraht-smoke.sh` or its own script under `tests/stack/`. Stack time is booked
with the coordinator.
