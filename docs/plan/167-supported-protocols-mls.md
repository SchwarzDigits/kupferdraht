# Step 167 — The account says it speaks MLS: `supported_protocols = ["mls"]`

**Status: commissioned 2026-11-25** (Volker). Small; after Step 166. The principle it
enforces (Volker, 2026-11-25): Kupferdraht speaks MLS only, never Proteus, not even in
part; it talks to clients and rooms that speak MLS.

## Problem

brig keeps, per user, the protocols the user's clients support (`supported_protocols`).
A new account starts with `["proteus"]` (Wire's default, brig-rs the same), and
Kupferdraht never changes it: the backend claims of every Kupferdraht user that it
speaks Proteus only — the opposite of the truth. Wire's clients pick a 1:1's protocol
from the intersection of both users' lists; webapp `{proteus, mls}` ∩ Kupferdraht
`{proteus}` = `{proteus}`, so the webapp wants the 1:1 over Proteus and shows the MLS
1:1 with a Kupferdraht user read-only ("different protocols", observations §94).

## Scope

1. **After a login, before any room is created or joined**, `PUT
   /v14/self/supported-protocols` with `{"supported_protocols":["mls"]}` — only when
   `GET /self` does not already answer exactly `["mls"]` (idempotent; no call on
   every start). A failure is logged and does not fail the login. Never `proteus` in
   the list, not transitionally. Existing accounts with `["proteus"]` are corrected at
   their next login.
2. **The `wire/` layer:** `supported_protocols` read from `/self` (today deliberately
   ignored, `wire/dto.rs`), the `PUT` in `wire/http/`; the decision in
   `client/login_pipeline.rs` beside the other post-login steps.
3. **No new public call.** The CLI's `login` and `status` show the account's
   protocols; `kupferdraht-smoke.sh` (in the Rust server's repository) gains the
   check `GET /users/{id}/supported-protocols` → `["mls"]`.
4. **Docs:** `docs/backend-spec.md` (the Step 166 paragraph becomes the present
   tense), `docs/non-goals.md` NG-6 (why the list says MLS alone), `docs/wire/
   api-notes.md` (the route and the webapp's protocol choice), `docs/cli.md`,
   `CHANGELOG.md`. No ADR: the MLS-only decision is ADR-0010 and NG-6.

## Out of scope

- Proteus in any form; a user who speaks Proteus only gets no 1:1 with a Kupferdraht
  user, which is the expected outcome and is documented as such.
- Changing the team's `mls` feature or its `supportedProtocols`.

## TDD sketch

- httpmock: a login whose `/self` says `["proteus"]` sends the `PUT` exactly once with
  `["mls"]`; one whose `/self` says `["mls"]` sends none; a failing `PUT` (500) logs
  and the login still succeeds — red;
- the request body never contains `proteus` (a pin on the builder) — red.

Smoke on wire-dev (the Rust stack) and on server 4: `GET /users/{id}/supported-protocols`
answers `["mls"]` after a login; in the webapp a 1:1 with a Kupferdraht user is
writable in both directions.
