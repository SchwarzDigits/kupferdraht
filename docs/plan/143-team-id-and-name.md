# Step 143 — The own team on the account: id and name

**Status: commissioned 2026-11-05** (from the browser embedder's
requirements for a work-queue client, R1, measured there against
`4f30f02` and re-checked against `main` `9e1a11c`). Taken with Volker on
commissioning: the names `team_id` / `team_name` (`teamId` / `teamName`
in the browser), as of the last login with no live refresh, and after
the v0.2.1 release (141, 142, 130b) — the FFI record `Account` grows by
two fields.

## Problem

1. **The client cannot show the user's team.** The embedder's client
   names the team at the top of its navigation and will later scope
   team-wide features by its id. `GET /self` is read at login into
   `wire::dto::SelfUser`, whose `team` (the team UUID, absent for a
   personal account) serves only as `delete_room`'s fallback for a
   conversation without an owning team (Step 23, `client/rooms.rs`, by a
   second `/self` fetch). It is not on `Account`, not in `self_user`,
   not on the FFI and not in the browser.
2. **The team's name is not known at all.** Nothing calls
   `GET /v14/teams/{tid}` (only `DELETE /teams/{tid}/conversations/{cid}`
   exists). The Swagger's `Team` carries `id`, `creator`, `name` and
   `icon` as required fields; `404 no-team` is its documented failure.

## Scope

1. **Wire layer.** `HttpClient::get_team(api_version, token, tid)` →
   `GET /v14/teams/{tid}`, the path segment through `path_segment`
   (Step 137), a `wire::dto::Team` that reads `id` and `name` only.
   `SelfUser::team` stays as it is.
2. **Login.** After `fetch_self`, in `authenticate` and `finalize_sso`
   alike (`client/login_pipeline.rs`), a team member's team is fetched
   once. Any failure — `403`, `404`, `5xx`, transport — is a `warn!`
   and `team_name = None`; the login goes on (the ADR-0012 posture of
   the feature-config fetch). The id is stored lowercase; an empty
   `team` is no team (as `delete_room` treats it, Step 95c). A personal
   account makes no request.
3. **`Account` and its persistence.** `Account::team_id() ->
   Option<&str>` and `Account::team_name() -> Option<&str>`, a snapshot
   like `display_name` and `handle`. Migration V33 (the next free number
   when the step starts) adds the nullable columns `team_id` and
   `team_name` to `self_user`; `SelfUserRecord` carries both, and every
   writer keeps them: the manager's login
   persistence (`manager/mod.rs`), the CLI's (`cli/auth.rs`
   `persist_session_state`) and the profile update (`client/profile.rs`,
   which rewrites the row for `set_display_name` and its siblings and
   must not drop the team). `build_resume_account` reads them back, so
   a resumed client has the values of its last login.
4. **Public surface, in the same commit:**
   - FFI record `Account` (`ffi/mod.rs`): `team_id` and `team_name`,
     both `Option<String>`.
   - wasm `Client` (`wasm/manager.rs`): getters `teamId` and `teamName`,
     `string | undefined`.
   - `kupferdraht-tabs`: `["Client", "teamId"]` and
     `["Client", "teamName"]` in `protocol.js` `GETTERS`, both on
     `ClientProxy` in `index.d.ts`. Getters are snapshotted into the
     handle, so `METHODS` and `proxy.js` do not change.
5. **CLI.** `login`, `activate` and `sso finalize` print a `team:` line
   (name and id, `—` without a team); `status` prints the persisted
   values, which exercises the resume path.
6. **Docs:** `docs/library.md` (`Account`), `docs/bindings.md` (the FFI
   record, the browser getters, the tabs getters), `docs/cli.md`,
   `docs/wire/api-notes.md` (`GET /teams/{tid}` with the live
   observation of phase 1), `docs/api-mapping-matrix-rust-sdk.md` (the
   two getters as "Wire extension": matrix-rust-sdk has no team; the
   precedent is `handle`), `docs/ARCHITECTURE.md` §7 (V33), `README.md`
   (one line in "What it does"), `CHANGELOG.md` `[Unreleased]` under
   Added.

## Out of scope

- **A live refresh.** Wire sends `team.update`, but the embedder lives
  with "as of the login". Consequence, written into `docs/library.md`
  and `docs/bindings.md`: when `GET /teams/{tid}` fails at login,
  `team_name` stays `None` until the next full login — a resume is
  offline and reads the stored row, and a browser session resumes for
  weeks. A fetch at the first sync cycle when the name is missing would
  be a step of its own.
- The team's icon, splash screen and members; team-scoped features.
- `delete_room`'s own `/self` fetch: it could read `Account::team_id`,
  but the snapshot can be older than the session; it stays.
- The device's account registry in `global.db` (`AccountEntry`): no
  team there.

## Phase-1 questions

1. **Can a plain member read the team?** The Swagger names only
   `200` and `404`. Live probe on the wire-server-rs stack with the
   smoke's throwaway team: `GET /v14/teams/{tid}` as its owner and as
   its member, and as a personal account against a foreign team id;
   record the answers in `docs/wire/api-notes.md`. If a member is
   refused, the name stays `None` for members and the plan says so
   before the red tests.

## TDD sketch

- httpmock, login: a team member gets both values; the `GET` carries
  the bearer token; a personal account makes no `/teams` request and
  gets `None` for both; `/teams` answering `500`, `404` or not at all
  → the login succeeds with `team_id` set and `team_name` `None`; an
  uppercase id from `/self` is stored lowercase; an empty `team` is no
  team.
- Storage: V33 applies over a V32 store (existing rows read `NULL`);
  resume returns both values of the last login; `set_display_name`
  keeps them. `migration_immutability` pins V33 like the others.
- FFI: the record carries both fields (the existing `ffi/mod.rs` unit
  test of the `Account` conversion).
- Tabs: `step_94_tab_proxy_parity` / `step_125_client_handle` see the
  two getters in `GETTERS` and `index.d.ts`.
- Gate: the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown` (the wasm getters).
- Smoke: `kupferdraht-smoke.sh --throwaway`, 18/18, plus the CLI's
  `login` / `status` lines for the owner and the member of the
  throwaway team, compared with the team settings.
