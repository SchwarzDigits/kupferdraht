# Step 152 — `refresh_account`: the own account and team, read on request

**Status: MERGED 2026-11-09** — `033729a` on `step/152-refresh-account`, merged to
`main` as PR #67 (`a2ff73d`); gate green on 1.98 (fmt, 0 build warnings, clippy, 1619
tests, deny; wasm32 check, clippy `--lib` and CI's wasm clippy line); the browser suite
green locally (18 suites, 82 tests, headless Firefox) and in CI; the Kotlin bindings
built and tested. Smoked 2026-11-09 on the wire-server-rs stack with the CLI built from
this tree: `kupferdraht-smoke.sh --throwaway` 24/24; two devices of one account, the
second changing name and handle, the first showing them only after `account-refresh`;
a plain team member reading a team renamed in galley's table (wire-server-rs has no
`PUT /teams/{tid}`), kept by a later command. Scope: `Client::refresh_account` (FFI,
browser `refreshAccount()`, CLI `account-refresh`) reads `GET /self` and
`GET /teams/{tid}` into the row and the live account, all or nothing; the own profile
calls update the account; `Account::avatar_keys`, `login_avatar_keys` deprecated. Red
tests approved by Volker on 2026-11-09, with three deviations:

1. **`login_avatar_keys` stays, deprecated, beside the new `avatar_keys`**, so nothing
   breaks and Steps 152 and 147 can ship as 0.5.1 right after 0.5.0.
2. **Under `kupferdraht-tabs` the calling tab updates itself.** The leader answers
   the target's getters after the call (`returns: "$getters"` in `METHODS`), and the
   proxy takes them before it resolves. The own profile calls (`setDisplayName`,
   `setHandle`, `uploadAvatar`, `removeAvatar`) return them too, so the calling tab's
   getters follow its own changes. Left for later: the `account` message to the
   other tabs and the getters of `reopen` after a leader change. Each tab calls
   `refreshAccount()` for itself, at its start and focus.
3. **No test of a refresh racing a `set_display_name`:** with fixed mocks it cannot
   show; `account_gate` serialises request, row and swap, which reading proves.

**Commissioned 2026-11-09** (from the browser embedder's requirements
for a work-queue client, part 3, R10, measured there against `4bf2317`; the
embedder agreed to this shape). Taken with Volker on commissioning:

- reading, not events. `team.update` reaches only the team's admins and is not
  stored (observations §99), so it could never bring a member's renamed team,
  live or after an offline time. Kupferdraht dispatches neither `team.update`
  nor `user.update`, and reads on its own at no point: no stale flag, no timer,
  no read at resume. The embedder decides when, for instance at the start and
  on window focus; without a call the values stay those of the last login;
- `Client::refresh_account()`, in the browser `refreshAccount(): Promise<void>`
  in every tab. The embedder needs no signal: when the promise resolves, the
  getters are current. All or nothing: on any failure it rejects and the old
  values stay;
- this revises two earlier calls: Step 126 left the live `Account`
  deliberately untouched, and Step 143 put a live refresh out of scope;
- no migration: rows from before v0.4.0 (V33's `NULL`s), and a team name a login
  failed to fetch, are filled by the first call;
- no ADR (Volker): observations §99 and this file record the reasons;
- after Step 146, before Step 147.

## Problem

1. **Nothing can read the current values.** A login reads `GET /self` and
   `GET /teams/{tid}` and stores them in the `self_user` row; a resume rebuilds
   the `Account` from that row (`client/resume.rs`, `build_resume_account`).
   Neither request is reachable afterwards, and the browser surface hands out no
   Wire token an embedder could use itself (`serviceToken` is for other
   services). So a renamed team, a changed icon, and a name, handle or picture
   changed on another device stay unseen until the next full login, and a
   browser session resumes for weeks. A row from before v0.4.0 shows no team
   name at all.
2. **The `Account` is frozen.** `Client.account` is an `Arc<Account>`
   (`client/mod.rs`), and `teamId`, `teamName`, `displayName`, `handle` and
   `email` read it. Even the client's own `set_display_name` and `set_handle`
   reach only the row, not the getters of the running session.
3. **A getter cannot read the row.** The getters are synchronous
   (`#[wasm_bindgen(getter)]`, FFI `account()`) and the store is async, unlike
   `own_avatar_keys`, which is why the `Account` itself has to change.
4. **The tabs freeze the values again.** `GETTERS` are snapshotted once, when
   the leader returns the handle (`protocol.js`, `leader.js` `wrap`,
   `proxy.js`); a re-read in a tab answers the value of that moment, and
   `reopen` after a leader change takes the new handle but not its getters.
5. **Two doc comments promise what does not exist.** `set_display_name` and
   `update_email` (`client/profile.rs`) say `sync_metadata` refreshes the
   account. It refreshes the room list only.

## Scope

1. **`Client::refresh_account() -> Result<(), KupferdrahtError>`.**
   - `GET /self`, then `GET /teams/{tid}` when `/self` names a team (lowercase;
     an empty `team` is none, as at login). Both answers are in hand before
     anything is written.
   - Any failure returns its error, row and account untouched: a transport
     failure, a `4xx`, a `/self` whose `qualified_id` is not this account's.
     That is data a backend can get wrong, so a recoverable error, no assertion.
   - A `/self` without a team clears the three team fields.
   - The mapping is the login's: `AvatarKeys::from_assets`, and one shared rule
     for the icon (`"default"` and empty are none).
   - A changed name, handle or avatar marks the room list stale, so the own
     `members()` row (`apply_self_profile`) follows at the next pass, and Step
     146 counts it.
2. **`fetch_own_team` splits.** It turns a failure into `name: None`
   (`client/login_pipeline.rs`), right for a login that must go on, wrong here:
   a short network failure would erase a stored name. A strict fetch that
   returns the error, and the login's lenient wrapper around it.
3. **A live account.** `Client.account` becomes `RwLock<Arc<Account>>`, poison
   recovered like `avatar_cache` (`PoisonError::into_inner`). `Client::account()`
   keeps its signature and answers the current one; the direct readers
   (`client/membership.rs`, `client/mentions.rs`, `client/sync.rs`) go through
   it. An `Arc` a caller holds stays a snapshot. The row stays the durable truth.
4. **One write path** (`client/own_account.rs`). The row is written whole, so a
   second writer could lose an update. An `account_gate`
   (`tokio::sync::Mutex<()>`, like `mls_write_gate`) spans request, row write and
   swap in `refresh_account`, `set_display_name`, `set_handle`, and the profile
   request of `upload_avatar` / `remove_avatar` (the image uploads stay outside).
   Order: request, row, account; a failed row write leaves the account. A session
   without a store swaps only. So the getters now follow the client's own calls
   too. `update_email` stays as it is: the address changes when the user confirms
   it, and the next `refresh_account` finds it.
5. **`Account::login_avatar_keys` becomes `avatar_keys`:** the old name would
   lie. `team_avatar` reads the live account, so `teamAvatar()` follows; the
   cache is keyed by asset key, so a new icon is a new entry.
6. **The browser.** `#[wasm_bindgen(js_name = refreshAccount)]`; the comments of
   `handle`, `teamId`, `teamName`, `displayName` and `teamAvatar` lose "as of the
   login" and "does not change it".
7. **The tabs.**
   - `METHODS` gains `["Client", "refreshAccount", "idempotent", null]`; `proxy.js`,
     `index.d.ts` and the README follow, `step_94_tab_proxy_parity` holds them.
   - After a success the leader reads `displayName`, `email`, `handle`, `teamId`
     and `teamName`, returns them in the response and posts
     `{ kind: 'account', v, epoch, userId, getters }` on the request channel. A
     message kind of its own, not a `LEADER_EVENTS` entry: the embedder asked for
     no signal, and none reaches it.
   - The response carries the values because `BroadcastChannel` orders nothing
     between channels, and the promise must not resolve before the getters are
     right: `ClientProxy.refreshAccount` applies them, then resolves `undefined`.
   - The transport knows its `ClientProxy`s by `userId` (today only `startSync`
     subscriptions see a broadcast) and applies a message to all of them. A
     message of an older leader is ignored (Step 116): unlike an event, whose data
     was stored, it would move a getter back.
   - `#resolveTarget` also takes `fresh.getters` from `reopen`, so a leader change
     cannot leave a tab on old values.
   - `PROTOCOL_VERSION` stays 1, as for every method added since Step 94.
8. **FFI.** `ffi::Client::refresh_account`; the `Account` record already reads
   the current value through `account()`.
9. **CLI.** `account-refresh`: resume, refresh, print name, handle, email, team id
   and team name.
10. **Docs:** `docs/library.md` (the `Account` section's "a snapshot of the last
    login", `refresh_account`, `avatar_keys`); `docs/bindings.md`; the tabs README;
    `docs/cli.md`; `docs/api-mapping-matrix-rust-sdk.md` (`refresh_account`, Drift,
    nearest `Account::fetch_user_profile`; `avatar_keys`);
    `docs/wire/api-notes.md` (`GET /teams/{tid}` is no longer login-only);
    `docs/backend-spec.md` §5 and §12.3 (what a client reads on request, and that it
    does not need `team.update`); `docs/ARCHITECTURE.md` (the `Account`, the row);
    `docs/non-goals.md` (Kupferdraht does not follow account changes by itself);
    the doc comments of `client/account.rs` and `client/profile.rs`; `CHANGELOG.md`
    `[Unreleased]` (Added; `BREAKING:` for `avatar_keys`; Changed for getters that
    now follow the client's own calls); `README.md`.

## Out of scope

- Dispatching `team.update` or `user.update`, a timer, a read at resume or after a
  lost cursor. The embedder's next call covers all of them.
- A return value saying whether anything changed: the embedder compares the
  getters, and agreed to `void`.
- `teamAvatarKey`: `teamAvatar()` answers from the cache while the key is the same.
  Only if the embedder asks.
- Another process's refresh in this process's live account (native, ADR-0024): the
  row is shared and read before every write, the getters follow at this process's
  next `refresh_account` or resume.
- A user who joins or leaves a team beyond what `/self` says; the team's members.

## TDD sketch

- Native, httpmock:
  - a call replaces name, handle, email, team id, team name, icon key and avatar
    keys, in the row and in `account()`; a resume after it has the same values;
  - partial failures, one test each: `/self` fails; `/teams` fails (the stored name
    survives); a `/self` of another user. Row and account untouched each time;
  - a personal account makes no team request; a `/self` without a team clears the
    team; `"default"` and an empty icon are none;
  - a V33 row with `NULL`s is filled by a call, no migration;
  - `set_display_name`, `set_handle`, `upload_avatar`, `remove_avatar`: `account()`
    follows, and a failed request or row write leaves it;
  - a `refresh_account` racing a `set_display_name` loses neither write;
  - a changed name marks the room list stale, an unchanged one does not;
  - a login whose team request fails still goes on with `name: None`.
- Tabs (browser suite):
  - the promise resolves with the calling tab's getters current;
  - a second tab's `ClientProxy` follows, with and without `startSync`;
  - a rejected call moves nothing; a late message of an older leader moves nothing;
  - a leader change takes the new getters; parity sees `refreshAccount`.
- Gate: the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown`, the `ci.yml` browser test targets and
  `./scripts/wasm-test.sh`.
- Smoke (wire-server-rs stack, CLI): `profile-set` and a handle change from a
  second device, `account-refresh` on the first shows them, the second `sync` shows
  the own member row; a personal account; a stopped backend leaves the values and
  exits with the error.
- The team rename: the local stack has no public `PUT /teams/{tid}` and sends no
  `team.update` (`docs/wire/api-notes.md`, observations §99), so against the team's
  dev backend, with Volker: rename, `account-refresh`, a new `teamName` for an
  ordinary member. The same run measures §99 live: who receives `team.update`.
